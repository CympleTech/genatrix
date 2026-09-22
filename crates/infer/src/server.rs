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
    /// The embedder, when this process was given one.
    embedder: Option<Arc<crate::embedder::Embedder>>,
    embedding_model_name: String,
}

/// An OpenAI-style embeddings request: one text or several.
#[derive(serde::Deserialize)]
struct EmbeddingsRequest {
    model: String,
    input: EmbeddingsInput,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum EmbeddingsInput {
    One(String),
    Many(Vec<String>),
}

/// Embed a batch of texts. Design 04: vectors, only here.
async fn embeddings(State(app): State<Arc<App>>, Json(req): Json<EmbeddingsRequest>) -> Response {
    let Some(embedder) = app.embedder.clone() else {
        return error(
            StatusCode::NOT_FOUND,
            "model_not_found",
            "this process was started without an embedding model",
        );
    };
    if req.model != app.embedding_model_name && req.model != "default" {
        return error(
            StatusCode::NOT_FOUND,
            "model_not_found",
            format!(
                "embeddings are served only as `{}`",
                app.embedding_model_name
            ),
        );
    }
    let texts = match req.input {
        EmbeddingsInput::One(t) => vec![t],
        EmbeddingsInput::Many(v) => v,
    };
    if texts.len() > 256 {
        return error(
            StatusCode::BAD_REQUEST,
            "too_many_inputs",
            "at most 256 texts per call",
        );
    }
    let started = std::time::Instant::now();
    let name = app.embedding_model_name.clone();
    let count = texts.len();
    let result = tokio::task::spawn_blocking(move || embedder.embed(&texts)).await;
    match result {
        Ok(Ok((vectors, tokens))) => {
            tracing::info!(
                texts = count,
                prompt_tokens = tokens,
                secs = started.elapsed().as_secs_f32(),
                "embeddings"
            );
            let data: Vec<serde_json::Value> = vectors
                .into_iter()
                .enumerate()
                .map(|(index, embedding)| {
                    serde_json::json!({ "object": "embedding", "index": index, "embedding": embedding })
                })
                .collect();
            Json(serde_json::json!({
                "object": "list",
                "data": data,
                "model": name,
                "usage": { "prompt_tokens": tokens, "total_tokens": tokens }
            }))
            .into_response()
        }
        Ok(Err(e)) => error(StatusCode::BAD_GATEWAY, "inference_error", e.to_string()),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "inference_error",
            e.to_string(),
        ),
    }
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
    embedding_model_dir: Option<&Path>,
    embedding_model_name: &str,
) -> anyhow::Result<()> {
    let embedder = match embedding_model_dir {
        Some(dir) => {
            let t = std::time::Instant::now();
            let embedder = crate::embedder::Embedder::load(dir)?;
            tracing::info!(
                model = embedding_model_name,
                dims = embedder.dims(),
                secs = t.elapsed().as_secs_f32(),
                "embedding model loaded"
            );
            Some(Arc::new(embedder))
        }
        None => None,
    };
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
        embedder,
        embedding_model_name: embedding_model_name.to_owned(),
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
        .route("/v1/embeddings", post(embeddings))
        .with_state(app);
    axum::serve(listener, router).await?;
    Ok(())
}
