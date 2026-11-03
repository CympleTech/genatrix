//! The calls to Lark's task API this connector makes, all of them reads.
//!
//! Everything is behind [`Remote`] so the sync can be tested without
//! Lark; [`Http`] is the real one.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::auth::{Token, agent};
use crate::brand::Brand;

/// What went wrong with a call.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The access token is no longer accepted: refresh, then try again.
    #[error("the sign-in was not accepted: {0}")]
    Unauthorized(String),
    /// The app lacks a permission; only the user can fix that.
    #[error("{0}")]
    Forbidden(String),
    /// It may pass.
    #[error("{0}")]
    Transient(String),
}

/// Lark, as far as this connector reads it.
pub trait Remote {
    /// The tasks in the user's "assigned to me" view; open ones, done ones,
    /// or both.
    fn my_tasks(&self, completed: Option<bool>) -> Result<Vec<Value>, ApiError>;
    /// One task, or `None` when it is gone or no longer the user's to see.
    fn task(&self, guid: &str) -> Result<Option<Value>, ApiError>;
    /// A task's comments, oldest first.
    fn comments(&self, guid: &str) -> Result<Vec<Value>, ApiError>;
    /// A task list's name.
    fn tasklist_name(&self, guid: &str) -> Result<Option<String>, ApiError>;
    /// Names for user ids, as many as can be read.
    fn user_names(&self, ids: &[String]) -> Result<BTreeMap<String, String>, ApiError>;
}

/// The real Lark, as one signed-in user.
pub struct Http {
    base: String,
    bearer: String,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

/// Codes Lark answers with when the access token is the problem.
const TOKEN_CODES: &[i64] = &[99_991_661, 99_991_663, 99_991_668, 99_991_677];
/// Codes for "not found" and "no access to this one".
const GONE_CODES: &[i64] = &[1_470_403, 1_470_404];

impl Http {
    /// Lark for this sign-in.
    #[must_use]
    pub fn new(brand: Brand, token: &Token) -> Self {
        Self {
            base: brand.api(),
            bearer: format!("Bearer {}", token.access_token),
        }
    }

    /// One GET; the `data` of the answer, or `None` for gone.
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Option<Value>, ApiError> {
        let mut request = agent()
            .get(&format!("{}{path}", self.base))
            .header("Authorization", &self.bearer);
        for (key, value) in query {
            request = request.query(*key, value);
        }
        let mut response = request
            .call()
            .map_err(|e| ApiError::Transient(format!("could not reach Lark: {e}")))?;
        let status = response.status().as_u16();
        let body: Value = response.body_mut().read_json().unwrap_or(Value::Null);
        answer(status, &body)
    }

    /// Every page of a listing.
    fn pages(&self, path: &str, query: &[(&str, String)]) -> Result<Vec<Value>, ApiError> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        // A bound against a server that never says "no more".
        for _ in 0..200 {
            let mut q = query.to_vec();
            q.push(("page_size", "100".into()));
            if let Some(t) = &token {
                q.push(("page_token", t.clone()));
            }
            let Some(data) = self.get(path, &q)? else {
                break;
            };
            if let Some(items) = data.get("items").and_then(Value::as_array) {
                out.extend(items.iter().cloned());
            }
            let more = data
                .get("has_more")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            token = data
                .get("page_token")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(str::to_owned);
            if !more || token.is_none() {
                break;
            }
        }
        Ok(out)
    }
}

/// Read an answer: `data` on success, `None` for gone, the right error
/// otherwise.
fn answer(status: u16, body: &Value) -> Result<Option<Value>, ApiError> {
    let code = body.get("code").and_then(Value::as_i64).unwrap_or(-1);
    let msg = body
        .get("msg")
        .and_then(Value::as_str)
        .unwrap_or("no reason given")
        .to_owned();
    if code == 0 {
        return Ok(Some(body.get("data").cloned().unwrap_or(Value::Null)));
    }
    if status == 401 || TOKEN_CODES.contains(&code) {
        return Err(ApiError::Unauthorized(format!("{code}: {msg}")));
    }
    if GONE_CODES.contains(&code) || status == 404 {
        return Ok(None);
    }
    if status == 403 || code == 99_991_672 || code == 99_991_679 {
        return Err(ApiError::Forbidden(format!(
            "the Lark app is missing a permission ({code}: {msg}); design 05 lists the ones it needs"
        )));
    }
    Err(ApiError::Transient(format!(
        "Lark said {status} ({code}): {msg}"
    )))
}

impl Remote for Http {
    fn my_tasks(&self, completed: Option<bool>) -> Result<Vec<Value>, ApiError> {
        let mut query = vec![
            ("type", "my_tasks".to_owned()),
            ("user_id_type", "open_id".to_owned()),
        ];
        if let Some(c) = completed {
            query.push(("completed", c.to_string()));
        }
        self.pages("/task/v2/tasks", &query)
    }

    fn task(&self, guid: &str) -> Result<Option<Value>, ApiError> {
        Ok(self
            .get(
                &format!("/task/v2/tasks/{}", path_part(guid)),
                &[("user_id_type", "open_id".into())],
            )?
            .and_then(|d| d.get("task").cloned()))
    }

    fn comments(&self, guid: &str) -> Result<Vec<Value>, ApiError> {
        self.pages(
            "/task/v2/comments",
            &[
                ("resource_type", "task".into()),
                ("resource_id", guid.to_owned()),
                ("direction", "asc".into()),
                ("user_id_type", "open_id".into()),
            ],
        )
    }

    fn tasklist_name(&self, guid: &str) -> Result<Option<String>, ApiError> {
        Ok(self
            .get(
                &format!("/task/v2/tasklists/{}", path_part(guid)),
                &[("user_id_type", "open_id".into())],
            )?
            .and_then(|d| {
                d.pointer("/tasklist/name")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }))
    }

    fn user_names(&self, ids: &[String]) -> Result<BTreeMap<String, String>, ApiError> {
        let mut names = BTreeMap::new();
        for chunk in ids.chunks(50) {
            let mut query: Vec<(&str, String)> = vec![("user_id_type", "open_id".into())];
            query.extend(chunk.iter().map(|id| ("user_ids", id.clone())));
            let Some(data) = self.get("/contact/v3/users/batch", &query)? else {
                continue;
            };
            for user in data
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let (Some(id), Some(name)) = (
                    user.get("open_id").and_then(Value::as_str),
                    user.get("name").and_then(Value::as_str),
                ) {
                    names.insert(id.to_owned(), name.to_owned());
                }
            }
        }
        Ok(names)
    }
}

/// A path segment from an id Lark gave us. Its ids are letters, digits and
/// dashes; anything else is refused rather than escaped, so a hostile id
/// cannot turn into another path.
fn path_part(id: &str) -> &str {
    if !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        id
    } else {
        "invalid"
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn answers_are_sorted_into_data_gone_and_the_right_error() {
        assert_eq!(
            answer(200, &json!({"code": 0, "data": {"x": 1}})).unwrap(),
            Some(json!({"x": 1}))
        );
        assert_eq!(
            answer(404, &json!({"code": 1_470_404, "msg": "gone"})).unwrap(),
            None
        );
        assert!(matches!(
            answer(401, &json!({"code": 99_991_677, "msg": "expired"})),
            Err(ApiError::Unauthorized(_))
        ));
        assert!(matches!(
            answer(400, &json!({"code": 99_991_672, "msg": "scope"})),
            Err(ApiError::Forbidden(_))
        ));
        assert!(matches!(
            answer(500, &json!({"code": 1_470_500})),
            Err(ApiError::Transient(_))
        ));
    }

    #[test]
    fn an_id_cannot_become_another_path() {
        assert_eq!(
            path_part("d300a75f-c56a-4be9-80d1-e47653028ceb"),
            "d300a75f-c56a-4be9-80d1-e47653028ceb"
        );
        assert_eq!(path_part("../tasklists"), "invalid");
        assert_eq!(path_part("a?b=c"), "invalid");
    }
}
