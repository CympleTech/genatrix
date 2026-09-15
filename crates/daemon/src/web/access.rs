//! Who may reach the interface.
//!
//! On loopback, everyone who can open a socket to it is already someone with
//! an account on this machine, and design 08 says an attacker with that is
//! outside the threat model anyway. So loopback asks for nothing.
//!
//! Any other address is a different situation entirely. The interface reads
//! every mail and message the user has, and it has no login. Bound to a
//! network address without a check, it would hand that to whoever else is on
//! the wifi. So a non-loopback bind requires a token, generated per run and
//! printed with the link. Open the link once and a cookie keeps you in.
//!
//! This is not authentication in any serious sense: it is one shared secret
//! over plain HTTP, and anyone who can watch the traffic can take it. It is
//! enough to make "let me look at this from my phone" reasonable, and it is
//! not enough to put this on an untrusted network. Design 06 says local; this
//! only stretches it to a network the user trusts.

use std::net::IpAddr;

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// How the interface decides who gets in.
#[derive(Clone, Debug)]
pub enum Access {
    /// Loopback: no check.
    Open,
    /// Everything else: this token, in a cookie or the query string.
    Token(String),
}

/// Name of the cookie that remembers the token.
const COOKIE: &str = "genatrix_access";

impl Access {
    /// Decide what the given bind address needs.
    pub fn for_address(ip: IpAddr, provided: Option<String>) -> std::io::Result<Self> {
        if ip.is_loopback() {
            return Ok(Self::Open);
        }
        Ok(Self::Token(match provided {
            Some(token) => token,
            None => random_token()?,
        }))
    }

    /// The token, when one is needed.
    #[must_use]
    pub fn token(&self) -> Option<&str> {
        match self {
            Self::Open => None,
            Self::Token(t) => Some(t),
        }
    }
}

fn random_token() -> std::io::Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(hex::encode(bytes))
}

/// Let the request through, or explain why not.
pub async fn guard(State(access): State<Access>, request: Request, next: Next) -> Response {
    let Access::Token(expected) = &access else {
        return next.run(request).await;
    };

    if cookie_token(&request).as_deref() == Some(expected.as_str()) {
        return next.run(request).await;
    }

    // A fresh visitor arrives with the token in the link. Accept it, and
    // hand back a cookie so the rest of the session is ordinary.
    if query_token(&request).as_deref() == Some(expected.as_str()) {
        let path = request.uri().path().to_owned();
        let response = next.run(request).await;
        return (
            [
                (
                    header::SET_COOKIE,
                    format!("{COOKIE}={expected}; Path=/; HttpOnly; SameSite=Strict"),
                ),
                (header::CONTENT_LOCATION, path),
            ],
            response,
        )
            .into_response();
    }

    (
        StatusCode::UNAUTHORIZED,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "This copy of Genatrix is reachable over the network, so it needs the \
         access token.\n\nOpen the link the daemon printed when it started; it \
         has the token in it.\n",
    )
        .into_response()
}

fn cookie_token(request: &Request) -> Option<String> {
    request
        .headers()
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value.to_owned())
}

fn query_token(request: &Request) -> Option<String> {
    request
        .uri()
        .query()?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| *name == "token")
        .map(|(_, value)| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn loopback_asks_for_nothing() {
        for ip in [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ] {
            let access = Access::for_address(ip, None).unwrap();
            assert!(matches!(access, Access::Open));
            assert!(access.token().is_none());
        }
    }

    #[test]
    fn any_other_address_gets_a_token_whether_or_not_one_was_asked_for() {
        let wildcard = Access::for_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED), None).unwrap();
        assert_eq!(wildcard.token().map(str::len), Some(32));

        let lan = Access::for_address(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)), None).unwrap();
        assert!(lan.token().is_some());
        assert_ne!(
            wildcard.token(),
            lan.token(),
            "a fresh token for each run, not a fixed one"
        );
    }

    #[test]
    fn a_token_can_be_supplied_so_a_link_survives_a_restart() {
        let access =
            Access::for_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED), Some("abc".into())).unwrap();
        assert_eq!(access.token(), Some("abc"));
    }
}
