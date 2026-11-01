//! One executor session: check the ticket, start the executor, send the one
//! prompt, answer its permission requests by the scope, report what it says.
//!
//! Design: `docs/design/13-executors.md`, "宿主" and "两层批准"; proposed
//! invariants 22 and 25.

use std::path::{Path, PathBuf};
use std::time::Duration;

use genatrix_dispatch::scope::{self, Answer, Ask, ToolKind};
use genatrix_keys::TicketKey;
use genatrix_llm::ticket::{Purpose, TicketStore};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::acp::{self, AUTH_REQUIRED, Client, Incoming, METHOD_NOT_FOUND};
use crate::isolation::Isolation;
use crate::wire::{EVENT_TEXT, End, Event, FromHost, Start, cut};

/// How long a cancelled session gets to wind down before it is ended.
const WIND_DOWN: Duration = Duration::from_secs(10);

/// Check that `text` may be sent to `executor` under `ticket`: signed with
/// `key`, unexpired, unused, for exactly these bytes, this executor, and the
/// delegate purpose at a level that may leave the device.
pub fn admit(ticket: &str, text: &str, executor: &str, key: &str) -> Result<(), String> {
    let key = TicketKey::from_hex(key).map_err(|_| "the delegation key is malformed".to_owned())?;
    let t = TicketStore::new()
        .admit(Some(ticket), text.as_bytes(), executor, &key)
        .map_err(|e| format!("the delegation ticket was refused: {e}"))?;
    if t.purpose != Purpose::Delegate {
        return Err(format!(
            "the ticket is for `{}`, not for delegation",
            t.purpose.as_str()
        ));
    }
    if !t.level.may_leave_device() {
        return Err(format!(
            "the ticket is for {} content, which does not leave this machine",
            t.level.as_str()
        ));
    }
    Ok(())
}

/// Run one session to its end. `out` gets every event and permission
/// answer; the returned [`End`] is the last word. Nothing reaches the
/// executor unless [`admit`] passes first.
pub async fn run(
    start: Start,
    out: mpsc::UnboundedSender<FromHost>,
    mut cancel: oneshot::Receiver<()>,
) -> End {
    if let Err(e) = admit(&start.ticket, &start.text, &start.executor.name, &start.key) {
        return failed("refused", e);
    }
    let workspace = start.scope.workspace.clone();
    // `cd` into the workspace as the executor's user: the host itself may
    // not be allowed into it. The path and the search path are arguments,
    // never shell text.
    let mut argv = vec![
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        "cd \"$0\" || exit 1; if [ -n \"$1\" ]; then PATH=\"$1\"; export PATH; fi; shift; exec \"$@\""
            .to_owned(),
        workspace.clone(),
        start.executor.path.clone().unwrap_or_default(),
    ];
    argv.extend(start.executor.command.iter().cloned());
    let argv = start.isolation.wrap(&argv);
    let isolated = matches!(start.isolation, Isolation::User { .. });
    let env = if isolated {
        vec![(
            "PATH".to_owned(),
            "/usr/bin:/bin:/usr/sbin:/sbin".to_owned(),
        )]
    } else {
        Vec::new()
    };
    let (client, mut incoming) = match Client::spawn(&argv, &env, isolated) {
        Ok(c) => c,
        Err(e) => return failed("failed", format!("could not start the executor: {e}")),
    };

    let outcome = tokio::time::timeout(
        Duration::from_secs(start.timeout_secs.max(1)),
        converse(&client, &mut incoming, &start, &out, &mut cancel),
    )
    .await;
    let end = match outcome {
        Ok(end) => end,
        Err(_) => failed(
            "cancelled",
            format!(
                "the step ran past its {} seconds and was stopped",
                start.timeout_secs
            ),
        ),
    };
    let tail = client.stderr_tail();
    client.end().await;
    match end.error {
        Some(ref e) if !tail.trim().is_empty() && end.stop == "failed" => End {
            error: Some(format!(
                "{e}\n\nThe executor said:\n{}",
                cut(tail.trim(), 2000)
            )),
            ..end
        },
        _ => end,
    }
}

#[allow(clippy::too_many_lines)] // one ACP conversation, start to end, reads best whole
async fn converse(
    client: &Client,
    incoming: &mut mpsc::UnboundedReceiver<Incoming>,
    start: &Start,
    out: &mpsc::UnboundedSender<FromHost>,
    cancel: &mut oneshot::Receiver<()>,
) -> End {
    let mut answer = String::new();

    // Starting up and opening a session may also bring requests; none are
    // expected, and any that come are refused while we wait.
    let init = match call(
        client,
        incoming,
        start,
        out,
        &mut answer,
        "initialize",
        acp::initialize_params(),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return failed("failed", format!("the executor did not start ACP: {e}")),
    };
    let version = init
        .get("protocolVersion")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if version != acp::PROTOCOL_VERSION {
        return failed(
            "failed",
            format!("the executor speaks ACP version {version}; this host speaks 1"),
        );
    }
    let session = match call(
        client,
        incoming,
        start,
        out,
        &mut answer,
        "session/new",
        json!({"cwd": start.scope.workspace, "mcpServers": []}),
    )
    .await
    {
        Ok(v) => v,
        Err(e) if e.code == AUTH_REQUIRED => {
            return failed(
                "failed",
                format!(
                    "`{}` is not signed in for the executor's user; run `genatrix exec login {}`",
                    start.executor.name, start.executor.name
                ),
            );
        }
        Err(e) => {
            return failed(
                "failed",
                format!("the executor would not open a session: {e}"),
            );
        }
    };
    let Some(session_id) = session.get("sessionId").cloned() else {
        return failed("failed", "the executor opened a session with no id".into());
    };

    let prompt = json!({
        "sessionId": session_id,
        "prompt": [{"type": "text", "text": start.text}]
    });
    let mut reply = match client.request("session/prompt", prompt).await {
        Ok(rx) => rx,
        Err(e) => return failed("failed", format!("could not send the prompt: {e}")),
    };
    loop {
        tokio::select! {
            r = &mut reply => {
                // The executor's last words came before its answer; the
                // reader queued them first, so they are waiting here.
                while let Ok(m) = incoming.try_recv() {
                    handle(client, m, start, out, &mut answer).await;
                }
                return match r {
                    Ok(Ok(v)) => End {
                        stop: v.get("stopReason").and_then(Value::as_str).unwrap_or("end_turn").to_owned(),
                        answer,
                        error: None,
                    },
                    Ok(Err(e)) => End { stop: "failed".into(), answer, error: Some(e.to_string()) },
                    Err(_) => End { stop: "failed".into(), answer, error: Some("the executor ended".into()) },
                };
            }
            m = incoming.recv() => {
                let Some(m) = m else {
                    return End { stop: "failed".into(), answer, error: Some("the executor ended".into()) };
                };
                handle(client, m, start, out, &mut answer).await;
            }
            _ = &mut *cancel => {
                let _ = client.notify("session/cancel", json!({"sessionId": session_id})).await;
                // Give it a moment to say it stopped; then it is ended anyway.
                let wound = tokio::time::timeout(WIND_DOWN, async {
                    loop {
                        tokio::select! {
                            r = &mut reply => return r.ok(),
                            m = incoming.recv() => match m {
                                Some(m) => handle(client, m, start, out, &mut answer).await,
                                None => return None,
                            },
                        }
                    }
                }).await;
                let _ = wound;
                return End { stop: "cancelled".into(), answer, error: Some("stopped by the user".into()) };
            }
        }
    }
}

/// A request whose answer is awaited while the agent's own requests are
/// handled.
async fn call(
    client: &Client,
    incoming: &mut mpsc::UnboundedReceiver<Incoming>,
    start: &Start,
    out: &mpsc::UnboundedSender<FromHost>,
    answer: &mut String,
    method: &str,
    params: Value,
) -> Result<Value, acp::RpcError> {
    let mut rx = client
        .request(method, params)
        .await
        .map_err(|e| acp::RpcError {
            code: -32098,
            message: e.to_string(),
        })?;
    loop {
        tokio::select! {
            r = &mut rx => {
                return r.unwrap_or_else(|_| Err(acp::RpcError { code: -32099, message: "the executor ended".into() }));
            }
            m = incoming.recv() => match m {
                Some(m) => handle(client, m, start, out, answer).await,
                None => return Err(acp::RpcError { code: -32099, message: "the executor ended".into() }),
            },
        }
    }
}

async fn handle(
    client: &Client,
    message: Incoming,
    start: &Start,
    out: &mpsc::UnboundedSender<FromHost>,
    answer: &mut String,
) {
    match message {
        Incoming::Request { id, method, params } if method == "session/request_permission" => {
            let ask = ask_of(&params, &start.scope.workspace);
            let verdict = scope::answer(&start.scope, &ask);
            let allow = verdict == Answer::Allow;
            let _ = out.send(FromHost::Asked {
                ask,
                answer: verdict,
            });
            let options = params.get("options").cloned().unwrap_or(Value::Null);
            let _ = client
                .respond(id, acp::permission_outcome(&options, allow))
                .await;
        }
        Incoming::Request { id, method, .. } => {
            // File system, terminal, and anything else: not offered.
            let _ = client
                .respond_error(id, METHOD_NOT_FOUND, &format!("`{method}` is not offered"))
                .await;
        }
        Incoming::Notification { method, params } if method == "session/update" => {
            if let Some(event) = event_of(&params, answer) {
                let _ = out.send(FromHost::Event(event));
            }
        }
        Incoming::Notification { .. } => {}
    }
}

/// Turn an update into an event, and keep the executor's words to the user.
fn event_of(params: &Value, answer: &mut String) -> Option<Event> {
    let update = params.get("update")?;
    let kind = update.get("sessionUpdate").and_then(Value::as_str)?;
    let text_of = |v: &Value| {
        v.get("content")
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let (kind, text) = match kind {
        "agent_message_chunk" => {
            let t = text_of(update);
            answer.push_str(&t);
            ("message", t)
        }
        "agent_thought_chunk" => ("thought", text_of(update)),
        "tool_call" => (
            "tool",
            update
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("a tool")
                .to_owned(),
        ),
        "tool_call_update" => {
            let status = update.get("status").and_then(Value::as_str)?;
            let title = update
                .get("title")
                .and_then(Value::as_str)
                .or_else(|| update.get("toolCallId").and_then(Value::as_str))
                .unwrap_or("a tool");
            ("tool_update", format!("{title}: {status}"))
        }
        "plan" => (
            "plan",
            update
                .get("entries")
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|e| {
                            Some(format!(
                                "[{}] {}",
                                e.get("status").and_then(Value::as_str).unwrap_or("?"),
                                e.get("content").and_then(Value::as_str)?
                            ))
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
        ),
        _ => return None,
    };
    if text.is_empty() {
        return None;
    }
    Some(Event {
        kind: kind.to_owned(),
        text: cut(&text, EVENT_TEXT),
    })
}

/// What a permission request asks for, as the scope reads it.
#[must_use]
pub fn ask_of(params: &Value, workspace: &str) -> Ask {
    let call = params.get("toolCall").cloned().unwrap_or(Value::Null);
    let kind: ToolKind = call
        .get("kind")
        .cloned()
        .and_then(|k| serde_json::from_value(k).ok())
        .unwrap_or(ToolKind::Other);
    let raw = call.get("rawInput").cloned().unwrap_or(Value::Null);
    let mut paths: Vec<String> = call
        .get("locations")
        .and_then(Value::as_array)
        .map(|ls| {
            ls.iter()
                .filter_map(|l| l.get("path").and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    for key in [
        "file_path",
        "path",
        "notebook_path",
        "abs_path",
        "destination",
    ] {
        if let Some(p) = raw.get(key).and_then(Value::as_str) {
            paths.push(p.to_owned());
        }
    }
    let command = match raw.get("command") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(parts)) => {
            let parts: Vec<String> = parts
                .iter()
                .filter_map(|p| p.as_str().map(str::to_owned))
                .collect();
            // `["bash", "-lc", "cargo test"]` is the command `cargo test`.
            match parts.as_slice() {
                [shell, flag, inner]
                    if ["bash", "sh", "zsh", "/bin/bash", "/bin/sh", "/bin/zsh"]
                        .contains(&shell.as_str())
                        && ["-c", "-lc"].contains(&flag.as_str()) =>
                {
                    Some(inner.clone())
                }
                _ => Some(parts.join(" ")),
            }
        }
        _ => None,
    };
    Ask {
        kind,
        paths: paths.iter().map(|p| resolve(p, workspace)).collect(),
        command,
    }
}

/// An absolute path with symbolic links resolved as far as the path exists
/// and this process may look. Where it may not, the path stays as written
/// and the scope judges it by its words; the executor's own user, which
/// cannot read what is not its own, holds underneath.
fn resolve(path: &str, workspace: &str) -> String {
    let full = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        Path::new(workspace).join(path)
    };
    let Some(lexical) = scope::normalize(&full.to_string_lossy()) else {
        return full.to_string_lossy().into_owned();
    };
    let mut existing = PathBuf::from(&lexical);
    let mut rest = Vec::new();
    while !existing.exists() {
        match (
            existing.file_name().map(ToOwned::to_owned),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => return lexical,
        }
    }
    let Ok(mut real) = existing.canonicalize() else {
        return lexical;
    };
    // The workspace itself may sit behind a link (`/tmp` is `/private/tmp`);
    // judge paths in its own spelling.
    if let Ok(root) = Path::new(workspace).canonicalize()
        && let Ok(inner) = real.strip_prefix(&root)
    {
        real = Path::new(workspace).join(inner);
    }
    for name in rest.into_iter().rev() {
        real.push(name);
    }
    real.to_string_lossy().into_owned()
}

fn failed(stop: &str, error: String) -> End {
    End {
        stop: stop.to_owned(),
        answer: String::new(),
        error: Some(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_wrapped_command_is_judged_by_what_it_runs() {
        let params = json!({"toolCall": {"kind": "execute", "rawInput": {"command": ["bash", "-lc", "cargo test"]}}});
        assert_eq!(ask_of(&params, "/w").command.as_deref(), Some("cargo test"));
        let params =
            json!({"toolCall": {"kind": "execute", "rawInput": {"command": "npm run check"}}});
        assert_eq!(
            ask_of(&params, "/w").command.as_deref(),
            Some("npm run check")
        );
    }

    #[test]
    fn an_unknown_kind_is_other_and_paths_come_from_both_places() {
        let params = json!({"toolCall": {
            "kind": "teleport",
            "locations": [{"path": "/w/a.rs"}],
            "rawInput": {"file_path": "b.rs"}
        }});
        let ask = ask_of(&params, "/w");
        assert_eq!(ask.kind, ToolKind::Other);
        assert_eq!(ask.paths, vec!["/w/a.rs", "/w/b.rs"]);
    }

    #[test]
    fn a_link_out_of_the_workspace_is_seen_for_what_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("escape")).unwrap();
        let ws_s = ws.to_string_lossy().into_owned();
        let resolved = resolve("escape/secret.txt", &ws_s);
        assert!(!resolved.starts_with(&ws_s), "{resolved}");
        let inside = resolve("src/new.rs", &ws_s);
        assert_eq!(inside, format!("{ws_s}/src/new.rs"));
    }
}
