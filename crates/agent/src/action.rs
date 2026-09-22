//! Actions: the only way an effect reaches the world outside this machine,
//! and the only thing in the system that waits for a person.
//!
//! Design: `docs/design/03-agent-layer.md`, "动作".
//!
//! "Approval happens in the interface" is a product principle, not a
//! security design: the interface reaches the core through some endpoint, and
//! that endpoint is what has to be right. So approval is a state transition
//! with four conditions, all checked here:
//!
//! - it names a version, and that version's hash must match, so what was
//!   approved is what runs;
//! - editing the draft makes a new version, which silently voids an earlier
//!   approval rather than carrying it over;
//! - approval yields a one-time execution token, so a connector that has
//!   already run cannot run again;
//! - a pending action expires, so Monday's approval cannot execute on Friday.
//!
//! No effect crate can construct an approval; it arrives as a value from the
//! endpoint that authenticated the user.

use chrono::{DateTime, Duration, Utc};
use genatrix_model::ItemId;
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ulid::Ulid;

/// How long a proposal waits before it lapses.
pub const DEFAULT_TTL_DAYS: i64 = 3;

/// What an action would do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Effect {
    /// Send a mail.
    SendMail {
        /// Account to send from.
        account: String,
        /// Recipients.
        to: Vec<String>,
        /// Subject line.
        subject: String,
        /// Message this replies to, if any.
        in_reply_to: Option<String>,
        /// The conversation's `References` chain, ending with the message
        /// replied to, so the other side's client threads it (design 05).
        #[serde(default)]
        references: Vec<String>,
    },
    /// Send a chat message.
    SendMessage {
        /// Account to send from.
        account: String,
        /// Conversation to send to.
        chat: String,
        /// Message this replies to, if any.
        reply_to: Option<ItemId>,
    },
    /// Add a calendar event.
    CreateEvent {
        /// Calendar account.
        account: String,
    },
    /// Write something to the profile.
    ///
    /// Executed by a least-privilege writer inside the core, never by a
    /// connector: design 03 separates these because a connector that could
    /// also write memory would be a connector that could rewrite the user's
    /// picture of their own life.
    WriteMemory {
        /// Which part of the profile.
        collection: String,
    },
}

impl Effect {
    /// Stable name for records and for the interface.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SendMail { .. } => "send_mail",
            Self::SendMessage { .. } => "send_message",
            Self::CreateEvent { .. } => "create_event",
            Self::WriteMemory { .. } => "write_memory",
        }
    }

    /// The account this effect would act as, when it acts through a
    /// connector. A connector checks this against what it is allowed to do.
    #[must_use]
    pub fn account(&self) -> Option<&str> {
        match self {
            Self::SendMail { account, .. }
            | Self::SendMessage { account, .. }
            | Self::CreateEvent { account } => Some(account),
            Self::WriteMemory { .. } => None,
        }
    }

    /// Whether a connector may execute this, as opposed to the core.
    #[must_use]
    pub const fn runs_in_a_connector(&self) -> bool {
        !matches!(self, Self::WriteMemory { .. })
    }
}

/// Who wrote a version of the draft.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    /// The model proposed it.
    Model,
    /// The user edited it.
    User,
}

/// One immutable version of what would be sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    /// Position in the history, from 1.
    pub seq: u32,
    /// The content.
    pub payload: String,
    /// SHA-256 of the content, hex. What approval binds to.
    pub payload_hash: String,
    /// Who wrote it.
    pub author: Author,
    /// When.
    pub created_at: DateTime<Utc>,
}

impl Version {
    fn new(seq: u32, payload: String, author: Author) -> Self {
        let payload_hash = hex::encode(Sha256::digest(payload.as_bytes()));
        Self {
            seq,
            payload,
            payload_hash,
            author,
            created_at: Utc::now(),
        }
    }
}

/// A one-time permission to carry out an approved action.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionToken(String);

impl ExecutionToken {
    fn generate() -> Self {
        let mut bytes = [0u8; 32];
        if rand::rngs::OsRng.try_fill_bytes(&mut bytes).is_err() {
            // Without randomness there is no token worth issuing.
            return Self(String::new());
        }
        Self(hex::encode(bytes))
    }

    /// Whether this token is usable at all.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.0.len() == 64
    }

    /// A token that can never authorize anything. For tests and for the
    /// "no token was presented" path.
    #[must_use]
    pub fn default_invalid() -> Self {
        Self(String::new())
    }

    /// The token as text, for handing to the executor over the connector
    /// protocol. The only way out of the type.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// A token as the executor presents it.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        Self(text.to_owned())
    }
}

impl std::fmt::Debug for ExecutionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExecutionToken(..)")
    }
}

/// Where an action stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Status {
    /// Waiting for the user.
    Pending,
    /// Approved, not yet carried out.
    Approved {
        /// The version that was approved.
        version: u32,
    },
    /// The user said no.
    Declined {
        /// Why. Short is fine; the reason is the most valuable signal the
        /// profile gets (design 07).
        reason: String,
    },
    /// Carried out.
    Executed {
        /// The item the effect produced, once it comes back through sync.
        result: Option<ItemId>,
    },
    /// It was attempted and failed before anything went out.
    Failed {
        /// What happened.
        detail: String,
    },
    /// It was submitted and the outcome is not known.
    ///
    /// A mail handed to a server that then dropped the connection is here,
    /// not in `Failed`. Design 05 is firm about this: a vague failure that
    /// tempts the user to press send again is worse than an honest
    /// "possibly sent, waiting for sync to confirm".
    Unknown {
        /// What happened.
        detail: String,
    },
    /// Nobody looked at it in time.
    Expired,
}

/// A proposed effect, its drafts, and where it stands.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Action {
    /// Identifier.
    pub id: String,
    /// The run that proposed it.
    pub run_id: String,
    /// What it would do.
    pub effect: Effect,
    /// The drafts, oldest first. Never fewer than one.
    pub versions: Vec<Version>,
    /// Why the model suggests it, in a sentence.
    pub rationale: String,
    /// What it read to decide. Shown above the draft, because seeing the
    /// reasoning before the words is what makes approval a judgement rather
    /// than a reflex (design 06).
    pub evidence: Vec<ItemId>,
    /// Where it stands.
    pub status: Status,
    /// When it was proposed.
    pub created_at: DateTime<Utc>,
    /// When the user decided.
    pub decided_at: Option<DateTime<Utc>>,
    /// When it lapses if nobody does.
    pub expires_at: DateTime<Utc>,
    /// The live execution token, present only while approved and unused.
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<ExecutionToken>,
    /// When the token was spent, that is, when a connector took the action
    /// to carry it out. A connector that then never reports leaves the
    /// action here; after a while the core calls that outcome unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handed_at: Option<DateTime<Utc>>,
    /// For a mail whose outcome is unknown: the `Message-ID` it was
    /// submitted under, so that a later sync can confirm it went out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_as: Option<String>,
}

/// Why a transition was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ActionError {
    /// The action is not in a state where this makes sense.
    #[error("action is {found}, not {expected}")]
    WrongStatus {
        /// Where it is.
        found: &'static str,
        /// Where it would need to be.
        expected: &'static str,
    },
    /// The approval named a version that does not exist.
    #[error("no version {0}")]
    NoSuchVersion(u32),
    /// The approval named a version whose content has changed since.
    #[error("version {version} does not have the approved content")]
    VersionChanged {
        /// Which version.
        version: u32,
    },
    /// The action lapsed.
    #[error("action expired at {at}")]
    Expired {
        /// When.
        at: DateTime<Utc>,
    },
    /// The execution token is wrong, missing, or already used.
    #[error("execution token is not valid for this action")]
    BadToken,
}

/// What the approval endpoint passes in.
///
/// It carries the version and its hash because the user approved a specific
/// draft on screen. If an edit landed in between, the hash will not match and
/// the approval is refused rather than applied to different words.
#[derive(Clone, Debug)]
pub struct Approval {
    /// Version the user was looking at.
    pub version: u32,
    /// Hash of the content they were looking at.
    pub payload_hash: String,
}

impl Action {
    /// Propose an action. It starts pending with one model-written draft.
    #[must_use]
    pub fn propose(
        run_id: impl Into<String>,
        effect: Effect,
        payload: impl Into<String>,
        rationale: impl Into<String>,
        evidence: Vec<ItemId>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Ulid::new().to_string(),
            run_id: run_id.into(),
            effect,
            versions: vec![Version::new(1, payload.into(), Author::Model)],
            rationale: rationale.into(),
            evidence,
            status: Status::Pending,
            created_at: now,
            decided_at: None,
            expires_at: now + Duration::days(DEFAULT_TTL_DAYS),
            token: None,
            handed_at: None,
            submitted_as: None,
        }
    }

    /// The token issued at approval, while it is unspent. The core hands
    /// it to the connector once; `begin_execution` then spends it.
    #[must_use]
    pub fn token_for_execution(&self) -> Option<ExecutionToken> {
        self.token.clone().filter(ExecutionToken::is_valid)
    }

    /// The most recent draft.
    ///
    /// # Panics
    ///
    /// Never: an action always has at least one version.
    #[must_use]
    pub fn current(&self) -> &Version {
        self.versions
            .last()
            .expect("an action always has a version")
    }

    /// Record an edit. This voids any approval: what was approved is not
    /// what would now be sent.
    pub fn edit(&mut self, payload: impl Into<String>) -> Result<&Version, ActionError> {
        match self.status {
            Status::Pending | Status::Approved { .. } => {}
            _ => {
                return Err(ActionError::WrongStatus {
                    found: status_name(&self.status),
                    expected: "pending or approved",
                });
            }
        }
        let seq = self.current().seq + 1;
        self.versions
            .push(Version::new(seq, payload.into(), Author::User));
        self.status = Status::Pending;
        self.token = None;
        self.decided_at = None;
        Ok(self.current())
    }

    /// Approve the version the user was looking at.
    pub fn approve(
        &mut self,
        approval: &Approval,
        now: DateTime<Utc>,
    ) -> Result<ExecutionToken, ActionError> {
        if !matches!(self.status, Status::Pending) {
            return Err(ActionError::WrongStatus {
                found: status_name(&self.status),
                expected: "pending",
            });
        }
        if now >= self.expires_at {
            self.status = Status::Expired;
            return Err(ActionError::Expired {
                at: self.expires_at,
            });
        }
        let version = self
            .versions
            .iter()
            .find(|v| v.seq == approval.version)
            .ok_or(ActionError::NoSuchVersion(approval.version))?;
        if version.payload_hash != approval.payload_hash {
            return Err(ActionError::VersionChanged {
                version: approval.version,
            });
        }
        if version.seq != self.current().seq {
            // Approving an older draft after an edit would send the wrong
            // words even though the hash checks out.
            return Err(ActionError::VersionChanged {
                version: approval.version,
            });
        }
        let token = ExecutionToken::generate();
        self.token = Some(token.clone());
        self.status = Status::Approved {
            version: version.seq,
        };
        self.decided_at = Some(now);
        Ok(token)
    }

    /// Decline, with a reason.
    pub fn decline(
        &mut self,
        reason: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<(), ActionError> {
        if !matches!(self.status, Status::Pending | Status::Approved { .. }) {
            return Err(ActionError::WrongStatus {
                found: status_name(&self.status),
                expected: "pending or approved",
            });
        }
        self.status = Status::Declined {
            reason: reason.into(),
        };
        self.token = None;
        self.decided_at = Some(now);
        Ok(())
    }

    /// Lapse the action if its time has passed. Returns whether it did.
    pub fn expire_if_due(&mut self, now: DateTime<Utc>) -> bool {
        if matches!(self.status, Status::Pending) && now >= self.expires_at {
            self.status = Status::Expired;
            self.token = None;
            return true;
        }
        false
    }

    /// Check everything an executor must check, and spend the token.
    ///
    /// Returns the exact version to send. Design 05 requires the connector to
    /// re-check the account as well; that check belongs there because only
    /// the connector knows what it was granted.
    pub fn begin_execution(
        &mut self,
        token: &ExecutionToken,
        now: DateTime<Utc>,
    ) -> Result<Version, ActionError> {
        let Status::Approved { version } = &self.status else {
            return Err(ActionError::WrongStatus {
                found: status_name(&self.status),
                expected: "approved",
            });
        };
        let version = *version;
        if now >= self.expires_at {
            self.status = Status::Expired;
            self.token = None;
            return Err(ActionError::Expired {
                at: self.expires_at,
            });
        }
        let held = self.token.as_ref().ok_or(ActionError::BadToken)?;
        if !token.is_valid() || held != token {
            return Err(ActionError::BadToken);
        }
        let version = self
            .versions
            .iter()
            .find(|v| v.seq == version)
            .ok_or(ActionError::NoSuchVersion(version))?
            .clone();
        // Spent: a second attempt with the same token finds nothing.
        self.token = None;
        self.handed_at = Some(now);
        Ok(version)
    }

    /// Whether a connector holds this action right now: approved, token
    /// spent, no report yet.
    #[must_use]
    pub const fn is_in_a_connectors_hands(&self) -> bool {
        matches!(self.status, Status::Approved { .. }) && self.token.is_none()
    }

    /// The user takes an approval back before a connector has acted on it.
    ///
    /// Possible only while the token is unspent: once a connector holds the
    /// action the words may already be on their way, and the honest state
    /// then is whatever the connector reports, not a withdrawal that would
    /// pretend otherwise.
    pub fn withdraw(&mut self, reason: &str, now: DateTime<Utc>) -> Result<(), ActionError> {
        if !matches!(self.status, Status::Approved { .. }) {
            return Err(ActionError::WrongStatus {
                found: status_name(&self.status),
                expected: "approved",
            });
        }
        if self.token.is_none() {
            return Err(ActionError::WrongStatus {
                found: "in a connector's hands",
                expected: "approved and not yet taken",
            });
        }
        self.token = None;
        self.status = Status::Declined {
            reason: reason.to_owned(),
        };
        self.decided_at = Some(now);
        Ok(())
    }

    /// Record how execution ended.
    pub fn finish(&mut self, status: Status) -> Result<(), ActionError> {
        match status {
            Status::Executed { .. } | Status::Failed { .. } | Status::Unknown { .. } => {
                self.status = status;
                self.token = None;
                Ok(())
            }
            _ => Err(ActionError::WrongStatus {
                found: status_name(&status),
                expected: "executed, failed or unknown",
            }),
        }
    }
}

const fn status_name(status: &Status) -> &'static str {
    match status {
        Status::Pending => "pending",
        Status::Approved { .. } => "approved",
        Status::Declined { .. } => "declined",
        Status::Executed { .. } => "executed",
        Status::Failed { .. } => "failed",
        Status::Unknown { .. } => "unknown",
        Status::Expired => "expired",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action() -> Action {
        Action::propose(
            "run-1",
            Effect::SendMail {
                account: "me@example.com".into(),
                to: vec!["maria@example.com".into()],
                subject: "Re: proposal".into(),
                in_reply_to: Some("<abc@example.com>".into()),
                references: vec![],
            },
            "Maria, confirmed on both points.",
            "She is waiting on clause 4 and you stated your position last week.",
            vec![ItemId::new()],
        )
    }

    fn approval_of(a: &Action) -> Approval {
        Approval {
            version: a.current().seq,
            payload_hash: a.current().payload_hash.clone(),
        }
    }

    #[test]
    fn an_approval_can_be_withdrawn_until_a_connector_takes_it() {
        let now = Utc::now();
        let mut a = action();
        assert!(
            a.withdraw("changed my mind", now).is_err(),
            "nothing to withdraw while pending"
        );
        let token = a.approve(&approval_of(&a), now).unwrap();
        let mut held = a.clone();
        a.withdraw("changed my mind", now).unwrap();
        assert!(matches!(a.status, Status::Declined { ref reason } if reason == "changed my mind"));
        assert!(a.token_for_execution().is_none(), "the token dies with it");

        held.begin_execution(&token, now).unwrap();
        assert!(held.is_in_a_connectors_hands());
        assert_eq!(held.handed_at, Some(now));
        assert!(
            held.withdraw("too late", now).is_err(),
            "once handed out the words may be on their way; only the report can say"
        );
    }

    #[test]
    fn a_proposal_starts_pending_with_one_model_draft() {
        let a = action();
        assert!(matches!(a.status, Status::Pending));
        assert_eq!(a.versions.len(), 1);
        assert_eq!(a.current().author, Author::Model);
        assert_eq!(a.effect.kind(), "send_mail");
        assert_eq!(a.effect.account(), Some("me@example.com"));
        assert!(a.effect.runs_in_a_connector());
    }

    #[test]
    fn approval_binds_to_the_content_on_screen() {
        let mut a = action();
        let token = a.approve(&approval_of(&a), Utc::now()).unwrap();
        assert!(token.is_valid());
        assert!(matches!(a.status, Status::Approved { version: 1 }));
    }

    #[test]
    fn an_approval_for_different_content_is_refused() {
        let mut a = action();
        let stale = Approval {
            version: 1,
            payload_hash: "0".repeat(64),
        };
        assert_eq!(
            a.approve(&stale, Utc::now()),
            Err(ActionError::VersionChanged { version: 1 })
        );
        assert!(matches!(a.status, Status::Pending), "and nothing changed");
    }

    #[test]
    fn editing_voids_an_approval_that_was_already_given() {
        let mut a = action();
        let token = a.approve(&approval_of(&a), Utc::now()).unwrap();
        a.edit("Maria, confirmed on clause 4 only.").unwrap();
        assert!(matches!(a.status, Status::Pending));
        assert_eq!(a.versions.len(), 2);
        assert_eq!(a.current().author, Author::User);
        assert_eq!(
            a.begin_execution(&token, Utc::now()),
            Err(ActionError::WrongStatus {
                found: "pending",
                expected: "approved"
            }),
            "the old token is worthless"
        );
    }

    #[test]
    fn approving_a_superseded_draft_is_refused() {
        let mut a = action();
        let old = approval_of(&a);
        a.edit("a different message").unwrap();
        assert_eq!(
            a.approve(&old, Utc::now()),
            Err(ActionError::VersionChanged { version: 1 })
        );
    }

    #[test]
    fn a_token_works_once() {
        let mut a = action();
        let token = a.approve(&approval_of(&a), Utc::now()).unwrap();
        let version = a.begin_execution(&token, Utc::now()).unwrap();
        assert_eq!(version.payload, "Maria, confirmed on both points.");
        assert_eq!(
            a.begin_execution(&token, Utc::now()),
            Err(ActionError::BadToken),
            "no send-twice"
        );
    }

    #[test]
    fn another_token_does_not_work() {
        let mut a = action();
        a.approve(&approval_of(&a), Utc::now()).unwrap();
        let forged = ExecutionToken::generate();
        assert_eq!(
            a.begin_execution(&forged, Utc::now()),
            Err(ActionError::BadToken)
        );
    }

    #[test]
    fn an_approval_cannot_execute_after_the_action_lapses() {
        let mut a = action();
        let token = a.approve(&approval_of(&a), Utc::now()).unwrap();
        let friday = a.expires_at + Duration::hours(1);
        assert!(matches!(
            a.begin_execution(&token, friday),
            Err(ActionError::Expired { .. })
        ));
        assert!(matches!(a.status, Status::Expired));
    }

    #[test]
    fn a_pending_action_lapses_on_its_own() {
        let mut a = action();
        assert!(!a.expire_if_due(Utc::now()));
        assert!(a.expire_if_due(a.expires_at));
        assert!(matches!(a.status, Status::Expired));
        assert!(a.approve(&approval_of(&a), Utc::now()).is_err());
    }

    #[test]
    fn declining_keeps_the_reason() {
        let mut a = action();
        a.decline("too long", Utc::now()).unwrap();
        assert_eq!(
            a.status,
            Status::Declined {
                reason: "too long".into()
            }
        );
        assert!(a.decline("again", Utc::now()).is_err());
    }

    #[test]
    fn an_unclear_send_is_unknown_not_failed() {
        let mut a = action();
        let token = a.approve(&approval_of(&a), Utc::now()).unwrap();
        a.begin_execution(&token, Utc::now()).unwrap();
        a.finish(Status::Unknown {
            detail: "connection dropped after submission".into(),
        })
        .unwrap();
        assert!(matches!(a.status, Status::Unknown { .. }));
    }

    #[test]
    fn writing_memory_never_runs_in_a_connector() {
        let effect = Effect::WriteMemory {
            collection: "facts".into(),
        };
        assert!(!effect.runs_in_a_connector());
        assert_eq!(effect.account(), None);
    }

    #[test]
    fn a_token_never_prints_itself() {
        let token = ExecutionToken::generate();
        assert_eq!(format!("{token:?}"), "ExecutionToken(..)");
    }
}
