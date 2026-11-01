//! Who counts as this machine.
//!
//! Design: `docs/design/02-trust-boundary.md`, invariants 11 and 12 as
//! revised by `docs/design/13-executors.md` (ruling 4);
//! `docs/design/06-interface.md`, "形态".
//!
//! A request from 127.0.0.1 used to be the user, no questions asked. That
//! held while every process on this machine was the user's own. It stops
//! holding the moment something on this machine runs commands for someone
//! else, such as a coding agent that read a hostile issue: one `curl` and it
//! approves its own push. It never held against a web page in the user's
//! browser that rebinds its name to 127.0.0.1.
//!
//! So "this machine" is now a secret, not an address: a random key in a file
//! under the data directory that only the user's system account can read.
//! The menu bar shell reads it and presents it as a header and a cookie; a
//! terminal reads it through `genatrix open`, which trades it for a one-time
//! link that sets the cookie in the browser. A request is local when it comes
//! from loopback *and* carries the key. Either alone is not enough: the key
//! from another address is a leak, and loopback without the key is anyone.

use std::io::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{ConnectInfo, Extension, Path as UrlPath, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use subtle::ConstantTimeEq;

use super::access::Caller;
use crate::system::System;

/// The header the menu bar shell and `genatrix open` send the key in.
pub const HEADER: &str = "x-genatrix-local";

/// The cookie a browser on this machine carries the key in.
pub const COOKIE: &str = "genatrix_local";

/// How long a one-time sign-in link lives.
pub const LINK_LIFETIME: Duration = Duration::from_secs(60);

/// The key that makes a request local, and the one-time links that hand it
/// to a browser.
pub struct Local {
    key: String,
    links: Mutex<Vec<(String, Instant)>>,
}

impl Local {
    /// Read the key at `path`, or make one there, readable by this user only.
    pub fn load_or_create(path: &Path) -> std::io::Result<Self> {
        let key = match std::fs::read_to_string(path) {
            Ok(text) if text.trim().len() == 64 => text.trim().to_owned(),
            Ok(_) | Err(_) => {
                let mut bytes = [0u8; 32];
                rand::fill(&mut bytes);
                let key = hex::encode(bytes);
                write_private(path, &key)?;
                key
            }
        };
        Ok(Self {
            key,
            links: Mutex::new(Vec::new()),
        })
    }

    /// The key, for tests and for the terminal's own use.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Whether the request carries the key, in the header or the cookie.
    pub fn presented<B>(&self, request: &axum::http::Request<B>) -> bool {
        let header = request
            .headers()
            .get(HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let cookie = super::access::cookie_value(request.headers(), COOKIE);
        [header, cookie]
            .into_iter()
            .flatten()
            .any(|v| v.as_bytes().ct_eq(self.key.as_bytes()).into())
    }

    /// A fresh one-time link token.
    fn link(&self) -> String {
        let mut bytes = [0u8; 24];
        rand::fill(&mut bytes);
        let token = hex::encode(bytes);
        let mut links = self
            .links
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        links.retain(|(_, until)| *until > now);
        links.push((token.clone(), now + LINK_LIFETIME));
        token
    }

    /// Spend a link token: true once, if it was made here and has not lapsed.
    fn spend(&self, token: &str) -> bool {
        let mut links = self
            .links
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let found = links
            .iter()
            .position(|(t, until)| *until > now && t.as_bytes().ct_eq(token.as_bytes()).into());
        match found {
            Some(i) => {
                links.remove(i);
                true
            }
            None => false,
        }
    }
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())?;
    // `mode` applies only when the file is created; a file left by someone
    // else with wider permissions is narrowed here.
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
}

/// The routes: making a link (this machine only) and following one.
pub fn routes() -> axum::Router<Arc<System>> {
    axum::Router::new()
        .route("/api/local/link", post(make_link))
        .route("/local/{token}", get(follow))
}

/// Make a one-time link that signs a browser on this machine in.
async fn make_link(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
) -> Response {
    if caller != Caller::Local {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "only this machine can make a sign-in link" })),
        )
            .into_response();
    }
    Json(serde_json::json!({
        "token": system.local.link(),
        "expires_in_secs": LINK_LIFETIME.as_secs(),
    }))
    .into_response()
}

/// Follow a link: from loopback, with a live token, the browser gets the key
/// as a cookie and lands on the page. The cookie is `SameSite=Strict`, so
/// another site's page cannot borrow it.
async fn follow(
    State(system): State<Arc<System>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    UrlPath(token): UrlPath<String>,
) -> Response {
    if !peer.ip().is_loopback() || !system.local.spend(&token) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "This link has been used, has expired, or was opened on another machine. Run `genatrix open` again.",
        )
            .into_response();
    }
    let cookie = format!(
        "{COOKIE}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000",
        system.local.key()
    );
    (
        StatusCode::SEE_OTHER,
        [
            (header::SET_COOKIE, cookie),
            (header::LOCATION, "/".to_owned()),
        ],
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_is_made_once_and_readable_by_this_user_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run").join("local.key");
        let first = Local::load_or_create(&path).unwrap();
        assert_eq!(first.key().len(), 64);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let again = Local::load_or_create(&path).unwrap();
        assert_eq!(first.key(), again.key(), "the same key across restarts");
    }

    #[test]
    fn a_link_is_spent_once() {
        let dir = tempfile::tempdir().unwrap();
        let local = Local::load_or_create(&dir.path().join("local.key")).unwrap();
        let token = local.link();
        assert!(!local.spend("not-a-token"));
        assert!(local.spend(&token));
        assert!(!local.spend(&token));
    }
}
