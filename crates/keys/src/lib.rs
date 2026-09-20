//! Key material types shared by the encrypted stores and the egress path.
//!
//! Design: `docs/design/08-storage.md` (database and file keys) and
//! `docs/design/02-trust-boundary.md` (the ticket key).
//!
//! [`MasterKey`] is the one secret; the database and file keys come out of it
//! by HKDF-SHA256 with a labelled, versioned info string. Passphrase wrapping
//! and the keychain are still to come.

#![forbid(unsafe_code)]

mod db_key;
mod master;
mod ticket_key;

pub use db_key::DbKey;
pub use master::{FileKey, MasterKey};
pub use ticket_key::TicketKey;
