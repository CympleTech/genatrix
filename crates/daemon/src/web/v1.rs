//! The interface API, version 1 (design 15, "数据接口").
//!
//! The endpoints the built-in interface grew up with are cut to its pages.
//! These are cut to the data instead: items, threads, people, tasks,
//! commitments, work, actions, definitions, records and agents' views, each
//! with plain filters and a page at a time, so an interface you change can
//! put them together as it likes without a new core. A device reads here
//! exactly what it could read before; nothing writes.
//!
//! The contract is `v1.json`, served at `/api/v1/schema` and handed to an
//! executor that changes an interface. Fields and endpoints are added, never
//! changed or removed; a change that cannot be an addition is version 2,
//! beside this one. `/api/meta` says which versions this core speaks.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::{DateTime, Utc};
use genatrix_model::{Item, ItemId, Level};
use genatrix_store::{ItemQuery, ItemVersion};
use serde::{Deserialize, Serialize};

use super::api::{ApiError, action_view, task_parties};
use super::work::{DispatchView, ProjectView};
use crate::system::System;

type Shared = State<Arc<System>>;

/// The contract, as data.
pub const SCHEMA: &str = include_str!("v1.json");

/// Every path this version serves, as the schema names it.
#[cfg_attr(not(test), allow(dead_code))] // read by the contract's test
pub const PATHS: &[&str] = &[
    "/api/meta",
    "/api/v1/schema",
    "/api/v1/items",
    "/api/v1/items/{id}",
    "/api/v1/threads",
    "/api/v1/people",
    "/api/v1/tasks",
    "/api/v1/commitments",
    "/api/v1/work",
    "/api/v1/actions",
    "/api/v1/projects",
    "/api/v1/spaces",
    "/api/v1/records",
    "/api/v1/views",
    "/api/v1/views/{agent}/{name}",
];

/// The routes.
pub fn routes() -> axum::Router<Arc<System>> {
    axum::Router::new()
        .route("/api/meta", get(meta))
        .route("/api/v1/schema", get(schema))
        .route("/api/v1/items", get(items))
        .route("/api/v1/items/{id}", get(item))
        .route("/api/v1/threads", get(threads))
        .route("/api/v1/people", get(people))
        .route("/api/v1/tasks", get(tasks))
        .route("/api/v1/commitments", get(commitments))
        .route("/api/v1/work", get(work))
        .route("/api/v1/actions", get(actions))
        .route("/api/v1/projects", get(projects))
        .route("/api/v1/spaces", get(spaces))
        .route("/api/v1/records", get(records))
        .route("/api/v1/views", get(views))
        .route("/api/v1/views/{agent}/{name}", get(view))
}

/// A page of a list: the rows, and where the next page starts, if there is
/// one.
#[derive(Serialize)]
struct Page<T> {
    rows: Vec<T>,
    next: Option<String>,
}

fn limit(asked: Option<u32>) -> u32 {
    asked.unwrap_or(50).clamp(1, 500)
}

async fn meta(State(system): Shared) -> Json<serde_json::Value> {
    let agents_with_views = system.store.agent_view_agents().unwrap_or_default();
    Json(serde_json::json!({
        "core": env!("CARGO_PKG_VERSION"),
        "api": [1],
        "interface_api": crate::ui::API_VERSION,
        "schema": "/api/v1/schema",
        "capabilities": {
            "model": system.model_state.borrow().is_ready(),
            "search_by_meaning": system.embedder_configured(),
            "github": crate::executors::github::token().is_some(),
            "views": agents_with_views,
        },
    }))
}

async fn schema() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        SCHEMA,
    )
}

/// One item, as version 1 shows it.
#[derive(Serialize)]
struct ItemV1 {
    id: String,
    /// RFC 3339, in the sender's offset.
    at: String,
    connector: String,
    account: String,
    /// `inbound`, `outbound`, `internal` or `neutral`.
    direction: String,
    author: Option<Who>,
    recipients: Vec<Who>,
    thread: ThreadRef,
    subject: Option<String>,
    text: String,
    /// `public`, `personal` or `secret`.
    level: String,
    category: String,
    attachments: usize,
    tombstoned: bool,
}

#[derive(Serialize)]
struct Who {
    id: String,
    name: String,
    me: bool,
}

#[derive(Serialize)]
struct ThreadRef {
    id: String,
    title: String,
    kind: String,
}

fn who(system: &System, me: Option<genatrix_model::PersonId>, id: genatrix_model::PersonId) -> Who {
    Who {
        id: id.to_string(),
        name: system
            .store
            .get_person(id)
            .ok()
            .flatten()
            .map(|p| p.display_name)
            .unwrap_or_default(),
        me: me == Some(id),
    }
}

fn kind_word<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn item_v1(system: &System, item: &Item) -> Result<ItemV1, ApiError> {
    let me = system.store.self_person()?.map(|p| p.id);
    let thread = system.store.get_thread(item.thread_id)?;
    Ok(ItemV1 {
        id: item.id.to_string(),
        at: item.occurred_at.to_rfc3339(),
        connector: item.source.connector.as_str().to_owned(),
        account: item.source.account.clone(),
        direction: kind_word(&item.direction),
        author: item.author.map(|a| who(system, me, a)),
        recipients: item
            .recipients
            .iter()
            .map(|r| who(system, me, *r))
            .collect(),
        thread: ThreadRef {
            id: item.thread_id.to_string(),
            title: thread
                .as_ref()
                .and_then(|t| t.title.clone())
                .unwrap_or_default(),
            kind: thread
                .as_ref()
                .map(|t| kind_word(&t.kind))
                .unwrap_or_default(),
        },
        subject: match &item.payload {
            genatrix_model::Payload::Mail { subject, .. } => Some(subject.clone()),
            genatrix_model::Payload::Task { title, .. } => Some(title.clone()),
            _ => None,
        },
        text: item.text.clone(),
        level: item.sensitivity.as_str().to_owned(),
        category: system.store.item_category(item.id)?.as_str().to_owned(),
        attachments: item.blobs.len(),
        tombstoned: item.tombstoned,
    })
}

#[derive(Deserialize, Default)]
struct ItemsQuery {
    /// RFC 3339: at or after.
    since: Option<String>,
    /// RFC 3339: before. The `next` of a page is this.
    before: Option<String>,
    thread: Option<String>,
    person: Option<String>,
    connector: Option<String>,
    direction: Option<String>,
    /// At most this sensitive.
    level: Option<String>,
    /// Words in the text.
    q: Option<String>,
    /// Newsletters and promotions too.
    #[serde(default)]
    quiet: bool,
    limit: Option<u32>,
}

fn time(s: Option<&str>, what: &str) -> Result<Option<DateTime<Utc>>, String> {
    s.filter(|s| !s.is_empty())
        .map(|s| {
            DateTime::parse_from_rfc3339(s)
                .map(|t| t.with_timezone(&Utc))
                .map_err(|_| format!("`{what}` is not an RFC 3339 time"))
        })
        .transpose()
}

fn bad(why: impl Into<String>) -> Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": why.into() })),
    )
        .into_response()
}

async fn items(State(system): Shared, Query(q): Query<ItemsQuery>) -> Result<Response, ApiError> {
    let limit = limit(q.limit);
    let (since, until) = match (
        time(q.since.as_deref(), "since"),
        time(q.before.as_deref(), "before"),
    ) {
        (Ok(s), Ok(u)) => (s, u),
        (Err(e), _) | (_, Err(e)) => return Ok(bad(e)),
    };
    let mut query = ItemQuery {
        since,
        until,
        limit,
        version: ItemVersion::Current,
        hide_quiet: !q.quiet,
        ..Default::default()
    };
    if let Some(t) = q.thread.as_deref().filter(|s| !s.is_empty()) {
        let Ok(t) = t.parse() else {
            return Ok(bad("`thread` is not a thread id"));
        };
        query.thread = Some(t);
    }
    if let Some(p) = q.person.as_deref().filter(|s| !s.is_empty()) {
        let Ok(p) = p.parse() else {
            return Ok(bad("`person` is not a person id"));
        };
        query.person = Some(p);
    }
    if let Some(c) = q.connector.as_deref().filter(|s| !s.is_empty()) {
        let Ok(c) = serde_json::from_value(serde_json::Value::String(c.to_owned())) else {
            return Ok(bad("`connector` is not a connector"));
        };
        query.connectors = vec![c];
    }
    if let Some(d) = q.direction.as_deref().filter(|s| !s.is_empty()) {
        let Ok(d) = serde_json::from_value(serde_json::Value::String(d.to_owned())) else {
            return Ok(bad("`direction` is inbound, outbound, internal or neutral"));
        };
        query.direction = Some(d);
    }
    if let Some(l) = q.level.as_deref().filter(|s| !s.is_empty()) {
        let Ok(l) = serde_json::from_value::<Level>(serde_json::Value::String(l.to_owned())) else {
            return Ok(bad("`level` is public, personal or secret"));
        };
        query.max_level = Some(l);
    }
    if let Some(words) = q.q.as_deref().filter(|s| !s.trim().is_empty()) {
        query.text = Some(words.trim().to_owned());
        query.hide_quiet = false;
    }
    let found = system.store.query_items(&query)?;
    let next = (found.len() == limit as usize)
        .then(|| found.last().map(|i| i.occurred_at.to_rfc3339()))
        .flatten();
    let rows = found
        .iter()
        .map(|i| item_v1(&system, i))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(Page { rows, next }).into_response())
}

async fn item(State(system): Shared, Path(id): Path<String>) -> Result<Response, ApiError> {
    let Ok(id) = id.parse::<ItemId>() else {
        return Ok(bad("not an item id"));
    };
    match system.store.get_item(id)? {
        Some(i) => Ok(Json(item_v1(&system, &i)?).into_response()),
        None => Ok((
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no such item" })),
        )
            .into_response()),
    }
}

#[derive(Deserialize, Default)]
struct KindQuery {
    kind: Option<String>,
    limit: Option<u32>,
}

#[derive(Serialize)]
struct ThreadV1 {
    id: String,
    kind: String,
    title: String,
    connector: String,
    account: String,
    first_at: Option<String>,
    last_at: Option<String>,
    junk: bool,
}

async fn threads(
    State(system): Shared,
    Query(q): Query<KindQuery>,
) -> Result<Json<Page<ThreadV1>>, ApiError> {
    let mut all = system.store.all_threads()?;
    all.sort_by_key(|t| std::cmp::Reverse(t.last_at));
    let rows = all
        .into_iter()
        .filter(|t| {
            q.kind
                .as_deref()
                .is_none_or(|k| k.is_empty() || kind_word(&t.kind) == k)
        })
        .take(limit(q.limit) as usize)
        .map(|t| ThreadV1 {
            junk: system.store.is_junk_thread(&t.source).unwrap_or(false),
            id: t.id.to_string(),
            kind: kind_word(&t.kind),
            title: t.title.unwrap_or_default(),
            connector: t.source.connector.as_str().to_owned(),
            account: t.source.account,
            first_at: t.first_at.map(|a| a.to_rfc3339()),
            last_at: t.last_at.map(|a| a.to_rfc3339()),
        })
        .collect();
    Ok(Json(Page { rows, next: None }))
}

#[derive(Serialize)]
struct PersonV1 {
    id: String,
    name: String,
    me: bool,
    handles: Vec<HandleV1>,
}

#[derive(Serialize)]
struct HandleV1 {
    kind: String,
    value: String,
}

async fn people(
    State(system): Shared,
    Query(q): Query<KindQuery>,
) -> Result<Json<Page<PersonV1>>, ApiError> {
    let handles = system.store.all_handles()?;
    let rows = system
        .store
        .all_persons()?
        .into_iter()
        .take(limit(q.limit.or(Some(500))) as usize)
        .map(|p| PersonV1 {
            handles: handles
                .iter()
                .filter(|h| h.person_id == p.id)
                .map(|h| HandleV1 {
                    kind: kind_word(&h.kind),
                    value: h.value.clone(),
                })
                .collect(),
            id: p.id.to_string(),
            name: p.display_name,
            me: p.is_self,
        })
        .collect();
    Ok(Json(Page { rows, next: None }))
}

#[derive(Deserialize, Default)]
struct SpaceQuery {
    space: Option<String>,
}

async fn tasks(
    State(system): Shared,
    Query(q): Query<SpaceQuery>,
) -> Result<Json<Page<serde_json::Value>>, ApiError> {
    let rows = task_parties(&system, q.space.as_deref().filter(|s| !s.is_empty()))?
        .into_iter()
        .filter_map(|p| serde_json::to_value(p).ok())
        .collect();
    Ok(Json(Page { rows, next: None }))
}

#[derive(Serialize)]
struct CommitmentV1 {
    id: String,
    what: String,
    from: String,
    to: Option<String>,
    due: Option<String>,
    status: String,
    standing: String,
    evidence: Vec<String>,
    created_at: String,
}

async fn commitments(
    State(system): Shared,
    Query(q): Query<KindQuery>,
) -> Result<Json<Page<CommitmentV1>>, ApiError> {
    let rows = system
        .store
        .pending_commitments_upto(limit(q.limit))?
        .into_iter()
        .map(|c| CommitmentV1 {
            id: c.id.to_string(),
            what: c.what,
            from: c.from.to_string(),
            to: c.to.map(|t| t.to_string()),
            due: c.due.map(|d| d.to_rfc3339()),
            status: kind_word(&c.status),
            standing: kind_word(&c.standing),
            evidence: c.evidence.iter().map(ToString::to_string).collect(),
            created_at: c.created_at.to_rfc3339(),
        })
        .collect();
    Ok(Json(Page { rows, next: None }))
}

#[derive(Deserialize, Default)]
struct WorkQuery {
    project: Option<String>,
    state: Option<String>,
    limit: Option<u32>,
}

async fn work(
    State(system): Shared,
    Query(q): Query<WorkQuery>,
) -> Result<Json<Page<DispatchView>>, ApiError> {
    let rows = system
        .store
        .list_dispatches(
            q.project.as_deref().filter(|s| !s.is_empty()),
            limit(q.limit),
        )?
        .into_iter()
        .filter(|d| {
            q.state
                .as_deref()
                .is_none_or(|s| s.is_empty() || d.state == s)
        })
        .map(Into::into)
        .collect();
    Ok(Json(Page { rows, next: None }))
}

#[derive(Deserialize, Default)]
struct StatusQuery {
    status: Option<String>,
    limit: Option<u32>,
}

async fn actions(
    State(system): Shared,
    Query(q): Query<StatusQuery>,
) -> Result<Json<Page<serde_json::Value>>, ApiError> {
    let list = system.actions.list(
        &system.store,
        q.status.as_deref().filter(|s| !s.is_empty()),
        limit(q.limit),
    )?;
    let rows = list
        .iter()
        .filter_map(|a| serde_json::to_value(action_view(&system, a, None)).ok())
        .collect();
    Ok(Json(Page { rows, next: None }))
}

async fn projects(
    State(system): Shared,
    Query(q): Query<SpaceQuery>,
) -> Result<Json<Page<ProjectView>>, ApiError> {
    let defs = crate::executors::definitions(&system.store)?;
    let rows = defs
        .projects
        .into_iter()
        .filter(|(p, _)| {
            q.space
                .as_deref()
                .is_none_or(|s| s.is_empty() || p.workspace == s)
        })
        .map(|(p, _)| p.into())
        .collect();
    Ok(Json(Page { rows, next: None }))
}

async fn spaces(State(system): Shared) -> Result<Json<Page<serde_json::Value>>, ApiError> {
    let defs = crate::executors::definitions(&system.store)?;
    let rows = defs
        .workspaces
        .into_iter()
        .map(|(w, _)| serde_json::json!({ "name": w.name, "title": w.shown(), "accounts": w.accounts }))
        .collect();
    Ok(Json(Page { rows, next: None }))
}

#[derive(Deserialize, Default)]
struct RecordsQuery {
    kind: Option<String>,
    /// Entries after this sequence number. The `next` of a page is this.
    after: Option<i64>,
    limit: Option<u32>,
}

#[derive(Serialize)]
struct RecordV1 {
    seq: i64,
    at: String,
    kind: String,
    subject: String,
    body: serde_json::Value,
}

async fn records(
    State(system): Shared,
    Query(q): Query<RecordsQuery>,
) -> Result<Json<Page<RecordV1>>, ApiError> {
    let limit = limit(q.limit);
    let entries = system.ledger.entries(&genatrix_ledger::EntryFilter {
        kind: q.kind.filter(|k| !k.is_empty()),
        subject: None,
        after_seq: q.after.unwrap_or(0),
        limit,
    })?;
    let next = (entries.len() == limit as usize)
        .then(|| entries.last().map(|e| e.seq.to_string()))
        .flatten();
    let rows = entries
        .into_iter()
        .map(|e| RecordV1 {
            seq: e.seq,
            at: e.at.to_rfc3339(),
            kind: e.kind,
            subject: e.subject,
            body: e.body,
        })
        .collect();
    Ok(Json(Page { rows, next }))
}

async fn views(State(system): Shared) -> Result<Json<Page<serde_json::Value>>, ApiError> {
    let rows = system
        .store
        .agent_views()?
        .into_iter()
        .map(|v| serde_json::json!({ "agent": v.agent, "name": v.name, "title": v.title, "at": v.at }))
        .collect();
    Ok(Json(Page { rows, next: None }))
}

async fn view(
    State(system): Shared,
    Path((agent, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    match system.store.agent_view(&agent, &name)? {
        Some(v) => Ok(Json(serde_json::json!({
            "agent": v.agent, "name": v.name, "title": v.title, "at": v.at,
            "view": serde_json::from_str::<serde_json::Value>(&v.data).unwrap_or_default(),
        }))
        .into_response()),
        None => Ok((
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no such view" })),
        )
            .into_response()),
    }
}
