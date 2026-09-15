//! The glue: what happens between "the agent layer wants to ask a model" and
//! "an answer comes back".
//!
//! Design: `docs/design/02-trust-boundary.md` for the order, and
//! `docs/design/03-agent-layer.md` for why the agent layer does not do this
//! itself.
//!
//! Every layer below has been right on its own. This is where they meet, and
//! the meeting has an order that matters:
//!
//! ```text
//! gate.prepare   assemble, redact, write the ledger entry, mint a ticket
//! gateway        check the ticket against the bytes, forward or refuse
//! gate.record    write down how it ended
//! restore        turn pseudonyms back into names, here, never out there
//! ```
//!
//! Two things are deliberate. The outcome is recorded even when the call
//! fails, because "we tried to send this and do not know what happened" is
//! exactly the state a person needs to see. And pseudonyms are restored after
//! the record is written, so what the ledger holds is what the provider saw,
//! not a tidied-up version of it.

use std::sync::Arc;

use genatrix_agent::protocol::RawReply;
use genatrix_agent::run::{CallError, Called, ModelCaller};
use genatrix_gate::gate::{EgressGate, Outcome, Prepared, Request};
use genatrix_llm::local::LocalClient;
use genatrix_llm::server::TICKET_HEADER;
use genatrix_model::PersonId;

/// Resolves a pseudonym key back to the name to show. The agent layer hands
/// the gate a set of identities keyed by person; this turns a key back into
/// a person's name once the answer is home.
pub trait Names: Send + Sync {
    /// The display name for a person, if we still know them.
    fn display_name(&self, person: PersonId) -> Option<String>;
}

/// Nobody has a name. Used in tests, and wherever restoring is not wanted.
#[derive(Clone, Copy, Debug, Default)]
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "used by tests and by future callers")
)]
pub struct NoNames;

impl Names for NoNames {
    fn display_name(&self, _person: PersonId) -> Option<String> {
        None
    }
}

/// Carries out model calls for the agent layer.
pub struct GatewayCaller<N> {
    gate: Arc<EgressGate>,
    gateway: LocalClient,
    names: N,
}

impl<N> std::fmt::Debug for GatewayCaller<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayCaller")
            .field("gateway", &self.gateway.socket())
            .finish_non_exhaustive()
    }
}

impl<N: Names> GatewayCaller<N> {
    /// Build a caller over an open gate and a gateway socket.
    pub fn new(
        gate: Arc<EgressGate>,
        gateway_socket: impl Into<std::path::PathBuf>,
        names: N,
    ) -> Self {
        Self {
            gate,
            gateway: LocalClient::new(gateway_socket),
            names,
        }
    }

    /// Whether the gateway answers.
    pub async fn gateway_healthy(&self) -> bool {
        self.gateway.healthy().await
    }

    /// Send a prepared call and record how it ended.
    async fn send(&self, prepared: &Prepared) -> Result<String, CallError> {
        let sent = self
            .gateway
            .post_with_header(
                "/v1/chat/completions",
                prepared.body.clone(),
                Some((TICKET_HEADER, &prepared.ticket)),
            )
            .await;

        let (outcome, result) = match sent {
            Ok((status, body)) if (200..300).contains(&status) => (
                Outcome::Sent {
                    response_hash: EgressGate::hash_response(&body),
                },
                Ok(String::from_utf8_lossy(&body).into_owned()),
            ),
            Ok((status, body)) => {
                let detail = format!(
                    "gateway returned {status}: {}",
                    String::from_utf8_lossy(&body)
                        .chars()
                        .take(300)
                        .collect::<String>()
                );
                (
                    Outcome::Failed {
                        detail: detail.clone(),
                    },
                    Err(CallError::Refused(detail)),
                )
            }
            Err(e) => {
                // The bytes may or may not have gone out. Saying "failed"
                // would be a guess; design 05 asks for the honest state.
                let detail = e.to_string();
                (
                    Outcome::Unknown {
                        detail: detail.clone(),
                    },
                    Err(CallError::Unreachable(detail)),
                )
            }
        };

        if let Err(e) = self.gate.record_outcome(&prepared.egress_id, &outcome) {
            tracing::error!(error = %e, egress = %prepared.egress_id, "could not record the outcome");
        }
        result
    }

    /// Pull the assistant's text out of a chat completion response.
    fn read_reply(body: &str) -> Result<RawReply, CallError> {
        let value: serde_json::Value = serde_json::from_str(body)
            .map_err(|e| CallError::Refused(format!("the model's reply was not JSON: {e}")))?;
        let text = value
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CallError::Refused("the model's reply had no message content".into()))?;
        Ok(RawReply::text(text))
    }
}

impl<N: Names> ModelCaller for GatewayCaller<N> {
    async fn call(&self, request: &Request) -> Result<Called, CallError> {
        let prepared = self
            .gate
            .prepare(request)
            .map_err(|e| CallError::Refused(e.to_string()))?;
        tracing::debug!(
            egress = %prepared.egress_id,
            target = %prepared.target,
            decision = ?prepared.decision,
            "sending"
        );
        let body = self.send(&prepared).await?;
        let mut reply = Self::read_reply(&body)?;

        // Names come back only here, on this machine, after the record of
        // what went out has already been written.
        if let Some(redaction) = &prepared.redaction {
            reply.text = redaction.restore(&reply.text, |key| {
                key.parse::<PersonId>()
                    .ok()
                    .and_then(|p| self.names.display_name(p))
            });
        }
        Ok(Called {
            egress_id: prepared.egress_id,
            reply,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chat_completion_reply_is_read() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"hello"}}]}"#;
        assert_eq!(
            GatewayCaller::<NoNames>::read_reply(body).unwrap().text,
            "hello"
        );
    }

    #[test]
    fn a_reply_we_cannot_read_is_refused_rather_than_guessed_at() {
        for body in ["not json", r#"{"choices":[]}"#, r#"{"error":{"type":"x"}}"#] {
            assert!(
                GatewayCaller::<NoNames>::read_reply(body).is_err(),
                "should refuse: {body}"
            );
        }
    }
}
