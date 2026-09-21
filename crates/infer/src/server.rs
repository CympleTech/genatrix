//! OpenAI-style chat completions over a Unix socket, backed by MLX.

use std::convert::Infallible;
use std::path::Path;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use crabllm_core::{ChatCompletionRequest, Provider};
use crabllm_mlx::{MlxPool, MlxProvider};
use futures::StreamExt;
use serde::Serialize;
use tokio::net::UnixListener;

struct App {
    provider: MlxProvider,
    model_dir: String,
    model_name: String,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: ErrorDetail<'a>,
}

#[derive(Serialize)]
struct ErrorDetail<'a> {
    message: String,
    r#type: &'a str,
}

fn error(status: StatusCode, kind: &'static str, message: impl Into<String>) -> Response {
    (
        status,
        Json(ErrorBody {
            error: ErrorDetail {
                message: message.into(),
                r#type: kind,
            },
        }),
    )
        .into_response()
}

async fn health(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "model": app.model_name }))
}

async fn models(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "object": "list",
        "data": [{ "id": app.model_name, "object": "model", "owned_by": "local" }]
    }))
}

async fn chat(State(app): State<Arc<App>>, Json(mut req): Json<ChatCompletionRequest>) -> Response {
    if req.model != app.model_name && req.model != "default" {
        return error(
            StatusCode::NOT_FOUND,
            "model_not_found",
            format!("this process serves only `{}`", app.model_name),
        );
    }
    // MLX resolves models by directory; the public name stays ours.
    req.model = app.model_dir.clone();
    let name = app.model_name.clone();

    if req.stream == Some(true) {
        match app.provider.chat_completion_stream(&req).await {
            Ok(stream) => {
                let events = stream
                    .map(move |chunk| {
                        let event = match chunk {
                            Ok(mut c) => {
                                c.model.clone_from(&name);
                                serde_json::to_string(&c).map_or_else(
                                    |e| {
                                        Event::default()
                                            .data(format!("{{\"error\":{{\"message\":\"{e}\"}}}}"))
                                    },
                                    |json| Event::default().data(json),
                                )
                            }
                            Err(e) => Event::default().data(format!(
                                "{{\"error\":{{\"message\":{}}}}}",
                                serde_json::to_string(&e.to_string()).unwrap_or_default()
                            )),
                        };
                        Ok::<_, Infallible>(event)
                    })
                    .chain(futures::stream::once(async {
                        Ok(Event::default().data("[DONE]"))
                    }));
                Sse::new(events)
                    .keep_alive(KeepAlive::default())
                    .into_response()
            }
            Err(e) => error(StatusCode::BAD_GATEWAY, "inference_error", e.to_string()),
        }
    } else {
        let started = std::time::Instant::now();
        match app.provider.chat_completion(&req).await {
            Ok(mut resp) => {
                resp.model = name;
                // Design 04 gives every role a latency budget; this is where
                // the truth about it is written down. Counts only, never
                // content.
                let (prompt, completion) = resp
                    .usage
                    .as_ref()
                    .map_or((0, 0), |u| (u.prompt_tokens, u.completion_tokens));
                tracing::info!(
                    prompt_tokens = prompt,
                    completion_tokens = completion,
                    max_tokens = req.max_tokens.unwrap_or(0),
                    secs = started.elapsed().as_secs_f32(),
                    "completion"
                );
                Json(resp).into_response()
            }
            Err(e) => error(StatusCode::BAD_GATEWAY, "inference_error", e.to_string()),
        }
    }
}

/// Serve until the process is killed.
pub async fn run(
    model_dir: &Path,
    model_name: &str,
    socket: &Path,
    idle_timeout: u64,
) -> anyhow::Result<()> {
    if !model_dir.join("config.json").exists() {
        anyhow::bail!(
            "{} does not look like a model directory",
            model_dir.display()
        );
    }
    let pool = Arc::new(MlxPool::new(if idle_timeout == 0 {
        u64::MAX / 4
    } else {
        idle_timeout
    })?);
    let app = Arc::new(App {
        provider: MlxProvider::new(pool),
        model_dir: model_dir.canonicalize()?.to_string_lossy().into_owned(),
        model_name: model_name.to_owned(),
    });

    // Warm up so the first real request does not pay the load.
    let warm = ChatCompletionRequest {
        model: app.model_dir.clone(),
        messages: vec![crabllm_core::Message::user("hi")],
        max_tokens: Some(1),
        ..Default::default()
    };
    let t = std::time::Instant::now();
    app.provider.chat_completion(&warm).await?;
    tracing::info!(
        model = model_name,
        secs = t.elapsed().as_secs_f32(),
        "model loaded"
    );

    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if socket.exists() {
        std::fs::remove_file(socket)?;
    }
    let listener = UnixListener::bind(socket)?;
    tracing::info!(socket = %socket.display(), "listening");

    let router = Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat))
        .with_state(app);
    axum::serve(listener, router).await?;
    Ok(())
}
