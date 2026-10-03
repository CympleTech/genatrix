//! Newsletters and promotions on the page (design 06, "订阅与广告"): the
//! senders gathered under one entry, and the user's word about a sender.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use genatrix_model::Category;
use serde::Deserialize;

use crate::system::System;

fn refuse(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

async fn senders(State(system): State<Arc<System>>) -> Response {
    match system.store.quiet_senders(500) {
        Ok((senders, total)) => Json(serde_json::json!({
            "total": total,
            "senders": senders.iter().map(|s| serde_json::json!({
                "id": s.person.to_string(),
                "name": s.name,
                "items": s.items,
                "last_at": chrono::DateTime::from_timestamp_millis(s.last_ms)
                    .map(|t| t.format("%Y-%m-%d").to_string()),
                "category": s.category.as_str(),
            })).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct Said {
    category: String,
}

/// "This is (not) an advertisement": every item from this sender, and what
/// they send later, takes the category.
async fn set_category(
    State(system): State<Arc<System>>,
    Path(id): Path<String>,
    Json(body): Json<Said>,
) -> Response {
    let Ok(person) = id.parse() else {
        return refuse(StatusCode::NOT_FOUND, "that is not a person");
    };
    let Some(category) = Category::parse(&body.category) else {
        return refuse(StatusCode::BAD_REQUEST, "no such category");
    };
    match crate::pipeline::categorize::set_sender(&system.store, person, category) {
        Ok(n) => {
            *system
                .people
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            Json(serde_json::json!({ "items": n })).into_response()
        }
        Err(e) => refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// The routes.
pub fn routes() -> axum::Router<Arc<System>> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/quiet", get(senders))
        .route("/api/person/{id}/category", post(set_category))
}
