//! Key material types shared by the encrypted stores and the egress path.
//!
//! Design: `docs/design/08-storage.md` (database and file keys) and
//! `docs/design/02-trust-boundary.md` (the ticket key).
//!
//! Derivation from the master key (HKDF, passphrase wrapping, the recovery
//! key) lands here with milestone M1. Phase zero needs two key types.

#![forbid(unsafe_code)]

mod db_key;
mod ticket_key;

pub use db_key::DbKey;
pub use ticket_key::TicketKey;
