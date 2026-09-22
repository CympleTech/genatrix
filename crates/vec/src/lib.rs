//! Vector search inside `SQLite`: the `sqlite-vec` extension, registered.
//!
//! Design: `docs/design/08-storage.md`: no separate vector database; the
//! extension is enough at the scale of one person's life, and the index
//! lives in the same encrypted file as everything else.
//!
//! This crate exists because registering an `SQLite` extension is one call
//! into C, and the store forbids unsafe code outright. The one unsafe block
//! is here, alone, where it can be read in full.

/// Register `sqlite-vec` with every connection this process opens from now
/// on. Safe to call more than once; only the first call does anything.
pub fn register() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: `sqlite3_vec_init` has the `sqlite3_auto_extension`
        // entry-point signature; the extension's own tests register it the
        // same way. Registering an extension is a process-wide, idempotent
        // operation on `SQLite`'s side, and it happens here before any
        // connection is opened.
        #[allow(unsafe_code)]
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut std::os::raw::c_char,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> std::os::raw::c_int,
            >(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_extension_answers_once_registered() {
        super::register();
        super::register();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let version: String = conn
            .query_row("select vec_version()", [], |r| r.get(0))
            .unwrap();
        assert!(version.starts_with('v'), "{version}");
        conn.execute_batch("CREATE VIRTUAL TABLE t USING vec0(embedding float[3])")
            .unwrap();
        conn.execute(
            "INSERT INTO t(rowid, embedding) VALUES (1, '[1,0,0]'), (2, '[0,1,0]')",
            [],
        )
        .unwrap();
        let nearest: i64 = conn
            .query_row(
                "SELECT rowid FROM t WHERE embedding MATCH '[0.9,0.1,0]' AND k = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nearest, 1);
    }
}
