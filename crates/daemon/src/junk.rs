//! Junk (design 01, "垃圾"; design 06, "标为垃圾"): a person, or a group or
//! channel, the user does not want. What is there is removed, files and
//! all; what arrives later is dropped in ingest. The ledger keeps the
//! counts, not the content.

use anyhow::{Context, bail};
use genatrix_model::{PersonId, ThreadId, ThreadKind};
use genatrix_store::Purged;

use crate::system::System;

/// What marking would remove.
pub fn count_person(system: &System, person: PersonId) -> anyhow::Result<usize> {
    Ok(system.store.person_conversation_items(person)?.len())
}

/// What marking would remove.
pub fn count_thread(system: &System, thread: ThreadId) -> anyhow::Result<usize> {
    Ok(system.store.thread_items(thread)?.len())
}

/// Mark a person as junk and remove their conversation with the user.
pub fn mark_person(system: &System, person: PersonId) -> anyhow::Result<Purged> {
    let found = system.store.get_person(person)?.context("no such person")?;
    if found.is_self {
        bail!("you cannot mark yourself as junk");
    }
    let ids = system.store.person_conversation_items(person)?;
    let done = remove(system, &ids)?;
    system
        .store
        .set_person_junk(person, Some(&chrono::Utc::now().to_rfc3339()))?;
    record(system, "person", &person.to_string(), &done)?;
    Ok(done)
}

/// Mark a group or channel as junk and remove its messages.
pub fn mark_thread(system: &System, thread: ThreadId) -> anyhow::Result<Purged> {
    let found = system
        .store
        .get_thread(thread)?
        .context("no such conversation")?;
    if !matches!(found.kind, ThreadKind::GroupChat | ThreadKind::Channel) {
        bail!("only a group or a channel is marked this way; mark the person instead");
    }
    let ids = system.store.thread_items(thread)?;
    let done = remove(system, &ids)?;
    system
        .store
        .set_thread_junk(thread, Some(&chrono::Utc::now().to_rfc3339()))?;
    record(system, "thread", &thread.to_string(), &done)?;
    Ok(done)
}

/// Take the mark away: new messages are kept again. What was removed stays
/// removed.
pub fn restore(system: &System, kind: &str, id: &str) -> anyhow::Result<()> {
    match kind {
        "person" => system
            .store
            .set_person_junk(id.parse().context("not a person")?, None)?,
        "thread" => system
            .store
            .set_thread_junk(id.parse().context("not a conversation")?, None)?,
        other => bail!("nothing called {other} is marked"),
    }
    system
        .ledger
        .append("junk_restored", id, &serde_json::json!({ "kind": kind }))?;
    forget_overviews(system);
    Ok(())
}

fn remove(system: &System, ids: &std::collections::HashSet<String>) -> anyhow::Result<Purged> {
    let done = system.store.remove_items(ids)?;
    for h in &done.raw_files {
        let _ = system.raw_files.remove(h);
    }
    for h in &done.blob_files {
        let _ = system.blob_files.remove(h);
    }
    forget_overviews(system);
    Ok(done)
}

fn record(system: &System, kind: &str, id: &str, done: &Purged) -> anyhow::Result<()> {
    system.ledger.append(
        "junk",
        id,
        &serde_json::json!({
            "kind": kind,
            "items": done.items,
            "raws": done.raws,
            "commitments": done.commitments,
            "files": done.raw_files.len() + done.blob_files.len(),
        }),
    )?;
    Ok(())
}

/// The people and groups lists counted what just went.
fn forget_overviews(system: &System) {
    *system
        .people
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    *system
        .groups
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}
