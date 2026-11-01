//! A stand-in executor that speaks ACP, for the tests. Never installed.
//!
//! It does what its prompt tells it in lines that start with `@fake`:
//!
//! - `@fake write <path> <text>`: ask to edit, write the file if allowed
//! - `@fake run <command>`: ask to execute; nothing is run
//! - `@fake read <path>`: ask to read
//! - `@fake mode`: ask to switch its own mode
//! - `@fake say <text>`: say it
//! - `@fake sleep <seconds>`: wait
//! - `@fake leave`: exit without answering
//!
//! After each request it says `allowed <what>` or `denied <what>`, so a test
//! can see the answer from the executor's side too. With the environment
//! variable `FAKE_AUTH` set, it asks to be signed in instead of opening a
//! session.

#![allow(clippy::too_many_lines, clippy::match_same_arms)] // a test fixture, one script

use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn send(value: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

fn read(lines: &mut impl Iterator<Item = std::io::Result<String>>) -> Option<Value> {
    loop {
        let line = lines.next()?.ok()?;
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            return Some(v);
        }
    }
}

fn say(session: &Value, text: &str) {
    send(
        &json!({"jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": session,
            "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}
        }}),
    );
}

fn main() {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut next_id = 1000u64;
    while let Some(msg) = read(&mut lines) {
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match method.as_str() {
            "initialize" => send(&json!({"jsonrpc": "2.0", "id": id, "result": {
                "protocolVersion": 1, "agentCapabilities": {}, "authMethods": []
            }})),
            "session/new" => {
                if std::env::var_os("FAKE_AUTH").is_some() {
                    send(
                        &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": "Authentication required"}}),
                    );
                } else {
                    send(&json!({"jsonrpc": "2.0", "id": id, "result": {"sessionId": "s1"}}));
                }
            }
            "session/cancel" => {}
            "session/prompt" => {
                let session = params.get("sessionId").cloned().unwrap_or(Value::Null);
                let text = params["prompt"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let cwd = std::env::current_dir().unwrap_or_default();
                let mut stop = "end_turn";
                for line in text.lines().filter_map(|l| l.trim().strip_prefix("@fake ")) {
                    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
                    let ask = |kind: &str,
                               raw: Value,
                               locations: Value,
                               next_id: &mut u64,
                               lines: &mut std::io::Lines<std::io::StdinLock<'_>>|
                     -> bool {
                        *next_id += 1;
                        let rid = *next_id;
                        send(
                            &json!({"jsonrpc": "2.0", "id": rid, "method": "session/request_permission", "params": {
                                "sessionId": session,
                                "toolCall": {"toolCallId": format!("t{rid}"), "title": kind, "kind": kind, "rawInput": raw, "locations": locations},
                                "options": [
                                    {"optionId": "always", "name": "Always", "kind": "allow_always"},
                                    {"optionId": "yes", "name": "Yes", "kind": "allow_once"},
                                    {"optionId": "no", "name": "No", "kind": "reject_once"}
                                ]
                            }}),
                        );
                        loop {
                            let Some(reply) = read(lines) else {
                                return false;
                            };
                            if reply.get("id").and_then(Value::as_u64) == Some(rid) {
                                return reply["result"]["outcome"]["optionId"] == "yes";
                            }
                        }
                    };
                    match verb {
                        "write" => {
                            let (path, body) = rest.split_once(' ').unwrap_or((rest, ""));
                            let full = cwd.join(path);
                            let ok = ask(
                                "edit",
                                json!({"file_path": path}),
                                json!([{"path": full}]),
                                &mut next_id,
                                &mut lines,
                            );
                            if ok {
                                if let Some(dir) = full.parent() {
                                    let _ = std::fs::create_dir_all(dir);
                                }
                                let _ = std::fs::write(&full, body);
                            }
                            say(
                                &session,
                                &format!(
                                    "{} write {path}\n",
                                    if ok { "allowed" } else { "denied" }
                                ),
                            );
                        }
                        "run" => {
                            let ok = ask(
                                "execute",
                                json!({"command": rest}),
                                json!([]),
                                &mut next_id,
                                &mut lines,
                            );
                            say(
                                &session,
                                &format!("{} run {rest}\n", if ok { "allowed" } else { "denied" }),
                            );
                        }
                        "read" => {
                            let ok = ask(
                                "read",
                                json!({"path": rest}),
                                json!([{"path": rest}]),
                                &mut next_id,
                                &mut lines,
                            );
                            say(
                                &session,
                                &format!("{} read {rest}\n", if ok { "allowed" } else { "denied" }),
                            );
                        }
                        "mode" => {
                            let ok = ask(
                                "switch_mode",
                                json!({}),
                                json!([]),
                                &mut next_id,
                                &mut lines,
                            );
                            say(
                                &session,
                                &format!("{} mode\n", if ok { "allowed" } else { "denied" }),
                            );
                        }
                        "say" => say(&session, &format!("{rest}\n")),
                        "sleep" => {
                            let secs: u64 = rest.trim().parse().unwrap_or(1);
                            std::thread::sleep(std::time::Duration::from_secs(secs));
                            stop = "end_turn";
                        }
                        "leave" => return,
                        _ => {}
                    }
                }
                send(&json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": stop}}));
            }
            "fs/read_text_file" => {}
            _ => {
                if !id.is_null() && !method.is_empty() {
                    send(
                        &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "no"}}),
                    );
                }
            }
        }
    }
}
