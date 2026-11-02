//! What an executor hands back at the end of each turn.
//!
//! Design: `docs/design/13-executors.md`, "每一轮的交代". An executor ends
//! every turn with one JSON object: either it is done, with a title and a
//! summary, or it needs the user to decide something, with one question and
//! a few options. Like a verdict, this is structured: whether the work waits
//! for the user follows from this object, never from a paragraph of prose.
//!
//! An executor that forgets the object has not asked anything: its whole
//! answer is its summary. Waiting for a question nobody asked would stall
//! the work for nothing; going on without one costs at most a turn.

use serde::{Deserialize, Serialize};

/// The most options a question may offer.
pub const MAX_OPTIONS: usize = 4;

/// The end of one turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "genatrix", rename_all = "snake_case")]
pub enum Report {
    /// The turn's work is done.
    Done {
        /// One line, fit for a commit subject. `None` when it gave none.
        title: Option<String>,
        /// What changed and why, what it decided on the user's behalf,
        /// whether the commands pass.
        summary: String,
    },
    /// The user must decide something before the work can go on.
    Ask(Question),
}

/// A question for the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// The question.
    pub question: String,
    /// Up to [`MAX_OPTIONS`] answers to choose from; the user may also
    /// write their own.
    pub options: Vec<String>,
    /// Which option the executor recommends, by index.
    pub recommend: Option<usize>,
    /// Where the work stands.
    pub progress: String,
}

/// Read the report from an executor's answer: the last JSON object in it
/// with a `"genatrix"` field of `"done"` or `"ask"`. Without one, the turn
/// is done and the whole answer is its summary.
#[must_use]
pub fn read_report(answer: &str) -> Report {
    let mut found: Option<(usize, serde_json::Value)> = None;
    for (start, _) in answer.match_indices('{') {
        let mut stream =
            serde_json::Deserializer::from_str(&answer[start..]).into_iter::<serde_json::Value>();
        if let Some(Ok(value)) = stream.next()
            && matches!(
                value.get("genatrix").and_then(|v| v.as_str()),
                Some("done" | "ask")
            )
        {
            found = Some((start, value));
        }
    }
    let Some((start, value)) = found else {
        return Report::Done {
            title: None,
            summary: answer.trim().to_owned(),
        };
    };
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    // What it said before the object, for when the object says little.
    let before = answer[..start].trim().to_owned();
    if value["genatrix"] == "ask"
        && let Some(question) = text("question")
    {
        let options: Vec<String> = value
            .get("options")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|o| o.as_str().map(str::trim))
                    .filter(|o| !o.is_empty())
                    .take(MAX_OPTIONS)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let recommend = value
            .get("recommend")
            .and_then(serde_json::Value::as_u64)
            .and_then(|i| usize::try_from(i).ok())
            .filter(|i| *i < options.len());
        return Report::Ask(Question {
            question,
            options,
            recommend,
            progress: text("progress").unwrap_or(before),
        });
    }
    Report::Done {
        title: text("title").map(|t| one_line(&t)),
        summary: text("summary").unwrap_or(before),
    }
}

/// The first line, at most 72 characters: a commit subject.
fn one_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() <= 72 {
        line.to_owned()
    } else {
        let cut: String = line.chars().take(71).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_gives_a_title_and_a_summary() {
        let r = read_report(
            "I added the section.\n{\"genatrix\": \"done\", \"title\": \"Add a Rust guide\", \
             \"summary\": \"Added a README section; chose links over inline examples.\"}",
        );
        assert_eq!(
            r,
            Report::Done {
                title: Some("Add a Rust guide".into()),
                summary: "Added a README section; chose links over inline examples.".into(),
            }
        );
    }

    #[test]
    fn a_question_has_options_and_a_recommendation_in_range() {
        let r = read_report(
            "{\"genatrix\": \"ask\", \"question\": \"Which database?\", \
             \"options\": [\"SQLite\", \"Postgres\", \"\", \"a\", \"b\", \"c\"], \
             \"recommend\": 9, \"progress\": \"The models are written.\"}",
        );
        let Report::Ask(q) = r else { panic!("{r:?}") };
        assert_eq!(q.question, "Which database?");
        assert_eq!(q.options, ["SQLite", "Postgres", "a", "b"]);
        assert_eq!(q.recommend, None);
        assert_eq!(q.progress, "The models are written.");
    }

    #[test]
    fn without_a_report_the_turn_is_done_and_says_so_in_its_own_words() {
        assert_eq!(
            read_report("  Changed two files.  "),
            Report::Done {
                title: None,
                summary: "Changed two files.".into()
            }
        );
        // A question with no question is not a question.
        let r = read_report("Done mostly.\n{\"genatrix\": \"ask\", \"options\": [\"x\"]}");
        assert!(matches!(r, Report::Done { ref summary, .. } if summary == "Done mostly."));
    }

    #[test]
    fn the_last_report_counts_and_a_long_title_is_cut() {
        let long = "x".repeat(100);
        let r = read_report(&format!(
            "{{\"genatrix\": \"ask\", \"question\": \"q\"}} then {{\"genatrix\": \"done\", \
             \"title\": \"{long}\\nsecond line\", \"summary\": \"s\"}}"
        ));
        let Report::Done { title, .. } = r else {
            panic!()
        };
        assert_eq!(title.unwrap().chars().count(), 72);
    }
}
