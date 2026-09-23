//! Accounts and models from the Settings page.
//!
//! Design: `docs/design/09-install-recover-migrate.md` ("先落地的是设置页"),
//! `docs/design/05-connectors.md` ("在界面上接入"), `docs/design/04-model-layer.md`
//! ("模型下载"), and `docs/design/02-trust-boundary.md`, invariant 12: adding,
//! signing in to and removing accounts is accepted from loopback only. A
//! paired phone can see how every account is doing; it cannot type a
//! password into a page that may reach the core over plain HTTP.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use genatrix_connector_telegram::login::{LoginFlow, Next};
use serde::{Deserialize, Serialize};

use super::access::Caller;
use crate::setup::{self, Kind};
use crate::system::System;

/// How long a Telegram sign-in may sit between its steps.
const LOGIN_LIFETIME: Duration = Duration::from_secs(10 * 60);

fn refuse(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

/// Invariant 12: credentials are typed on the machine Genatrix runs on.
fn local_only(caller: &Caller) -> Option<Response> {
    (*caller != Caller::Local).then(|| {
        refuse(
            StatusCode::FORBIDDEN,
            "accounts are added and removed on the computer Genatrix runs on",
        )
    })
}

async fn restart_connectors(system: &Arc<System>) {
    if let Err(e) = crate::running::restart(system).await {
        tracing::warn!(error = %e, "the connectors did not start again after an account change");
    }
}

#[derive(Serialize)]
struct AccountView {
    /// `mail` or `telegram`.
    kind: &'static str,
    /// The address or the phone number: what removing it names.
    id: String,
    /// What to show: the address, or the Telegram name and number.
    name: String,
    /// Where it reads from.
    detail: String,
    can_send: bool,
    /// The sync state's name, and the same in one line.
    state: String,
    text: String,
}

#[derive(Serialize)]
struct SetupView {
    /// This request comes from the machine itself, so the forms work.
    local: bool,
    accounts: Vec<AccountView>,
    /// Whether this build can sign in to Telegram at all.
    telegram: bool,
}

async fn view(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<SetupView>, Response> {
    let list = crate::accounts::Accounts::load(&system.config.accounts_path())
        .map_err(|e| refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let states: std::collections::BTreeMap<_, _> = system
        .accounts
        .snapshot()
        .into_iter()
        .map(|a| (a.address.clone(), a))
        .collect();
    let state_of = |id: &str| {
        states.get(id).map_or_else(
            || ("stopped".to_owned(), "not running".to_owned()),
            |a| {
                let name = serde_json::to_value(&a.sync)
                    .ok()
                    .and_then(|v| v.get("state").and_then(|s| s.as_str()).map(str::to_owned))
                    .unwrap_or_default();
                (name, a.text.clone())
            },
        )
    };
    let mut accounts = Vec::new();
    for a in list.mail.values() {
        let (state, text) = state_of(&a.address);
        accounts.push(AccountView {
            kind: "mail",
            id: a.address.clone(),
            name: a.address.clone(),
            detail: format!("{}:{}", a.imap_host, a.imap_port),
            can_send: a.smtp_host.is_some(),
            state,
            text,
        });
    }
    for a in list.telegram.values() {
        let (state, text) = state_of(&a.phone);
        accounts.push(AccountView {
            kind: "telegram",
            id: a.phone.clone(),
            name: if a.name.is_empty() {
                a.phone.clone()
            } else {
                format!("{} · {}", a.name, a.phone)
            },
            detail: "Telegram".to_owned(),
            can_send: true,
            state,
            text,
        });
    }
    Ok(Json(SetupView {
        local: caller == Caller::Local,
        accounts,
        telegram: genatrix_connector_telegram::Credentials::available(),
    }))
}

#[derive(Deserialize)]
struct MailBody {
    address: String,
    password: String,
    #[serde(default)]
    imap_host: Option<String>,
}

/// Add a mailbox: tried against its server, kept only if that worked, and
/// the mail connector started again with it.
async fn add_mail(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<MailBody>,
) -> Response {
    if let Some(no) = local_only(&caller) {
        return no;
    }
    let address = body.address.trim().to_lowercase();
    if !address.contains('@') {
        return refuse(
            StatusCode::BAD_REQUEST,
            "that does not look like an email address",
        );
    }
    let host = body
        .imap_host
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty());
    if host.is_none() && crate::accounts::known_servers(&address).is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Genatrix does not know this provider's mail server; enter its IMAP server",
                "needs_server": true,
            })),
        )
            .into_response();
    }
    match setup::add_mail(&system, &address, host, &body.password).await {
        Ok(account) => {
            tracing::info!(account = %account.address, "a mailbox was added from the page");
            restart_connectors(&system).await;
            Json(serde_json::json!({ "added": account.address })).into_response()
        }
        Err(e) => refuse(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Deserialize)]
struct PhoneBody {
    phone: String,
}

#[derive(Deserialize)]
struct StepBody {
    flow: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    password: String,
}

/// Start a Telegram sign-in: Telegram sends a code to the phone.
async fn telegram_start(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<PhoneBody>,
) -> Response {
    if let Some(no) = local_only(&caller) {
        return no;
    }
    let phone: String = body
        .phone
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    if !phone.starts_with('+') || phone.len() < 8 {
        return refuse(
            StatusCode::BAD_REQUEST,
            "the number needs its country code, as in +64 21 ...",
        );
    }
    let credentials = match genatrix_connector_telegram::Credentials::find() {
        Ok(c) => c,
        Err(e) => return refuse(StatusCode::BAD_REQUEST, e.to_string()),
    };
    let flow = match LoginFlow::start(&credentials, &phone).await {
        Ok(f) => f,
        Err(e) => return refuse(StatusCode::BAD_REQUEST, e.to_string()),
    };
    let id = ulid::Ulid::new().to_string();
    let mut logins = system.logins.lock().await;
    logins.retain(|_, (at, _)| at.elapsed() < LOGIN_LIFETIME);
    logins.insert(id.clone(), (Instant::now(), flow));
    Json(serde_json::json!({ "flow": id })).into_response()
}

/// Take a sign-in out of the table for one step. Put back while it goes on.
async fn take_flow(system: &System, id: &str) -> Option<(Instant, LoginFlow)> {
    let mut logins = system.logins.lock().await;
    logins.retain(|_, (at, _)| at.elapsed() < LOGIN_LIFETIME);
    logins.remove(id)
}

async fn signed_in(
    system: &Arc<System>,
    phone: &str,
    signed: &genatrix_connector_telegram::login::SignedIn,
) -> Response {
    if let Err(e) = setup::keep_telegram(system, phone, signed) {
        return refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    tracing::info!(%phone, "a Telegram account was signed in from the page");
    restart_connectors(system).await;
    Json(serde_json::json!({ "done": true, "name": signed.name })).into_response()
}

/// The code Telegram sent. Either done, or a password is wanted.
async fn telegram_code(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<StepBody>,
) -> Response {
    if let Some(no) = local_only(&caller) {
        return no;
    }
    let Some((at, mut flow)) = take_flow(&system, &body.flow).await else {
        return refuse(StatusCode::GONE, "this sign-in has lapsed; start again");
    };
    match flow.code(&body.code).await {
        Ok(Next::Done(signed)) => {
            let phone = flow.phone().to_owned();
            signed_in(&system, &phone, &signed).await
        }
        Ok(Next::Password { hint }) => {
            system.logins.lock().await.insert(body.flow, (at, flow));
            Json(serde_json::json!({ "password": true, "hint": hint })).into_response()
        }
        Err(e) => {
            system.logins.lock().await.insert(body.flow, (at, flow));
            refuse(StatusCode::BAD_REQUEST, e.to_string())
        }
    }
}

/// The two-step verification password.
async fn telegram_password(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<StepBody>,
) -> Response {
    if let Some(no) = local_only(&caller) {
        return no;
    }
    let Some((at, mut flow)) = take_flow(&system, &body.flow).await else {
        return refuse(StatusCode::GONE, "this sign-in has lapsed; start again");
    };
    match flow.password(&body.password).await {
        Ok(signed) => {
            let phone = flow.phone().to_owned();
            signed_in(&system, &phone, &signed).await
        }
        Err(e) => {
            system.logins.lock().await.insert(body.flow, (at, flow));
            refuse(StatusCode::BAD_REQUEST, e.to_string())
        }
    }
}

#[derive(Deserialize)]
struct RemoveBody {
    kind: Kind,
    id: String,
}

/// Remove an account. What it fetched stays.
async fn remove(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<RemoveBody>,
) -> Response {
    if let Some(no) = local_only(&caller) {
        return no;
    }
    match setup::remove(&system, body.kind, &body.id) {
        Ok(removed) => {
            if removed {
                tracing::info!(account = %body.id, "an account was removed from the page");
                restart_connectors(&system).await;
            }
            Json(serde_json::json!({ "removed": removed })).into_response()
        }
        Err(e) => refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Serialize)]
struct ModelView {
    id: &'static str,
    role: &'static str,
    repo: &'static str,
    size: u64,
    present: u64,
}

#[derive(Serialize)]
struct ModelPage {
    local: bool,
    models: Vec<ModelView>,
    total: u64,
    present: u64,
    free: Option<u64>,
    /// The model side's state, as the status line says it.
    state: String,
    ready: bool,
    download: crate::download::Progress,
}

async fn models(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
) -> Json<ModelPage> {
    let config = &system.config;
    let models: Vec<ModelView> = crate::download::CATALOG
        .iter()
        .map(|m| ModelView {
            id: m.id,
            role: m.role,
            repo: m.repo,
            size: m.size(),
            present: m.present(config),
        })
        .collect();
    let state = system.model_state.borrow().clone();
    Json(ModelPage {
        local: caller == Caller::Local,
        total: models.iter().map(|m| m.size).sum(),
        present: models.iter().map(|m| m.present).sum(),
        models,
        free: crate::download::free_space(&config.data_dir),
        ready: state.is_ready(),
        state: state.describe(),
        download: system
            .download
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    })
}

/// Fetch whatever of the catalog is missing.
async fn download(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
) -> Response {
    if let Some(no) = local_only(&caller) {
        return no;
    }
    match crate::download::start(&system) {
        Ok(()) => Json(serde_json::json!({ "started": true })).into_response(),
        Err(e) => refuse(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

/// The routes.
pub fn routes() -> axum::Router<Arc<System>> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/setup", get(view))
        .route("/api/setup/mail", post(add_mail))
        .route("/api/setup/telegram/start", post(telegram_start))
        .route("/api/setup/telegram/code", post(telegram_code))
        .route("/api/setup/telegram/password", post(telegram_password))
        .route("/api/setup/remove", post(remove))
        .route("/api/model", get(models))
        .route("/api/model/download", post(download))
}
