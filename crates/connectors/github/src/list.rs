//! Reading a repository's open issues and pull requests (design 13 v0.7,
//! "读 issue 和 PR"). Read only: nothing here changes anything on GitHub.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::rest::{self, RestError};
use crate::valid_repo;

/// How many pages of a hundred open issues are read at most.
pub const ISSUE_PAGES: u32 = 5;
/// How many pages of a hundred open pull requests are read at most.
pub const PULL_PAGES: u32 = 3;
/// How many recently closed pull requests are read, to see what became of
/// the ones Genatrix opened.
pub const CLOSED_PULLS: u32 = 50;
/// How much of an issue's text is kept.
pub const BODY_CAP: usize = 16 * 1024;

/// One open issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    /// Its number in the repository.
    pub number: u64,
    /// Its title.
    pub title: String,
    /// Its text, cut at [`BODY_CAP`].
    pub body: String,
    /// Where it is on GitHub.
    pub url: String,
    /// Who opened it.
    pub author: String,
    /// Its labels.
    pub labels: Vec<String>,
    /// Who it is assigned to.
    pub assignees: Vec<String>,
    /// How many comments it has.
    pub comments: u64,
    /// When it was opened, RFC 3339.
    pub created: String,
    /// When it last changed, RFC 3339.
    pub updated: String,
}

/// One pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pull {
    /// Its number in the repository.
    pub number: u64,
    /// Its title.
    pub title: String,
    /// Where it is on GitHub.
    pub url: String,
    /// Who opened it.
    pub author: String,
    /// `open`, `closed` or `merged`.
    pub state: String,
    /// Whether it is a draft.
    pub draft: bool,
    /// The branch with the work.
    pub head: String,
    /// Whether that branch is in another repository, a fork.
    pub from_fork: bool,
    /// The branch it goes into.
    pub base: String,
    /// When it last changed, RFC 3339.
    pub updated: String,
}

/// What one repository has open.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    /// Open issues, most recently changed first.
    pub issues: Vec<Issue>,
    /// Open pull requests, then recently closed ones.
    pub pulls: Vec<Pull>,
    /// Whether there were more open issues than were read.
    pub more_issues: bool,
    /// Whether there were more open pull requests than were read.
    pub more_pulls: bool,
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn login(v: Option<&Value>) -> String {
    v.and_then(|u| u.get("login"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Cut text at a character boundary at or below `cap` bytes.
fn capped(mut s: String, cap: usize) -> String {
    if s.len() > cap {
        let mut at = cap;
        while !s.is_char_boundary(at) {
            at -= 1;
        }
        s.truncate(at);
        s.push_str("\n…");
    }
    s
}

/// One issue from GitHub's JSON; `None` for a pull request, which GitHub
/// lists among the issues too, or for something without a number.
#[must_use]
pub fn issue(v: &Value) -> Option<Issue> {
    if v.get("pull_request").is_some() {
        return None;
    }
    Some(Issue {
        number: v.get("number").and_then(Value::as_u64)?,
        title: text(v, "title").trim().to_owned(),
        body: capped(text(v, "body").trim().to_owned(), BODY_CAP),
        url: text(v, "html_url"),
        author: login(v.get("user")),
        labels: v
            .get("labels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|l| l.get("name").and_then(Value::as_str).map(str::to_owned))
            .collect(),
        assignees: v
            .get("assignees")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|a| login(Some(a)))
            .filter(|a| !a.is_empty())
            .collect(),
        comments: v.get("comments").and_then(Value::as_u64).unwrap_or(0),
        created: text(v, "created_at"),
        updated: text(v, "updated_at"),
    })
}

/// One pull request from GitHub's JSON, for repository `repo`.
#[must_use]
pub fn pull(v: &Value, repo: &str) -> Option<Pull> {
    let head = v.get("head");
    let head_repo = head
        .and_then(|h| h.get("repo"))
        .and_then(|r| r.get("full_name"))
        .and_then(Value::as_str);
    let merged = v.get("merged_at").is_some_and(|m| !m.is_null());
    let state = match (text(v, "state").as_str(), merged) {
        (_, true) => "merged",
        ("open", _) => "open",
        _ => "closed",
    };
    Some(Pull {
        number: v.get("number").and_then(Value::as_u64)?,
        title: text(v, "title").trim().to_owned(),
        url: text(v, "html_url"),
        author: login(v.get("user")),
        state: state.to_owned(),
        draft: v.get("draft").and_then(Value::as_bool).unwrap_or(false),
        head: head.map(|h| text(h, "ref")).unwrap_or_default(),
        from_fork: head_repo.is_some_and(|r| !r.eq_ignore_ascii_case(repo)),
        base: v.get("base").map(|b| text(b, "ref")).unwrap_or_default(),
        updated: text(v, "updated_at"),
    })
}

/// Read one repository's open issues and pull requests.
pub fn list(token: &str, repo: &str) -> Result<Listing, RestError> {
    if !valid_repo(repo) {
        return Err(RestError::Known(format!("`{repo}` is not owner/name")));
    }
    let mut out = Listing::default();
    for page in 1..=ISSUE_PAGES {
        let got = rest::get_list(
            token,
            &format!("/repos/{repo}/issues?state=open&sort=updated&per_page=100&page={page}"),
            "Issues",
        )?;
        let full = got.len() == 100;
        out.issues.extend(got.iter().filter_map(issue));
        if !full {
            break;
        }
        out.more_issues = page == ISSUE_PAGES;
    }
    for page in 1..=PULL_PAGES {
        let got = rest::get_list(
            token,
            &format!(
                "/repos/{repo}/pulls?state=open&sort=updated&direction=desc&per_page=100&page={page}"
            ),
            "Pull requests",
        )?;
        let full = got.len() == 100;
        out.pulls.extend(got.iter().filter_map(|v| pull(v, repo)));
        if !full {
            break;
        }
        out.more_pulls = page == PULL_PAGES;
    }
    let closed = rest::get_list(
        token,
        &format!(
            "/repos/{repo}/pulls?state=closed&sort=updated&direction=desc&per_page={CLOSED_PULLS}"
        ),
        "Pull requests",
    )?;
    out.pulls
        .extend(closed.iter().filter_map(|v| pull(v, repo)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn an_issue_is_read_and_a_pull_request_among_the_issues_is_not() {
        let v = json!({
            "number": 12, "title": " Login 500s ", "body": "On Safari.",
            "html_url": "https://github.com/o/r/issues/12",
            "user": {"login": "stranger"},
            "labels": [{"name": "bug"}], "assignees": [{"login": "me"}],
            "comments": 3, "created_at": "2026-11-01T00:00:00Z", "updated_at": "2026-11-02T00:00:00Z"
        });
        let i = issue(&v).unwrap();
        assert_eq!(i.number, 12);
        assert_eq!(i.title, "Login 500s");
        assert_eq!(i.labels, ["bug"]);
        assert_eq!(i.assignees, ["me"]);
        assert_eq!(i.author, "stranger");
        let mut pr = v;
        pr["pull_request"] = json!({"url": "…"});
        assert!(issue(&pr).is_none());
    }

    #[test]
    fn a_long_issue_is_cut_on_a_character() {
        let body = "界".repeat(BODY_CAP);
        let i = issue(&json!({"number": 1, "body": body})).unwrap();
        assert!(i.body.len() <= BODY_CAP + "\n…".len());
        assert!(i.body.ends_with('…'));
    }

    #[test]
    fn a_pull_request_says_whether_it_was_merged_and_where_its_branch_is() {
        let v = json!({
            "number": 7, "title": "Fix", "html_url": "u", "user": {"login": "me"},
            "state": "closed", "merged_at": "2026-11-03T00:00:00Z", "draft": false,
            "head": {"ref": "genatrix/01abc", "repo": {"full_name": "O/R"}},
            "base": {"ref": "main"}, "updated_at": "t"
        });
        let p = pull(&v, "o/r").unwrap();
        assert_eq!(p.state, "merged");
        assert_eq!(p.head, "genatrix/01abc");
        assert!(!p.from_fork);
        let mut forked = v;
        forked["merged_at"] = Value::Null;
        forked["head"]["repo"]["full_name"] = json!("someone/r");
        let p = pull(&forked, "o/r").unwrap();
        assert_eq!(p.state, "closed");
        assert!(p.from_fork);
    }
}
