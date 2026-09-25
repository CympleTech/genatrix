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
        .route("/api/item/{id}/draft", post(draft_reply))
        .route("/api/actions", get(actions))
        .route("/api/action/{id}", get(action))
        .route("/api/action/{id}/edit", post(edit_action))
        .route("/api/action/{id}/approve", post(approve_action))
        .route("/api/action/{id}/decline", post(decline_action))
        .route("/api/people", get(people))
        .route("/api/person/{id}", get(person))
        .route("/api/person/{id}/chat", get(chat))
        .route("/api/chats", get(chats))
        .route("/api/group/{id}", get(group))
        .route("/api/group/{id}/chat", get(group_chat))
        .route("/api/person/{id}/relationship", post(set_relationship))
        .route("/api/commitment/{id}", post(set_commitment))
        .route("/api/ledger", get(ledger))
        .route("/api/run/{id}", get(run_record))
        .route("/api/ask", post(ask))
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
    /// The most pressing open promises, not all of them: this is the screen
    /// for the day, and the rest are on each person's page.
    commitments: Vec<CommitmentView>,
    /// How many there are in all, so the page can say what it left out.
    commitments_total: u64,
    /// Up to five pending actions (design 06); the rest are on their tab.
    actions: Vec<ActionView>,
    pending_actions: u64,
}

/// How many open promises Today shows before it stops and says how many
/// more there are. Reading every one of them was the most expensive thing
/// the interface did, and it did it every five seconds.
const TODAY_COMMITMENTS: u32 = 25;

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
        .pending_commitments_upto(TODAY_COMMITMENTS)?
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
    let pending = system.actions.list(&system.store, Some("pending"), 5)?;
    let actions = pending
        .iter()
        .map(|a| action_view(&system, a, Some(system.actions.nonce_for(&a.id))))
        .collect();
    Ok(Json(Today {
        digest,
        commitments,
        commitments_total: system.store.count_pending_commitments()?,
        actions,
        pending_actions: system.store.count_actions("pending")?,
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
    /// The start of the newest thing they wrote: the line under the name in
    /// the list of conversations.
    last_text: Option<String>,
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
struct ChatQuery {
    /// Milliseconds: only messages older than this. Absent for the newest.
    #[serde(default)]
    before: Option<i64>,
    /// Milliseconds: only messages newer than this, which is how an open
    /// conversation picks up what arrived since it was drawn.
    #[serde(default)]
    after: Option<i64>,
    #[serde(default = "default_chat_limit")]
    limit: u32,
}

const fn default_chat_limit() -> u32 {
    40
}

/// Characters of one message shown in the stream before it is folded; the
/// whole of it is one tap away, as the item itself.
const CHAT_TEXT: usize = 1200;

/// One message in a conversation (design 06 v0.5, "对话").
#[derive(Serialize)]
struct ChatMessage {
    id: String,
    /// Local calendar day, for the separators.
    day: String,
    /// Local time of day.
    time: String,
    ms: i64,
    /// The user's own message: drawn on the right.
    mine: bool,
    connector: String,
    /// `mail`, `direct`, `group` or `channel`.
    place: &'static str,
    /// For a group or channel, its name: the message was not said to the
    /// user alone, and the stream says so.
    place_name: Option<String>,
    subject: Option<String>,
    text: String,
    folded: bool,
    level: String,
    /// Who said it, in a group or channel, where there is more than one
    /// other voice. Absent in a one-to-one conversation.
    author: Option<String>,
}

#[derive(Serialize)]
struct ChatPage {
    /// Oldest first, as a conversation is read.
    messages: Vec<ChatMessage>,
    /// Whether there is more before the first of these.
    earlier: bool,
}

/// Messages as the conversation draws them, oldest first as given.
fn chat_messages(
    system: &System,
    items: &[Item],
    with_author: bool,
) -> Result<Vec<ChatMessage>, ApiError> {
    let thread_ids: Vec<_> = items.iter().map(|i| i.thread_id).collect();
    let threads = system.store.threads_by_id(&thread_ids)?;
    // A page names a handful of people; look each up once.
    let mut names: std::collections::BTreeMap<genatrix_model::PersonId, String> =
        std::collections::BTreeMap::new();
    Ok(items
        .iter()
        .map(|item| {
            let thread = threads.get(&item.thread_id);
            let place = match thread.map(|t| t.kind) {
                Some(genatrix_model::ThreadKind::GroupChat) => "group",
                Some(genatrix_model::ThreadKind::Channel) => "channel",
                Some(genatrix_model::ThreadKind::DirectChat) => "direct",
                _ => "mail",
            };
            let mine = item.direction == genatrix_model::Direction::Outbound;
            let author = (with_author && !mine)
                .then(|| {
                    item.author.map(|a| {
                        names
                            .entry(a)
                            .or_insert_with(|| name_of(system, Some(a)))
                            .clone()
                    })
                })
                .flatten();
            let local = item.occurred_at.with_timezone(&chrono::Local);
            let text = item.text.trim();
            let shown: String = text.chars().take(CHAT_TEXT).collect();
            ChatMessage {
                id: item.id.to_string(),
                day: local.format("%Y-%m-%d").to_string(),
                time: local.format("%H:%M").to_string(),
                ms: item.occurred_at.timestamp_millis(),
                mine,
                connector: item.source.connector.as_str().to_owned(),
                place,
                place_name: matches!(place, "group" | "channel")
                    .then(|| thread.and_then(|t| t.title.clone()))
                    .flatten(),
                subject: match &item.payload {
                    genatrix_model::Payload::Mail { subject, .. } if !subject.is_empty() => {
                        Some(subject.clone())
                    }
                    _ => None,
                },
                folded: shown.len() < text.len(),
                text: shown,
                level: item.sensitivity.as_str().to_owned(),
                author,
            }
        })
        .collect())
}

/// A conversation with one person, a page at a time, newest page first.
async fn chat(
    State(system): Shared,
    Path(id): Path<String>,
    Query(query): Query<ChatQuery>,
) -> Result<Response, ApiError> {
    let Ok(id) = id.parse::<genatrix_model::PersonId>() else {
        return Ok(not_found("that is not a person identifier"));
    };
    let limit = query.limit.clamp(1, 200);
    // One more than asked, to know whether there is an earlier page.
    let mut items =
        system
            .store
            .items_with_person_before(id, query.after, query.before, limit + 1)?;
    let earlier = items.len() > limit as usize;
    items.truncate(limit as usize);
    items.reverse();
    let messages = chat_messages(&system, &items, false)?;
    Ok(Json(ChatPage { messages, earlier }).into_response())
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
        .map(handle_view)
        .collect())
}

fn card(system: &System, o: &genatrix_store::PersonOverview) -> Result<PersonCard, ApiError> {
    Ok(card_with(
        o,
        handles_view(system, o.person.id)?,
        system
            .store
            .get_relationship(o.person.id)?
            .map(|r| r.roles)
            .unwrap_or_default(),
        None,
    ))
}

fn card_with(
    o: &genatrix_store::PersonOverview,
    handles: Vec<HandleView>,
    roles: Vec<String>,
    last_text: Option<String>,
) -> PersonCard {
    PersonCard {
        id: o.person.id.to_string(),
        name: o.person.display_name.clone(),
        handles,
        from_them: o.from_them,
        to_them: o.to_them,
        last_at: o.last_at.map(|t| t.format("%Y-%m-%d").to_string()),
        connectors: o.connectors.clone(),
        roles,
        last_text,
    }
}

fn handle_view(h: genatrix_model::Handle) -> HandleView {
    HandleView {
        kind: format!("{:?}", h.kind).to_lowercase(),
        value: h.value,
        inferred: h.confidence == genatrix_model::Confidence::Inferred,
    }
}

/// One entry in the list of conversations: a person, or a group or channel
/// (design 06 v0.6: a multi-party container is one party).
#[derive(Serialize)]
struct Party {
    /// `person`, `group`, `channel` or `agent`.
    kind: &'static str,
    id: String,
    name: String,
    last_at: Option<String>,
    last_ms: i64,
    last_text: Option<String>,
    /// In a group, who said the last thing.
    last_author: Option<String>,
    roles: Vec<String>,
    /// For a person, the messages either way one to one; for a group, all
    /// of its messages kept here.
    messages: u64,
}

/// Size and last activity of every group and channel, at most once a minute,
/// off the async runtime; the same reasoning as [`people_overview`].
async fn group_overview(
    system: &Arc<System>,
) -> Result<Vec<genatrix_store::GroupOverview>, ApiError> {
    const KEEP: std::time::Duration = std::time::Duration::from_secs(60);
    if let Some((at, cached)) = system
        .groups
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        && at.elapsed() < KEEP
    {
        return Ok(cached.clone());
    }
    let for_task = Arc::clone(system);
    let fresh = tokio::task::spawn_blocking(move || for_task.store.group_overview())
        .await
        .map_err(|e| ApiError(anyhow::anyhow!(e)))??;
    *system
        .groups
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some((std::time::Instant::now(), fresh.clone()));
    Ok(fresh)
}

fn day_of_ms(ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms).map(|t| {
        t.with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string()
    })
}

/// Every conversation, people and groups together, most recent first.
async fn chats(State(system): Shared) -> Result<Json<Vec<Party>>, ApiError> {
    const SHOWN: usize = 300;
    let people = people_overview(&system).await?;
    let groups = group_overview(&system).await?;

    let mut parties: Vec<(i64, bool, usize)> = people
        .iter()
        .enumerate()
        .map(|(i, o)| (o.last_at.map_or(0, |t| t.timestamp_millis()), true, i))
        .chain(
            groups
                .iter()
                .enumerate()
                .map(|(i, g)| (g.last_ms, false, i)),
        )
        .collect();
    parties.sort_by_key(|(ms, _, _)| std::cmp::Reverse(*ms));
    parties.truncate(SHOWN);

    let person_ids: Vec<_> = parties
        .iter()
        .filter(|p| p.1)
        .map(|p| people[p.2].person.id)
        .collect();
    let thread_ids: Vec<_> = parties
        .iter()
        .filter(|p| !p.1)
        .map(|p| groups[p.2].thread.id)
        .collect();
    let mut person_words = system.store.last_words(&person_ids)?;
    let mut roles = system.store.roles_for(&person_ids)?;
    let mut group_words = system.store.group_last_words(&thread_ids)?;
    let flat = |t: String| t.replace('\n', " ").trim().to_owned();

    let out = parties
        .into_iter()
        .map(|(ms, is_person, i)| {
            if is_person {
                let o = &people[i];
                Party {
                    kind: "person",
                    id: o.person.id.to_string(),
                    name: o.person.display_name.clone(),
                    last_at: day_of_ms(ms),
                    last_ms: ms,
                    last_text: person_words.remove(&o.person.id).map(flat),
                    last_author: None,
                    roles: roles.remove(&o.person.id).unwrap_or_default(),
                    messages: o.from_them + o.to_them,
                }
            } else {
                let g = &groups[i];
                let word = group_words.remove(&g.thread.id);
                Party {
                    kind: if g.thread.kind == genatrix_model::ThreadKind::Channel {
                        "channel"
                    } else {
                        "group"
                    },
                    id: g.thread.id.to_string(),
                    name: g.thread.title.clone().unwrap_or_default(),
                    last_at: day_of_ms(ms),
                    last_ms: ms,
                    last_author: word
                        .as_ref()
                        .and_then(|w| w.author)
                        .map(|a| name_of(&system, Some(a))),
                    last_text: word.map(|w| flat(w.text)),
                    roles: Vec::new(),
                    messages: g.messages,
                }
            }
        })
        .collect::<Vec<_>>();
    // Installed agents are parties too (design 06 v0.5, design 11): the
    // same list, ordered by when each last said something.
    let mut out = out;
    for agent in system.store.all_agents()? {
        let Ok(c) = super::agents::card(&system, &agent) else {
            continue;
        };
        out.push(Party {
            kind: "agent",
            id: c.id,
            name: c.name,
            last_at: day_of_ms(c.last_ms),
            last_ms: c.last_ms,
            last_text: c.last_text.map(flat),
            last_author: None,
            roles: Vec::new(),
            messages: c.runs,
        });
    }
    out.sort_by_key(|p| std::cmp::Reverse(p.last_ms));
    Ok(Json(out))
}

#[derive(Serialize)]
struct Speaker {
    name: String,
    count: u64,
    me: bool,
}

#[derive(Serialize)]
struct GroupDetail {
    id: String,
    name: String,
    /// `group` or `channel`.
    kind: &'static str,
    /// People who have said something here; not its membership, which is
    /// not known.
    voices: u64,
    messages: u64,
    mine: u64,
    first_at: Option<String>,
    last_at: Option<String>,
    speakers: Vec<Speaker>,
}

/// The facts about one group or channel: its own, not any member's.
async fn group(State(system): Shared, Path(id): Path<String>) -> Result<Response, ApiError> {
    let Ok(id) = id.parse::<genatrix_model::ThreadId>() else {
        return Ok(not_found("that is not a thread identifier"));
    };
    let Some(thread) = system.store.get_thread(id)? else {
        return Ok(not_found("no such conversation"));
    };
    if !matches!(
        thread.kind,
        genatrix_model::ThreadKind::GroupChat | genatrix_model::ThreadKind::Channel
    ) {
        return Ok(not_found("that is not a group or a channel"));
    }
    let overview = group_overview(&system).await?;
    let row = overview.iter().find(|g| g.thread.id == id);
    let me = system.store.self_person()?.map(|p| p.id);
    let speakers = system
        .store
        .speakers(id, 6)?
        .into_iter()
        .map(|(p, count)| Speaker {
            name: name_of(&system, Some(p)),
            count,
            me: me == Some(p),
        })
        .collect();
    Ok(Json(GroupDetail {
        id: id.to_string(),
        name: thread.title.clone().unwrap_or_default(),
        kind: if thread.kind == genatrix_model::ThreadKind::Channel {
            "channel"
        } else {
            "group"
        },
        voices: system.store.voices(id)?,
        messages: row.map_or(0, |g| g.messages),
        mine: row.map_or(0, |g| g.mine),
        first_at: row.and_then(|g| day_of_ms(g.first_ms)),
        last_at: row.and_then(|g| day_of_ms(g.last_ms)),
        speakers,
    })
    .into_response())
}

/// A group's or channel's conversation, a page at a time.
async fn group_chat(
    State(system): Shared,
    Path(id): Path<String>,
    Query(query): Query<ChatQuery>,
) -> Result<Response, ApiError> {
    let Ok(id) = id.parse::<genatrix_model::ThreadId>() else {
        return Ok(not_found("that is not a thread identifier"));
    };
    let limit = query.limit.clamp(1, 200);
    let mut items =
        system
            .store
            .items_in_thread_before(id, query.after, query.before, limit + 1)?;
    let earlier = items.len() > limit as usize;
    items.truncate(limit as usize);
    items.reverse();
    let messages = chat_messages(&system, &items, true)?;
    Ok(Json(ChatPage { messages, earlier }).into_response())
}

/// The arithmetic behind the People page, worked out at most once a minute.
///
/// It is two passes over every item in the store, and the answer changes
/// only when mail arrives, so recomputing it on every visit to the page
/// made the page cost as much as the corpus is large. The work runs off
/// the async runtime, because a scan of hundreds of megabytes of encrypted
/// pages must not sit on a thread that other requests need.
async fn people_overview(
    system: &Arc<System>,
) -> Result<Vec<genatrix_store::PersonOverview>, ApiError> {
    const KEEP: std::time::Duration = std::time::Duration::from_secs(60);
    if let Some((at, cached)) = system
        .people
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        && at.elapsed() < KEEP
    {
        return Ok(cached.clone());
    }
    let for_task = Arc::clone(system);
    let fresh = tokio::task::spawn_blocking(move || {
        for_task.store.people_overview(500).map(|people| {
            people
                .into_iter()
                .filter(|o| o.from_them + o.to_them > 0)
                .collect::<Vec<_>>()
        })
    })
    .await
    .map_err(|e| ApiError(anyhow::anyhow!(e)))??;
    *system
        .people
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some((std::time::Instant::now(), fresh.clone()));
    Ok(fresh)
}

/// Everyone the user has exchanged messages with, most recent first
/// (design 07: the relationship's statistics are arithmetic, not opinion).
async fn people(
    State(system): Shared,
    Query(query): Query<PeopleQuery>,
) -> Result<Json<Vec<PersonCard>>, ApiError> {
    let mut overview = people_overview(&system).await?;
    overview.truncate(query.limit.min(500) as usize);
    // Two questions for the whole page, not two per card.
    let ids: Vec<_> = overview.iter().map(|o| o.person.id).collect();
    let mut handles = system.store.handles_for(&ids)?;
    let mut roles = system.store.roles_for(&ids)?;
    let mut last = system.store.last_words(&ids)?;
    let out: Vec<PersonCard> = overview
        .iter()
        .map(|o| {
            card_with(
                o,
                handles
                    .remove(&o.person.id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(handle_view)
                    .collect(),
                roles.remove(&o.person.id).unwrap_or_default(),
                last.remove(&o.person.id)
                    .map(|t| t.replace('\n', " ").trim().to_owned()),
            )
        })
        .collect();
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

#[derive(Serialize)]
pub(super) struct ActionView {
    id: String,
    kind: String,
    /// Where it would go, in words: recipients and subject, or a chat's name.
    target: String,
    account: String,
    rationale: String,
    evidence: Vec<SourceRef>,
    draft: String,
    version: u32,
    payload_hash: String,
    status: String,
    status_detail: String,
    created_at: String,
    expires_in_secs: i64,
    /// Present on a read of one pending action: what approving must present
    /// back.
    nonce: Option<String>,
    /// Drafted where. Local, always, in phase one.
    drafted: &'static str,
    versions: u32,
    /// Approved and not yet taken by a connector: the user can still take
    /// the approval back.
    can_withdraw: bool,
    /// The outbound item the action produced, once it has.
    result: Option<SourceRef>,
    /// Whether the draft may be edited. An agent's proposal may not.
    editable: bool,
    /// For an agent's proposal: what the fixed template shows.
    card: Option<genatrix_agent::Card>,
}

pub(super) fn action_view(
    system: &System,
    a: &genatrix_agent::Action,
    nonce: Option<String>,
) -> ActionView {
    use genatrix_agent::action::Status;
    let (target, account) = match &a.effect {
        genatrix_agent::Effect::SendMail {
            account,
            to,
            subject,
            ..
        } => (format!("{} · {subject}", to.join(", ")), account.clone()),
        genatrix_agent::Effect::SendMessage { account, chat, .. } => {
            let title = system
                .store
                .find_thread(&genatrix_model::Source::new(
                    genatrix_model::Connector::Telegram,
                    account.clone(),
                    chat.clone(),
                ))
                .ok()
                .flatten()
                .and_then(|t| t.title)
                .unwrap_or_else(|| chat.clone());
            (title, account.clone())
        }
        genatrix_agent::Effect::CreateEvent { account } => ("calendar".to_owned(), account.clone()),
        genatrix_agent::Effect::WriteMemory { collection } => (collection.clone(), String::new()),
        genatrix_agent::Effect::Agent { name, label, .. } => {
            (format!("{name} · {label}"), String::new())
        }
    };
    let card = match &a.effect {
        genatrix_agent::Effect::Agent { card, .. } => Some(card.clone()),
        _ => None,
    };
    let status_detail = match &a.status {
        Status::Declined { reason } => reason.clone(),
        Status::Failed { detail } | Status::Unknown { detail } => detail.clone(),
        Status::Approved { .. } if a.is_in_a_connectors_hands() => {
            "handed to the connector".to_owned()
        }
        _ => String::new(),
    };
    let result = match &a.status {
        Status::Executed { result: Some(id) } => source_ref(system, *id),
        _ => None,
    };
    let can_withdraw =
        matches!(a.status, Status::Approved { .. }) && a.token_for_execution().is_some();
    let current = a.current();
    ActionView {
        id: a.id.clone(),
        kind: a.effect.kind().to_owned(),
        target,
        account,
        rationale: a.rationale.clone(),
        evidence: a
            .evidence
            .iter()
            .filter_map(|id| source_ref(system, *id))
            .collect(),
        draft: current.payload.clone(),
        version: current.seq,
        payload_hash: current.payload_hash.clone(),
        status: crate::actions::status_of(a).to_owned(),
        status_detail,
        created_at: a.created_at.format("%Y-%m-%d %H:%M").to_string(),
        expires_in_secs: crate::actions::expires_in(a, chrono::Utc::now()),
        nonce,
        drafted: "on this device",
        versions: u32::try_from(a.versions.len()).unwrap_or(0),
        can_withdraw,
        result,
        editable: a.effect.editable(),
        card,
    }
}

#[derive(Deserialize)]
struct ActionsQuery {
    #[serde(default)]
    status: String,
    #[serde(default = "default_actions_limit")]
    limit: u32,
}

const fn default_actions_limit() -> u32 {
    50
}

/// Actions, pending ones with a nonce each (design 06: the approval panel;
/// design 03: what was shown is what may be approved).
async fn actions(
    State(system): Shared,
    Query(query): Query<ActionsQuery>,
) -> Result<Json<Vec<ActionView>>, ApiError> {
    let status = (!query.status.is_empty()).then_some(query.status.as_str());
    let list = system
        .actions
        .list(&system.store, status, query.limit.min(500))?;
    Ok(Json(
        list.iter()
            .map(|a| {
                let nonce = matches!(a.status, genatrix_agent::action::Status::Pending)
                    .then(|| system.actions.nonce_for(&a.id));
                action_view(&system, a, nonce)
            })
            .collect(),
    ))
}

async fn action(State(system): Shared, Path(id): Path<String>) -> Result<Response, ApiError> {
    let Some(a) = system.actions.get(&system.store, &id)? else {
        return Ok(not_found("no such action"));
    };
    let nonce = matches!(a.status, genatrix_agent::action::Status::Pending)
        .then(|| system.actions.nonce_for(&a.id));
    Ok(Json(action_view(&system, &a, nonce)).into_response())
}

/// Draft a reply to an item: a pending action, for the panel.
async fn draft_reply(State(system): Shared, Path(id): Path<String>) -> Result<Response, ApiError> {
    let Ok(id) = id.parse::<ItemId>() else {
        return Ok(not_found("that is not an item identifier"));
    };
    let Some(item) = system.store.get_item(id)? else {
        return Ok(not_found("no such item"));
    };
    if !system.model_state.borrow().is_ready() {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "the model side is not ready" })),
        )
            .into_response());
    }
    let mut ctx = RunContext::begin(&system.ledger, &system.caller, "draft", 4)?;
    let run_id = ctx.run_id.clone();
    let drafted = match crate::pipeline::draft::reply_to(&system.store, &mut ctx, &item).await {
        Ok(d) => {
            ctx.done()?;
            d
        }
        Err(e) => {
            ctx.stopped(e.to_string())?;
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response());
        }
    };
    // A chat reply goes to the conversation the item is in.
    let effect = match drafted.effect {
        genatrix_agent::Effect::SendMessage {
            account, reply_to, ..
        } => {
            let chat = system
                .store
                .get_thread(item.thread_id)?
                .map(|t| t.source.external_id)
                .unwrap_or_default();
            genatrix_agent::Effect::SendMessage {
                account,
                chat,
                reply_to,
            }
        }
        other => other,
    };
    let action = system.actions.propose(
        &system.store,
        &system.ledger,
        &run_id,
        effect,
        drafted.reply,
        drafted.rationale,
        drafted.evidence,
    )?;
    let nonce = system.actions.nonce_for(&action.id);
    Ok(Json(action_view(&system, &action, Some(nonce))).into_response())
}

#[derive(Deserialize)]
struct EditAction {
    payload: String,
}

async fn edit_action(
    State(system): Shared,
    Path(id): Path<String>,
    Json(body): Json<EditAction>,
) -> Result<Response, ApiError> {
    match system
        .actions
        .edit(&system.store, &system.ledger, &id, body.payload)
    {
        Ok(a) => {
            let nonce = system.actions.nonce_for(&a.id);
            Ok(Json(action_view(&system, &a, Some(nonce))).into_response())
        }
        Err(e) => Ok(decision_error(&e)),
    }
}

#[derive(Deserialize)]
struct ApproveAction {
    version: u32,
    payload_hash: String,
    nonce: String,
}

/// Approval: this version, this hash, from the page that showed it. Only
/// reachable on this machine, like every route here.
async fn approve_action(
    State(system): Shared,
    Path(id): Path<String>,
    Json(body): Json<ApproveAction>,
) -> Result<Response, ApiError> {
    let approval = genatrix_agent::Approval {
        version: body.version,
        payload_hash: body.payload_hash,
    };
    match system
        .actions
        .approve(&system.store, &system.ledger, &id, &approval, &body.nonce)
    {
        Ok(a) => {
            // An agent's own effect is carried out by the core, now, rather
            // than waiting for the next pass of the loop.
            if matches!(a.effect, genatrix_agent::Effect::Agent { .. }) {
                let system = std::sync::Arc::clone(&system);
                tokio::spawn(async move {
                    if let Err(e) = crate::agents::execute_approved(system).await {
                        tracing::warn!(error = %e, "could not carry out an agent's approved action");
                    }
                });
            }
            Ok(Json(action_view(&system, &a, None)).into_response())
        }
        Err(e) => Ok(decision_error(&e)),
    }
}

#[derive(Deserialize)]
struct DeclineAction {
    #[serde(default)]
    reason: String,
}

async fn decline_action(
    State(system): Shared,
    Path(id): Path<String>,
    Json(body): Json<DeclineAction>,
) -> Result<Response, ApiError> {
    match system
        .actions
        .decline(&system.store, &system.ledger, &id, &body.reason)
    {
        Ok(a) => Ok(Json(action_view(&system, &a, None)).into_response()),
        Err(e) => Ok(decision_error(&e)),
    }
}

fn decision_error(e: &crate::actions::DecisionError) -> Response {
    use crate::actions::DecisionError;
    let status = match e {
        DecisionError::NotFound => StatusCode::NOT_FOUND,
        DecisionError::BadNonce | DecisionError::Action(_) => StatusCode::CONFLICT,
        DecisionError::Other(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(serde_json::json!({ "error": e.to_string() }))).into_response()
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
    /// Here, and not only on Today, because the page asks for the status
    /// every few seconds and this is the one number it needs that often.
    /// Today is expensive; nothing should poll it.
    pending_actions: u64,
}

async fn status(State(system): Shared) -> Result<Json<Status>, ApiError> {
    let model = system.model_state.borrow().clone();
    let slow = slow_status(&system).await?;
    Ok(Json(Status {
        model_text: model.describe(),
        model,
        embedded: system.store.count_embedded_items()?,
        summarized: system.store.count_annotated_items("summary")?,
        judged: system.store.count_annotated_items("sensitivity")?,
        items: system.store.count_items()?,
        pending_actions: system.store.count_actions("pending")?,
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
async fn slow_status(system: &Arc<System>) -> Result<crate::system::SlowStatus, ApiError> {
    const KEEP: std::time::Duration = std::time::Duration::from_secs(30);
    if let Some((at, cached)) = *system
        .slow_status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        && at.elapsed() < KEEP
    {
        return Ok(cached);
    }
    // Walking tens of thousands of files, and decoding every egress record,
    // is not something to do on a thread the rest of the interface needs.
    let for_task = Arc::clone(system);
    let fresh = tokio::task::spawn_blocking(move || {
        Ok::<_, ApiError>(crate::system::SlowStatus {
            bytes_on_disk: for_task.raw_files.size_on_disk()?
                + for_task.blob_files.size_on_disk()?,
            bytes_left_device: bytes_out(&for_task)?,
        })
    })
    .await
    .map_err(|e| ApiError(anyhow::anyhow!(e)))??;
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
    /// Every run, newest first: what ran and how it ended (design 06,
    /// "运行记录").
    runs: Vec<RunLine>,
    /// Every step an action took, newest first (design 06, "动作记录").
    actions: Vec<ActionEvent>,
}

#[derive(Serialize)]
struct RunLine {
    id: String,
    at: String,
    task: String,
    max_steps: u32,
    /// `done`, `stopped`, or `running`.
    end: String,
    steps: u32,
    reason: String,
}

#[derive(Serialize)]
struct ActionEvent {
    at: String,
    action: String,
    event: String,
    by: String,
    kind: String,
    version: u32,
    detail: String,
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
        runs: runs(&system)?,
        actions: action_events(&system)?,
    }))
}

/// The last runs with how each ended.
fn runs(system: &System) -> Result<Vec<RunLine>, ApiError> {
    use genatrix_agent::run::{RunEnd, RunStart};
    let starts = system.ledger.entries(&EntryFilter {
        kind: Some(kind::RUN.into()),
        limit: 60,
        ..Default::default()
    })?;
    let ends = system.ledger.entries(&EntryFilter {
        kind: Some(kind::RUN_END.into()),
        limit: 200,
        ..Default::default()
    })?;
    let mut out = Vec::with_capacity(starts.len());
    for entry in starts.iter().rev() {
        let Ok(start) = entry.decode::<RunStart>() else {
            continue;
        };
        let end = ends
            .iter()
            .find(|e| e.subject == entry.subject)
            .and_then(|e| e.decode::<RunEnd>().ok());
        let (end, steps, reason) = match end {
            Some(RunEnd::Done { steps, .. }) => ("done".to_owned(), steps, String::new()),
            Some(RunEnd::Stopped { steps, reason }) => ("stopped".to_owned(), steps, reason),
            None => ("running".to_owned(), 0, String::new()),
        };
        out.push(RunLine {
            id: entry.subject.clone(),
            at: entry.at.format("%Y-%m-%d %H:%M").to_string(),
            task: start.task,
            max_steps: start.max_steps,
            end,
            steps,
            reason,
        });
    }
    Ok(out)
}

/// Every recorded step of every action, newest first.
fn action_events(system: &System) -> Result<Vec<ActionEvent>, ApiError> {
    let entries = system.ledger.entries(&EntryFilter {
        kind: Some(kind::ACTION.into()),
        limit: 200,
        ..Default::default()
    })?;
    Ok(entries
        .iter()
        .rev()
        .filter_map(|entry| {
            let r = entry.decode::<crate::actions::ActionRecord>().ok()?;
            Some(ActionEvent {
                at: entry.at.format("%Y-%m-%d %H:%M").to_string(),
                action: r.action,
                event: r.event,
                by: r.by,
                kind: r.kind,
                version: r.version,
                detail: r.detail,
            })
        })
        .collect())
}

#[derive(Serialize)]
struct RunStep {
    at: String,
    kind: String,
    body: serde_json::Value,
}

/// One run, every record under it, oldest first: the replay design 03
/// promises and design 06 opens from the folded steps line.
async fn run_record(State(system): Shared, Path(id): Path<String>) -> Result<Response, ApiError> {
    if id.len() != 26 || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Ok(not_found("that is not a run identifier"));
    }
    let entries = system.ledger.entries(&EntryFilter {
        subject: Some(id),
        limit: 200,
        ..Default::default()
    })?;
    let steps: Vec<RunStep> = entries
        .iter()
        .filter(|e| e.kind.starts_with("run"))
        .map(|e| RunStep {
            at: e.at.format("%Y-%m-%d %H:%M:%S").to_string(),
            kind: e.kind.clone(),
            body: e.body.clone(),
        })
        .collect();
    if steps.is_empty() {
        return Ok(not_found("no such run"));
    }
    Ok(Json(steps).into_response())
}

#[derive(Deserialize)]
struct AskBody {
    question: String,
    #[serde(default)]
    history: Vec<crate::conversation::Exchange>,
}

#[derive(Serialize)]
struct AskReply {
    run_id: String,
    answer: String,
    cited: Vec<SourceRef>,
    steps: Vec<String>,
    actions: Vec<ActionView>,
    stopped: bool,
    /// Where it was answered. Local, always, in phase one (design 06: every
    /// reply says whether it was done here or in the cloud).
    answered: &'static str,
}

/// A question to the conversation (design 03, "会话"; design 06, "对话").
async fn ask(State(system): Shared, Json(body): Json<AskBody>) -> Result<Response, ApiError> {
    let question = body.question.trim();
    if question.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "ask something" })),
        )
            .into_response());
    }
    if !system.model_state.borrow().is_ready() {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "the model is not ready yet" })),
        )
            .into_response());
    }
    let history: Vec<_> = body.history.into_iter().rev().take(6).rev().collect();
    let desk = crate::conversation::Desk {
        store: &system.store,
        ledger: &system.ledger,
        actions: &system.actions,
        typed_level: system.gate.level_of_typed_text(question),
        embed: system.embedder_configured(),
    };
    let reply = crate::conversation::ask(&desk, &system.caller, &history, question)
        .await
        .map_err(|e| ApiError(anyhow::anyhow!(e.to_string())))?;
    let actions = reply
        .actions
        .iter()
        .filter_map(|id| system.actions.get(&system.store, id).ok().flatten())
        .map(|a| {
            let nonce = Some(system.actions.nonce_for(&a.id));
            action_view(&system, &a, nonce)
        })
        .collect();
    Ok(Json(AskReply {
        run_id: reply.run_id,
        answer: reply.answer,
        cited: reply
            .cited
            .iter()
            .filter_map(|id| source_ref(&system, *id))
            .collect(),
        steps: reply.steps,
        actions,
        stopped: reply.stopped,
        answered: "on this device",
    })
    .into_response())
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
