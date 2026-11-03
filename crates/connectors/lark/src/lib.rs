//! The Lark (Feishu) connector: the tasks assigned to the user, and their
//! comments, read only.
//!
//! Design: `docs/design/05-connectors.md`, "飞书（Lark）：你负责的任务".
//!
//! Lark is the one place Genatrix uses OAuth. The user's company creates a
//! custom app; the user signs in to it with the device flow, so nothing has
//! to call back to this machine; the core keeps the app secret in the
//! keychain and the rotating sign-in, like a Telegram session, in the
//! encrypted store beside the cursors. Signing in happens in the core
//! ([`auth`]); reading happens in the `genatrix-lark` process ([`sync`]),
//! which refreshes the sign-in and hands every new refresh token to the
//! core before it uses the access token that came with it.

#![forbid(unsafe_code)]

pub mod api;
pub mod auth;
pub mod brand;
pub mod normalize;
pub mod sync;

pub use auth::{Secret, Token};
pub use brand::Brand;

/// The scope the sign-in is kept under in the core's cursors.
pub const SIGN_IN_SCOPE: &str = "oauth";
/// The scope the sync position is kept under.
pub const TASKS_SCOPE: &str = "tasks";
