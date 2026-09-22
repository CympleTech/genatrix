//! Drafting a reply, for the user to approve, change, or decline.
//!
//! Design: `docs/design/03-agent-layer.md` (the drafter is an effect tool:
//! it proposes an action and has no way to send) and
//! `docs/design/07-memory-profile.md` (the user's own messages are the
//! style to match; edited drafts are the most expensive signal and are
//! recorded as versions).
//!
//! The model sees the conversation so far, in envelopes, and a few of the
//! user's own recent messages to the same person as the voice to write in.
//! It answers with one JSON object: why a reply is due, and the reply. The
//! reply becomes version one of a pending action; nothing is sent by
//! anyone but a connector, and only after the user approves.

use genatrix_agent::Effect;
use genatrix_agent::envelope::{self, Source as EnvelopeSource};
use genatrix_agent::protocol::{json_object, strip_reasoning};
use genatrix_agent::run::{ModelCaller, RunContext, RunError};
use genatrix_gate::gate::{Initiator, Message, Request};
use genatrix_llm::ticket::Purpose;
use genatrix_model::{Connector, Direction, Item, Level, Payload, PersonId};
use genatrix_store::{ItemQuery, ItemVersion, Store};

use super::store_error;

/// Messages of the conversation the model sees.
const CONTEXT: usize = 12;
/// The user's own recent messages shown as the voice to match.
const VOICE: usize = 5;
/// Characters of each message.
const EXCERPT_CHARS: usize = 700;

const SYSTEM: &str = "/no_think
You draft a reply on behalf of the account holder, called ME, to the last \
message in a conversation. Write as ME would: the messages marked as ME's own \
are the voice to match, in length, tone and language. Answer what was asked; \
do not invent facts, dates or commitments ME has not stated. No subject line, \
no signature unless ME's own messages end with one. Keep it as short as the \
conversation's messages are.

Output exactly one JSON object and nothing else:
{\"rationale\": \"<one sentence addressed to ME as you, on why a reply is due and what it does>\", \"reply\": \"<the reply text>\"}";

/// What the drafter proposes: the effect and the words.
#[derive(Clone, Debug)]
pub struct Drafted {
    /// Where it would go.
    pub effect: Effect,
    /// The reply.
    pub reply: String,
    /// Why, in a sentence.
    pub rationale: String,
    /// What it was drawn from: the message replied to and the context.
    pub evidence: Vec<genatrix_model::ItemId>,
}

/// Why a reply cannot be drafted for an item.
#[derive(Debug, thiserror::Error)]
pub enum DraftError {
    /// Not something one replies to.
    #[error("only mail and chat messages get replies")]
    NotAMessage,
    /// The user's own message.
    #[error("that is the account holder's own message")]
    OwnMessage,
    /// No sender to reply to.
    #[error("the message has no sender to reply to")]
    NoSender,
    /// The model or the store.
    #[error(transparent)]
    Run(#[from] RunError),
}

/// Draft a reply to `item`.
pub async fn reply_to<C: ModelCaller>(
    store: &Store,
    ctx: &mut RunContext<'_, C>,
    item: &Item,
) -> Result<Drafted, DraftError> {
    let effect = effect_for(item)?;
    let me = store
        .self_person()
        .map_err(|e| RunError::Ledger(store_error(&e)))?
        .map(|p| p.id);
    if item.author.is_some() && item.author == me {
        return Err(DraftError::OwnMessage);
    }

    // The conversation so far, oldest first, ending with the item.
    let mut context: Vec<Item> = store
        .query_items(&ItemQuery {
            thread: Some(item.thread_id),
            until: Some(item.occurred_at.with_timezone(&chrono::Utc)),
            version: ItemVersion::Current,
            limit: u32::try_from(CONTEXT).unwrap_or(12),
            ..Default::default()
        })
        .map_err(|e| RunError::Ledger(store_error(&e)))?;
    context.retain(|i| i.id != item.id);
    context.reverse();
    context.push(item.clone());

    // The user's voice: their recent messages to this person, elsewhere too.
    let voice: Vec<Item> = match (me, item.author) {
        (Some(me), Some(them)) => store
            .items_with_person(them, 60)
            .map_err(|e| RunError::Ledger(store_error(&e)))?
            .into_iter()
            .filter(|i| i.author == Some(me) && i.direction == Direction::Outbound)
            .take(VOICE)
            .collect(),
        _ => Vec::new(),
    };

    let mut sources: Vec<EnvelopeSource> = Vec::new();
    for (n, v) in voice.iter().enumerate() {
        sources.push(EnvelopeSource {
            id: format!("voice-{}", n + 1),
            label: format!("ME wrote earlier ({})", v.occurred_at.format("%Y-%m-%d")),
            body: excerpt(v),
        });
    }
    for (n, c) in context.iter().enumerate() {
        let who = if c.author.is_some() && c.author == me {
            "ME".to_owned()
        } else {
            name_of(store, c.author)
        };
        sources.push(EnvelopeSource {
            id: format!("msg-{}", n + 1),
            label: format!("{who}, {}", c.occurred_at.format("%Y-%m-%d %H:%M")),
            body: excerpt(c),
        });
    }
    let zone = envelope::data_zone(&sources);

    let request = Request {
        purpose: Purpose::Draft,
        initiator: Initiator::User,
        items: context.iter().map(|i| i.id).collect(),
        level: Level::Secret,
        identities: vec![],
        messages: vec![
            Message::system(SYSTEM),
            Message::user(format!(
                "{}\nDraft ME's reply to the last message (msg-{}). One JSON object.",
                zone.text,
                context.len()
            )),
        ],
        max_tokens: Some(500),
        temperature: Some(0.3),
        stream: false,
    };
    let (rationale, reply) = ctx
        .ask("draft-reply", &request, |raw| parse(&raw.text))
        .await?;
    Ok(Drafted {
        effect,
        reply,
        rationale,
        evidence: context.iter().map(|i| i.id).collect(),
    })
}

/// Where a reply to this item would go.
fn effect_for(item: &Item) -> Result<Effect, DraftError> {
    match (&item.payload, item.source.connector) {
        (
            Payload::Mail {
                subject,
                from,
                message_id,
                references,
                ..
            },
            Connector::Imap,
        ) => {
            if from.is_empty() {
                return Err(DraftError::NoSender);
            }
            let mut chain = references.clone();
            if let Some(id) = message_id
                && !chain.contains(id)
            {
                chain.push(id.clone());
            }
            let subject = if subject.to_lowercase().starts_with("re:") {
                subject.clone()
            } else {
                format!("Re: {subject}")
            };
            Ok(Effect::SendMail {
                account: item.source.account.clone(),
                to: vec![from.clone()],
                subject,
                in_reply_to: message_id.clone(),
                references: chain,
            })
        }
        (Payload::Message { .. }, Connector::Telegram) => Ok(Effect::SendMessage {
            account: item.source.account.clone(),
            // The thread's key is the conversation: filled in by the caller
            // from the thread, since the item carries only its id.
            chat: String::new(),
            reply_to: Some(item.id),
        }),
        _ => Err(DraftError::NotAMessage),
    }
}

fn excerpt(item: &Item) -> String {
    let mut out = String::new();
    if let Payload::Mail { subject, .. } = &item.payload
        && !subject.is_empty()
    {
        out.push_str("Subject: ");
        out.push_str(subject);
        out.push('\n');
    }
    out.extend(item.text.chars().take(EXCERPT_CHARS));
    out
}

fn name_of(store: &Store, person: Option<PersonId>) -> String {
    person
        .and_then(|p| store.get_person(p).ok().flatten())
        .map_or_else(|| "someone".to_owned(), |p| p.display_name)
}

/// One JSON object with a rationale and a reply, or a refusal.
fn parse(raw: &str) -> Result<(String, String), String> {
    let text = strip_reasoning(raw);
    let object = json_object(text).ok_or("no JSON object in the reply")?;
    let value: serde_json::Value = serde_json::from_str(object).map_err(|e| e.to_string())?;
    let reply = value
        .get("reply")
        .and_then(|r| r.as_str())
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .ok_or("no reply text")?;
    let rationale = value
        .get("rationale")
        .and_then(|r| r.as_str())
        .map_or("", str::trim)
        .to_owned();
    Ok((rationale, reply.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_object_is_read_and_a_missing_reply_refused() {
        let raw = "<think>\n</think>\n```json\n{\"rationale\": \"They asked about Friday.\", \"reply\": \"Friday works, see you at noon.\"}\n```";
        let (why, reply) = parse(raw).unwrap();
        assert_eq!(why, "They asked about Friday.");
        assert_eq!(reply, "Friday works, see you at noon.");
        assert!(parse("{\"rationale\": \"x\"}").is_err());
        assert!(parse("no json here").is_err());
    }
}
