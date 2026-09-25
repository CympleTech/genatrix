//! Forward-only migrations, one SQL file per version, applied in order at
//! open. The version lives in `PRAGMA user_version`.

use rusqlite::Connection;

use crate::error::{Error, Result};

/// Highest migration this build knows.
pub const SCHEMA_VERSION: i64 = 10;

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_init.sql")),
    (2, include_str!("../migrations/0002_sync_cursor.sql")),
    (3, include_str!("../migrations/0003_chunk_vec.sql")),
    (4, include_str!("../migrations/0004_commitment_digest.sql")),
    (5, include_str!("../migrations/0005_relationship.sql")),
    (6, include_str!("../migrations/0006_action.sql")),
    (7, include_str!("../migrations/0007_device.sql")),
    (8, include_str!("../migrations/0008_read_indexes.sql")),
    (9, include_str!("../migrations/0009_one_to_one.sql")),
    (10, include_str!("../migrations/0010_agents.sql")),
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
