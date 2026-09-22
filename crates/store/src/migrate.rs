//! Forward-only migrations, one SQL file per version, applied in order at
//! open. The version lives in `PRAGMA user_version`.

use rusqlite::Connection;

use crate::error::{Error, Result};

/// Highest migration this build knows.
pub const SCHEMA_VERSION: i64 = 4;

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_init.sql")),
    (2, include_str!("../migrations/0002_sync_cursor.sql")),
    (3, include_str!("../migrations/0003_chunk_vec.sql")),
    (4, include_str!("../migrations/0004_commitment_digest.sql")),
];

pub(crate) fn current_version(conn: &Connection) -> Result<i64> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
}

/// Bring the database to [`SCHEMA_VERSION`]. Each step runs in its own
/// transaction; a failure leaves the file at the last completed version.
pub(crate) fn run(conn: &mut Connection) -> Result<()> {
    let found = current_version(conn)?;
    if found > SCHEMA_VERSION {
        return Err(Error::SchemaTooNew {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    for (version, sql) in MIGRATIONS {
        if *version <= found {
            continue;
        }
        tracing::info!(version, "applying migration");
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}
