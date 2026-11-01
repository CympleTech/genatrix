//! Given a recipe and what has happened, what happens next.
//!
//! Design: `docs/design/13-executors.md`, "流程模板": the next step follows
//! from a structured verdict, never from a paragraph of prose. A verdict
//! that cannot be read stops the work; it does not count as a pass
//! (design 03: malformed is a result, not a crash, and not a yes).

use serde::{Deserialize, Serialize};

use crate::recipe::Recipe;

/// A reviewer's judgement, in the one shape it is asked for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    /// Whether the work passes.
    pub pass: bool,
    /// What should change, for the next step.
    pub notes: String,
}

/// One step that has finished.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    /// The step's id.
    pub step: String,
    /// The verdict, for a verdict step; `None` when it could not be read.
    pub verdict: Option<Verdict>,
}

/// What to do now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    /// Run this step. `round` counts from 1 inside the loop.
    Run {
        /// Index into the recipe's steps.
        step: usize,
        /// Which time round the loop this is; 1 outside it.
        round: u32,
    },
    /// Every step is done.
    Finish {
        /// The last verdict, if the recipe has a loop: whether it passed.
        review_passed: Option<bool>,
    },
    /// The work cannot go on.
    Stop {
        /// Why, in words for the page.
        reason: String,
    },
}

/// The next step for `recipe`, after `done`.
#[must_use]
pub fn next(recipe: &Recipe, done: &[Done]) -> Next {
    let lp = recipe.repeat.as_ref().and_then(|l| {
        Some((
            recipe.index_of(&l.between[0])?,
            recipe.index_of(&l.between[1])?,
            l.max,
        ))
    });
    let mut pos = 0usize;
    let mut fixes = 0u32;
    let mut passed = None;
    for d in done {
        let Some(step) = recipe.steps.get(pos) else {
            return Next::Stop {
                reason: format!("step `{}` ran after the recipe ended", d.step),
            };
        };
        if step.id != d.step {
            return Next::Stop {
                reason: format!("step `{}` ran where `{}` was due", d.step, step.id),
            };
        }
        match lp {
            Some((a, b, max)) if pos == a => match &d.verdict {
                None => {
                    return Next::Stop {
                        reason: format!("the verdict of `{}` could not be read", step.id),
                    };
                }
                Some(v) if v.pass => {
                    passed = Some(true);
                    pos = b + 1;
                }
                Some(_) if fixes < max => {
                    passed = Some(false);
                    pos = b;
                }
                Some(_) => {
                    passed = Some(false);
                    pos = b + 1;
                }
            },
            Some((a, b, _)) if pos == b => {
                fixes += 1;
                pos = a;
            }
            _ => pos += 1,
        }
    }
    if pos >= recipe.steps.len() {
        return Next::Finish {
            review_passed: passed,
        };
    }
    let round = match lp {
        Some((a, b, _)) if pos == a || pos == b => fixes + 1,
        _ => 1,
    };
    Next::Run { step: pos, round }
}

/// Read a verdict from a reviewer's closing words: the last JSON object in
/// them with `"verdict": "pass" | "changes"` and optional `"notes"`.
#[must_use]
pub fn read_verdict(text: &str) -> Option<Verdict> {
    let mut found = None;
    for (start, _) in text.match_indices('{') {
        let mut stream =
            serde_json::Deserializer::from_str(&text[start..]).into_iter::<serde_json::Value>();
        if let Some(Ok(value)) = stream.next()
            && let Some(word) = value.get("verdict").and_then(|v| v.as_str())
        {
            let pass = match word {
                "pass" => true,
                "changes" => false,
                _ => continue,
            };
            let notes = value
                .get("notes")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            found = Some(Verdict { pass, notes });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe() -> Recipe {
        crate::builtin::recipes()
            .into_iter()
            .find(|r| r.name == "implement-and-review")
            .unwrap()
    }

    fn done(step: &str) -> Done {
        Done {
            step: step.into(),
            verdict: None,
        }
    }

    fn verdict(pass: bool) -> Done {
        Done {
            step: "review".into(),
            verdict: Some(Verdict {
                pass,
                notes: "n".into(),
            }),
        }
    }

    #[test]
    fn a_pass_goes_straight_to_the_end() {
        let r = recipe();
        assert_eq!(next(&r, &[]), Next::Run { step: 0, round: 1 });
        assert_eq!(
            next(&r, &[done("implement")]),
            Next::Run { step: 1, round: 1 }
        );
        assert_eq!(
            next(&r, &[done("implement"), verdict(true)]),
            Next::Finish {
                review_passed: Some(true)
            }
        );
    }

    /// Design 13: the loop ends at its bound even when the reviewer still
    /// wants changes; it does not run on until someone agrees.
    #[test]
    fn the_loop_stops_at_its_bound() {
        let r = recipe();
        let mut history = vec![done("implement"), verdict(false)];
        assert_eq!(next(&r, &history), Next::Run { step: 2, round: 1 });
        history.push(done("fix"));
        assert_eq!(next(&r, &history), Next::Run { step: 1, round: 2 });
        history.push(verdict(false));
        assert_eq!(next(&r, &history), Next::Run { step: 2, round: 2 });
        history.push(done("fix"));
        history.push(verdict(false));
        assert_eq!(
            next(&r, &history),
            Next::Finish {
                review_passed: Some(false)
            },
            "two fixes, then the verdict stands"
        );
    }

    #[test]
    fn an_unreadable_verdict_stops_the_work() {
        let r = recipe();
        let history = vec![done("implement"), done("review")];
        assert!(matches!(next(&r, &history), Next::Stop { .. }));
    }

    #[test]
    fn steps_cannot_be_added_or_reordered() {
        let r = recipe();
        assert!(matches!(next(&r, &[done("deploy")]), Next::Stop { .. }));
        let finished = vec![done("implement"), verdict(true), done("fix")];
        assert!(matches!(next(&r, &finished), Next::Stop { .. }));
    }

    #[test]
    fn a_verdict_is_read_from_the_last_json_object() {
        let text = "I looked.\n```json\n{\"verdict\": \"changes\", \"notes\": \"add a test\"}\n```\n\
                    Then again:\n{\"verdict\": \"pass\"}";
        assert_eq!(
            read_verdict(text),
            Some(Verdict {
                pass: true,
                notes: String::new()
            })
        );
        assert_eq!(
            read_verdict("{\"verdict\": \"changes\", \"notes\": \"x\"}").map(|v| v.pass),
            Some(false)
        );
        assert_eq!(read_verdict("looks fine to me"), None);
        assert_eq!(read_verdict("{\"verdict\": \"maybe\"}"), None);
    }
}
