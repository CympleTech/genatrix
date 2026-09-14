//! Output collars: the shapes a model is allowed to produce.
//!
//! Design: `docs/design/03-agent-layer.md`, "输出收口".
//!
//! The second of the three defences. A classifier may emit one of three
//! words and nothing else; a summary may only cite identifiers that were
//! actually in front of it. A sentence buried in a mail cannot widen that,
//! because widening it is not a thing the shape permits.
//!
//! It is also what makes an eight-billion-parameter model useful. Given one
//! narrow question and a closed set of answers it does well; given an open
//! field it wanders. Every collar here turns an open field into a choice.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::protocol::strip_reasoning;

/// Why a reply did not fit its collar.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CollarError {
    /// The reply was not one of the permitted answers.
    #[error("expected one of [{expected}], got `{got}`")]
    NotAChoice {
        /// The permitted answers.
        expected: String,
        /// What the model said, truncated.
        got: String,
    },
    /// The reply should have been JSON and was not.
    #[error("expected {shape}: {detail}")]
    NotTheShape {
        /// What was expected.
        shape: &'static str,
        /// What went wrong.
        detail: String,
    },
}

/// Read a reply as one of a fixed set of answers.
///
/// Lenient about wrapping, strict about the vocabulary: a model that says
/// "The answer is: secret." has answered, and one that invents a fourth
/// option has not.
pub fn choice<'a, T: Copy>(raw: &str, options: &'a [(&'a str, T)]) -> Result<T, CollarError> {
    let text = strip_reasoning(raw).to_lowercase();
    let mut hit: Option<T> = None;
    for (word, value) in options {
        // Whole-word match, so "public" does not match inside "republic".
        if text
            .match_indices(&word.to_lowercase())
            .any(|(i, w)| is_word_boundary(&text, i, i + w.len()))
        {
            if hit.is_some() {
                return Err(CollarError::NotAChoice {
                    expected: options
                        .iter()
                        .map(|(w, _)| *w)
                        .collect::<Vec<_>>()
                        .join(", "),
                    got: truncate(&text),
                });
            }
            hit = Some(*value);
        }
    }
    hit.ok_or_else(|| CollarError::NotAChoice {
        expected: options
            .iter()
            .map(|(w, _)| *w)
            .collect::<Vec<_>>()
            .join(", "),
        got: truncate(&text),
    })
}

fn is_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    before.is_none_or(|c| !c.is_alphanumeric() && c != '_')
        && after.is_none_or(|c| !c.is_alphanumeric() && c != '_')
}

fn truncate(text: &str) -> String {
    text.chars().take(60).collect()
}

/// One statement in a summary, with what it rests on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    /// The statement.
    pub text: String,
    /// Identifiers it cites. Empty means the interface marks it unsupported.
    pub sources: Vec<String>,
}

impl Point {
    /// Whether anything backs this statement up.
    #[must_use]
    pub fn grounded(&self) -> bool {
        !self.sources.is_empty()
    }
}

/// A summary that has been checked against what the model was shown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// The points that survived.
    pub points: Vec<Point>,
    /// Points dropped because they cited something that was not there, kept
    /// as text so a person can see what the model tried to claim.
    pub dropped: Vec<String>,
}

impl Summary {
    /// Whether every surviving point rests on something.
    #[must_use]
    pub fn fully_grounded(&self) -> bool {
        self.points.iter().all(Point::grounded)
    }
}

#[derive(Deserialize)]
struct RawSummary {
    points: Vec<RawPoint>,
}

#[derive(Deserialize)]
struct RawPoint {
    text: String,
    #[serde(default)]
    sources: Vec<String>,
}

/// Read a summary and check every citation.
///
/// Granularity is the point, not the sentence: design 03 settles this,
/// because a citation after every clause turns prose into a pile of
/// footnotes. A point citing an identifier that was not in the context is
/// dropped whole, not trimmed: the model was talking about something it
/// could not see, so the sentence is not trustworthy either.
pub fn summary(raw: &str, allowed: &[String]) -> Result<Summary, CollarError> {
    let text = strip_reasoning(raw);
    let json = crate::protocol::json_object(text).ok_or(CollarError::NotTheShape {
        shape: "a JSON object with a `points` array",
        detail: "no JSON object in the reply".into(),
    })?;
    let parsed: RawSummary = serde_json::from_str(json).map_err(|e| CollarError::NotTheShape {
        shape: "a JSON object with a `points` array",
        detail: e.to_string(),
    })?;
    let allowed: BTreeSet<&str> = allowed.iter().map(String::as_str).collect();
    let mut out = Summary::default();
    for point in parsed.points {
        let text = point.text.trim().to_owned();
        if text.is_empty() {
            continue;
        }
        if point.sources.iter().any(|s| !allowed.contains(s.as_str())) {
            out.dropped.push(text);
            continue;
        }
        out.points.push(Point {
            text,
            sources: point.sources,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use genatrix_model::Level;

    const LEVELS: &[(&str, Level)] = &[
        ("public", Level::Public),
        ("personal", Level::Personal),
        ("secret", Level::Secret),
    ];

    #[test]
    fn a_bare_word_is_a_choice() {
        assert_eq!(choice("secret", LEVELS).unwrap(), Level::Secret);
        assert_eq!(
            choice("<think>\n\n</think>\n\nPublic", LEVELS).unwrap(),
            Level::Public
        );
        assert_eq!(
            choice("The answer is: personal.", LEVELS).unwrap(),
            Level::Personal
        );
    }

    #[test]
    fn a_word_inside_another_word_is_not_a_choice() {
        assert!(choice("the republic of letters", LEVELS).is_err());
    }

    #[test]
    fn an_invented_answer_is_refused() {
        let err = choice("confidential", LEVELS).unwrap_err();
        assert!(matches!(err, CollarError::NotAChoice { .. }));
        assert!(err.to_string().contains("public, personal, secret"));
    }

    #[test]
    fn hedging_between_two_answers_is_refused() {
        assert!(
            choice("it is either public or personal", LEVELS).is_err(),
            "two answers are no answer"
        );
    }

    #[test]
    fn a_summary_keeps_points_that_cite_what_was_shown() {
        let allowed = vec!["i1".to_owned(), "i2".to_owned()];
        let s = summary(
            r#"{"points":[
                 {"text":"Maria wants milestone two moved by two weeks","sources":["i1"]},
                 {"text":"Support can drop to business hours","sources":["i1","i2"]}
               ]}"#,
            &allowed,
        )
        .unwrap();
        assert_eq!(s.points.len(), 2);
        assert!(s.dropped.is_empty());
        assert!(s.fully_grounded());
    }

    #[test]
    fn a_point_citing_something_that_was_not_there_is_dropped_whole() {
        let allowed = vec!["i1".to_owned()];
        let s = summary(
            r#"{"points":[
                 {"text":"real point","sources":["i1"]},
                 {"text":"invented point","sources":["i9"]}
               ]}"#,
            &allowed,
        )
        .unwrap();
        assert_eq!(s.points.len(), 1);
        assert_eq!(s.points[0].text, "real point");
        assert_eq!(s.dropped, ["invented point"]);
    }

    #[test]
    fn a_point_with_no_citation_survives_but_is_marked() {
        let s = summary(
            r#"{"points":[{"text":"a claim with no basis","sources":[]}]}"#,
            &["i1".to_owned()],
        )
        .unwrap();
        assert_eq!(s.points.len(), 1);
        assert!(!s.points[0].grounded());
        assert!(!s.fully_grounded());
    }

    #[test]
    fn a_summary_that_is_not_the_shape_is_refused() {
        assert!(summary("here are some thoughts", &[]).is_err());
        assert!(summary(r#"{"bullets":[]}"#, &[]).is_err());
    }

    #[test]
    fn empty_points_are_skipped() {
        let s = summary(
            r#"{"points":[{"text":"   ","sources":["i1"]},{"text":"real","sources":["i1"]}]}"#,
            &["i1".to_owned()],
        )
        .unwrap();
        assert_eq!(s.points.len(), 1);
    }
}
