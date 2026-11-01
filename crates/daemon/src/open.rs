//! `genatrix open`: sign the browser on this machine in, and open the page.
//!
//! Design: `docs/design/06-interface.md`, "形态"; `docs/design/02-trust-boundary.md`,
//! invariant 11 as revised by design 13.
//!
//! A browser on 127.0.0.1 is no longer this machine by its address alone. The
//! terminal is: it runs as the user and can read the local key. So it asks
//! the running core for a one-time link with the key, and opens that; the
//! core answers the link with the key as a cookie the browser keeps.

use std::path::Path;

use anyhow::{Context as _, bail};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::web::local::HEADER;

/// Ask the core on `port` for a sign-in link, then open it or print it.
pub async fn open(key_path: &Path, port: u16, print: bool) -> anyhow::Result<()> {
    let key = std::fs::read_to_string(key_path)
        .with_context(|| {
            format!(
                "no local key at {}: start Genatrix for this data directory first",
                key_path.display()
            )
        })?
        .trim()
        .to_owned();
    let token = link(port, &key).await?;
    let url = format!("http://127.0.0.1:{port}/local/{token}");
    if print {
        println!("{url}");
        println!("Open it within a minute, in a browser on this machine. It works once.");
        return Ok(());
    }
    let status = std::process::Command::new("/usr/bin/open")
        .arg(&url)
        .status()
        .context("could not run /usr/bin/open")?;
    if !status.success() {
        println!("{url}");
    }
    Ok(())
}

async fn link(port: u16, key: &str) -> anyhow::Result<String> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .with_context(|| format!("nothing answers on 127.0.0.1:{port}; is Genatrix running?"))?;
    let request = format!(
        "POST /api/local/link HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{HEADER}: {key}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await?;
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).await?;
    let reply = String::from_utf8_lossy(&reply);
    let (head, body) = reply
        .split_once("\r\n\r\n")
        .context("the core's answer was not HTTP")?;
    let status = head.lines().next().unwrap_or_default();
    if !status.contains(" 200 ") {
        bail!(
            "the core said {status}: is this the data directory it runs with, and the port it listens on?"
        );
    }
    let json: serde_json::Value =
        serde_json::from_str(body.trim()).context("the core's answer was not JSON")?;
    json["token"]
        .as_str()
        .map(str::to_owned)
        .context("the core's answer had no link")
}
