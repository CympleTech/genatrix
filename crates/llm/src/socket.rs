//! Checking that a Unix socket path can actually be used.
//!
//! Two things go wrong here often enough to be worth catching early rather
//! than at bind time, where the operating system reports them as a bare
//! `Permission denied` with no mention of which path it meant:
//!
//! - the path is longer than macOS allows, which is a surprisingly short 104
//!   bytes including the terminator;
//! - the directory cannot be created, usually because a configuration
//!   template was copied without replacing a placeholder, and the result
//!   points somewhere nobody may write.
//!
//! A configuration that cannot work should fail the check, not start and
//! then die.

use std::path::{Path, PathBuf};

/// The `sun_path` limit on macOS, terminator included.
pub const MAX_SOCKET_PATH: usize = 104;

/// Why a socket path is unusable.
#[derive(Debug, thiserror::Error)]
pub enum SocketPathError {
    /// A relative path would resolve differently depending on who started
    /// the process.
    #[error("socket path must be absolute: {path}")]
    NotAbsolute {
        /// The path.
        path: PathBuf,
    },
    /// Longer than the platform allows.
    #[error("socket path is {len} bytes, over the {MAX_SOCKET_PATH} byte limit: {path}")]
    TooLong {
        /// The path.
        path: PathBuf,
        /// Its length in bytes.
        len: usize,
    },
    /// The directory it would live in cannot be made.
    #[error(
        "cannot create the directory for {path}: {source}. \
         The nearest directory that exists is {existing}; check for a \
         placeholder left in the configuration."
    )]
    Undirectoried {
        /// The socket path.
        path: PathBuf,
        /// The closest ancestor that does exist.
        existing: PathBuf,
        /// What the operating system said.
        source: std::io::Error,
    },
}

/// Check that a socket path is absolute, short enough, and in a directory we
/// can create. Creates the directory as a side effect when it can, which is
/// what binding would do anyway.
pub fn check(path: &Path) -> Result<(), SocketPathError> {
    if !path.is_absolute() {
        return Err(SocketPathError::NotAbsolute {
            path: path.to_path_buf(),
        });
    }
    let len = path.as_os_str().len();
    if len >= MAX_SOCKET_PATH {
        return Err(SocketPathError::TooLong {
            path: path.to_path_buf(),
            len,
        });
    }
    let Some(dir) = path.parent() else {
        return Ok(());
    };
    if let Err(source) = std::fs::create_dir_all(dir) {
        return Err(SocketPathError::Undirectoried {
            path: path.to_path_buf(),
            existing: nearest_existing(dir),
            source,
        });
    }
    Ok(())
}

/// The closest ancestor of `dir` that exists, for an error message that
/// points at where the path stops making sense.
fn nearest_existing(dir: &Path) -> PathBuf {
    let mut current = dir;
    loop {
        if current.exists() {
            return current.to_path_buf();
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return PathBuf::from("/"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_usable_path_passes_and_its_directory_appears() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run").join("gateway.sock");
        check(&path).unwrap();
        assert!(path.parent().unwrap().is_dir());
    }

    #[test]
    fn a_relative_path_is_refused() {
        let err = check(Path::new("run/gateway.sock")).unwrap_err();
        assert!(matches!(err, SocketPathError::NotAbsolute { .. }), "{err}");
    }

    #[test]
    fn a_path_over_the_platform_limit_is_refused() {
        let long = format!("/tmp/{}/gateway.sock", "d".repeat(120));
        let err = check(Path::new(&long)).unwrap_err();
        match err {
            SocketPathError::TooLong { len, .. } => assert!(len >= MAX_SOCKET_PATH),
            other => panic!("{other}"),
        }
    }

    #[test]
    fn an_uncopied_placeholder_is_caught_with_a_useful_message() {
        // The real bug this exists for: a template that says /Users/you/ and
        // was pasted as-is. /Users is not writable, so the directory cannot
        // be made, and binding would report only "Permission denied".
        let err = check(Path::new("/Users/you/.genatrix-dev/run/gateway.sock")).unwrap_err();
        let message = err.to_string();
        assert!(
            matches!(err, SocketPathError::Undirectoried { .. }),
            "{message}"
        );
        assert!(message.contains("/Users/you"), "{message}");
        assert!(message.contains("placeholder"), "{message}");
    }
}
