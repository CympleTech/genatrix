//! Sensitivity classification, the first pipeline.
//!
//! Design: `docs/design/02-trust-boundary.md` for what the levels mean and
//! who may move them; `docs/design/04-model-layer.md` for why this batches.
//!
//! Rules run first and settle most items on their own. What is left goes to
//! the local model, ten at a time. That number is not a guess: the spike in
//! `docs/plan/spikes/02-local-model.md` measured one item per call at 1.16
//! seconds and 78 per cent agreement, and ten per call at 356 milliseconds
//! each and 92 per cent. Batching is faster *and* more accurate, because
//! seeing ten messages side by side gives the model contrast that an isolated
//! one does not.
//!
//! The model can only raise a level. It never lowers one, and it is never
//! asked about an item the rules already called secret, because there is
//! nowhere higher to go.

use std::collections::BTreeMap;

use genatrix_agent::envelope::{self, Source as EnvelopeSource};
use genatrix_agent::run::{ModelCaller, RunContext, RunError};
use genatrix_gate::gate::{Initiator, Message, Request};
use genatrix_gate::rules::{Candidate, RuleSet};
use genatrix_llm::ticket::Purpose;
use genatrix_model::Payload;
use genatrix_model::{
    Annotation, AnnotationKind, Item, Level, Producer, annotation::effective_level,
};
use genatrix_store::{ItemQuery, Store};

/// How many items go into one model call.
pub const BATCH: usize = 10;

/// What one run of the pipeline did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Items looked at.
    pub seen: usize,
    /// Items the rules settled without asking the model.
    pub by_rule: usize,
    /// Items the model was asked about.
    pub asked: usize,
    /// Items the model raised above the rules' floor.
    pub raised: usize,
    /// How many ended at each level.
    pub levels: BTreeMap<String, usize>,
}

/// `/no_think` is Qwen3's soft switch for its reasoning block. It is not a
/// good mechanism: it costs tokens, and the model still emits an empty block.
/// The right lever is the chat template's `enable_thinking`, which the local
/// inference process does not pass through yet. Until it does, the switch
/// goes here and the token budget below leaves room for a stray block.
///
/// Leaving it out is not a small mistake. The first run of this pipeline did,
/// and the model spent its whole budget reasoning and produced no labels at
/// all, which is the same failure the spike hit for the same reason.
const SYSTEM: &str = "/no_think
You classify messages for someone's personal assistant. \
For each numbered message reply with its label.

public = newsletters, marketing, mass notifications, public channel posts
personal = private communication between specific people; use this when unsure
secret = credentials, verification codes, bank or card or account numbers, \
identity numbers, health or medical details, legal matters, salary or \
detailed personal finances

Output exactly one line per message, in the form `N: label`, and nothing else.";

/// Classify every item that still needs it.
///
/// Resumable and idempotent, which matters because the first attempt at this
/// pipeline failed halfway: the rules had already written their judgements,
/// so a naive "has any judgement yet" test skipped every item forever
/// afterwards. An item needs work until it has a model judgement, or until
/// the rules have called it secret and there is nowhere higher to go.
pub async fn run<C: ModelCaller>(
    store: &Store,
    rules: &RuleSet,
    ctx: &mut RunContext<'_, C>,
) -> Result<Report, RunError> {
    let mut report = Report::default();
    let mut pending: Vec<(Item, Level)> = Vec::new();

    for item in all_items(store)? {
        let judged = judgements(store, &item)?;
        let by_model = judged
            .iter()
            .any(|(producer, _)| matches!(producer, Producer::Model { .. }));

        // The rules are deterministic, so their verdict is computed once and
        // reused; re-running them every pass would pile up identical
        // annotations saying the same thing.
        let existing_rule = judged
            .iter()
            .find(|(producer, _)| matches!(producer, Producer::Rule { .. }))
            .map(|(_, level)| *level);
        let floor = if let Some(level) = existing_rule {
            level
        } else {
            let headers = headers_of(&item);
            let domain = sender_domain_of(&item);
            let judgement = rules.judge(&Candidate {
                connector: item.source.connector,
                thread_kind: thread_kind_of(store, &item),
                text: &item.text,
                headers: &headers,
                sender_domain: domain.as_deref(),
            });
            record(
                store,
                &item,
                judgement.level,
                Producer::Rule {
                    rule: judgement.reason(),
                    version: rules.version.clone(),
                },
            )?;
            judgement.level
        };

        if floor == Level::Secret {
            // Nowhere higher to go; asking would spend a call to learn
            // nothing. Only count it the first time, so a second run
            // reports honestly that it had nothing to do.
            if existing_rule.is_none() {
                report.seen += 1;
                report.by_rule += 1;
                settle(store, &item, &mut report)?;
            }
        } else if !by_model {
            report.seen += 1;
            pending.push((item, floor));
        }
    }

    for chunk in pending.chunks(BATCH) {
        let labels = ask(ctx, chunk).await?;
        for (offset, (item, floor)) in chunk.iter().enumerate() {
            report.asked += 1;
            if let Some(level) = labels.get(&(offset + 1)) {
                if *level > *floor {
                    report.raised += 1;
                }
                record(
                    store,
                    item,
                    floor.escalate(*level),
                    Producer::Model {
                        model: "local".into(),
                        prompt_version: "classify/1".into(),
                    },
                )?;
            }
            settle(store, item, &mut report)?;
        }
    }

    Ok(report)
}

/// Ask about one batch and read back the labels by position.
async fn ask<C: ModelCaller>(
    ctx: &mut RunContext<'_, C>,
    chunk: &[(Item, Level)],
) -> Result<BTreeMap<usize, Level>, RunError> {
    let sources: Vec<EnvelopeSource> = chunk
        .iter()
        .enumerate()
        .map(|(i, (item, _))| EnvelopeSource {
            id: (i + 1).to_string(),
            label: format!("message {}", i + 1),
            body: excerpt(item),
        })
        .collect();
    let zone = envelope::data_zone(&sources);
    let expected = chunk.len();

    let request = Request {
        purpose: Purpose::Classify,
        initiator: Initiator::Rule {
            name: "classify".into(),
        },
        items: chunk.iter().map(|(i, _)| i.id).collect(),
        // The prompt holds whole message bodies, so it is as sensitive as the
        // most sensitive of them. Classify never leaves the device anyway,
        // but the record should say what it carried.
        level: Level::Secret,
        identities: vec![],
        messages: vec![
            Message::system(SYSTEM),
            Message::user(format!(
                "{}\nNow give one line per message, `N: label`, for messages 1 to {expected}.",
                zone.text
            )),
        ],
        // Room for the labels, plus room for a reasoning block the switch
        // above was supposed to prevent.
        max_tokens: Some(64 * u32::try_from(expected).unwrap_or(10) + 256),
        temperature: Some(0.0),
        stream: false,
    };

    ctx.ask("classify-batch", &request, move |reply| {
        parse_labels(&reply.text, expected)
    })
    .await
}

/// How much of a message the model sees. Design 04 gives the classifier a
/// budget of a few hundred milliseconds per item, and on this hardware the
/// model reads about 160 prompt tokens a second, so a batch of ten can carry
/// roughly 800 tokens in all: a few dozen per message. Whole bodies of
/// marketing mail run to thousands of tokens each and blew that budget by
/// two orders of magnitude on the first real mailbox. The rules have already
/// read the whole text for the patterns that matter most; the model's
/// question is coarser, and who wrote it, what it is about and how it opens
/// answer it. Links go: they are the most token-dense and least telling
/// part of a message.
const EXCERPT_CHARS: usize = 200;

/// Sender, subject and the opening of the text, without links.
fn excerpt(item: &Item) -> String {
    let mut out = String::new();
    if let Payload::Mail { subject, from, .. } = &item.payload {
        if !from.is_empty() {
            out.push_str("From: ");
            out.push_str(from);
            out.push('\n');
        }
        if !subject.is_empty() {
            out.push_str("Subject: ");
            out.push_str(subject);
            out.push('\n');
        }
    }
    let without_links = without_links(&item.text);
    let body: String = without_links.chars().take(EXCERPT_CHARS).collect();
    out.push_str(body.trim());
    if without_links.chars().count() > EXCERPT_CHARS {
        out.push_str(" …");
    }
    out
}

/// The text with every `http(s)://…` run removed and whitespace collapsed.
fn without_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("http") {
        let (before, tail) = rest.split_at(at);
        if tail.starts_with("http://") || tail.starts_with("https://") {
            out.push_str(before);
            let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
            rest = &tail[end..];
        } else {
            out.push_str(before);
            out.push_str("http");
            rest = &tail[4..];
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Read `N: label` lines. Missing lines are not invented: an item the model
/// did not answer for keeps the rules' floor.
fn parse_labels(raw: &str, expected: usize) -> Result<BTreeMap<usize, Level>, String> {
    let text = genatrix_agent::protocol::strip_reasoning(raw);
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let Some((number, label)) = line.split_once(':') else {
            continue;
        };
        let Ok(n) = number
            .trim()
            .trim_start_matches(['-', '*', ' '])
            .parse::<usize>()
        else {
            continue;
        };
        if n == 0 || n > expected {
            continue;
        }
        let word = label
            .trim()
            .trim_matches(['.', '*', '`', '"'])
            .to_lowercase();
        let level = match word.as_str() {
            "public" => Level::Public,
            "personal" => Level::Personal,
            "secret" => Level::Secret,
            _ => continue,
        };
        out.insert(n, level);
    }
    if out.is_empty() {
        return Err(format!(
            "no `N: label` lines in the reply: {}",
            text.chars().take(120).collect::<String>()
        ));
    }
    Ok(out)
}

/// Write a judgement down as an annotation. The item's own field is only a
/// cache of what these add up to.
fn record(store: &Store, item: &Item, level: Level, producer: Producer) -> Result<(), RunError> {
    let annotation = Annotation::new(
        item.id,
        producer,
        AnnotationKind::Sensitivity {
            level,
            reason: String::new(),
        },
    );
    store
        .insert_annotation(&annotation)
        .map_err(|e| RunError::Ledger(store_error(&e)))?;
    Ok(())
}

/// Refresh the cached level from every judgement about an item.
fn settle(store: &Store, item: &Item, report: &mut Report) -> Result<(), RunError> {
    let annotations = store
        .annotations_of(item.id)
        .map_err(|e| RunError::Ledger(store_error(&e)))?;
    let level = effective_level(&annotations);
    store
        .set_item_sensitivity(item.id, level)
        .map_err(|e| RunError::Ledger(store_error(&e)))?;
    *report.levels.entry(level.as_str().to_owned()).or_default() += 1;
    Ok(())
}

/// The headers the rules read, from the item itself.
///
/// They travel on the item rather than being looked up somewhere, so a
/// message that came from a connector and one that came from anywhere else
/// are judged by exactly the same evidence.
fn headers_of(item: &Item) -> Vec<(String, String)> {
    match &item.payload {
        genatrix_model::Payload::Mail { headers, .. } => headers.clone(),
        _ => Vec::new(),
    }
}

/// The sender's domain, for the sender rules.
fn sender_domain_of(item: &Item) -> Option<String> {
    match &item.payload {
        genatrix_model::Payload::Mail { from, .. } => from
            .rsplit_once('@')
            .map(|(_, domain)| domain.trim_end_matches('>').trim().to_lowercase()),
        _ => None,
    }
}

fn all_items(store: &Store) -> Result<Vec<Item>, RunError> {
    store
        .query_items(&ItemQuery {
            limit: 10_000,
            ..Default::default()
        })
        .map_err(|e| RunError::Ledger(store_error(&e)))
}

/// Every sensitivity judgement about an item, with who made it.
fn judgements(store: &Store, item: &Item) -> Result<Vec<(Producer, Level)>, RunError> {
    Ok(store
        .annotations_of(item.id)
        .map_err(|e| RunError::Ledger(store_error(&e)))?
        .into_iter()
        .filter_map(|a| match a.kind {
            AnnotationKind::Sensitivity { level, .. } => Some((a.producer, level)),
            _ => None,
        })
        .collect())
}

fn thread_kind_of(store: &Store, item: &Item) -> genatrix_model::ThreadKind {
    store
        .get_thread(item.thread_id)
        .ok()
        .flatten()
        .map_or(genatrix_model::ThreadKind::MailThread, |t| t.kind)
}

/// The run context reports failures as ledger errors; a store failure during
/// a pipeline is the same kind of "we cannot continue safely".
fn store_error(e: &genatrix_store::Error) -> genatrix_ledger::Error {
    genatrix_ledger::Error::HeadMismatch(format!("store: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_go_and_the_opening_stays() {
        let text = "Sale ends midnight ( https://click.example/u/?qs=abc123 ) View in browser https://x.example/y\nKia ora";
        assert_eq!(
            without_links(text),
            "Sale ends midnight ( ) View in browser Kia ora"
        );
        assert_eq!(without_links("an http proxy"), "an http proxy");
    }

    #[test]
    fn labels_are_read_by_position() {
        let labels = parse_labels("1: public\n2: secret\n3: personal", 3).unwrap();
        assert_eq!(labels[&1], Level::Public);
        assert_eq!(labels[&2], Level::Secret);
        assert_eq!(labels[&3], Level::Personal);
    }

    #[test]
    fn decoration_and_reasoning_blocks_are_tolerated() {
        let labels = parse_labels(
            "<think>\n\n</think>\n- 1: **public**\n- 2: `secret`.\nHope that helps!",
            2,
        )
        .unwrap();
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[&2], Level::Secret);
    }

    #[test]
    fn an_invented_label_or_number_is_dropped_not_guessed() {
        let labels = parse_labels("1: confidential\n2: personal\n9: secret", 2).unwrap();
        assert_eq!(labels.len(), 1, "only the usable line survives");
        assert_eq!(labels[&2], Level::Personal);
    }

    #[test]
    fn a_reply_with_no_usable_line_is_malformed() {
        assert!(parse_labels("I would rather not say.", 3).is_err());
    }
}
