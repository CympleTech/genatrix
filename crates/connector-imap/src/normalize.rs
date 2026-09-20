//! Turning a fetched message into the shape the rest of the system uses.
//!
//! Design: `docs/design/01-data-model.md` for the entities and
//! `docs/design/05-connectors.md` for what mail specifically becomes.
//!
//! This is a pure function of the bytes. It resolves no people and writes
//! nothing: the core owns identity, because deciding that two addresses are
//! the same person is an assertion about the world and a connector has no
//! business making one. What comes out is what the bytes say, and the core
//! does the rest.

use chrono::{DateTime, FixedOffset};
use mail_parser::{Address, MessageParser, MimeHeaders};

use crate::text;

/// Headers worth keeping beside an item. The sensitivity rules read these,
/// and a mailbox's worth of full headers is a lot of noise to carry for the
/// handful that mean something.
const KEPT_HEADERS: [&str; 8] = [
    "list-unsubscribe",
    "list-id",
    "precedence",
    "auto-submitted",
    "x-autoreply",
    "return-path",
    "reply-to",
    "x-mailer",
];

/// One address as the message gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mailbox {
    /// The address, lowercased.
    pub address: String,
    /// The display name, if the message had one.
    pub name: Option<String>,
}

impl Mailbox {
    /// What to show before the core knows who this is.
    #[must_use]
    pub fn display(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.address.clone())
    }
}

/// A file that came with a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    /// File name, if it had one.
    pub name: Option<String>,
    /// Media type.
    pub mime: String,
    /// The bytes.
    pub bytes: Vec<u8>,
}

/// A message, normalized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mail {
    /// Subject line.
    pub subject: String,
    /// Who sent it.
    pub from: Option<Mailbox>,
    /// Who it was addressed to.
    pub to: Vec<Mailbox>,
    /// Who was copied.
    pub cc: Vec<Mailbox>,
    /// When the sender says it was sent, with their offset.
    pub date: Option<DateTime<FixedOffset>>,
    /// `Message-ID`, without the angle brackets.
    pub message_id: Option<String>,
    /// `In-Reply-To`, without the angle brackets.
    pub in_reply_to: Option<String>,
    /// `References`, in order, without the angle brackets.
    pub references: Vec<String>,
    /// The body as plain text, with markup and the quoted reply removed.
    pub text: String,
    /// Files that came with it.
    pub attachments: Vec<Attachment>,
    /// The headers the rules care about, names lowercased.
    pub headers: Vec<(String, String)>,
}

/// Why a message could not be read.
#[derive(Debug, thiserror::Error)]
pub enum NormalizeError {
    /// The bytes are not a message this parser can make sense of.
    #[error("the message could not be parsed")]
    Unparsable,
}

/// Normalize a fetched message.
pub fn normalize(raw: &[u8]) -> Result<Mail, NormalizeError> {
    let parsed = MessageParser::default()
        .parse(raw)
        .ok_or(NormalizeError::Unparsable)?;

    // Prefer what the sender wrote as text. Fall back to converting the HTML
    // part, which is what most mail now is.
    let body = parsed
        .body_text(0)
        .map(std::borrow::Cow::into_owned)
        .filter(|t| !t.trim().is_empty())
        .or_else(|| parsed.body_html(0).map(|h| text::from_html(&h)))
        .unwrap_or_default();

    let attachments = parsed
        .attachments()
        .map(|part| Attachment {
            name: part.attachment_name().map(str::to_owned),
            mime: part.content_type().map_or_else(
                || "application/octet-stream".to_owned(),
                |c| match c.subtype() {
                    Some(sub) => format!("{}/{}", c.ctype(), sub),
                    None => c.ctype().to_owned(),
                },
            ),
            bytes: part.contents().to_vec(),
        })
        .collect();

    let headers = parsed
        .headers()
        .iter()
        .filter_map(|header| {
            let name = header.name().to_lowercase();
            KEPT_HEADERS
                .contains(&name.as_str())
                .then(|| (name, header_text(header)))
        })
        .collect();

    Ok(Mail {
        subject: parsed.subject().unwrap_or_default().to_owned(),
        from: parsed.from().and_then(first_mailbox),
        to: parsed.to().map(mailboxes).unwrap_or_default(),
        cc: parsed.cc().map(mailboxes).unwrap_or_default(),
        date: parsed
            .date()
            .and_then(|d| DateTime::parse_from_rfc3339(&d.to_rfc3339()).ok()),
        message_id: parsed.message_id().map(unbracket),
        in_reply_to: parsed
            .in_reply_to()
            .as_text_list()
            .and_then(|list| list.first().map(|id| unbracket(id)))
            .or_else(|| parsed.in_reply_to().as_text().map(unbracket)),
        references: parsed
            .references()
            .as_text_list()
            .map(|list| list.iter().map(|id| unbracket(id)).collect())
            .or_else(|| parsed.references().as_text().map(|id| vec![unbracket(id)]))
            .unwrap_or_default(),
        text: text::strip_quote(&body),
        attachments,
        headers,
    })
}

/// The identifier a message is known by within its account.
///
/// Design 01 requires this to be unique across the whole account, which a
/// bare IMAP UID is not: it means nothing without the folder it came from,
/// and nothing again once the server changes that folder's validity marker.
/// Gmail's own message id has none of those problems, so it wins when the
/// server offers it.
#[must_use]
pub fn external_id(
    gmail_message_id: Option<u64>,
    folder: &str,
    uidvalidity: u32,
    uid: u32,
) -> String {
    match gmail_message_id {
        Some(id) => format!("gm:{id:x}"),
        None => format!("{folder}/{uidvalidity}/{uid}"),
    }
}

/// Which conversation a message belongs to.
///
/// Gmail's thread id when there is one. Otherwise the oldest identifier the
/// message refers to, which is the root of the chain it is part of. Failing
/// both, its own identifier, so it is a conversation of one rather than being
/// lumped in with every other mail that has no references.
#[must_use]
pub fn thread_key(mail: &Mail, gmail_thread_id: Option<u64>) -> String {
    if let Some(id) = gmail_thread_id {
        return format!("gm:t{id:x}");
    }
    if let Some(root) = mail.references.first() {
        return format!("ref:{root}");
    }
    if let Some(parent) = &mail.in_reply_to {
        return format!("ref:{parent}");
    }
    match &mail.message_id {
        Some(id) => format!("ref:{id}"),
        None => format!("subject:{}", normalize_subject(&mail.subject)),
    }
}

/// A subject with the reply and forward prefixes taken off, for the last
/// resort where a message carries no identifiers at all.
fn normalize_subject(subject: &str) -> String {
    let mut rest = subject.trim();
    loop {
        let lower = rest.to_lowercase();
        let stripped = ["re:", "fw:", "fwd:", "回复:", "答复:", "转发:"]
            .iter()
            .find_map(|prefix| lower.starts_with(prefix).then(|| &rest[prefix.len()..]));
        match stripped {
            Some(next) => rest = next.trim_start(),
            None => break,
        }
    }
    rest.to_lowercase()
}

fn header_text(header: &mail_parser::Header<'_>) -> String {
    header
        .value()
        .as_text()
        .map(str::to_owned)
        .or_else(|| header.value().as_text_list().map(|list| list.join(", ")))
        .unwrap_or_default()
}

fn first_mailbox(address: &Address<'_>) -> Option<Mailbox> {
    mailboxes(address).into_iter().next()
}

fn mailboxes(address: &Address<'_>) -> Vec<Mailbox> {
    address
        .iter()
        .filter_map(|addr| {
            addr.address().map(|a| Mailbox {
                address: a.trim().to_lowercase(),
                name: addr
                    .name()
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_owned),
            })
        })
        .collect()
}

fn unbracket(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &[u8] = b"From: Maria Alvarez <Maria@Contoso.example>\r\n\
To: me@example.com\r\n\
Cc: Ops <ops@contoso.example>\r\n\
Subject: Re: revised proposal\r\n\
Date: Mon, 1 Sep 2026 10:00:00 +0800\r\n\
Message-ID: <abc123@contoso.example>\r\n\
In-Reply-To: <root@example.com>\r\n\
References: <root@example.com> <mid@example.com>\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
Friday works for me.\r\n\
\r\n\
On Mon, 1 Sep 2026 at 09:00, Neo <me@example.com> wrote:\r\n\
> Can we move milestone two?\r\n";

    #[test]
    fn a_plain_message_comes_apart_correctly() {
        let mail = normalize(PLAIN).unwrap();
        assert_eq!(mail.subject, "Re: revised proposal");
        assert_eq!(
            mail.from,
            Some(Mailbox {
                address: "maria@contoso.example".into(),
                name: Some("Maria Alvarez".into())
            }),
            "the address is normalized, the name is kept as written"
        );
        assert_eq!(mail.to[0].address, "me@example.com");
        assert_eq!(mail.cc[0].display(), "Ops");
        assert_eq!(mail.message_id.as_deref(), Some("abc123@contoso.example"));
        assert_eq!(mail.in_reply_to.as_deref(), Some("root@example.com"));
        assert_eq!(mail.references, ["root@example.com", "mid@example.com"]);
        assert_eq!(
            mail.date.unwrap().to_rfc3339(),
            "2026-09-01T10:00:00+08:00",
            "the sender's own offset survives"
        );
        assert_eq!(mail.text, "Friday works for me.", "the quote is gone");
    }

    #[test]
    fn an_html_only_message_becomes_readable_text() {
        let raw = b"From: news@example.com\r\n\
Subject: Digest\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<html><body><style>p{color:red}</style><p>This week:</p>\
<ul><li>a Rust release</li></ul>\
<img src=\"https://tracker.example/p.gif\"></body></html>";
        let mail = normalize(raw).unwrap();
        assert_eq!(mail.text, "This week:\na Rust release");
        assert!(!mail.text.contains("tracker.example"));
    }

    #[test]
    fn a_message_with_both_parts_prefers_what_the_sender_typed() {
        let raw = b"From: a@example.com\r\n\
Subject: Both\r\n\
Content-Type: multipart/alternative; boundary=b\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
\r\n\
the plain version\r\n\
--b\r\n\
Content-Type: text/html\r\n\
\r\n\
<p>the html version</p>\r\n\
--b--\r\n";
        assert_eq!(normalize(raw).unwrap().text, "the plain version");
    }

    #[test]
    fn attachments_come_out_with_their_bytes() {
        let raw = b"From: a@example.com\r\n\
Subject: Contract\r\n\
Content-Type: multipart/mixed; boundary=b\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
\r\n\
see attached\r\n\
--b\r\n\
Content-Type: application/pdf\r\n\
Content-Disposition: attachment; filename=\"contract.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
aGVsbG8=\r\n\
--b--\r\n";
        let mail = normalize(raw).unwrap();
        assert_eq!(mail.text, "see attached");
        assert_eq!(mail.attachments.len(), 1);
        assert_eq!(mail.attachments[0].name.as_deref(), Some("contract.pdf"));
        assert_eq!(mail.attachments[0].mime, "application/pdf");
        assert_eq!(mail.attachments[0].bytes, b"hello");
    }

    #[test]
    fn only_the_headers_the_rules_read_are_kept() {
        let raw = b"From: news@example.com\r\n\
Subject: Digest\r\n\
List-Unsubscribe: <https://example.com/u>\r\n\
Precedence: bulk\r\n\
X-Spam-Score: 0.1\r\n\
Received: from somewhere by somewhere else\r\n\
\r\n\
body\r\n";
        let mail = normalize(raw).unwrap();
        let names: Vec<&str> = mail.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"list-unsubscribe"));
        assert!(names.contains(&"precedence"));
        assert!(!names.contains(&"x-spam-score"), "{names:?}");
        assert!(!names.contains(&"received"), "{names:?}");
    }

    #[test]
    fn an_identifier_is_unique_across_the_account() {
        // A bare UID is not: two folders can both have message 5.
        assert_eq!(external_id(None, "INBOX", 10, 5), "INBOX/10/5");
        assert_ne!(
            external_id(None, "INBOX", 10, 5),
            external_id(None, "Archive", 10, 5)
        );
        assert_ne!(
            external_id(None, "INBOX", 10, 5),
            external_id(None, "INBOX", 11, 5),
            "a renumbered folder means a different message"
        );
        assert_eq!(external_id(Some(0x1a2b), "INBOX", 10, 5), "gm:1a2b");
    }

    #[test]
    fn a_reply_lands_in_the_same_conversation_as_its_root() {
        let reply = normalize(PLAIN).unwrap();
        let root_raw = b"From: me@example.com\r\n\
Subject: revised proposal\r\n\
Message-ID: <root@example.com>\r\n\
\r\n\
here it is\r\n";
        let root = normalize(root_raw).unwrap();
        assert_eq!(thread_key(&reply, None), "ref:root@example.com");
        assert_eq!(
            thread_key(&root, None),
            "ref:root@example.com",
            "the root is in its own thread, which the reply joins"
        );
    }

    #[test]
    fn gmails_own_thread_wins_when_the_server_offers_it() {
        let mail = normalize(PLAIN).unwrap();
        assert_eq!(thread_key(&mail, Some(0xff)), "gm:tff");
    }

    #[test]
    fn a_message_with_no_identifiers_falls_back_to_its_subject() {
        let raw = b"From: a@example.com\r\nSubject: Re: Fwd: Weekly sync\r\n\r\nbody\r\n";
        let mail = normalize(raw).unwrap();
        assert_eq!(thread_key(&mail, None), "subject:weekly sync");
    }

    #[test]
    fn a_message_missing_everything_optional_still_normalizes() {
        let mail = normalize(b"Subject: bare\r\n\r\nbody").unwrap();
        assert_eq!(mail.subject, "bare");
        assert_eq!(mail.text, "body");
        assert!(mail.from.is_none());
        assert!(mail.to.is_empty());
        assert!(mail.date.is_none());
        assert!(mail.attachments.is_empty());
    }
}
