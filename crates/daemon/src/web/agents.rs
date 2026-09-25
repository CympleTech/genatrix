//! Functional agents on the page (design 11): the list, what a package may
//! do before it is installed, its trial run, install, its conversation and
//! its record, pause, export, uninstall.
//!
//! Installing, like adding an account, is accepted from this machine only
//! until management devices exist (design 02, invariants 12 and 18; design
//! 06 v0.7 allows a management device later, and this is stricter).

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use genatrix_host::{Invocation, Package, RunError};
use genatrix_store::{AgentState, StoredAgent};
use serde::{Deserialize, Serialize};

use super::access::Caller;
use super::api::{ActionView, action_view};
use crate::agents::life::{self, Risk, Trial};
use crate::agents::words::{Lang, describe};
use crate::system::System;

/// How long an uploaded package waits for its install.
const STAGED_FOR: Duration = Duration::from_secs(15 * 60);
/// The largest package accepted.
const MAX_PACKAGE: usize = 32 << 20;

fn refuse(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

fn local_only(caller: &Caller) -> Option<Response> {
    (*caller != Caller::Local).then(|| {
        refuse(
            StatusCode::FORBIDDEN,
            "agents are installed and removed on the computer Genatrix runs on",
        )
    })
}

fn failed(e: &anyhow::Error) -> Response {
    refuse(StatusCode::BAD_REQUEST, e.to_string())
}

/// One line of what a manifest allows.
#[derive(Serialize)]
struct Line {
    label: &'static str,
    text: String,
}

fn lines(m: &genatrix_host::Manifest, lang: Lang) -> Vec<Line> {
    describe(m, lang)
        .into_iter()
        .map(|(label, text)| Line { label, text })
        .collect()
}

/// An agent as the list and the conversation header show it.
#[derive(Serialize)]
pub(super) struct AgentCard {
    pub id: String,
    pub name: String,
    purpose: String,
    author: String,
    state: &'static str,
    level: &'static str,
    version: String,
    installed_ms: i64,
    pub last_ms: i64,
    pub last_text: Option<String>,
    pub runs: u64,
    pending: u64,
    approved: u64,
    declined: u64,
    space_bytes: u64,
    risk: Risk,
}

pub(super) fn card(system: &System, agent: &StoredAgent) -> anyhow::Result<AgentCard> {
    let manifest = crate::agents::load(system, agent).map(|p| p.manifest).ok();
    let runs = system.store.agent_runs(&agent.id, 1000)?;
    let (mut pending, mut approved, mut declined) = (0, 0, 0);
    for id in runs.iter().flat_map(|r| &r.proposals) {
        match system
            .actions
            .get(&system.store, id)?
            .map(|a| crate::actions::status_of(&a))
        {
            Some("pending") => pending += 1,
            Some("approved" | "executed") => approved += 1,
            Some("declined") => declined += 1,
            _ => {}
        }
    }
    let last = runs.first();
    Ok(AgentCard {
        id: agent.id.clone(),
        name: agent.name.clone(),
        purpose: manifest
            .as_ref()
            .map(|m| m.purpose.clone())
            .unwrap_or_default(),
        author: manifest
            .as_ref()
            .map(|m| m.author.clone())
            .unwrap_or_default(),
        state: match agent.state {
            AgentState::Active => "active",
            AgentState::Paused => "paused",
        },
        level: agent.space_level.as_str(),
        version: agent.version.chars().take(12).collect(),
        installed_ms: agent.installed_ms,
        last_ms: last.map_or(agent.installed_ms, |r| r.started_ms),
        last_text: last.and_then(|r| r.answer.clone().or_else(|| r.detail.clone())),
        runs: runs.len() as u64,
        pending,
        approved,
        declined,
        space_bytes: life::space_bytes(system, &agent.id),
        risk: manifest.as_ref().map_or(Risk::Reads, life::risk),
    })
}

async fn list(State(system): State<Arc<System>>) -> Response {
    let agents = match system.store.all_agents() {
        Ok(a) => a,
        Err(e) => return refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let cards: Result<Vec<_>, _> = agents.iter().map(|a| card(&system, a)).collect();
    match cards {
        Ok(c) => Json(c).into_response(),
        Err(e) => refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Serialize)]
struct Preview {
    hash: String,
    name: String,
    author: String,
    purpose: String,
    lines: Vec<Line>,
    risk: Risk,
    trial: Trial,
}

/// A package uploaded as it is: read, described, tried, and held for the
/// install that may follow.
async fn preview(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    let lang = language(&headers);
    if let Some(no) = local_only(&caller) {
        return no;
    }
    let package = match Package::read(body.to_vec()) {
        Ok(p) => p,
        Err(e) => return refuse(StatusCode::BAD_REQUEST, e.to_string()),
    };
    system
        .agents
        .stage(package.hash.clone(), package.bytes.clone(), STAGED_FOR);
    let m = package.manifest.clone();
    let hash = package.hash.clone();
    let trial = match life::trial(Arc::clone(&system), package).await {
        Ok(t) => t,
        Err(e) => return failed(&e),
    };
    Json(Preview {
        hash,
        name: m.name.clone(),
        author: m.author.clone(),
        purpose: m.purpose.clone(),
        lines: lines(&m, lang),
        risk: life::risk(&m),
        trial,
    })
    .into_response()
}

#[derive(Deserialize)]
struct InstallBody {
    hash: String,
}

/// Install the package the user just read, by its hash.
async fn install(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<InstallBody>,
) -> Response {
    if let Some(no) = local_only(&caller) {
        return no;
    }
    let Some(bytes) = system.agents.take_staged(&body.hash) else {
        return refuse(
            StatusCode::CONFLICT,
            "that package is no longer waiting; choose the file again",
        );
    };
    let now = chrono::Utc::now().timestamp_millis();
    match crate::agents::install(&system, bytes, now).and_then(|a| card(&system, &a)) {
        Ok(c) => Json(c).into_response(),
        Err(e) => failed(&e),
    }
}

/// One exchange in an agent's conversation.
#[derive(Serialize)]
struct Turn {
    at: String,
    /// What the user said; absent for a run the agent's triggers started.
    you: Option<String>,
    /// What woke it, when it was not the user.
    woke_by: Option<String>,
    answer: Option<String>,
    outcome: String,
    detail: Option<String>,
    reads: usize,
    actions: Vec<ActionView>,
}

fn turn(system: &System, r: &genatrix_store::AgentRun) -> Turn {
    let actions = r
        .proposals
        .iter()
        .filter_map(|id| system.actions.get(&system.store, id).ok().flatten())
        .map(|a| {
            let nonce = matches!(a.status, genatrix_agent::action::Status::Pending)
                .then(|| system.actions.nonce_for(&a.id));
            action_view(system, &a, nonce)
        })
        .collect();
    let at = chrono::DateTime::from_timestamp_millis(r.started_ms)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default();
    let (you, woke_by) = match r.invocation.as_str() {
        "message" => (r.input.clone(), None),
        "items" => (
            None,
            Some(format!(
                "{} new item(s)",
                r.input
                    .as_deref()
                    .map_or(0, |i| i.split(',').filter(|s| !s.is_empty()).count())
            )),
        ),
        other => (
            None,
            Some(format!("{other} {}", r.input.clone().unwrap_or_default())),
        ),
    };
    Turn {
        at,
        you,
        woke_by,
        answer: r.answer.clone(),
        outcome: r.outcome.clone(),
        detail: r.detail.clone(),
        reads: r.reads.len(),
        actions,
    }
}

#[derive(Serialize)]
struct Detail {
    card: AgentCard,
    lines: Vec<Line>,
    /// Oldest first, as a conversation reads.
    turns: Vec<Turn>,
}

fn language(headers: &axum::http::HeaderMap) -> Lang {
    Lang::from_header(
        headers
            .get(header::ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok()),
    )
}

async fn detail(
    State(system): State<Arc<System>>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    let lang = language(&headers);
    let agent = match system.store.get_agent(&id) {
        Ok(Some(a)) => a,
        Ok(None) => return refuse(StatusCode::NOT_FOUND, "no such agent"),
        Err(e) => return refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let manifest = crate::agents::load(&system, &agent).map(|p| p.manifest);
    let runs = system.store.agent_runs(&id, 60).unwrap_or_default();
    let mut turns: Vec<Turn> = runs
        .iter()
        .filter(|r| r.invocation != "apply")
        .map(|r| turn(&system, r))
        .collect();
    turns.reverse();
    match card(&system, &agent) {
        Ok(card) => Json(Detail {
            card,
            lines: manifest.map(|m| lines(&m, lang)).unwrap_or_default(),
            turns,
        })
        .into_response(),
        Err(e) => failed(&e),
    }
}

#[derive(Deserialize)]
struct MessageBody {
    text: String,
}

/// The user writes to an agent; the answer and any proposals come back.
async fn message(
    State(system): State<Arc<System>>,
    Path(id): Path<String>,
    Json(body): Json<MessageBody>,
) -> Response {
    let text = body.text.trim().chars().take(4000).collect::<String>();
    if text.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "say something");
    }
    let outcome =
        match crate::agents::run(Arc::clone(&system), &id, Invocation::Message(text)).await {
            Ok(o) => o,
            Err(e) => return failed(&e),
        };
    if let Err(RunError::Refused(why)) = &outcome.result {
        return refuse(StatusCode::BAD_REQUEST, why.clone());
    }
    match system.store.agent_runs(&id, 1) {
        Ok(runs) => match runs.first() {
            Some(r) => Json(turn(&system, r)).into_response(),
            None => refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the run was not recorded",
            ),
        },
        Err(e) => refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn pause(State(system): State<Arc<System>>, Path(id): Path<String>) -> Response {
    match life::pause(&system, &id) {
        Ok(withdrawn) => {
            Json(serde_json::json!({ "ok": true, "withdrawn": withdrawn })).into_response()
        }
        Err(e) => failed(&e),
    }
}

async fn resume(State(system): State<Arc<System>>, Path(id): Path<String>) -> Response {
    match life::resume(&system, &id) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => failed(&e),
    }
}

/// The agent's space as a JSON file to keep.
async fn export(State(system): State<Arc<System>>, Path(id): Path<String>) -> Response {
    match life::export(&system, &id) {
        Ok(value) => {
            let name = format!("genatrix-agent-{id}.json");
            (
                [
                    (header::CONTENT_TYPE, "application/json".to_owned()),
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"{name}\""),
                    ),
                ],
                serde_json::to_string_pretty(&value).unwrap_or_default(),
            )
                .into_response()
        }
        Err(e) => failed(&e),
    }
}

async fn uninstall(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> Response {
    if let Some(no) = local_only(&caller) {
        return no;
    }
    match life::uninstall(&system, &id) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => failed(&e),
    }
}

/// The routes.
pub fn routes() -> axum::Router<Arc<System>> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/agents", get(list))
        .route(
            "/api/agents/preview",
            post(preview).layer(DefaultBodyLimit::max(MAX_PACKAGE)),
        )
        .route("/api/agents/install", post(install))
        .route("/api/agent/{id}", get(detail))
        .route("/api/agent/{id}/message", post(message))
        .route("/api/agent/{id}/pause", post(pause))
        .route("/api/agent/{id}/resume", post(resume))
        .route("/api/agent/{id}/export", get(export))
        .route("/api/agent/{id}/uninstall", post(uninstall))
}

impl crate::agents::Agents {
    /// Hold an uploaded package until it is installed or goes stale.
    fn stage(&self, hash: String, bytes: Vec<u8>, lifetime: Duration) {
        let mut staged = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        staged.retain(|_, (at, _)| at.elapsed() < lifetime);
        staged.insert(hash, (Instant::now(), bytes));
    }

    fn take_staged(&self, hash: &str) -> Option<Vec<u8>> {
        let mut staged = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        staged
            .remove(hash)
            .filter(|(at, _)| at.elapsed() < STAGED_FOR)
            .map(|(_, b)| b)
    }
}
