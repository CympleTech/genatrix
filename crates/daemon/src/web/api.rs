//! The JSON the page reads, and the one thing it writes.
//!
//! The write is the user's judgement of an item's level (design 02: a user
//! judgement wins outright; design 06: raise by a click, lower by a click
//! on an option that says what lowering means). Nothing here proposes or
//! approves an action: the approval endpoint is the one place that has to
//! be built carefully rather than quickly (design 03), and it arrives with
//! the first pipeline that proposes something.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use genatrix_gate::gate::EgressRecord;
use genatrix_ledger::{EntryFilter, kind};
use genatrix_model::annotation::effective_level;
use genatrix_model::{Annotation, AnnotationKind, Item, ItemId, Level, Producer};
use genatrix_store::{ItemQuery, ItemVersion};
use serde::{Deserialize, Serialize};

use genatrix_agent::run::RunContext;

use crate::system::System;

type Shared = State<Arc<System>>;

/// Every route the page uses.
pub fn routes() -> Router<Arc<System>> {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/timeline", get(timeline))
        .route("/api/item/{id}", get(item))
        .route("/api/item/{id}/level", post(set_level))
        .route("/api/review", get(review))
        .route("/api/today", get(today))
        .route("/api/people", get(people))
        .route("/api/person/{id}", get(person))
        .route("/api/person/{id}/relationship", post(set_relationship))
        .route("/api/commitment/{id}", post(set_commitment))
        .route("/api/ledger", get(ledger))
        .route("/api/accounts", get(accounts))
}

#[derive(Serialize)]
struct SourceRef {
    id: String,
    at: String,
    connector: String,
    who: String,
}

#[derive(Serialize)]
struct PointView {
    text: String,
    sources: Vec<SourceRef>,
}

#[derive(Serialize)]
struct GroupView {
    group: genatrix_model::DigestGroup,
    points: Vec<PointView>,
}

#[derive(Serialize)]
struct DigestView {
    day: String,
    generated_at: String,
    considered: u32,
    groups: Vec<GroupView>,
}

#[derive(Serialize)]
struct CommitmentView {
    id: String,
    what: String,
    due: Option<String>,
    from: String,
    to: Option<String>,
    mine: bool,
    status: genatrix_model::CommitmentStatus,
    standing: genatrix_model::Standing,
    evidence: Vec<SourceRef>,
}

#[derive(Serialize)]
struct Today {
    digest: Option<DigestView>,
    commitments: Vec<CommitmentView>,
}

fn source_ref(system: &System, id: ItemId) -> Option<SourceRef> {
    let item = system.store.get_item(id).ok().flatten()?;
    Some(SourceRef {
        id: item.id.to_string(),
        at: item.occurred_at.format("%Y-%m-%d %H:%M").to_string(),
        connector: item.source.connector.as_str().to_owned(),
        who: name_of(system, item.author),
    })
}

/// The first screen (design 06): the latest digest and the open
/// commitments, each with what it was drawn from.
async fn today(State(system): Shared) -> Result<Json<Today>, ApiError> {
    let digest = system.store.latest_digest()?.map(|d| DigestView {
        day: d.day.to_string(),
        generated_at: d.generated_at.format("%Y-%m-%d %H:%M").to_string(),
        considered: d.considered,
        groups: d
            .groups
            .iter()
            .map(|(group, points)| GroupView {
                group: *group,
                points: points
                    .iter()
                    .map(|p| PointView {
                        text: p.text.clone(),
                        sources: p
                            .sources
                            .iter()
                            .filter_map(|id| source_ref(&system, *id))
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    });
    let me = system.store.self_person()?.map(|p| p.id);
    let now = chrono::Utc::now();
    let commitments = system
        .store
        .pending_commitments()?
        .into_iter()
        .map(|c| CommitmentView {
            id: c.id.to_string(),
            mine: c.is_mine(me),
            from: name_of(&system, Some(c.from)),
            to: c.to.map(|p| name_of(&system, Some(p))),
            due: c.due.map(|d| d.format("%Y-%m-%d").to_string()),
            // Overdue is a fact about the clock, decided when read.
            status: if c.due.is_some_and(|d| d < now)
                && c.status == genatrix_model::CommitmentStatus::Open
            {
                genatrix_model::CommitmentStatus::Overdue
            } else {
                c.status
            },
            standing: c.standing,
            what: c.what,
            evidence: c
                .evidence
                .iter()
                .filter_map(|id| source_ref(&system, *id))
                .collect(),
        })
        .collect();
    Ok(Json(Today {
        digest,
        commitments,
    }))
}

#[derive(Serialize)]
struct PersonCard {
    id: String,
    name: String,
    handles: Vec<HandleView>,
    from_them: u64,
    to_them: u64,
    last_at: Option<String>,
    connectors: Vec<String>,
    roles: Vec<String>,
}

#[derive(Serialize)]
struct HandleView {
    kind: String,
    value: String,
    inferred: bool,
}

#[derive(Serialize)]
struct StatsView {
    from_them: u64,
    to_them: u64,
    first_at: Option<String>,
    last_at: Option<String>,
    months: Vec<u32>,
    reply_hours: Option<f64>,
    language: String,
    connectors: Vec<(String, u64)>,
}

#[derive(Serialize)]
struct PersonDetail {
    card: PersonCard,
    stats: StatsView,
    notes: String,
    recent: Vec<Row>,
    commitments: Vec<CommitmentView>,
}

#[derive(Deserialize)]
struct PeopleQuery {
    #[serde(default = "default_people_limit")]
    limit: u32,
}

const fn default_people_limit() -> u32 {
    60
}

fn handles_view(
    system: &System,
    id: genatrix_model::PersonId,
) -> Result<Vec<HandleView>, ApiError> {
    Ok(system
        .store
        .handles_of(id)?
        .into_iter()
        .map(|h| HandleView {
            kind: format!("{:?}", h.kind).to_lowercase(),
            value: h.value,
            inferred: h.confidence == genatrix_model::Confidence::Inferred,
        })
        .collect())
}

fn card(system: &System, o: &genatrix_store::PersonOverview) -> Result<PersonCard, ApiError> {
    Ok(PersonCard {
        id: o.person.id.to_string(),
        name: o.person.display_name.clone(),
        handles: handles_view(system, o.person.id)?,
        from_them: o.from_them,
        to_them: o.to_them,
        last_at: o.last_at.map(|t| t.format("%Y-%m-%d").to_string()),
        connectors: o.connectors.clone(),
        roles: system
            .store
            .get_relationship(o.person.id)?
            .map(|r| r.roles)
            .unwrap_or_default(),
    })
}

/// Everyone the user has exchanged messages with, most recent first
/// (design 07: the relationship's statistics are arithmetic, not opinion).
async fn people(
    State(system): Shared,
    Query(query): Query<PeopleQuery>,
) -> Result<Json<Vec<PersonCard>>, ApiError> {
    let mut out = Vec::new();
    for o in system.store.people_overview(query.limit.min(500))? {
        if o.from_them + o.to_them == 0 {
            continue;
        }
        out.push(card(&system, &o)?);
    }
    Ok(Json(out))
}

/// One person: who they are, what passed between you, what you said about
/// them, what was promised either way.
async fn person(State(system): Shared, Path(id): Path<String>) -> Result<Response, ApiError> {
    let Ok(id) = id.parse::<genatrix_model::PersonId>() else {
        return Ok(not_found("that is not a person identifier"));
    };
    let Some(p) = system.store.get_person(id)? else {
        return Ok(not_found("no such person"));
    };
    let stats = system.store.relationship_stats(id)?;
    let relationship = system.store.get_relationship(id)?.unwrap_or_default();
    let overview = genatrix_store::PersonOverview {
        person: p,
        from_them: stats.from_them,
        to_them: stats.to_them,
        last_at: stats.last_at,
        connectors: stats.connectors.keys().cloned().collect(),
    };
    let mut recent = Vec::new();
    for item in system.store.items_with_person(id, 30)? {
        recent.push(row(&system, &item)?);
    }
    let me = system.store.self_person()?.map(|p| p.id);
    let commitments = system
        .store
        .commitments_with(id)?
        .into_iter()
        .map(|c| CommitmentView {
            id: c.id.to_string(),
            mine: c.is_mine(me),
            from: name_of(&system, Some(c.from)),
            to: c.to.map(|p| name_of(&system, Some(p))),
            due: c.due.map(|d| d.format("%Y-%m-%d").to_string()),
            status: c.status,
            standing: c.standing,
            what: c.what,
            evidence: c
                .evidence
                .iter()
                .filter_map(|id| source_ref(&system, *id))
                .collect(),
        })
        .collect();
    Ok(Json(PersonDetail {
        card: card(&system, &overview)?,
        stats: StatsView {
            from_them: stats.from_them,
            to_them: stats.to_them,
            first_at: stats.first_at.map(|t| t.format("%Y-%m-%d").to_string()),
            last_at: stats.last_at.map(|t| t.format("%Y-%m-%d").to_string()),
            months: stats.months,
            reply_hours: stats.reply_hours,
            language: stats.language,
            connectors: stats.connectors.into_iter().collect(),
        },
        notes: relationship.notes,
        recent,
        commitments,
    })
    .into_response())
}

#[derive(Deserialize)]
struct SetRelationship {
    roles: Vec<String>,
    notes: String,
}

/// Roles and notes are the user's alone (design 07).
async fn set_relationship(
    State(system): Shared,
    Path(id): Path<String>,
    Json(body): Json<SetRelationship>,
) -> Result<Response, ApiError> {
    let Ok(id) = id.parse::<genatrix_model::PersonId>() else {
        return Ok(not_found("that is not a person identifier"));
    };
    if system.store.get_person(id)?.is_none() {
        return Ok(not_found("no such person"));
    }
    let roles: Vec<String> = body
        .roles
        .into_iter()
        .map(|r| r.trim().chars().take(40).collect::<String>())
        .filter(|r| !r.is_empty())
        .collect();
    system.store.set_relationship(
        id,
        &genatrix_store::Relationship {
            roles,
            notes: body.notes.chars().take(4000).collect(),
        },
    )?;
    Ok(Json(serde_json::json!({ "ok": true })).into_response())
}

#[derive(Deserialize)]
struct SetCommitment {
    standing: genatrix_model::Standing,
    #[serde(default)]
    status: Option<genatrix_model::CommitmentStatus>,
}

/// The user's word on a commitment: confirmed, rejected, done (design 07).
async fn set_commitment(
    State(system): Shared,
    Path(id): Path<String>,
    Json(body): Json<SetCommitment>,
) -> Result<Response, ApiError> {
    let Ok(id) = id.parse::<genatrix_model::CommitmentId>() else {
        return Ok(not_found("that is not a commitment identifier"));
    };
    let Some(current) = system.store.get_commitment(id)? else {
        return Ok(not_found("no such commitment"));
    };
    let status = body.status.unwrap_or(match body.standing {
        genatrix_model::Standing::Rejected => genatrix_model::CommitmentStatus::Cancelled,
        _ => current.status,
    });
    system.store.set_commitment(id, body.standing, status)?;
    Ok(Json(serde_json::json!({ "ok": true })).into_response())
}

#[derive(Deserialize)]
struct SetLevel {
    level: Level,
}

/// The user's judgement of one item. Recorded as an annotation of theirs,
/// which the effective level then follows (design 02). Nothing the machine
/// said is deleted: the record shows the disagreement.
async fn set_level(
    State(system): Shared,
    Path(id): Path<String>,
    Json(body): Json<SetLevel>,
) -> Result<Response, ApiError> {
    let Ok(id) = id.parse::<ItemId>() else {
        return Ok(not_found("that is not an item identifier"));
    };
    if system.store.get_item(id)?.is_none() {
        return Ok(not_found("no such item"));
    }
    system.store.insert_annotation(&Annotation::new(
        id,
        Producer::User,
        AnnotationKind::Sensitivity {
            level: body.level,
            reason: "set by you".into(),
        },
    ))?;
    let annotations = system.store.annotations_of(id)?;
    system
        .store
        .set_item_sensitivity(id, effective_level(&annotations))?;
    Ok(Json(detail(&system, id)?).into_response())
}

#[derive(Serialize)]
struct ReviewRow {
    row: Row,
    subject: Option<String>,
    text: String,
    /// What the model said, for the "agree" button to agree with.
    model_level: String,
}

#[derive(Serialize)]
struct Tally {
    /// Items the model has judged.
    judged: u64,
    /// Of those, items the user has confirmed or corrected.
    reviewed: u64,
    /// Of those, items where the user's level equals the model's.
    agreed: u64,
}

#[derive(Serialize)]
struct Review {
    items: Vec<ReviewRow>,
    tally: Tally,
}

#[derive(Deserialize)]
struct ReviewQuery {
    #[serde(default = "default_review_count")]
    count: u32,
}

const fn default_review_count() -> u32 {
    20
}

/// A sample to confirm or correct, and how the review stands. Design 04
/// builds the evaluation set from the user's judgements; M2 is done when a
/// hundred of them agree with the model nine times in ten.
async fn review(
    State(system): Shared,
    Query(query): Query<ReviewQuery>,
) -> Result<Json<Review>, ApiError> {
    let mut items = Vec::new();
    for item in system.store.items_for_review(query.count.min(200))? {
        let model_level = system
            .store
            .annotations_of(item.id)?
            .iter()
            .rev()
            .find_map(|a| match (&a.producer, &a.kind) {
                (Producer::Model { .. }, AnnotationKind::Sensitivity { level, .. }) => {
                    Some(level.as_str().to_owned())
                }
                _ => None,
            })
            .unwrap_or_default();
        let subject = match &item.payload {
            genatrix_model::Payload::Mail { subject, .. } => Some(subject.clone()),
            _ => None,
        };
        items.push(ReviewRow {
            row: row(&system, &item)?,
            subject,
            text: item.text.chars().take(600).collect(),
            model_level,
        });
    }
    Ok(Json(Review {
        items,
        tally: tally(&system)?,
    }))
}

/// How many items the model judged, how many the user has looked at, and how
/// often they agreed. One pass over the annotations.
fn tally(system: &System) -> Result<Tally, ApiError> {
    use std::collections::BTreeMap;

    /// What the model and the user last said about one item.
    #[derive(Default)]
    struct Said {
        model: Option<Level>,
        user: Option<(chrono::DateTime<chrono::Utc>, Level)>,
    }

    let mut per_item: BTreeMap<ItemId, Said> = BTreeMap::new();
    for a in system.store.all_annotations()? {
        let AnnotationKind::Sensitivity { level, .. } = &a.kind else {
            continue;
        };
        let said = per_item.entry(a.item_id).or_default();
        match &a.producer {
            Producer::Model { .. } => said.model = Some(*level),
            Producer::User => {
                if said.user.is_none_or(|(t, _)| a.created_at >= t) {
                    said.user = Some((a.created_at, *level));
                }
            }
            Producer::Rule { .. } => {}
        }
    }
    let mut tally = Tally {
        judged: 0,
        reviewed: 0,
        agreed: 0,
    };
    for said in per_item.values() {
        let Some(model) = said.model else { continue };
        tally.judged += 1;
        if let Some((_, user)) = said.user {
            tally.reviewed += 1;
            if user == model {
                tally.agreed += 1;
            }
        }
    }
    Ok(tally)
}

#[derive(Serialize)]
struct AccountsView {
    accounts: Vec<crate::syncing::AccountState>,
}

/// How each account's sync is doing. Design 05: one state per account,
/// readable at a glance, with the backfill progress alongside.
async fn accounts(State(system): Shared) -> Json<AccountsView> {
    Json(AccountsView {
        accounts: system.accounts.snapshot(),
    })
}

/// Anything that goes wrong reading, rendered as JSON so the page can say so.
struct ApiError(anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::warn!(error = %self.0, "request failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": self.0.to_string() })),
        )
            .into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

#[derive(Serialize)]
struct Status {
    items: u64,
    ledger_entries: i64,
    cloud_enabled: bool,
    rules_version: String,
    bytes_left_device: usize,
    bytes_on_disk: u64,
    data_dir: String,
    model: crate::models::ModelState,
    model_text: String,
    /// Items with a vector, with a summary, with a model judgement.
    embedded: u64,
    summarized: u64,
    judged: u64,
}

async fn status(State(system): Shared) -> Result<Json<Status>, ApiError> {
    let model = system.model_state.borrow().clone();
    let slow = slow_status(&system)?;
    Ok(Json(Status {
        model_text: model.describe(),
        model,
        embedded: system.store.count_embedded_items()?,
        summarized: system.store.count_annotated_items("summary")?,
        judged: system.store.count_annotated_items("sensitivity")?,
        items: system.store.count_items()?,
        ledger_entries: system.ledger.len()?,
        cloud_enabled: system.gate.cloud_enabled(),
        rules_version: system.gate.rules().version.clone(),
        bytes_left_device: slow.bytes_left_device,
        bytes_on_disk: slow.bytes_on_disk,
        data_dir: system.config.data_dir.display().to_string(),
    }))
}

/// Walking tens of thousands of files and decoding every egress record
/// takes seconds; the answers change slowly. Kept for half a minute.
fn slow_status(system: &System) -> Result<crate::system::SlowStatus, ApiError> {
    const KEEP: std::time::Duration = std::time::Duration::from_secs(30);
    if let Some((at, cached)) = *system
        .slow_status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        && at.elapsed() < KEEP
    {
        return Ok(cached);
    }
    let fresh = crate::system::SlowStatus {
        bytes_on_disk: system.raw_files.size_on_disk()? + system.blob_files.size_on_disk()?,
        bytes_left_device: bytes_out(system)?,
    };
    *system
        .slow_status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some((std::time::Instant::now(), fresh));
    Ok(fresh)
}

#[derive(Deserialize)]
struct TimelineQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    level: String,
    #[serde(default)]
    connector: String,
    #[serde(default = "default_limit")]
    limit: u32,
}

const fn default_limit() -> u32 {
    100
}

#[derive(Serialize)]
struct Row {
    id: String,
    at: String,
    connector: String,
    direction: String,
    author: String,
    thread: String,
    level: String,
    /// Why it sits at that level, in a few words. Shown on hover: design 06
    /// asks that a marker always be able to explain itself.
    level_reason: String,
    preview: String,
    has_more: bool,
}

async fn timeline(
    State(system): Shared,
    Query(query): Query<TimelineQuery>,
) -> Result<Json<Vec<Row>>, ApiError> {
    let items = if query.q.trim().is_empty() {
        system.store.query_items(&ItemQuery {
            limit: query.limit,
            version: ItemVersion::Current,
            ..Default::default()
        })?
    } else {
        let mut items = system.store.search_items(&query.q, query.limit)?;
        // By meaning as well as by words (design 06: full text and vectors,
        // one timeline). Only while the model side answers; a search never
        // waits for it.
        if system.embedder_configured() && system.model_state.borrow().is_ready() {
            for (item, _) in nearest(&system, &query.q, query.limit).await {
                if !items.iter().any(|i| i.id == item.id) {
                    items.push(item);
                }
            }
            items.sort_by_key(|i| std::cmp::Reverse(i.occurred_at.timestamp_millis()));
            items.truncate(query.limit as usize);
        }
        items
    };

    let mut rows = Vec::with_capacity(items.len());
    for item in items {
        if !query.level.is_empty() && item.sensitivity.as_str() != query.level {
            continue;
        }
        if !query.connector.is_empty() && item.source.connector.as_str() != query.connector {
            continue;
        }
        rows.push(row(&system, &item)?);
    }
    Ok(Json(rows))
}

/// Items near a typed query, by vector. The query is embedded as the
/// user's own text at the level the rules give it; it never leaves the
/// device either way, the embedder being local by design.
async fn nearest(system: &System, q: &str, limit: u32) -> Vec<(Item, f32)> {
    /// A unit-vector L2 distance this large means a cosine under about 0.8,
    /// which for e5 is "not about the same thing".
    const FAR: f32 = 0.63;
    let Ok(mut ctx) = RunContext::begin(&system.ledger, &system.caller, "search", 2) else {
        return Vec::new();
    };
    let level = system.gate.level_of_typed_text(q);
    let vector = match crate::pipeline::embed::embed_query(&mut ctx, q, level).await {
        Ok(v) => {
            let _ = ctx.done();
            v
        }
        Err(e) => {
            let _ = ctx.stopped(e.to_string());
            tracing::warn!(error = %e, "search embedding failed; full text only");
            return Vec::new();
        }
    };
    if vector.is_empty() {
        return Vec::new();
    }
    match system.store.similar_items(&vector, limit) {
        Ok(hits) => hits.into_iter().filter(|(_, d)| *d <= FAR).collect(),
        Err(e) => {
            tracing::warn!(error = %e, "vector search failed");
            Vec::new()
        }
    }
}

#[derive(Serialize)]
struct Detail {
    row: Row,
    text: String,
    subject: Option<String>,
    recipients: Vec<String>,
    judgements: Vec<Judgement>,
    /// The model's summary, when the message was long enough to get one.
    summary: Option<String>,
    tombstoned: bool,
}

#[derive(Serialize)]
struct Judgement {
    by: String,
    detail: String,
    level: String,
    at: String,
}

async fn item(State(system): Shared, Path(id): Path<String>) -> Result<Response, ApiError> {
    let Ok(id) = id.parse::<ItemId>() else {
        return Ok(not_found("that is not an item identifier"));
    };
    if system.store.get_item(id)?.is_none() {
        return Ok(not_found("no such item"));
    }
    Ok(Json(detail(&system, id)?).into_response())
}

/// One item in full, with every judgement made about it.
fn detail(system: &System, id: ItemId) -> Result<Detail, ApiError> {
    let item = system
        .store
        .get_item(id)?
        .ok_or_else(|| anyhow::anyhow!("item {id} vanished"))?;

    let mut judgements = Vec::new();
    let mut summary = None;
    for annotation in system.store.annotations_of(id)? {
        if annotation.superseded_by.is_none()
            && let AnnotationKind::Summary { text, .. } = &annotation.kind
        {
            summary = Some(text.clone());
            continue;
        }
        let AnnotationKind::Sensitivity { level, .. } = &annotation.kind else {
            continue;
        };
        let (by, detail) = match &annotation.producer {
            Producer::Rule { rule, version } => ("rule", format!("{rule} (rules {version})")),
            Producer::Model {
                model,
                prompt_version,
            } => ("model", format!("{model} ({prompt_version})")),
            Producer::User => ("you", String::new()),
        };
        judgements.push(Judgement {
            by: by.to_owned(),
            detail,
            level: level.as_str().to_owned(),
            at: annotation.created_at.format("%Y-%m-%d %H:%M").to_string(),
        });
    }

    let recipients = item
        .recipients
        .iter()
        .map(|p| name_of(system, Some(*p)))
        .collect();
    let subject = match &item.payload {
        genatrix_model::Payload::Mail { subject, .. } => Some(subject.clone()),
        _ => None,
    };

    Ok(Detail {
        row: row(system, &item)?,
        text: item.text.clone(),
        subject,
        recipients,
        judgements,
        summary,
        tombstoned: item.tombstoned,
    })
}

#[derive(Serialize)]
struct LedgerView {
    headline: String,
    entries: i64,
    verified: bool,
    calls: Vec<Call>,
}

#[derive(Serialize)]
struct Call {
    at: String,
    purpose: String,
    level: String,
    target: String,
    location: String,
    decision: String,
    items: usize,
    /// Present only when the request left the device. This is the whole point
    /// of the page: what a provider saw, byte for byte.
    payload: Option<String>,
}

async fn ledger(State(system): Shared) -> Result<Json<LedgerView>, ApiError> {
    let verified = system.ledger.verify().is_ok();
    let entries = system.ledger.entries(&EntryFilter {
        kind: Some(kind::EGRESS.into()),
        limit: 500,
        ..Default::default()
    })?;

    let mut calls = Vec::with_capacity(entries.len());
    let mut bytes = 0usize;
    let mut left = 0usize;
    for entry in entries.iter().rev() {
        let Ok(record) = entry.decode::<EgressRecord>() else {
            continue;
        };
        if let Some(payload) = &record.payload {
            bytes += payload.len();
            left += 1;
        }
        calls.push(Call {
            at: entry.at.format("%Y-%m-%d %H:%M").to_string(),
            purpose: record.purpose,
            level: record.level,
            target: record.target,
            location: record.location,
            decision: format!("{:?}", record.decision),
            items: record.items.len(),
            payload: record.payload,
        });
    }

    let headline = if bytes == 0 {
        "0 bytes have left this device.".to_owned()
    } else {
        format!("{bytes} bytes left this device in {left} request(s).")
    };

    Ok(Json(LedgerView {
        headline,
        entries: system.ledger.len()?,
        verified,
        calls,
    }))
}

fn row(system: &System, item: &Item) -> Result<Row, ApiError> {
    let thread = system
        .store
        .get_thread(item.thread_id)?
        .and_then(|t| t.title)
        .unwrap_or_default();
    let preview: String = item.text.replace('\n', " ").chars().take(160).collect();
    Ok(Row {
        id: item.id.to_string(),
        at: item.occurred_at.format("%Y-%m-%d %H:%M").to_string(),
        connector: item.source.connector.as_str().to_owned(),
        direction: format!("{:?}", item.direction).to_lowercase(),
        author: name_of(system, item.author),
        thread,
        level: item.sensitivity.as_str().to_owned(),
        level_reason: level_reason(system, item),
        has_more: item.text.chars().count() > 160,
        preview,
    })
}

/// The one-line explanation behind a level marker.
fn level_reason(system: &System, item: &Item) -> String {
    let Ok(annotations) = system.store.annotations_of(item.id) else {
        return String::new();
    };
    // The judgement that decided it: the user's if there is one, otherwise
    // whichever machine judgement matches the level in force.
    let mut best = String::new();
    for annotation in annotations {
        let AnnotationKind::Sensitivity { level, .. } = &annotation.kind else {
            continue;
        };
        match &annotation.producer {
            Producer::User => return "you set this".to_owned(),
            Producer::Rule { rule, .. } if *level == item.sensitivity => {
                best = format!("rule: {rule}");
            }
            Producer::Model { .. } if *level == item.sensitivity && best.is_empty() => {
                "the local model judged this".clone_into(&mut best);
            }
            _ => {}
        }
    }
    if best.is_empty() && item.sensitivity == Level::default() {
        "no rule matched, so it is private".clone_into(&mut best);
    }
    best
}

fn name_of(system: &System, person: Option<genatrix_model::PersonId>) -> String {
    person
        .and_then(|p| system.store.get_person(p).ok().flatten())
        .map_or_else(|| "unknown".to_owned(), |p| p.display_name)
}

fn bytes_out(system: &System) -> Result<usize, ApiError> {
    let entries = system.ledger.entries(&EntryFilter {
        kind: Some(kind::EGRESS.into()),
        limit: 10_000,
        ..Default::default()
    })?;
    Ok(entries
        .iter()
        .filter_map(|e| e.decode::<EgressRecord>().ok())
        .filter_map(|r| r.payload)
        .map(|p| p.len())
        .sum())
}

fn not_found(message: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": message })),
    )
        .into_response()
}
