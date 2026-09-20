//! What a connector is, and what it is allowed to do.
//!
//! Design: `docs/design/05-connectors.md`.
//!
//! A connector is the whole contact surface between Genatrix and one outside
//! system. It does three things and only three: fetch source objects and
//! normalize them, keep syncing without losing or duplicating anything, and
//! carry out actions a person has approved. It does not judge sensitivity, it
//! does not call models, it does not decide anything. It is a courier.
//!
//! Two ideas carry most of the weight here.
//!
//! **Capabilities are issued, not claimed.** A connector that declared its own
//! allowed hosts would be a connector that could change its mind once
//! compromised. The core issues [`AccountCapability`] from what the user
//! approved, and the connector process runs under a sandbox built from it, so
//! the boundary is outside the code that might be subverted.
//!
//! **Checkpoints make resumption exact.** Each account's progress is a
//! [`Checkpoint`] the core holds. Together with the idempotence of
//! `Source`, losing one means refetching, never losing or doubling.

#![forbid(unsafe_code)]

pub mod backfill;
pub mod capability;
pub mod checkpoint;
pub mod error;

pub use backfill::{Backfill, Progress};
pub use capability::{AccountCapability, Host};
pub use checkpoint::{Checkpoint, Cursor};
pub use error::{Fault, Severity};
