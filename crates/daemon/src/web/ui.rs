//! The shell and the interfaces on the wire (design 15).
//!
//! `/` and every other page path is the shell: the trusted top-level page,
//! the same for everyone, compiled in. It loads this device's interface in
//! a sandboxed frame from `/ui/<hash>/`. Those files are served to anyone,
//! like the page always was: they are code, addressed by hash, and hold no
//! data. Their headers keep them in the sandbox even when opened on their
//! own: no origin, no network but this core's own files, no navigation, no
//! forms going anywhere.
//!
//! `/api/ui` is the shell's: which interface this device uses, the others,
//! and their versions. The shell never passes these requests on for the
//! interface (web/src/lib/gate.ts).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;

use super::access::Caller;
use crate::system::System;
use crate::ui;

type Shared = State<Arc<System>>;

const SHELL_HTML: &str = include_str!("dist/shell/shell.html");
const SHELL_JS: &str = include_str!("dist/shell/shell.js");
const SHELL_CSS: &str = include_str!("dist/shell/shell.css");
const MANIFEST: &str = include_str!("dist/shell/manifest.webmanifest");
const ICON_SVG: &str = include_str!("dist/shell/favicon.svg");
const ICON_32: &[u8] = include_bytes!("dist/shell/favicon-32.png");
const ICON_180: &[u8] = include_bytes!("dist/shell/icon-180.png");
const ICON_512: &[u8] = include_bytes!("dist/shell/icon-512.png");

/// The shell's policy: its own scripts and styles, requests to this core,
/// and frames from this core only.
pub const SHELL_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
     img-src 'self' data: blob:; font-src 'self'; connect-src 'self'; frame-src 'self'; \
     worker-src 'none'; object-src 'none'; form-action 'self'; base-uri 'none'; \
     frame-ancestors 'none'";

/// An interface's policy (invariant 35). Sandboxed even when opened on its
/// own; its own files and nothing else; no connections, frames, forms that
/// leave, or framing by anything but the shell. `here` is this core's
/// origin as the browser addressed it, named beside `'self'` because a
/// sandboxed document has no origin of its own to compare with.
#[must_use]
pub fn frame_csp(here: &str) -> String {
    format!(
        "default-src 'none'; script-src 'self' {here}; style-src 'self' {here} 'unsafe-inline'; \
         img-src 'self' {here} data: blob:; font-src 'self' {here}; media-src 'self' {here} data: blob:; \
         connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; \
         manifest-src 'none'; form-action 'none'; base-uri 'none'; frame-ancestors 'self' {here}; \
         sandbox allow-scripts allow-forms"
    )
}

/// The routes.
pub fn routes() -> axum::Router<Arc<System>> {
    axum::Router::new()
        .route("/shell.js", get(script))
        .route("/shell.css", get(style))
        .route("/manifest.webmanifest", get(manifest))
        .route("/favicon.svg", get(icon))
        .route("/favicon.ico", get(icon_32))
        .route("/icon-180.png", get(icon_180))
        .route("/icon-512.png", get(icon_512))
        .route("/ui/{hash}/{*path}", get(frame_file))
        .route("/api/ui", get(state))
        .route("/api/ui/choose", post(choose))
        .route("/api/ui/copy", post(copy))
        .route("/api/ui/approve", post(approve))
        .route("/api/ui/change", post(change))
}

/// The shell, for every page path.
pub async fn shell() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            (header::CONTENT_SECURITY_POLICY, SHELL_CSP),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        SHELL_HTML,
    )
}

async fn script() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        SHELL_JS,
    )
}

async fn style() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        SHELL_CSS,
    )
}

async fn manifest() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        MANIFEST,
    )
}

async fn icon() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/svg+xml")], ICON_SVG)
}

/// For browsers that ask for `/favicon.ico` on their own. A PNG under that
/// name is read the same.
async fn icon_32() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/png")], ICON_32)
}

/// For a phone's home screen, which wants a picture, not a drawing.
async fn icon_180() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/png")], ICON_180)
}

async fn icon_512() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/png")], ICON_512)
}

/// This core's origin as the browser addressed it, for the frame's policy.
/// Only a plain host and port is believed; anything else leaves `'self'`
/// alone to do the work.
fn origin_of(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    let plain = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b));
    if plain {
        format!("http://{host} https://{host}")
    } else {
        String::new()
    }
}

async fn frame_file(
    State(system): Shared,
    headers: HeaderMap,
    Path((hash, path)): Path<(String, String)>,
) -> Response {
    let not_found = || (StatusCode::NOT_FOUND, "no such file").into_response();
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return not_found();
    }
    let Some(kind) = ui::content_type(&path) else {
        return not_found();
    };
    let bytes = match ui::file(&system, &hash, &path) {
        Ok(Some(b)) => b,
        Ok(None) => return not_found(),
        Err(e) => {
            tracing::warn!(error = %e, "an interface file could not be read");
            return (StatusCode::INTERNAL_SERVER_ERROR, "could not read it").into_response();
        }
    };
    let mut response = bytes.into_response();
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    // The address is the content: it never changes.
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    // The sandboxed frame has no origin; its scripts and styles are asked
    // for across origins, and get no cookies either way.
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    if let Ok(csp) = HeaderValue::from_str(&frame_csp(&origin_of(&headers))) {
        h.insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    response
}

fn device_key(caller: &Caller) -> String {
    match caller {
        Caller::Local => "local".to_owned(),
        Caller::Device(id) => id.clone(),
    }
}

fn refuse(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

fn broke(e: &anyhow::Error) -> Response {
    refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// What the shell needs: what this device loads, and for the panel, every
/// interface, its versions and who uses it.
async fn state(State(system): Shared, Extension(caller): Extension<Caller>) -> Response {
    match state_of(&system, &device_key(&caller)) {
        Ok(v) => Json(v).into_response(),
        Err(e) => broke(&e),
    }
}

fn state_of(system: &System, device: &str) -> anyhow::Result<serde_json::Value> {
    let (using, load) = ui::load_for(system, device)?;
    let chosen = system.store.device_ui(device)?.is_some();
    let picks = system.store.device_uis()?;
    let used_by = |name: &str| -> Vec<String> {
        picks
            .iter()
            .filter(|(_, u)| u == name)
            .map(|(d, _)| d.clone())
            .collect()
    };
    let stored = system.store.uis()?;
    let active: Vec<String> = std::iter::once("local".to_owned())
        .chain(
            system
                .store
                .all_devices()?
                .into_iter()
                .filter(|d| d.revoked_at.is_none())
                .map(|d| d.id),
        )
        .collect();
    // A device that picked nothing, or an interface no longer there, runs
    // the built-in one.
    let runs_builtin = |d: &String| {
        picks.iter().find(|(id, _)| id == d).is_none_or(|(_, u)| {
            u == ui::DEFAULT || !stored.iter().any(|s| &s.name == u && s.current.is_some())
        })
    };
    let mut uis = vec![serde_json::json!({
        "name": ui::DEFAULT, "title": "", "builtin": true, "made_by": "",
        "devices": active.iter().filter(|d| runs_builtin(d)).cloned().collect::<Vec<_>>(),
        "versions": [],
    })];
    for u in &stored {
        let versions: Vec<_> = system
            .store
            .ui_versions(&u.name)?
            .into_iter()
            .map(|v| {
                serde_json::json!({
                    "current": u.current.as_deref() == Some(v.hash.as_str()),
                    "hash": v.hash, "note": v.note, "created_at": v.created_at,
                    "approved_at": v.approved_at,
                })
            })
            .collect();
        uis.push(serde_json::json!({
            "name": u.name, "title": u.title, "builtin": false, "made_by": u.made_by,
            "devices": used_by(&u.name), "versions": versions,
            "project": ui::has_project(system, &u.name).then(|| ui::project_name(&u.name)),
        }));
    }
    let mut devices = vec![serde_json::json!({
        "id": "local", "name": "", "this": device == "local",
        "ui": picks.iter().find(|(d, _)| d == "local").map(|(_, u)| u.clone()),
    })];
    for d in system.store.all_devices()? {
        if d.revoked_at.is_some() {
            continue;
        }
        devices.push(serde_json::json!({
            "id": d.id, "name": d.name, "this": d.id == device,
            "ui": picks.iter().find(|(id, _)| *id == d.id).map(|(_, u)| u.clone()),
        }));
    }
    Ok(serde_json::json!({
        "device": device, "using": using, "load": load, "chosen": chosen,
        "default_hash": ui::default_hash(), "api": ui::API_VERSION,
        "others": stored.len(), "uis": uis, "devices": devices,
        "deps_ready": ui::deps_ready(system),
    }))
}

#[derive(Deserialize)]
struct Choose {
    ui: String,
}

/// Pick the interface this device uses: one already approved, so nothing
/// new runs that was not approved (invariant 37).
async fn choose(
    State(system): Shared,
    Extension(caller): Extension<Caller>,
    Json(body): Json<Choose>,
) -> Response {
    if body.ui != ui::DEFAULT {
        match system.store.ui(&body.ui) {
            Ok(Some(u)) if u.current.is_some() => {}
            Ok(Some(_)) => {
                return refuse(
                    StatusCode::CONFLICT,
                    "that interface has no approved version yet",
                );
            }
            Ok(None) => return refuse(StatusCode::NOT_FOUND, "there is no such interface"),
            Err(e) => return broke(&e.into()),
        }
    }
    match system.store.set_device_ui(&device_key(&caller), &body.ui) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => broke(&e.into()),
    }
}

#[derive(Deserialize)]
struct Copy {
    from: String,
    title: String,
}

/// Copy an interface, to change it on its own.
async fn copy(
    State(system): Shared,
    Extension(caller): Extension<Caller>,
    Json(body): Json<Copy>,
) -> Response {
    match ui::copy(&system, &body.from, &body.title, &device_key(&caller)) {
        Ok(name) => Json(serde_json::json!({ "name": name })).into_response(),
        Err(e) => refuse(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Deserialize)]
struct Approve {
    ui: String,
    hash: String,
}

/// Approve a version, or go back to one approved before. When other
/// devices use the interface, it changes what runs on them: a high-privilege
/// request (design 15, ruling 2).
async fn approve(
    State(system): Shared,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    Json(body): Json<Approve>,
) -> Response {
    match system.store.ui_version(&body.ui, &body.hash) {
        Ok(Some(_)) => {}
        Ok(None) => return refuse(StatusCode::NOT_FOUND, "there is no such version"),
        Err(e) => return broke(&e.into()),
    }
    let device = device_key(&caller);
    let others = match ui::others_using(&system, &body.ui, &device) {
        Ok(o) => o,
        Err(e) => return broke(&e),
    };
    if !others.is_empty()
        && let Err(no) = super::admin::high(
            &system,
            &caller,
            &headers,
            "approve_ui",
            &body.ui,
            "other devices use this interface: approve its new version on the computer Genatrix runs on, or from an admin device",
        )
        .await
    {
        return no;
    }
    let version = match system.store.ui_version(&body.ui, &body.hash) {
        Ok(v) => v,
        Err(e) => return broke(&e.into()),
    };
    if let Err(e) = system.store.approve_ui_version(
        &body.ui,
        &body.hash,
        &device,
        &chrono::Utc::now().to_rfc3339(),
    ) {
        return broke(&e.into());
    }
    // The source follows what is approved, so going back to an earlier
    // version is where the next change starts (design 15 v0.4).
    if let Some(commit) = version.and_then(|v| v.source_commit)
        && let Err(e) = ui::move_source(&system, &body.ui, &commit)
    {
        tracing::warn!(ui = %body.ui, error = %e, "the source could not follow the approved version");
    }
    Json(serde_json::json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
struct Change {
    ui: String,
    task: String,
    /// Where the person is as they ask: a path, and what the page is.
    #[serde(default)]
    page: Option<String>,
}

/// Ask for a change to an interface (design 15, "让 AI 改界面"): a work card
/// on its source project, waiting for approval like any other. When the
/// work is done, its build waits here as a new version.
async fn change(State(system): Shared, Json(body): Json<Change>) -> Response {
    if !ui::has_project(&system, &body.ui) {
        return refuse(
            StatusCode::CONFLICT,
            "this interface has no source to change; copy the built-in one, or one that has",
        );
    }
    // An interface made by an earlier core builds the way this one does.
    if let Err(e) = ui::refresh_source(&system, &body.ui) {
        tracing::warn!(ui = %body.ui, error = %e, "the interface's own files could not be brought up to date");
    }
    let task = match body
        .page
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        Some(page) => format!(
            "{}\n\n(Asked while looking at {page}, in this interface.)",
            body.task.trim()
        ),
        None => body.task,
    };
    let request = crate::executors::draft::Request {
        project: ui::project_name(&body.ui),
        recipe: "change-interface".into(),
        task,
        public: false,
        source: None,
        assign: std::collections::BTreeMap::new(),
    };
    match crate::executors::draft::draft(&system, &request) {
        Ok(action) => Json(serde_json::json!({ "action": action.id })).into_response(),
        Err(crate::executors::draft::DraftError::Refused(why)) => {
            refuse(StatusCode::BAD_REQUEST, why)
        }
        Err(crate::executors::draft::DraftError::Other(e)) => broke(&e),
    }
}
