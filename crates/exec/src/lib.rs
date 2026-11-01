//! The executor host: one executor session, inside an approved scope.
//!
//! Design: `docs/design/13-executors.md`, "宿主", "ACP", "两层批准" and
//! "本机隔离".
//!
//! The core starts `genatrix-exec` for each step of a dispatch and talks to
//! it over the process's own standard input and output, so no socket and no
//! token are needed: nothing else holds those pipes. The host runs as the
//! user, like a connector. It starts the executor, such as Claude Code
//! through its ACP adapter, as the executor's own system user, and drives
//! it with ACP. It does three things and nothing else:
//!
//! 1. It sends only text that carries a valid delegation ticket for exactly
//!    those bytes ([`session`], proposed invariant 22).
//! 2. It answers every permission the executor asks for by the approved
//!    scope, and refuses what falls outside (proposed invariant 25).
//! 3. It passes what the executor says back to the core.
//!
//! It does not understand the work, call a model, or decide anything. The
//! git work around a session, preparing a workspace and taking commits out
//! as a bundle, is in [`workspace`], run by the core as the executor's user.

pub mod acp;
pub mod isolation;
pub mod session;
pub mod wire;
pub mod workspace;
