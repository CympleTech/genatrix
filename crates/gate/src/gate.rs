//! The egress gate: the one way out.
//!
//! Design: `docs/design/02-trust-boundary.md`, "出境闸门".
//!
//! Every model call goes through here, local ones included. The order is
//! fixed and is the point of the whole thing:
//!
//! ```text
//! assemble -> redact -> write the ledger entry -> issue the ticket
//!          -> the caller sends -> write the result
//! ```
//!
//! The record is written *before* the request is sent, so what it holds is
//! "the bytes we tried to send". Recording afterwards would leave a gap in
//! which bytes left the device and nothing said so.

use bytes::Bytes;
use genatrix_keys::TicketKey;
use genatrix_ledger::{Ledger, kind};
use genatrix_llm::registry::{Location, ModelEntry, Registry};
use genatrix_llm::ticket::{Purpose, Ticket, TicketLevel};
use genatrix_model::{ItemId, Level};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ulid::Ulid;

use crate::redact::{Identity, Redacted, redact};
use crate::rules::RuleSet;

/// Who asked for this call. Recorded, never trusted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Initiator {
    /// The user, directly.
    User,
    /// An agent run.
    Agent {
        /// The run it belongs to.
        run: String,
    },
    /// An installed functional agent (design 11).
    Installed {
        /// Which agent.
        agent: String,
        /// The hash of the version that ran.
        version: String,
        /// The run.
        run: String,
        /// Whether the user let this agent use a cloud model. Off unless
        /// switched on for this agent (design 11, ruling 12). It can only
        /// narrow where a call goes, never widen it.
        cloud: bool,
    },
    /// A pipeline or scheduled rule.
    Rule {
        /// Which one.
        name: String,
    },
}

/// One message in the prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// `system`, `user`, or `assistant`.
    pub role: String,
    /// The text.
    pub content: String,
}

impl Message {
    /// A system message.
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }

    /// A user message.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
}

/// What the agent layer hands to the gate.
#[derive(Clone, Debug)]
pub struct Request {
    /// Why this call is being made.
    pub purpose: Purpose,
    /// Who asked.
    pub initiator: Initiator,
    /// Items whose content went into the prompt. Provenance for the record.
    pub items: Vec<ItemId>,
    /// The highest level among those items and the user's own words.
    pub level: Level,
    /// People named in the prompt, for pseudonymization.
    pub identities: Vec<Identity>,
    /// The prompt.
    pub messages: Vec<Message>,
    /// Optional generation limits.
    pub max_tokens: Option<u32>,
    /// Optional sampling temperature.
    pub temperature: Option<f64>,
    /// Whether the answer should stream.
    pub stream: bool,
}

/// What the gate decided, as it goes into the ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Sent as-is to the chosen model. This covers an ordinary local call:
    /// with the cloud switched off, local is the configuration, not a
    /// fallback, and calling it one would drain the word of meaning.
    Allowed,
    /// Redacted, then sent to a cloud model.
    AllowedRedacted,
    /// A cloud model was available and switched on, but this content or this
    /// purpose was not allowed to use it, so a local model answered instead.
    /// This is the case the interface surfaces as "answered on this device,
    /// because the content is secret".
    FallbackLocal,
}

/// How a call ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// A reply came back.
    Sent {
        /// SHA-256 of the reply, hex.
        response_hash: String,
    },
    /// The request was made but the result is not known, for example the
    /// connection dropped after the bytes went out.
    Unknown {
        /// What happened.
        detail: String,
    },
    /// The request failed before or during sending.
    Failed {
        /// What happened.
        detail: String,
    },
}

/// The ledger record for one egress. Design 02's `EgressRecord`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressRecord {
    /// Who asked.
    pub initiator: Initiator,
    /// Why.
    pub purpose: String,
    /// Which items went in.
    pub items: Vec<String>,
    /// The level of the content.
    pub level: String,
    /// Registry name of the model.
    pub target: String,
    /// Whether that model is on this machine.
    pub location: String,
    /// What the gate decided.
    pub decision: Decision,
    /// SHA-256 of the bytes that will be sent, hex.
    pub payload_hash: String,
    /// The bytes themselves, redacted, only when they leave the device.
    /// A local call records the hash and the provenance but not a second
    /// copy of content that is already in the database.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
}

/// A prepared call: everything the caller needs to send it, and nothing that
/// would let them send something else.
#[derive(Debug)]
pub struct Prepared {
    /// Identifier of the ledger record, for reporting the outcome.
    pub egress_id: String,
    /// Registry name of the model to call.
    pub target: String,
    /// Where it runs.
    pub location: Location,
    /// The exact bytes to POST.
    pub body: Bytes,
    /// The ticket header value.
    pub ticket: String,
    /// What the gate decided.
    pub decision: Decision,
    /// The pseudonym map, when the prompt was redacted, so the reply can be
    /// restored on this machine.
    pub redaction: Option<Redacted>,
}

/// Things that stop a call before it starts.
#[derive(Debug, thiserror::Error)]
pub enum GateError {
    /// No model is configured that may serve this purpose at this level.
    #[error("no model available for purpose `{purpose}` at level `{level}`")]
    NoModel {
        /// The purpose asked for.
        purpose: &'static str,
        /// The level of the content.
        level: &'static str,
    },
    /// The ledger could not be written, so nothing is sent.
    #[error("ledger write failed, request not sent: {0}")]
    Ledger(#[from] genatrix_ledger::Error),
    /// The prompt could not be encoded.
    #[error("encoding the request failed: {0}")]
    Encode(#[from] serde_json::Error),
    /// The ticket could not be minted.
    #[error("minting the ticket failed: {0}")]
    Ticket(#[from] std::io::Error),
}

/// The gate.
pub struct EgressGate {
    rules: RuleSet,
    registry: Registry,
    ledger: std::sync::Arc<Ledger>,
    key: TicketKey,
    cloud_enabled: bool,
}

impl std::fmt::Debug for EgressGate {
    /// Deliberately partial: the ticket key never appears in a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EgressGate")
            .field("cloud_enabled", &self.cloud_enabled)
            .field("rules_version", &self.rules.version)
            .field("models", &self.registry.models.len())
            .finish_non_exhaustive()
    }
}

impl EgressGate {
    /// Build a gate. The cloud is off unless it is switched on here, which
    /// is design 02's default: a fresh install talks to nobody.
    #[must_use]
    pub fn new(
        rules: RuleSet,
        registry: Registry,
        ledger: std::sync::Arc<Ledger>,
        key: TicketKey,
    ) -> Self {
        Self {
            rules,
            registry,
            ledger,
            key,
            cloud_enabled: false,
        }
    }

    /// Switch the cloud on or off.
    #[must_use]
    pub fn with_cloud(mut self, enabled: bool) -> Self {
        self.cloud_enabled = enabled;
        self
    }

    /// Whether cloud models may be used at all.
    #[must_use]
    pub const fn cloud_enabled(&self) -> bool {
        self.cloud_enabled
    }

    /// The rules this gate judges with.
    #[must_use]
    pub const fn rules(&self) -> &RuleSet {
        &self.rules
    }

    /// The level of something the user typed.
    ///
    /// Design 02 asks for this explicitly: people paste verification codes
    /// into chat boxes, and a prompt is content like any other.
    #[must_use]
    pub fn level_of_typed_text(&self, text: &str) -> Level {
        if crate::patterns::scan(text)
            .iter()
            .any(|m| self.rules.pattern_is_enabled(m.kind))
        {
            Level::Secret
        } else {
            Level::default()
        }
    }

    /// Choose a model, redact if it is leaving the device, write the record,
    /// and mint a ticket bound to the exact bytes.
    pub fn prepare(&self, request: &Request) -> Result<Prepared, GateError> {
        // What the ticket would say if this went to the cloud. Used only to
        // ask the registry whether a cloud model is even a candidate.
        let prospective = match request.level {
            Level::Public => TicketLevel::Public,
            Level::Personal => TicketLevel::Redacted,
            Level::Secret => TicketLevel::Secret,
        };
        let preferred = self.registry.resolve(request.purpose, prospective);
        // An installed agent without its own cloud switch stays local,
        // whatever the global switch says.
        let agent_kept_local =
            matches!(request.initiator, Initiator::Installed { cloud: false, .. });
        let entry = match preferred {
            Some(e)
                if e.location() == Location::Cloud && (!self.cloud_enabled || agent_kept_local) =>
            {
                self.registry.resolve_local(request.purpose)
            }
            other => other,
        }
        .ok_or(GateError::NoModel {
            purpose: request.purpose.as_str(),
            level: request.level.as_str(),
        })?;

        let leaving = entry.location() == Location::Cloud;
        // Was the cloud a real option for this purpose, or is it simply off?
        let cloud_was_available = self.cloud_enabled
            && self.registry.chain(request.purpose).iter().any(|n| {
                self.registry
                    .get(n)
                    .is_some_and(|e| e.location() == Location::Cloud)
            });

        let (messages, redaction) = if leaving && request.level == Level::Personal {
            let mut out = Vec::with_capacity(request.messages.len());
            let mut map = None;
            for m in &request.messages {
                let r = redact(&m.content, &request.identities);
                // One pseudonym assignment per request: merge the maps, and
                // keep the first, since `redact` numbers from one each call.
                match &mut map {
                    None => map = Some(r.clone()),
                    Some(existing) => existing.pseudonyms.extend(r.pseudonyms.clone()),
                }
                out.push(Message {
                    role: m.role.clone(),
                    content: r.text,
                });
            }
            (out, map)
        } else {
            (request.messages.clone(), None)
        };

        let ticket_level = match (leaving, request.level) {
            (_, Level::Public) => TicketLevel::Public,
            (true, Level::Personal) => TicketLevel::Redacted,
            (false, Level::Personal) => TicketLevel::Personal,
            (_, Level::Secret) => TicketLevel::Secret,
        };
        let decision = if leaving {
            if ticket_level == TicketLevel::Redacted {
                Decision::AllowedRedacted
            } else {
                Decision::Allowed
            }
        } else if cloud_was_available {
            Decision::FallbackLocal
        } else {
            Decision::Allowed
        };

        let body = encode_body(entry, &messages, request)?;
        let payload_hash = hex::encode(Sha256::digest(&body));

        let egress_id = Ulid::new().to_string();
        let record = EgressRecord {
            initiator: request.initiator.clone(),
            purpose: request.purpose.as_str().to_owned(),
            items: request.items.iter().map(ToString::to_string).collect(),
            level: request.level.as_str().to_owned(),
            target: entry.name.clone(),
            location: if leaving { "cloud" } else { "local" }.to_owned(),
            decision,
            payload_hash: payload_hash.clone(),
            payload: leaving.then(|| String::from_utf8_lossy(&body).into_owned()),
        };
        // Before the ticket exists, so nothing can be sent that is not
        // already written down.
        self.ledger.append(kind::EGRESS, &egress_id, &record)?;

        let ticket = Ticket::issue(
            &body,
            entry.name.clone(),
            entry.model.clone(),
            request.purpose,
            ticket_level,
            caller_of(&request.initiator),
        )?
        .encode(&self.key);

        Ok(Prepared {
            egress_id,
            target: entry.name.clone(),
            location: entry.location(),
            body,
            ticket,
            decision,
            redaction,
        })
    }

    /// Record how a prepared call ended.
    pub fn record_outcome(&self, egress_id: &str, outcome: &Outcome) -> Result<(), GateError> {
        self.ledger
            .append(kind::EGRESS_RESULT, egress_id, outcome)?;
        Ok(())
    }

    /// Hash a reply, for [`Outcome::Sent`].
    #[must_use]
    pub fn hash_response(body: &[u8]) -> String {
        hex::encode(Sha256::digest(body))
    }
}

fn caller_of(initiator: &Initiator) -> String {
    match initiator {
        Initiator::User => "user".into(),
        Initiator::Agent { run } => format!("agent:{run}"),
        Initiator::Installed { agent, run, .. } => format!("installed:{agent}:{run}"),
        Initiator::Rule { name } => format!("rule:{name}"),
    }
}

fn encode_body(
    entry: &ModelEntry,
    messages: &[Message],
    request: &Request,
) -> Result<Bytes, serde_json::Error> {
    let mut body = serde_json::Map::new();
    // The registry name, not the upstream model: the gateway resolves it and
    // the ticket's target must match what the gateway routes to.
    body.insert("model".into(), entry.name.clone().into());
    if request.purpose == Purpose::Embed {
        // An embeddings request: the texts are the messages' contents, in
        // order. Nothing else applies to it.
        let input: Vec<&str> = messages.iter().map(|m| m.content.as_str()).collect();
        body.insert("input".into(), serde_json::to_value(input)?);
        return Ok(Bytes::from(serde_json::to_vec(&body)?));
    }
    body.insert("messages".into(), serde_json::to_value(messages)?);
    if let Some(t) = request.max_tokens {
        body.insert("max_tokens".into(), t.into());
    }
    if let Some(t) = request.temperature {
        body.insert("temperature".into(), t.into());
    }
    if request.stream {
        body.insert("stream".into(), true.into());
    }
    Ok(Bytes::from(serde_json::to_vec(&body)?))
}
