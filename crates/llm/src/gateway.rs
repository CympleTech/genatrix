//! The decision the gateway makes about every request, separated from the
//! HTTP that carries it so it can be tested on its own.
//!
//! Routing is derived from the request body, never from the ticket. The
//! ticket commits to the body's hash, so a caller cannot pick a target and
//! then present a permission for a different one: the two must agree, and
//! the body decides. That is what makes the target check meaningful.

use bytes::Bytes;
use serde::Deserialize;

use crate::policy::{self, PolicyError};
use crate::registry::{ModelEntry, Registry};
use crate::ticket::{Ticket, TicketError, TicketStore};
use genatrix_keys::TicketKey;

/// Why a request was not forwarded.
#[derive(Debug, thiserror::Error)]
pub enum Refusal {
    /// The body was not a JSON object with a `model` field.
    #[error("request body is not a chat completion request: {0}")]
    Unparsable(String),
    /// No model of that name is configured.
    #[error("unknown model `{0}`")]
    UnknownModel(String),
    /// The ticket did not check out.
    #[error(transparent)]
    Ticket(#[from] TicketError),
    /// The ticket checked out but does not permit this destination.
    #[error(transparent)]
    Policy(#[from] PolicyError),
}

impl Refusal {
    /// The HTTP status this refusal deserves.
    #[must_use]
    pub const fn status(&self) -> u16 {
        match self {
            Self::Unparsable(_) => 400,
            Self::UnknownModel(_) => 404,
            Self::Ticket(_) => 401,
            Self::Policy(_) => 403,
        }
    }

    /// A short machine-readable tag, for the ledger and for clients.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::Unparsable(_) => "bad_request",
            Self::UnknownModel(_) => "model_not_found",
            Self::Ticket(_) => "ticket_rejected",
            Self::Policy(_) => "policy_refused",
        }
    }
}

/// What the gateway will do.
#[derive(Debug)]
pub struct Forward<'a> {
    /// The model to send to.
    pub entry: &'a ModelEntry,
    /// The ticket that permitted it, now spent.
    pub ticket: Ticket,
}

#[derive(Deserialize)]
struct ModelField {
    model: String,
}

/// Route and authorize one request.
///
/// The order matters. The model is resolved from the body first so that the
/// ticket is checked against where the request is actually going; then the
/// ticket, which also spends its nonce; then the policy. A request refused
/// by policy has still spent its ticket, which is correct: the permission
/// was used up in the attempt.
pub fn decide<'a>(
    registry: &'a Registry,
    tickets: &TicketStore,
    key: &TicketKey,
    ticket_header: Option<&str>,
    body: &Bytes,
) -> Result<Forward<'a>, Refusal> {
    let parsed: ModelField =
        serde_json::from_slice(body).map_err(|e| Refusal::Unparsable(e.to_string()))?;
    let entry = registry
        .get(&parsed.model)
        .ok_or_else(|| Refusal::UnknownModel(parsed.model.clone()))?;
    let ticket = tickets.admit(ticket_header, body, &entry.name, key)?;
    policy::check_request(entry.location(), &ticket)?;
    Ok(Forward { entry, ticket })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Endpoint, Location};
    use crate::ticket::{Purpose, TicketLevel};
    use std::collections::BTreeMap;

    fn registry() -> Registry {
        Registry {
            models: vec![
                ModelEntry {
                    name: "local".into(),
                    endpoint: Endpoint::LocalSocket {
                        path: "/tmp/gx/infer.sock".into(),
                    },
                    model: "qwen3-8b-4bit".into(),
                    purposes: crate::registry::ALL_PURPOSES.to_vec(),
                    context_length: 32_768,
                },
                ModelEntry {
                    name: "cloud".into(),
                    endpoint: Endpoint::Anthropic {
                        base_url: "https://api.anthropic.com".into(),
                        key_ref: "anthropic".into(),
                    },
                    model: "claude-sonnet-5".into(),
                    purposes: vec![Purpose::Summarize, Purpose::Draft],
                    context_length: 200_000,
                },
            ],
            chains: BTreeMap::default(),
        }
    }

    fn key() -> TicketKey {
        TicketKey::from_bytes([5; 32])
    }

    fn body(model: &str) -> Bytes {
        Bytes::from(format!(
            r#"{{"model":"{model}","messages":[{{"role":"user","content":"hi"}}]}}"#
        ))
    }

    fn ticket_for(b: &Bytes, target: &str, purpose: Purpose, level: TicketLevel) -> String {
        Ticket::issue(b, target, "m", purpose, level, "gate")
            .unwrap()
            .encode(&key())
    }

    #[test]
    fn a_well_formed_request_is_forwarded() {
        let (r, s) = (registry(), TicketStore::new());
        let b = body("cloud");
        let h = ticket_for(&b, "cloud", Purpose::Draft, TicketLevel::Redacted);
        let fwd = decide(&r, &s, &key(), Some(&h), &b).unwrap();
        assert_eq!(fwd.entry.name, "cloud");
        assert_eq!(fwd.entry.location(), Location::Cloud);
    }

    #[test]
    fn secret_content_never_reaches_a_cloud_model() {
        let (r, s) = (registry(), TicketStore::new());
        let b = body("cloud");
        let h = ticket_for(&b, "cloud", Purpose::Draft, TicketLevel::Secret);
        let err = decide(&r, &s, &key(), Some(&h), &b).unwrap_err();
        assert_eq!(err.status(), 403);
        assert_eq!(err.tag(), "policy_refused");
    }

    #[test]
    fn the_same_content_is_allowed_locally() {
        let (r, s) = (registry(), TicketStore::new());
        let b = body("local");
        let h = ticket_for(&b, "local", Purpose::Draft, TicketLevel::Secret);
        assert!(decide(&r, &s, &key(), Some(&h), &b).is_ok());
    }

    #[test]
    fn a_ticket_for_the_local_model_does_not_authorize_the_cloud_one() {
        let (r, s) = (registry(), TicketStore::new());
        // The gate approved a local call; the body asks for the cloud model.
        let b = body("cloud");
        let h = ticket_for(&b, "local", Purpose::Draft, TicketLevel::Redacted);
        let err = decide(&r, &s, &key(), Some(&h), &b).unwrap_err();
        assert_eq!(err.status(), 401);
        assert!(matches!(
            err,
            Refusal::Ticket(TicketError::WrongTarget { .. })
        ));
    }

    #[test]
    fn a_swapped_body_is_refused_even_with_a_valid_ticket() {
        let (r, s) = (registry(), TicketStore::new());
        let approved = body("cloud");
        let h = ticket_for(&approved, "cloud", Purpose::Draft, TicketLevel::Redacted);
        let smuggled = Bytes::from(
            r#"{"model":"cloud","messages":[{"role":"user","content":"my card is 4111 1111 1111 1111"}]}"#,
        );
        let err = decide(&r, &s, &key(), Some(&h), &smuggled).unwrap_err();
        assert!(matches!(err, Refusal::Ticket(TicketError::PayloadMismatch)));
    }

    #[test]
    fn classification_is_refused_at_the_cloud_even_with_a_redacted_ticket() {
        let mut r = registry();
        // Even if someone misconfigures the cloud entry to claim it.
        r.models[1].purposes.push(Purpose::Classify);
        let s = TicketStore::new();
        let b = body("cloud");
        let h = ticket_for(&b, "cloud", Purpose::Classify, TicketLevel::Redacted);
        let err = decide(&r, &s, &key(), Some(&h), &b).unwrap_err();
        assert!(matches!(
            err,
            Refusal::Policy(PolicyError::PurposeIsLocalOnly { .. })
        ));
        assert!(
            r.validate().is_err(),
            "and the registry would not have started"
        );
    }

    #[test]
    fn no_ticket_no_request() {
        let (r, s) = (registry(), TicketStore::new());
        let b = body("cloud");
        let err = decide(&r, &s, &key(), None, &b).unwrap_err();
        assert_eq!(err.status(), 401);
    }

    #[test]
    fn a_ticket_works_once() {
        let (r, s) = (registry(), TicketStore::new());
        let b = body("cloud");
        let h = ticket_for(&b, "cloud", Purpose::Draft, TicketLevel::Public);
        assert!(decide(&r, &s, &key(), Some(&h), &b).is_ok());
        assert!(matches!(
            decide(&r, &s, &key(), Some(&h), &b).unwrap_err(),
            Refusal::Ticket(TicketError::Replayed)
        ));
    }

    #[test]
    fn unknown_models_and_junk_bodies_are_refused() {
        let (r, s) = (registry(), TicketStore::new());
        let b = body("nope");
        let h = ticket_for(&b, "nope", Purpose::Draft, TicketLevel::Public);
        assert_eq!(
            decide(&r, &s, &key(), Some(&h), &b).unwrap_err().status(),
            404
        );
        let junk = Bytes::from_static(b"not json");
        assert_eq!(
            decide(&r, &s, &key(), Some(&h), &junk)
                .unwrap_err()
                .status(),
            400
        );
    }
}
