//! Carrying out approved actions: pulled from the core, checked again,
//! sent once, reported once.
//!
//! Design: `docs/design/05-connectors.md`, "动作执行". The core has already
//! checked status, expiry and token before handing an action out. This
//! side checks what only it can see: that the kind is one the account was
//! granted, that the account is its own, that the words match the hash the
//! user approved, and that the submission host is one it may reach. Then
//! it sends, exactly once, and tells the core how that went. There is no
//! retry anywhere in this file, and design 05 says why.

use std::time::Duration;

use genatrix_connector::capability::{AccountCapability, Host};
use genatrix_connector::protocol::{ActionToDo, Report, outcome};
use sha2::{Digest, Sha256};

use crate::ipc::{IpcSink, to_wire};
use crate::send::{self, SendError};

/// How often the core is asked for approved actions.
pub const PULL_EVERY: Duration = Duration::from_secs(15);

/// Keep asking the core for approved actions on this account and carry
/// them out, for as long as the process runs.
pub async fn run(sink: IpcSink, capability: AccountCapability, password: String) {
    loop {
        match sink.pull_actions().await {
            Ok(actions) => {
                for action in actions {
                    let report = carry_out(&capability, &password, action).await;
                    if let Err(e) = sink.report(report).await {
                        tracing::warn!(error = %e, "the core did not take the report");
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not ask for approved actions"),
        }
        tokio::time::sleep(PULL_EVERY).await;
    }
}

/// One action, start to report.
pub async fn carry_out(
    capability: &AccountCapability,
    password: &str,
    action: ActionToDo,
) -> Report {
    let outcome = match check(capability, &action) {
        Ok(effect) => match send::compose(&effect, &action.payload) {
            Ok(outgoing) => {
                let host = submission_host(capability);
                match host {
                    Some(host) => {
                        let sent =
                            send::submit(&host, &capability.account, password, &outgoing.message)
                                .await;
                        match sent {
                            Ok(()) => Outcome::Executed(Box::new(outgoing)),
                            Err(SendError::Failed(d)) => Outcome::Failed(d),
                            Err(SendError::Unknown(d)) => {
                                Outcome::Unknown(d, outgoing.message_id.clone())
                            }
                        }
                    }
                    None => Outcome::Failed(
                        "this account was granted no server to send through".to_owned(),
                    ),
                }
            }
            Err(e) => Outcome::Failed(e.detail().to_owned()),
        },
        Err(detail) => Outcome::Failed(detail),
    };
    let (outcome, detail, message, message_id) = match outcome {
        Outcome::Executed(outgoing) => {
            tracing::info!(action = %action.id, "sent");
            let message_id = outgoing.message_id;
            (
                outcome::EXECUTED,
                String::new(),
                Some(to_wire(&outgoing.incoming)),
                Some(message_id),
            )
        }
        Outcome::Failed(detail) => {
            tracing::warn!(action = %action.id, %detail, "not sent");
            (outcome::FAILED, detail, None, None)
        }
        Outcome::Unknown(detail, message_id) => {
            tracing::warn!(action = %action.id, %detail, "outcome unknown");
            (outcome::UNKNOWN, detail, None, Some(message_id))
        }
    };
    Report {
        account: capability.account.clone(),
        action_id: action.id,
        token: action.token,
        outcome: outcome.to_owned(),
        detail,
        message,
        chat: None,
        message_id,
    }
}

enum Outcome {
    Executed(Box<send::Outgoing>),
    Failed(String),
    Unknown(String, String),
}

/// The checks this side can make. Any one failing is a refusal with the
/// reason, never a guess.
fn check(capability: &AccountCapability, action: &ActionToDo) -> Result<send::SendMail, String> {
    if action.kind != "send_mail" {
        return Err(format!("the mail connector does not do {}", action.kind));
    }
    if !capability.may_do(&action.kind) {
        return Err(format!(
            "{} was not granted {}",
            capability.account, action.kind
        ));
    }
    let hash = hex::encode(Sha256::digest(action.payload.as_bytes()));
    if hash != action.payload_hash {
        return Err("the words do not match the approved version".to_owned());
    }
    let effect: send::SendMail = serde_json::from_str(&action.effect_json)
        .map_err(|e| format!("the effect could not be read: {e}"))?;
    if effect.account != capability.account {
        return Err(format!(
            "the action is for {}, and this is {}",
            effect.account, capability.account
        ));
    }
    Ok(effect)
}

/// The host this account may submit through: the granted host on a
/// submission port, checked against the capability like every connection.
fn submission_host(capability: &AccountCapability) -> Option<Host> {
    capability
        .hosts
        .iter()
        .find(|h| h.port == 587 || h.port == 465)
        .filter(|h| capability.may_reach(h))
        .cloned()
}

#[cfg(test)]
mod tests {
    use genatrix_model::Connector;

    use super::*;

    fn granted() -> AccountCapability {
        AccountCapability::new(Connector::Imap, "me@example.com")
            .with_host(Host::new("imap.example.com", 993))
            .with_host(Host::new("smtp.example.com", 587))
            .with_effect("send_mail")
    }

    fn todo(payload: &str) -> ActionToDo {
        ActionToDo {
            id: "a1".into(),
            kind: "send_mail".into(),
            token: "t".into(),
            version: 1,
            payload_hash: hex::encode(Sha256::digest(payload.as_bytes())),
            effect_json: serde_json::json!({
                "kind": "send_mail", "account": "me@example.com",
                "to": ["ann@example.com"], "subject": "Re: x",
                "in_reply_to": "a1@example.com", "references": ["a1@example.com"]
            })
            .to_string(),
            payload: payload.into(),
            reply_to_external_id: None,
        }
    }

    #[test]
    fn every_check_refuses_on_its_own() {
        assert!(check(&granted(), &todo("hi")).is_ok());

        let mut wrong_words = todo("hi");
        wrong_words.payload = "bye".into();
        assert!(
            check(&granted(), &wrong_words)
                .unwrap_err()
                .contains("approved version")
        );

        let mut other_kind = todo("hi");
        other_kind.kind = "send_message".into();
        assert!(check(&granted(), &other_kind).is_err());

        let not_granted = AccountCapability::new(Connector::Imap, "me@example.com")
            .with_host(Host::new("smtp.example.com", 587));
        assert!(
            check(&not_granted, &todo("hi"))
                .unwrap_err()
                .contains("not granted")
        );

        let mut other_account = todo("hi");
        other_account.effect_json = other_account.effect_json.replace(
            "\"account\":\"me@example.com\"",
            "\"account\":\"you@example.com\"",
        );
        assert!(
            check(&granted(), &other_account)
                .unwrap_err()
                .contains("this is me@example.com")
        );
    }

    #[test]
    fn the_submission_host_is_the_granted_one_on_a_submission_port() {
        assert_eq!(
            submission_host(&granted()),
            Some(Host::new("smtp.example.com", 587))
        );
        let read_only = AccountCapability::new(Connector::Imap, "me@example.com")
            .with_host(Host::new("imap.example.com", 993));
        assert_eq!(submission_host(&read_only), None);
    }

    #[tokio::test]
    async fn a_refused_action_is_reported_failed_with_the_reason_and_nothing_else() {
        let mut action = todo("hi");
        action.payload_hash = "0000".into();
        let report = carry_out(&granted(), "pw", action).await;
        assert_eq!(report.outcome, outcome::FAILED);
        assert!(report.detail.contains("approved version"));
        assert!(report.message.is_none());
        assert_eq!(report.token, "t");
    }
}
