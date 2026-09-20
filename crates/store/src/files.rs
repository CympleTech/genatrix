//! Content-addressed files on disk, each encrypted with its own key.
//!
//! Design: `docs/design/08-storage.md`, "逐文件加密".
//!
//! Raw records and attachments do not go in the database. An attachment can
//! be hundreds of megabytes, and a database carrying those is a database that
//! is slow to back up and slow to compact. They go in the file system instead,
//! named by the hash of their contents, which also means the same attachment
//! arriving in ten mails is stored once.
//!
//! Every file is encrypted with `XChaCha20-Poly1305` under a key derived from
//! the master key and that file's content hash. Authenticated: a file that has
//! been altered does not decrypt, it fails. The file name is the hash in
//! clear, which tells an onlooker whether a particular known file is present.
//! The design accepts that; the threat model has no adversary who would learn
//! anything useful from it.
//!
//! On disk:
//!
//! ```text
//! blobs/ab/cd/abcd...ef      one file, named by its content hash
//!   byte 0     format version
//!   bytes 1..25 nonce
//!   rest       ciphertext and tag
//! ```

use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use genatrix_keys::MasterKey;
use genatrix_model::ContentHash;
use rand::TryRngCore;

use crate::error::{Error, Result};

/// The format this build writes. Present in every file so a later scheme can
/// be told apart without guessing.
const VERSION: u8 = 1;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = 1 + NONCE_LEN;

/// A directory of encrypted, content-addressed files.
pub struct FileStore {
    root: PathBuf,
    master: MasterKey,
}

impl std::fmt::Debug for FileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileStore")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl FileStore {
    /// Open a store rooted at `root`, creating it if needed.
    pub fn open(root: impl Into<PathBuf>, master: MasterKey) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root, master })
    }

    /// Where a file with this hash lives.
    ///
    /// Two levels of two hex characters, so a store with a million files has
    /// a few hundred per directory rather than a million in one.
    #[must_use]
    pub fn path_of(&self, hash: &ContentHash) -> PathBuf {
        let hex = hash.to_string();
        self.root.join(&hex[0..2]).join(&hex[2..4]).join(&hex)
    }

    /// Whether these contents are already stored.
    #[must_use]
    pub fn has(&self, hash: &ContentHash) -> bool {
        self.path_of(hash).exists()
    }

    /// Store some bytes and return their hash.
    ///
    /// Storing the same bytes twice is a no-op: the file is already there,
    /// under the same name, and rewriting it would only risk a torn file for
    /// no gain.
    pub fn put(&self, bytes: &[u8]) -> Result<ContentHash> {
        let hash = ContentHash::of(bytes);
        let path = self.path_of(&hash);
        if path.exists() {
            return Ok(hash);
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }

        let mut nonce = [0u8; NONCE_LEN];
        rand::rngs::OsRng
            .try_fill_bytes(&mut nonce)
            .map_err(|e| Error::Io(std::io::Error::other(e)))?;

        let key = self.master.file_key(hash.as_bytes());
        let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
        // The header is authenticated but not encrypted, so a file whose
        // version byte was flipped fails to open rather than being read
        // under the wrong scheme.
        let header = [&[VERSION][..], &nonce[..]].concat();
        let sealed = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: bytes,
                    aad: &header,
                },
            )
            .map_err(|_| Error::Corrupt {
                table: "files",
                column: "contents",
                detail: "encryption failed".into(),
            })?;

        // Write beside the target and rename, so a file that exists is a file
        // that is complete.
        let temporary = path.with_extension("partial");
        std::fs::write(&temporary, [header, sealed].concat())?;
        std::fs::rename(&temporary, &path)?;
        Ok(hash)
    }

    /// Read the bytes back.
    ///
    /// Fails if the file is missing, truncated, written by a scheme this
    /// build does not know, or altered in any way.
    pub fn get(&self, hash: &ContentHash) -> Result<Vec<u8>> {
        let path = self.path_of(hash);
        let stored = std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::NotFound(format!("file {hash}"))
            } else {
                Error::Io(e)
            }
        })?;
        if stored.len() < HEADER_LEN {
            return Err(corrupt_file("file is shorter than its header"));
        }
        let (header, sealed) = stored.split_at(HEADER_LEN);
        if header[0] != VERSION {
            return Err(corrupt_file(&format!(
                "file format version {} is not one this build writes ({VERSION})",
                header[0]
            )));
        }
        let key = self.master.file_key(hash.as_bytes());
        let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
        let plain = cipher
            .decrypt(
                XNonce::from_slice(&header[1..]),
                Payload {
                    msg: sealed,
                    aad: header,
                },
            )
            .map_err(|_| {
                corrupt_file("file does not decrypt: wrong key, or it has been altered")
            })?;

        // The name is a promise about the contents. Check it, so a file moved
        // or renamed on disk cannot be served as something else.
        if ContentHash::of(&plain) != *hash {
            return Err(corrupt_file("file contents do not match the name"));
        }
        Ok(plain)
    }

    /// Forget a file. Used when the user deletes a source and its attachments
    /// go with it.
    pub fn remove(&self, hash: &ContentHash) -> Result<()> {
        match std::fs::remove_file(self.path_of(hash)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// Bytes on disk, for the interface's space report.
    pub fn size_on_disk(&self) -> Result<u64> {
        fn walk(dir: &Path) -> std::io::Result<u64> {
            let mut total = 0;
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    total += walk(&entry.path())?;
                } else {
                    total += entry.metadata()?.len();
                }
            }
            Ok(total)
        }
        Ok(walk(&self.root)?)
    }
}

fn corrupt_file(detail: &str) -> Error {
    Error::Corrupt {
        table: "files",
        column: "contents",
        detail: detail.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path, key: u8) -> FileStore {
        FileStore::open(dir, MasterKey::from_bytes([key; 32])).unwrap()
    }

    #[test]
    fn bytes_go_in_and_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let files = store(dir.path(), 1);
        let hash = files.put(b"an attachment").unwrap();
        assert!(files.has(&hash));
        assert_eq!(files.get(&hash).unwrap(), b"an attachment");
    }

    #[test]
    fn the_same_contents_are_stored_once() {
        let dir = tempfile::tempdir().unwrap();
        let files = store(dir.path(), 1);
        let a = files.put(b"the same file attached twice").unwrap();
        let before = files.size_on_disk().unwrap();
        let b = files.put(b"the same file attached twice").unwrap();
        assert_eq!(a, b);
        assert_eq!(files.size_on_disk().unwrap(), before);
    }

    #[test]
    fn nothing_readable_is_left_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let files = store(dir.path(), 1);
        let hash = files.put(b"the verification code is 482913").unwrap();
        let raw = std::fs::read(files.path_of(&hash)).unwrap();
        assert!(
            !raw.windows(6).any(|w| w == b"482913"),
            "the plaintext is on disk"
        );
        assert_eq!(raw[0], VERSION);
    }

    #[test]
    fn another_key_cannot_read_it() {
        let dir = tempfile::tempdir().unwrap();
        let hash = store(dir.path(), 1).put(b"private").unwrap();
        let err = store(dir.path(), 2).get(&hash).unwrap_err();
        assert!(matches!(err, Error::Corrupt { .. }), "{err}");
    }

    #[test]
    fn an_altered_file_fails_rather_than_returning_something_else() {
        let dir = tempfile::tempdir().unwrap();
        let files = store(dir.path(), 1);
        let hash = files.put(b"the original contents").unwrap();
        let path = files.path_of(&hash);

        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        std::fs::write(&path, &bytes).unwrap();
        assert!(files.get(&hash).is_err(), "a flipped bit must be noticed");

        // And the header is covered too, not just the body.
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[1] ^= 0x01;
        std::fs::write(&path, &bytes).unwrap();
        assert!(files.get(&hash).is_err());
    }

    #[test]
    fn a_file_moved_under_another_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let files = store(dir.path(), 1);
        let real = files.put(b"contents A").unwrap();
        let other = files.put(b"contents B").unwrap();
        // Put A's bytes where B belongs.
        std::fs::copy(files.path_of(&real), files.path_of(&other)).unwrap();
        assert!(
            files.get(&other).is_err(),
            "the name is a promise about the contents"
        );
    }

    #[test]
    fn an_unknown_format_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let files = store(dir.path(), 1);
        let hash = files.put(b"contents").unwrap();
        let path = files.path_of(&hash);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] = 99;
        std::fs::write(&path, bytes).unwrap();
        let err = files.get(&hash).unwrap_err();
        assert!(err.to_string().contains("99"), "{err}");
    }

    #[test]
    fn a_missing_file_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let files = store(dir.path(), 1);
        let hash = ContentHash::of(b"never stored");
        assert!(!files.has(&hash));
        assert!(matches!(files.get(&hash), Err(Error::NotFound(_))));
        files.remove(&hash).expect("removing nothing is fine");
    }

    #[test]
    fn files_are_spread_over_directories() {
        let dir = tempfile::tempdir().unwrap();
        let files = store(dir.path(), 1);
        let hash = files.put(b"x").unwrap();
        let hex = hash.to_string();
        let path = files.path_of(&hash);
        assert!(path.ends_with(&hex));
        assert_eq!(
            path.parent()
                .unwrap()
                .parent()
                .unwrap()
                .file_name()
                .unwrap(),
            &hex[0..2]
        );
    }

    #[test]
    fn a_partial_write_is_never_visible_as_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let files = store(dir.path(), 1);
        let hash = files.put(b"contents").unwrap();
        // Nothing with the temporary extension survives a successful write.
        assert!(!files.path_of(&hash).with_extension("partial").exists());
    }
}
