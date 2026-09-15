//! The local web interface.
//!
//! Design: `docs/design/06-interface.md`.
//!
//! A browser cannot speak to a Unix socket, so this is the one part of the
//! system that listens on a TCP port. It listens on `127.0.0.1` by default,
//! where nothing else can reach it. It can be told to listen elsewhere, which
//! is genuinely useful for looking at it from a phone, and which brings an
//! access token with it: see [`access`].
//!
//! The page is plain HTML, CSS and a little JavaScript, compiled into the
//! binary. No build step, no package manager, nothing to install. When the
//! interface grows past what that carries, it can grow a framework; until
//! then a compile chain would buy nothing.

mod access;
mod api;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;

use crate::system::System;
use access::Access;

const INDEX_HTML: &str = include_str!("assets/index.html");
const STYLE_CSS: &str = include_str!("assets/style.css");
const APP_JS: &str = include_str!("assets/app.js");

/// Where and how to listen.
#[derive(Clone, Debug)]
pub struct Serving {
    /// Address to bind. Loopback unless asked otherwise.
    pub bind: IpAddr,
    /// Port.
    pub port: u16,
    /// A token to reuse, so a link survives a restart. Ignored on loopback.
    pub token: Option<String>,
}

/// Serve the interface until the process is stopped.
pub async fn serve(system: Arc<System>, serving: &Serving) -> anyhow::Result<()> {
    let access = Access::for_address(serving.bind, serving.token.clone())?;

    let app = Router::new()
        .route("/", get(index))
        .route("/style.css", get(style))
        .route("/app.js", get(script))
        .merge(api::routes())
        .with_state(system)
        .layer(axum::middleware::from_fn_with_state(
            access.clone(),
            access::guard,
        ));

    let addr = SocketAddr::from((serving.bind, serving.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    announce(serving, &access);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Say where it is, who can reach it, and what that means.
fn announce(serving: &Serving, access: &Access) {
    match access.token() {
        None => {
            println!("Genatrix is at http://127.0.0.1:{}", serving.port);
            println!("Only this machine can reach it.");
        }
        Some(token) => {
            let host = if serving.bind.is_unspecified() {
                local_address().unwrap_or_else(|| serving.bind.to_string())
            } else {
                serving.bind.to_string()
            };
            println!(
                "Genatrix is at http://{host}:{}/?token={token}",
                serving.port
            );
            println!();
            println!(
                "It is listening on {}, so other machines on this network",
                serving.bind
            );
            println!("can reach it. They need that link; the token is in it.");
            println!();
            println!("This is plain HTTP with one shared secret. Fine on a network you");
            println!("trust, not fine on one you do not. Everything here is your mail.");
        }
    }
}

/// A network address of this machine, so the printed link is one somebody can
/// actually click. Asks the routing table which interface reaches the world,
/// rather than guessing at a name: on this machine the answer is `en1`, on the
/// next one it will be something else.
fn local_address() -> Option<String> {
    let interface = default_interface()?;
    address_of(&interface)
}

fn default_interface() -> Option<String> {
    let output = std::process::Command::new("route")
        .args(["-n", "get", "default"])
        .output()
        .ok()?;
    String::from_utf8(output.stdout)
        .ok()?
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("interface:")
                .map(str::trim)
                .map(str::to_owned)
        })
}

fn address_of(interface: &str) -> Option<String> {
    let output = std::process::Command::new("ipconfig")
        .args(["getifaddr", interface])
        .output()
        .ok()?;
    let address = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!address.is_empty()).then_some(address)
}

async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        INDEX_HTML,
    )
}

async fn style() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        STYLE_CSS,
    )
}

async fn script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        APP_JS,
    )
}
