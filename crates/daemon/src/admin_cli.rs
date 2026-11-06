//! `genatrix admin`: admin devices from a terminal on this machine.
//!
//! Design: `docs/design/06-interface.md` v0.7: the admin role is granted on
//! this machine; on a machine without a desktop, from the command line,
//! which can read the local key and so is this machine. It asks the running
//! core over loopback with that key, as `genatrix open` does. The
//! passphrase is typed at a prompt, never on the command line.

use std::path::Path;

use anyhow::{Context as _, bail};
use clap::Subcommand;

use crate::open::{ask, local_key};

#[derive(Debug, Subcommand)]
pub enum AdminAction {
    /// Paired devices, and which are admin devices.
    Devices,
    /// Set or change the admin passphrase. Asked at a prompt.
    Passphrase,
    /// Make a paired device an admin device, by its id or its name.
    Grant { device: String },
    /// Make an admin device an ordinary one again.
    Ungrant { device: String },
    /// Lift the lock that five wrong passphrases put on remote administration.
    Unlock,
}

pub async fn run(key_path: &Path, port: u16, action: AdminAction) -> anyhow::Result<()> {
    let key = local_key(key_path)?;
    match action {
        AdminAction::Devices => {
            let (status, devices) = ask(port, &key, "GET", "/api/devices", None).await?;
            check(status, &devices)?;
            let state = ask(port, &key, "GET", "/api/admin", None).await?.1;
            for d in devices.as_array().into_iter().flatten() {
                let role = if d["revoked_at"].is_string() {
                    "revoked"
                } else if d["admin"] == true {
                    "admin"
                } else {
                    "ordinary"
                };
                println!(
                    "{}  {:<8}  {}",
                    d["id"].as_str().unwrap_or_default(),
                    role,
                    d["name"].as_str().unwrap_or_default()
                );
            }
            println!(
                "passphrase: {}; remote administration: {}",
                if state["passphrase_set"] == true {
                    "set"
                } else {
                    "not set"
                },
                if state["locked"] == true {
                    "locked"
                } else {
                    "open"
                }
            );
        }
        AdminAction::Passphrase => {
            let first = rpassword::prompt_password("New admin passphrase: ")?;
            let again = rpassword::prompt_password("Again: ")?;
            if first != again {
                bail!("the two did not match; nothing changed");
            }
            let body = serde_json::json!({ "passphrase": first });
            let (status, said) =
                ask(port, &key, "POST", "/api/admin/passphrase", Some(&body)).await?;
            check(status, &said)?;
            println!(
                "The admin passphrase is set. Admin devices ask for it on every high-privilege change."
            );
        }
        AdminAction::Grant { device } => role(port, &key, &device, true).await?,
        AdminAction::Ungrant { device } => role(port, &key, &device, false).await?,
        AdminAction::Unlock => {
            let (status, said) = ask(port, &key, "POST", "/api/admin/unlock", None).await?;
            check(status, &said)?;
            println!("Remote administration is open again.");
        }
    }
    Ok(())
}

async fn role(port: u16, key: &str, device: &str, admin: bool) -> anyhow::Result<()> {
    let (status, devices) = ask(port, key, "GET", "/api/devices", None).await?;
    check(status, &devices)?;
    let active: Vec<&serde_json::Value> = devices
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["revoked_at"].is_null())
        .collect();
    let by_id = active.iter().copied().find(|d| d["id"] == device);
    let by_name: Vec<&serde_json::Value> = active
        .iter()
        .copied()
        .filter(|d| d["name"] == device)
        .collect();
    let found = match (by_id, by_name.as_slice()) {
        (Some(d), _) | (None, &[d]) => d,
        (None, []) => {
            bail!("no active device is called {device}; `genatrix admin devices` lists them")
        }
        (None, _) => bail!("more than one device is called {device}; give its id"),
    };
    let id = found["id"].as_str().context("a device without an id")?;
    let body = serde_json::json!({ "admin": admin });
    let (status, said) = ask(
        port,
        key,
        "POST",
        &format!("/api/device/{id}/admin"),
        Some(&body),
    )
    .await?;
    check(status, &said)?;
    let name = found["name"].as_str().unwrap_or(id);
    if admin {
        println!(
            "{name} is an admin device. High-privilege changes from it ask for the passphrase each time."
        );
    } else {
        println!("{name} is an ordinary device again.");
    }
    Ok(())
}

fn check(status: u16, said: &serde_json::Value) -> anyhow::Result<()> {
    if status == 200 {
        return Ok(());
    }
    match said["error"].as_str() {
        Some(e) => bail!("{e}"),
        None => bail!(
            "the core said {status}: is this the data directory it runs with, and the port it listens on?"
        ),
    }
}
