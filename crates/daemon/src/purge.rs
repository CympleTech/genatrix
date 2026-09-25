//! `genatrix purge`: remove what one connector brought in, so it is
//! fetched again under today's rules. The user asks for it by name, reads
//! the counts, and says yes; the core must not be running, so no connector
//! writes while its data goes.

use std::io::Write as _;

use anyhow::bail;
use genatrix_model::Connector;

use crate::config::Config;
use crate::system::System;

/// Sync cursors that survive a purge: the Telegram session, so the account
/// stays signed in and history is simply fetched again.
const KEEP: [&str; 1] = ["session"];

/// Whether a core is running on this data directory: it listens for its
/// connectors.
fn core_running(config: &Config) -> bool {
    ["imap", "telegram"].iter().any(|kind| {
        std::os::unix::net::UnixStream::connect(
            config.data_dir.join("run").join(format!("{kind}.sock")),
        )
        .is_ok()
    })
}

/// Purge one connector's data.
pub fn run(
    config: &Config,
    system: &System,
    connector: Connector,
    dry_run: bool,
    yes: bool,
) -> anyhow::Result<()> {
    if !dry_run && core_running(config) {
        bail!(
            "Genatrix is running on this data directory; stop it first (quit from the menu bar, or `genatrix service uninstall`)"
        );
    }
    let plan = system.store.purge_connector(connector, &KEEP, true)?;
    println!("From {}:", connector.as_str());
    println!("  items (all versions)  {}", plan.items);
    println!(
        "  annotations           {} (of which levels you set yourself: {})",
        plan.annotations, plan.yours
    );
    println!("  search chunks         {}", plan.chunks);
    println!("  raw records           {}", plan.raws);
    println!("  conversations         {}", plan.threads);
    println!(
        "  promises resting only on these  {} (of which you confirmed or rejected: {})",
        plan.commitments, plan.judged_promises
    );
    println!("  attachments           {}", plan.blobs);
    println!(
        "  sync positions        {} (the sign-in is kept)",
        plan.cursors
    );
    println!("People, their roles and notes, digests and actions stay.");
    if dry_run {
        println!("Nothing was changed.");
        return Ok(());
    }
    if !yes {
        print!("Type DELETE to remove these: ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if line.trim() != "DELETE" {
            println!("Nothing was changed.");
            return Ok(());
        }
    }
    let done = system.store.purge_connector(connector, &KEEP, false)?;
    let mut files = 0;
    for h in &done.raw_files {
        if system.raw_files.remove(h).is_ok() {
            files += 1;
        }
    }
    for h in &done.blob_files {
        if system.blob_files.remove(h).is_ok() {
            files += 1;
        }
    }
    system.ledger.append(
        "purge",
        connector.as_str(),
        &serde_json::json!({
            "items": done.items,
            "raws": done.raws,
            "threads": done.threads,
            "commitments": done.commitments,
            "cursors": done.cursors,
            "files": files,
        }),
    )?;
    println!("Removed {} items and {files} files.", done.items);
    println!(
        "Start Genatrix again and it fetches {} anew.",
        connector.as_str()
    );
    Ok(())
}
