//! Egress tickets: a signed permission to send exactly these bytes to
//! exactly this target, once, within the next few seconds.
//!
//! Design: `docs/design/02-trust-boundary.md`, "出境闸门".
//!
//! A ticket that only said "personal, redacted" would let any component
//! holding one send *different* bytes; the gateway could not tell. So the
//! ticket commits to the hash of the exact payload, names the target, and
//! carries a nonce that the gateway will accept only once.

use std::collections::HashMap;
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use chrono::{DateTime, Duration, Utc};
use genatrix_keys::TicketKey;
use hmac::{Hmac, Mac};
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// How long a ticket is valid. Seconds: it is minted immediately before the
/// request is handed to the gateway.
pub const DEFAULT_TTL_SECS: i64 = 30;

/// Why a request is being made. A closed set: a new purpose is a design
/// change first, and its policy is decided there, not in code (design 02).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    /// Sensitivity judgement. Reads everything; never leaves the device.
    Classify,
    /// Structured extraction: commitments, dates, people. Never leaves.
    Extract,
    /// Vectorization. Never leaves.
    Embed,
    /// Suggesting that two handles are the same person. Never leaves.
    IdentitySuggestion,
    /// Summarizing one item or a group.
    Summarize,
    /// Drafting a reply.
    Draft,
    /// Translating content.
    Translate,
    /// Rewriting a search query.
    SearchRewrite,
    /// Planning inside a conversation.
    Plan,
}

impl Purpose {
    /// Stable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Classify => "classify",
            Self::Extract => "extract",
            Self::Embed => "embed",
            Self::IdentitySuggestion => "identity_suggestion",
            Self::Summarize => "summarize",
            Self::Draft => "draft",
            Self::Translate => "translate",
            Self::SearchRewrite => "search_rewrite",
            Self::Plan => "plan",
        }
    }

    /// Whether this purpose may ever be served by a cloud provider.
    ///
    /// The four that read whole corpora never may. This is not a setting.
    #[must_use]
    pub const fn may_use_cloud(self) -> bool {
        match self {
            Self::Classify | Self::Extract | Self::Embed | Self::IdentitySuggestion => false,
            Self::Summarize | Self::Draft | Self::Translate | Self::SearchRewrite | Self::Plan => {
                true
            }
        }
    }
}

/// The sensitivity of the bytes a ticket covers.
///
/// `Redacted` is not a level of the data model; it records that content
/// which was `Personal` has been through redaction. Only `Public` and
/// `Redacted` may reach a cloud provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketLevel {
    /// Public content, sent as-is.
    Public,
    /// Personal content that has been redacted.
    Redacted,
    /// Personal content, not redacted.
    Personal,
    /// Secret content.
    Secret,
}

impl TicketLevel {
    /// Stable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Redacted => "redacted",
            Self::Personal => "personal",
            Self::Secret => "secret",
        }
    }

    /// Whether content at this level may leave the device.
    #[must_use]
    pub const fn may_leave_device(self) -> bool {
        matches!(self, Self::Public | Self::Redacted)
    }
}

/// A signed, single-use permission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    /// SHA-256 of the exact request body the gateway will forward.
    #[serde(with = "hex_bytes")]
    pub payload_hash: [u8; 32],
    /// Registry name of the target provider.
    pub target: String,
    /// Model asked for.
    pub model: String,
    /// Why.
    pub purpose: Purpose,
    /// Sensitivity of the payload.
    pub level: TicketLevel,
    /// Which component asked. Recorded, not trusted.
    pub caller: String,
    /// Single-use marker.
    #[serde(with = "hex_bytes_16")]
    pub nonce: [u8; 16],
    /// After this instant the ticket is worthless.
    pub expires_at: DateTime<Utc>,
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(d)?;
        let mut out = [0u8; 32];
        hex::decode_to_slice(&s, &mut out).map_err(serde::de::Error::custom)?;
        Ok(out)
    }
}

mod hex_bytes_16 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8; 16], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 16], D::Error> {
        let s = String::deserialize(d)?;
        let mut out = [0u8; 16];
        hex::decode_to_slice(&s, &mut out).map_err(serde::de::Error::custom)?;
        Ok(out)
    }
}

/// Why a ticket was refused. The gateway turns every one of these into a
/// refusal; none of them is recoverable by retrying with the same ticket.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TicketError {
    /// No ticket on the request.
    #[error("no egress ticket")]
    Missing,
    /// The header could not be decoded.
    #[error("malformed egress ticket: {0}")]
    Malformed(String),
    /// The signature does not match this key.
    #[error("egress ticket signature is not valid")]
    BadSignature,
    /// The ticket's validity window has passed.
    #[error("egress ticket expired")]
    Expired,
    /// The nonce has been used already.
    #[error("egress ticket already used")]
    Replayed,
    /// The bytes do not hash to what the ticket promised.
    #[error("request body does not match the ticket")]
    PayloadMismatch,
    /// The ticket was issued for a different provider.
    #[error("egress ticket names target `{expected}`, request went to `{actual}`")]
    WrongTarget {
        /// What the ticket says.
        expected: String,
        /// Where the request was routed.
        actual: String,
    },
}

impl Ticket {
    /// Mint a ticket for a payload. Called by the egress gate, after
    /// redaction and after the ledger entry has been written.
    pub fn issue(
        payload: &[u8],
        target: impl Into<String>,
        model: impl Into<String>,
        purpose: Purpose,
        level: TicketLevel,
        caller: impl Into<String>,
    ) -> std::io::Result<Self> {
        let mut nonce = [0u8; 16];
        rand::rngs::OsRng
            .try_fill_bytes(&mut nonce)
            .map_err(std::io::Error::other)?;
        Ok(Self {
            payload_hash: Sha256::digest(payload).into(),
            target: target.into(),
            model: model.into(),
            purpose,
            level,
            caller: caller.into(),
            nonce,
            expires_at: Utc::now() + Duration::seconds(DEFAULT_TTL_SECS),
        })
    }

    /// Serialize and sign. The result goes in a request header.
    ///
    /// # Panics
    ///
    /// If the ticket cannot be serialized, which cannot happen for this type.
    #[must_use]
    pub fn encode(&self, key: &TicketKey) -> String {
        let json = serde_json::to_vec(self).expect("ticket is serializable");
        let mut mac =
            HmacSha256::new_from_slice(key.as_bytes()).expect("hmac accepts any key size");
        mac.update(&json);
        let sig = mac.finalize().into_bytes();
        format!("{}.{}", B64.encode(&json), B64.encode(sig))
    }

    /// Decode and check the signature. Does not check expiry, replay, or
    /// payload: [`TicketStore::admit`] does all of that together.
    ///
    /// # Errors
    ///
    /// If the header is not two base64 parts, or the signature does not match.
    ///
    /// # Panics
    ///
    /// If HMAC rejects the key length, which cannot happen for a 32-byte key.
    pub fn decode(header: &str, key: &TicketKey) -> Result<Self, TicketError> {
        let (json_b64, sig_b64) = header
            .split_once('.')
            .ok_or_else(|| TicketError::Malformed("expected two dot-separated parts".into()))?;
        let json = B64
            .decode(json_b64)
            .map_err(|e| TicketError::Malformed(e.to_string()))?;
        let sig = B64
            .decode(sig_b64)
            .map_err(|e| TicketError::Malformed(e.to_string()))?;
        let mut mac =
            HmacSha256::new_from_slice(key.as_bytes()).expect("hmac accepts any key size");
        mac.update(&json);
        let expected = mac.finalize().into_bytes();
        if expected.ct_eq(&sig).unwrap_u8() != 1 {
            return Err(TicketError::BadSignature);
        }
        serde_json::from_slice(&json).map_err(|e| TicketError::Malformed(e.to_string()))
    }
}

/// Remembers spent nonces so a ticket works exactly once.
///
/// Entries are dropped once their ticket could no longer be valid anyway,
/// so the map stays the size of a few seconds of traffic.
#[derive(Debug, Default)]
pub struct TicketStore {
    seen: Mutex<HashMap<[u8; 16], DateTime<Utc>>>,
}

impl TicketStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Check a ticket against the request that carries it and spend it.
    ///
    /// Every condition from design 02 in one place: signature, expiry,
    /// replay, payload, target. On success the nonce is consumed, so a
    /// second call with the same ticket fails.
    pub fn admit(
        &self,
        header: Option<&str>,
        body: &[u8],
        routed_target: &str,
        key: &TicketKey,
    ) -> Result<Ticket, TicketError> {
        let header = header.ok_or(TicketError::Missing)?;
        let ticket = Ticket::decode(header, key)?;
        let now = Utc::now();
        if ticket.expires_at <= now {
            return Err(TicketError::Expired);
        }
        let actual: [u8; 32] = Sha256::digest(body).into();
        if actual.ct_eq(&ticket.payload_hash).unwrap_u8() != 1 {
            return Err(TicketError::PayloadMismatch);
        }
        if ticket.target != routed_target {
            return Err(TicketError::WrongTarget {
                expected: ticket.target.clone(),
                actual: routed_target.to_owned(),
            });
        }
        let mut seen = self
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        seen.retain(|_, exp| *exp > now);
        if seen.insert(ticket.nonce, ticket.expires_at).is_some() {
            return Err(TicketError::Replayed);
        }
        Ok(ticket)
    }

    /// Number of nonces currently remembered. For tests and diagnostics.
    pub fn remembered(&self) -> usize {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> TicketKey {
        TicketKey::from_bytes([9; 32])
    }

    fn ticket(body: &[u8]) -> Ticket {
        Ticket::issue(
            body,
            "anthropic",
            "claude-sonnet-5",
            Purpose::Summarize,
            TicketLevel::Redacted,
            "gate",
        )
        .unwrap()
    }

    #[test]
    fn a_good_ticket_is_admitted_once() {
        let store = TicketStore::new();
        let body = br#"{"model":"claude-sonnet-5"}"#;
        let header = ticket(body).encode(&key());
        let admitted = store
            .admit(Some(&header), body, "anthropic", &key())
            .unwrap();
        assert_eq!(admitted.purpose, Purpose::Summarize);
        assert_eq!(
            store.admit(Some(&header), body, "anthropic", &key()),
            Err(TicketError::Replayed)
        );
    }

    #[test]
    fn different_bytes_are_refused() {
        let store = TicketStore::new();
        let header = ticket(b"the payload the gate approved").encode(&key());
        assert_eq!(
            store.admit(
                Some(&header),
                b"something else entirely",
                "anthropic",
                &key()
            ),
            Err(TicketError::PayloadMismatch)
        );
    }

    #[test]
    fn a_ticket_for_one_provider_does_not_work_for_another() {
        let store = TicketStore::new();
        let body = b"payload";
        let header = ticket(body).encode(&key());
        let err = store
            .admit(Some(&header), body, "openai", &key())
            .unwrap_err();
        assert!(matches!(err, TicketError::WrongTarget { .. }), "{err:?}");
    }

    #[test]
    fn another_key_cannot_mint_tickets() {
        let store = TicketStore::new();
        let body = b"payload";
        let forged = ticket(body).encode(&TicketKey::from_bytes([1; 32]));
        assert_eq!(
            store.admit(Some(&forged), body, "anthropic", &key()),
            Err(TicketError::BadSignature)
        );
    }

    #[test]
    fn tampering_with_the_claims_breaks_the_signature() {
        let body = b"payload";
        let header = ticket(body).encode(&key());
        let (json_b64, sig) = header.split_once('.').unwrap();
        let mut claims: serde_json::Value =
            serde_json::from_slice(&B64.decode(json_b64).unwrap()).unwrap();
        claims["level"] = serde_json::json!("redacted");
        claims["target"] = serde_json::json!("openai");
        let forged = format!("{}.{sig}", B64.encode(serde_json::to_vec(&claims).unwrap()));
        assert_eq!(
            TicketStore::new().admit(Some(&forged), body, "openai", &key()),
            Err(TicketError::BadSignature)
        );
    }

    #[test]
    fn an_expired_ticket_is_refused_and_pruned() {
        let store = TicketStore::new();
        let body = b"payload";
        let mut t = ticket(body);
        t.expires_at = Utc::now() - Duration::seconds(1);
        let header = t.encode(&key());
        assert_eq!(
            store.admit(Some(&header), body, "anthropic", &key()),
            Err(TicketError::Expired)
        );
        assert_eq!(store.remembered(), 0, "expired tickets do not fill the map");
    }

    #[test]
    fn missing_and_malformed_headers_are_refused() {
        let store = TicketStore::new();
        assert_eq!(
            store.admit(None, b"x", "anthropic", &key()),
            Err(TicketError::Missing)
        );
        assert!(matches!(
            store.admit(Some("not-a-ticket"), b"x", "anthropic", &key()),
            Err(TicketError::Malformed(_))
        ));
        assert!(matches!(
            store.admit(Some("!!!.???"), b"x", "anthropic", &key()),
            Err(TicketError::Malformed(_))
        ));
    }

    #[test]
    fn corpus_reading_purposes_may_never_use_cloud() {
        for p in [
            Purpose::Classify,
            Purpose::Extract,
            Purpose::Embed,
            Purpose::IdentitySuggestion,
        ] {
            assert!(!p.may_use_cloud(), "{p:?}");
        }
        for p in [Purpose::Summarize, Purpose::Draft, Purpose::Translate] {
            assert!(p.may_use_cloud(), "{p:?}");
        }
    }

    #[test]
    fn only_public_and_redacted_may_leave() {
        assert!(TicketLevel::Public.may_leave_device());
        assert!(TicketLevel::Redacted.may_leave_device());
        assert!(!TicketLevel::Personal.may_leave_device());
        assert!(!TicketLevel::Secret.may_leave_device());
    }
}
