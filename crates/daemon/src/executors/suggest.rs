//! A sentence into a draft: the local model picks the project and the
//! recipe and writes the task out (design 13, "调度").
//!
//! The model fills in a form; it does not dispatch anything. What it
//! suggests lands in the page's form, where the user reads it, changes it,
//! and drafts it, and the draft still waits for approval. Its choice of
//! project and recipe is checked against what is on file; a name it made up
//! is replaced by the first real one, not trusted.

use genatrix_agent::protocol::{json_object, strip_reasoning};
use genatrix_agent::run::ModelCaller as _;
use genatrix_gate::gate::{Initiator, Message, Request};
use genatrix_llm::ticket::Purpose;

use crate::system::System;

/// What the model suggests.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Suggestion {
    /// A project on file.
    pub project: String,
    /// A recipe on file.
    pub recipe: String,
    /// The task, written out.
    pub task: String,
}

/// Ask the local model to turn `text` into a draft.
pub async fn suggest(system: &System, text: &str) -> Result<Suggestion, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("say what the work is".into());
    }
    if !system.model_state.borrow().is_ready() {
        return Err("the model side is not ready; fill the form in yourself".into());
    }
    let defs = super::definitions(&system.store).map_err(|e| e.to_string())?;
    let Some((first, _)) = defs.projects.first() else {
        return Err("there are no projects yet".into());
    };
    let projects: Vec<String> = defs
        .projects
        .iter()
        .map(|(p, _)| {
            format!(
                "- {} ({})",
                p.name,
                p.github.as_deref().unwrap_or("no GitHub")
            )
        })
        .collect();
    let recipes: Vec<String> = defs
        .recipes
        .iter()
        .map(|(r, _)| format!("- {}: {}", r.name, r.title))
        .collect();
    let request = Request {
        purpose: Purpose::Plan,
        initiator: Initiator::User,
        items: Vec::new(),
        level: system.gate.level_of_typed_text(text),
        identities: Vec::new(),
        messages: vec![
            Message::system(format!(
                "You turn a request into a work order for software executors. Pick the \
                 project and the recipe that fit, and write the task out as a developer \
                 would want to read it: what to change, why, and how to know it works. Use \
                 the request's language. Add no facts the request does not give.\n\n\
                 Projects:\n{}\n\nRecipes:\n{}\n\n\
                 Answer with one JSON object: {{\"project\": \"...\", \"recipe\": \"...\", \
                 \"task\": \"...\"}}",
                projects.join("\n"),
                recipes.join("\n")
            )),
            Message::user(text.to_owned()),
        ],
        max_tokens: Some(600),
        temperature: Some(0.2),
        stream: false,
    };
    let called = system
        .caller
        .call(&request)
        .await
        .map_err(|e| e.to_string())?;
    let reply = strip_reasoning(&called.reply.text);
    let json = json_object(reply).ok_or("the model gave no answer in the shape asked")?;
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let pick = |key: &str, known: &[String], fallback: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|v| known.iter().any(|k| k == v))
            .unwrap_or(fallback)
            .to_owned()
    };
    let project_names: Vec<String> = defs.projects.iter().map(|(p, _)| p.name.clone()).collect();
    let recipe_names: Vec<String> = defs.recipes.iter().map(|(r, _)| r.name.clone()).collect();
    let task = value
        .get("task")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(text)
        .to_owned();
    Ok(Suggestion {
        project: pick("project", &project_names, &first.name),
        recipe: pick("recipe", &recipe_names, "implement-and-review"),
        task,
    })
}
