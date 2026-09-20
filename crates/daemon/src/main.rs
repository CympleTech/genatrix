//! `genatrix`: the core process.
//!
//! Design: `docs/design/06-interface.md` and
//! `docs/design/09-install-recover-migrate.md`.
//!
//! Not yet a daemon: it has no web interface and no connectors, so what it
//! offers is the commands needed to see the whole path work end to end. Each
//! one is a step the finished product will take on its own.

#![forbid(unsafe_code)]

mod accounts;
mod caller;
mod config;
mod ingest;
mod keys;
mod names;
mod pipeline;
mod seed;
mod system;
mod web;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use genatrix_agent::run::RunContext;
use genatrix_keys::TicketKey;
use genatrix_ledger::{EntryFilter, kind};
use genatrix_store::{ItemQuery, ItemVersion};

use crate::config::Config;
use crate::system::System;

/// Environment variable carrying the key shared with the gateway.
const KEY_ENV: &str = "GENATRIX_TICKET_KEY";

#[derive(Debug, Parser)]
#[command(name = "genatrix", about = "Genatrix core")]
struct Args {
    /// Data directory. Defaults to ~/Library/Application Support/Genatrix.
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create the data directory, the keys, and the databases.
    Init,
    /// Put synthetic items in, so there is something to work on before a
    /// connector exists.
    Seed,
    /// Judge the sensitivity of every item that has none yet.
    Classify,
    /// Show the timeline.
    Timeline {
        /// How many items.
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Show what has left this device.
    Ledger,
    /// Add a mailbox.
    Account {
        /// The address. For a well-known provider that is all it takes.
        #[arg(long)]
        add: String,
        /// The IMAP server, when it cannot be worked out from the address.
        #[arg(long)]
        imap_host: Option<String>,
    },
    /// Fetch mail for every account.
    Sync {
        /// Stop after this many messages per account.
        #[arg(long, default_value_t = 500)]
        limit: usize,
    },
    /// Write everything out in the open format, so it can be read elsewhere.
    Export {
        /// Directory to write into. Created if it does not exist.
        #[arg(long)]
        to: PathBuf,
    },
    /// Serve the local interface.
    Serve {
        /// Address to listen on. Loopback by default, which only this machine
        /// can reach. Anything else needs an access token, which is generated
        /// and printed with the link.
        #[arg(long, default_value = "127.0.0.1")]
        bind: std::net::IpAddr,
        /// Port.
        #[arg(long, default_value_t = 7717)]
        port: u16,
        /// Reuse this access token instead of a fresh one, so a link keeps
        /// working across restarts. Ignored on loopback.
        #[arg(long)]
        token: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,genatrix=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let config = Config::under(args.data_dir.unwrap_or_else(Config::default_data_dir));

    match args.command {
        Command::Init => init(&config),
        Command::Seed => seed(&config),
        Command::Classify => classify(&config).await,
        Command::Timeline { limit } => timeline(&config, limit),
        Command::Ledger => ledger(&config),
        Command::Export { to } => export(&config, &to),
        Command::Account { add, imap_host } => account(&config, &add, imap_host),
        Command::Sync { limit } => sync(&config, limit).await,
        Command::Serve { bind, port, token } => {
            serve(&config, &web::Serving { bind, port, token }).await
        }
    }
}

/// A key is needed for anything that might call a model. Commands that only
/// read use a throwaway one: nothing they do can mint a usable ticket.
fn ticket_key(required: bool) -> anyhow::Result<TicketKey> {
    match std::env::var(KEY_ENV) {
        Ok(hex) => Ok(TicketKey::from_hex(&hex)?),
        Err(_) if !required => Ok(TicketKey::generate()?),
        Err(_) => anyhow::bail!(
            "{KEY_ENV} is not set. Set the same value the gateway was started with:\n\n  \
             export {KEY_ENV}=<the gateway's key>\n\n\
             It is the secret that lets this process mint permission to send."
        ),
    }
}

fn init(config: &Config) -> anyhow::Result<()> {
    config.create_dirs()?;
    if !config.gateway_config_path().exists() {
        anyhow::bail!(
            "no gateway configuration at {}.\n\
             Write one first; the README has a template. It says which models \
             exist and where each one runs, and this process needs to agree \
             with the gateway about that.",
            config.gateway_config_path().display()
        );
    }
    let system = System::open(config.clone(), ticket_key(false)?)?;
    println!("data directory  {}", config.data_dir.display());
    println!("store           {} item(s)", system.store.count_items()?);
    println!(
        "files           {} bytes of raw records, {} bytes of attachments",
        system.raw_files.size_on_disk()?,
        system.blob_files.size_on_disk()?
    );
    println!("ledger          {} entr(ies)", system.ledger.len()?);
    println!("rules           {}", config.rules_path().display());
    println!("gateway socket  {}", config.gateway_socket.display());
    println!(
        "cloud           {}",
        if system.gate.cloud_enabled() {
            "enabled"
        } else {
            "off"
        }
    );
    Ok(())
}

fn seed(config: &Config) -> anyhow::Result<()> {
    let system = System::open(config.clone(), ticket_key(false)?)?;
    let added = seed::run(&system.store, &system.raw_files)?;
    println!(
        "added {added} of {} sample item(s); {} in the store, {} bytes of raw records on disk",
        seed::count(),
        system.store.count_items()?,
        system.raw_files.size_on_disk()?
    );
    Ok(())
}

async fn classify(config: &Config) -> anyhow::Result<()> {
    let system = System::open(config.clone(), ticket_key(true)?)?;
    if !system.caller.gateway_healthy().await {
        anyhow::bail!(
            "the gateway is not answering at {}. Start it, and the inference \
             process behind it, before classifying.",
            config.gateway_socket.display()
        );
    }

    let mut ctx = RunContext::begin(&system.ledger, &system.caller, "classify", 64)?;
    let store = system.store.clone();
    let started = std::time::Instant::now();
    let report = pipeline::classify::run(&store, system.gate.rules(), &mut ctx).await;
    let steps = ctx.steps();
    match &report {
        Ok(_) => ctx.done()?,
        Err(e) => ctx.stopped(e.to_string())?,
    };
    let report = report?;

    println!(
        "classified {} item(s) in {:.1?}",
        report.seen,
        started.elapsed()
    );
    println!("  settled by rules alone   {}", report.by_rule);
    println!(
        "  asked the local model    {} in {steps} call(s)",
        report.asked
    );
    println!("  raised above the rules   {}", report.raised);
    for (level, count) in &report.levels {
        println!("  {level:<24} {count}");
    }
    Ok(())
}

/// Environment variable carrying a mailbox password, read at sync time and
/// never written down. Design 08 replaces this with the keychain.
const PASSWORD_ENV: &str = "GENATRIX_IMAP_PASSWORD";

fn account(config: &Config, address: &str, imap_host: Option<String>) -> anyhow::Result<()> {
    config.create_dirs()?;
    let path = config.accounts_path();
    let mut accounts = accounts::Accounts::load(&path)?;
    accounts.add_mail(address, imap_host)?;
    accounts.save(&path)?;

    let account = &accounts.mail[&address.trim().to_lowercase()];
    println!("added {}", account.address);
    println!("  reads from  {}:{}", account.imap_host, account.imap_port);
    match &account.smtp_host {
        Some(host) => println!("  sends via   {host}:{}", account.smtp_port),
        None => println!("  sends via   not configured, so this account cannot send"),
    }
    println!();
    println!("No password was stored. Set {PASSWORD_ENV} when syncing:");
    println!();
    println!("  export {PASSWORD_ENV}=<your app password>");
    println!("  genatrix sync");
    println!();
    println!("On Gmail that is an app password, not your account password:");
    println!("turn on two-step verification, then create one under App passwords.");
    Ok(())
}

/// Fetch mail for every account: history newest first, then anything new.
async fn sync(config: &Config, limit: usize) -> anyhow::Result<()> {
    use genatrix_connector::capability::Host;
    use genatrix_connector::{Checkpoint, Cursor, Progress};
    use genatrix_connector_imap::imap::{Credentials, Imap};
    use genatrix_connector_imap::sync::Sync as MailSync;

    let system = System::open(config.clone(), ticket_key(false)?)?;
    let accounts = accounts::Accounts::load(&config.accounts_path())?;
    if accounts.mail.is_empty() {
        anyhow::bail!("no accounts yet. `genatrix account --add you@example.com` adds one.");
    }
    let password = std::env::var(PASSWORD_ENV).map_err(|_| {
        anyhow::anyhow!(
            "{PASSWORD_ENV} is not set. It is read each run and never written down:\n\n  \
             export {PASSWORD_ENV}=<your app password>"
        )
    })?;
    let grant = accounts.grant();

    for account in accounts.mail.values() {
        let host = Host::new(&account.imap_host, account.imap_port);
        let capability = grant
            .account(&account.address)
            .ok_or_else(|| anyhow::anyhow!("{} has no capability", account.address))?
            .clone();

        println!("{}", account.address);
        let credentials = Credentials {
            account: account.address.clone(),
            password: password.clone(),
            imap: host.clone(),
        };
        let imap = Imap::connect(&credentials).await?;
        let mail = MailSync::new(imap, capability);
        mail.check_host(&host)?;

        let folders = mail.folders().await?;
        let mut taken = 0usize;
        for folder in &folders {
            println!("  {} ({} message(s))", folder.name, folder.count);

            // Anything new first: it is the part the user is waiting for.
            let mut checkpoint = Checkpoint::new(
                &account.address,
                &folder.name,
                Cursor::ImapUid {
                    uidvalidity: folder.uidvalidity,
                    highest: 0,
                },
            );
            let (fresh, _) = mail
                .catch_up(&folder.name, folder.uidvalidity, &mut checkpoint)
                .await?;
            taken += store_batch(&system, &fresh)?;

            // Then history, newest first, until the limit.
            let mut progress = Progress::default();
            let mut before = None;
            while taken < limit {
                let (batch, oldest, pass) = mail
                    .backfill(&folder.name, folder.uidvalidity, before, &mut progress)
                    .await?;
                taken += store_batch(&system, &batch)?;
                before = oldest;
                if !pass.more {
                    break;
                }
                println!("    {}", progress.describe());
            }
        }
        println!("  {taken} new item(s)");
    }

    println!();
    println!("{} item(s) in the store", system.store.count_items()?);
    println!("`genatrix classify` judges the new ones.");
    Ok(())
}

/// Store a batch and say how many were new.
fn store_batch(
    system: &System,
    batch: &[genatrix_connector_imap::sync::Incoming],
) -> anyhow::Result<usize> {
    let mut added = 0;
    for incoming in batch {
        if let ingest::Ingested::Added(_) = ingest::mail(
            &system.store,
            &system.raw_files,
            &system.blob_files,
            incoming,
        )? {
            added += 1;
        }
    }
    Ok(added)
}

async fn serve(config: &Config, serving: &web::Serving) -> anyhow::Result<()> {
    let system = std::sync::Arc::new(System::open(config.clone(), ticket_key(false)?)?);
    web::serve(system, serving).await
}

fn timeline(config: &Config, limit: u32) -> anyhow::Result<()> {
    let system = System::open(config.clone(), ticket_key(false)?)?;
    let items = system.store.query_items(&ItemQuery {
        limit,
        version: ItemVersion::Current,
        ..Default::default()
    })?;
    if items.is_empty() {
        println!("nothing yet. `genatrix seed` puts sample items in.");
        return Ok(());
    }
    for item in items {
        let who = item
            .author
            .and_then(|a| system.store.get_person(a).ok().flatten())
            .map_or_else(|| "unknown".to_owned(), |p| p.display_name);
        let mark = match item.sensitivity {
            genatrix_model::Level::Public => "public  ",
            genatrix_model::Level::Personal => "personal",
            genatrix_model::Level::Secret => "SECRET  ",
        };
        println!(
            "{}  {mark}  {:<9} {:<18} {}",
            item.occurred_at.format("%m-%d %H:%M"),
            item.source.connector.as_str(),
            truncate(&who, 18),
            truncate(&item.text.replace('\n', " "), 64)
        );
    }
    Ok(())
}

/// Design 01 fixes the shape of this: the model as JSONL, plus the raw
/// records and attachments as files. Not the database, the model, so another
/// implementation can read it. Plain text, which the output says out loud.
fn export(config: &Config, to: &std::path::Path) -> anyhow::Result<()> {
    let system = System::open(config.clone(), ticket_key(false)?)?;
    system.store.checkpoint()?;
    let summary = system.store.export_to(to)?;

    // The bytes come out decrypted: an export that needed Genatrix to read it
    // would not be an export.
    let mut files = 0usize;
    let mut bytes = 0usize;
    for (name, store) in [("raw", &system.raw_files), ("blobs", &system.blob_files)] {
        let dir = to.join(name);
        std::fs::create_dir_all(&dir)?;
        for hash in hashes_in(config, name) {
            let contents = store.get(&hash)?;
            bytes += contents.len();
            files += 1;
            std::fs::write(dir.join(hash.to_string()), contents)?;
        }
    }

    println!("wrote {}", to.display());
    println!(
        "  {} items, {} threads, {} people, {} annotations",
        summary.items, summary.threads, summary.persons, summary.annotations
    );
    println!("  {files} files, {bytes} bytes");
    println!();
    println!("This is your data in the clear, with no encryption. Put it somewhere safe.");
    Ok(())
}

/// Which files a store holds. The store is content-addressed, so the names on
/// disk are the whole index.
fn hashes_in(config: &Config, which: &str) -> Vec<genatrix_model::ContentHash> {
    let root = config.data_dir.join(which);
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && let Ok(hash) = name.parse()
            {
                out.push(hash);
            }
        }
    }
    out
}

fn ledger(config: &Config) -> anyhow::Result<()> {
    let system = System::open(config.clone(), ticket_key(false)?)?;
    let verified = system.ledger.verify()?;

    let egress = system.ledger.entries(&EntryFilter {
        kind: Some(kind::EGRESS.into()),
        limit: 10_000,
        ..Default::default()
    })?;
    let mut left_the_device = 0usize;
    let mut bytes_out = 0usize;
    for entry in &egress {
        if let Ok(record) = entry.decode::<genatrix_gate::gate::EgressRecord>()
            && let Some(payload) = record.payload
        {
            left_the_device += 1;
            bytes_out += payload.len();
        }
    }

    // The sentence design 06 asks the records page to open with.
    if bytes_out == 0 {
        println!("0 bytes have left this device.");
    } else {
        println!("{bytes_out} bytes left this device in {left_the_device} request(s).");
    }
    println!();
    println!("{} model call(s) recorded", egress.len());
    println!(
        "{} entr(ies) in the chain, verified to the root",
        verified.entries
    );
    Ok(())
}

fn truncate(text: &str, width: usize) -> String {
    let mut out: String = text.chars().take(width).collect();
    if text.chars().count() > width {
        out.push('…');
    }
    out
}
