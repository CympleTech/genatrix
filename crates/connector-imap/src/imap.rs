//! The wire: IMAP itself.
//!
//! Design: `docs/design/05-connectors.md`.
//!
//! Deliberately the thinnest part of this crate. Everything that decides
//! anything lives in [`crate::sync`] and is tested against a fake server;
//! this only turns those requests into IMAP commands. When something here is
//! wrong, it is wrong in a way a real server reveals immediately, which is
//! the kind of wrong worth having in the part that cannot be tested offline.
//!
//! Verified against Gmail. What that run taught: the server offers its
//! extensions, `async-imap` parses `X-GM-MSGID` but has no accessor for
//! `X-GM-THRID`, so threads fall back to the `References` chain until it
//! does; and "All Mail" alone holds every message, so it is the only Gmail
//! folder read.

use std::collections::HashSet;

use async_imap::Session;
use async_imap::types::NameAttribute;
use async_native_tls::TlsConnector;
use futures::StreamExt;
use genatrix_connector::Fault;
use genatrix_connector::capability::Host;
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

use crate::source::{Fetched, Folder, MailSource};

type Stream = TlsStream;
type TlsStream = async_native_tls::TlsStream<Compat<TcpStream>>;

/// Everything needed to reach one mailbox.
#[derive(Clone, Debug)]
pub struct Credentials {
    /// The account, which is also the login name for every provider we care
    /// about.
    pub account: String,
    /// An app-specific password. Design 00 rules out OAuth: ordinary people
    /// do not create developer credentials.
    pub password: String,
    /// Where to read mail.
    pub imap: Host,
}

/// A live IMAP session.
pub struct Imap {
    session: tokio::sync::Mutex<Session<Stream>>,
    account: String,
    /// Whether the server offers Gmail's extensions, which give an
    /// account-wide message id and a real thread id. Worth a lot: without
    /// them an identifier has to be built out of folder and UID, which the
    /// server may renumber.
    gmail: bool,
}

impl std::fmt::Debug for Imap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Imap")
            .field("account", &self.account)
            .field("gmail_extensions", &self.gmail)
            .finish_non_exhaustive()
    }
}

impl Imap {
    /// Connect and log in.
    ///
    /// The caller has already checked the host against the account's
    /// capability; that check is not repeated here, because a check in the
    /// same function as the connection is a check an attacker who reached
    /// this code has already passed. The one that protects the user is the
    /// process sandbox.
    pub async fn connect(credentials: &Credentials) -> Result<Self, Fault> {
        let account = credentials.account.clone();
        let address = (credentials.imap.name.as_str(), credentials.imap.port);

        let tcp = TcpStream::connect(address)
            .await
            .map_err(|e| connect_fault(&account, &credentials.imap, &e))?;
        let tls = TlsConnector::new()
            .connect(&credentials.imap.name, tcp.compat())
            .await
            .map_err(|e| Fault::transient(&account, format!("could not start TLS: {e}")))?;

        let client = async_imap::Client::new(tls);
        let mut session = client
            .login(&credentials.account, &credentials.password)
            .await
            .map_err(|(e, _)| login_fault(&account, &e))?;

        let capabilities = session
            .capabilities()
            .await
            .map_err(|e| Fault::transient(&account, format!("{e}")))?;
        let gmail = capabilities.has_str("X-GM-EXT-1");

        Ok(Self {
            session: tokio::sync::Mutex::new(session),
            account,
            gmail,
        })
    }

    /// Whether the server offers Gmail's extensions.
    #[must_use]
    pub const fn has_gmail_extensions(&self) -> bool {
        self.gmail
    }

    /// Close the session politely.
    pub async fn logout(&self) -> Result<(), Fault> {
        self.session
            .lock()
            .await
            .logout()
            .await
            .map_err(|e| Fault::transient(&self.account, format!("{e}")))
    }

    /// What to ask for in a fetch. Gmail's identifiers when it has them.
    const fn fetch_items(&self) -> &'static str {
        if self.gmail {
            "(UID X-GM-MSGID X-GM-THRID BODY.PEEK[])"
        } else {
            "(UID BODY.PEEK[])"
        }
    }
}

impl MailSource for Imap {
    async fn folders(&self) -> Result<Vec<Folder>, Fault> {
        let mut session = self.session.lock().await;

        let mut listed = Vec::new();
        {
            let mut names = session
                .list(Some(""), Some("*"))
                .await
                .map_err(|e| Fault::transient(&self.account, format!("{e}")))?;
            while let Some(name) = names.next().await {
                let name = name.map_err(|e| Fault::transient(&self.account, format!("{e}")))?;
                if name.attributes().contains(&NameAttribute::NoSelect) {
                    continue;
                }
                let attributes: Vec<String> =
                    name.attributes().iter().map(|a| format!("{a:?}")).collect();
                listed.push((name.name().to_owned(), attributes));
            }
        }

        let mut folders = Vec::with_capacity(listed.len());
        for (name, attributes) in listed {
            let wanted = worth_reading(self.gmail, &name, &attributes);
            if !wanted {
                folders.push(Folder {
                    name,
                    uidvalidity: 0,
                    count: 0,
                    wanted: false,
                });
                continue;
            }
            // `examine` rather than `select`: read-only, so nothing here can
            // mark a message as seen behind the user's back.
            let mailbox = session
                .examine(&name)
                .await
                .map_err(|e| Fault::transient(&self.account, format!("{name}: {e}")))?;
            folders.push(Folder {
                uidvalidity: mailbox.uid_validity.unwrap_or(0),
                count: mailbox.exists,
                name,
                wanted: true,
            });
        }
        Ok(folders)
    }

    async fn uids(&self, folder: &str, above: Option<u32>) -> Result<Vec<u32>, Fault> {
        let mut session = self.session.lock().await;
        session
            .examine(folder)
            .await
            .map_err(|e| Fault::transient(&self.account, format!("{folder}: {e}")))?;
        let query = match above {
            Some(highest) => format!("UID {}:*", highest.saturating_add(1)),
            None => "ALL".to_owned(),
        };
        let found: HashSet<u32> = session
            .uid_search(&query)
            .await
            .map_err(|e| Fault::transient(&self.account, format!("{folder}: {e}")))?;
        let mut uids: Vec<u32> = found
            .into_iter()
            // `UID n:*` always returns at least one message even when none is
            // above n, because a range with a wildcard cannot be empty. Drop
            // anything that is not actually new.
            .filter(|uid| above.is_none_or(|a| *uid > a))
            .collect();
        uids.sort_unstable();
        Ok(uids)
    }

    async fn fetch(&self, folder: &str, uids: &[u32]) -> Result<Vec<Fetched>, Fault> {
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let items = self.fetch_items();
        let set = uid_set(uids);

        let mut session = self.session.lock().await;
        session
            .examine(folder)
            .await
            .map_err(|e| Fault::transient(&self.account, format!("{folder}: {e}")))?;
        let mut messages = session
            .uid_fetch(&set, items)
            .await
            .map_err(|e| Fault::transient(&self.account, format!("{folder}: {e}")))?;

        let mut out = Vec::with_capacity(uids.len());
        while let Some(message) = messages.next().await {
            let message =
                message.map_err(|e| Fault::transient(&self.account, format!("{folder}: {e}")))?;
            // A message deleted between listing and fetching comes back
            // without a body. That is ordinary, not a failure.
            let (Some(uid), Some(body)) = (message.uid, message.body()) else {
                continue;
            };
            out.push(Fetched {
                uid,
                gmail_message_id: message.gmail_msg_id().copied(),
                // Requested, and the server sends it, but the library
                // exposes no way to read it. Threading falls back to the
                // References chain, which is what every non-Gmail server
                // gets anyway.
                gmail_thread_id: None,
                raw: body.to_vec(),
            });
        }
        Ok(out)
    }
}

/// Whether a folder should be read.
///
/// On Gmail every message, sent ones included, is in "All Mail" exactly
/// once, and every other folder is a label, a view onto it. Reading any label
/// as well would fetch each message once more per label, for nothing. Drafts
/// and spam are skipped because they are not correspondence.
///
/// `attributes` are the folder's special-use markers as the library prints
/// them, such as `All`, `Sent`, `Junk`.
fn worth_reading(gmail: bool, name: &str, attributes: &[String]) -> bool {
    let lower = name.to_lowercase();
    let has = |attribute: &str| attributes.iter().any(|a| a.contains(attribute));

    if has("Junk") || has("Trash") || has("Drafts") {
        return false;
    }
    if gmail {
        return has("All") || lower.ends_with("all mail");
    }
    !lower.contains("junk") && !lower.contains("spam") && !lower.contains("trash")
}

/// Collapse a list of UIDs into IMAP's range syntax, so a batch of fifty
/// consecutive messages is one short command rather than fifty numbers.
fn uid_set(uids: &[u32]) -> String {
    let mut sorted: Vec<u32> = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut parts: Vec<String> = Vec::new();
    let mut run = match sorted.first() {
        Some(first) => (*first, *first),
        None => return String::new(),
    };
    for uid in sorted.into_iter().skip(1) {
        if uid == run.1 + 1 {
            run.1 = uid;
        } else {
            parts.push(render_run(run));
            run = (uid, uid);
        }
    }
    parts.push(render_run(run));
    parts.join(",")
}

fn render_run((start, end): (u32, u32)) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}:{end}")
    }
}

fn connect_fault(account: &str, host: &Host, error: &std::io::Error) -> Fault {
    match error.kind() {
        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::TimedOut => {
            Fault::transient(account, format!("{host} did not answer: {error}"))
        }
        _ => Fault::permanent(account, format!("could not reach {host}: {error}")),
    }
}

/// Tell apart "try again" from "only you can fix this".
///
/// A refused login is the second kind: retrying a wrong password is how an
/// account gets locked, and design 05 puts this case in front of the user
/// with one button rather than in a log.
fn login_fault(account: &str, error: &async_imap::error::Error) -> Fault {
    let text = error.to_string();
    let lower = text.to_lowercase();
    if lower.contains("authenticationfailed")
        || lower.contains("invalid credentials")
        || lower.contains("login failed")
        || lower.contains("authentication failed")
    {
        return Fault::needs_user(
            account,
            "the password was refused. If this is Gmail, the app password may \
             have been revoked; generate a new one and sign in again.",
        );
    }
    Fault::transient(account, format!("could not sign in: {text}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_becomes_ranges_rather_than_a_list_of_numbers() {
        assert_eq!(uid_set(&[1, 2, 3, 4, 5]), "1:5");
        assert_eq!(uid_set(&[1, 2, 3, 7, 9, 10, 11]), "1:3,7,9:11");
        assert_eq!(uid_set(&[42]), "42");
        assert_eq!(uid_set(&[]), "");
    }

    fn attrs(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn on_gmail_only_all_mail_is_read_because_it_holds_everything_once() {
        assert!(worth_reading(true, "[Gmail]/All Mail", &attrs(&["All"])));
        assert!(!worth_reading(true, "[Gmail]/Sent Mail", &attrs(&["Sent"])));
        assert!(!worth_reading(true, "INBOX", &attrs(&[])));
        assert!(!worth_reading(true, "Receipts", &attrs(&[])));
        assert!(!worth_reading(true, "[Gmail]/Spam", &attrs(&["Junk"])));
        assert!(!worth_reading(true, "[Gmail]/Trash", &attrs(&["Trash"])));
    }

    #[test]
    fn elsewhere_every_folder_is_read_except_the_bins() {
        assert!(worth_reading(false, "INBOX", &attrs(&[])));
        assert!(worth_reading(false, "Sent", &attrs(&["Sent"])));
        assert!(worth_reading(false, "Receipts", &attrs(&[])));
        assert!(!worth_reading(false, "Junk", &attrs(&["Junk"])));
        assert!(!worth_reading(false, "Spam", &attrs(&[])));
        assert!(!worth_reading(false, "Drafts", &attrs(&["Drafts"])));
    }

    #[test]
    fn a_jumbled_batch_with_repeats_still_becomes_a_tidy_set() {
        assert_eq!(uid_set(&[5, 1, 3, 2, 5, 4]), "1:5");
    }

    #[test]
    fn a_refused_password_is_the_users_problem_and_says_what_to_do() {
        let error = async_imap::error::Error::Bad("AUTHENTICATIONFAILED".into());
        let fault = login_fault("me@example.com", &error);
        assert!(
            !fault.retryable(),
            "retrying a wrong password locks accounts"
        );
        assert!(fault.detail.contains("app password"), "{}", fault.detail);
    }

    #[test]
    fn a_server_having_a_bad_day_is_worth_retrying() {
        let error = async_imap::error::Error::Bad("temporary system failure".into());
        assert!(login_fault("me@example.com", &error).retryable());
    }

    #[test]
    fn a_refused_connection_is_transient_and_a_missing_host_is_not() {
        let host = Host::new("imap.example.com", 993);
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        assert!(connect_fault("a", &host, &refused).retryable());
        let unknown = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(!connect_fault("a", &host, &unknown).retryable());
    }
}
