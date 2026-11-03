//! Signing in as the user: the device flow, refreshing, and who it is.
//!
//! Design: `docs/design/05-connectors.md`, "应用与授权：一处例外". The
//! device flow (RFC 8628) needs no address to call back to: Genatrix asks
//! for a short code and a link, the user opens the link on any device and
//! confirms, and the token endpoint answers the next poll with a sign-in.
//!
//! Refresh tokens are spent on use: every refresh returns a new one and the
//! old one stops working at once. A [`Token`] is therefore not a password
//! but a session that moves, and it is kept the way a Telegram session is.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::brand::Brand;

/// What the app is asked for. Read only; `offline_access` is what brings a
/// refresh token. Names take two: `contact:contact.base:readonly` lets the
/// user lookup be called at all, `contact:user.base:readonly` puts the name
/// in its answer.
pub const SCOPES: &str = "task:task:read task:comment:read task:tasklist:read \
                          contact:contact.base:readonly contact:user.base:readonly \
                          offline_access";

/// The two permissions a sign-in needs for people's names.
pub const NAME_SCOPES: [&str; 2] = [
    "contact:contact.base:readonly",
    "contact:user.base:readonly",
];

/// The app the user's company created, and which platform it is on. What
/// the core hands the connector as an account's secret.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secret {
    /// The app's id, `cli_...`.
    pub app_id: String,
    /// The app's secret.
    pub app_secret: String,
    /// Lark or Feishu.
    #[serde(default)]
    pub brand: Brand,
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secret")
            .field("app_id", &self.app_id)
            .field("brand", &self.brand)
            .finish_non_exhaustive()
    }
}

/// A sign-in: the access token and the refresh token that replaces it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    /// Sent with every call.
    pub access_token: String,
    /// When it stops working, milliseconds since the epoch.
    pub access_expires_ms: i64,
    /// Spent on the next refresh.
    pub refresh_token: String,
    /// When it stops working; zero when none was given.
    pub refresh_expires_ms: i64,
    /// What the user granted.
    #[serde(default)]
    pub scope: String,
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Token")
            .field("access_expires_ms", &self.access_expires_ms)
            .field("refresh_expires_ms", &self.refresh_expires_ms)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl Token {
    /// Read a token endpoint's answer.
    fn from_answer(body: &Value, now_ms: i64) -> Option<Self> {
        let access_token = body.get("access_token")?.as_str()?.to_owned();
        let seconds = |key: &str| body.get(key).and_then(Value::as_i64);
        let refresh_token = body
            .get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Some(Self {
            access_token,
            access_expires_ms: now_ms + seconds("expires_in").unwrap_or(7200) * 1000,
            refresh_expires_ms: if refresh_token.is_empty() {
                0
            } else {
                now_ms + seconds("refresh_token_expires_in").unwrap_or(0) * 1000
            },
            refresh_token,
            scope: body
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        })
    }

    /// Whether the access token should be replaced before the next call:
    /// it runs out within five minutes.
    #[must_use]
    pub const fn stale(&self, now_ms: i64) -> bool {
        self.access_expires_ms - now_ms < 5 * 60 * 1000
    }
}

/// What went wrong signing in or staying signed in.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// Only signing in again will help.
    #[error("{0}")]
    SignInAgain(String),
    /// It may pass: the network, the server.
    #[error("{0}")]
    Transient(String),
}

pub(crate) fn agent() -> ureq::Agent {
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

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn reason(body: &Value) -> String {
    body.get("error_description")
        .or_else(|| body.get("msg"))
        .or_else(|| body.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("no reason given")
        .to_owned()
}

/// A device sign-in waiting for the user.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pending {
    /// What the poll sends; not for the user.
    pub device_code: String,
    /// The short code the user confirms.
    pub user_code: String,
    /// Where the user goes.
    pub verification_uri: String,
    /// The same, with the code filled in.
    pub verification_uri_complete: String,
    /// Seconds between polls.
    pub interval: u64,
    /// When the code runs out, milliseconds since the epoch.
    pub expires_ms: i64,
}

/// Ask for a device code and a link for the user.
pub fn start_device(secret: &Secret) -> Result<Pending, AuthError> {
    let url = format!(
        "https://{}/oauth/v1/device_authorization",
        secret.brand.accounts_host()
    );
    let basic = base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{}", secret.app_id, secret.app_secret));
    let response = agent()
        .post(&url)
        .header("Authorization", &format!("Basic {basic}"))
        .send_form([("client_id", secret.app_id.as_str()), ("scope", SCOPES)])
        .map_err(|e| AuthError::Transient(format!("could not reach Lark: {e}")))?;
    let (status, body) = read(response);
    let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_owned);
    match (status, text("device_code"), text("user_code")) {
        (200, Some(device_code), Some(user_code)) => {
            let verification_uri = text("verification_uri").unwrap_or_default();
            Ok(Pending {
                device_code,
                user_code,
                verification_uri_complete: text("verification_uri_complete")
                    .unwrap_or_else(|| verification_uri.clone()),
                verification_uri,
                interval: body.get("interval").and_then(Value::as_u64).unwrap_or(5),
                expires_ms: now_ms()
                    + body
                        .get("expires_in")
                        .and_then(Value::as_i64)
                        .unwrap_or(240)
                        * 1000,
            })
        }
        _ => Err(AuthError::SignInAgain(format!(
            "Lark would not start a sign-in ({status}): {}. Check the app id and secret, \
             and that the app is published with the permissions design 05 lists.",
            reason(&body)
        ))),
    }
}

/// How one poll went.
#[derive(Debug)]
pub enum Poll {
    /// The user has not confirmed yet.
    Waiting,
    /// Poll less often.
    SlowDown,
    /// Signed in.
    Done(Token),
    /// The user said no, or the code ran out.
    Over(String),
}

/// Ask once whether the user has confirmed.
pub fn poll_device(secret: &Secret, pending: &Pending) -> Result<Poll, AuthError> {
    if now_ms() > pending.expires_ms {
        return Ok(Poll::Over("the code ran out; start again".into()));
    }
    let url = format!(
        "https://{}/open-apis/authen/v2/oauth/token",
        secret.brand.open_host()
    );
    let response = agent()
        .post(&url)
        .send_form([
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", pending.device_code.as_str()),
            ("client_id", secret.app_id.as_str()),
            ("client_secret", secret.app_secret.as_str()),
        ])
        .map_err(|e| AuthError::Transient(format!("could not reach Lark: {e}")))?;
    let (_, body) = read(response);
    Ok(read_poll(&body, now_ms()))
}

fn read_poll(body: &Value, now_ms: i64) -> Poll {
    if let Some(token) = Token::from_answer(body, now_ms) {
        return Poll::Done(token);
    }
    match body.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => Poll::Waiting,
        Some("slow_down") => Poll::SlowDown,
        Some("access_denied") => Poll::Over("the sign-in was declined".into()),
        Some("expired_token" | "invalid_grant") => {
            Poll::Over("the code ran out; start again".into())
        }
        _ => Poll::Over(reason(body)),
    }
}

/// Spend the refresh token for a new sign-in.
pub fn refresh(secret: &Secret, token: &Token) -> Result<Token, AuthError> {
    let now = now_ms();
    if token.refresh_token.is_empty() {
        return Err(AuthError::SignInAgain(
            "the sign-in cannot be renewed (no offline_access); sign in again".into(),
        ));
    }
    if token.refresh_expires_ms != 0 && token.refresh_expires_ms < now {
        return Err(AuthError::SignInAgain(
            "the sign-in ran out while this machine was away; sign in again".into(),
        ));
    }
    let url = format!(
        "https://{}/open-apis/authen/v2/oauth/token",
        secret.brand.open_host()
    );
    let response = agent()
        .post(&url)
        .send_json(json!({
            "grant_type": "refresh_token",
            "client_id": secret.app_id,
            "client_secret": secret.app_secret,
            "refresh_token": token.refresh_token,
        }))
        .map_err(|e| AuthError::Transient(format!("could not reach Lark: {e}")))?;
    let (status, body) = read(response);
    read_refresh(status, &body, now)
}

fn read_refresh(status: u16, body: &Value, now_ms: i64) -> Result<Token, AuthError> {
    if let Some(token) = Token::from_answer(body, now_ms) {
        return Ok(token);
    }
    let code = body.get("code").and_then(Value::as_i64).unwrap_or_default();
    if status >= 500 || matches!(code, 20050 | 20072) {
        return Err(AuthError::Transient(format!(
            "Lark could not renew the sign-in just now: {}",
            reason(body)
        )));
    }
    Err(AuthError::SignInAgain(format!(
        "Lark would not renew the sign-in ({code}): {}; sign in again",
        reason(body)
    )))
}

/// Who signed in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Me {
    /// The user's id for this app.
    pub open_id: String,
    /// Their name.
    pub name: String,
    /// Their email, when Lark gives it.
    pub email: Option<String>,
}

/// Ask Lark who the token belongs to.
pub fn whoami(brand: Brand, token: &Token) -> Result<Me, AuthError> {
    let response = agent()
        .get(&format!("{}/authen/v1/user_info", brand.api()))
        .header("Authorization", &format!("Bearer {}", token.access_token))
        .call()
        .map_err(|e| AuthError::Transient(format!("could not reach Lark: {e}")))?;
    let (status, body) = read(response);
    let data = body.get("data").cloned().unwrap_or(Value::Null);
    let text = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    match text("open_id") {
        Some(open_id) => Ok(Me {
            open_id,
            name: text("name").unwrap_or_default(),
            email: text("enterprise_email").or_else(|| text("email")),
        }),
        None => Err(AuthError::SignInAgain(format!(
            "Lark did not say who this is ({status}): {}",
            reason(&body)
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_answer_becomes_a_token_with_absolute_times() {
        let t = Token::from_answer(
            &json!({"access_token": "a", "expires_in": 7200, "refresh_token": "r",
                    "refresh_token_expires_in": 604_800, "scope": "task:task:read"}),
            1_000,
        )
        .unwrap();
        assert_eq!(t.access_expires_ms, 1_000 + 7_200_000);
        assert_eq!(t.refresh_expires_ms, 1_000 + 604_800_000);
        assert!(!t.stale(1_000));
        assert!(
            t.stale(1_000 + 7_200_000 - 60_000),
            "renewed before it runs out"
        );
        let shown = format!("{t:?}");
        assert!(
            !shown.contains("\"a\"") && !shown.contains("\"r\""),
            "the debug form does not print the tokens: {shown}"
        );
    }

    #[test]
    fn polling_tells_waiting_from_done_from_over() {
        assert!(matches!(
            read_poll(&json!({"error": "authorization_pending"}), 0),
            Poll::Waiting
        ));
        assert!(matches!(
            read_poll(&json!({"error": "slow_down"}), 0),
            Poll::SlowDown
        ));
        assert!(matches!(
            read_poll(&json!({"error": "access_denied"}), 0),
            Poll::Over(_)
        ));
        assert!(matches!(
            read_poll(&json!({"access_token": "a", "refresh_token": "r"}), 0),
            Poll::Done(_)
        ));
    }

    #[test]
    fn a_spent_or_revoked_refresh_token_means_signing_in_again_and_an_outage_does_not() {
        for code in [20037, 20064, 20073, 20026] {
            assert!(matches!(
                read_refresh(400, &json!({"code": code, "error": "invalid_grant"}), 0),
                Err(AuthError::SignInAgain(_))
            ));
        }
        assert!(matches!(
            read_refresh(503, &json!({"code": 20072}), 0),
            Err(AuthError::Transient(_))
        ));
        assert!(read_refresh(200, &json!({"code": 0, "access_token": "n"}), 0).is_ok());
    }

    #[test]
    fn the_secret_is_not_printed() {
        let s = Secret {
            app_id: "cli_1".into(),
            app_secret: "hush".into(),
            brand: Brand::Lark,
        };
        assert!(!format!("{s:?}").contains("hush"));
    }
}
