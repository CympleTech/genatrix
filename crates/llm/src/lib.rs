//! Roles, the model registry, and the gateway that enforces egress policy.
//!
//! Design: `docs/design/04-model-layer.md`, with the policy it enforces in
//! `docs/design/02-trust-boundary.md`.
//!
//! The gateway is the second of two independent layers. The first is the
//! egress gate in the core: it understands items, computes a level, redacts,
//! writes the ledger entry, and issues a ticket. This layer understands none
//! of that. It checks a ticket against the bytes in front of it and refuses
//! anything it cannot account for. Either layer failing alone is not enough
//! to put personal data in front of a cloud provider.
//!
//! Two rules are enforced here and nowhere else:
//!
//! - A cloud provider accepts only `Public` or `Redacted` content.
//! - A purpose that reads whole corpora (classification, extraction,
//!   embedding, sensitivity judgement) never reaches a cloud provider,
//!   whatever its ticket says.

#![forbid(unsafe_code)]

pub mod config;
pub mod gateway;
pub mod local;
pub mod policy;
pub mod registry;
pub mod server;
pub mod ticket;

pub use config::{Config, ConfigError};
pub use gateway::{Forward, Refusal, decide};
pub use local::LocalClient;
pub use policy::{PolicyError, check_request};
pub use registry::{Location, ModelEntry, Registry};
pub use server::{Gateway, TICKET_HEADER, router, serve};
pub use ticket::{Purpose, Ticket, TicketError, TicketLevel, TicketStore};
