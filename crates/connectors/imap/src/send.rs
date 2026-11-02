//! Sending one approved mail through the submission port.
//!
//! Design: `docs/design/05-connectors.md`, "发送" and "动作执行".
//!
//! Three things this module is careful about. The reply carries
//! `In-Reply-To` and `References`, or the other side's client will not
//! thread it. The `Message-ID` is chosen here, before the server sees the
//! message, so the same bytes can be kept as the outbound item at once and
//! recognised when sync brings the server's copy back. And a failure is
//! only called a failure when the server refused: a connection that drops
//! after the message was handed over is an unknown outcome, never a reason
//! to send again.

use genatrix_connector::capability::Host;
use lettre::message::{Mailbox, header};
use lettre::transport::smtp::authentication::Credentials as SmtpCredentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::Deserialize;

use crate::normalize;
use crate::sync::Incoming;

/// The `send_mail` effect as the core serializes it. Only the fields a
/// sender needs; the connector does not depend on the agent crate.
#[derive(Clone, Debug, Deserialize)]
pub struct SendMail {
    /// Account to send from; has to be this one.
    pub account: String,
    /// Recipients.
    pub to: Vec<String>,
    /// Subject line.
    pub subject: String,
    /// Message this replies to, without brackets.
    #[serde(default)]
    pub in_reply_to: Option<String>,
    /// The conversation's `References` chain, without brackets.
    #[serde(default)]
    pub references: Vec<String>,
}

/// How a send ended when it did not succeed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendError {
    /// Nothing went out: refused before or at submission.
    Failed(String),
    /// Handed over; the final answer never came.
    Unknown(String),
}

impl SendError {
    /// The words, for the report.
    #[must_use]
    pub fn detail(&self) -> &str {
        match self {
            Self::Failed(d) | Self::Unknown(d) => d,
        }
    }
}

/// A message ready to go, and the shape it will be kept in.
pub struct Outgoing {
    /// The message, for the transport.
    pub message: Message,
    /// The `Message-ID` it was given, without brackets.
    pub message_id: String,
    /// The same bytes as the core keeps them.
    pub incoming: Incoming,
}

/// Compose the reply. Pure: nothing is sent here.
pub fn compose(effect: &SendMail, payload: &str) -> Result<Outgoing, SendError> {
    let from: Mailbox = effect
        .account
        .parse()
        .map_err(|e| SendError::Failed(format!("the account is not an address: {e}")))?;
    let domain = effect
        .account
        .rsplit_once('@')
        .map_or("genatrix.local", |(_, d)| d);
    let message_id = format!("{}.genatrix@{domain}", ulid::Ulid::new());

    let mut builder = Message::builder()
        .from(from)
        .subject(effect.subject.clone())
        .message_id(Some(format!("<{message_id}>")))
        .date_now()
        .user_agent("Genatrix".to_owned());
    if effect.to.is_empty() {
        return Err(SendError::Failed("no recipient".to_owned()));
    }
    for to in &effect.to {
        let mailbox: Mailbox = to
            .parse()
            .map_err(|e| SendError::Failed(format!("{to} is not an address: {e}")))?;
        builder = builder.to(mailbox);
    }
    if let Some(id) = &effect.in_reply_to {
        builder = builder.in_reply_to(format!("<{id}>"));
    }
    if !effect.references.is_empty() {
        let chain: Vec<String> = effect.references.iter().map(|r| format!("<{r}>")).collect();
        builder = builder.references(chain.join(" "));
    }
    let message = builder
        .header(header::ContentType::TEXT_PLAIN)
        .body(payload.to_owned())
        .map_err(|e| SendError::Failed(format!("could not build the message: {e}")))?;

    let raw = message.formatted();
    let mail = normalize::normalize(&raw)
        .map_err(|e| SendError::Failed(format!("the composed message does not parse: {e}")))?;
    let thread_key = normalize::thread_key(&mail, None);
    Ok(Outgoing {
        incoming: Incoming {
            account: effect.account.clone(),
            external_id: format!("mid:{message_id}"),
            thread_key,
            raw,
            mail,
        },
        message,
        message_id,
    })
}

/// Submit the message, once. STARTTLS on the submission port; the
/// account's password, which is the same app password IMAP uses.
pub async fn submit(
    host: &Host,
    account: &str,
    password: &str,
    message: &Message,
) -> Result<(), SendError> {
    let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host.name)
        .map_err(|e| SendError::Failed(format!("could not set up the submission: {e}")))?
        .port(host.port)
        .credentials(SmtpCredentials::new(
            account.to_owned(),
            password.to_owned(),
        ))
        .timeout(Some(std::time::Duration::from_secs(60)))
        .build();
    match transport.send(message.clone()).await {
        Ok(_) => Ok(()),
        Err(e) => Err(classify(&e)),
    }
}

/// Which of the two failures this is (design 05: "提交后连接中断、没收到
/// 服务器的最终应答，结果是 Unknown，不是 Failed").
///
/// A response from the server, permanent or transient, means the server
/// said no and nothing went out. A TLS or client-side error before any
/// exchange means the same. Anything else, a dropped connection, a
/// timeout, a shut transport, may have come after the message was handed
/// over, and the honest word for that is unknown.
#[must_use]
pub fn classify(e: &lettre::transport::smtp::Error) -> SendError {
    if e.is_permanent() || e.is_transient() || e.is_response() || e.is_client() || e.is_tls() {
        SendError::Failed(format!("the server refused: {e}"))
    } else {
        SendError::Unknown(format!(
            "the connection failed before the server answered: {e}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect() -> SendMail {
        SendMail {
            account: "me@example.com".into(),
            to: vec!["ann@example.com".into()],
            subject: "Re: lunch".into(),
            in_reply_to: Some("a1@example.com".into()),
            references: vec!["a0@example.com".into(), "a1@example.com".into()],
        }
    }

    #[test]
    fn a_reply_threads_and_is_kept_under_its_own_id() {
        let out = compose(&effect(), "Friday works.").unwrap();
        let text = String::from_utf8(out.incoming.raw.clone()).unwrap();
        assert!(text.contains("In-Reply-To: <a1@example.com>"), "{text}");
        assert!(
            text.contains("References: <a0@example.com> <a1@example.com>"),
            "{text}"
        );
        assert!(text.contains(&format!("Message-ID: <{}>", out.message_id)));
        assert!(out.message_id.ends_with("@example.com"));
        assert_eq!(out.incoming.external_id, format!("mid:{}", out.message_id));
        assert_eq!(
            out.incoming.mail.message_id.as_deref(),
            Some(out.message_id.as_str())
        );
        assert_eq!(out.incoming.mail.text.trim(), "Friday works.");
        assert_eq!(out.incoming.mail.to[0].address, "ann@example.com");
        assert_eq!(out.incoming.thread_key, "ref:a0@example.com");
    }

    #[test]
    fn a_reply_to_nobody_or_to_nonsense_fails_before_anything_goes_out() {
        let mut e = effect();
        e.to.clear();
        assert!(matches!(compose(&e, "x"), Err(SendError::Failed(_))));
        let mut e = effect();
        e.to = vec!["not an address".into()];
        assert!(matches!(compose(&e, "x"), Err(SendError::Failed(_))));
    }

    #[test]
    fn two_messages_never_share_an_id() {
        let a = compose(&effect(), "x").unwrap();
        let b = compose(&effect(), "x").unwrap();
        assert_ne!(a.message_id, b.message_id);
    }
}
