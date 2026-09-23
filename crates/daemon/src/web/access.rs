//! Who may reach the interface.
//!
//! Design: `docs/design/06-interface.md`, "形态"; `docs/design/08-storage.md`,
//! the network boundary; `docs/design/02-trust-boundary.md`, invariant 11.
//!
//! On loopback, everyone who can open a socket to it is already someone with
//! an account on this machine, and design 08 puts an attacker with that
//! outside the threat model. So loopback asks for nothing.
//!
//! Any other address is a paired device or nobody. A device is paired once,
//! from this machine: the settings page asks for a code, the phone opens the
//! page with the code, and the page trades it for a long-lived secret kept in
//! a cookie. The core keeps a hash of the secret, never the secret. A
//! revoked device is refused like a stranger. The page itself, and the one
//! endpoint that trades a code for a secret, are open to everyone, because a
//! phone has to be able to load the page to pair; every `/api/` route besides
//! that is behind the check.
//!
//! What this does not do, said plainly: TLS. The daemon is meant to be bound
//! to a private network's address, where the tunnel encrypts and identifies;
//! the startup message says so.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::system::System;

/// Name of the cookie that carries a device's credential: `<id>.<secret>`.
pub const COOKIE: &str = "genatrix_device";

/// Where a request came from, as far as access is concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    /// This machine.
    Local,
    /// A paired device, by id.
    Device(String),
}

/// Let the request through, or explain why not.
pub async fn guard(
    State(system): State<Arc<System>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request,
    next: Next,
) -> Response {
    if peer.ip().is_loopback() {
        request.extensions_mut().insert(Caller::Local);
        return next.run(request).await;
    }
    let path = request.uri().path();
    // The page and its assets, and the pairing exchange: open, so that a
    // phone with a code can get as far as using it. Nothing in them is data.
    if !path.starts_with("/api/") || path == "/api/pair" {
        return next.run(request).await;
    }
    match device_of(&system, &request) {
        Some(id) => {
            request.extensions_mut().insert(Caller::Device(id));
            next.run(request).await
        }
        None => refused(),
    }
}

/// The paired device a request comes from, if its cookie names one that is
/// still active and the secret matches.
pub fn device_of(system: &System, request: &Request) -> Option<String> {
    let cookie = cookie_value(request, COOKIE)?;
    let (id, secret) = cookie.split_once('.')?;
    let device = system.store.get_device(id).ok().flatten()?;
    if !device.is_active() {
        return None;
    }
    let presented = hash_secret(secret);
    if presented
        .as_bytes()
        .ct_eq(device.secret_hash.as_bytes())
        .into()
    {
        let _ = system
            .store
            .touch_device(id, &chrono::Utc::now().to_rfc3339());
        Some(device.id)
    } else {
        None
    }
}

/// SHA-256 of a device secret, hex. What the store keeps.
#[must_use]
pub fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

fn refused() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"error":"this device is not paired with this Genatrix; pair it from the settings page on the machine it runs on"}"#,
    )
        .into_response()
}

fn cookie_value(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(n, _)| *n == name)
        .map(|(_, value)| value.to_owned())
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode, header};
    use tower::ServiceExt as _;

    use super::*;

    const LAN: SocketAddr =
        SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)), 50000);
    const HERE: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 50000);

    fn app(system: &Arc<System>) -> axum::Router {
        axum::Router::new()
            .merge(super::super::api::routes())
            .merge(super::super::devices::routes())
            .merge(super::super::setup::routes())
            .fallback(axum::routing::get(|| async { "page" }))
            .layer(axum::middleware::from_fn_with_state(
                Arc::clone(system),
                guard,
            ))
            .layer(axum::Extension(super::super::Serving {
                bind: std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                port: 7717,
            }))
            .with_state(Arc::clone(system))
    }

    async fn call(
        app: &axum::Router,
        from: SocketAddr,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> axum::response::Response {
        let mut request = HttpRequest::builder().method(method).uri(path);
        if let Some(c) = cookie {
            request = request.header(header::COOKIE, c);
        }
        let request = match body {
            Some(json) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json.to_string()))
                .unwrap(),
            None => request.body(Body::empty()).unwrap(),
        };
        let mut request = request;
        request.extensions_mut().insert(ConnectInfo(from));
        app.clone().oneshot(request).await.unwrap()
    }

    async fn json(response: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    /// Design 02, invariant 11.
    #[tokio::test]
    async fn a_stranger_on_the_network_gets_the_page_and_nothing_else() {
        let (_dir, system) = crate::system::test_system();
        let app = app(&system);
        assert_eq!(
            call(&app, LAN, "GET", "/", None, None).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            call(&app, LAN, "GET", "/api/status", None, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&app, LAN, "GET", "/api/actions", None, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&app, LAN, "POST", "/api/pair/start", None, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "a stranger cannot even ask for a code"
        );
        assert_eq!(
            call(&app, HERE, "GET", "/api/status", None, None)
                .await
                .status(),
            StatusCode::OK,
            "loopback asks for nothing"
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // one device's whole life, in order
    async fn a_code_made_here_pairs_one_device_once_and_revocation_ends_it() {
        let (_dir, system) = crate::system::test_system();
        let app = app(&system);

        let started = json(call(&app, HERE, "POST", "/api/pair/start", None, None).await).await;
        let code = started["code"].as_str().unwrap().to_owned();
        assert_eq!(code.len(), 6);

        // A wrong guess spends the code.
        let wrong = call(
            &app,
            LAN,
            "POST",
            "/api/pair",
            None,
            Some(serde_json::json!({"code": "000000"})),
        )
        .await;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        let spent = call(
            &app,
            LAN,
            "POST",
            "/api/pair",
            None,
            Some(serde_json::json!({"code": code})),
        )
        .await;
        assert_eq!(spent.status(), StatusCode::UNAUTHORIZED, "one try per code");

        let started = json(call(&app, HERE, "POST", "/api/pair/start", None, None).await).await;
        let code = started["code"].as_str().unwrap().to_owned();
        let paired = call(
            &app,
            LAN,
            "POST",
            "/api/pair",
            None,
            Some(serde_json::json!({"code": code, "name": "phone"})),
        )
        .await;
        assert_eq!(paired.status(), StatusCode::OK);
        let cookie = paired
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        assert!(cookie.starts_with("genatrix_device="));
        let device = json(paired).await;
        let id = device["id"].as_str().unwrap().to_owned();

        // The same code does not pair a second device.
        let again = call(
            &app,
            LAN,
            "POST",
            "/api/pair",
            None,
            Some(serde_json::json!({"code": code})),
        )
        .await;
        assert_eq!(again.status(), StatusCode::UNAUTHORIZED);

        // The cookie is the key, and only the whole cookie.
        assert_eq!(
            call(&app, LAN, "GET", "/api/status", Some(&cookie), None)
                .await
                .status(),
            StatusCode::OK
        );
        let (name, value) = cookie.split_once('=').unwrap();
        let tampered = format!("{name}={}x", &value[..value.len() - 1]);
        assert_eq!(
            call(&app, LAN, "GET", "/api/status", Some(&tampered), None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        // The store holds a hash, never the secret.
        let stored = system.store.get_device(&id).unwrap().unwrap();
        assert!(!value.contains(&stored.secret_hash));
        assert_eq!(stored.secret_hash.len(), 64);

        // A paired device may approve, but may not start a pairing.
        assert_eq!(
            call(&app, LAN, "POST", "/api/pair/start", Some(&cookie), None)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );

        // Revoked: the cookie is worth nothing, and the record remains.
        let revoked = call(
            &app,
            HERE,
            "POST",
            &format!("/api/device/{id}/revoke"),
            None,
            None,
        )
        .await;
        assert_eq!(revoked.status(), StatusCode::OK);
        assert_eq!(
            call(&app, LAN, "GET", "/api/status", Some(&cookie), None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let devices = json(call(&app, HERE, "GET", "/api/devices", None, None).await).await;
        assert_eq!(devices.as_array().unwrap().len(), 1);
        assert!(devices[0]["revoked_at"].is_string());
    }

    async fn paired_cookie(app: &axum::Router) -> String {
        let started = json(call(app, HERE, "POST", "/api/pair/start", None, None).await).await;
        let code = started["code"].as_str().unwrap().to_owned();
        let paired = call(
            app,
            LAN,
            "POST",
            "/api/pair",
            None,
            Some(serde_json::json!({"code": code})),
        )
        .await;
        paired
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }

    /// Design 02, invariant 12: credentials are typed on this machine.
    #[tokio::test]
    async fn accounts_are_added_and_removed_from_this_machine_only() {
        let (_dir, system) = crate::system::test_system();
        let app = app(&system);
        let phone = paired_cookie(&app).await;
        let writes = [
            (
                "/api/setup/mail",
                serde_json::json!({"address": "a@example.com", "password": "x"}),
            ),
            (
                "/api/setup/telegram/start",
                serde_json::json!({"phone": "+64210000000"}),
            ),
            (
                "/api/setup/telegram/code",
                serde_json::json!({"flow": "x", "code": "1"}),
            ),
            (
                "/api/setup/telegram/password",
                serde_json::json!({"flow": "x", "password": "x"}),
            ),
            (
                "/api/setup/remove",
                serde_json::json!({"kind": "mail", "id": "a@example.com"}),
            ),
            ("/api/model/download", serde_json::json!({})),
        ];
        for (path, body) in &writes {
            assert_eq!(
                call(&app, LAN, "POST", path, None, Some(body.clone()))
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED,
                "a stranger: {path}"
            );
            assert_eq!(
                call(&app, LAN, "POST", path, Some(&phone), Some(body.clone()))
                    .await
                    .status(),
                StatusCode::FORBIDDEN,
                "a paired device may look but not type a password: {path}"
            );
        }
        // A paired device sees the accounts, and is told it is not the machine.
        let seen = json(call(&app, LAN, "GET", "/api/setup", Some(&phone), None).await).await;
        assert_eq!(seen["local"], false);
        let here = json(call(&app, HERE, "GET", "/api/setup", None, None).await).await;
        assert_eq!(here["local"], true);
        // From here the form is answered, and a malformed address is refused
        // before anything is tried or stored.
        let bad = call(
            &app,
            HERE,
            "POST",
            "/api/setup/mail",
            None,
            Some(serde_json::json!({"address": "not an address", "password": "x"})),
        )
        .await;
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
        let unknown = json(
            call(
                &app,
                HERE,
                "POST",
                "/api/setup/mail",
                None,
                Some(
                    serde_json::json!({"address": "me@unknown-provider.example", "password": "x"}),
                ),
            )
            .await,
        )
        .await;
        assert_eq!(
            unknown["needs_server"], true,
            "an unknown provider asks for its server"
        );
        let models = json(call(&app, HERE, "GET", "/api/model", None, None).await).await;
        assert_eq!(models["models"].as_array().unwrap().len(), 2);
    }
}
