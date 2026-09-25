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
    Payload, PersonId, Producer, Standing, ThreadKind,
};
use genatrix_store::Store;

use super::store_error;

/// The label that marks a message as read for promises. The version is
/// part of it: a new prompt reads everything again.
pub const LABEL: &str = "commitments/2";
/// Messages per model call.
pub const BATCH: usize = 8;
/// Messages handed to the model in one pass; the rest wait for the next.
const PER_PASS: usize = BATCH * 3;
/// Messages looked at in one pass. Most are set aside by the rules without
/// the model, so a pass can look at many.
const LOOKED_AT: u32 = 400;
/// How far back promises are read (design 07, "什么算承诺").
const WINDOW_DAYS: i64 = 30;
/// A promise said again within this many days is the same promise.
const SAME_WITHIN_DAYS: i64 = 14;
/// Characters of a message the model sees.
const EXCERPT_CHARS: usize = 600;

const SYSTEM: &str = "/no_think
You read messages between the account holder, called ME, and other people. \
Find promises. A promise is the writer committing, to the person they are \
writing to, to do a specific thing in the future: send, pay, give, call, \
book, finish, reply, attend. Most messages contain none; that is normal. \
When unsure, it is not a promise.

These are NOT promises:
- something already done, reported in the past (\"I sent it\", \"提交了\", \"做了体检\")
- the writer's own plans that nobody was promised (\"去鸟岛看看\", \"going to the market\")
- questions, suggestions, wishes, maybes (\"这周末吧?\", \"maybe next week\")
- requests to the other person (\"can you send it\")
- notices from shops, services and systems (orders, bookings, receipts)

For each promise output one line of JSON and nothing else:
{\"id\": <message number>, \"who\": \"me\" or \"them\", \"what\": \"<verb and object, who it is for if said; in the message's language>\", \"due\": \"YYYY-MM-DD\" or null}
\"who\" is who made the promise. \"what\" must say what will be done and to \
what: \"send Alice the signed lease\", not \"send\". \"due\" only if the \
message names a day or deadline. A message with no promise gets no line. \
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
        .items_without_label(LABEL, LOOKED_AT)
        .map_err(|e| RunError::Ledger(store_error(&e)))?;

    // The rules first (design 07, "什么算承诺"): recent correspondence
    // between people. What they set aside is marked read without the model.
    let since = Utc::now() - chrono::Duration::days(WINDOW_DAYS);
    let (mut readable, skipped): (Vec<Item>, Vec<Item>) = candidates
        .into_iter()
        .partition(|i| worth_reading(store, i, since));
    for item in &skipped {
        mark(store, item)?;
        report.read += 1;
    }
    // The model reads a few batches a pass; the rest stay unmarked for the
    // next one.
    readable.truncate(PER_PASS);

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
            if !says_enough(&f.what) {
                continue;
            }
            let since =
                item.occurred_at.with_timezone(&Utc) - chrono::Duration::days(SAME_WITHIN_DAYS);
            if store
                .open_commitment_like(from, to, &f.what, since)
                .map_err(|e| RunError::Ledger(store_error(&e)))?
            {
                continue;
            }
            // A deadline before the message itself is the model copying the
            // message's date, not a deadline.
            let written = item.occurred_at.date_naive();
            let commitment = Commitment {
                id: CommitmentId::new(),
                from,
                to,
                what: f.what.clone(),
                due: f
                    .due
                    .filter(|d| *d >= written)
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

/// Whether the rules let the model read this message for promises: mail or
/// chat, to or from the user, not public, recent, between people. In a
/// group only the user's own words; channels and bulk or automatic mail
/// not at all.
fn worth_reading(store: &Store, item: &Item, since: chrono::DateTime<Utc>) -> bool {
    if item.sensitivity == Level::Public
        || !matches!(item.direction, Direction::Inbound | Direction::Outbound)
        || item.occurred_at.with_timezone(&Utc) < since
    {
        return false;
    }
    match &item.payload {
        Payload::Mail { from, headers, .. } => {
            let bulk = headers.iter().any(|(name, value)| match name.as_str() {
                "list-unsubscribe" | "list-id" | "x-autoreply" => true,
                "precedence" => matches!(
                    value.trim().to_lowercase().as_str(),
                    "bulk" | "list" | "junk"
                ),
                "auto-submitted" => !value.trim().eq_ignore_ascii_case("no"),
                _ => false,
            });
            let from = from.to_lowercase();
            let machine = [
                "noreply",
                "no-reply",
                "donotreply",
                "do-not-reply",
                "notifications@",
                "mailer-daemon",
            ]
            .iter()
            .any(|m| from.contains(m));
            !bulk && !machine
        }
        Payload::Message { .. } => {
            let kind = store
                .get_thread(item.thread_id)
                .ok()
                .flatten()
                .map(|t| t.kind);
            match kind {
                Some(ThreadKind::Channel) => false,
                Some(ThreadKind::GroupChat) => item.direction == Direction::Outbound,
                _ => true,
            }
        }
        _ => false,
    }
}

/// Whether a promise says what will be done and to what: two words at
/// least, or four characters in a script written without spaces.
fn says_enough(what: &str) -> bool {
    let what = what.trim();
    let wide = what.chars().filter(|c| !c.is_ascii()).count();
    what.split_whitespace().count() >= 2 || wide >= 4
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
                version: "2".into(),
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
    use genatrix_model::{Connector, ItemId, Raw, Source, Thread, ThreadId};

    #[test]
    fn a_promise_says_what_and_to_what() {
        assert!(says_enough("send Alice the lease"));
        assert!(says_enough("周五前发报价"));
        assert!(!says_enough("send"));
        assert!(!says_enough("预约"));
        assert!(!says_enough("抓紧找"));
    }

    fn item(
        store: &Store,
        kind: ThreadKind,
        connector: Connector,
        dir: Direction,
        days_ago: i64,
        payload: Payload,
    ) -> Item {
        let ext = ulid::Ulid::new().to_string();
        let source = Source::new(connector, "me", &ext);
        let raw = Raw::describe(source.clone(), "text/plain", ext.as_bytes());
        store.insert_raw(&raw).unwrap();
        let thread = store
            .upsert_thread(&Thread {
                id: ThreadId::new(),
                kind,
                source: Source::new(connector, "me", format!("t{ext}")),
                title: None,
                members: vec![],
                first_at: None,
                last_at: None,
            })
            .unwrap();
        let when = Utc::now() - chrono::Duration::days(days_ago);
        Item {
            id: ItemId::new(),
            source,
            raw_id: raw.id,
            supersedes: None,
            thread_id: thread,
            occurred_at: when.fixed_offset(),
            ingested_at: Utc::now(),
            direction: dir,
            author: None,
            recipients: vec![],
            text: "I will send it on Friday".into(),
            blobs: vec![],
            sensitivity: Level::Personal,
            tombstoned: false,
            payload,
        }
    }

    fn mail(from: &str, headers: Vec<(&str, &str)>) -> Payload {
        Payload::Mail {
            subject: "s".into(),
            from: from.into(),
            to: vec![],
            cc: vec![],
            message_id: None,
            in_reply_to: None,
            references: vec![],
            labels: vec![],
            headers: headers
                .into_iter()
                .map(|(a, b)| (a.to_owned(), b.to_owned()))
                .collect(),
        }
    }

    fn chat() -> Payload {
        Payload::Message {
            reply_to: None,
            forwarded_from: None,
            edited: false,
        }
    }

    #[test]
    fn the_rules_read_recent_words_between_people() {
        use Direction::{Inbound, Outbound};
        use ThreadKind::{Channel, DirectChat, GroupChat, MailThread};
        let store = Store::open_in_memory(&genatrix_keys::DbKey::from_bytes([2; 32])).unwrap();
        let since = Utc::now() - chrono::Duration::days(WINDOW_DAYS);
        let check = |kind, connector, dir, days, payload| {
            let i = item(&store, kind, connector, dir, days, payload);
            worth_reading(&store, &i, since)
        };
        assert!(check(
            MailThread,
            Connector::Imap,
            Inbound,
            2,
            mail("alice@example.com", vec![])
        ));
        assert!(
            !check(
                MailThread,
                Connector::Imap,
                Inbound,
                40,
                mail("alice@example.com", vec![])
            ),
            "too old"
        );
        assert!(!check(
            MailThread,
            Connector::Imap,
            Inbound,
            2,
            mail("a@x.com", vec![("list-unsubscribe", "<mailto:u>")])
        ));
        assert!(!check(
            MailThread,
            Connector::Imap,
            Inbound,
            2,
            mail("a@x.com", vec![("precedence", "bulk")])
        ));
        assert!(!check(
            MailThread,
            Connector::Imap,
            Inbound,
            2,
            mail("Shop <noreply@shop.com>", vec![])
        ));
        assert!(check(DirectChat, Connector::Telegram, Inbound, 2, chat()));
        assert!(check(GroupChat, Connector::Telegram, Outbound, 2, chat()));
        assert!(
            !check(GroupChat, Connector::Telegram, Inbound, 2, chat()),
            "others in a group"
        );
        assert!(!check(Channel, Connector::Telegram, Inbound, 2, chat()));
    }

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
