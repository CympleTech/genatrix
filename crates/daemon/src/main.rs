//! `genatrix`: the core process.
//!
//! Design: `docs/design/06-interface.md` and
//! `docs/design/09-install-recover-migrate.md`.
//!
//! Not yet a daemon: it has no web interface and no connectors, so what it
//! offers is the commands needed to see the whole path work end to end. Each
//! one is a step the finished product will take on its own.

#![forbid(unsafe_code)]

mod caller;
mod config;
mod keys;
mod names;
mod pipeline;
mod seed;
mod system;

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
    let added = seed::run(&system.store)?;
    println!(
        "added {added} of {} sample item(s); {} in the store",
        seed::count(),
        system.store.count_items()?
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
    let headers_of = move |item: &genatrix_model::Item| seed::headers_for(&item.source.external_id);
    let domain_of =
        move |item: &genatrix_model::Item| seed::sender_domain_for(&item.source.external_id);

    let started = std::time::Instant::now();
    let report = pipeline::classify::run(
        &store,
        system.gate.rules(),
        &mut ctx,
        &headers_of,
        &domain_of,
    )
    .await;
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
