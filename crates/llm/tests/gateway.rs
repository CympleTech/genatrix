//! The gateway over a real socket, with a stub standing in for the local
//! inference process.
//!
//! These tests exercise the promise of design 02: nothing reaches a model
//! without a ticket that matches the exact bytes, and nothing that may not
//! leave the device reaches a cloud entry. The stub records everything it is
//! asked, so "the request never arrived" is an assertion, not an inference.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::routing::post;
use axum::{Json, Router};
use bytes::Bytes;
use genatrix_keys::TicketKey;
use genatrix_llm::config::Config;
use genatrix_llm::local::LocalClient;
use genatrix_llm::registry::{Endpoint, ModelEntry, Registry};
use genatrix_llm::server::{Gateway, TICKET_HEADER, router};
use genatrix_llm::ticket::{Purpose, Ticket, TicketLevel};
use serde_json::{Value, json};
use tokio::net::UnixListener;

/// What the stub upstream saw.
type Seen = Arc<Mutex<Vec<Value>>>;

struct Harness {
    gateway_socket: PathBuf,
    seen: Seen,
    _dir: tempfile::TempDir,
}

fn key() -> TicketKey {
    TicketKey::from_bytes([42; 32])
}

fn start() -> Harness {
    // Short directory: macOS caps Unix socket paths at 104 bytes.
    let dir = tempfile::Builder::new()
        .prefix("gx")
        .tempdir_in(std::env::temp_dir())
        .unwrap();
    let upstream_socket = dir.path().join("up.sock");
    let gateway_socket = dir.path().join("gw.sock");

    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = seen.clone();
    let stub = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let recorder = recorder.clone();
            async move {
                recorder.lock().unwrap().push(body);
                Json(json!({
                    "id": "chatcmpl-stub",
                    "object": "chat.completion",
                    "created": 0,
                    "model": "stub",
                    "choices": [{
                        "index": 0,
                        "message": { "role": "assistant", "content": "ok" },
                        "finish_reason": "stop"
                    }]
                }))
            }
        }),
    );
    let up_listener = UnixListener::bind(&upstream_socket).unwrap();
    tokio::spawn(async move {
        axum::serve(up_listener, stub).await.unwrap();
    });

    let config = Config {
        socket: gateway_socket.clone(),
        registry: Registry {
            models: vec![
                ModelEntry {
                    name: "local".into(),
                    endpoint: Endpoint::LocalSocket {
                        path: upstream_socket.to_string_lossy().into_owned(),
                    },
                    model: "qwen3-8b-4bit".into(),
                    purposes: genatrix_llm::registry::ALL_PURPOSES.to_vec(),
                    context_length: 32_768,
                },
                ModelEntry {
                    name: "cloud".into(),
                    endpoint: Endpoint::Anthropic {
                        base_url: "https://api.anthropic.com".into(),
                        key_ref: "anthropic".into(),
                    },
                    model: "claude-sonnet-5".into(),
                    purposes: vec![Purpose::Summarize, Purpose::Draft],
                    context_length: 200_000,
                },
            ],
            chains: BTreeMap::default(),
        },
    };
    config.registry.validate().unwrap();

    let gw_listener = UnixListener::bind(&gateway_socket).unwrap();
    let gateway = Arc::new(Gateway::new(config, key()));
    tokio::spawn(async move {
        axum::serve(gw_listener, router(gateway)).await.unwrap();
    });

    Harness {
        gateway_socket,
        seen,
        _dir: dir,
    }
}

impl Harness {
    fn upstream_calls(&self) -> Vec<Value> {
        self.seen.lock().unwrap().clone()
    }

    async fn send(&self, body: &Bytes, ticket: Option<&str>) -> (u16, Value) {
        // Reuse our own Unix client; it is the only HTTP client in the crate.
        let client = LocalClient::new(&self.gateway_socket);
        let (status, body) = client
            .post_with_header(
                "/v1/chat/completions",
                body.clone(),
                ticket.map(|t| (TICKET_HEADER, t)),
            )
            .await
            .unwrap();
        let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, value)
    }
}

fn body(model: &str, text: &str) -> Bytes {
    Bytes::from(
        json!({ "model": model, "messages": [{ "role": "user", "content": text }] }).to_string(),
    )
}

fn ticket(b: &Bytes, target: &str, purpose: Purpose, level: TicketLevel) -> String {
    Ticket::issue(b, target, "m", purpose, level, "gate")
        .unwrap()
        .encode(&key())
}

#[tokio::test]
async fn a_ticketed_request_reaches_the_local_model_with_the_upstream_name() {
    let h = start();
    let b = body("local", "summarize this");
    let t = ticket(&b, "local", Purpose::Summarize, TicketLevel::Personal);
    let (status, reply) = h.send(&b, Some(&t)).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["choices"][0]["message"]["content"], "ok");
    let calls = h.upstream_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0]["model"], "qwen3-8b-4bit",
        "the registry name is rewritten to the model the process knows"
    );
    assert_eq!(calls[0]["messages"][0]["content"], "summarize this");
}

#[tokio::test]
async fn without_a_ticket_nothing_reaches_the_model() {
    let h = start();
    let b = body("local", "please summarize");
    let (status, _) = h.send(&b, None).await;
    assert_eq!(status, 401);
    assert!(h.upstream_calls().is_empty());
}

#[tokio::test]
async fn a_ticket_cannot_be_reused() {
    let h = start();
    let b = body("local", "one");
    let t = ticket(&b, "local", Purpose::Summarize, TicketLevel::Public);
    assert_eq!(h.send(&b, Some(&t)).await.0, 200);
    let (status, err) = h.send(&b, Some(&t)).await;
    assert_eq!(status, 401);
    assert_eq!(err["error"]["type"], "ticket_rejected");
    assert_eq!(
        h.upstream_calls().len(),
        1,
        "the replay never reached the model"
    );
}

#[tokio::test]
async fn swapping_the_body_after_approval_is_caught() {
    let h = start();
    let approved = body("local", "the text the gate saw");
    let t = ticket(
        &approved,
        "local",
        Purpose::Summarize,
        TicketLevel::Personal,
    );
    let smuggled = body("local", "my card number is 4111 1111 1111 1111");
    let (status, err) = h.send(&smuggled, Some(&t)).await;
    assert_eq!(status, 401);
    assert_eq!(err["error"]["type"], "ticket_rejected");
    assert!(h.upstream_calls().is_empty());
}

#[tokio::test]
async fn secret_content_is_refused_at_a_cloud_model_and_allowed_locally() {
    let h = start();

    let to_cloud = body("cloud", "HbA1c 7.1%");
    let t = ticket(&to_cloud, "cloud", Purpose::Draft, TicketLevel::Secret);
    let (status, err) = h.send(&to_cloud, Some(&t)).await;
    assert_eq!(status, 403);
    assert_eq!(err["error"]["type"], "policy_refused");

    let to_local = body("local", "HbA1c 7.1%");
    let t = ticket(&to_local, "local", Purpose::Draft, TicketLevel::Secret);
    assert_eq!(h.send(&to_local, Some(&t)).await.0, 200);
    assert_eq!(h.upstream_calls().len(), 1);
}

#[tokio::test]
async fn classification_is_refused_at_a_cloud_model_however_it_is_ticketed() {
    let h = start();
    let b = body("cloud", "some message");
    let t = ticket(&b, "cloud", Purpose::Classify, TicketLevel::Public);
    let (status, err) = h.send(&b, Some(&t)).await;
    assert_eq!(status, 403);
    assert_eq!(err["error"]["type"], "policy_refused");
}

#[tokio::test]
async fn a_permitted_cloud_request_is_not_sent_because_this_build_has_no_cloud() {
    let h = start();
    let b = body("cloud", "a redacted summary request");
    let t = ticket(&b, "cloud", Purpose::Summarize, TicketLevel::Redacted);
    let (status, err) = h.send(&b, Some(&t)).await;
    assert_eq!(
        status, 501,
        "policy allowed it; the transport does not exist"
    );
    assert_eq!(err["error"]["type"], "cloud_not_enabled");
}

#[tokio::test]
async fn a_forged_ticket_is_refused() {
    let h = start();
    let b = body("local", "hello");
    let forged = Ticket::issue(
        &b,
        "local",
        "m",
        Purpose::Draft,
        TicketLevel::Public,
        "attacker",
    )
    .unwrap()
    .encode(&TicketKey::from_bytes([0; 32]));
    let (status, _) = h.send(&b, Some(&forged)).await;
    assert_eq!(status, 401);
    assert!(h.upstream_calls().is_empty());
}
