//! Domain model for the tax engine.
//!
//! This crate defines types and invariants and performs no I/O: no filesystem,
//! no network, no model calls. Extraction happens outside the engine — an agent
//! reads the document and submits structured fields, which [`document::
//! ExtractedInvoice::validate`] then checks arithmetically. The rule is that a
//! model may report figures but never compute them.

pub mod account;
pub mod bank;
pub mod document;
pub mod error;
pub mod gst;
pub mod ids;
pub mod ledger;
pub mod money;
pub mod provenance;
pub mod taxyear;

pub use account::{Account, AccountKind};
pub use bank::{BankTransaction, MatchCandidate, MatchStrength};
pub use document::{
    Document, DocumentSource, DocumentStatus, ExtractedInvoice, Extraction, LineItem, Severity,
    ValidationIssue,
};
pub use error::{Result, TaxError};
pub use gst::{GstRate, GstTreatment};
pub use ids::{AccountCode, BankTxnId, DocumentId, EntryId, ExtractionId, PostingId, ReturnRunId};
pub use ledger::{Entry, EntryBuilder, EntrySource, EntryStatus, ForeignAmount, Posting};
pub use money::{Currency, Money, Rounding};
pub use provenance::{Contribution, Provenance, ReturnLine, SourceRef};
pub use taxyear::{GstFrequency, GstPeriod, TaxYear};
