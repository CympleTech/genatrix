//! The mail connector's side of the core socket, and the shapes that cross it.
//!
//! Design: `docs/design/05-connectors.md`, "进程模型".
//!
//! [`IpcSink`] is a [`Sink`] whose store is on the other side of a socket:
//! every batch and every cursor becomes a call. One connection serves every
//! account in the process, one call at a time, which is plenty: the calls
//! are few and the answers are quick.
//!
//! A connection that dies is not something this process can mend. The core
//! restarts connectors it loses, so the honest response is to leave, which
//! is what [`IpcSink`] does.

use std::sync::Arc;

use chrono::DateTime;
use genatrix_connector::protocol::{
    self, Body, CallError, Client, MailAddress, MailAttachment, MailHeader, MailMessage,
};
use genatrix_connector::{Cursor, Fault, SyncState};
use tokio::sync::Mutex;

use crate::normalize::{Attachment, Mail, Mailbox};
use crate::sync::Incoming;
use crate::watch::Sink;

/// The core, over the socket.
#[derive(Clone)]
pub struct IpcSink {
    client: Arc<Mutex<Client>>,
    account: String,
}

impl IpcSink {
    /// A sink for one account over a shared connection.
    #[must_use]
    pub fn new(client: Arc<Mutex<Client>>, account: impl Into<String>) -> Self {
        Self {
            client,
            account: account.into(),
        }
    }

    async fn call(&self, body: Body) -> Result<Body, Fault> {
        match self.client.lock().await.call(body).await {
            Ok(body) => Ok(body),
            Err(CallError::Refused(detail)) => Err(Fault::transient(&self.account, detail)),
            Err(e) => {
                // Nothing in this process can be trusted to finish once the
                // core is gone; the core will start a fresh one.
                tracing::error!(error = %e, "the core connection is gone; leaving");
                std::process::exit(2);
            }
        }
    }

    /// Tell the core how the account is doing.
    pub async fn status(&self, state: &SyncState) -> Result<(), Fault> {
        let state_json = serde_json::to_string(state)
            .map_err(|e| Fault::transient(&self.account, format!("encoding state: {e}")))?;
        self.call(Body::Status(protocol::Status {
            account: self.account.clone(),
            state_json,
        }))
        .await
        .map(|_| ())
    }
}

impl Sink for IpcSink {
    async fn store(&self, batch: &[Incoming]) -> Result<usize, Fault> {
        match self
            .call(Body::Store(protocol::Store {
                account: self.account.clone(),
                messages: batch.iter().map(to_wire).collect(),
            }))
            .await?
        {
            Body::Stored(stored) => Ok(stored.new as usize),
            _ => Err(Fault::transient(
                &self.account,
                "the core answered out of turn",
            )),
        }
    }

    async fn load(&self, scope: &str) -> Result<Option<Cursor>, Fault> {
        match self
            .call(Body::LoadCursor(protocol::LoadCursor {
                account: self.account.clone(),
                scope: scope.to_owned(),
            }))
            .await?
        {
            Body::CursorLoaded(loaded) => loaded
                .cursor_json
                .map(|json| {
                    serde_json::from_str(&json)
                        .map_err(|e| Fault::transient(&self.account, format!("bad cursor: {e}")))
                })
                .transpose(),
            _ => Err(Fault::transient(
                &self.account,
                "the core answered out of turn",
            )),
        }
    }

    async fn save(&self, scope: &str, cursor: &Cursor) -> Result<(), Fault> {
        let cursor_json = serde_json::to_string(cursor)
            .map_err(|e| Fault::transient(&self.account, format!("encoding cursor: {e}")))?;
        self.call(Body::SaveCursor(protocol::SaveCursor {
            account: self.account.clone(),
            scope: scope.to_owned(),
            cursor_json,
        }))
        .await
        .map(|_| ())
    }
}

/// An incoming message as it crosses the socket.
#[must_use]
pub fn to_wire(incoming: &Incoming) -> MailMessage {
    let mail = &incoming.mail;
    MailMessage {
        external_id: incoming.external_id.clone(),
        thread_key: incoming.thread_key.clone(),
        raw: incoming.raw.clone(),
        subject: mail.subject.clone(),
        from: mail.from.as_ref().map(address_to_wire),
        to: mail.to.iter().map(address_to_wire).collect(),
        cc: mail.cc.iter().map(address_to_wire).collect(),
        date: mail.date.map(|d| d.to_rfc3339()),
        message_id: mail.message_id.clone(),
        in_reply_to: mail.in_reply_to.clone(),
        references: mail.references.clone(),
        text: mail.text.clone(),
        attachments: mail
            .attachments
            .iter()
            .map(|a| MailAttachment {
                name: a.name.clone(),
                mime: a.mime.clone(),
                bytes: a.bytes.clone(),
            })
            .collect(),
        headers: mail
            .headers
            .iter()
            .map(|(name, value)| MailHeader {
                name: name.clone(),
                value: value.clone(),
            })
            .collect(),
    }
}

/// An incoming message, back from the socket. The core's side of
/// [`to_wire`].
#[must_use]
pub fn from_wire(account: &str, message: MailMessage) -> Incoming {
    Incoming {
        account: account.to_owned(),
        external_id: message.external_id,
        thread_key: message.thread_key,
        raw: message.raw,
        mail: Mail {
            subject: message.subject,
            from: message.from.map(address_from_wire),
            to: message.to.into_iter().map(address_from_wire).collect(),
            cc: message.cc.into_iter().map(address_from_wire).collect(),
            date: message
                .date
                .and_then(|d| DateTime::parse_from_rfc3339(&d).ok()),
            message_id: message.message_id,
            in_reply_to: message.in_reply_to,
            references: message.references,
            text: message.text,
            attachments: message
                .attachments
                .into_iter()
                .map(|a| Attachment {
                    name: a.name,
                    mime: a.mime,
                    bytes: a.bytes,
                })
                .collect(),
            headers: message
                .headers
                .into_iter()
                .map(|h| (h.name, h.value))
                .collect(),
        },
    }
}

fn address_to_wire(mailbox: &Mailbox) -> MailAddress {
    MailAddress {
        address: mailbox.address.clone(),
        name: mailbox.name.clone(),
    }
}

fn address_from_wire(address: MailAddress) -> Mailbox {
    Mailbox {
        address: address.address,
        name: address.name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalize;

    #[test]
    fn a_message_crosses_the_wire_unchanged() {
        let raw = b"From: Ann <ann@example.com>\r\n\
To: me@example.com\r\n\
Cc: bob@example.com\r\n\
Subject: lunch\r\n\
Date: Mon, 1 Sep 2026 10:00:00 +0800\r\n\
Message-ID: <a1@example.com>\r\n\
In-Reply-To: <a0@example.com>\r\n\
References: <a0@example.com>\r\n\
List-Id: <friends.example.com>\r\n\
Content-Type: text/plain\r\n\
\r\n\
Friday?\r\n";
        let mail = normalize::normalize(raw).unwrap();
        let incoming = Incoming {
            account: "me@example.com".into(),
            external_id: "mid:a1@example.com".into(),
            thread_key: "ref:a0@example.com".into(),
            raw: raw.to_vec(),
            mail,
        };
        let back = from_wire("me@example.com", to_wire(&incoming));
        assert_eq!(back, incoming);
        assert_eq!(
            back.mail.date.unwrap().offset().local_minus_utc(),
            8 * 3600,
            "the sender's offset survives"
        );
    }
}
