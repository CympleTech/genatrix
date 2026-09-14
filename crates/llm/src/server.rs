//! The gateway process: one socket, one route, one decision.

use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use genatrix_keys::TicketKey;
use http_body_util::BodyExt;
use serde::Serialize;
use tokio::net::UnixListener;

use crate::config::Config;
use crate::gateway::decide;
use crate::local::LocalClient;
use crate::registry::Endpoint;
use crate::ticket::TicketStore;

/// The header a caller puts its egress ticket in.
pub const TICKET_HEADER: &str = "x-genatrix-ticket";

/// Everything the request handler needs.
pub struct Gateway {
    config: Config,
    tickets: TicketStore,
    key: TicketKey,
}

impl Gateway {
    /// Build a gateway. The configuration has already been validated.
    #[must_use]
    pub fn new(config: Config, key: TicketKey) -> Self {
        Self {
            config,
            tickets: TicketStore::new(),
            key,
        }
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    message: String,
    r#type: String,
}

fn refuse(status: u16, kind: &str, message: impl Into<String>) -> Response {
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(ErrorBody {
            error: ErrorDetail {
                message: message.into(),
                r#type: kind.to_owned(),
            },
        }),
    )
        .into_response()
}

async fn health(State(gw): State<Arc<Gateway>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "models": gw.config.registry.models.iter().map(|m| &m.name).collect::<Vec<_>>(),
    }))
}

async fn models(State(gw): State<Arc<Gateway>>) -> Json<serde_json::Value> {
    let data: Vec<_> = gw
        .config
        .registry
        .models
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.name,
                "object": "model",
                "owned_by": match m.location() {
                    crate::registry::Location::Local => "local",
                    crate::registry::Location::Cloud => "cloud",
                },
                "context_length": m.context_length,
                "purposes": m.purposes.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            })
        })
        .collect();
    Json(serde_json::json!({ "object": "list", "data": data }))
}

async fn chat(State(gw): State<Arc<Gateway>>, headers: HeaderMap, body: Bytes) -> Response {
    let ticket_header = headers
        .get(TICKET_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let forward = match decide(
        &gw.config.registry,
        &gw.tickets,
        &gw.key,
        ticket_header.as_deref(),
        &body,
    ) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(refusal = %e, tag = e.tag(), "request refused");
            return refuse(e.status(), e.tag(), e.to_string());
        }
    };

    match &forward.entry.endpoint {
        Endpoint::LocalSocket { path } => {
            // Rewrite the registry name to the model identifier the local
            // process knows. The body is otherwise untouched.
            let upstream_body = match rewrite_model(&body, &forward.entry.model) {
                Ok(b) => b,
                Err(e) => return refuse(400, "bad_request", e),
            };
            match LocalClient::new(path)
                .post_json_streaming("/v1/chat/completions", upstream_body)
                .await
            {
                Ok((status, incoming)) => {
                    let status =
                        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                    (status, Body::new(incoming.map_err(axum::Error::new))).into_response()
                }
                Err(e) => {
                    tracing::error!(error = %e, "local inference unreachable");
                    refuse(503, "local_unavailable", e.to_string())
                }
            }
        }
        Endpoint::OpenAi { .. } | Endpoint::Anthropic { .. } => {
            // Deliberately not implemented yet. Design 10 keeps the cloud
            // switched off for the whole of phase one, and an untested
            // egress path is exactly the thing this gateway exists to
            // prevent. The policy above is real and tested; the transport
            // lands with the cloud switch in phase two.
            tracing::warn!(model = %forward.entry.name, "cloud transport not built yet");
            refuse(
                501,
                "cloud_not_enabled",
                "this build has no cloud transport; the request was allowed by policy but cannot be sent",
            )
        }
    }
}

/// Replace the `model` field with the identifier the upstream expects.
fn rewrite_model(body: &Bytes, upstream_model: &str) -> Result<Bytes, String> {
    let mut value: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let obj = value
        .as_object_mut()
        .ok_or_else(|| "request body must be a JSON object".to_string())?;
    obj.insert(
        "model".into(),
        serde_json::Value::String(upstream_model.to_owned()),
    );
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|e| e.to_string())
}

/// Build the router. Exposed so tests can drive it without a socket.
pub fn router(gateway: Arc<Gateway>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat))
        .with_state(gateway)
}

/// Listen on the configured Unix socket until killed.
pub async fn serve(gateway: Arc<Gateway>) -> anyhow::Result<()> {
    let socket: &Path = &gateway.config.socket;
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if socket.exists() {
        std::fs::remove_file(socket)?;
    }
    let listener = UnixListener::bind(socket)?;
    tracing::info!(
        socket = %socket.display(),
        models = gateway.config.registry.models.len(),
        "gateway listening"
    );
    axum::serve(listener, router(gateway)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewriting_the_model_leaves_everything_else_alone() {
        let body = Bytes::from(r#"{"model":"local","messages":[{"role":"user"}],"stream":true}"#);
        let out = rewrite_model(&body, "qwen3-8b-4bit").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["model"], "qwen3-8b-4bit");
        assert_eq!(v["stream"], true);
        assert_eq!(v["messages"][0]["role"], "user");
    }

    #[test]
    fn a_non_object_body_is_refused() {
        assert!(rewrite_model(&Bytes::from_static(b"[1,2]"), "m").is_err());
        assert!(rewrite_model(&Bytes::from_static(b"nope"), "m").is_err());
    }
}
