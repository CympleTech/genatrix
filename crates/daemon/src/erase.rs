//! Deleting everything, and the requirements check before anything starts.
//!
//! Design: `docs/design/09-install-recover-migrate.md`, "卸载" and "要求";
//! design 10, "第一次外部用户": before a friend starts, they are told they
//! can delete it all at any time, and this is that button.
//!
//! Erasing stops the connectors, ends every Telegram session on Telegram's
//! side, removes the accounts' passwords and the master key from the
//! keychain, removes the data directory, removes a launch agent installed
//! the developer's way when it serves this directory, and ends the process
//! with [`ERASED`]. The menu bar shell reads that code as "unregister the
//! login item and quit", so nothing brings the core back.

use std::sync::Arc;

use serde::Serialize;

use crate::keychain::Keychain;
use crate::system::System;

/// The exit code that means "everything was deleted; do not start me again".
/// The menu bar shell knows it too.
pub const ERASED: i32 = 64;

/// Delete everything this data directory holds and everything kept for it
/// elsewhere. Returns what was done, for the log; the caller ends the process.
pub async fn erase(system: &Arc<System>) -> anyhow::Result<Vec<String>> {
    let mut done = Vec::new();
    let config = &system.config;

    // The connectors first, so nothing writes while the directory goes.
    for task in system
        .connector_tasks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .drain(..)
    {
        task.abort();
    }
    done.push("stopped the connectors".to_owned());

    let accounts = crate::accounts::Accounts::load(&config.accounts_path()).unwrap_or_default();
    for phone in accounts.telegram.keys() {
        crate::setup::sign_out_telegram(system, phone).await;
        done.push(format!("ended the Telegram session for {phone}"));
    }
    for address in accounts.mail.keys() {
        if Keychain::mail().forget(address).unwrap_or(false) {
            done.push(format!(
                "removed the password for {address} from the keychain"
            ));
        }
    }
    if crate::keys::forget_master_key(config)? {
        done.push("removed the master key from the keychain".to_owned());
    }

    // A launch agent installed with `service install` for this directory.
    // Its file goes; the running job is not booted out, because that would
    // kill this process halfway. Exiting with ERASED ends it cleanly.
    if let Ok(plist) = crate::service::plist_path()
        && let Ok(text) = std::fs::read_to_string(&plist)
        && text.contains(&format!("<string>{}</string>", config.data_dir.display()))
    {
        std::fs::remove_file(&plist)?;
        done.push(format!("removed {}", plist.display()));
    }

    std::fs::remove_dir_all(&config.data_dir)?;
    done.push(format!("removed {}", config.data_dir.display()));
    Ok(done)
}

/// One line of the requirements check.
#[derive(Serialize)]
pub struct Check {
    /// `chip`, `macos`, `memory`, `disk`.
    pub what: &'static str,
    pub ok: bool,
    /// Whether a failure stops the installation (design 09: chip and disk do,
    /// memory only lowers the quality).
    pub required: bool,
    /// What was found, in words the page shows.
    pub found: String,
}

/// Design 09, "要求": Apple silicon, one of the last two major macOS
/// releases, 16 GB of memory, 30 GB of free disk.
#[must_use]
pub fn requirements(system: &System) -> Vec<Check> {
    const MEMORY: u64 = 16 * 1024 * 1024 * 1024;
    const DISK: u64 = 30 * 1000 * 1000 * 1000;
    const OLDEST_MACOS: u32 = 15;
    let chip = sysctl("machdep.cpu.brand_string").unwrap_or_default();
    let arm = std::env::consts::ARCH == "aarch64";
    let macos = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|v| v.trim().to_owned())
        .unwrap_or_default();
    let major: u32 = macos
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .unwrap_or(0);
    let memory: u64 = sysctl("hw.memsize")
        .and_then(|m| m.parse().ok())
        .unwrap_or(0);
    // The models count against the disk too; once they are in place, what
    // remains to be checked is only what they still need.
    let models: u64 = crate::download::CATALOG
        .iter()
        .map(crate::download::Model::size)
        .sum();
    let present: u64 = crate::download::CATALOG
        .iter()
        .map(|m| m.present(&system.config))
        .sum();
    let free = crate::download::free_space(&system.config.data_dir).unwrap_or(0);
    let need = DISK.saturating_sub(present.min(models));
    vec![
        Check {
            what: "chip",
            ok: arm,
            required: true,
            found: if chip.is_empty() {
                std::env::consts::ARCH.to_owned()
            } else {
                chip
            },
        },
        Check {
            what: "macos",
            ok: major >= OLDEST_MACOS,
            required: false,
            found: format!("macOS {macos}"),
        },
        Check {
            what: "memory",
            ok: memory >= MEMORY,
            required: false,
            found: format!("{} GB", memory / (1024 * 1024 * 1024)),
        },
        Check {
            what: "disk",
            ok: free >= need,
            required: true,
            found: format!("{} GB free", free / 1_000_000_000),
        },
    ]
}

fn sysctl(name: &str) -> Option<String> {
    let out = std::process::Command::new("sysctl")
        .arg("-n")
        .arg(name)
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn erasing_a_data_directory_leaves_nothing_of_it() {
        let (dir, system) = crate::system::test_system();
        let data = system.config.data_dir.clone();
        assert!(data.join("data.db").exists());
        let done = erase(&system).await.unwrap();
        assert!(!data.exists(), "the directory is gone");
        assert!(done.iter().any(|d| d.starts_with("removed ")));
        drop(dir);
    }

    #[test]
    fn the_check_says_what_it_found_for_each_requirement() {
        let (_dir, system) = crate::system::test_system();
        let checks = requirements(&system);
        let names: Vec<_> = checks.iter().map(|c| c.what).collect();
        assert_eq!(names, ["chip", "macos", "memory", "disk"]);
        assert!(checks.iter().all(|c| !c.found.is_empty()));
        assert!(checks[0].required && checks[3].required && !checks[2].required);
    }
}
