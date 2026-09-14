//! The chain head, kept in a small file beside the database so that a
//! rewritten database can be noticed. Format: one line, `<seq> <hex hash>`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::ledger::Hash;

pub(crate) struct HeadFile {
    path: PathBuf,
}

impl HeadFile {
    pub(crate) fn beside(db: &Path) -> Self {
        let mut p = db.as_os_str().to_owned();
        p.push(".head");
        Self { path: p.into() }
    }

    pub(crate) fn read(&self) -> Result<Option<(i64, Hash)>> {
        let text = match fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut parts = text.split_whitespace();
        let (Some(seq), Some(hex_hash)) = (parts.next(), parts.next()) else {
            return Err(Error::HeadMismatch("head file is malformed".into()));
        };
        let seq: i64 = seq
            .parse()
            .map_err(|_| Error::HeadMismatch("head file seq is not a number".into()))?;
        let mut hash = [0u8; 32];
        hex::decode_to_slice(hex_hash, &mut hash)
            .map_err(|_| Error::HeadMismatch("head file hash is not 32 hex bytes".into()))?;
        Ok(Some((seq, hash)))
    }

    /// Write atomically: temp file then rename.
    pub(crate) fn write(&self, seq: i64, hash: &Hash) -> Result<()> {
        let tmp = self.path.with_extension("head.tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            writeln!(f, "{seq} {}", hex::encode(hash))?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}
