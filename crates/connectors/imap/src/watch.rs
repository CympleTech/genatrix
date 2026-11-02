//! Keeping an account up to date, for as long as the process runs.
//!
//! Design: `docs/design/05-connectors.md`, "回填", "实时同步" and "恢复".
//!
//! One connection per account, doing two jobs in turns. Each round first
//! takes whatever is new in every folder, then one batch of history from the
//! first folder that still has some. While history is coming in, new mail
//! therefore waits at most one batch; once it is all in, the connection
//! waits on the server instead, with IDLE where the server has it and a
//! timed poll where it does not, and takes what arrives.
//!
//! On first sight of a folder nothing is fetched. The live cursor is set at
//! the newest message and the history cursor at the top, so history is
//! walked newest first from the very first round, and no message is fetched
//! twice on the way.
//!
//! The engine never holds the truth about progress. Cursors are handed to
//! the core through [`Sink`] after every batch, after the batch itself has
//! been stored, so a crash between the two refetches a batch and loses
//! nothing. That order is the whole of the crash-safety story, and it is
//! the same order everywhere in this file.

use std::future::Future;
use std::time::Duration;

use chrono::Utc;
use genatrix_connector::backfill::Progress;
use genatrix_connector::capability::AccountCapability;
use genatrix_connector::{Checkpoint, Cursor, Fault, SyncState};
use tokio::sync::watch;

use crate::source::{Folder, MailSource};
use crate::sync::{Incoming, Sync};

/// How long to wait for the server before checking anyway. Design 05: poll
/// once a minute where there is no IDLE. With IDLE the wait usually ends
/// early, and the check on timeout is a cheap search for UIDs above the
/// cursor.
pub const WAIT: Duration = Duration::from_secs(60);

/// The scope a folder's history cursor is stored under, beside the folder's
/// own live cursor.
fn history_scope(folder: &str) -> String {
    format!("{folder}#history")
}

/// What the core offers the engine: somewhere to put mail, and the cursors.
///
/// In the finished layout this is the connector protocol over IPC (design
/// 05). In-process for now, and the shape is the same either way: the
/// connector decides what to fetch, the core decides what it means.
pub trait Sink: Send + std::marker::Sync {
    /// Store a batch. Returns how many were new; the rest were already held,
    /// which is the ordinary result of refetching.
    fn store(&self, batch: &[Incoming]) -> impl Future<Output = Result<usize, Fault>> + Send;

    /// The cursor last saved for a scope, if any.
    fn load(&self, scope: &str) -> impl Future<Output = Result<Option<Cursor>, Fault>> + Send;

    /// Save a cursor. Called after the batch it describes has been stored.
    fn save(&self, scope: &str, cursor: &Cursor) -> impl Future<Output = Result<(), Fault>> + Send;
}

impl<K: Sink> Sink for &K {
    async fn store(&self, batch: &[Incoming]) -> Result<usize, Fault> {
        (**self).store(batch).await
    }

    async fn load(&self, scope: &str) -> Result<Option<Cursor>, Fault> {
        (**self).load(scope).await
    }

    async fn save(&self, scope: &str, cursor: &Cursor) -> Result<(), Fault> {
        (**self).save(scope, cursor).await
    }
}

/// What one round did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Round {
    /// Messages stored that were not there before.
    pub new: usize,
    /// Whether every folder's history is in.
    pub complete: bool,
}

/// One account, kept up to date.
pub struct Watcher<S, K> {
    sync: Sync<S>,
    sink: K,
    account: String,
    status: watch::Sender<SyncState>,
    progress: Progress,
    /// The folder to wait on: the first one worth reading.
    primary: Option<String>,
}

impl<S: MailSource + Send + std::marker::Sync, K: Sink> Watcher<S, K> {
    /// Watch the account `sync` reads, storing through `sink`, reporting
    /// through `status`.
    pub fn new(sync: Sync<S>, sink: K, status: watch::Sender<SyncState>) -> Self {
        let account = sync.account().to_owned();
        Self {
            sync,
            sink,
            account,
            status,
            progress: Progress::default(),
            primary: None,
        }
    }

    /// How far the history has got.
    #[must_use]
    pub const fn progress(&self) -> &Progress {
        &self.progress
    }

    /// One round: what is new in every folder, then one batch of history.
    pub async fn round(&mut self) -> Result<Round, Fault> {
        let folders = self.sync.folders().await?;
        self.primary = folders.first().map(|f| f.name.clone());

        let mut new = 0;
        for folder in &folders {
            new += self.catch_up(folder).await?;
        }

        // Where each folder's history has got to, now that every folder has
        // a cursor.
        let mut histories = Vec::with_capacity(folders.len());
        for folder in &folders {
            histories.push(self.history_of(folder).await?);
        }

        let mut done: u64 = 0;
        let mut worked = false;
        for (folder, history) in folders.iter().zip(&histories) {
            if history.complete {
                done += u64::from(folder.count);
                continue;
            }
            if worked {
                continue;
            }
            worked = true;
            let (taken, remaining) = self.backfill_step(folder, history.oldest).await?;
            new += taken;
            done += u64::from(folder.count).saturating_sub(remaining as u64);
        }

        let complete = !worked || self.all_complete(&folders).await.unwrap_or(false);

        self.progress.total = Some(folders.iter().map(|f| u64::from(f.count)).sum());
        self.progress.done = done;
        self.progress.complete = complete;

        self.status.send_replace(if complete {
            SyncState::Live {
                synced_at: Utc::now(),
            }
        } else {
            SyncState::Backfilling {
                progress: self.progress.clone(),
            }
        });

        Ok(Round { new, complete })
    }

    /// Keep going until something stops it, and say what did.
    ///
    /// `attempt` is the caller's count of failures in a row; it is reset
    /// after every round that works, so an outage that has passed is
    /// forgotten rather than counted against the next one.
    pub async fn run(&mut self, attempt: &mut u32) -> Fault {
        loop {
            match self.round().await {
                Ok(round) => {
                    *attempt = 0;
                    if round.complete {
                        let Some(primary) = self.primary.clone() else {
                            // No folders at all. Nothing to wait on; check
                            // again after the poll interval.
                            tokio::time::sleep(WAIT).await;
                            continue;
                        };
                        if let Err(fault) = self.sync.wait_for_change(&primary, WAIT).await {
                            return fault;
                        }
                    }
                }
                Err(fault) => return fault,
            }
        }
    }

    /// Take what is new in one folder. On first sight, or after the server
    /// renumbered it, set the cursors up instead and fetch nothing.
    async fn catch_up(&mut self, folder: &Folder) -> Result<usize, Fault> {
        let scope = folder.name.clone();
        let known = match self.sink.load(&scope).await? {
            Some(cursor @ Cursor::ImapUid { uidvalidity, .. })
                if uidvalidity == folder.uidvalidity =>
            {
                Some(cursor)
            }
            _ => None,
        };

        let Some(cursor) = known else {
            let newest = self.sync.newest_uid(&folder.name).await?;
            self.sink
                .save(
                    &scope,
                    &Cursor::ImapUid {
                        uidvalidity: folder.uidvalidity,
                        highest: newest,
                    },
                )
                .await?;
            self.sink
                .save(
                    &history_scope(&folder.name),
                    &Cursor::ImapHistory {
                        uidvalidity: folder.uidvalidity,
                        oldest: None,
                        complete: newest == 0,
                    },
                )
                .await?;
            return Ok(0);
        };

        let mut checkpoint = Checkpoint::new(&self.account, &scope, cursor);
        let (mails, _) = self
            .sync
            .catch_up(&folder.name, folder.uidvalidity, &mut checkpoint)
            .await?;
        let new = self.sink.store(&mails).await?;
        self.sink.save(&scope, &checkpoint.cursor).await?;
        Ok(new)
    }

    /// One batch of a folder's history, newest first from where it left off.
    /// Returns how many were new and how many are still behind.
    async fn backfill_step(
        &mut self,
        folder: &Folder,
        before: Option<u32>,
    ) -> Result<(usize, usize), Fault> {
        let mut pass_progress = Progress::default();
        let (batch, oldest, pass) = self
            .sync
            .backfill(&folder.name, folder.uidvalidity, before, &mut pass_progress)
            .await?;
        let new = self.sink.store(&batch).await?;
        self.sink
            .save(
                &history_scope(&folder.name),
                &Cursor::ImapHistory {
                    uidvalidity: folder.uidvalidity,
                    oldest,
                    complete: !pass.more,
                },
            )
            .await?;
        if pass_progress.reached.is_some() {
            self.progress.reached = pass_progress.reached;
        }
        Ok((new, pass.remaining))
    }

    /// A folder's history position. Always present after `catch_up`, and
    /// always for the folder's current validity, because `catch_up` resets
    /// both cursors when the validity moves.
    async fn history_of(&self, folder: &Folder) -> Result<History, Fault> {
        match self.sink.load(&history_scope(&folder.name)).await? {
            Some(Cursor::ImapHistory {
                uidvalidity,
                oldest,
                complete,
            }) if uidvalidity == folder.uidvalidity => Ok(History { oldest, complete }),
            _ => Ok(History {
                oldest: None,
                complete: false,
            }),
        }
    }

    async fn all_complete(&self, folders: &[Folder]) -> Result<bool, Fault> {
        for folder in folders {
            if !self.history_of(folder).await?.complete {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[derive(Clone, Copy, Debug)]
struct History {
    oldest: Option<u32>,
    complete: bool,
}

/// Keep one account up to date across reconnections, until a fault only
/// the user can fix.
///
/// `connect` makes a fresh connection each time one is needed. A transient
/// fault is waited out with the backoff from design 05 and the account is
/// reconnected; the other two kinds end this, with the state left where the
/// interface can read it. Returns the fault that ended it.
pub async fn run_account<S, K, C, F>(
    capability: AccountCapability,
    connect: C,
    sink: K,
    status: watch::Sender<SyncState>,
) -> Fault
where
    S: MailSource + Send + std::marker::Sync,
    K: Sink + Clone,
    C: Fn() -> F,
    F: Future<Output = Result<S, Fault>>,
{
    let mut attempt: u32 = 0;
    loop {
        status.send_replace(SyncState::Connecting);
        let fault = match connect().await {
            Ok(source) => {
                let mut watcher = Watcher::new(
                    Sync::new(source, capability.clone()),
                    sink.clone(),
                    status.clone(),
                );
                watcher.run(&mut attempt).await
            }
            Err(fault) => fault,
        };

        if !fault.retryable() {
            tracing::warn!(account = %capability.account, %fault, "stopped");
            status.send_replace(SyncState::after(&fault, attempt));
            return fault;
        }
        attempt = attempt.saturating_add(1);
        tracing::info!(account = %capability.account, %fault, attempt, "retrying");
        status.send_replace(SyncState::after(&fault, attempt));
        tokio::time::sleep(Fault::backoff(attempt)).await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use genatrix_connector::capability::Host;
    use genatrix_model::Connector;

    use super::*;
    use crate::source::fake::{FakeFolder, FakeServer};

    fn capability() -> AccountCapability {
        AccountCapability::new(Connector::Imap, "me@example.com")
            .with_host(Host::new("imap.example.com", 993))
    }

    /// The core, reduced to a map: items keyed by identifier, cursors by
    /// scope. Dropping a repeated identifier is what `Source` uniqueness
    /// does in the real store.
    #[derive(Clone, Default)]
    struct Memory {
        items: Arc<Mutex<BTreeMap<String, String>>>,
        cursors: Arc<Mutex<BTreeMap<String, Cursor>>>,
    }

    impl Memory {
        fn ids(&self) -> Vec<String> {
            self.items.lock().unwrap().keys().cloned().collect()
        }

        fn cursor(&self, scope: &str) -> Option<Cursor> {
            self.cursors.lock().unwrap().get(scope).cloned()
        }
    }

    impl Sink for Memory {
        async fn store(&self, batch: &[Incoming]) -> Result<usize, Fault> {
            let mut items = self.items.lock().unwrap();
            let mut new = 0;
            for incoming in batch {
                if items
                    .insert(incoming.external_id.clone(), incoming.mail.text.clone())
                    .is_none()
                {
                    new += 1;
                }
            }
            Ok(new)
        }

        async fn load(&self, scope: &str) -> Result<Option<Cursor>, Fault> {
            Ok(self.cursors.lock().unwrap().get(scope).cloned())
        }

        async fn save(&self, scope: &str, cursor: &Cursor) -> Result<(), Fault> {
            self.cursors
                .lock()
                .unwrap()
                .insert(scope.to_owned(), cursor.clone());
            Ok(())
        }
    }

    fn mailbox(count: u32) -> Arc<FakeServer> {
        let messages: Vec<(u32, String)> = (1..=count)
            .map(|uid| (uid, format!("body {uid}")))
            .collect();
        let borrowed: Vec<(u32, &str)> = messages
            .iter()
            .map(|(uid, body)| (*uid, body.as_str()))
            .collect();
        Arc::new(FakeServer::with_messages("INBOX", &borrowed))
    }

    fn watcher(
        server: &Arc<FakeServer>,
        sink: &Memory,
    ) -> (Watcher<Arc<FakeServer>, Memory>, watch::Receiver<SyncState>) {
        let (tx, rx) = watch::channel(SyncState::Starting);
        let sync = Sync::new(Arc::clone(server), capability()).with_batch(3);
        (Watcher::new(sync, sink.clone(), tx), rx)
    }

    #[tokio::test]
    async fn the_first_round_fetches_nothing_and_points_both_cursors_at_the_top() {
        let server = mailbox(10);
        let sink = Memory::default();
        let (mut w, _) = watcher(&server, &sink);

        let round = w.round().await.unwrap();
        // The round sets the cursors up and then takes one batch of history.
        assert_eq!(round.new, 3);
        assert!(!round.complete);
        assert_eq!(
            sink.cursor("INBOX"),
            Some(Cursor::ImapUid {
                uidvalidity: 1,
                highest: 10
            })
        );
        assert_eq!(
            sink.cursor("INBOX#history"),
            Some(Cursor::ImapHistory {
                uidvalidity: 1,
                oldest: Some(8),
                complete: false
            })
        );
        let ids = sink.ids();
        assert!(
            ids.iter().any(|id| id.ends_with(":10@example.com")),
            "{ids:?}"
        );
        assert!(
            ids.iter().any(|id| id.ends_with(":8@example.com")),
            "{ids:?}"
        );
        assert!(
            !ids.iter().any(|id| id.ends_with(":1@example.com")),
            "newest first"
        );
    }

    #[tokio::test]
    async fn history_arrives_one_batch_per_round_and_then_the_account_is_live() {
        let server = mailbox(7);
        let sink = Memory::default();
        let (mut w, status) = watcher(&server, &sink);

        let mut rounds = 0;
        loop {
            rounds += 1;
            let round = w.round().await.unwrap();
            if round.complete {
                break;
            }
            assert!(matches!(*status.borrow(), SyncState::Backfilling { .. }));
        }
        assert_eq!(rounds, 3, "7 messages in batches of 3");
        assert_eq!(sink.ids().len(), 7);
        assert!(matches!(*status.borrow(), SyncState::Live { .. }));
        assert_eq!(w.progress().done, 7);
        assert_eq!(w.progress().total, Some(7));
        assert_eq!(server.fetch_calls(), 3, "nothing was fetched twice");
    }

    #[tokio::test]
    async fn mail_arriving_during_the_backfill_is_taken_next_round() {
        let server = mailbox(9);
        let sink = Memory::default();
        let (mut w, _) = watcher(&server, &sink);

        w.round().await.unwrap();
        server.deliver("INBOX", 10, "just arrived");
        let round = w.round().await.unwrap();
        assert_eq!(round.new, 4, "the new one, and a batch of history");
        assert!(sink.ids().iter().any(|id| id.ends_with(":10@example.com")));
        assert_eq!(
            sink.cursor("INBOX"),
            Some(Cursor::ImapUid {
                uidvalidity: 1,
                highest: 10
            })
        );
    }

    #[tokio::test]
    async fn a_restart_picks_up_where_the_cursors_say() {
        let server = mailbox(8);
        let sink = Memory::default();
        {
            let (mut w, _) = watcher(&server, &sink);
            w.round().await.unwrap();
            w.round().await.unwrap();
        }
        assert_eq!(sink.ids().len(), 6);
        let fetched_before = server.fetch_calls();

        // A new process, the same store.
        let (mut w, _) = watcher(&server, &sink);
        let round = w.round().await.unwrap();
        assert_eq!(round.new, 2, "only the two that were left");
        assert!(round.complete);
        assert_eq!(server.fetch_calls(), fetched_before + 1);
    }

    #[tokio::test]
    async fn a_renumbered_folder_is_walked_again_and_nothing_is_stored_twice() {
        let server = mailbox(4);
        let sink = Memory::default();
        let (mut w, _) = watcher(&server, &sink);
        while !w.round().await.unwrap().complete {}
        assert_eq!(sink.ids().len(), 4);

        server.renumber("INBOX", 2);
        let round = w.round().await.unwrap();
        assert!(!round.complete, "history has to be walked again");
        assert_eq!(
            sink.cursor("INBOX"),
            Some(Cursor::ImapUid {
                uidvalidity: 2,
                highest: 4
            })
        );
        assert_eq!(
            sink.cursor("INBOX#history"),
            Some(Cursor::ImapHistory {
                uidvalidity: 2,
                oldest: Some(2),
                complete: false
            })
        );
        // The identifier is the Message-ID, which the renumbering did not
        // touch, so the refetched mail is recognised and dropped.
        assert_eq!(round.new, 0, "the same mail under new numbers");
        assert_eq!(sink.ids().len(), 4);
    }

    #[tokio::test]
    async fn a_dropped_connection_keeps_what_was_already_stored() {
        let server = mailbox(6);
        let sink = Memory::default();
        let (mut w, _) = watcher(&server, &sink);
        w.round().await.unwrap();
        server.break_next_fetches(1);
        let fault = w.round().await.unwrap_err();
        assert!(fault.retryable());
        assert_eq!(sink.ids().len(), 3);
        // And the next round carries on from the cursor.
        let round = w.round().await.unwrap();
        assert_eq!(round.new, 3);
        assert!(round.complete);
    }

    #[tokio::test]
    async fn every_wanted_folder_is_caught_up_but_history_comes_one_folder_at_a_time() {
        let server = mailbox(2);
        server.add_folder(FakeFolder {
            name: "Archive".into(),
            uidvalidity: 5,
            wanted: true,
            messages: vec![],
        });
        server.deliver("Archive", 11, "old");
        server.deliver("Archive", 12, "older");
        let sink = Memory::default();
        let (mut w, _) = watcher(&server, &sink);

        let first = w.round().await.unwrap();
        assert_eq!(first.new, 2, "one batch, from INBOX only");
        let second = w.round().await.unwrap();
        assert_eq!(second.new, 2, "then the Archive");
        assert!(second.complete);
        assert_eq!(w.progress().done, 4);
    }

    #[tokio::test(start_paused = true)]
    async fn the_live_loop_waits_on_the_server_and_takes_what_arrives() {
        let server = mailbox(2);
        let sink = Memory::default();
        let (tx, status) = watch::channel(SyncState::Starting);
        let sync = Sync::new(Arc::clone(&server), capability()).with_batch(5);
        let mut w = Watcher::new(sync, sink.clone(), tx);

        let server_for_later = Arc::clone(&server);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(90)).await;
            server_for_later.deliver("INBOX", 3, "new");
        });

        let mut attempt = 0;
        let outcome = tokio::time::timeout(Duration::from_secs(200), w.run(&mut attempt)).await;
        assert!(outcome.is_err(), "the loop does not end on its own");
        assert!(server.waits() >= 2, "it waited rather than spun");
        assert!(sink.ids().iter().any(|id| id.ends_with(":3@example.com")));
        assert!(matches!(*status.borrow(), SyncState::Live { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn a_transient_fault_is_retried_with_backoff_and_forgotten_once_it_passes() {
        let server = mailbox(4);
        server.break_next_fetches(1);
        let sink = Memory::default();
        let (tx, rx) = watch::channel(SyncState::Starting);
        let mut seen: Vec<SyncState> = Vec::new();
        let mut watching = rx;
        let collector = tokio::spawn(async move {
            while watching.changed().await.is_ok() {
                seen.push(watching.borrow().clone());
                if matches!(seen.last(), Some(SyncState::Live { .. })) {
                    break;
                }
            }
            seen
        });

        let connect_server = Arc::clone(&server);
        let running = tokio::spawn(run_account(
            capability(),
            move || {
                let s = Arc::clone(&connect_server);
                async move { Ok::<_, Fault>(s) }
            },
            sink.clone(),
            tx,
        ));

        let seen = tokio::time::timeout(Duration::from_secs(600), collector)
            .await
            .expect("the account came back")
            .unwrap();
        running.abort();

        assert!(
            seen.iter()
                .any(|s| matches!(s, SyncState::Retrying { attempt: 1, .. })),
            "{seen:?}"
        );
        assert!(matches!(seen.last(), Some(SyncState::Live { .. })));
        assert_eq!(sink.ids().len(), 4);
    }

    #[tokio::test]
    async fn a_refused_password_stops_the_account_and_says_so() {
        let sink = Memory::default();
        let (tx, status) = watch::channel(SyncState::Starting);
        let fault = run_account(
            capability(),
            || async {
                Err::<Arc<FakeServer>, Fault>(Fault::needs_user(
                    "me@example.com",
                    "the password was refused",
                ))
            },
            sink,
            tx,
        )
        .await;
        assert!(!fault.retryable());
        assert!(matches!(*status.borrow(), SyncState::NeedsLogin { .. }));
    }
}
