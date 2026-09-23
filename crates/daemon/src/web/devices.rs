//! Pairing a device, and the list of paired devices.
//!
//! Design: `docs/design/06-interface.md`, "形态". A code is made on this
//! machine and lives a few minutes; a phone that presents it gets a device
//! secret in a cookie and is a paired device from then on. Codes are made
//! only from loopback, used once, and there is only ever one live code, so
//! guessing is a six-digit lottery with one ticket per five minutes.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use super::access::{COOKIE, Caller, hash_secret};
use crate::system::System;

/// How long a pairing code is good for.
pub const CODE_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// The one pairing code that may be live.
#[derive(Default)]
pub struct Pairing {
    live: Mutex<Option<(String, Instant)>>,
}

impl Pairing {
    /// Make a fresh code, replacing any live one.
    pub fn start(&self) -> String {
        let code = format!("{:06}", random_u32() % 1_000_000);
        *self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((code.clone(), Instant::now() + CODE_LIFETIME));
        code
    }

    /// Spend the live code if it matches and has not lapsed.
    pub fn take(&self, code: &str) -> bool {
        let mut live = self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((expected, until)) = live.as_ref() else {
            return false;
        };
        let good = expected == code.trim() && Instant::now() < *until;
        // Right or wrong, one try per code: a wrong guess spends it too.
        *live = None;
        good
    }
}

fn random_u32() -> u32 {
    let mut bytes = [0u8; 4];
    fill_random(&mut bytes);
    u32::from_be_bytes(bytes)
}

fn fill_random(bytes: &mut [u8]) {
    use std::io::Read;
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(bytes);
    }
}

#[derive(Serialize)]
struct Started {
    code: String,
    /// Seconds the code is good for.
    expires_in_secs: u64,
    /// Where another device can open the page, first the likeliest. The page
    /// that asks for a code is usually open on 127.0.0.1, which on a phone
    /// means the phone, so the link in the QR code cannot come from it.
    /// Empty when the core listens on loopback only and no device can pair.
    reachable_at: Vec<String>,
}

/// Make a pairing code. Loopback only: pairing starts on the machine.
async fn start(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
    Extension(serving): Extension<super::Serving>,
) -> Response {
    if caller != Caller::Local {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "pairing starts on the machine Genatrix runs on" })),
        )
            .into_response();
    }
    let code = system.pairing.start();
    Json(Started {
        code,
        expires_in_secs: CODE_LIFETIME.as_secs(),
        reachable_at: super::reachable_at(&serving),
    })
    .into_response()
}

#[derive(Deserialize)]
struct PairBody {
    code: String,
    #[serde(default)]
    name: String,
}

#[derive(Serialize)]
struct DeviceView {
    id: String,
    name: String,
    created_at: String,
    last_seen: Option<String>,
    revoked_at: Option<String>,
    /// The device making this request.
    this: bool,
}

/// Trade a code for a device credential. Open to everyone, because the
/// phone has nothing yet; the code is what it has.
async fn pair(State(system): State<Arc<System>>, Json(body): Json<PairBody>) -> Response {
    if !system.pairing.take(&body.code) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "that code is not the live one; make a new one on the machine" })),
        )
            .into_response();
    }
    let mut secret_bytes = [0u8; 32];
    fill_random(&mut secret_bytes);
    let secret = hex::encode(secret_bytes);
    let id = ulid::Ulid::new().to_string();
    let name = body.name.trim();
    let device = genatrix_store::Device {
        id: id.clone(),
        name: if name.is_empty() {
            "a device".to_owned()
        } else {
            name.chars().take(60).collect()
        },
        secret_hash: hash_secret(&secret),
        created_at: chrono::Utc::now().to_rfc3339(),
        last_seen: None,
        revoked_at: None,
    };
    if let Err(e) = system.store.insert_device(&device) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    tracing::info!(device = %device.id, name = %device.name, "a device was paired");
    (
        [(
            header::SET_COOKIE,
            // A year, HttpOnly, and Lax so the page can be opened from a
            // link. No `Secure`: the tunnel is the transport security here,
            // and the page is served over plain HTTP inside it.
            format!("{COOKIE}={id}.{secret}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000"),
        )],
        Json(view(&device, true)),
    )
        .into_response()
}

fn view(d: &genatrix_store::Device, this: bool) -> DeviceView {
    DeviceView {
        id: d.id.clone(),
        name: d.name.clone(),
        created_at: d.created_at.clone(),
        last_seen: d.last_seen.clone(),
        revoked_at: d.revoked_at.clone(),
        this,
    }
}

/// Every paired device, the caller's marked.
async fn list(State(system): State<Arc<System>>, Extension(caller): Extension<Caller>) -> Response {
    let me = match &caller {
        Caller::Device(id) => Some(id.as_str()),
        Caller::Local => None,
    };
    match system.store.all_devices() {
        Ok(devices) => Json(
            devices
                .iter()
                .map(|d| view(d, me == Some(d.id.as_str())))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Revoke a device. From then on its cookie is worth nothing.
async fn revoke(State(system): State<Arc<System>>, Path(id): Path<String>) -> Response {
    match system
        .store
        .revoke_device(&id, &chrono::Utc::now().to_rfc3339())
    {
        Ok(true) => {
            tracing::info!(device = %id, "a device was revoked");
            Json(serde_json::json!({ "revoked": id })).into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no such active device" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// The pairing and device routes.
pub fn routes() -> axum::Router<Arc<System>> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/pair/start", post(start))
        .route("/api/pair", post(pair))
        .route("/api/devices", get(list))
        .route("/api/device/{id}/revoke", post(revoke))
}
