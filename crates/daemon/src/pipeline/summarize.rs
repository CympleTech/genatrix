//! One summary per message that is long enough to need one.
//!
//! Design: `docs/design/03-agent-layer.md` (the ingestion pipeline ends in
//! a summary; a summary cites the items it was drawn from) and
//! `docs/design/06-interface.md` (shown under the text, marked as AI's).
//!
//! A message shorter than [`MIN_CHARS`] is its own summary and gets none;
//! saying the same thing again in fewer words would be noise. Everything
//! longer gets up to three short lines in the language of the message,
//! facts only. The prompt holds the whole body, so the request is secret
//! and stays on this machine; personal content may go to the cloud redacted
//! in phase two, and this is where that decision will be made.

use genatrix_agent::protocol::strip_reasoning;
use genatrix_agent::run::{ModelCaller, RunContext, RunError};
use genatrix_gate::gate::{Initiator, Message, Request};
use genatrix_llm::ticket::Purpose;
use genatrix_model::{Annotation, AnnotationKind, Item, Level, Payload, Producer};
use genatrix_store::Store;

use super::store_error;

/// Below this many characters a message is short enough to read as it is.
pub const MIN_CHARS: usize = 280;
/// Items per pass.
pub const ITEMS_PER_PASS: u32 = 20;
/// How much of a long message the model sees. Enough for the point of it;
/// prompt tokens are the cost (about 150 a second on this hardware).
const BODY_CHARS: usize = 2400;

const SYSTEM: &str = "/no_think
You summarize one message for the person who received it. \
Write at most three short lines, each a fact from the message: what it is, \
what it asks or says, any date, amount or deadline. Use the language the \
message is written in. No advice, no greeting, no commentary, nothing that is \
not in the message. Output the lines and nothing else.";

/// What one pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Items summarized.
    pub summarized: usize,
}

/// Summarize a batch of items that have none yet.
pub async fn run<C: ModelCaller>(
    store: &Store,
    ctx: &mut RunContext<'_, C>,
) -> Result<Report, RunError> {
    let mut report = Report::default();
    let items = store
        .items_without_summary(MIN_CHARS, ITEMS_PER_PASS)
        .map_err(|e| RunError::Ledger(store_error(&e)))?;
    for item in items {
        let text = ask(ctx, &item).await?;
        store
            .insert_annotation(&Annotation::new(
                item.id,
                Producer::Model {
                    model: "local".into(),
                    prompt_version: "summary/1".into(),
                },
                AnnotationKind::Summary {
                    text,
                    sources: vec![item.id],
                },
            ))
            .map_err(|e| RunError::Ledger(store_error(&e)))?;
        report.summarized += 1;
    }
    Ok(report)
}

async fn ask<C: ModelCaller>(ctx: &mut RunContext<'_, C>, item: &Item) -> Result<String, RunError> {
    let mut body = String::new();
    if let Payload::Mail { subject, from, .. } = &item.payload {
        if !from.is_empty() {
            body.push_str("From: ");
            body.push_str(from);
            body.push('\n');
        }
        if !subject.is_empty() {
            body.push_str("Subject: ");
            body.push_str(subject);
            body.push('\n');
        }
    }
    body.push('\n');
    body.extend(item.text.chars().take(BODY_CHARS));
    if item.text.chars().count() > BODY_CHARS {
        body.push_str("\n[…]");
    }

    let request = Request {
        purpose: Purpose::Summarize,
        initiator: Initiator::Rule {
            name: "summarize".into(),
        },
        items: vec![item.id],
        level: Level::Secret,
        identities: vec![],
        messages: vec![Message::system(SYSTEM), Message::user(body)],
        max_tokens: Some(200),
        temperature: Some(0.0),
        stream: false,
    };
    ctx.ask("summarize", &request, |reply| collar(&reply.text))
        .await
}

/// Up to three non-empty lines, bullets removed, or a refusal.
fn collar(raw: &str) -> Result<String, String> {
    let text = strip_reasoning(raw);
    let lines: Vec<String> = text
        .lines()
        .map(|l| {
            l.trim()
                .trim_start_matches(['-', '*', '•', '·'])
                .trim()
                .to_owned()
        })
        .filter(|l| !l.is_empty())
        .take(3)
        .collect();
    if lines.is_empty() {
        return Err("no summary lines".into());
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_collar_keeps_three_clean_lines_and_refuses_nothing_at_all() {
        let raw = "<think>\n\n</think>\n\n- Invoice for September: NZ$ 120.\n- Due 30 September.\n- Pay by bank transfer.\n- Extra line.";
        assert_eq!(
            collar(raw).unwrap(),
            "Invoice for September: NZ$ 120.\nDue 30 September.\nPay by bank transfer."
        );
        assert!(collar("<think></think>\n\n   ").is_err());
    }
}
