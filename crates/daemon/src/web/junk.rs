//! Marking a party as junk from the page (design 06, "标为垃圾"). A paired
//! phone may do it too: it removes what the user does not want and opens
//! nothing to anyone.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::system::System;

fn refuse(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

fn answer(r: anyhow::Result<serde_json::Value>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => refuse(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn person_count(State(system): State<Arc<System>>, Path(id): Path<String>) -> Response {
    answer((|| {
        let person = id.parse()?;
        Ok(serde_json::json!({ "items": crate::junk::count_person(&system, person)? }))
    })())
}

async fn person_mark(State(system): State<Arc<System>>, Path(id): Path<String>) -> Response {
    answer((|| {
        let done = crate::junk::mark_person(&system, id.parse()?)?;
        Ok(serde_json::json!({ "removed": done.items }))
    })())
}

async fn group_count(State(system): State<Arc<System>>, Path(id): Path<String>) -> Response {
    answer((|| {
        let thread = id.parse()?;
        Ok(serde_json::json!({ "items": crate::junk::count_thread(&system, thread)? }))
    })())
}

async fn group_mark(State(system): State<Arc<System>>, Path(id): Path<String>) -> Response {
    answer((|| {
        let done = crate::junk::mark_thread(&system, id.parse()?)?;
        Ok(serde_json::json!({ "removed": done.items }))
    })())
}

async fn list(State(system): State<Arc<System>>) -> Response {
    answer((|| {
        let rows: Vec<serde_json::Value> = system
            .store
            .junk()?
            .into_iter()
            .map(|j| serde_json::json!({ "kind": j.kind, "id": j.id, "name": j.name, "at": j.at }))
            .collect();
        Ok(serde_json::json!(rows))
    })())
}

async fn restore(
    State(system): State<Arc<System>>,
    Path((kind, id)): Path<(String, String)>,
) -> Response {
    answer(crate::junk::restore(&system, &kind, &id).map(|()| serde_json::json!({ "ok": true })))
}

/// The routes.
pub fn routes() -> axum::Router<Arc<System>> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/person/{id}/junk", get(person_count).post(person_mark))
        .route("/api/group/{id}/junk", get(group_count).post(group_mark))
        .route("/api/junk", get(list))
        .route("/api/junk/{kind}/{id}/restore", post(restore))
}
