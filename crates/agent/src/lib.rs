//! Task, Run, tools and tool protocols, pipelines, Action.
//!
//! Design: `docs/design/03-agent-layer.md`.
//!
//! The layer that turns a task into a series of model calls and tool calls.
//! It talks to no model directly: it assembles context and hands it to the
//! egress gate. It touches nothing outside the machine: an outward effect
//! can only be proposed, and a person approves it.
//!
//! Three defences against the fact that every item body was written by
//! someone else, none of them complete on its own:
//!
//! 1. [`envelope`] puts untrusted content where it cannot be read as an
//!    instruction.
//! 2. [`output`] constrains what a model is allowed to emit.
//! 3. [`action`] keeps outward effects behind a human.
//!
//! The worst a successful injection achieves is a wrong summary.

#![forbid(unsafe_code)]

pub mod action;
pub mod envelope;
pub mod output;
pub mod protocol;
pub mod run;

pub use action::{Action, ActionError, Approval, Effect, ExecutionToken, Status};
pub use envelope::{DataZone, Source, data_zone};
pub use output::{CollarError, Point, Summary, choice, summary};
pub use protocol::{Intent, RawReply, ToolCall, ToolProtocol, ToolSpec, strip_reasoning};
pub use run::{CallError, Called, ModelCaller, RunContext, RunEnd, RunError, StepRecord};
