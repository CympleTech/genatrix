//! Mail, over IMAP for reading and SMTP for sending.
//!
//! Design: `docs/design/05-connectors.md`.
//!
//! IMAP rather than a provider API, because design 00 fixes the audience:
//! ordinary people, who do not create OAuth credentials. The cost is a fiddly
//! app-password flow on Gmail; the benefit is that one connector covers every
//! mailbox there is.

#![forbid(unsafe_code)]

pub mod imap;
pub mod normalize;
pub mod source;
pub mod sync;
pub mod text;
pub mod watch;

pub use imap::{Credentials, Imap};
pub use normalize::{Attachment, Mail, external_id, normalize, thread_key};
pub use source::{Fetched, Folder, MailSource, Wake};
pub use sync::{Incoming, Pass, Sync};
pub use watch::{Round, Sink, Watcher, run_account};
