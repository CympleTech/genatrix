//! The daily digest: what the last day brought, in three groups.
//!
//! Design: `docs/design/03-agent-layer.md` (the daily pipeline: the last 24
//! hours, grouped, one point per group entry, sources on every point) and
//! `docs/design/06-interface.md` (the Today page: needs your reply, you
//! promised, worth knowing).
//!
//! The model sorts each message into a group and writes one line for it,
//! in the message's language, with the sender named. It sees messages in
//! envelopes and answers by their numbers; a line about a number outside
//! the batch is dropped, and every kept point carries the item it came
//! from, so the page can open it. "You promised" is not the model's: it is
//! the open commitments, which have their own evidence.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use genatrix_agent::envelope::{self, Source as EnvelopeSource};
use genatrix_agent::protocol::strip_reasoning;
use genatrix_agent::run::{ModelCaller, RunContext, RunError};
use genatrix_gate::gate::{Initiator, Message, Request};
use genatrix_llm::ticket::Purpose;
use genatrix_model::{
    AnnotationKind, Digest, DigestGroup, DigestPoint, Direction, Item, Level, Payload, PersonId,
};
use genatrix_store::{ItemQuery, ItemVersion, Store};

use super::store_error;

/// Messages per model call.
pub const BATCH: usize = 10;
/// The most personal messages a day's digest considers, newest first.
const MAX_PERSONAL: usize = 120;
/// The most public ones (newsletters, channels) it considers.
const MAX_PUBLIC: usize = 30;
/// Characters of a message the model sees.
const EXCERPT_CHARS: usize = 400;
/// The most "worth knowing" lines kept.
const MAX_KNOW: usize = 12;

const SYSTEM: &str = "/no_think
You prepare the morning digest for one person, called ME, from the messages \
that arrived. For each numbered message decide:
- reply: a person is asking ME something or waiting on ME.
- know: worth one line for ME to know today; no action needed.
- skip: marketing, notifications, group chatter, or nothing new.
Then write one short line (under 120 characters) saying who and what, in the \
language of the message, for reply and know. Nothing for skip.

Output one JSON line per message and nothing else:
{\"id\": <number>, \"group\": \"reply\" | \"know\" | \"skip\", \"point\": \"<line or empty>\"}";

/// One judged message.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sorted {
    id: usize,
    group: Option<DigestGroup>,
    point: String,
}

/// Make the digest for `day` from the 24 hours before `now`.
pub async fn run<C: ModelCaller>(
    store: &Store,
    ctx: &mut RunContext<'_, C>,
    now: DateTime<Utc>,
    day: NaiveDate,
) -> Result<Digest, RunError> {
    let me = store
        .self_person()
        .map_err(|e| RunError::Ledger(store_error(&e)))?
        .map(|p| p.id);
    let recent = store
        .query_items(&ItemQuery {
            since: Some(now - Duration::hours(24)),
            until: Some(now),
            version: ItemVersion::Current,
            limit: 2000,
            ..Default::default()
        })
        .map_err(|e| RunError::Ledger(store_error(&e)))?;

    // What the user wrote themselves is not news to them.
    let mut personal: Vec<Item> = Vec::new();
    let mut public: Vec<Item> = Vec::new();
    for item in recent {
        if item.direction == Direction::Outbound || item.text.trim().is_empty() {
            continue;
        }
        if item.sensitivity == Level::Public {
            if public.len() < MAX_PUBLIC {
                public.push(item);
            }
        } else if personal.len() < MAX_PERSONAL {
            personal.push(item);
        }
    }
    let considered: Vec<Item> = personal.into_iter().chain(public).collect();

    let mut reply = Vec::new();
    let mut know = Vec::new();
    for batch in considered.chunks(BATCH) {
        for sorted in ask(ctx, store, me, batch).await? {
            let Some(item) = batch.get(sorted.id.wrapping_sub(1)) else {
                continue;
            };
            let point = DigestPoint {
                text: sorted.point,
                sources: vec![item.id],
            };
            match sorted.group {
                Some(DigestGroup::NeedsReply) => reply.push((item.occurred_at, point)),
                Some(DigestGroup::WorthKnowing) => know.push((item.occurred_at, point)),
                _ => {}
            }
        }
    }
    reply.sort_by_key(|(at, _)| std::cmp::Reverse(at.timestamp()));
    know.sort_by_key(|(at, _)| std::cmp::Reverse(at.timestamp()));
    know.truncate(MAX_KNOW);

    // The user's own open promises, with their evidence.
    let promised: Vec<DigestPoint> = store
        .pending_commitments()
        .map_err(|e| RunError::Ledger(store_error(&e)))?
        .into_iter()
        .filter(|c| c.is_mine(me))
        .map(|c| DigestPoint {
            text: match c.due {
                Some(due) => format!("{} · by {}", c.what, due.format("%b %-d")),
                None => c.what,
            },
            sources: c.evidence,
        })
        .collect();

    Ok(Digest {
        day,
        generated_at: Utc::now(),
        considered: u32::try_from(considered.len()).unwrap_or(u32::MAX),
        groups: vec![
            (
                DigestGroup::NeedsReply,
                reply.into_iter().map(|(_, p)| p).collect(),
            ),
            (DigestGroup::Promised, promised),
            (
                DigestGroup::WorthKnowing,
                know.into_iter().map(|(_, p)| p).collect(),
            ),
        ],
    })
}

async fn ask<C: ModelCaller>(
    ctx: &mut RunContext<'_, C>,
    store: &Store,
    me: Option<PersonId>,
    batch: &[Item],
) -> Result<Vec<Sorted>, RunError> {
    let sources: Vec<EnvelopeSource> = batch
        .iter()
        .enumerate()
        .map(|(i, item)| EnvelopeSource {
            id: (i + 1).to_string(),
            label: format!("message {}", i + 1),
            body: describe(store, me, item),
        })
        .collect();
    let zone = envelope::data_zone(&sources);
    let expected = batch.len();
    let request = Request {
        purpose: Purpose::Summarize,
        initiator: Initiator::Rule {
            name: "daily-digest".into(),
        },
        items: batch.iter().map(|i| i.id).collect(),
        level: Level::Secret,
        identities: vec![],
        messages: vec![
            Message::system(SYSTEM),
            Message::user(format!(
                "{}\nNow one JSON line for each of messages 1 to {expected}.",
                zone.text
            )),
        ],
        max_tokens: Some(80 * u32::try_from(expected).unwrap_or(10) + 100),
        temperature: Some(0.0),
        stream: false,
    };
    ctx.ask("digest-batch", &request, move |reply| {
        Ok(parse(&reply.text, expected))
    })
    .await
}

/// Who, where, when, and what: the summary when there is one, the opening
/// otherwise.
fn describe(store: &Store, me: Option<PersonId>, item: &Item) -> String {
    let who = match item.author {
        Some(p) if me == Some(p) => "ME".to_owned(),
        Some(p) => store
            .get_person(p)
            .ok()
            .flatten()
            .map_or_else(|| "someone".to_owned(), |x| x.display_name),
        None => "someone".to_owned(),
    };
    let thread = store
        .get_thread(item.thread_id)
        .ok()
        .flatten()
        .and_then(|t| t.title)
        .unwrap_or_default();
    let mut out = format!(
        "From {who}{} via {} at {}\n",
        if thread.is_empty() || thread == who {
            String::new()
        } else {
            format!(" in {thread}")
        },
        item.source.connector.as_str(),
        item.occurred_at.format("%H:%M")
    );
    if let Payload::Mail { subject, .. } = &item.payload
        && !subject.is_empty()
    {
        out.push_str("Subject: ");
        out.push_str(subject);
        out.push('\n');
    }
    let summary = store.annotations_of(item.id).ok().and_then(|annotations| {
        annotations.into_iter().find_map(|a| match a.kind {
            AnnotationKind::Summary { text, .. } if a.superseded_by.is_none() => Some(text),
            _ => None,
        })
    });
    if let Some(summary) = summary {
        out.push_str(&summary);
    } else {
        let text = super::classify::without_links(&item.text);
        out.extend(text.chars().take(EXCERPT_CHARS));
    }
    out
}

fn parse(raw: &str, expected: usize) -> Vec<Sorted> {
    let text = strip_reasoning(raw);
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line
            .trim()
            .trim_start_matches("```json")
            .trim_end_matches("```")
            .trim();
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(id) = value.get("id").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let id = usize::try_from(id).unwrap_or(0);
        if id == 0 || id > expected {
            continue;
        }
        let group = match value.get("group").and_then(|g| g.as_str()) {
            Some("reply") => Some(DigestGroup::NeedsReply),
            Some("know") => Some(DigestGroup::WorthKnowing),
            Some("skip") => None,
            _ => continue,
        };
        let point: String = value
            .get("point")
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .trim()
            .chars()
            .take(160)
            .collect();
        if group.is_some() && point.is_empty() {
            continue;
        }
        out.push(Sorted { id, group, point });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_sorted_into_groups_and_the_rest_dropped() {
        let raw = r#"{"id": 1, "group": "reply", "point": "Ann asks about Thursday"}
{"id": 2, "group": "skip", "point": ""}
{"id": 3, "group": "know", "point": ""}
{"id": 7, "group": "know", "point": "outside"}
{"id": 4, "group": "know", "point": "Invoice due Friday"}"#;
        let sorted = parse(raw, 4);
        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0].group, Some(DigestGroup::NeedsReply));
        assert_eq!(sorted[1].group, None);
        assert_eq!(sorted[2].id, 4);
    }
}
