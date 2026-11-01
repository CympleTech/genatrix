//! A small ACP client: JSON-RPC 2.0, one JSON object per line, over an
//! executor's standard input and output.
//!
//! Design: `docs/design/13-executors.md`, "ACP". Genatrix is the client and
//! the executor is the agent. Only what a session needs is here: start up,
//! open a session, send one prompt, and meanwhile answer the agent's own
//! requests. The client advertises no file system and no terminal, so the
//! agent works with its own tools in its own workspace and never borrows
//! the core's; any request for those is answered "method not found".
//!
//! Written here rather than taken from the `agent-client-protocol` crate:
//! that crate's API has moved between minor versions, and the part a client
//! needs is a few dozen lines of JSON. The shapes follow protocol version 1.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc, oneshot};

/// The protocol version this client speaks.
pub const PROTOCOL_VERSION: u64 = 1;

/// JSON-RPC's "method not found".
pub const METHOD_NOT_FOUND: i64 = -32601;

/// ACP's "authentication required".
pub const AUTH_REQUIRED: i64 = -32000;

/// The longest line accepted from an executor.
const MAX_LINE: usize = 8 * 1024 * 1024;

/// An error the agent answered with.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message} ({code})")]
pub struct RpcError {
    /// The JSON-RPC code.
    pub code: i64,
    /// Its message.
    pub message: String,
}

/// What a request came back with.
pub type Reply = Result<Value, RpcError>;

/// Something the agent sent unasked.
#[derive(Clone, Debug)]
pub enum Incoming {
    /// A request, which wants an answer.
    Request {
        /// Its id, echoed in the answer.
        id: Value,
        /// The method.
        method: String,
        /// Its parameters.
        params: Value,
    },
    /// A notification.
    Notification {
        /// The method.
        method: String,
        /// Its parameters.
        params: Value,
    },
}

/// A running executor and the conversation with it.
pub struct Client {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<StdMutex<HashMap<u64, oneshot::Sender<Reply>>>>,
    next: AtomicU64,
    child: Child,
    stderr: Arc<StdMutex<String>>,
}

impl Client {
    /// Start `argv` and begin reading what it says.
    pub fn spawn(
        argv: &[String],
        env: &[(String, String)],
        clear_env: bool,
    ) -> std::io::Result<(Self, mpsc::UnboundedReceiver<Incoming>)> {
        let (program, rest) = argv
            .split_first()
            .ok_or_else(|| std::io::Error::other("no command for this executor"))?;
        let mut command = Command::new(program);
        command
            .args(rest)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if clear_env {
            command.env_clear();
        }
        for (k, v) in env {
            command.env(k, v);
        }
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("no stdout"))?;
        let mut err = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("no stderr"))?;

        let pending: Arc<StdMutex<HashMap<u64, oneshot::Sender<Reply>>>> = Arc::default();
        let (tx, rx) = mpsc::unbounded_channel();
        let stderr = Arc::new(StdMutex::new(String::new()));

        let tail = Arc::clone(&stderr);
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while let Ok(n) = err.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut t = tail
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                t.push_str(&String::from_utf8_lossy(&buf[..n]));
                if t.len() > 8192 {
                    let cut = t.len() - 4096;
                    let cut = (cut..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(0);
                    t.drain(..cut);
                }
            }
        });

        let waiting = Arc::clone(&pending);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match lines.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) if line.len() > MAX_LINE => break,
                    Ok(_) => {}
                }
                let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                    // Not JSON: an adapter's stray log line. Ignored.
                    continue;
                };
                dispatch(&value, &waiting, &tx);
            }
            // The executor is gone: everyone still waiting hears so.
            let mut w = waiting
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (_, s) in w.drain() {
                let _ = s.send(Err(RpcError {
                    code: -32099,
                    message: "the executor ended".into(),
                }));
            }
        });

        Ok((
            Self {
                stdin: Arc::new(Mutex::new(stdin)),
                pending,
                next: AtomicU64::new(1),
                child,
                stderr,
            },
            rx,
        ))
    }

    async fn send(&self, value: &Value) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&line).await?;
        stdin.flush().await
    }

    /// Send a request; the receiver gets its answer.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
    ) -> std::io::Result<oneshot::Receiver<Reply>> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, tx);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        Ok(rx)
    }

    /// Send a request and wait for its answer.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let rx = self.request(method, params).await.map_err(|e| RpcError {
            code: -32098,
            message: e.to_string(),
        })?;
        rx.await.unwrap_or_else(|_| {
            Err(RpcError {
                code: -32099,
                message: "the executor ended".into(),
            })
        })
    }

    /// Send a notification.
    pub async fn notify(&self, method: &str, params: Value) -> std::io::Result<()> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    /// Answer one of the agent's requests.
    pub async fn respond(&self, id: Value, result: Value) -> std::io::Result<()> {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}))
            .await
    }

    /// Answer one of the agent's requests with an error.
    pub async fn respond_error(&self, id: Value, code: i64, message: &str) -> std::io::Result<()> {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
            .await
    }

    /// The last of what the executor wrote to its standard error.
    #[must_use]
    pub fn stderr_tail(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// End the executor: ask politely, then make sure. Through `sudo`, a
    /// terminate signal is passed on to the program; a kill would stop only
    /// `sudo` and leave the program running.
    pub async fn end(mut self) {
        if let Some(pid) = self.child.id() {
            let _ = Command::new("/bin/kill")
                .args(["-TERM", &pid.to_string()])
                .status()
                .await;
        }
        let waited =
            tokio::time::timeout(std::time::Duration::from_secs(5), self.child.wait()).await;
        if waited.is_err() {
            let _ = self.child.start_kill();
            let _ = self.child.wait().await;
        }
    }
}

fn dispatch(
    value: &Value,
    pending: &StdMutex<HashMap<u64, oneshot::Sender<Reply>>>,
    incoming: &mpsc::UnboundedSender<Incoming>,
) {
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    match (method, value.get("id").cloned()) {
        (Some(method), Some(id)) if !id.is_null() => {
            let _ = incoming.send(Incoming::Request { id, method, params });
        }
        (Some(method), _) => {
            let _ = incoming.send(Incoming::Notification { method, params });
        }
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else { return };
            let Some(sender) = pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id)
            else {
                return;
            };
            let reply = if let Some(e) = value.get("error") {
                Err(RpcError {
                    code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: e
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("error")
                        .to_owned(),
                })
            } else {
                Ok(value.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = sender.send(reply);
        }
        (None, None) => {}
    }
}

/// The parameters of `initialize`: no file system, no terminal.
#[must_use]
pub fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false
        },
        "clientInfo": {"name": "genatrix", "version": env!("CARGO_PKG_VERSION")}
    })
}

/// The answer to a permission request: the first option of the wanted
/// kind, preferring the one-time kinds, so an "always" is never chosen when
/// a "once" is offered. With no fitting option, the request is cancelled,
/// which ACP reads as a refusal.
#[must_use]
pub fn permission_outcome(options: &Value, allow: bool) -> Value {
    let wanted: &[&str] = if allow {
        &["allow_once", "allow_always"]
    } else {
        &["reject_once", "reject_always"]
    };
    let list = options.as_array().cloned().unwrap_or_default();
    for kind in wanted {
        if let Some(id) = list
            .iter()
            .find(|o| o.get("kind").and_then(Value::as_str) == Some(kind))
            .and_then(|o| o.get("optionId").cloned())
        {
            return json!({"outcome": {"outcome": "selected", "optionId": id}});
        }
    }
    json!({"outcome": {"outcome": "cancelled"}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_picks_a_reject_option_and_never_an_always_allow() {
        let options = json!([
            {"optionId": "a", "name": "Always", "kind": "allow_always"},
            {"optionId": "o", "name": "Once", "kind": "allow_once"},
            {"optionId": "r", "name": "No", "kind": "reject_once"}
        ]);
        assert_eq!(
            permission_outcome(&options, true)["outcome"]["optionId"],
            "o"
        );
        assert_eq!(
            permission_outcome(&options, false)["outcome"]["optionId"],
            "r"
        );
        let only_allow = json!([{"optionId": "o", "kind": "allow_once"}]);
        assert_eq!(
            permission_outcome(&only_allow, false)["outcome"]["outcome"],
            "cancelled"
        );
    }

    #[test]
    fn the_client_offers_no_file_system_and_no_terminal() {
        let p = initialize_params();
        assert_eq!(p["clientCapabilities"]["fs"]["readTextFile"], false);
        assert_eq!(p["clientCapabilities"]["fs"]["writeTextFile"], false);
        assert_eq!(p["clientCapabilities"]["terminal"], false);
    }
}
