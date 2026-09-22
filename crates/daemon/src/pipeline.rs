//! Pipelines: fixed sequences, written down, where the model fills blanks.
//!
//! Design: `docs/design/03-agent-layer.md`.

pub mod classify;
pub mod commitments;
pub mod digest;
pub mod draft;
pub mod embed;
pub mod summarize;

/// A store failure during a pipeline is the same kind of "we cannot continue
/// safely" as a ledger failure, and is reported through the same type.
pub(crate) fn store_error(e: &genatrix_store::Error) -> genatrix_ledger::Error {
    genatrix_ledger::Error::HeadMismatch(format!("store: {e}"))
}
