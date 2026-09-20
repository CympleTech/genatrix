//! What the sync engine needs from a mail server, and nothing more.
//!
//! Design: `docs/design/05-connectors.md`.
//!
//! The interesting part of a connector is not the protocol. It is the order
//! things are fetched in, what happens when a connection dies halfway, and
//! whether picking up again loses or repeats anything. None of that is about
//! IMAP, so none of it is written against IMAP: it is written against this,
//! and IMAP is one implementation behind it.
//!
//! That split is also what makes the engine testable. A fake source with a
//! scripted mailbox can be made to renumber a folder, drop a connection
//! mid-fetch, or grow while being read, which are exactly the cases that go
//! wrong in the field and are impossible to arrange against a real server.

use std::future::Future;

use genatrix_connector::Fault;

/// A folder on the server, and what the server says about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    /// Name as the server gives it.
    pub name: String,
    /// The validity marker. When this changes, every UID in the folder means
    /// something different and the folder has to be read again.
    pub uidvalidity: u32,
    /// How many messages are in it, for the progress estimate.
    pub count: u32,
    /// Whether this folder is worth reading. Gmail's "All Mail" holds every
    /// message once, so reading the label folders as well would fetch each
    /// message many times for nothing.
    pub wanted: bool,
}

/// One message as fetched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    /// Its UID within the folder.
    pub uid: u32,
    /// Gmail's account-wide message id, when the server offers it.
    pub gmail_message_id: Option<u64>,
    /// Gmail's thread id, when the server offers it.
    pub gmail_thread_id: Option<u64>,
    /// The message, exactly as it arrived.
    pub raw: Vec<u8>,
}

/// A mail server, reduced to what a connector actually does with one.
pub trait MailSource {
    /// Folders worth reading.
    fn folders(&self) -> impl Future<Output = Result<Vec<Folder>, Fault>> + Send;

    /// The UIDs in a folder, ascending, within a range.
    ///
    /// `above` is exclusive, so passing the highest UID already taken asks
    /// for exactly what is new. `None` means from the beginning.
    fn uids(
        &self,
        folder: &str,
        above: Option<u32>,
    ) -> impl Future<Output = Result<Vec<u32>, Fault>> + Send;

    /// Fetch messages by UID. May return fewer than asked for: a message can
    /// be deleted between listing and fetching, which is ordinary.
    fn fetch(
        &self,
        folder: &str,
        uids: &[u32],
    ) -> impl Future<Output = Result<Vec<Fetched>, Fault>> + Send;
}

#[cfg(test)]
pub mod fake;
