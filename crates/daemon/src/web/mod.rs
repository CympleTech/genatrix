//! The web interface: one daemon, any device with a browser.
//!
//! Design: `docs/design/06-interface.md`, "形态".
//!
//! A browser cannot speak to a Unix socket, so this is the one part of the
//! system that listens on a TCP port. It listens on `127.0.0.1` by default,
//! where nothing else can reach it. It can be told to listen on a private
//! network's address, where paired devices reach it: see [`access`] and
//! [`devices`].
//!
//! The page is built from `web/` with Svelte and Vite into `dist/`, and the
//! four files there are compiled into the binary. The build output is
//! committed with the source, so a Rust toolchain alone builds the core; see
//! `web/README.md` for the front-end side.

mod access;
mod api;
mod devices;

use std::future::IntoFuture as _;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;

pub use devices::Pairing;

use crate::system::System;

const INDEX_HTML: &str = include_str!("dist/index.html");
const APP_JS: &str = include_str!("dist/app.js");
const APP_CSS: &str = include_str!("dist/app.css");
const MANIFEST: &str = include_str!("dist/manifest.webmanifest");
const ICON_SVG: &str = include_str!("dist/icon.svg");

/// Where to listen.
#[derive(Clone, Debug)]
pub struct Serving {
    /// Address to bind. Loopback unless asked otherwise.
    pub bind: IpAddr,
    /// Port.
    pub port: u16,
}

/// Serve the interface until the process is stopped.
pub async fn serve(system: Arc<System>, serving: &Serving) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/app.js", get(script))
        .route("/app.css", get(style))
        .route("/manifest.webmanifest", get(manifest))
        .route("/icon.svg", get(icon))
        .merge(api::routes())
        .merge(devices::routes())
        // Every other path is the page: the app routes on the client side,
        // so a link to /approvals or /pair?code=... opens on the right screen.
        .fallback(get(index))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&system),
            access::guard,
        ))
        .layer(axum::Extension(serving.clone()))
        .with_state(system);

    let addr = SocketAddr::from((serving.bind, serving.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    announce(serving);
    let service = app.into_make_service_with_connect_info::<SocketAddr>();
    // Bound to one particular address that is not loopback, the core would
    // be unreachable from this machine: the menu bar shell asks 127.0.0.1,
    // and a pairing code can only be made from loopback, so no device could
    // ever be paired. Loopback is listened on as well in that case. The
    // wildcard address already covers it.
    if needs_loopback_too(serving.bind) {
        let local = tokio::net::TcpListener::bind(SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            serving.port,
        )))
        .await?;
        println!(
            "Also at http://127.0.0.1:{}, for the menu bar and for pairing.",
            serving.port
        );
        tokio::try_join!(
            axum::serve(listener, service.clone()).into_future(),
            axum::serve(local, service).into_future(),
        )?;
    } else {
        axum::serve(listener, service).await?;
    }
    Ok(())
}

/// Whether a bind address leaves this machine without a loopback listener.
fn needs_loopback_too(bind: IpAddr) -> bool {
    !bind.is_loopback() && !bind.is_unspecified()
}

/// Say where it is, who can reach it, and what that means.
fn announce(serving: &Serving) {
    if serving.bind.is_loopback() {
        println!("Genatrix is at http://127.0.0.1:{}", serving.port);
        println!("Only this machine can reach it.");
        return;
    }
    let host = if serving.bind.is_unspecified() {
        local_address().unwrap_or_else(|| serving.bind.to_string())
    } else {
        serving.bind.to_string()
    };
    println!("Genatrix is at http://{host}:{}", serving.port);
    println!();
    println!(
        "It is listening on {}, so other devices that can reach that address",
        serving.bind
    );
    println!("can open the page. Only a paired device gets past it: pair a phone");
    println!("from Settings on this machine.");
    println!();
    println!("This is plain HTTP. Bind it to a private network you trust, such as a");
    println!("Tailscale or WireGuard address; the tunnel is the encryption. Everything");
    println!("here is your mail and messages.");
}

/// Addresses another device can open the page at, as base URLs. Nothing
/// when only loopback is listened on; the address bound to when it is a
/// particular one; otherwise the address of the interface that carries the
/// default route, which on a home network is the one a phone can see.
pub fn reachable_at(serving: &Serving) -> Vec<String> {
    if serving.bind.is_loopback() {
        return Vec::new();
    }
    let host = if serving.bind.is_unspecified() {
        match local_address() {
            Some(a) => a,
            None => return Vec::new(),
        }
    } else {
        serving.bind.to_string()
    };
    vec![format!("http://{host}:{}", serving.port)]
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
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        INDEX_HTML,
    )
}

async fn script() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        APP_JS,
    )
}

async fn style() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        APP_CSS,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_is_given_the_address_it_can_reach_and_never_loopback() {
        let only_here = Serving {
            bind: "127.0.0.1".parse().unwrap(),
            port: 7717,
        };
        assert!(
            reachable_at(&only_here).is_empty(),
            "no device can pair with loopback only"
        );
        let lan = Serving {
            bind: "192.168.3.34".parse().unwrap(),
            port: 7717,
        };
        assert_eq!(
            reachable_at(&lan),
            vec!["http://192.168.3.34:7717".to_owned()]
        );
    }

    #[test]
    fn a_particular_lan_address_keeps_loopback_and_the_others_need_nothing_more() {
        let lan: IpAddr = "192.168.3.34".parse().unwrap();
        assert!(
            needs_loopback_too(lan),
            "the shell and pairing need 127.0.0.1"
        );
        assert!(!needs_loopback_too("127.0.0.1".parse().unwrap()));
        assert!(
            !needs_loopback_too("0.0.0.0".parse().unwrap()),
            "the wildcard covers it"
        );
        assert!(!needs_loopback_too("::".parse().unwrap()));
    }
}
