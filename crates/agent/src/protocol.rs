//! How a model says "call this tool", and how we read it back.
//!
//! Design: `docs/design/03-agent-layer.md`, "工具协议".
//!
//! The way a model expresses a tool call is each vendor's business and
//! changes with every release. The agent layer must not know. So this is one
//! trait with three implementations, chosen per model from the registry, and
//! everything above treats them alike. Swapping models is a line in the
//! registry, not a change here.
//!
//! `Malformed` is a first-class result, not an error. A small model will
//! sometimes produce a shape we did not ask for; the caller decides whether
//! to retry, to fall back, or to stop. Nothing panics and nothing guesses.

use serde::{Deserialize, Serialize};

/// A tool the model may call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Name the model uses.
    pub name: String,
    /// One line saying what it does.
    pub description: String,
    /// JSON Schema for the arguments.
    pub parameters: serde_json::Value,
}

/// A tool call the model asked for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Which tool.
    pub name: String,
    /// Arguments, unvalidated: the caller checks them against the schema.
    pub arguments: serde_json::Value,
}

/// What came back from a model, before interpretation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawReply {
    /// The assistant's text, exactly as returned.
    pub text: String,
    /// Tool calls the provider parsed for us, when it has that concept.
    pub tool_calls: Vec<ToolCall>,
}

impl RawReply {
    /// A text-only reply.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tool_calls: Vec::new(),
        }
    }
}

/// What the model meant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    /// Call these tools.
    Tools(Vec<ToolCall>),
    /// This is the answer.
    Answer(String),
    /// The reply did not have a shape we can act on.
    Malformed {
        /// What was wrong, for the run record.
        reason: String,
        /// The reply, so a person can see what happened.
        raw: String,
    },
}

/// Strip a reasoning block from a reply.
///
/// Learned the hard way in `docs/plan/spikes/02-local-model.md`: Qwen3 emits
/// an empty `<think></think>` even with thinking switched off, and a parser
/// that did not know scored zero out of fifty. Never trust a model to leave
/// scaffolding out; take it off here, once, for every protocol.
#[must_use]
pub fn strip_reasoning(text: &str) -> &str {
    const CLOSERS: [&str; 3] = ["</think>", "</thinking>", "</reasoning>"];
    let mut rest = text;
    for closer in CLOSERS {
        if let Some(i) = rest.rfind(closer) {
            rest = &rest[i + closer.len()..];
        }
    }
    rest.trim()
}

/// One way of asking for and reading tool calls.
pub trait ToolProtocol: Send + Sync {
    /// Text to append to the instruction zone describing the tools. Empty
    /// when the provider carries tool definitions out of band.
    fn instructions(&self, tools: &[ToolSpec]) -> String;

    /// Whether tool definitions should be sent in the provider's own field.
    fn sends_tool_schema(&self) -> bool;

    /// Read a reply.
    fn interpret(&self, reply: &RawReply) -> Intent;
}

/// The provider parses tool calls for us.
#[derive(Clone, Copy, Debug, Default)]
pub struct Native;

impl ToolProtocol for Native {
    fn instructions(&self, _tools: &[ToolSpec]) -> String {
        String::new()
    }

    fn sends_tool_schema(&self) -> bool {
        true
    }

    fn interpret(&self, reply: &RawReply) -> Intent {
        if reply.tool_calls.is_empty() {
            Intent::Answer(strip_reasoning(&reply.text).to_owned())
        } else {
            Intent::Tools(reply.tool_calls.clone())
        }
    }
}

/// The model writes a JSON object; we parse it.
///
/// For models that hold a shape well but have no tool interface, and as the
/// thing every other protocol degrades to.
#[derive(Clone, Copy, Debug, Default)]
pub struct Structured;

impl ToolProtocol for Structured {
    fn instructions(&self, tools: &[ToolSpec]) -> String {
        if tools.is_empty() {
            return String::new();
        }
        let list = tools
            .iter()
            .map(|t| {
                format!(
                    "- {}: {}\n  arguments: {}",
                    t.name, t.description, t.parameters
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "You may use these tools:\n{list}\n\n\
             Reply with one JSON object and nothing else. To use a tool:\n\
             {{\"tool\": \"<name>\", \"arguments\": {{...}}}}\n\
             To answer:\n{{\"answer\": \"<text>\"}}"
        )
    }

    fn sends_tool_schema(&self) -> bool {
        false
    }

    fn interpret(&self, reply: &RawReply) -> Intent {
        let text = strip_reasoning(&reply.text);
        let Some(json) = json_object(text) else {
            return Intent::Malformed {
                reason: "no JSON object in the reply".into(),
                raw: text.to_owned(),
            };
        };
        let value: serde_json::Value = match serde_json::from_str(json) {
            Ok(v) => v,
            Err(e) => {
                return Intent::Malformed {
                    reason: format!("not valid JSON: {e}"),
                    raw: text.to_owned(),
                };
            }
        };
        if let Some(answer) = value.get("answer").and_then(|v| v.as_str()) {
            return Intent::Answer(answer.to_owned());
        }
        match value.get("tool").and_then(|v| v.as_str()) {
            Some(name) => Intent::Tools(vec![ToolCall {
                name: name.to_owned(),
                arguments: value
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
            }]),
            None => Intent::Malformed {
                reason: "object has neither `answer` nor `tool`".into(),
                raw: text.to_owned(),
            },
        }
    }
}

/// Decoding is constrained to a schema, so the shape is guaranteed.
///
/// The local process can do this; it is the most reliable of the three and
/// the reason a small model can be trusted with a narrow task. Until the
/// inference process exposes the knob it behaves exactly like `Structured`,
/// which is the honest degradation: same contract, weaker guarantee.
#[derive(Clone, Copy, Debug, Default)]
pub struct Constrained;

impl ToolProtocol for Constrained {
    fn instructions(&self, tools: &[ToolSpec]) -> String {
        Structured.instructions(tools)
    }

    fn sends_tool_schema(&self) -> bool {
        false
    }

    fn interpret(&self, reply: &RawReply) -> Intent {
        Structured.interpret(reply)
    }
}

/// Find the outermost JSON object in a reply, tolerating prose and fences
/// around it. Returns `None` when the braces do not balance.
#[must_use]
pub fn json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ToolSpec {
        ToolSpec {
            name: "search_items".into(),
            description: "search the timeline".into(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    #[test]
    fn reasoning_blocks_are_stripped() {
        assert_eq!(strip_reasoning("<think>\n\n</think>\n\nsecret"), "secret");
        assert_eq!(strip_reasoning("<think>a</think>b<think>c</think>d"), "d");
        assert_eq!(strip_reasoning("plain answer"), "plain answer");
        assert_eq!(strip_reasoning("  spaced  "), "spaced");
    }

    #[test]
    fn native_reads_the_providers_tool_calls() {
        let reply = RawReply {
            text: "<think></think>".into(),
            tool_calls: vec![ToolCall {
                name: "search_items".into(),
                arguments: serde_json::json!({"q": "friday"}),
            }],
        };
        match Native.interpret(&reply) {
            Intent::Tools(calls) => assert_eq!(calls[0].name, "search_items"),
            other => panic!("{other:?}"),
        }
        assert!(Native.instructions(&[spec()]).is_empty());
        assert!(Native.sends_tool_schema());
    }

    #[test]
    fn structured_reads_a_json_object() {
        match Structured.interpret(&RawReply::text(
            r#"<think></think>{"tool":"search_items","arguments":{"q":"friday"}}"#,
        )) {
            Intent::Tools(calls) => {
                assert_eq!(calls[0].name, "search_items");
                assert_eq!(calls[0].arguments["q"], "friday");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            Structured.interpret(&RawReply::text(r#"{"answer":"done"}"#)),
            Intent::Answer("done".into())
        );
    }

    #[test]
    fn structured_finds_the_object_inside_prose_and_fences() {
        let reply = RawReply::text(
            "Sure, here you go:\n```json\n{\"answer\": \"it is {nested} fine\"}\n```\nhope that helps",
        );
        assert_eq!(
            Structured.interpret(&reply),
            Intent::Answer("it is {nested} fine".into())
        );
    }

    #[test]
    fn a_shape_we_cannot_act_on_is_malformed_not_an_error() {
        for raw in [
            "I am not going to answer in JSON.",
            r#"{"thoughts": "hmm"}"#,
            r#"{"answer": "unterminated"#,
        ] {
            assert!(
                matches!(
                    Structured.interpret(&RawReply::text(raw)),
                    Intent::Malformed { .. }
                ),
                "should be malformed: {raw}"
            );
        }
    }

    #[test]
    fn the_malformed_result_keeps_the_reply_so_a_person_can_look() {
        match Structured.interpret(&RawReply::text("nope")) {
            Intent::Malformed { raw, reason } => {
                assert_eq!(raw, "nope");
                assert!(!reason.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn every_protocol_answers_the_same_questions() {
        let protocols: [&dyn ToolProtocol; 3] = [&Native, &Structured, &Constrained];
        for p in protocols {
            let reply = RawReply::text(r#"{"answer":"ok"}"#);
            let intent = p.interpret(&reply);
            assert!(
                matches!(intent, Intent::Answer(_) | Intent::Malformed { .. }),
                "protocols agree on the shape of the result"
            );
            let _ = p.instructions(&[spec()]);
            let _ = p.sends_tool_schema();
        }
    }
}
