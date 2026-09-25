//! A functional agent's space: its own encrypted `SQLite` file, with its own
//! key (design 08, 11). The agent runs SQL here and nowhere else.
//!
//! What keeps it in: a separate file, so another agent's data is not a
//! query condition away but a different file with a different key; no
//! database may be attached, by limit and by the authorizer; no pragma, no
//! virtual table, no extension; one statement per call; a page ceiling for
//! the quota; and a deadline checked while a statement runs, because time
//! spent in here is not counted by the sandbox's fuel.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, OpenFlags};

use crate::error::{Error, Result};
use genatrix_keys::DbKey;

/// A value in a space, as an agent sees it.
#[derive(Clone, Debug, PartialEq)]
pub enum SpaceValue {
    /// SQL NULL.
    Null,
    /// A 64-bit integer.
    Integer(i64),
    /// A float.
    Real(f64),
    /// Text.
    Text(String),
    /// Bytes.
    Bytes(Vec<u8>),
}

/// The most rows one query returns.
const MAX_ROWS: usize = 10_000;
/// The most bytes one query returns, summed over its values.
const MAX_RESULT_BYTES: usize = 32 << 20;
/// The longest statement accepted.
const MAX_SQL: i32 = 100_000;
/// The largest single value.
const MAX_VALUE: i32 = 16 << 20;
/// Page size fixed so the quota is exact.
const PAGE_SIZE: u32 = 4096;

/// Functions an agent may not call even if the build has them.
const FORBIDDEN_FUNCTIONS: [&str; 3] = ["load_extension", "sqlcipher_export", "fts3_tokenizer"];

/// One agent's space.
pub struct Space {
    conn: Mutex<Connection>,
    deadline: Arc<Mutex<Option<Instant>>>,
}

impl std::fmt::Debug for Space {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Space")
    }
}

impl Space {
    /// Open or create a space at `path`, keyed with `key`, holding at most
    /// `quota_mb` megabytes.
    pub fn open(path: impl AsRef<Path>, key: &DbKey, quota_mb: u32) -> Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        Self::init(Connection::open_with_flags(path, flags)?, key, quota_mb)
    }

    /// A space in memory, for tests and dry runs.
    pub fn open_in_memory(key: &DbKey, quota_mb: u32) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, key, quota_mb)
    }

    fn init(conn: Connection, key: &DbKey, quota_mb: u32) -> Result<Self> {
        conn.execute_batch(&format!("PRAGMA key = {};", key.pragma_literal()))?;
        if conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| {
                r.get::<_, i64>(0)
            })
            .is_err()
        {
            return Err(Error::WrongKey);
        }
        let pages = u64::from(quota_mb.max(1)) * (1 << 20) / u64::from(PAGE_SIZE);
        conn.execute_batch(&format!(
            "PRAGMA page_size = {PAGE_SIZE};
             PRAGMA temp_store = MEMORY;
             PRAGMA trusted_schema = OFF;
             PRAGMA max_page_count = {pages};"
        ))?;
        conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
        conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
        conn.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_SQL)?;
        conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE)?;

        let deadline: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let watched = Arc::clone(&deadline);
        conn.progress_handler(
            10_000,
            Some(move || {
                watched
                    .lock()
                    .ok()
                    .and_then(|d| *d)
                    .is_some_and(|d| Instant::now() >= d)
            }),
        )?;
        // Everything after this is the agent's, so the authorizer goes last.
        conn.authorizer(Some(authorize))?;
        Ok(Self {
            conn: Mutex::new(conn),
            deadline,
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Stop any statement still running at `deadline`. `None` lifts it.
    pub fn set_deadline(&self, deadline: Option<Instant>) {
        *self
            .deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = deadline;
    }

    /// Run one statement. Returns the rows it changed.
    pub fn execute(&self, sql: &str, params: &[SpaceValue]) -> std::result::Result<u64, String> {
        let conn = self.conn();
        let mut stmt = conn.prepare(sql).map_err(explain)?;
        let values: Vec<Value> = params.iter().map(to_sql).collect();
        let n = stmt
            .execute(rusqlite::params_from_iter(values))
            .map_err(explain)?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Run one query and return its rows.
    pub fn query(
        &self,
        sql: &str,
        params: &[SpaceValue],
    ) -> std::result::Result<Vec<Vec<SpaceValue>>, String> {
        let conn = self.conn();
        let mut stmt = conn.prepare(sql).map_err(explain)?;
        let width = stmt.column_count();
        let values: Vec<Value> = params.iter().map(to_sql).collect();
        let mut rows = stmt
            .query(rusqlite::params_from_iter(values))
            .map_err(explain)?;
        let (mut out, mut bytes) = (Vec::new(), 0usize);
        while let Some(row) = rows.next().map_err(explain)? {
            if out.len() == MAX_ROWS {
                return Err(format!("more than {MAX_ROWS} rows; add a LIMIT"));
            }
            let mut cells = Vec::with_capacity(width);
            for i in 0..width {
                let v = from_sql(row.get_ref(i).map_err(explain)?);
                bytes += match &v {
                    SpaceValue::Text(s) => s.len(),
                    SpaceValue::Bytes(b) => b.len(),
                    _ => 8,
                };
                cells.push(v);
            }
            if bytes > MAX_RESULT_BYTES {
                return Err("the result is too large".into());
            }
            out.push(cells);
        }
        Ok(out)
    }

    /// Begin an apply: everything until [`Space::end_apply`] lands whole or
    /// not at all.
    pub fn begin_apply(&self) -> std::result::Result<(), String> {
        self.conn()
            .execute_batch("SAVEPOINT genatrix_apply")
            .map_err(explain)
    }

    /// End an apply: keep its writes, or undo every one of them.
    pub fn end_apply(&self, keep: bool) {
        let conn = self.conn();
        let sql = if keep {
            "RELEASE genatrix_apply"
        } else {
            "ROLLBACK TO genatrix_apply; RELEASE genatrix_apply"
        };
        // An agent that committed on its own has already ended the
        // savepoint; there is nothing left to end.
        let _ = conn.execute_batch(sql);
    }

    /// End the run: roll back a transaction the agent left open.
    pub fn finish(&self) -> bool {
        let conn = self.conn();
        if conn.is_autocommit() {
            false
        } else {
            let _ = conn.execute_batch("ROLLBACK");
            true
        }
    }
}

fn authorize(ctx: AuthContext<'_>) -> Authorization {
    match ctx.action {
        AuthAction::Attach { .. }
        | AuthAction::Detach { .. }
        | AuthAction::Pragma { .. }
        | AuthAction::CreateVtable { .. }
        | AuthAction::DropVtable { .. } => Authorization::Deny,
        AuthAction::Function { function_name }
            if FORBIDDEN_FUNCTIONS
                .iter()
                .any(|f| f.eq_ignore_ascii_case(function_name)) =>
        {
            Authorization::Deny
        }
        _ => Authorization::Allow,
    }
}

fn explain(e: rusqlite::Error) -> String {
    match e {
        rusqlite::Error::MultipleStatement => "one statement at a time".into(),
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::DiskFull => {
            "the space is full".into()
        }
        rusqlite::Error::SqliteFailure(f, _)
            if f.code == rusqlite::ErrorCode::OperationInterrupted =>
        {
            "the run's time is up".into()
        }
        rusqlite::Error::SqliteFailure(f, _)
            if f.code == rusqlite::ErrorCode::AuthorizationForStatementDenied =>
        {
            "not allowed in a space".into()
        }
        other => other.to_string(),
    }
}

fn to_sql(v: &SpaceValue) -> Value {
    match v {
        SpaceValue::Null => Value::Null,
        SpaceValue::Integer(i) => Value::Integer(*i),
        SpaceValue::Real(f) => Value::Real(*f),
        SpaceValue::Text(s) => Value::Text(s.clone()),
        SpaceValue::Bytes(b) => Value::Blob(b.clone()),
    }
}

fn from_sql(v: ValueRef<'_>) -> SpaceValue {
    match v {
        ValueRef::Null => SpaceValue::Null,
        ValueRef::Integer(i) => SpaceValue::Integer(i),
        ValueRef::Real(f) => SpaceValue::Real(f),
        ValueRef::Text(t) => SpaceValue::Text(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => SpaceValue::Bytes(b.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(quota: u32) -> Space {
        Space::open_in_memory(&DbKey::from_bytes([4; 32]), quota).unwrap()
    }

    #[test]
    fn tables_rows_and_values_round_trip() {
        let s = space(10);
        s.execute("CREATE TABLE t (a INTEGER, b TEXT, c BLOB, d REAL)", &[])
            .unwrap();
        s.execute(
            "INSERT INTO t VALUES (?1, ?2, ?3, ?4)",
            &[
                SpaceValue::Integer(7),
                SpaceValue::Text("seven".into()),
                SpaceValue::Bytes(vec![7]),
                SpaceValue::Real(0.5),
            ],
        )
        .unwrap();
        let rows = s.query("SELECT * FROM t", &[]).unwrap();
        assert_eq!(
            rows,
            vec![vec![
                SpaceValue::Integer(7),
                SpaceValue::Text("seven".into()),
                SpaceValue::Bytes(vec![7]),
                SpaceValue::Real(0.5),
            ]]
        );
    }

    #[test]
    fn nothing_outside_the_file() {
        // Invariant 15: no other database can be reached from a space.
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("other.db");
        let s = space(10);
        let attach = format!("ATTACH DATABASE '{}' AS other", other.display());
        assert!(s.execute(&attach, &[]).is_err());
        assert!(!other.exists());
        for denied in [
            "PRAGMA key = 'x'",
            "PRAGMA max_page_count = 1000000",
            "PRAGMA writable_schema = ON",
            "CREATE VIRTUAL TABLE v USING fts5(x)",
        ] {
            assert!(s.execute(denied, &[]).is_err(), "{denied}");
        }
        assert!(s.query("SELECT load_extension('x')", &[]).is_err());
    }

    #[test]
    fn one_statement_per_call() {
        let s = space(10);
        let e = s
            .execute("CREATE TABLE a (x); CREATE TABLE b (x)", &[])
            .unwrap_err();
        assert_eq!(e, "one statement at a time");
    }

    #[test]
    fn the_quota_holds() {
        let s = space(1);
        s.execute("CREATE TABLE t (b BLOB)", &[]).unwrap();
        let mut full = None;
        for _ in 0..8 {
            if let Err(e) = s.execute("INSERT INTO t VALUES (zeroblob(262144))", &[]) {
                full = Some(e);
                break;
            }
        }
        assert_eq!(full.as_deref(), Some("the space is full"));
    }

    #[test]
    fn a_long_statement_stops_at_the_deadline() {
        let s = space(10);
        s.set_deadline(Some(Instant::now() + std::time::Duration::from_millis(50)));
        let e = s
            .query(
                "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n) SELECT count(*) FROM n",
                &[],
            )
            .unwrap_err();
        assert_eq!(e, "the run's time is up");
    }

    #[test]
    fn an_apply_lands_whole_or_not_at_all() {
        let s = space(10);
        s.execute("CREATE TABLE t (x)", &[]).unwrap();
        s.begin_apply().unwrap();
        s.execute("INSERT INTO t VALUES (1)", &[]).unwrap();
        s.end_apply(false);
        s.begin_apply().unwrap();
        s.execute("INSERT INTO t VALUES (2)", &[]).unwrap();
        s.end_apply(true);
        assert_eq!(
            s.query("SELECT x FROM t", &[]).unwrap(),
            vec![vec![SpaceValue::Integer(2)]]
        );
        assert!(!s.finish());
    }

    #[test]
    fn a_transaction_left_open_is_rolled_back() {
        let s = space(10);
        s.execute("CREATE TABLE t (x)", &[]).unwrap();
        s.execute("BEGIN", &[]).unwrap();
        s.execute("INSERT INTO t VALUES (1)", &[]).unwrap();
        assert!(s.finish());
        assert_eq!(
            s.query("SELECT count(*) FROM t", &[]).unwrap(),
            vec![vec![SpaceValue::Integer(0)]]
        );
    }

    #[test]
    fn another_key_cannot_open_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("space.db");
        {
            let s = Space::open(&path, &DbKey::from_bytes([1; 32]), 10).unwrap();
            s.execute("CREATE TABLE t (x)", &[]).unwrap();
        }
        let e = Space::open(&path, &DbKey::from_bytes([2; 32]), 10).unwrap_err();
        assert!(matches!(e, Error::WrongKey));
    }
}
