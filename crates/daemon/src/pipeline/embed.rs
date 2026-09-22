//! Chunk and vectorize what has arrived, so it can be searched by meaning.
//!
//! Design: `docs/design/03-agent-layer.md` (the ingestion pipeline: judge,
//! chunk, vectorize, summarize) and `docs/design/01-data-model.md` (an
//! Embedding annotation per chunk; chunking belongs to the producer).
//!
//! Chunks are cut at paragraph and sentence boundaries to about
//! [`CHUNK_CHARS`] characters, with the subject in front of the first, and
//! carry e5's `passage:` prefix; the query side adds `query:`. Byte ranges
//! into the item's text are kept so a hit can be shown in place later.
//!
//! The embedder reads whole bodies, so every request is marked secret and
//! resolved to a local model; the registry refuses an `embed` purpose on a
//! cloud model at load, so there is no path out.

use genatrix_agent::run::{ModelCaller, RunContext, RunError};
use genatrix_gate::gate::{Initiator, Message, Request};
use genatrix_llm::ticket::Purpose;
use genatrix_model::{Item, Level, Payload, Producer};
use genatrix_store::Store;

use super::store_error;

/// Target chunk length in characters.
pub const CHUNK_CHARS: usize = 700;
/// Texts per embeddings call.
pub const BATCH: usize = 32;
/// Items per pass.
pub const ITEMS_PER_PASS: u32 = 40;

/// The producer every chunk vector is filed under. The chunking rule is
/// part of it: a different cut is a different producer.
fn producer() -> Producer {
    Producer::Model {
        model: "embed".into(),
        prompt_version: "chunk/1".into(),
    }
}

/// What one pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Items vectorized.
    pub items: usize,
    /// Chunks vectorized.
    pub chunks: usize,
}

/// One chunk of one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// Chunk index within the item.
    pub idx: u32,
    /// Byte range in the item's text.
    pub range: (u32, u32),
    /// What the embedder sees: the prefix, the subject for the first chunk,
    /// the text.
    pub passage: String,
}

/// Cut an item's text into chunks.
#[must_use]
pub fn chunks_of(item: &Item) -> Vec<Chunk> {
    let text = item.text.as_str();
    let subject = match &item.payload {
        Payload::Mail { subject, .. } if !subject.trim().is_empty() => Some(subject.trim()),
        _ => None,
    };
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut idx = 0u32;
    let total = text.len();
    if text.trim().is_empty() {
        // Nothing but a subject, or nothing at all.
        if let Some(subject) = subject {
            out.push(Chunk {
                idx: 0,
                range: (0, 0),
                passage: format!("passage: {subject}"),
            });
        }
        return out;
    }
    while start < total {
        let end = cut_at(text, start, CHUNK_CHARS);
        let piece = text[start..end].trim();
        if !piece.is_empty() {
            let passage = match (idx, subject) {
                (0, Some(subject)) => format!("passage: {subject}\n{piece}"),
                _ => format!("passage: {piece}"),
            };
            out.push(Chunk {
                idx,
                range: (
                    u32::try_from(start).unwrap_or(u32::MAX),
                    u32::try_from(end).unwrap_or(u32::MAX),
                ),
                passage,
            });
            idx += 1;
        }
        start = end;
    }
    out
}

/// The byte offset to end a chunk at: after about `want` characters, at the
/// last paragraph break if there is one, else the last sentence end, else
/// the last space, else a character boundary.
fn cut_at(text: &str, start: usize, want: usize) -> usize {
    let rest = &text[start..];
    let mut chars = rest.char_indices();
    let Some((limit, _)) = chars.nth(want) else {
        return text.len();
    };
    let window = &rest[..limit];
    let boundary = window
        .rfind("\n\n")
        .map(|i| i + 2)
        .or_else(|| {
            window
                .rfind(['。', '！', '？', '.', '!', '?', '\n'])
                .map(|i| i + window[i..].chars().next().map_or(1, char::len_utf8))
        })
        .or_else(|| window.rfind(' ').map(|i| i + 1))
        .filter(|i| *i > want / 3)
        .unwrap_or(limit);
    start + boundary
}

/// Vectorize a batch of items that have no vectors yet.
pub async fn run<C: ModelCaller>(
    store: &Store,
    ctx: &mut RunContext<'_, C>,
) -> Result<Report, RunError> {
    let mut report = Report::default();
    let items = store
        .items_without_embedding(ITEMS_PER_PASS)
        .map_err(|e| RunError::Ledger(store_error(&e)))?;

    let mut pending: Vec<(Item, Chunk)> = Vec::new();
    for item in items {
        let mut chunks = chunks_of(&item);
        if chunks.is_empty() {
            // Nothing to embed; mark it so it is not picked up forever.
            chunks.push(Chunk {
                idx: 0,
                range: (0, 0),
                passage: "passage: (empty message)".into(),
            });
        }
        for chunk in chunks {
            pending.push((item.clone(), chunk));
        }
    }

    for batch in pending.chunks(BATCH) {
        let vectors = ask(ctx, batch).await?;
        for ((item, chunk), vector) in batch.iter().zip(vectors) {
            store
                .put_embedding(item.id, chunk.idx, chunk.range, &vector, &producer())
                .map_err(|e| RunError::Ledger(store_error(&e)))?;
            report.chunks += 1;
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for (item, _) in &pending {
        seen.insert(item.id);
    }
    report.items = seen.len();
    Ok(report)
}

/// Embed one text the user typed, for search. `None` when the model has no
/// answer for it.
pub async fn embed_query<C: ModelCaller>(
    ctx: &mut RunContext<'_, C>,
    text: &str,
    level: Level,
) -> Result<Vec<f32>, RunError> {
    let request = Request {
        purpose: Purpose::Embed,
        initiator: Initiator::User,
        items: vec![],
        level,
        identities: vec![],
        messages: vec![Message::user(format!("query: {}", text.trim()))],
        max_tokens: None,
        temperature: None,
        stream: false,
    };
    let mut vectors = ctx
        .ask("embed-query", &request, |reply| {
            parse_vectors(&reply.text, 1)
        })
        .await?;
    Ok(vectors.pop().unwrap_or_default())
}

async fn ask<C: ModelCaller>(
    ctx: &mut RunContext<'_, C>,
    batch: &[(Item, Chunk)],
) -> Result<Vec<Vec<f32>>, RunError> {
    let expected = batch.len();
    let request = Request {
        purpose: Purpose::Embed,
        initiator: Initiator::Rule {
            name: "embed".into(),
        },
        items: batch.iter().map(|(i, _)| i.id).collect(),
        level: Level::Secret,
        identities: vec![],
        messages: batch
            .iter()
            .map(|(_, c)| Message::user(c.passage.clone()))
            .collect(),
        max_tokens: None,
        temperature: None,
        stream: false,
    };
    ctx.ask("embed-batch", &request, move |reply| {
        parse_vectors(&reply.text, expected)
    })
    .await
}

/// Read an OpenAI-style embeddings reply: `data[i].embedding`, in order.
fn parse_vectors(raw: &str, expected: usize) -> Result<Vec<Vec<f32>>, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("not JSON: {e}"))?;
    let data = value
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| "no `data` array".to_owned())?;
    if data.len() != expected {
        return Err(format!("{} vectors for {expected} texts", data.len()));
    }
    let mut out: Vec<(usize, Vec<f32>)> = Vec::with_capacity(expected);
    for (position, entry) in data.iter().enumerate() {
        let index = entry
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .map_or(position, |i| usize::try_from(i).unwrap_or(position));
        let vector: Vec<f32> = entry
            .get("embedding")
            .and_then(|e| e.as_array())
            .ok_or_else(|| "an entry without an embedding".to_owned())?
            .iter()
            .map(|v| {
                #[expect(clippy::cast_possible_truncation, reason = "f32 is the vector's type")]
                v.as_f64().map(|f| f as f32)
            })
            .collect::<Option<_>>()
            .ok_or_else(|| "a non-numeric vector".to_owned())?;
        if vector.len() != genatrix_store::EMBEDDING_DIMS {
            return Err(format!(
                "{} dimensions, expected {}",
                vector.len(),
                genatrix_store::EMBEDDING_DIMS
            ));
        }
        out.push((index, vector));
    }
    out.sort_by_key(|(i, _)| *i);
    Ok(out.into_iter().map(|(_, v)| v).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use genatrix_model::{Connector, Direction, ItemId, RawId, Source, ThreadId};

    fn item(subject: &str, text: &str) -> Item {
        Item {
            id: ItemId::new(),
            source: Source::new(Connector::Imap, "me", "x"),
            raw_id: RawId::new(),
            supersedes: None,
            thread_id: ThreadId::new(),
            occurred_at: chrono::Utc::now().into(),
            ingested_at: chrono::Utc::now(),
            direction: Direction::Inbound,
            author: None,
            recipients: vec![],
            text: text.to_owned(),
            blobs: vec![],
            sensitivity: Level::Personal,
            tombstoned: false,
            payload: Payload::Mail {
                subject: subject.to_owned(),
                from: String::new(),
                to: vec![],
                cc: vec![],
                message_id: None,
                in_reply_to: None,
                references: vec![],
                labels: vec![],
                headers: vec![],
            },
        }
    }

    #[test]
    fn a_short_message_is_one_chunk_with_its_subject_in_front() {
        let chunks = chunks_of(&item("lunch", "Friday works for me."));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].passage, "passage: lunch\nFriday works for me.");
        assert_eq!(chunks[0].range, (0, 20));
    }

    #[test]
    fn a_long_message_is_cut_at_paragraphs_and_covers_every_byte() {
        let paragraph = "第一段很长很长的中文句子，说了很多事情。".repeat(40);
        let text = format!("{paragraph}\n\n{paragraph}\n\nfinal words here.");
        let it = item("s", &text);
        let chunks = chunks_of(&it);
        assert!(chunks.len() >= 3, "{}", chunks.len());
        assert_eq!(chunks[0].range.0, 0);
        for pair in chunks.windows(2) {
            assert_eq!(pair[0].range.1, pair[1].range.0, "no gap, no overlap");
        }
        assert_eq!(
            chunks.last().unwrap().range.1,
            u32::try_from(text.len()).unwrap()
        );
        for c in &chunks {
            assert!(
                c.passage.chars().count() <= CHUNK_CHARS + 40,
                "{}",
                c.passage.len()
            );
        }
        assert!(
            chunks[1].passage.starts_with("passage: 第一段"),
            "no subject after the first"
        );
    }

    #[test]
    fn an_empty_body_still_embeds_its_subject() {
        let chunks = chunks_of(&item("Invoice 42", "   "));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].passage, "passage: Invoice 42");
    }

    #[test]
    fn vectors_come_back_in_input_order_whatever_order_they_arrive_in() {
        let dims = genatrix_store::EMBEDDING_DIMS;
        let v = |x: f32| vec![x; dims];
        let raw = serde_json::json!({
            "data": [
                { "index": 1, "embedding": v(1.0) },
                { "index": 0, "embedding": v(0.0) },
            ]
        })
        .to_string();
        let out = parse_vectors(&raw, 2).unwrap();
        assert!(out[0][0].abs() < f32::EPSILON);
        assert!((out[1][0] - 1.0).abs() < f32::EPSILON);
        assert!(parse_vectors(&raw, 3).is_err(), "a short answer is refused");
        assert!(parse_vectors("{\"data\":[{\"index\":0,\"embedding\":[1,2]}]}", 1).is_err());
    }
}
