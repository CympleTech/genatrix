//! Actions in the core: proposed by a pipeline, decided by the user,
//! handed to a connector, and every step of it recorded.
//!
//! Design: `docs/design/03-agent-layer.md`, "动作"; `docs/design/06-interface.md`,
//! "审批面板". The type and its transitions live in the agent crate; this
//! module is what wraps them in the store and the ledger, and what the
//! approval endpoint and the connectors talk to.
//!
//! The approval endpoint is local only and carries a nonce: a page that
//! showed the user an action got a nonce with it, and an approval has to
//! present that nonce along with the version and its hash. What was seen is
//! what is approved; what is approved is what runs; and a request that did
//! not come from a page that showed the action is refused.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use genatrix_agent::action::{Action, Approval, Effect, ExecutionToken, Status, Version};
use genatrix_ledger::{Ledger, kind};
use genatrix_model::ItemId;
use genatrix_store::{ActionColumns, Store};
use serde::{Deserialize, Serialize};

/// What the ledger keeps about an action, at each step.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionRecord {
    /// The action.
    pub action: String,
    /// `proposed`, `edited`, `approved`, `declined`, `expired`, `executing`,
    /// `executed`, `failed`, `unknown`.
    pub event: String,
    /// The version concerned.
    pub version: u32,
    /// Its hash, so the record says what was seen.
    pub payload_hash: String,
    /// `model`, `user`, `connector`, `core`.
    pub by: String,
    /// The kind of effect.
    pub kind: String,
    /// A reason or result, in words.
    #[serde(default)]
    pub detail: String,
}

/// The store and the ledger, for actions.
pub struct Actions {
    nonces: Mutex<HashMap<String, String>>,
}

impl Default for Actions {
    fn default() -> Self {
        Self {
            nonces: Mutex::new(HashMap::new()),
        }
    }
}

/// What can go wrong deciding an action.
#[derive(Debug, thiserror::Error)]
pub enum DecisionError {
    /// No such action.
    #[error("no such action")]
    NotFound,
    /// The nonce does not match what the page was given.
    #[error("this approval did not come from the page that showed the action")]
    BadNonce,
    /// The action's own rules refused.
    #[error(transparent)]
    Action(#[from] genatrix_agent::action::ActionError),
    /// Storage.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

fn status_name(status: &Status) -> &'static str {
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

/// Status name, for the interface.
#[must_use]
pub fn status_of(action: &Action) -> &'static str {
    status_name(&action.status)
}

// The handle exists so that every decision goes through one place that also
// holds the page nonces; the reads and the proposal do not touch the nonces,
// and are methods all the same so callers hold one thing.
#[allow(clippy::unused_self)]
impl Actions {
    /// Write the action as it is now.
    fn save(store: &Store, action: &Action) -> anyhow::Result<()> {
        let value = serde_json::to_string(action)?;
        store.put_action(
            &ActionColumns {
                id: &action.id,
                run_id: &action.run_id,
                kind: action.effect.kind(),
                account: action.effect.account(),
                status: status_of(action),
                created_at: &action.created_at.to_rfc3339(),
                expires_at: &action.expires_at.to_rfc3339(),
            },
            &value,
        )?;
        Ok(())
    }

    fn record(
        ledger: &Ledger,
        action: &Action,
        event: &str,
        by: &str,
        detail: impl Into<String>,
    ) -> anyhow::Result<()> {
        let current = action.current();
        ledger.append(
            kind::ACTION,
            &action.id,
            &ActionRecord {
                action: action.id.clone(),
                event: event.to_owned(),
                version: current.seq,
                payload_hash: current.payload_hash.clone(),
                by: by.to_owned(),
                kind: action.effect.kind().to_owned(),
                detail: detail.into(),
            },
        )?;
        Ok(())
    }

    /// A pipeline proposed something. Stored and recorded.
    #[allow(clippy::too_many_arguments)] // mirrors `Action::propose`, plus the two stores
    pub fn propose(
        &self,
        store: &Store,
        ledger: &Ledger,
        run_id: &str,
        effect: Effect,
        payload: String,
        rationale: String,
        evidence: Vec<ItemId>,
    ) -> anyhow::Result<Action> {
        let action = Action::propose(run_id, effect, payload, rationale, evidence);
        Self::save(store, &action)?;
        Self::record(ledger, &action, "proposed", "model", "")?;
        Ok(action)
    }

    /// One action, if it exists.
    pub fn get(&self, store: &Store, id: &str) -> anyhow::Result<Option<Action>> {
        let Some(stored) = store.get_action(id)? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(&stored.value)?))
    }

    /// Actions to show, newest first.
    pub fn list(
        &self,
        store: &Store,
        status: Option<&str>,
        limit: u32,
    ) -> anyhow::Result<Vec<Action>> {
        store
            .list_actions(status, limit)?
            .into_iter()
            .map(|s| Ok(serde_json::from_str(&s.value)?))
            .collect()
    }

    /// A nonce for a page that is about to show this action. Presenting it
    /// back is part of approving.
    pub fn nonce_for(&self, action_id: &str) -> String {
        let nonce = format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new());
        self.nonces
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(action_id.to_owned(), nonce.clone());
        nonce
    }

    fn nonce_matches(&self, action_id: &str, nonce: &str) -> bool {
        self.nonces
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(action_id)
            .is_some_and(|n| n == nonce)
    }

    fn take_nonce(&self, action_id: &str) -> Option<String> {
        self.nonces
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(action_id)
    }

    /// The user changed the draft: a new version, and any earlier approval
    /// is void (design 03).
    pub fn edit(
        &self,
        store: &Store,
        ledger: &Ledger,
        id: &str,
        payload: String,
    ) -> Result<Action, DecisionError> {
        let mut action = self.get(store, id)?.ok_or(DecisionError::NotFound)?;
        action.edit(payload)?;
        Self::save(store, &action)?;
        Self::record(ledger, &action, "edited", "user", "")?;
        Ok(action)
    }

    /// The user approved this version of this action, from the page that
    /// showed it. Yields an execution token, kept with the action for the
    /// connector.
    pub fn approve(
        &self,
        store: &Store,
        ledger: &Ledger,
        id: &str,
        approval: &Approval,
        nonce: &str,
    ) -> Result<Action, DecisionError> {
        let mut action = self.get(store, id)?.ok_or(DecisionError::NotFound)?;
        if nonce.is_empty() || !self.nonce_matches(id, nonce) {
            return Err(DecisionError::BadNonce);
        }
        let outcome = action.approve(approval, Utc::now());
        // Expiry noticed on the way is worth writing down either way.
        Self::save(store, &action)?;
        match outcome {
            Ok(_token) => {
                // Spent only now: an approval refused for a stale version
                // leaves the page able to try again with the current one.
                self.take_nonce(id);
                Self::record(ledger, &action, "approved", "user", "")?;
                Ok(action)
            }
            Err(e) => {
                if matches!(action.status, Status::Expired) {
                    Self::record(ledger, &action, "expired", "core", "")?;
                }
                Err(e.into())
            }
        }
    }

    /// The user declined, with a reason (design 03: it may be short, but it
    /// is there).
    pub fn decline(
        &self,
        store: &Store,
        ledger: &Ledger,
        id: &str,
        reason: &str,
    ) -> Result<Action, DecisionError> {
        let mut action = self.get(store, id)?.ok_or(DecisionError::NotFound)?;
        let reason = reason.trim();
        let reason = if reason.is_empty() {
            "no reason given"
        } else {
            reason
        };
        action.decline(reason, Utc::now())?;
        Self::save(store, &action)?;
        Self::record(ledger, &action, "declined", "user", reason)?;
        Ok(action)
    }

    /// Lapse what nobody decided in time. Returns how many.
    pub fn expire_due(&self, store: &Store, ledger: &Ledger) -> anyhow::Result<usize> {
        let now = Utc::now();
        let mut expired = 0;
        for mut action in self.list(store, Some("pending"), 1000)? {
            if action.expire_if_due(now) {
                Self::save(store, &action)?;
                Self::record(ledger, &action, "expired", "core", "")?;
                expired += 1;
            }
        }
        Ok(expired)
    }

    #[allow(dead_code)] // the connector side of design 03 lands next
    /// Approved actions a connector may execute for an account, with their
    /// tokens. Marked as handed out in the ledger; the action stays
    /// approved until the connector reports.
    pub fn approved_for(
        &self,
        store: &Store,
        ledger: &Ledger,
        account: &str,
    ) -> anyhow::Result<Vec<(Action, Version, ExecutionToken)>> {
        let mut out = Vec::new();
        for stored in store.actions_for_account(account, "approved")? {
            let mut action: Action = serde_json::from_str(&stored.value)?;
            let Some(token) = action.token_for_execution() else {
                continue;
            };
            let now = Utc::now();
            match action.begin_execution(&token, now) {
                Ok(version) => {
                    // The token is spent inside the action; what the
                    // connector holds is the one copy that can still act.
                    Self::save(store, &action)?;
                    Self::record(
                        ledger,
                        &action,
                        "executing",
                        "core",
                        "handed to the connector",
                    )?;
                    out.push((action, version, token));
                }
                Err(e) => {
                    Self::save(store, &action)?;
                    Self::record(ledger, &action, "refused", "core", e.to_string())?;
                }
            }
        }
        Ok(out)
    }

    #[allow(dead_code)] // the connector side of design 03 lands next
    /// The connector reported. Status becomes executed, failed or unknown;
    /// the result item, when there is one, closes the loop in the data.
    pub fn finish(
        &self,
        store: &Store,
        ledger: &Ledger,
        id: &str,
        status: Status,
    ) -> anyhow::Result<Option<Action>> {
        let Some(mut action) = self.get(store, id)? else {
            return Ok(None);
        };
        let event = match &status {
            Status::Executed { .. } => "executed",
            Status::Failed { .. } => "failed",
            Status::Unknown { .. } => "unknown",
            _ => "finished",
        };
        let detail = match &status {
            Status::Executed { result } => result.map(|r| r.to_string()).unwrap_or_default(),
            Status::Failed { detail } | Status::Unknown { detail } => detail.clone(),
            _ => String::new(),
        };
        action.finish(status)?;
        Self::save(store, &action)?;
        Self::record(ledger, &action, event, "connector", detail)?;
        Ok(Some(action))
    }
}

/// Seconds until an action lapses, for the card's corner.
#[must_use]
pub fn expires_in(action: &Action, now: DateTime<Utc>) -> i64 {
    (action.expires_at - now).num_seconds().max(0)
}
