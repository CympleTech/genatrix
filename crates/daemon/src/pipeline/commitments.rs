//! Who promised whom what, by when.
//!
//! Design: `docs/design/03-agent-layer.md` (the commitment pipeline runs
//! after ingestion over inbound and outbound messages) and
//! `docs/design/07-memory-profile.md` (a commitment is an assertion with
//! evidence; the model's are inferred until the user says otherwise; a
//! rejection is remembered).
//!
//! Each message is read once; a label annotation marks it read, promise or
//! no promise, so the pass never circles. The model sees who wrote the
//! message and to whom, and is asked only for promises the text states.
//! Anything it returns about a message not in the batch is dropped
//! (design 03: a claim without a source in the context is no claim).

use chrono::{NaiveDate, TimeZone, Utc};
use genatrix_agent::envelope::{self, Source as EnvelopeSource};
use genatrix_agent::protocol::strip_reasoning;
use genatrix_agent::run::{ModelCaller, RunContext, RunError};
use genatrix_gate::gate::{Initiator, Message, Request};
use genatrix_llm::ticket::Purpose;
use genatrix_model::{
    Annotation, AnnotationKind, Commitment, CommitmentId, CommitmentStatus, Direction, Item, Level,
    Payload, PersonId, Producer, Standing,
};
use genatrix_store::Store;

use super::store_error;

/// The label that marks a message as read for promises. The version is
/// part of it: a new prompt reads everything again.
pub const LABEL: &str = "commitments/1";
/// Messages per model call.
pub const BATCH: usize = 8;
/// Characters of a message the model sees.
const EXCERPT_CHARS: usize = 600;

const SYSTEM: &str = "/no_think
You read messages between the account holder, called ME, and other people. \
Find explicit promises: something the writer says they will do, give, send, \
pay, attend or decide, for the other party. Only what the message states; \
never guess. Ignore marketing, notifications and anything not written by a \
person to a person.

For each promise output one line of JSON and nothing else:
{\"id\": <message number>, \"who\": \"me\" or \"them\", \"what\": \"<short, in the message's language>\", \"due\": \"YYYY-MM-DD\" or null}
\"who\" is who made the promise. A message with no promise gets no line. \
If there is nothing at all, output nothing.";

/// What one pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Messages read.
    pub read: usize,
    /// Promises recorded.
    pub found: usize,
}

/// One promise as the model returned it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Position in the batch, 1-based.
    pub id: usize,
    /// Whether the writer was the account holder.
    pub by_me: bool,
    /// What.
    pub what: String,
    /// By when.
    pub due: Option<NaiveDate>,
}

/// Read a batch of messages nobody has read for promises yet.
pub async fn run<C: ModelCaller>(
    store: &Store,
    ctx: &mut RunContext<'_, C>,
) -> Result<Report, RunError> {
    let mut report = Report::default();
    let me = store
        .self_person()
        .map_err(|e| RunError::Ledger(store_error(&e)))?
        .map(|p| p.id);
    let candidates: Vec<Item> = store
        .items_without_label(LABEL, u32::try_from(BATCH * 3).unwrap_or(24))
        .map_err(|e| RunError::Ledger(store_error(&e)))?;

    // Correspondence only: mail and chat, to or from the user, not public.
    let (readable, skipped): (Vec<Item>, Vec<Item>) = candidates.into_iter().partition(|i| {
        matches!(i.payload, Payload::Mail { .. } | Payload::Message { .. })
            && matches!(i.direction, Direction::Inbound | Direction::Outbound)
            && i.sensitivity != Level::Public
    });
    for item in &skipped {
        mark(store, item)?;
        report.read += 1;
    }

    for batch in readable.chunks(BATCH) {
        let found = ask(ctx, store, me, batch).await?;
        for f in &found {
            let Some(item) = batch.get(f.id.wrapping_sub(1)) else {
                continue;
            };
            let Some(me) = me else { break };
            let other = other_party(store, item, me);
            let (from, to) = if f.by_me {
                (me, other)
            } else {
                match item.author {
                    Some(author) if author != me => (author, Some(me)),
                    _ => continue,
                }
            };
            let commitment = Commitment {
                id: CommitmentId::new(),
                from,
                to,
                what: f.what.clone(),
                due: f
                    .due
                    .and_then(|d| d.and_hms_opt(23, 59, 0))
                    .map(|t| Utc.from_utc_datetime(&t)),
                evidence: vec![item.id],
                status: CommitmentStatus::Open,
                standing: Standing::Inferred,
                created_at: Utc::now(),
            };
            store
                .insert_commitment(&commitment)
                .map_err(|e| RunError::Ledger(store_error(&e)))?;
            report.found += 1;
        }
        for item in batch {
            mark(store, item)?;
            report.read += 1;
        }
    }
    Ok(report)
}

/// The person on the other side of a message from the user.
fn other_party(store: &Store, item: &Item, me: PersonId) -> Option<PersonId> {
    if item.direction == Direction::Outbound {
        item.recipients.iter().find(|p| **p != me).copied()
    } else {
        item.author.filter(|a| *a != me)
    }
    .or_else(|| {
        store
            .get_thread(item.thread_id)
            .ok()
            .flatten()
            .and_then(|t| t.members.into_iter().find(|p| *p != me))
    })
}

fn mark(store: &Store, item: &Item) -> Result<(), RunError> {
    store
        .insert_annotation(&Annotation::new(
            item.id,
            Producer::Rule {
                rule: "commitments".into(),
                version: "1".into(),
            },
            AnnotationKind::Label {
                label: LABEL.into(),
            },
        ))
        .map_err(|e| RunError::Ledger(store_error(&e)))
}

async fn ask<C: ModelCaller>(
    ctx: &mut RunContext<'_, C>,
    store: &Store,
    me: Option<PersonId>,
    batch: &[Item],
) -> Result<Vec<Found>, RunError> {
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
        purpose: Purpose::Extract,
        initiator: Initiator::Rule {
            name: "commitments".into(),
        },
        items: batch.iter().map(|i| i.id).collect(),
        level: Level::Secret,
        identities: vec![],
        messages: vec![
            Message::system(SYSTEM),
            Message::user(format!(
                "{}\nNow list the promises in messages 1 to {expected}, one JSON line each, or nothing.",
                zone.text
            )),
        ],
        max_tokens: Some(400),
        temperature: Some(0.0),
        stream: false,
    };
    ctx.ask("commitments", &request, move |reply| {
        Ok(parse(&reply.text, expected))
    })
    .await
}

/// Who wrote it, to whom, when, and what it says.
fn describe(store: &Store, me: Option<PersonId>, item: &Item) -> String {
    let name = |p: Option<PersonId>| -> String {
        match p {
            Some(p) if me == Some(p) => "ME".to_owned(),
            Some(p) => store
                .get_person(p)
                .ok()
                .flatten()
                .map_or_else(|| "someone".to_owned(), |x| x.display_name),
            None => "someone".to_owned(),
        }
    };
    let to = if item.direction == Direction::Outbound {
        name(item.recipients.first().copied())
    } else {
        "ME".to_owned()
    };
    let mut out = format!(
        "Written by {} to {} on {}\n",
        name(item.author),
        to,
        item.occurred_at.format("%Y-%m-%d")
    );
    if let Payload::Mail { subject, .. } = &item.payload
        && !subject.is_empty()
    {
        out.push_str("Subject: ");
        out.push_str(subject);
        out.push('\n');
    }
    let text = super::classify::without_links(&item.text);
    out.extend(text.chars().take(EXCERPT_CHARS));
    out
}

/// JSON lines, checked. Lines that are not JSON, name a message outside
/// the batch, or say nothing are dropped without complaint: the model was
/// told "nothing" is an answer.
fn parse(raw: &str, expected: usize) -> Vec<Found> {
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
        let by_me = match value.get("who").and_then(|w| w.as_str()) {
            Some("me") => true,
            Some("them") => false,
            _ => continue,
        };
        let Some(what) = value
            .get("what")
            .and_then(|w| w.as_str())
            .map(str::trim)
            .filter(|w| !w.is_empty())
        else {
            continue;
        };
        let due = value
            .get("due")
            .and_then(|d| d.as_str())
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
        out.push(Found {
            id,
            by_me,
            what: what.chars().take(200).collect(),
            due,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promises_are_read_and_the_rest_is_dropped() {
        let raw = r#"<think></think>
{"id": 1, "who": "me", "what": "send the quote", "due": "2026-09-26"}
not json
{"id": 9, "who": "me", "what": "outside the batch", "due": null}
{"id": 2, "who": "nobody", "what": "x"}
{"id": 3, "who": "them", "what": "  pay the invoice  ", "due": "soon"}
{"id": 4, "who": "me", "what": ""}"#;
        let found = parse(raw, 4);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id, 1);
        assert!(found[0].by_me);
        assert_eq!(found[0].due, NaiveDate::from_ymd_opt(2026, 9, 26));
        assert_eq!(found[1].what, "pay the invoice");
        assert!(!found[1].by_me);
        assert_eq!(found[1].due, None, "a date it cannot read is no date");
        assert!(parse("", 4).is_empty());
    }
}
