//! Telegram, as the account holder sees it.
//!
//! Design: `docs/design/05-connectors.md`, "Telegram：用户账号协议". The
//! user-account protocol rather than a bot, because only the user's own
//! login sees the user's own history. Signing in works as it does on a
//! phone: number, code, and a password when two-step verification is on.
//!
//! What this crate holds: the session as a value that can be kept in the
//! core's encrypted store ([`session`]), the sign-in steps ([`login`]), the
//! reading of dialogs, history and live updates into the shapes the core
//! stores ([`sync`]), and the application credentials ([`credentials`]).
//! The binary in `main.rs` runs it as a connector process.

#![forbid(unsafe_code)]

pub mod credentials;
pub mod login;
pub mod session;
pub mod sync;

pub use credentials::Credentials;
pub use session::{JsonSession, Snapshot};
