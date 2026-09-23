//! Sync cursors: where each connector has got to.
//!
//! Design: `docs/design/05-connectors.md`, "幂等与检查点". The checkpoint
//! lives in the core and the connector updates it. The store keeps it keyed
//! by connector, account and scope, and treats the cursor itself as an opaque
//! string, because its shape is the connector's business and changes with it.

use chrono::{DateTime, Utc};
use genatrix_model::Connector;
use rusqlite::{OptionalExtension, params};

use crate::error::Result;
use crate::store::Store;
use crate::time::{utc_from_col, utc_to_col};

const T: &str = "sync_cursor";

/// One stored cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredCursor {
    /// Which subdivision of the account: a folder, a chat, or `""`.
    pub scope: String,
    /// The cursor, as the connector handed it over.
    pub cursor: String,
    /// When it was last written.
    pub updated_at: DateTime<Utc>,
}

impl Store {
    /// Write a cursor, replacing any earlier one for the same scope.
    pub fn put_sync_cursor(
        &self,
        connector: Connector,
        account: &str,
        scope: &str,
        cursor: &str,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT INTO sync_cursor (connector, account, scope, cursor, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (connector, account, scope)
             DO UPDATE SET cursor = excluded.cursor, updated_at = excluded.updated_at",
            params![
                connector.as_str(),
                account,
                scope,
                cursor,
                utc_to_col(Utc::now())
            ],
        )?;
        Ok(())
    }

    /// Delete one cursor. Returns whether there was one. Used for the one
    /// scope that holds a secret, a Telegram session, when its account is
    /// removed; the history cursors stay, so signing in again resumes.
    pub fn delete_sync_cursor(
        &self,
        connector: Connector,
        account: &str,
        scope: &str,
    ) -> Result<bool> {
        let n = self.conn().execute(
            "DELETE FROM sync_cursor WHERE connector = ?1 AND account = ?2 AND scope = ?3",
            params![connector.as_str(), account, scope],
        )?;
        Ok(n > 0)
    }

    /// Read one cursor.
    pub fn get_sync_cursor(
        &self,
        connector: Connector,
        account: &str,
        scope: &str,
    ) -> Result<Option<StoredCursor>> {
        let row = self
            .conn()
            .query_row(
                "SELECT scope, cursor, updated_at FROM sync_cursor
                 WHERE connector = ?1 AND account = ?2 AND scope = ?3",
                params![connector.as_str(), account, scope],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(scope, cursor, at)| {
            Ok(StoredCursor {
                scope,
                cursor,
                updated_at: utc_from_col(T, "updated_at", &at)?,
            })
        })
        .transpose()
    }

    /// Every cursor an account has.
    pub fn list_sync_cursors(
        &self,
        connector: Connector,
        account: &str,
    ) -> Result<Vec<StoredCursor>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT scope, cursor, updated_at FROM sync_cursor
             WHERE connector = ?1 AND account = ?2 ORDER BY scope",
        )?;
        let rows = stmt.query_map(params![connector.as_str(), account], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (scope, cursor, at) = row?;
            Ok(StoredCursor {
                scope,
                cursor,
                updated_at: utc_from_col(T, "updated_at", &at)?,
            })
        })
        .collect()
    }

    /// Forget an account's cursors, so its next sync starts from nothing.
    /// The items stay; idempotence makes the refetch harmless.
    pub fn clear_sync_cursors(&self, connector: Connector, account: &str) -> Result<usize> {
        Ok(self.conn().execute(
            "DELETE FROM sync_cursor WHERE connector = ?1 AND account = ?2",
            params![connector.as_str(), account],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DbKey;

    fn store() -> Store {
        Store::open_in_memory(&DbKey::from_bytes([3; 32])).unwrap()
    }

    #[test]
    fn a_cursor_is_written_read_back_and_replaced() {
        let s = store();
        assert!(
            s.get_sync_cursor(Connector::Imap, "a", "INBOX")
                .unwrap()
                .is_none()
        );
        s.put_sync_cursor(Connector::Imap, "a", "INBOX", "{\"highest\":5}")
            .unwrap();
        s.put_sync_cursor(Connector::Imap, "a", "INBOX", "{\"highest\":9}")
            .unwrap();
        let found = s
            .get_sync_cursor(Connector::Imap, "a", "INBOX")
            .unwrap()
            .unwrap();
        assert_eq!(found.cursor, "{\"highest\":9}");
        assert_eq!(found.scope, "INBOX");
    }

    #[test]
    fn cursors_are_kept_apart_by_account_and_connector() {
        let s = store();
        s.put_sync_cursor(Connector::Imap, "a", "INBOX", "1")
            .unwrap();
        s.put_sync_cursor(Connector::Imap, "b", "INBOX", "2")
            .unwrap();
        s.put_sync_cursor(Connector::Telegram, "a", "", "3")
            .unwrap();
        s.put_sync_cursor(Connector::Imap, "a", "Sent", "4")
            .unwrap();
        let a: Vec<String> = s
            .list_sync_cursors(Connector::Imap, "a")
            .unwrap()
            .into_iter()
            .map(|c| c.cursor)
            .collect();
        assert_eq!(a, vec!["1", "4"]);
        assert_eq!(s.clear_sync_cursors(Connector::Imap, "a").unwrap(), 2);
        assert_eq!(s.list_sync_cursors(Connector::Imap, "a").unwrap().len(), 0);
        assert_eq!(s.list_sync_cursors(Connector::Imap, "b").unwrap().len(), 1);
    }
}
