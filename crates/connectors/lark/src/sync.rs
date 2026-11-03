//! One round of reading: what changed since the last one.
//!
//! Design: `docs/design/05-connectors.md`, "拉取". Every round lists the
//! user's open tasks. A task whose `updated_at` moved is stored again, with
//! its comments. A task that was open last round and is not now is read on
//! its own: done, it is stored as done; gone, it is let go. Comments do not
//! move a task's `updated_at`, so every [`COMMENT_SWEEP_MS`] the open
//! tasks' comments are read regardless. The first round also reads the
//! done tasks once, as history.
//!
//! A round returns what to store and the position after it; the caller
//! stores first and saves the position second, so an interruption costs a
//! repeat, never a gap.

use std::collections::BTreeMap;

use genatrix_connector::protocol::{TaskComment, TaskRecord};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::{ApiError, Remote};
use crate::normalize::{self, Names};

/// How often open tasks' comments are read whether or not the task moved.
pub const COMMENT_SWEEP_MS: i64 = 10 * 60 * 1000;

/// Where reading got to. Kept in the core as the account's cursor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    /// Open tasks, by guid, with the `updated_at` last stored.
    #[serde(default)]
    pub open: BTreeMap<String, String>,
    /// The latest comment change stored, by task guid.
    #[serde(default)]
    pub comments: BTreeMap<String, String>,
    /// Whether the done tasks have been read once.
    #[serde(default)]
    pub history: bool,
    /// When the comments were last read for every open task.
    #[serde(default)]
    pub comments_swept_ms: i64,
}

/// What a round found.
#[derive(Debug, Default)]
pub struct Round {
    /// Tasks to store, as they stand now.
    pub tasks: Vec<TaskRecord>,
    /// Comments to store.
    pub comments: Vec<TaskComment>,
    /// Where reading will have got to once these are stored.
    pub position: Position,
}

fn later(a: &str, b: &str) -> bool {
    a.parse::<i64>().unwrap_or(0) > b.parse::<i64>().unwrap_or(0)
}

/// Look up the names a set of tasks needs and are not known yet. A name
/// Lark will not give (no contacts permission, someone outside the
/// company) is asked for once.
fn learn(remote: &impl Remote, names: &mut Names, tasks: &[Value]) {
    let users: Vec<String> = tasks
        .iter()
        .flat_map(normalize::people)
        .filter(|id| !names.users.contains_key(id) && !names.tried.contains(id))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if !users.is_empty() {
        match remote.user_names(&users) {
            // Lark answers without the `name` field when the sign-in lacks
            // `contact:user.base:readonly`: no error, just no names.
            Ok(found) if found.is_empty() => tracing::warn!(
                asked = users.len(),
                "Lark gave no names; the sign-in needs contact:user.base:readonly"
            ),
            Ok(found) => names.users.extend(found),
            Err(e) => {
                tracing::warn!(error = %e, "Lark names could not be read; people show as ids");
            }
        }
        names.tried.extend(users);
    }
    let lists: Vec<String> = tasks
        .iter()
        .flat_map(normalize::lists)
        .filter(|id| !names.lists.contains_key(id) && !names.tried.contains(id))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    for id in lists {
        match remote.tasklist_name(&id) {
            Ok(Some(name)) => {
                names.lists.insert(id.clone(), name);
            }
            Ok(None) => {}
            Err(e) => tracing::info!(error = %e, "a Lark task list name could not be read"),
        }
        names.tried.insert(id);
    }
}

/// One round.
///
/// # Errors
///
/// Any error reading the open tasks; the caller decides whether to
/// refresh, wait, or ask the user. Errors on one task's details or
/// comments end the round too, so nothing is skipped silently.
pub fn round(
    remote: &impl Remote,
    names: &mut Names,
    before: &Position,
    now_ms: i64,
) -> Result<Round, ApiError> {
    let mut position = before.clone();
    let mut out = Round::default();

    let open = remote.my_tasks(Some(false))?;
    let changed: Vec<Value> = open
        .iter()
        .filter(|t| {
            normalize::guid(t).is_some_and(|g| {
                before
                    .open
                    .get(g)
                    .is_none_or(|seen| *seen != normalize::updated(t))
            })
        })
        .cloned()
        .collect();

    // Open last round, not now: done, moved to someone else, or deleted.
    let still_open: std::collections::BTreeSet<&str> =
        open.iter().filter_map(normalize::guid).collect();
    let mut closed = Vec::new();
    for guid in before.open.keys() {
        if still_open.contains(guid.as_str()) {
            continue;
        }
        match remote.task(guid)? {
            Some(task) if normalize::updated(&task) != before.open[guid] => closed.push(task),
            _ => {}
        }
        position.open.remove(guid);
        position.comments.remove(guid);
    }

    let mut history = Vec::new();
    if !before.history {
        history = remote.my_tasks(Some(true))?;
        position.history = true;
    }

    let sweep = now_ms - before.comments_swept_ms >= COMMENT_SWEEP_MS;
    if sweep {
        position.comments_swept_ms = now_ms;
    }

    learn(
        remote,
        names,
        &changed
            .iter()
            .chain(&closed)
            .chain(&history)
            .cloned()
            .collect::<Vec<_>>(),
    );

    let mut with_comments: Vec<(String, String)> = Vec::new();
    for task in changed.iter().chain(&closed).chain(&history) {
        let Some(record) = normalize::task(task, names) else {
            continue;
        };
        with_comments.push((
            record.thread_key["task:".len()..].to_owned(),
            record.title.clone(),
        ));
        out.tasks.push(record);
    }
    for task in &changed {
        if let Some(g) = normalize::guid(task) {
            position.open.insert(g.to_owned(), normalize::updated(task));
        }
    }
    if sweep {
        for task in &open {
            if let Some(g) = normalize::guid(task)
                && !with_comments.iter().any(|(c, _)| c == g)
            {
                let title = task
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                with_comments.push((g.to_owned(), title));
            }
        }
    }

    for (guid, title) in with_comments {
        let seen = before.comments.get(&guid).cloned().unwrap_or_default();
        let mut latest = seen.clone();
        for c in remote.comments(&guid)? {
            let at = normalize::comment_updated(&c);
            if !later(&at, &seen) {
                continue;
            }
            if later(&at, &latest) {
                latest.clone_from(&at);
            }
            if let Some(comment) = normalize::comment(&c, &guid, &title) {
                out.comments.push(comment);
            }
        }
        if position.open.contains_key(&guid) && !latest.is_empty() {
            position.comments.insert(guid, latest);
        }
    }

    out.position = position;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use serde_json::json;

    use super::*;
    use crate::normalize::tests::sample;

    #[derive(Default)]
    struct Fake {
        open: RefCell<Vec<Value>>,
        done: RefCell<Vec<Value>>,
        comments: RefCell<BTreeMap<String, Vec<Value>>>,
        comment_reads: RefCell<u32>,
    }

    impl Remote for Fake {
        fn my_tasks(&self, completed: Option<bool>) -> Result<Vec<Value>, ApiError> {
            Ok(match completed {
                Some(true) => self.done.borrow().clone(),
                _ => self.open.borrow().clone(),
            })
        }
        fn task(&self, guid: &str) -> Result<Option<Value>, ApiError> {
            Ok(self
                .done
                .borrow()
                .iter()
                .find(|t| t["guid"] == guid)
                .cloned())
        }
        fn comments(&self, guid: &str) -> Result<Vec<Value>, ApiError> {
            *self.comment_reads.borrow_mut() += 1;
            Ok(self
                .comments
                .borrow()
                .get(guid)
                .cloned()
                .unwrap_or_default())
        }
        fn tasklist_name(&self, _: &str) -> Result<Option<String>, ApiError> {
            Ok(Some("Web".into()))
        }
        fn user_names(&self, ids: &[String]) -> Result<BTreeMap<String, String>, ApiError> {
            Ok(ids
                .iter()
                .map(|i| (i.clone(), format!("name of {i}")))
                .collect())
        }
    }

    fn comment(id: &str, at: &str) -> Value {
        json!({"id": id, "content": "c", "creator": {"id": "ou_peer", "name": "Ann"},
               "created_at": at, "updated_at": at})
    }

    #[test]
    fn the_first_round_reads_open_and_done_tasks_with_their_comments() {
        let fake = Fake::default();
        fake.open.borrow_mut().push(sample("a", "10", false));
        fake.done.borrow_mut().push(sample("old", "5", true));
        fake.comments
            .borrow_mut()
            .insert("a".into(), vec![comment("1", "11")]);
        let mut names = Names::default();
        let r = round(&fake, &mut names, &Position::default(), 0).unwrap();
        let ids: Vec<_> = r.tasks.iter().map(|t| t.external_id.as_str()).collect();
        assert_eq!(ids, ["task:a", "task:old"]);
        assert_eq!(r.comments.len(), 1);
        assert!(r.position.history);
        assert_eq!(r.position.open.get("a").map(String::as_str), Some("10"));
        assert!(
            !r.position.open.contains_key("old"),
            "done tasks are not followed"
        );
        assert_eq!(r.tasks[0].lists[0].name, "Web");
        assert_eq!(r.tasks[0].assignees[0].name, "name of ou_me");
    }

    #[test]
    fn a_quiet_round_stores_nothing_and_a_change_stores_the_new_version() {
        let fake = Fake::default();
        fake.open.borrow_mut().push(sample("a", "10", false));
        fake.comments
            .borrow_mut()
            .insert("a".into(), vec![comment("1", "11")]);
        let mut names = Names::default();
        let first = round(&fake, &mut names, &Position::default(), 0).unwrap();

        let quiet = round(&fake, &mut names, &first.position, 1000).unwrap();
        assert!(quiet.tasks.is_empty() && quiet.comments.is_empty());

        fake.open.borrow_mut()[0] = sample("a", "20", false);
        let moved = round(&fake, &mut names, &quiet.position, 2000).unwrap();
        assert_eq!(moved.tasks.len(), 1);
        assert!(moved.comments.is_empty(), "the comment was stored already");
        assert_eq!(moved.position.open["a"], "20");
    }

    #[test]
    fn a_new_comment_on_an_unchanged_task_arrives_with_the_sweep() {
        let fake = Fake::default();
        fake.open.borrow_mut().push(sample("a", "10", false));
        let mut names = Names::default();
        let first = round(&fake, &mut names, &Position::default(), 0).unwrap();
        fake.comments
            .borrow_mut()
            .insert("a".into(), vec![comment("1", "11")]);

        let before_sweep = round(&fake, &mut names, &first.position, 60_000).unwrap();
        assert!(before_sweep.comments.is_empty());
        let swept = round(
            &fake,
            &mut names,
            &before_sweep.position,
            COMMENT_SWEEP_MS + 1,
        )
        .unwrap();
        assert_eq!(swept.comments.len(), 1);
        assert_eq!(swept.position.comments["a"], "11");
        let again = round(&fake, &mut names, &swept.position, 2 * COMMENT_SWEEP_MS + 2).unwrap();
        assert!(again.comments.is_empty(), "a comment is stored once");
    }

    #[test]
    fn a_task_done_since_the_last_round_is_stored_as_done_and_let_go() {
        let fake = Fake::default();
        fake.open.borrow_mut().push(sample("a", "10", false));
        let mut names = Names::default();
        let first = round(&fake, &mut names, &Position::default(), 0).unwrap();

        fake.open.borrow_mut().clear();
        fake.done.borrow_mut().push(sample("a", "30", true));
        let r = round(&fake, &mut names, &first.position, 1000).unwrap();
        assert_eq!(r.tasks.len(), 1);
        assert_eq!(r.tasks[0].status, "done");
        assert!(r.position.open.is_empty());

        // Gone altogether: let go, nothing stored.
        let mut gone = first.position.clone();
        gone.open.insert("vanished".into(), "1".into());
        fake.done.borrow_mut().clear();
        let r = round(&fake, &mut names, &gone, 2000).unwrap();
        assert!(r.tasks.is_empty());
        assert!(!r.position.open.contains_key("vanished"));
    }

    #[test]
    fn names_are_asked_for_once() {
        struct Counting(RefCell<u32>);
        impl Remote for Counting {
            fn my_tasks(&self, c: Option<bool>) -> Result<Vec<Value>, ApiError> {
                Ok(if c == Some(false) {
                    vec![sample("a", "1", false)]
                } else {
                    vec![]
                })
            }
            fn task(&self, _: &str) -> Result<Option<Value>, ApiError> {
                Ok(None)
            }
            fn comments(&self, _: &str) -> Result<Vec<Value>, ApiError> {
                Ok(vec![])
            }
            fn tasklist_name(&self, _: &str) -> Result<Option<String>, ApiError> {
                Ok(None)
            }
            fn user_names(&self, _: &[String]) -> Result<BTreeMap<String, String>, ApiError> {
                *self.0.borrow_mut() += 1;
                Err(ApiError::Forbidden("no contacts permission".into()))
            }
        }
        let remote = Counting(RefCell::new(0));
        let mut names = Names::default();
        let first = round(&remote, &mut names, &Position::default(), 0).unwrap();
        let mut again = first.position.clone();
        again.open.clear();
        round(&remote, &mut names, &again, 1).unwrap();
        assert_eq!(*remote.0.borrow(), 1);
    }
}
