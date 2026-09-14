//! Key material types shared by the encrypted stores.
//!
//! Design: `docs/design/08-storage.md`. Key derivation from the master key
//! (HKDF, passphrase wrapping, recovery key) lands here with milestone M1;
//! phase zero only needs the database key type.

#![forbid(unsafe_code)]

mod db_key;

pub use db_key::DbKey;
