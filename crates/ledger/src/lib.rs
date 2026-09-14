//! Append-only ledger: egress, run, and action records.
//!
//! Design: `docs/design/02-trust-boundary.md` (what is recorded),
//! `docs/design/03-agent-layer.md` (runs and actions), and
//! `docs/design/08-storage.md` (how it is stored).
//!
//! Every entry carries the hash of the previous one. The chain exists to
//! make "has this been altered" a question with a mechanical answer; it
//! defends against bugs and mistakes, not against an attacker holding the
//! key. Writers append; nobody updates or deletes, and the database enforces
//! that with triggers.
//!
//! The ledger is generic over what it records: entries have a `kind`, a
//! `subject` id, and a JSON body. The typed record structs live in the gate
//! and agent crates; this crate makes sure whatever they write stays written.

#![forbid(unsafe_code)]

mod error;
mod head;
mod ledger;

pub use error::{Error, Result};
pub use ledger::{Entry, EntryFilter, Hash, Ledger, Verification, kind};
