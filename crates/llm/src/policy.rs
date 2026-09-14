//! The rule the gateway exists to enforce.
//!
//! Design: `docs/design/02-trust-boundary.md`. Deny by default: a request is
//! forwarded to a cloud provider only when the ticket says the content may
//! leave the device *and* the purpose is one that may ever use the cloud.
//! Anything else is refused, and the caller falls back to a local model.

use crate::registry::Location;
use crate::ticket::{Purpose, Ticket, TicketLevel};

/// Why a request may not go where it was routed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PolicyError {
    /// The content is not allowed off the device.
    #[error("content is {level} and may not leave this device")]
    LevelNotAllowed {
        /// The level on the ticket.
        level: &'static str,
    },
    /// This kind of work never leaves the device.
    #[error("purpose `{purpose}` is always local")]
    PurposeIsLocalOnly {
        /// The purpose on the ticket.
        purpose: &'static str,
    },
}

/// Decide whether a ticket permits reaching a provider at `location`.
///
/// Local providers accept anything: nothing leaves the machine, so there is
/// nothing to refuse. Cloud providers must pass both checks.
pub fn check_request(location: Location, ticket: &Ticket) -> Result<(), PolicyError> {
    if location == Location::Local {
        return Ok(());
    }
    if !ticket.purpose.may_use_cloud() {
        return Err(PolicyError::PurposeIsLocalOnly {
            purpose: ticket.purpose.as_str(),
        });
    }
    if !ticket.level.may_leave_device() {
        return Err(PolicyError::LevelNotAllowed {
            level: ticket.level.as_str(),
        });
    }
    Ok(())
}

/// Whether a purpose and level combination could ever reach the cloud.
/// Used by the registry when it resolves a model, so a request that could
/// not be served by a cloud provider is never routed to one in the first
/// place.
#[must_use]
pub fn cloud_is_possible(purpose: Purpose, level: TicketLevel) -> bool {
    purpose.may_use_cloud() && level.may_leave_device()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    fn ticket(purpose: Purpose, level: TicketLevel) -> Ticket {
        Ticket {
            payload_hash: [0; 32],
            target: "t".into(),
            model: "m".into(),
            purpose,
            level,
            caller: "gate".into(),
            nonce: [0; 16],
            expires_at: Utc::now() + Duration::seconds(30),
        }
    }

    #[test]
    fn local_accepts_everything() {
        for purpose in [Purpose::Classify, Purpose::Draft] {
            for level in [
                TicketLevel::Public,
                TicketLevel::Redacted,
                TicketLevel::Personal,
                TicketLevel::Secret,
            ] {
                assert!(check_request(Location::Local, &ticket(purpose, level)).is_ok());
            }
        }
    }

    #[test]
    fn cloud_refuses_secret_and_unredacted_personal() {
        for level in [TicketLevel::Personal, TicketLevel::Secret] {
            let err = check_request(Location::Cloud, &ticket(Purpose::Draft, level)).unwrap_err();
            assert!(
                matches!(err, PolicyError::LevelNotAllowed { .. }),
                "{err:?}"
            );
        }
    }

    #[test]
    fn cloud_refuses_corpus_purposes_even_when_redacted() {
        let err = check_request(
            Location::Cloud,
            &ticket(Purpose::Classify, TicketLevel::Redacted),
        )
        .unwrap_err();
        assert!(
            matches!(err, PolicyError::PurposeIsLocalOnly { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn cloud_accepts_public_and_redacted_for_ordinary_work() {
        for level in [TicketLevel::Public, TicketLevel::Redacted] {
            assert!(check_request(Location::Cloud, &ticket(Purpose::Summarize, level)).is_ok());
        }
    }

    #[test]
    fn cloud_is_possible_agrees_with_check_request() {
        for purpose in [
            Purpose::Classify,
            Purpose::Extract,
            Purpose::Embed,
            Purpose::IdentitySuggestion,
            Purpose::Summarize,
            Purpose::Draft,
            Purpose::Translate,
            Purpose::SearchRewrite,
            Purpose::Plan,
        ] {
            for level in [
                TicketLevel::Public,
                TicketLevel::Redacted,
                TicketLevel::Personal,
                TicketLevel::Secret,
            ] {
                let by_check = check_request(Location::Cloud, &ticket(purpose, level)).is_ok();
                assert_eq!(
                    by_check,
                    cloud_is_possible(purpose, level),
                    "{purpose:?} {level:?}"
                );
            }
        }
    }
}
