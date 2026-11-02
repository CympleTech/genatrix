//! Work handed to executors, on the page (design 13, "界面").
//!
//! What any of the user's devices may do: see the projects and recipes,
//! draft a dispatch (which then waits for approval like any action), watch
//! one run, read its diff, stop it, and mark a pending task public. What
//! only this machine may do: file a project, recipe or role, because a
//! project decides which vendors see a repository (design 02, invariant 12).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

use super::access::Caller;
use super::api::action_view;
use crate::executors::draft::{self, DraftError};
use crate::system::System;

type Shared = State<Arc<System>>;

fn refuse(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

fn broke(e: &dyn std::fmt::Display) -> Response {
    refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// The routes.
pub fn routes() -> axum::Router<Arc<System>> {
    axum::Router::new()
        .route("/api/work", get(overview))
        .route("/api/work/draft", post(draft_work))
        .route("/api/work/define", post(define))
        .route("/api/work/suggest", post(suggest))
        .route("/api/work/{id}", get(one))
        .route("/api/work/{id}/diff", get(diff))
        .route("/api/work/{id}/stop", post(stop))
        .route("/api/work/{id}/push-again", post(push_again))
        .route("/api/action/{id}/level", post(level))
}

#[derive(Serialize)]
struct ProjectView {
    name: String,
    github: Option<String>,
    base: String,
    executors: Vec<String>,
    commands: Vec<String>,
    network: bool,
    delegate_personal: bool,
}

#[derive(Serialize)]
struct StepView {
    id: String,
    role: String,
    executor: String,
    produces: String,
}

#[derive(Serialize)]
struct RecipeView {
    name: String,
    title: String,
    steps: Vec<StepView>,
    rounds: Option<u32>,
}

#[derive(Serialize)]
struct RoleView {
    name: String,
    title: String,
    may: Vec<String>,
}

#[derive(Serialize)]
struct ExecutorView {
    name: String,
    vendor: String,
}

#[derive(Serialize)]
struct DispatchView {
    id: String,
    project: String,
    recipe: String,
    state: String,
    level: String,
    branch: String,
    base: Option<String>,
    tip: Option<String>,
    review_passed: Option<bool>,
    detail: Option<String>,
    started_ms: i64,
    ended_ms: Option<i64>,
}

impl From<genatrix_store::DispatchRow> for DispatchView {
    fn from(d: genatrix_store::DispatchRow) -> Self {
        Self {
            id: d.id,
            project: d.project,
            recipe: d.recipe,
            state: d.state,
            level: d.level,
            branch: d.branch,
            base: d.base,
            tip: d.tip,
            review_passed: d.review_passed,
            detail: d.detail,
            started_ms: d.started_ms,
            ended_ms: d.ended_ms,
        }
    }
}

#[derive(Serialize)]
struct Overview {
    projects: Vec<ProjectView>,
    recipes: Vec<RecipeView>,
    roles: Vec<RoleView>,
    executors: Vec<ExecutorView>,
    dispatches: Vec<DispatchView>,
    github: bool,
    /// Whether this request is from this machine, where projects are filed.
    local: bool,
}

fn word<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

async fn overview(State(system): Shared, Extension(caller): Extension<Caller>) -> Response {
    let defs = match crate::executors::definitions(&system.store) {
        Ok(d) => d,
        Err(e) => return broke(&e),
    };
    let dispatches = match system.store.list_dispatches(None, 30) {
        Ok(d) => d,
        Err(e) => return broke(&e),
    };
    let config = system.exec.config();
    Json(Overview {
        projects: defs
            .projects
            .into_iter()
            .map(|(p, _)| ProjectView {
                name: p.name,
                github: p.github,
                base: p.base,
                executors: p.executors,
                commands: p.commands,
                network: p.network,
                delegate_personal: p.delegate_personal,
            })
            .collect(),
        recipes: defs
            .recipes
            .into_iter()
            .map(|(r, _)| RecipeView {
                rounds: r.repeat.as_ref().map(|l| l.max),
                steps: r
                    .steps
                    .iter()
                    .map(|s| StepView {
                        id: s.id.clone(),
                        role: s.role.clone(),
                        executor: s.executor.clone(),
                        produces: word(&s.produces),
                    })
                    .collect(),
                name: r.name,
                title: r.title,
            })
            .collect(),
        roles: defs
            .roles
            .into_iter()
            .map(|(r, _)| RoleView {
                may: r.may.iter().map(word).collect(),
                name: r.name,
                title: r.title,
            })
            .collect(),
        executors: config
            .executors
            .iter()
            .map(|e| ExecutorView {
                name: e.name.clone(),
                vendor: e.vendor.clone(),
            })
            .collect(),
        dispatches: dispatches.into_iter().map(Into::into).collect(),
        github: crate::executors::github::token().is_some(),
        local: caller == Caller::Local,
    })
    .into_response()
}

#[derive(Deserialize)]
struct DraftBody {
    project: String,
    recipe: String,
    task: String,
    #[serde(default)]
    public: bool,
}

async fn draft_work(State(system): Shared, Json(body): Json<DraftBody>) -> Response {
    let request = draft::Request {
        project: body.project,
        recipe: body.recipe,
        task: body.task,
        public: body.public,
    };
    match draft::draft(&system, &request) {
        Ok(a) => {
            let nonce = Some(system.actions.nonce_for(&a.id));
            Json(action_view(&system, &a, nonce)).into_response()
        }
        Err(DraftError::Refused(why)) => refuse(StatusCode::BAD_REQUEST, why),
        Err(DraftError::Other(e)) => broke(&e),
    }
}

#[derive(Deserialize)]
struct SuggestBody {
    text: String,
}

async fn suggest(State(system): Shared, Json(body): Json<SuggestBody>) -> Response {
    match crate::executors::suggest::suggest(&system, &body.text).await {
        Ok(s) => Json(s).into_response(),
        Err(why) => refuse(StatusCode::BAD_REQUEST, why),
    }
}

#[derive(Deserialize)]
struct DefineBody {
    kind: String,
    text: String,
}

async fn define(
    State(system): Shared,
    Extension(caller): Extension<Caller>,
    Json(body): Json<DefineBody>,
) -> Response {
    if caller != Caller::Local {
        return refuse(
            StatusCode::FORBIDDEN,
            "projects, recipes and roles are filed on the computer Genatrix runs on",
        );
    }
    match crate::executors::file_definition(&system.store, &body.kind, &body.text) {
        Ok((name, hash)) => Json(serde_json::json!({"name": name, "hash": hash})).into_response(),
        Err(e) => refuse(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Deserialize)]
struct After {
    #[serde(default)]
    seq: i64,
    #[serde(default)]
    n: i64,
}

#[derive(Serialize)]
struct EventView {
    seq: i64,
    n: i64,
    at_ms: i64,
    kind: String,
    text: String,
}

#[derive(Serialize)]
struct StepRecord {
    seq: i64,
    step: String,
    round: i64,
    role: String,
    executor: String,
    state: String,
    stop: Option<String>,
    answer: Option<String>,
    verdict: Option<serde_json::Value>,
    commit_id: Option<String>,
    error: Option<String>,
    started_ms: i64,
    ended_ms: Option<i64>,
}

async fn one(
    State(system): Shared,
    Path(id): Path<String>,
    Query(after): Query<After>,
) -> Response {
    let row = match system.store.get_dispatch(&id) {
        Ok(Some(r)) => r,
        Ok(None) => return refuse(StatusCode::NOT_FOUND, "no such work"),
        Err(e) => return broke(&e),
    };
    let steps = match system.store.dispatch_steps(&id) {
        Ok(s) => s,
        Err(e) => return broke(&e),
    };
    let events = match system
        .store
        .dispatch_events(&id, Some((after.seq, after.n)), 500)
    {
        Ok(e) => e,
        Err(e) => return broke(&e),
    };
    let push_again = crate::executors::github::can_push_again(&system, &id);
    Json(serde_json::json!({
        "push_again": push_again,
        "dispatch": DispatchView::from(row),
        "steps": steps.into_iter().map(|s| StepRecord {
            verdict: s.verdict.as_deref().and_then(|v| serde_json::from_str(v).ok()),
            seq: s.seq, step: s.step, round: s.round, role: s.role, executor: s.executor,
            state: s.state, stop: s.stop, answer: s.answer, commit_id: s.commit_id,
            error: s.error, started_ms: s.started_ms, ended_ms: s.ended_ms,
        }).collect::<Vec<_>>(),
        "events": events.into_iter().map(|e| EventView {
            seq: e.seq, n: e.n, at_ms: e.at_ms, kind: e.kind, text: e.text,
        }).collect::<Vec<_>>(),
    }))
    .into_response()
}

async fn diff(State(system): Shared, Path(id): Path<String>) -> Response {
    let row = match system.store.get_dispatch(&id) {
        Ok(Some(r)) => r,
        Ok(None) => return refuse(StatusCode::NOT_FOUND, "no such work"),
        Err(e) => return broke(&e),
    };
    let (Some(base), Some(tip)) = (row.base.clone(), row.tip.clone()) else {
        return Json(serde_json::json!({"diff": ""})).into_response();
    };
    let mirror = system.exec.config().mirror(&row.project);
    match tokio::task::spawn_blocking(move || genatrix_exec::workspace::diff(&mirror, &base, &tip))
        .await
    {
        Ok(Ok(d)) => Json(serde_json::json!({"diff": d})).into_response(),
        Ok(Err(e)) => refuse(StatusCode::CONFLICT, e.to_string()),
        Err(e) => broke(&e),
    }
}

async fn stop(State(system): Shared, Path(id): Path<String>) -> Response {
    if system.exec.stop(&id) {
        Json(serde_json::json!({"stopping": true})).into_response()
    } else {
        refuse(StatusCode::CONFLICT, "this work is not running")
    }
}

/// Offer a failed push again (design 13): a new action, approved like any.
async fn push_again(State(system): Shared, Path(id): Path<String>) -> Response {
    match crate::executors::github::push_again(&system, &id) {
        Ok(Ok(a)) => {
            let nonce = Some(system.actions.nonce_for(&a.id));
            Json(action_view(&system, &a, nonce)).into_response()
        }
        Ok(Err(why)) => refuse(StatusCode::CONFLICT, why),
        Err(e) => broke(&e),
    }
}

#[derive(Deserialize)]
struct LevelBody {
    public: bool,
}

async fn level(
    State(system): Shared,
    Path(id): Path<String>,
    Json(body): Json<LevelBody>,
) -> Response {
    match draft::set_level(&system, &id, body.public) {
        Ok(a) => {
            let nonce = Some(system.actions.nonce_for(&a.id));
            Json(action_view(&system, &a, nonce)).into_response()
        }
        Err(DraftError::Refused(why)) => refuse(StatusCode::BAD_REQUEST, why),
        Err(DraftError::Other(e)) => broke(&e),
    }
}
