//! Sensitivity rules, redaction, and the egress gate.
//!
//! Design: `docs/design/02-trust-boundary.md`.
//!
//! This is the first of the two layers that decide what may leave the
//! device. It understands items, computes a level from rules, redacts, and
//! writes the ledger entry before anything is sent. The second layer is the
//! gateway in `genatrix-llm`, which understands none of that and checks only
//! the ticket this crate issues.
//!
//! Nothing here ever lowers a level. Machines escalate; only the user lowers,
//! and that arrives as an annotation, not as a call into this crate.

#![forbid(unsafe_code)]

pub mod gate;
pub mod patterns;
pub mod redact;
pub mod rules;

pub use gate::{Decision, EgressGate, GateError, Initiator, Message, Outcome, Prepared, Request};
pub use patterns::{Match, Secret};
pub use redact::{Identity, Redacted, redact};
pub use rules::{Candidate, Judgement, RuleError, RuleSet};
