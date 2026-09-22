//! Actions, kept whole.
//!
//! Design: `docs/design/03-agent-layer.md`, "动作". The action type lives in
//! the agent layer, above this crate, so the store keeps each action as the
//! JSON it is handed, beside the few columns that questions are asked by:
//! status, account, kind, time.

use rusqlite::{OptionalExtension, params};

use crate::error::Result;
use crate::store::Store;

/// One stored action: the columns, and the JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredAction {
    /// Identifier.
    pub id: String,
    /// Status name.
    pub status: String,
    /// The action, as JSON.
    pub value: String,
}

/// The columns an action is filed under.
#[derive(Clone, Debug)]
pub struct ActionColumns<'a> {
    /// Identifier.
    pub id: &'a str,
    /// The run that proposed it.
    pub run_id: &'a str,
    /// `send_mail`, `send_message`, ...
    pub kind: &'a str,
    /// The account it acts as, when it acts through a connector.
    pub account: Option<&'a str>,
    /// Status name.
    pub status: &'a str,
    /// RFC 3339.
    pub created_at: &'a str,
    /// RFC 3339.
    pub expires_at: &'a str,
}

impl Store {
    /// Write an action, new or changed.
    pub fn put_action(&self, columns: &ActionColumns<'_>, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO action (id, run_id, kind, account, status, created_at, expires_at, value)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (id) DO UPDATE SET status = excluded.status, value = excluded.value",
            params![
                columns.id,
                columns.run_id,
                columns.kind,
                columns.account,
                columns.status,
                columns.created_at,
                columns.expires_at,
                value,
            ],
        )?;
        Ok(())
    }

    /// One action.
    pub fn get_action(&self, id: &str) -> Result<Option<StoredAction>> {
        self.conn()
            .query_row(
                "SELECT id, status, value FROM action WHERE id = ?1",
                params![id],
                |r| {
                    Ok(StoredAction {
                        id: r.get(0)?,
                        status: r.get(1)?,
                        value: r.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Actions, newest first, optionally of one status.
    pub fn list_actions(&self, status: Option<&str>, limit: u32) -> Result<Vec<StoredAction>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, status, value FROM action
             WHERE (?1 IS NULL OR status = ?1)
             ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![status, limit], |r| {
            Ok(StoredAction {
                id: r.get(0)?,
                status: r.get(1)?,
                value: r.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<_, _>>()
            .map_err(Into::into)
    }

    /// Actions of one status for one account, oldest first: what a
    /// connector asks for.
    pub fn actions_for_account(&self, account: &str, status: &str) -> Result<Vec<StoredAction>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, status, value FROM action
             WHERE account = ?1 AND status = ?2 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![account, status], |r| {
            Ok(StoredAction {
                id: r.get(0)?,
                status: r.get(1)?,
                value: r.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<_, _>>()
            .map_err(Into::into)
    }

    /// How many actions are waiting.
    pub fn count_actions(&self, status: &str) -> Result<u64> {
        let n: i64 = self.conn().query_row(
            "SELECT count(*) FROM action WHERE status = ?1",
            params![status],
            |r| r.get(0),
        )?;
        Ok(u64::try_from(n).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DbKey;

    #[test]
    fn an_action_is_filed_found_and_updated() {
        let store = Store::open_in_memory(&DbKey::from_bytes([8; 32])).unwrap();
        let columns = ActionColumns {
            id: "a1",
            run_id: "r1",
            kind: "send_mail",
            account: Some("me@example.com"),
            status: "pending",
            created_at: "2026-09-22T00:00:00Z",
            expires_at: "2026-09-25T00:00:00Z",
        };
        store.put_action(&columns, "{\"v\":1}").unwrap();
        assert_eq!(store.count_actions("pending").unwrap(), 1);
        assert_eq!(store.list_actions(Some("pending"), 10).unwrap().len(), 1);
        assert!(
            store
                .actions_for_account("me@example.com", "approved")
                .unwrap()
                .is_empty()
        );
        let approved = ActionColumns {
            status: "approved",
            ..columns
        };
        store.put_action(&approved, "{\"v\":2}").unwrap();
        let found = store.get_action("a1").unwrap().unwrap();
        assert_eq!(found.status, "approved");
        assert_eq!(found.value, "{\"v\":2}");
        assert_eq!(
            store
                .actions_for_account("me@example.com", "approved")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(store.list_actions(None, 10).unwrap().len(), 1);
    }
}
