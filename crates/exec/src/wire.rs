//! The wire between the core and the host: the host's standard input and
//! output, length-prefixed JSON frames.
//!
//! Design: `docs/design/13-executors.md`, "宿主". Connectors speak protobuf
//! over a socket because a connector is long-lived and serves many accounts;
//! a host lives for one step, is started by the core with pipes nobody else
//! holds, and says a handful of things. JSON is enough.

use genatrix_dispatch::scope::{Answer, Ask, Scope};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::isolation::Isolation;

/// A frame larger than this is a bug or an attack.
pub const MAX_FRAME: u32 = 8 * 1024 * 1024;

/// An executor: its name and how it is started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Executor {
    /// Registry name; what delegation tickets name as their target.
    pub name: String,
    /// The command that starts its ACP adapter.
    pub command: Vec<String>,
    /// The search path it runs with. `sudo` resets the environment, so this
    /// is set inside the executor's own shell.
    #[serde(default)]
    pub path: Option<String>,
    /// The workspace whose sign-in it uses, if it has one of its own
    /// (design 14, invariant 32).
    #[serde(default)]
    pub sign_in: Option<String>,
}

/// Everything one session needs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Start {
    /// Which executor.
    pub executor: Executor,
    /// As whom it runs.
    pub isolation: Isolation,
    /// What it may do.
    pub scope: Scope,
    /// The text, exactly as the gate let it through.
    pub text: String,
    /// The delegation ticket for that text.
    pub ticket: String,
    /// The key delegation tickets are signed with, hex.
    pub key: String,
    /// Wall-clock limit for the session, in seconds.
    pub timeout_secs: u64,
}

/// Core to host.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "say", rename_all = "snake_case")]
pub enum ToHost {
    /// Start the session. The first frame, and only once.
    Start(Box<Start>),
    /// Stop now: the user pressed stop.
    Cancel,
}

/// What the executor said or did, for the record and the page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// `message`, `thought`, `tool`, `tool_update`, `plan`.
    pub kind: String,
    /// The text, cut to [`EVENT_TEXT`] bytes.
    pub text: String,
}

/// The most text one event keeps.
pub const EVENT_TEXT: usize = 4096;

/// How a session ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct End {
    /// ACP's stop reason (`end_turn`, `cancelled`, ...), or `failed`.
    pub stop: String,
    /// Everything the executor said to the user, joined: its answer.
    pub answer: String,
    /// What went wrong, when something did.
    pub error: Option<String>,
}

/// Host to core.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "say", rename_all = "snake_case")]
pub enum FromHost {
    /// Something happened.
    Event(Event),
    /// The executor asked for a permission and this was the answer.
    Asked {
        /// What it asked.
        ask: Ask,
        /// What it was told.
        answer: Answer,
    },
    /// The session is over. The last frame.
    End(End),
}

/// Write one frame.
pub async fn write<W: AsyncWrite + Unpin, T: Serialize>(
    w: &mut W,
    value: &T,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(value)?;
    let len = u32::try_from(body.len())
        .ok()
        .filter(|l| *l <= MAX_FRAME)
        .ok_or_else(|| std::io::Error::other("frame too large"))?;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await
}

/// Read one frame; `None` at a clean end of stream.
pub async fn read<R: AsyncRead + Unpin, T: for<'de> Deserialize<'de>>(
    r: &mut R,
) -> std::io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body).await?;
    Ok(Some(serde_json::from_slice(&body)?))
}

/// Cut text to at most `max` bytes on a character boundary.
#[must_use]
pub fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}
