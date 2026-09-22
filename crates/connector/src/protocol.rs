//! The wire between the core and a connector process.
//!
//! Design: `docs/design/05-connectors.md`, "进程模型": the core is the
//! server, connectors connect to it over a Unix socket, and the protocol is
//! length-prefixed protobuf. The envelope-with-oneof shape follows
//! crabtalk's; nothing else of it is used.
//!
//! A connector only ever asks and the core only ever answers, so a call is
//! one frame each way and nothing has to be matched up. The one thing the
//! core says unprompted is its answer to `Hello`: which accounts this
//! process is responsible for, with what it may reach and the secret it
//! needs. That secret is the reason for the token in `Hello`: the socket is
//! reachable by any process of the same user, and a connector the core did
//! not start does not get anyone's password.
//!
//! Cursors and states cross as the JSON they are stored and shown as. The
//! transport carries them; it does not read them.

use std::io;
use std::path::Path;

use prost::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// A frame larger than this is a bug or an attack, and is refused.
pub const MAX_FRAME: u32 = 64 * 1024 * 1024;

/// The environment variable a connector reads its token from. The core sets
/// it when it starts the process; the connector clears it once read.
pub const TOKEN_ENV: &str = "GENATRIX_CONNECTOR_TOKEN";

/// One frame.
#[derive(Clone, PartialEq, Message)]
pub struct Envelope {
    /// Correlation number. Unused while every call is one frame each way,
    /// carried so that it never has to be added later.
    #[prost(uint64, tag = "1")]
    pub id: u64,
    /// What it says.
    #[prost(oneof = "Body", tags = "2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12")]
    pub body: Option<Body>,
}

/// Everything either side can say.
#[derive(Clone, PartialEq, prost::Oneof)]
pub enum Body {
    /// Connector to core, first.
    #[prost(message, tag = "2")]
    Hello(Hello),
    /// Core to connector, in answer to `Hello`.
    #[prost(message, tag = "3")]
    Assign(Assign),
    /// A batch of messages to keep.
    #[prost(message, tag = "4")]
    Store(Store),
    /// How many of them were new.
    #[prost(message, tag = "5")]
    Stored(Stored),
    /// Where did I get to?
    #[prost(message, tag = "6")]
    LoadCursor(LoadCursor),
    /// Here, or nowhere yet.
    #[prost(message, tag = "7")]
    CursorLoaded(CursorLoaded),
    /// This is where I have got to.
    #[prost(message, tag = "8")]
    SaveCursor(SaveCursor),
    /// Noted.
    #[prost(message, tag = "9")]
    Saved(Saved),
    /// This is how the account is doing.
    #[prost(message, tag = "10")]
    Status(Status),
    /// Noted.
    #[prost(message, tag = "11")]
    Ack(Ack),
    /// The core could not do that.
    #[prost(message, tag = "12")]
    Failure(Failure),
}

/// Who is calling.
#[derive(Clone, PartialEq, Message)]
pub struct Hello {
    /// `imap`, `telegram`.
    #[prost(string, tag = "1")]
    pub connector: String,
    /// The token the core gave this process at start.
    #[prost(string, tag = "2")]
    pub token: String,
    /// The process, for the log.
    #[prost(uint32, tag = "3")]
    pub pid: u32,
}

/// What a connector process is responsible for.
#[derive(Clone, PartialEq, Message)]
pub struct Assign {
    /// One per account.
    #[prost(message, repeated, tag = "1")]
    pub accounts: Vec<Assignment>,
}

/// One account: what it may reach, and how to sign in.
#[derive(Clone, PartialEq, Message)]
pub struct Assignment {
    /// The `AccountCapability`, as JSON.
    #[prost(string, tag = "1")]
    pub capability_json: String,
    /// The password or session secret. Never logged, never stored by the
    /// connector; the core keeps it in the keychain.
    #[prost(string, tag = "2")]
    pub secret: String,
}

/// Messages to keep: mail, chat, or both.
#[derive(Clone, PartialEq, Message)]
pub struct Store {
    /// Whose.
    #[prost(string, tag = "1")]
    pub account: String,
    /// Mail.
    #[prost(message, repeated, tag = "2")]
    pub messages: Vec<MailMessage>,
    /// Chat.
    #[prost(message, repeated, tag = "3")]
    pub chats: Vec<ChatMessage>,
}

/// One chat message, normalized by the connector, with its original bytes.
#[derive(Clone, PartialEq, Message)]
pub struct ChatMessage {
    /// Account-wide identifier: the conversation and the message id.
    #[prost(string, tag = "1")]
    pub external_id: String,
    /// The conversation's identifier within the account.
    #[prost(string, tag = "2")]
    pub thread_key: String,
    /// `direct`, `group` or `channel`.
    #[prost(string, tag = "3")]
    pub thread_kind: String,
    /// The conversation's name.
    #[prost(string, tag = "4")]
    pub thread_title: String,
    /// Who sent it, when known. Absent for anonymous channel posts.
    #[prost(message, optional, tag = "5")]
    pub sender: Option<ChatSender>,
    /// Whether the account holder sent it.
    #[prost(bool, tag = "6")]
    pub outgoing: bool,
    /// When, RFC 3339.
    #[prost(string, tag = "7")]
    pub date: String,
    /// Plain text; formatting flattened, links kept as URLs.
    #[prost(string, tag = "8")]
    pub text: String,
    /// The message this one replies to, as an external id.
    #[prost(string, optional, tag = "9")]
    pub reply_to: Option<String>,
    /// The name of the original author when forwarded.
    #[prost(string, optional, tag = "10")]
    pub forwarded_from: Option<String>,
    /// Whether the source marks it as edited.
    #[prost(bool, tag = "11")]
    pub edited: bool,
    /// The message as the protocol delivered it, serialized.
    #[prost(bytes = "vec", tag = "12")]
    pub raw: Vec<u8>,
    /// Attached media, described; bytes are fetched later, on request.
    #[prost(message, repeated, tag = "13")]
    pub media: Vec<ChatMedia>,
}

/// Who sent a chat message.
#[derive(Clone, PartialEq, Message)]
pub struct ChatSender {
    /// The platform's numeric identifier.
    #[prost(int64, tag = "1")]
    pub id: i64,
    /// Handle, without the `@`.
    #[prost(string, optional, tag = "2")]
    pub username: Option<String>,
    /// Display name.
    #[prost(string, tag = "3")]
    pub name: String,
}

/// One attachment, described but not carried.
#[derive(Clone, PartialEq, Message)]
pub struct ChatMedia {
    /// `photo`, `document`, `sticker`, `voice`, ...
    #[prost(string, tag = "1")]
    pub kind: String,
    /// File name, when there is one.
    #[prost(string, optional, tag = "2")]
    pub name: Option<String>,
    /// Media type, when known.
    #[prost(string, optional, tag = "3")]
    pub mime: Option<String>,
    /// Size in bytes, when known.
    #[prost(uint64, optional, tag = "4")]
    pub size: Option<u64>,
}

/// How a store went.
#[derive(Clone, PartialEq, Message)]
pub struct Stored {
    /// How many were not there before.
    #[prost(uint32, tag = "1")]
    pub new: u32,
}

/// Ask for a cursor.
#[derive(Clone, PartialEq, Message)]
pub struct LoadCursor {
    /// Whose.
    #[prost(string, tag = "1")]
    pub account: String,
    /// Which subdivision.
    #[prost(string, tag = "2")]
    pub scope: String,
}

/// A cursor, or none.
#[derive(Clone, PartialEq, Message)]
pub struct CursorLoaded {
    /// The `Cursor`, as JSON, when one was saved.
    #[prost(string, optional, tag = "1")]
    pub cursor_json: Option<String>,
}

/// Record a cursor.
#[derive(Clone, PartialEq, Message)]
pub struct SaveCursor {
    /// Whose.
    #[prost(string, tag = "1")]
    pub account: String,
    /// Which subdivision.
    #[prost(string, tag = "2")]
    pub scope: String,
    /// The `Cursor`, as JSON.
    #[prost(string, tag = "3")]
    pub cursor_json: String,
}

/// Recorded.
#[derive(Clone, PartialEq, Message)]
pub struct Saved {}

/// How an account is doing.
#[derive(Clone, PartialEq, Message)]
pub struct Status {
    /// Whose.
    #[prost(string, tag = "1")]
    pub account: String,
    /// The `SyncState`, as JSON.
    #[prost(string, tag = "2")]
    pub state_json: String,
}

/// Noted.
#[derive(Clone, PartialEq, Message)]
pub struct Ack {}

/// The core could not do what was asked.
#[derive(Clone, PartialEq, Message)]
pub struct Failure {
    /// Why, for the connector's log and the account's state.
    #[prost(string, tag = "1")]
    pub detail: String,
}

/// One mail message, normalized by the connector, with its original bytes.
#[derive(Clone, PartialEq, Message)]
pub struct MailMessage {
    /// Account-wide identifier.
    #[prost(string, tag = "1")]
    pub external_id: String,
    /// Which conversation.
    #[prost(string, tag = "2")]
    pub thread_key: String,
    /// The message as it arrived.
    #[prost(bytes = "vec", tag = "3")]
    pub raw: Vec<u8>,
    /// Subject line.
    #[prost(string, tag = "4")]
    pub subject: String,
    /// Sender.
    #[prost(message, optional, tag = "5")]
    pub from: Option<MailAddress>,
    /// Recipients.
    #[prost(message, repeated, tag = "6")]
    pub to: Vec<MailAddress>,
    /// Copied.
    #[prost(message, repeated, tag = "7")]
    pub cc: Vec<MailAddress>,
    /// The sender's date, RFC 3339 with their offset.
    #[prost(string, optional, tag = "8")]
    pub date: Option<String>,
    /// `Message-ID` without brackets.
    #[prost(string, optional, tag = "9")]
    pub message_id: Option<String>,
    /// `In-Reply-To` without brackets.
    #[prost(string, optional, tag = "10")]
    pub in_reply_to: Option<String>,
    /// `References` without brackets.
    #[prost(string, repeated, tag = "11")]
    pub references: Vec<String>,
    /// Plain text body.
    #[prost(string, tag = "12")]
    pub text: String,
    /// Files that came with it.
    #[prost(message, repeated, tag = "13")]
    pub attachments: Vec<MailAttachment>,
    /// The headers the rules read.
    #[prost(message, repeated, tag = "14")]
    pub headers: Vec<MailHeader>,
}

/// An address with an optional name.
#[derive(Clone, PartialEq, Message)]
pub struct MailAddress {
    /// Lowercased.
    #[prost(string, tag = "1")]
    pub address: String,
    /// Display name.
    #[prost(string, optional, tag = "2")]
    pub name: Option<String>,
}

/// One attachment.
#[derive(Clone, PartialEq, Message)]
pub struct MailAttachment {
    /// File name.
    #[prost(string, optional, tag = "1")]
    pub name: Option<String>,
    /// Media type.
    #[prost(string, tag = "2")]
    pub mime: String,
    /// The bytes.
    #[prost(bytes = "vec", tag = "3")]
    pub bytes: Vec<u8>,
}

/// One header.
#[derive(Clone, PartialEq, Message)]
pub struct MailHeader {
    /// Lowercased name.
    #[prost(string, tag = "1")]
    pub name: String,
    /// Value.
    #[prost(string, tag = "2")]
    pub value: String,
}

/// Write one frame: a big-endian length, then the bytes.
pub async fn write_frame<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    envelope: &Envelope,
) -> io::Result<()> {
    let bytes = envelope.encode_to_vec();
    let len = u32::try_from(bytes.len())
        .ok()
        .filter(|l| *l <= MAX_FRAME)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await
}

/// Read one frame, or `None` when the other side has closed.
pub async fn read_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<Option<Envelope>> {
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes refused"),
        ));
    }
    let mut bytes = vec![0u8; len as usize];
    reader.read_exact(&mut bytes).await?;
    Envelope::decode(bytes.as_slice())
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// What can go wrong on a call.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    /// The connection is gone or the bytes made no sense.
    #[error("the core connection failed: {0}")]
    Io(#[from] io::Error),
    /// The core answered with a failure.
    #[error("the core refused: {0}")]
    Refused(String),
    /// The core answered with something that does not answer the question.
    #[error("the core answered out of turn")]
    Unexpected,
}

/// A connector's side of the socket.
pub struct Client {
    stream: UnixStream,
    next_id: u64,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

impl Client {
    /// Connect to the core's socket.
    pub async fn connect(path: &Path) -> io::Result<Self> {
        Ok(Self {
            stream: UnixStream::connect(path).await?,
            next_id: 1,
        })
    }

    /// Wrap an already connected stream, for tests and for a core that
    /// hands a connector a socket pair.
    #[must_use]
    pub const fn over(stream: UnixStream) -> Self {
        Self { stream, next_id: 1 }
    }

    /// One call: send, and wait for the one answer.
    pub async fn call(&mut self, body: Body) -> Result<Body, CallError> {
        let id = self.next_id;
        self.next_id += 1;
        write_frame(
            &mut self.stream,
            &Envelope {
                id,
                body: Some(body),
            },
        )
        .await?;
        let Some(reply) = read_frame(&mut self.stream).await? else {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the core closed the connection",
            )
            .into());
        };
        match reply.body {
            Some(Body::Failure(f)) => Err(CallError::Refused(f.detail)),
            Some(body) => Ok(body),
            None => Err(CallError::Unexpected),
        }
    }

    /// Introduce this process and learn what it is responsible for.
    pub async fn hello(&mut self, connector: &str, token: &str) -> Result<Assign, CallError> {
        match self
            .call(Body::Hello(Hello {
                connector: connector.to_owned(),
                token: token.to_owned(),
                pid: std::process::id(),
            }))
            .await?
        {
            Body::Assign(assign) => Ok(assign),
            _ => Err(CallError::Unexpected),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_frame_survives_the_wire() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let sent = Envelope {
            id: 7,
            body: Some(Body::Store(Store {
                account: "me@example.com".into(),
                messages: vec![MailMessage {
                    external_id: "gm:1".into(),
                    thread_key: "gm:t1".into(),
                    raw: b"From: a\r\n\r\nhi".to_vec(),
                    subject: "hi".into(),
                    from: Some(MailAddress {
                        address: "a@example.com".into(),
                        name: None,
                    }),
                    text: "hi".into(),
                    headers: vec![MailHeader {
                        name: "list-id".into(),
                        value: "x".into(),
                    }],
                    ..MailMessage::default()
                }],
                chats: vec![],
            })),
        };
        write_frame(&mut a, &sent).await.unwrap();
        let got = read_frame(&mut b).await.unwrap().unwrap();
        assert_eq!(got, sent);
        drop(a);
        assert!(read_frame(&mut b).await.unwrap().is_none(), "a clean close");
    }

    #[tokio::test]
    async fn an_oversized_frame_is_refused_before_it_is_read() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        a.write_all(&(MAX_FRAME + 1).to_be_bytes()).await.unwrap();
        let err = read_frame(&mut b).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn a_call_gets_its_answer_and_a_failure_becomes_an_error() {
        let (client_side, mut core_side) = UnixStream::pair().unwrap();
        let core = tokio::spawn(async move {
            let hello = read_frame(&mut core_side).await.unwrap().unwrap();
            assert!(matches!(hello.body, Some(Body::Hello(ref h)) if h.token == "t"));
            write_frame(
                &mut core_side,
                &Envelope {
                    id: hello.id,
                    body: Some(Body::Assign(Assign { accounts: vec![] })),
                },
            )
            .await
            .unwrap();
            let next = read_frame(&mut core_side).await.unwrap().unwrap();
            assert!(matches!(next.body, Some(Body::LoadCursor(_))));
            write_frame(
                &mut core_side,
                &Envelope {
                    id: next.id,
                    body: Some(Body::Failure(Failure {
                        detail: "no".into(),
                    })),
                },
            )
            .await
            .unwrap();
        });

        let mut client = Client::over(client_side);
        let assign = client.hello("imap", "t").await.unwrap();
        assert!(assign.accounts.is_empty());
        let err = client
            .call(Body::LoadCursor(LoadCursor {
                account: "a".into(),
                scope: "INBOX".into(),
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, CallError::Refused(ref d) if d == "no"));
        core.await.unwrap();
    }
}
