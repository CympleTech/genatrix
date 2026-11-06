//! High-privilege requests: from this machine, or from an admin device with
//! the admin passphrase each time.
//!
//! Design: `docs/design/06-interface.md` v0.7, "设备分两种"; design 02,
//! invariants 11 and 12. The admin role is granted on this machine only. The
//! passphrase is set on this machine, travels in the one request that needs
//! it and nowhere else, and the core keeps a slow hash of it. Five wrong
//! tries in a row lock remote administration until this machine unlocks it.
//! Every high-privilege request, and every wrong passphrase, goes into the
//! ledger, where every device sees it (the page's banner and Records).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use genatrix_ledger::{EntryFilter, kind};
use genatrix_store::ADMIN_TRIES;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq as _;

use super::access::Caller;
use crate::system::System;

/// The header a high-privilege request from an admin device carries the
/// passphrase in. Not a cookie: the browser never keeps it.
pub const HEADER: &str = "x-genatrix-passphrase";

/// PBKDF2-HMAC-SHA256 rounds. Tests use few, so they stay quick.
const ROUNDS: u32 = if cfg!(test) { 1_000 } else { 600_000 };

/// The shortest passphrase accepted.
pub const MIN_LEN: usize = 10;

/// One passphrase check at a time, and the latest high-privilege request,
/// which every page asks for every few seconds.
#[derive(Default)]
pub struct AdminGate {
    checking: tokio::sync::Mutex<()>,
    /// Read from the ledger once, then kept as requests are recorded.
    last: std::sync::Mutex<Last>,
}

/// The latest high-privilege request, once the ledger has been read.
#[derive(Default)]
enum Last {
    #[default]
    Unread,
    Read(Option<AdminLine>),
}

/// The stored form: `pbkdf2-sha256$<rounds>$<salt hex>$<hash hex>`.
pub fn hash_passphrase(passphrase: &str) -> String {
    let mut salt = [0u8; 16];
    rand::fill(&mut salt);
    let hash = derive(passphrase, &salt, ROUNDS);
    format!(
        "pbkdf2-sha256${ROUNDS}${}${}",
        hex::encode(salt),
        hex::encode(hash)
    )
}

fn derive(passphrase: &str, salt: &[u8], rounds: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(passphrase.as_bytes(), salt, rounds, &mut out);
    out
}

/// Whether a passphrase matches its stored form.
pub fn verify(passphrase: &str, stored: &str) -> bool {
    let mut parts = stored.split('$');
    let (Some("pbkdf2-sha256"), Some(rounds), Some(salt), Some(hash), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return false;
    };
    let (Ok(rounds), Ok(salt), Ok(hash)) =
        (rounds.parse::<u32>(), hex::decode(salt), hex::decode(hash))
    else {
        return false;
    };
    derive(passphrase, &salt, rounds).ct_eq(&hash).into()
}

/// What the ledger keeps of one high-privilege request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdminRecord {
    /// What was asked, as a code the page puts in words: `add_mailbox`,
    /// `start_telegram`, `start_lark`, `remove_account`, `install_agent`,
    /// `uninstall_agent`, `pair_code`, `file_definition`, `set_passphrase`,
    /// `unlock`, `grant_admin`, `ungrant_admin`.
    pub act: String,
    /// What it was about: an address, an agent, a kind of definition.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    /// "this machine", or the device's name.
    pub by: String,
    /// The device's id, when it was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// `allowed`, `wrong_passphrase` or `locked`.
    pub outcome: String,
}

fn record(system: &System, record: &AdminRecord) {
    match system.ledger.append(
        kind::ADMIN,
        record.device.as_deref().unwrap_or("here"),
        record,
    ) {
        Ok(entry) => {
            *system
                .admin
                .last
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Last::Read(Some(AdminLine {
                seq: entry.seq,
                at: entry.at.to_rfc3339(),
                record: record.clone(),
            }));
        }
        Err(e) => tracing::warn!(error = %e, "a high-privilege request could not be recorded"),
    }
    tracing::info!(act = %record.act, subject = %record.subject, by = %record.by, outcome = %record.outcome, "high-privilege request");
}

/// One line of the record, as the page shows it.
#[derive(Clone, Debug, Serialize)]
pub struct AdminLine {
    pub seq: i64,
    pub at: String,
    #[serde(flatten)]
    pub record: AdminRecord,
}

/// The latest high-privilege request, for the banner every device shows.
pub fn last(system: &System) -> Option<AdminLine> {
    let mut last = system
        .admin
        .last
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Last::Unread = *last {
        *last = Last::Read(recent(system, 1).into_iter().next());
    }
    match &*last {
        Last::Read(line) => line.clone(),
        Last::Unread => None,
    }
}

/// The latest high-privilege requests, newest first.
pub fn recent(system: &System, n: usize) -> Vec<AdminLine> {
    let entries = system
        .ledger
        .entries(&EntryFilter {
            kind: Some(kind::ADMIN.into()),
            ..Default::default()
        })
        .unwrap_or_default();
    entries
        .iter()
        .rev()
        .filter_map(|e| {
            Some(AdminLine {
                seq: e.seq,
                at: e.at.to_rfc3339(),
                record: e.decode().ok()?,
            })
        })
        .take(n)
        .collect()
}

fn refuse(status: StatusCode, body: serde_json::Value) -> Response {
    (status, Json(body)).into_response()
}

fn forbidden(message: &str) -> Response {
    refuse(
        StatusCode::FORBIDDEN,
        serde_json::json!({ "error": message }),
    )
}

/// The active admin device a caller is, if it is one.
fn admin_device(system: &System, caller: &Caller) -> Option<genatrix_store::Device> {
    let Caller::Device(id) = caller else {
        return None;
    };
    system
        .store
        .get_device(id)
        .ok()
        .flatten()
        .filter(|d| d.is_active() && d.admin)
}

/// Whether the caller may see and use the forms for high-privilege
/// requests: this machine, or an admin device while remote administration
/// is not locked. Each request is still checked on its own.
pub fn can_manage(system: &System, caller: &Caller) -> bool {
    *caller == Caller::Local
        || (admin_device(system, caller).is_some()
            && !system.store.admin_state().is_ok_and(|s| s.is_locked()))
}

/// The later steps of something a high-privilege request started, such as
/// the code of a Telegram sign-in: this machine or an admin device, without
/// the passphrase again. The start carried it.
// A handler's refusal is the response itself; boxing it buys nothing.
#[allow(clippy::result_large_err)]
pub fn manager(system: &System, caller: &Caller, nobody: &str) -> Result<(), Response> {
    if can_manage(system, caller) {
        Ok(())
    } else {
        Err(forbidden(nobody))
    }
}

/// Design 02, invariant 12: let a high-privilege request through, or say
/// why not. `nobody` is what an ordinary device is told.
// A handler's refusal is the response itself; boxing it buys nothing.
#[allow(clippy::result_large_err)]
pub async fn high(
    system: &Arc<System>,
    caller: &Caller,
    headers: &HeaderMap,
    act: &str,
    subject: &str,
    nobody: &str,
) -> Result<(), Response> {
    if *caller == Caller::Local {
        record(
            system,
            &AdminRecord {
                act: act.to_owned(),
                subject: subject.to_owned(),
                by: "this machine".to_owned(),
                device: None,
                outcome: "allowed".to_owned(),
            },
        );
        return Ok(());
    }
    let Some(device) = admin_device(system, caller) else {
        return Err(forbidden(nobody));
    };
    let _turn = system.admin.checking.lock().await;
    let state = system
        .store
        .admin_state()
        .map_err(|e| forbidden(&e.to_string()))?;
    if state.is_locked() {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            serde_json::json!({
                "error": "remote administration is locked after five wrong passphrases; unlock it in Settings on the machine Genatrix runs on",
                "locked": true,
            }),
        ));
    }
    let Some(stored) = state.passphrase else {
        return Err(forbidden(
            "no admin passphrase is set; set one in Settings on the machine Genatrix runs on",
        ));
    };
    let given = headers
        .get(HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if given.is_empty() {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            serde_json::json!({ "error": "this needs the admin passphrase", "need": "passphrase" }),
        ));
    }
    let right = tokio::task::spawn_blocking(move || verify(&given, &stored))
        .await
        .unwrap_or(false);
    let mut line = AdminRecord {
        act: act.to_owned(),
        subject: subject.to_owned(),
        by: device.name.clone(),
        device: Some(device.id.clone()),
        outcome: "allowed".to_owned(),
    };
    if right {
        let _ = system.store.admin_succeeded();
        record(system, &line);
        return Ok(());
    }
    let failures = system
        .store
        .admin_failed(&chrono::Utc::now().to_rfc3339())
        .unwrap_or(ADMIN_TRIES);
    "wrong_passphrase".clone_into(&mut line.outcome);
    record(system, &line);
    if failures >= ADMIN_TRIES {
        "locked".clone_into(&mut line.outcome);
        record(system, &line);
        return Err(refuse(
            StatusCode::FORBIDDEN,
            serde_json::json!({
                "error": "five wrong passphrases: remote administration is locked; unlock it in Settings on the machine Genatrix runs on",
                "locked": true,
            }),
        ));
    }
    let left = ADMIN_TRIES - failures;
    Err(refuse(
        StatusCode::FORBIDDEN,
        serde_json::json!({
            "error": format!("wrong admin passphrase; {left} more wrong and remote administration locks"),
            "need": "passphrase",
            "left": left,
        }),
    ))
}

// What the page needs to know to show the admin settings; each is a fact.
#[allow(clippy::struct_excessive_bools)]
#[derive(Serialize)]
struct AdminView {
    /// This request comes from the machine itself.
    here: bool,
    /// The device making this request is an admin device.
    admin: bool,
    /// The forms for high-privilege requests work here.
    manage: bool,
    passphrase_set: bool,
    locked: bool,
    failures: u32,
    /// The latest high-privilege requests, newest first.
    recent: Vec<AdminLine>,
}

async fn view(State(system): State<Arc<System>>, Extension(caller): Extension<Caller>) -> Response {
    let state = system.store.admin_state().unwrap_or_default();
    Json(AdminView {
        here: caller == Caller::Local,
        admin: admin_device(&system, &caller).is_some(),
        manage: can_manage(&system, &caller),
        passphrase_set: state.passphrase.is_some(),
        locked: state.is_locked(),
        failures: state.failures,
        recent: recent(&system, 20),
    })
    .into_response()
}

fn here_only(caller: &Caller) -> Option<Response> {
    (*caller != Caller::Local).then(|| {
        forbidden("the admin role and its passphrase are set on the machine Genatrix runs on")
    })
}

#[derive(Deserialize)]
struct PassphraseBody {
    passphrase: String,
}

/// Set or change the passphrase. This machine only.
async fn set_passphrase(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<PassphraseBody>,
) -> Response {
    if let Some(no) = here_only(&caller) {
        return no;
    }
    if body.passphrase.chars().count() < MIN_LEN {
        return refuse(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": format!("an admin passphrase has at least {MIN_LEN} characters") }),
        );
    }
    let hash = match tokio::task::spawn_blocking(move || hash_passphrase(&body.passphrase)).await {
        Ok(h) => h,
        Err(e) => return forbidden(&e.to_string()),
    };
    if let Err(e) = system.store.set_admin_passphrase(&hash) {
        return refuse(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        );
    }
    record(
        &system,
        &AdminRecord {
            act: "set_passphrase".to_owned(),
            subject: String::new(),
            by: "this machine".to_owned(),
            device: None,
            outcome: "allowed".to_owned(),
        },
    );
    Json(serde_json::json!({ "set": true })).into_response()
}

/// Lift the lock. This machine only.
async fn unlock(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
) -> Response {
    if let Some(no) = here_only(&caller) {
        return no;
    }
    if let Err(e) = system.store.unlock_admin() {
        return refuse(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        );
    }
    record(
        &system,
        &AdminRecord {
            act: "unlock".to_owned(),
            subject: String::new(),
            by: "this machine".to_owned(),
            device: None,
            outcome: "allowed".to_owned(),
        },
    );
    Json(serde_json::json!({ "unlocked": true })).into_response()
}

#[derive(Deserialize)]
struct RoleBody {
    admin: bool,
}

/// Make a paired device an admin device, or an ordinary one again. This
/// machine only (invariant 11), and only once a passphrase is set.
async fn role(
    State(system): State<Arc<System>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(body): Json<RoleBody>,
) -> Response {
    if let Some(no) = here_only(&caller) {
        return no;
    }
    if body.admin
        && system
            .store
            .admin_state()
            .map_or(true, |s| s.passphrase.is_none())
    {
        return refuse(
            StatusCode::CONFLICT,
            serde_json::json!({ "error": "set the admin passphrase first" }),
        );
    }
    match system.store.set_device_admin(&id, body.admin) {
        Ok(true) => {
            let name = system
                .store
                .get_device(&id)
                .ok()
                .flatten()
                .map_or_else(|| id.clone(), |d| d.name);
            record(
                &system,
                &AdminRecord {
                    act: if body.admin {
                        "grant_admin"
                    } else {
                        "ungrant_admin"
                    }
                    .to_owned(),
                    subject: name,
                    by: "this machine".to_owned(),
                    device: None,
                    outcome: "allowed".to_owned(),
                },
            );
            Json(serde_json::json!({ "id": id, "admin": body.admin })).into_response()
        }
        Ok(false) => refuse(
            StatusCode::NOT_FOUND,
            serde_json::json!({ "error": "no such active device" }),
        ),
        Err(e) => refuse(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

/// The routes.
pub fn routes() -> axum::Router<Arc<System>> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/admin", get(view))
        .route("/api/admin/passphrase", post(set_passphrase))
        .route("/api/admin/unlock", post(unlock))
        .route("/api/device/{id}/admin", post(role))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_passphrase_is_kept_as_a_salted_slow_hash() {
        let a = hash_passphrase("correct horse battery");
        let b = hash_passphrase("correct horse battery");
        assert_ne!(a, b, "salted");
        assert!(!a.contains("correct"));
        assert!(a.starts_with("pbkdf2-sha256$"));
        assert!(verify("correct horse battery", &a));
        assert!(!verify("correct horse batterY", &a));
        assert!(!verify("correct horse battery", "garbage"));
    }
}
