//! What went wrong, and whether anybody needs to be told.
//!
//! Design: `docs/design/05-connectors.md`, "恢复".
//!
//! Three kinds of trouble, and the difference between them is entirely about
//! the person: a dropped connection is the connector's problem and it should
//! deal with it silently, a changed password is the user's problem and only
//! they can fix it, and a deleted account is nobody's problem to fix. Sorting
//! them here rather than in the interface means the interface never has to
//! guess which one an error is.

use serde::{Deserialize, Serialize};

/// Who has to do something about a fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// It will pass. Back off and try again; say nothing.
    Transient,
    /// Only the user can fix it: a new password, a fresh login, a revoked
    /// session. The account stops until they do.
    NeedsUser,
    /// It will not come back: the account is gone, the server does not
    /// exist. Stop, explain, keep everything already fetched.
    Permanent,
}

/// Something that stopped a connector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{account}: {detail}")]
pub struct Fault {
    /// The account it happened to.
    pub account: String,
    /// Who has to act.
    pub severity: Severity,
    /// What happened, in words the user can read. This reaches the interface
    /// unchanged, so it is written for them and not for a log.
    pub detail: String,
}

impl Fault {
    /// It will pass on its own.
    pub fn transient(account: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            account: account.into(),
            severity: Severity::Transient,
            detail: detail.into(),
        }
    }

    /// Only the user can fix it.
    pub fn needs_user(account: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            account: account.into(),
            severity: Severity::NeedsUser,
            detail: detail.into(),
        }
    }

    /// It will not come back.
    pub fn permanent(account: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            account: account.into(),
            severity: Severity::Permanent,
            detail: detail.into(),
        }
    }

    /// Whether to keep trying.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(self.severity, Severity::Transient)
    }

    /// How long to wait before the next attempt, given how many have failed.
    ///
    /// Exponential with a ceiling. A connector that keeps hammering a server
    /// through an outage is a connector that gets the account rate-limited,
    /// which turns someone else's brief problem into the user's long one.
    #[must_use]
    pub fn backoff(attempt: u32) -> std::time::Duration {
        const CEILING_SECS: u64 = 15 * 60;
        let seconds = 2u64.saturating_pow(attempt.min(16)).min(CEILING_SECS);
        std::time::Duration::from_secs(seconds.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_transient_faults_are_retried() {
        assert!(Fault::transient("a", "the server hung up").retryable());
        assert!(!Fault::needs_user("a", "the password has changed").retryable());
        assert!(!Fault::permanent("a", "no such mailbox").retryable());
    }

    #[test]
    fn backoff_grows_and_then_stops_growing() {
        assert_eq!(Fault::backoff(0).as_secs(), 1);
        assert_eq!(Fault::backoff(1).as_secs(), 2);
        assert_eq!(Fault::backoff(5).as_secs(), 32);
        assert_eq!(Fault::backoff(30).as_secs(), 15 * 60, "and never longer");
    }

    #[test]
    fn a_fault_reads_as_a_sentence_for_the_user() {
        let fault = Fault::needs_user(
            "me@example.com",
            "the password has changed; sign in again to continue",
        );
        assert_eq!(
            fault.to_string(),
            "me@example.com: the password has changed; sign in again to continue"
        );
    }
}
