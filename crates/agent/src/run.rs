//! The pipeline runtime.
//!
//! Design: `docs/design/03-agent-layer.md`, "两种工作方式" and "运行记录".
//!
//! A pipeline is ordinary code: a fixed sequence someone wrote down, where
//! the model fills in blanks and never decides what happens next. So there is
//! no pipeline type here and no registry of them. What there is, is the
//! context a pipeline runs in: it counts steps, calls models through the
//! egress gate and nothing else, applies a collar to every answer, retries a
//! malformed one, and writes down what happened.
//!
//! The run record holds references, not copies. An item appears as an
//! identifier; a model call appears as the identifier of its egress record.
//! Copying bodies in here would quietly create a second store of personal
//! content with no level on it and no rules over it.

use std::future::Future;

use genatrix_gate::gate::Request;
use genatrix_ledger::{Ledger, kind};
use genatrix_model::ItemId;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::action::Action;
use crate::protocol::RawReply;

/// A model call that went out and came back.
#[derive(Clone, Debug)]
pub struct Called {
    /// Identifier of the egress record the gate wrote.
    pub egress_id: String,
    /// What the model said, with pseudonyms already restored.
    pub reply: RawReply,
}

/// Something that can carry out a prepared model call.
///
/// The agent layer performs no I/O: it assembles a request and hands it over.
/// The implementation in the daemon runs the whole path, gate to gateway to
/// ledger; a test supplies a stub. This is the seam that keeps this crate
/// free of sockets and this layer honest about who talks to whom.
pub trait ModelCaller {
    /// Send one request.
    fn call(&self, request: &Request) -> impl Future<Output = Result<Called, CallError>> + Send;
}

/// Why a model call did not produce an answer.
#[derive(Clone, Debug, thiserror::Error)]
pub enum CallError {
    /// The gate refused to prepare it.
    #[error("the egress gate refused: {0}")]
    Refused(String),
    /// The request was prepared but could not be delivered.
    #[error("the model could not be reached: {0}")]
    Unreachable(String),
}

/// Why a run stopped.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The step budget ran out.
    #[error("run exceeded its budget of {max} steps")]
    OutOfSteps {
        /// The budget.
        max: u32,
    },
    /// A model call failed.
    #[error(transparent)]
    Call(#[from] CallError),
    /// Every attempt at a step produced something we could not use.
    #[error("step `{step}` produced nothing usable after {attempts} attempts: {reason}")]
    Unusable {
        /// Which step.
        step: String,
        /// How many tries.
        attempts: u32,
        /// What was wrong with the last one.
        reason: String,
    },
    /// The run record could not be written, so the run does not continue.
    #[error("ledger write failed: {0}")]
    Ledger(#[from] genatrix_ledger::Error),
}

/// What a step did, as it goes into the ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum StepRecord {
    /// A model was asked something.
    Model {
        /// Step name from the pipeline.
        name: String,
        /// Why it was asked.
        purpose: String,
        /// The egress record, which holds the provenance and, if it left the
        /// device, what was sent.
        egress: String,
        /// Which attempt this was, from 1.
        attempt: u32,
        /// How it turned out.
        result: ModelResult,
    },
    /// A tool was run.
    Tool {
        /// Step name.
        name: String,
        /// Which tool.
        tool: String,
        /// The arguments, which we wrote, so they are safe to keep.
        arguments: serde_json::Value,
        /// Items the tool returned, by identifier.
        items: Vec<String>,
        /// SHA-256 of what it returned, so a replay can be checked without
        /// keeping a copy.
        result_hash: String,
    },
    /// Something worth recording that was not a call.
    Note {
        /// Step name.
        name: String,
        /// What happened.
        detail: String,
    },
}

/// How a model step turned out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ModelResult {
    /// The answer fitted its collar.
    Accepted,
    /// The answer did not fit and was discarded.
    Malformed {
        /// What was wrong.
        reason: String,
    },
}

/// The record written when a run starts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStart {
    /// What the run is for.
    pub task: String,
    /// Step budget.
    pub max_steps: u32,
}

/// The record written when a run ends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "end", rename_all = "snake_case")]
pub enum RunEnd {
    /// It finished.
    Done {
        /// Steps taken.
        steps: u32,
        /// Actions proposed.
        actions: Vec<String>,
    },
    /// It stopped early.
    Stopped {
        /// Steps taken.
        steps: u32,
        /// Why.
        reason: String,
    },
}

/// How many times a step will be retried when the answer does not fit.
pub const DEFAULT_ATTEMPTS: u32 = 2;

/// The context a pipeline runs in.
pub struct RunContext<'a, C> {
    /// Identifier of this run; the subject of every record it writes.
    pub run_id: String,
    task: String,
    caller: &'a C,
    ledger: &'a Ledger,
    steps: u32,
    max_steps: u32,
    attempts: u32,
    actions: Vec<Action>,
}

impl<C> std::fmt::Debug for RunContext<'_, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunContext")
            .field("run_id", &self.run_id)
            .field("task", &self.task)
            .field("steps", &self.steps)
            .field("max_steps", &self.max_steps)
            .finish_non_exhaustive()
    }
}

impl<'a, C: ModelCaller> RunContext<'a, C> {
    /// Start a run and write its opening record.
    pub fn begin(
        ledger: &'a Ledger,
        caller: &'a C,
        task: impl Into<String>,
        max_steps: u32,
    ) -> Result<Self, RunError> {
        let run_id = Ulid::new().to_string();
        let task = task.into();
        ledger.append(
            kind::RUN,
            &run_id,
            &RunStart {
                task: task.clone(),
                max_steps,
            },
        )?;
        Ok(Self {
            run_id,
            task,
            caller,
            ledger,
            steps: 0,
            max_steps,
            attempts: DEFAULT_ATTEMPTS,
            actions: Vec::new(),
        })
    }

    /// Change how many times a step is retried when the answer does not fit.
    #[must_use]
    pub const fn with_attempts(mut self, attempts: u32) -> Self {
        self.attempts = attempts;
        self
    }

    /// Steps taken so far.
    #[must_use]
    pub const fn steps(&self) -> u32 {
        self.steps
    }

    /// Actions proposed so far.
    #[must_use]
    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    /// Ask a model one narrow question and read the answer through a collar.
    ///
    /// `collar` turns a reply into the value the pipeline wants, or says why
    /// it could not. A reply that does not fit is retried, because a small
    /// model that lost its shape once often finds it again; a reply that
    /// still does not fit after the last attempt stops the step rather than
    /// being guessed at.
    ///
    /// Every attempt is a step against the budget and a line in the record,
    /// so a pipeline that spends its budget retrying is visible.
    pub async fn ask<T>(
        &mut self,
        step: &str,
        request: &Request,
        collar: impl Fn(&RawReply) -> Result<T, String>,
    ) -> Result<T, RunError> {
        let mut last_reason = String::new();
        for attempt in 1..=self.attempts {
            self.spend_step()?;
            let called = self.caller.call(request).await?;
            let (result, value) = match collar(&called.reply) {
                Ok(v) => (ModelResult::Accepted, Some(v)),
                Err(reason) => {
                    last_reason.clone_from(&reason);
                    (ModelResult::Malformed { reason }, None)
                }
            };
            self.ledger.append(
                kind::RUN_STEP,
                &self.run_id,
                &StepRecord::Model {
                    name: step.to_owned(),
                    purpose: request.purpose.as_str().to_owned(),
                    egress: called.egress_id,
                    attempt,
                    result,
                },
            )?;
            if let Some(v) = value {
                return Ok(v);
            }
        }
        Err(RunError::Unusable {
            step: step.to_owned(),
            attempts: self.attempts,
            reason: last_reason,
        })
    }

    /// Record that a tool ran. The tool itself is executed by the caller;
    /// this keeps the record honest about what was read.
    pub fn note_tool(
        &mut self,
        step: &str,
        tool: &str,
        arguments: serde_json::Value,
        items: &[ItemId],
        result: &[u8],
    ) -> Result<(), RunError> {
        self.spend_step()?;
        self.ledger.append(
            kind::RUN_STEP,
            &self.run_id,
            &StepRecord::Tool {
                name: step.to_owned(),
                tool: tool.to_owned(),
                arguments,
                items: items.iter().map(ToString::to_string).collect(),
                result_hash: hex::encode(<sha2::Sha256 as sha2::Digest>::digest(result)),
            },
        )?;
        Ok(())
    }

    /// Record something that was not a call.
    pub fn note(&mut self, step: &str, detail: impl Into<String>) -> Result<(), RunError> {
        self.ledger.append(
            kind::RUN_STEP,
            &self.run_id,
            &StepRecord::Note {
                name: step.to_owned(),
                detail: detail.into(),
            },
        )?;
        Ok(())
    }

    /// Propose an action. It is written down and waits for a person; nothing
    /// in this crate can approve it.
    ///
    /// # Errors
    ///
    /// If the record cannot be written, in which case the action does not
    /// exist as far as the system is concerned.
    ///
    /// # Panics
    ///
    /// Never: the action was pushed immediately above.
    pub fn propose(&mut self, action: Action) -> Result<&Action, RunError> {
        self.ledger.append(kind::ACTION, &action.id, &action)?;
        self.actions.push(action);
        Ok(self.actions.last().expect("just pushed"))
    }

    /// Close the run.
    pub fn finish(self, end: &RunEnd) -> Result<Vec<Action>, RunError> {
        self.ledger.append(kind::RUN_END, &self.run_id, end)?;
        Ok(self.actions)
    }

    /// Close the run as having completed.
    pub fn done(self) -> Result<Vec<Action>, RunError> {
        let end = RunEnd::Done {
            steps: self.steps,
            actions: self.actions.iter().map(|a| a.id.clone()).collect(),
        };
        self.finish(&end)
    }

    /// Close the run as having stopped early.
    pub fn stopped(self, reason: impl Into<String>) -> Result<Vec<Action>, RunError> {
        let end = RunEnd::Stopped {
            steps: self.steps,
            reason: reason.into(),
        };
        self.finish(&end)
    }

    fn spend_step(&mut self) -> Result<(), RunError> {
        if self.steps >= self.max_steps {
            return Err(RunError::OutOfSteps {
                max: self.max_steps,
            });
        }
        self.steps += 1;
        Ok(())
    }
}
