//! The core's side of keeping accounts up to date.
//!
//! Design: `docs/design/05-connectors.md`, "幂等与检查点" and "账号状态".
//!
//! The connector decides what to fetch and when; the core decides what a
//! fetched message means and where the cursors live. [`StoreSink`] is that
//! second half: ingestion into the store, and cursors in the store's
//! `sync_cursor` table as JSON the store does not read. [`Accounts`] is
//! where the interface looks to see how each account is doing.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use genatrix_connector::{Cursor, Fault, SyncState};
use genatrix_connector_imap::{Incoming, Sink};
use genatrix_model::Connector;
use serde::Serialize;
use tokio::sync::watch;

use crate::ingest::{self, Ingested};
use crate::system::System;

/// Where the mail connector puts what it fetched.
#[derive(Clone)]
pub struct StoreSink {
    system: Arc<System>,
    connector: Connector,
    account: String,
}

impl StoreSink {
    /// The sink for one mail account.
    pub fn new(system: Arc<System>, account: impl Into<String>) -> Self {
        Self::for_connector(system, Connector::Imap, account)
    }

    /// The sink for one account of any connector.
    pub fn for_connector(
        system: Arc<System>,
        connector: Connector,
        account: impl Into<String>,
    ) -> Self {
        Self {
            system,
            connector,
            account: account.into(),
        }
    }

    /// Store a batch of chat messages. Returns how many were new.
    pub fn store_chats(
        &self,
        batch: &[genatrix_connector::protocol::ChatMessage],
    ) -> Result<usize, Fault> {
        let mut new = 0;
        for chat in batch {
            match ingest::chat(
                &self.system.store,
                &self.system.raw_files,
                &self.account,
                chat,
            ) {
                Ok(Ingested::Added(_)) => new += 1,
                Ok(Ingested::AlreadyHad) => {}
                Err(e) => return Err(self.fault("could not store a message", e)),
            }
        }
        Ok(new)
    }

    /// A mail the account sent, arriving by sync, may be the one an action
    /// with an unknown outcome was submitted as. Then it did go out.
    fn maybe_confirm_sent(&self, incoming: &Incoming, id: genatrix_model::ItemId) {
        let Some(message_id) = &incoming.mail.message_id else {
            return;
        };
        let from_this_account = incoming
            .mail
            .from
            .as_ref()
            .is_some_and(|f| f.address.eq_ignore_ascii_case(&self.account));
        if !from_this_account {
            return;
        }
        match self.system.actions.confirm_sent(
            &self.system.store,
            &self.system.ledger,
            &self.account,
            message_id,
            id,
        ) {
            Ok(true) => tracing::info!(item = %id, "an unknown send was confirmed by sync"),
            Ok(false) => {}
            Err(e) => tracing::warn!(error = %e, "could not check the sent mail against actions"),
        }
    }

    /// A store failure, as the connector sees it: transient, because the
    /// likely causes, a full disk or a locked file, pass, and the backoff
    /// keeps the account from being hammered meanwhile.
    fn fault(&self, what: &str, error: impl std::fmt::Display) -> Fault {
        Fault::transient(&self.account, format!("{what}: {error}"))
    }
}

impl StoreSink {
    /// The cursor as stored, for the socket, which carries it as is.
    pub fn load_json(&self, scope: &str) -> Result<Option<String>, Fault> {
        Ok(self
            .system
            .store
            .get_sync_cursor(self.connector, &self.account, scope)
            .map_err(|e| self.fault("could not read the sync cursor", e))?
            .map(|stored| stored.cursor))
    }

    /// A cursor from the socket, stored as is.
    pub fn save_json(&self, scope: &str, cursor_json: &str) -> Result<(), Fault> {
        self.system
            .store
            .put_sync_cursor(self.connector, &self.account, scope, cursor_json)
            .map_err(|e| self.fault("could not write the sync cursor", e))
    }
}

impl Sink for StoreSink {
    async fn store(&self, batch: &[Incoming]) -> Result<usize, Fault> {
        let mut new = 0;
        for incoming in batch {
            match ingest::mail(
                &self.system.store,
                &self.system.raw_files,
                &self.system.blob_files,
                incoming,
            ) {
                Ok(Ingested::Added(id)) => {
                    new += 1;
                    self.maybe_confirm_sent(incoming, id);
                }
                Ok(Ingested::AlreadyHad) => {}
                Err(e) => return Err(self.fault("could not store a message", e)),
            }
        }
        Ok(new)
    }

    async fn load(&self, scope: &str) -> Result<Option<Cursor>, Fault> {
        let Some(json) = self.load_json(scope)? else {
            return Ok(None);
        };
        match serde_json::from_str(&json) {
            Ok(cursor) => Ok(Some(cursor)),
            Err(e) => {
                // A cursor this build cannot read is a cursor from another
                // build. Starting the scope over refetches and loses
                // nothing, which is the conservative answer design 05 asks
                // for.
                tracing::warn!(
                    account = %self.account,
                    scope,
                    error = %e,
                    "unreadable sync cursor; starting the scope over"
                );
                Ok(None)
            }
        }
    }

    async fn save(&self, scope: &str, cursor: &Cursor) -> Result<(), Fault> {
        let json = serde_json::to_string(cursor)
            .map_err(|e| self.fault("could not encode the sync cursor", e))?;
        self.save_json(scope, &json)
    }
}

/// One account's state as the interface sees it.
#[derive(Clone, Debug, Serialize)]
pub struct AccountState {
    /// The address.
    pub address: String,
    /// What it is doing.
    pub sync: SyncState,
    /// The same, in one line.
    pub text: String,
}

/// Every account's current state.
#[derive(Clone, Default)]
pub struct Accounts {
    states: Arc<Mutex<BTreeMap<String, watch::Receiver<SyncState>>>>,
}

impl Accounts {
    /// Start tracking an account. The connector reports through the sender.
    pub fn track(&self, address: &str) -> watch::Sender<SyncState> {
        let (tx, rx) = watch::channel(SyncState::Starting);
        self.states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(address.to_owned(), rx);
        tx
    }

    /// Stop tracking every account, before the connectors start again from
    /// the account list as it now is.
    pub fn clear(&self) {
        self.states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Where every account is right now.
    #[must_use]
    pub fn snapshot(&self) -> Vec<AccountState> {
        self.states
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(address, rx)| {
                let sync = rx.borrow().clone();
                AccountState {
                    address: address.clone(),
                    text: sync.describe(),
                    sync,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_follows_what_each_connector_last_said() {
        let accounts = Accounts::default();
        let a = accounts.track("a@example.com");
        let b = accounts.track("b@example.com");
        a.send_replace(SyncState::NeedsLogin {
            detail: "refused".into(),
        });
        b.send_replace(SyncState::Connecting);

        let snapshot = accounts.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].address, "a@example.com");
        assert!(matches!(snapshot[0].sync, SyncState::NeedsLogin { .. }));
        assert_eq!(snapshot[0].text, "needs a new login: refused");
        assert_eq!(snapshot[1].text, "connecting");
    }
}
