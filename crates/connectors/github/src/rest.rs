//! The two calls to GitHub's REST API this connector makes.

use serde_json::{Value, json};

use crate::valid_repo;

const API: &str = "https://api.github.com";

/// What went wrong with a call.
#[derive(Debug, thiserror::Error)]
pub enum RestError {
    /// Nothing was made.
    #[error("{0}")]
    Known(String),
    /// The request went, and what became of it is not known.
    #[error("{0}")]
    Unsure(String),
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn read(mut response: ureq::http::Response<ureq::Body>) -> (u16, Value) {
    let status = response.status().as_u16();
    let body: Value = response.body_mut().read_json().unwrap_or(Value::Null);
    (status, body)
}

/// The login the token belongs to.
pub fn whoami(token: &str) -> Result<String, RestError> {
    let response = agent()
        .get(&format!("{API}/user"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "genatrix")
        .call()
        .map_err(|e| RestError::Known(format!("could not reach GitHub: {e}")))?;
    let (status, body) = read(response);
    if status != 200 {
        return Err(RestError::Known(format!(
            "GitHub said {status}: {}",
            body.get("message")
                .and_then(Value::as_str)
                .unwrap_or("no reason")
        )));
    }
    body.get("login")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| RestError::Known("GitHub did not say who this is".into()))
}

/// Open a pull request; its address.
pub fn pull_request(
    token: &str,
    repo: &str,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
) -> Result<String, RestError> {
    if !valid_repo(repo) {
        return Err(RestError::Known(format!("`{repo}` is not owner/name")));
    }
    if title.trim().is_empty() {
        return Err(RestError::Known("a pull request needs a title".into()));
    }
    let response = agent()
        .post(&format!("{API}/repos/{repo}/pulls"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "genatrix")
        .send_json(json!({"title": title, "body": body, "head": head, "base": base}))
        .map_err(|e| {
            RestError::Unsure(format!(
                "the request may have reached GitHub ({e}); check the repository's pull \
                 requests before asking again"
            ))
        })?;
    let (status, body) = read(response);
    if status != 201 {
        return Err(RestError::Known(format!(
            "GitHub said {status}: {}",
            body.get("message")
                .and_then(Value::as_str)
                .unwrap_or("no reason")
        )));
    }
    body.get("html_url")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| RestError::Known("GitHub opened it but gave no address".into()))
}
