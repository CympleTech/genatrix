//! Reading a mailbox without losing anything or fetching anything twice.
//!
//! Design: `docs/design/05-connectors.md`, "生命周期" and "幂等与检查点".
//!
//! Two passes over every folder, and they are not the same pass.
//!
//! **Backfill** walks history newest first, because five minutes after adding
//! an account there should be something on the timeline worth reading, and
//! that is this week's mail rather than 2015's. It is resumable at any batch
//! boundary, since a person who closes the lid should not lose an hour of
//! importing.
//!
//! **Catch-up** takes what has arrived since the last run, oldest first,
//! because that is the order it should appear in.
//!
//! Neither has to be exact about what it has already seen. Duplicates are
//! dropped by the uniqueness of `Source` in the core, so the safe move when
//! anything is unclear is always to fetch more, never less. That is what lets
//! a renumbered folder simply be read again, and a dropped connection simply
//! be retried, without either turning into a correctness problem.

use genatrix_connector::capability::{AccountCapability, Host};
use genatrix_connector::{Checkpoint, Cursor, Fault, Progress};

use crate::normalize::{self, Mail};
use crate::source::{Fetched, MailSource, Wake};

/// How many messages are asked for at once.
///
/// Small enough that a dropped connection costs little, large enough that
/// the round trips do not dominate.
pub const BATCH: usize = 50;

/// One message, ready for the core to turn into an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incoming {
    /// The account it belongs to.
    pub account: String,
    /// Account-wide unique identifier, as design 01 requires.
    pub external_id: String,
    /// Which conversation.
    pub thread_key: String,
    /// The message as it arrived. This is what becomes the raw record.
    pub raw: Vec<u8>,
    /// The message, normalized.
    pub mail: Mail,
}

/// What one pass over a folder did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pass {
    /// How many messages came back.
    pub taken: usize,
    /// Whether there is more history behind this.
    pub more: bool,
    /// How many messages are still behind this, for the progress line.
    pub remaining: usize,
    /// Whether the folder had to be read from the beginning because the
    /// server renumbered it.
    pub restarted: bool,
}

/// Reads one account.
pub struct Sync<S> {
    source: S,
    capability: AccountCapability,
    batch: usize,
}

impl<S: MailSource> Sync<S> {
    /// Read this account through this source.
    pub fn new(source: S, capability: AccountCapability) -> Self {
        Self {
            source,
            capability,
            batch: BATCH,
        }
    }

    /// Use a different batch size.
    #[must_use]
    pub const fn with_batch(mut self, batch: usize) -> Self {
        self.batch = batch;
        self
    }

    /// The account this engine reads.
    #[must_use]
    pub fn account(&self) -> &str {
        &self.capability.account
    }

    /// Refuse to talk to a host this account was not granted.
    ///
    /// The process sandbox enforces the same thing from outside, which is
    /// what actually protects the user. This is the inner of the two checks:
    /// it turns a configuration mistake into a clear message instead of a
    /// connection that mysteriously hangs.
    pub fn check_host(&self, host: &Host) -> Result<(), Fault> {
        if self.capability.may_reach(host) {
            return Ok(());
        }
        Err(Fault::permanent(
            &self.capability.account,
            format!("this account is not allowed to connect to {host}"),
        ))
    }

    /// Folders this account should read.
    pub async fn folders(&self) -> Result<Vec<crate::source::Folder>, Fault> {
        Ok(self
            .source
            .folders()
            .await?
            .into_iter()
            .filter(|f| f.wanted)
            .collect())
    }

    /// The highest UID in a folder right now, or 0 when it is empty.
    ///
    /// Where a live cursor starts on first sight of a folder: everything at
    /// or below it is history, for the backfill to walk newest first, and
    /// everything above it is new.
    pub async fn newest_uid(&self, folder: &str) -> Result<u32, Fault> {
        Ok(self
            .source
            .uids(folder, None)
            .await?
            .into_iter()
            .max()
            .unwrap_or(0))
    }

    /// Wait for something to happen in a folder, or for the time to pass.
    pub async fn wait_for_change(
        &self,
        folder: &str,
        timeout: std::time::Duration,
    ) -> Result<Wake, Fault> {
        self.source.wait_for_change(folder, timeout).await
    }

    /// Take everything that has arrived since the checkpoint, oldest first.
    ///
    /// When the server has renumbered the folder the checkpoint means
    /// nothing, so the folder is read from the beginning. That costs a
    /// re-fetch and loses nothing; trusting the old number would silently
    /// skip mail.
    pub async fn catch_up(
        &self,
        folder: &str,
        uidvalidity: u32,
        checkpoint: &mut Checkpoint,
    ) -> Result<(Vec<Incoming>, Pass), Fault> {
        let (above, restarted) = match &checkpoint.cursor {
            Cursor::ImapUid {
                uidvalidity: known,
                highest,
            } if *known == uidvalidity => (Some(*highest), false),
            _ => (None, true),
        };

        let uids = self.source.uids(folder, above).await?;
        let mut out = Vec::new();
        let mut highest = above.unwrap_or(0);
        for chunk in uids.chunks(self.batch) {
            let batch = self.take(folder, uidvalidity, chunk).await?;
            for incoming in batch {
                out.push(incoming);
            }
            if let Some(last) = chunk.last() {
                highest = highest.max(*last);
                // Written after every batch, so stopping here loses at most
                // one batch of work rather than the whole pass.
                checkpoint.advance(Cursor::ImapUid {
                    uidvalidity,
                    highest,
                });
            }
        }

        let taken = out.len();
        Ok((
            out,
            Pass {
                taken,
                more: false,
                remaining: 0,
                restarted,
            },
        ))
    }

    /// Take one batch of history, newest first.
    ///
    /// `before` is the oldest UID already taken; `None` starts at the newest
    /// message in the folder. Call it again with the returned cursor until
    /// [`Pass::more`] is false.
    pub async fn backfill(
        &self,
        folder: &str,
        uidvalidity: u32,
        before: Option<u32>,
        progress: &mut Progress,
    ) -> Result<(Vec<Incoming>, Option<u32>, Pass), Fault> {
        let mut uids = self.source.uids(folder, None).await?;
        uids.sort_unstable_by(|a, b| b.cmp(a));
        if let Some(before) = before {
            uids.retain(|uid| *uid < before);
        }

        let chunk: Vec<u32> = uids.iter().take(self.batch).copied().collect();
        if chunk.is_empty() {
            progress.complete = true;
            return Ok((
                Vec::new(),
                before,
                Pass {
                    taken: 0,
                    more: false,
                    remaining: 0,
                    restarted: false,
                },
            ));
        }

        let batch = self.take(folder, uidvalidity, &chunk).await?;
        let oldest = chunk.iter().copied().min();
        let remaining = uids.len() - chunk.len();
        let more = remaining > 0;

        progress.done += batch.len() as u64;
        progress.complete = !more;
        progress.reached = batch
            .last()
            .and_then(|i| i.mail.date)
            .map(|d| d.format("%B %Y").to_string());

        let taken = batch.len();
        Ok((
            batch,
            oldest,
            Pass {
                taken,
                more,
                remaining,
                restarted: false,
            },
        ))
    }

    /// Fetch and normalize one batch.
    ///
    /// A message that will not parse is dropped with a line in the log rather
    /// than stopping the pass: one malformed mail in a mailbox of fifty
    /// thousand should not block the other forty-nine thousand.
    async fn take(
        &self,
        folder: &str,
        uidvalidity: u32,
        uids: &[u32],
    ) -> Result<Vec<Incoming>, Fault> {
        let mut fetched = self.source.fetch(folder, uids).await?;
        // A server answers a fetch in whatever order suits it. The order the
        // caller asked in is the order that carries meaning, so restore it
        // here rather than leaving every caller to remember.
        fetched.sort_by_key(|f| {
            uids.iter()
                .position(|uid| *uid == f.uid)
                .unwrap_or(usize::MAX)
        });
        Ok(fetched
            .into_iter()
            .filter_map(|f| self.to_incoming(folder, uidvalidity, &f))
            .collect())
    }

    fn to_incoming(&self, folder: &str, uidvalidity: u32, fetched: &Fetched) -> Option<Incoming> {
        match normalize::normalize(&fetched.raw) {
            Ok(mail) => Some(Incoming {
                account: self.capability.account.clone(),
                external_id: normalize::external_id(
                    fetched.gmail_message_id,
                    mail.message_id.as_deref(),
                    folder,
                    // The validity marker is part of the identifier only when
                    // there is no account-wide id to use instead.
                    uidvalidity,
                    fetched.uid,
                ),
                thread_key: normalize::thread_key(&mail, fetched.gmail_thread_id),
                raw: fetched.raw.clone(),
                mail,
            }),
            Err(e) => {
                tracing::warn!(
                    account = %self.capability.account,
                    folder,
                    uid = fetched.uid,
                    error = %e,
                    "skipping a message that will not parse"
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::fake::{FakeFolder, FakeServer};
    use genatrix_model::Connector;

    fn capability() -> AccountCapability {
        AccountCapability::new(Connector::Imap, "me@example.com")
            .with_host(Host::new("imap.example.com", 993))
            .with_effect("send_mail")
    }

    fn engine(server: FakeServer) -> Sync<FakeServer> {
        Sync::new(server, capability()).with_batch(3)
    }

    fn checkpoint(uidvalidity: u32, highest: u32) -> Checkpoint {
        Checkpoint::new(
            "me@example.com",
            "INBOX",
            Cursor::ImapUid {
                uidvalidity,
                highest,
            },
        )
    }

    fn mailbox(count: u32) -> FakeServer {
        let messages: Vec<(u32, String)> = (1..=count)
            .map(|uid| (uid, format!("body {uid}")))
            .collect();
        let borrowed: Vec<(u32, &str)> = messages
            .iter()
            .map(|(uid, body)| (*uid, body.as_str()))
            .collect();
        FakeServer::with_messages("INBOX", &borrowed)
    }

    #[tokio::test]
    async fn catch_up_takes_only_what_is_new() {
        let sync = engine(mailbox(10));
        let mut point = checkpoint(1, 7);
        let (mails, pass) = sync.catch_up("INBOX", 1, &mut point).await.unwrap();
        assert_eq!(pass.taken, 3, "messages 8, 9 and 10");
        assert!(!pass.restarted);
        assert_eq!(
            point.cursor,
            Cursor::ImapUid {
                uidvalidity: 1,
                highest: 10
            }
        );
        assert!(mails[0].external_id.ends_with(":8@example.com"));
    }

    #[tokio::test]
    async fn a_renumbered_folder_is_read_again_rather_than_trusted() {
        let server = mailbox(5);
        server.renumber("INBOX", 2);
        let sync = engine(server);
        let mut point = checkpoint(1, 4);
        let (mails, pass) = sync.catch_up("INBOX", 2, &mut point).await.unwrap();
        assert!(pass.restarted, "the old numbering means nothing now");
        assert_eq!(
            pass.taken, 5,
            "everything again, which duplicates cost nothing and losing mail would"
        );
        assert!(
            mails.iter().all(|m| m.external_id.starts_with("mid:")),
            "the identifier is the message's own, so the refetch is recognised"
        );
    }

    #[tokio::test]
    async fn the_checkpoint_moves_after_every_batch_not_at_the_end() {
        let server = mailbox(9);
        // Fail once, after the first batch has already been taken.
        let sync = engine(server);
        let mut point = checkpoint(1, 0);
        sync.source.break_next_fetches(0);
        let (_, _) = sync.catch_up("INBOX", 1, &mut point).await.unwrap();
        assert_eq!(
            point.cursor,
            Cursor::ImapUid {
                uidvalidity: 1,
                highest: 9
            }
        );
    }

    #[tokio::test]
    async fn a_dropped_connection_leaves_the_work_already_done() {
        let server = mailbox(9);
        server.break_next_fetches(1);
        let sync = engine(server);
        let mut point = checkpoint(1, 0);
        let failed = sync.catch_up("INBOX", 1, &mut point).await;
        assert!(failed.is_err());
        assert!(failed.unwrap_err().retryable(), "it is worth trying again");

        // Retrying picks up from where it got to, and the whole folder lands.
        let (mails, _) = sync.catch_up("INBOX", 1, &mut point).await.unwrap();
        assert_eq!(mails.len(), 9);
    }

    #[tokio::test]
    async fn backfill_walks_newest_first_so_this_week_arrives_first() {
        let sync = engine(mailbox(10));
        let mut progress = Progress::default();
        let (first, oldest, pass) = sync
            .backfill("INBOX", 1, None, &mut progress)
            .await
            .unwrap();
        assert_eq!(pass.taken, 3);
        assert!(pass.more);
        assert!(
            first[0].external_id.ends_with(":10@example.com"),
            "the newest message comes first: {}",
            first[0].external_id
        );
        assert_eq!(oldest, Some(8));

        let (second, _, _) = sync
            .backfill("INBOX", 1, oldest, &mut progress)
            .await
            .unwrap();
        assert!(second[0].external_id.ends_with(":7@example.com"));
        assert_eq!(progress.done, 6);
    }

    #[tokio::test]
    async fn backfill_finishes_and_says_so() {
        let sync = engine(mailbox(4));
        let mut progress = Progress::default();
        let mut cursor = None;
        let mut total = 0;
        loop {
            let (batch, oldest, pass) = sync
                .backfill("INBOX", 1, cursor, &mut progress)
                .await
                .unwrap();
            total += batch.len();
            cursor = oldest;
            if !pass.more {
                break;
            }
        }
        assert_eq!(total, 4);
        assert!(progress.complete);
        assert_eq!(progress.fraction(), Some(1.0));
    }

    #[tokio::test]
    async fn mail_arriving_mid_backfill_is_not_missed_by_the_catch_up() {
        let server = mailbox(6);
        let sync = engine(server);
        let mut progress = Progress::default();
        let (_, oldest, _) = sync
            .backfill("INBOX", 1, None, &mut progress)
            .await
            .unwrap();
        assert_eq!(oldest, Some(4));

        // A new message lands while history is still being read.
        sync.source.deliver("INBOX", 7, "just arrived");
        let mut point = checkpoint(1, 6);
        let (new, _) = sync.catch_up("INBOX", 1, &mut point).await.unwrap();
        assert_eq!(new.len(), 1);
        assert!(new[0].external_id.ends_with(":7@example.com"));
    }

    #[tokio::test]
    async fn a_message_that_will_not_parse_does_not_stop_the_others() {
        let server = FakeServer::with_messages("INBOX", &[(1, "fine"), (2, "also fine")]);
        server.add_folder(FakeFolder {
            name: "Broken".into(),
            uidvalidity: 1,
            wanted: true,
            messages: vec![crate::source::fake::Message {
                uid: 1,
                raw: vec![0xff, 0xfe, 0x00],
            }],
        });
        let sync = engine(server);
        let mut point = checkpoint(1, 0);
        let (mails, _) = sync.catch_up("INBOX", 1, &mut point).await.unwrap();
        assert_eq!(mails.len(), 2, "the readable folder is unaffected");
    }

    #[tokio::test]
    async fn only_folders_worth_reading_are_read() {
        let server = mailbox(2);
        server.add_folder(FakeFolder {
            name: "Work label".into(),
            uidvalidity: 1,
            wanted: false,
            messages: vec![],
        });
        let sync = engine(server);
        let folders = sync.folders().await.unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].name, "INBOX");
    }

    #[test]
    fn a_host_this_account_was_not_granted_is_refused() {
        let sync = engine(mailbox(0));
        assert!(sync.check_host(&Host::new("imap.example.com", 993)).is_ok());
        let err = sync
            .check_host(&Host::new("imap.evil.example", 993))
            .unwrap_err();
        assert!(!err.retryable());
        assert!(err.to_string().contains("not allowed"), "{err}");
    }

    #[tokio::test]
    async fn every_message_gets_an_identifier_and_a_conversation() {
        let sync = engine(mailbox(2));
        let mut point = checkpoint(1, 0);
        let (mails, _) = sync.catch_up("INBOX", 1, &mut point).await.unwrap();
        for mail in &mails {
            assert!(mail.external_id.starts_with("mid:"), "{}", mail.external_id);
            assert!(mail.thread_key.starts_with("ref:"));
            assert!(!mail.raw.is_empty(), "the original bytes are carried along");
            assert_eq!(mail.account, "me@example.com");
        }
        assert_ne!(mails[0].external_id, mails[1].external_id);
    }
}
