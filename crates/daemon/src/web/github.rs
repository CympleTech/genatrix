//! GitHub sign-ins on the page (design 06 v0.21; design 14, invariant 32):
//! a default token, and a token of its own for any space that wants one.
//!
//! Giving a token decides which GitHub account a space's work pushes as,
//! so it is a high-privilege request (design 02, invariant 12): this
//! machine, or an admin device with the passphrase. The token is checked
//! with GitHub through the sandboxed connector, then kept in the keychain;
//! it is never sent back to the page.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;

use super::access::Caller;
use crate::executors::github::{self, account_for};
use crate::system::System;

type Shared = State<Arc<System>>;

/// The routes.
pub fn routes() -> axum::Router<Arc<System>> {
    axum::Router::new()
        .route("/api/github", get(overview))
        .route("/api/github/token", post(give))
        .route("/api/github/forget", post(take))
}

fn refuse(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

fn account_json(found: Option<Result<String, String>>) -> serde_json::Value {
    match found {
        None => serde_json::Value::Null,
        Some(Ok(login)) => serde_json::json!({ "login": login }),
        Some(Err(why)) => serde_json::json!({ "login": null, "error": why }),
    }
}

/// The default sign-in and each space's own, by login.
async fn overview(State(system): Shared, Extension(caller): Extension<Caller>) -> Response {
    let spaces = match crate::executors::definitions(&system.store) {
        Ok(d) => d
            .workspaces
            .into_iter()
            .map(|(w, _)| w.name)
            .collect::<Vec<_>>(),
        Err(e) => return refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let default = github::login_of(&system, &account_for(None)).await;
    let mut own = serde_json::Map::new();
    for name in spaces {
        let found = github::login_of(&system, &account_for(Some(&name))).await;
        own.insert(name, account_json(found));
    }
    Json(serde_json::json!({
        "default": account_json(default),
        "spaces": own,
        "manage": super::admin::can_manage(&system, &caller),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct Give {
    /// The space it is for; none for the default.
    #[serde(default)]
    workspace: Option<String>,
    token: String,
}

// A handler's refusal is the response itself, as in `admin::high`.
#[allow(clippy::result_large_err)]
fn known(system: &System, workspace: Option<&str>) -> Result<(), Response> {
    let Some(name) = workspace else {
        return Ok(());
    };
    match crate::executors::definitions(&system.store) {
        Ok(d) if d.workspaces.iter().any(|(w, _)| w.name == name) => Ok(()),
        Ok(_) => Err(refuse(
            StatusCode::NOT_FOUND,
            format!("there is no space `{name}`"),
        )),
        Err(e) => Err(refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

async fn give(
    State(system): Shared,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    Json(body): Json<Give>,
) -> Response {
    let subject = body.workspace.clone().unwrap_or_default();
    if let Err(no) = super::admin::high(
        &system,
        &caller,
        &headers,
        "github_sign_in",
        &subject,
        "a GitHub token is given on the computer Genatrix runs on, or from an admin device",
    )
    .await
    {
        return no;
    }
    if let Err(no) = known(&system, body.workspace.as_deref()) {
        return no;
    }
    match github::sign_in(&system, body.workspace.as_deref(), &body.token).await {
        Ok(login) => Json(serde_json::json!({ "login": login })).into_response(),
        Err(e) => refuse(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Deserialize)]
struct Take {
    #[serde(default)]
    workspace: Option<String>,
}

async fn take(
    State(system): Shared,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    Json(body): Json<Take>,
) -> Response {
    let subject = body.workspace.clone().unwrap_or_default();
    if let Err(no) = super::admin::high(
        &system,
        &caller,
        &headers,
        "github_sign_out",
        &subject,
        "a GitHub token is taken away on the computer Genatrix runs on, or from an admin device",
    )
    .await
    {
        return no;
    }
    match github::sign_out(&system, body.workspace.as_deref()) {
        Ok(had) => Json(serde_json::json!({ "removed": had })).into_response(),
        Err(e) => refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
