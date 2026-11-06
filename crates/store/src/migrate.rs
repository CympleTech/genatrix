//! Forward-only migrations, one SQL file per version, applied in order at
//! open. The version lives in `PRAGMA user_version`.

use rusqlite::Connection;

use crate::error::{Error, Result};

/// Highest migration this build knows.
pub const SCHEMA_VERSION: i64 = 19;

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
    (11, include_str!("../migrations/0011_agent_cursor.sql")),
    (
        12,
        include_str!("../migrations/0012_foreign_key_indexes.sql"),
    ),
    (13, include_str!("../migrations/0013_blob_text.sql")),
    (14, include_str!("../migrations/0014_junk.sql")),
    (15, include_str!("../migrations/0015_category.sql")),
    (16, include_str!("../migrations/0016_dispatch.sql")),
    (17, include_str!("../migrations/0017_dispatch_reply.sql")),
    (18, include_str!("../migrations/0018_workspace.sql")),
    (19, include_str!("../migrations/0019_admin.sql")),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Migration 18 builds the definition table again to admit workspaces;
    /// every definition on file stays, active or not.
    #[test]
    fn admitting_workspaces_keeps_every_definition() {
        let mut conn = Connection::open_in_memory().unwrap();
        for (version, sql) in MIGRATIONS.iter().filter(|(v, _)| *v <= 17) {
            conn.execute_batch(sql).unwrap();
            conn.pragma_update(None, "user_version", version).unwrap();
        }
        conn.execute_batch(
            "INSERT INTO definition (kind, name, hash, text, added_ms, active)
                 VALUES ('project', 'p', 'h1', 'old', 1, 0), ('project', 'p', 'h2', 'new', 2, 1);",
        )
        .unwrap();
        run(&mut conn).unwrap();
        let rows: i64 = conn
            .query_row(
                "SELECT count(*) FROM definition WHERE name = 'p'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 2);
        conn.execute(
            "INSERT INTO definition (kind, name, hash, text, added_ms) VALUES ('workspace', 'w', 'h3', 't', 3)",
            [],
        )
        .unwrap();
        assert!(
            conn.execute(
                "INSERT INTO definition (kind, name, hash, text, added_ms) VALUES ('other', 'x', 'h4', 't', 4)",
                [],
            )
            .is_err()
        );
    }

    /// Migration 17 builds the dispatch tables again to widen a CHECK. Foreign
    /// keys are on while it runs; it must keep every row, and leave the steps
    /// and events pointing at the new `dispatch`.
    #[test]
    fn rebuilding_the_dispatch_tables_keeps_every_row_and_every_reference() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for (version, sql) in MIGRATIONS.iter().filter(|(v, _)| *v <= 16) {
            conn.execute_batch(sql).unwrap();
            conn.pragma_update(None, "user_version", version).unwrap();
        }
        conn.execute_batch(
            "INSERT INTO dispatch (id, project, recipe, recipe_hash, state, level, branch, started_ms)
                 VALUES ('d1', 'p', 'implement', 'h', 'finished', 'public', 'genatrix/d1', 1);
             INSERT INTO dispatch_step (dispatch_id, seq, step, round, role, executor, state, started_ms)
                 VALUES ('d1', 1, 'implement', 1, 'backend', 'claude-code', 'done', 2);
             INSERT INTO dispatch_event (dispatch_id, seq, n, at_ms, kind, text)
                 VALUES ('d1', 1, 1, 3, 'message', 'hello');",
        )
        .unwrap();

        run(&mut conn).unwrap();
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);
        let count = |table: &str| -> i64 {
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(
            (
                count("dispatch"),
                count("dispatch_step"),
                count("dispatch_event")
            ),
            (1, 1, 1)
        );
        conn.execute("UPDATE dispatch SET state = 'waiting' WHERE id = 'd1'", [])
            .unwrap();
        let fk: Vec<String> = conn
            .prepare("SELECT \"table\" FROM pragma_foreign_key_list('dispatch_step')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(fk, ["dispatch"]);
        // Removing the dispatch still takes its steps and events with it.
        conn.execute("DELETE FROM dispatch WHERE id = 'd1'", [])
            .unwrap();
        assert_eq!((count("dispatch_step"), count("dispatch_event")), (0, 0));
    }
}
