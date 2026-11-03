//! Lark's JSON into task and comment records.
//!
//! Design: `docs/design/05-connectors.md`, "归一". A task is its own
//! thread, `task:<guid>`; the task is `task:<guid>` in it, each comment
//! `comment:<id>`. The record keeps Lark's JSON as it came, as the raw.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use genatrix_connector::protocol::{Party, TaskComment, TaskListRef, TaskRecord};
use serde_json::Value;

/// Names known so far: users by open id, task lists by guid.
#[derive(Debug, Default)]
pub struct Names {
    /// Users.
    pub users: BTreeMap<String, String>,
    /// Task lists.
    pub lists: BTreeMap<String, String>,
    /// Ids already looked up, found or not, so a name Lark will not give
    /// is not asked for every round.
    pub tried: BTreeSet<String>,
}

/// A task's guid.
#[must_use]
pub fn guid(task: &Value) -> Option<&str> {
    task.get("guid")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// When a task last changed, as Lark says it: milliseconds, as a string.
#[must_use]
pub fn updated(task: &Value) -> String {
    text(task, "updated_at")
}

/// The thread a task and its comments share.
#[must_use]
pub fn thread_key(guid: &str) -> String {
    format!("task:{guid}")
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Milliseconds as Lark writes them, as RFC 3339. Zero and nothing are no
/// time at all.
fn time(ms: &str) -> Option<String> {
    let ms: i64 = ms.trim().parse().ok().filter(|ms| *ms > 0)?;
    DateTime::<Utc>::from_timestamp_millis(ms).map(|t| t.to_rfc3339())
}

/// The user ids a task names, for looking their names up.
#[must_use]
pub fn people(task: &Value) -> Vec<String> {
    let mut ids: Vec<String> = task
        .get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(task.get("creator"))
        .filter(|m| m.get("type").and_then(Value::as_str).unwrap_or("user") == "user")
        .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// The task lists a task is in.
#[must_use]
pub fn lists(task: &Value) -> Vec<String> {
    task.get("tasklists")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|l| l.get("tasklist_guid").and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn party(member: &Value, names: &Names) -> Option<Party> {
    let id = member.get("id").and_then(Value::as_str)?.to_owned();
    let name = member
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| names.users.get(&id).cloned())
        .unwrap_or_default();
    Some(Party { id, name })
}

/// One task, normalized. `None` for something without a guid.
#[must_use]
pub fn task(task: &Value, names: &Names) -> Option<TaskRecord> {
    let guid = guid(task)?;
    let completed = time(&text(task, "completed_at")).is_some();
    let status = if completed || text(task, "status") == "done" {
        "done"
    } else {
        "todo"
    };
    let assignees = task
        .get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| m.get("role").and_then(Value::as_str) == Some("assignee"))
        .filter(|m| m.get("type").and_then(Value::as_str).unwrap_or("user") == "user")
        .filter_map(|m| party(m, names))
        .collect();
    let created = time(&text(task, "created_at")).unwrap_or_default();
    Some(TaskRecord {
        external_id: thread_key(guid),
        thread_key: thread_key(guid),
        title: text(task, "summary").trim().to_owned(),
        description: text(task, "description").trim().to_owned(),
        status: status.to_owned(),
        due: task
            .get("due")
            .and_then(|d| d.get("timestamp"))
            .and_then(Value::as_str)
            .and_then(time),
        url: Some(text(task, "url")).filter(|u| !u.is_empty()),
        lists: lists(task)
            .into_iter()
            .map(|id| TaskListRef {
                name: names.lists.get(&id).cloned().unwrap_or_default(),
                id,
            })
            .collect(),
        creator: task.get("creator").and_then(|c| party(c, names)),
        assignees,
        updated: time(&updated(task)).unwrap_or_else(|| created.clone()),
        created,
        raw: serde_json::to_vec(task).unwrap_or_default(),
    })
}

/// One comment on a task, normalized.
#[must_use]
pub fn comment(comment: &Value, guid: &str, title: &str) -> Option<TaskComment> {
    let id = comment
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    let author = comment.get("creator").and_then(|c| {
        Some(Party {
            id: c.get("id").and_then(Value::as_str)?.to_owned(),
            name: text(c, "name"),
        })
    });
    Some(TaskComment {
        external_id: format!("comment:{id}"),
        thread_key: thread_key(guid),
        thread_title: title.to_owned(),
        author,
        date: time(&text(comment, "created_at")).unwrap_or_default(),
        text: text(comment, "content").trim().to_owned(),
        reply_to: Some(text(comment, "reply_to_comment_id"))
            .filter(|r| !r.is_empty() && r != "0")
            .map(|r| format!("comment:{r}")),
        raw: serde_json::to_vec(comment).unwrap_or_default(),
    })
}

/// When a comment last changed.
#[must_use]
pub fn comment_updated(comment: &Value) -> String {
    let u = text(comment, "updated_at");
    if u.is_empty() {
        text(comment, "created_at")
    } else {
        u
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;

    pub(crate) fn sample(guid: &str, updated: &str, done: bool) -> Value {
        json!({
            "guid": guid,
            "summary": " Fix the login page ",
            "description": "It 500s on Safari.",
            "due": {"timestamp": "1767225600000", "is_all_day": true},
            "creator": {"id": "ou_boss", "type": "user", "role": "creator"},
            "members": [
                {"id": "ou_me", "type": "user", "role": "assignee"},
                {"id": "ou_peer", "type": "user", "role": "follower"},
                {"id": "cli_bot", "type": "app", "role": "assignee"}
            ],
            "completed_at": if done { "1767000000000" } else { "0" },
            "created_at": "1766000000000",
            "updated_at": updated,
            "status": if done { "done" } else { "todo" },
            "url": "https://applink.larksuite.com/client/todo/detail?guid=x",
            "tasklists": [{"tasklist_guid": "tl-1", "section_guid": "s"}]
        })
    }

    #[test]
    fn a_task_keeps_what_the_cell_needs_and_the_raw_keeps_the_rest() {
        let mut names = Names::default();
        names.users.insert("ou_me".into(), "Neo".into());
        names.lists.insert("tl-1".into(), "Web".into());
        let t = task(&sample("g-1", "1766500000000", false), &names).unwrap();
        assert_eq!(t.external_id, "task:g-1");
        assert_eq!(t.thread_key, "task:g-1");
        assert_eq!(t.title, "Fix the login page");
        assert_eq!(t.status, "todo");
        assert!(t.due.as_deref().unwrap().starts_with("2026-01-01"));
        assert_eq!(
            t.lists,
            vec![TaskListRef {
                id: "tl-1".into(),
                name: "Web".into()
            }]
        );
        assert_eq!(
            t.assignees,
            vec![Party {
                id: "ou_me".into(),
                name: "Neo".into()
            }],
            "followers and apps are not assignees"
        );
        assert_eq!(t.creator.unwrap().id, "ou_boss");
        let raw: Value = serde_json::from_slice(&t.raw).unwrap();
        assert_eq!(raw["guid"], "g-1");

        let done = task(&sample("g-1", "1767000000000", true), &names).unwrap();
        assert_eq!(done.status, "done");
        assert_ne!(done.raw, t.raw, "a change is a new version");
    }

    #[test]
    fn people_are_users_only_and_named_once() {
        assert_eq!(
            people(&sample("g", "1", false)),
            vec!["ou_boss", "ou_me", "ou_peer"]
        );
    }

    #[test]
    fn a_comment_names_its_author_and_what_it_answers() {
        let c = comment(
            &json!({"id": "7", "content": " LGTM ", "creator": {"id": "ou_peer", "name": "Ann"},
                    "reply_to_comment_id": "6", "created_at": "1766000000000"}),
            "g-1",
            "Fix",
        )
        .unwrap();
        assert_eq!(c.external_id, "comment:7");
        assert_eq!(c.thread_key, "task:g-1");
        assert_eq!(c.text, "LGTM");
        assert_eq!(c.reply_to.as_deref(), Some("comment:6"));
        assert_eq!(c.author.unwrap().name, "Ann");
        let first = comment(&json!({"id": "6", "reply_to_comment_id": "0"}), "g", "").unwrap();
        assert_eq!(first.reply_to, None);
    }
}
