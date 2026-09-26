//! NZ tax, the first functional agent (design 11): it reads PDF invoices
//! in the user's mail, checks every figure against the arithmetic and the
//! rule file, proposes each purchase as an entry for the user to approve,
//! keeps the approved entries in an append-only ledger in its own space,
//! and computes GST101 and IR3 figures from them.
//!
//! The domain model and the rule files come from the finance project
//! (`taxcore`, `taxrules`). The model reports what a document says and
//! picks an account; it never computes a figure.

pub mod books;
pub mod reading;
pub mod rules;
pub mod words;

#[cfg(target_arch = "wasm32")]
mod guest;
