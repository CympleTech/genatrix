//! `genatrix-llm`: the model gateway.
//!
//! Design: `docs/design/04-model-layer.md`; the policy it enforces is in
//! `docs/design/02-trust-boundary.md`.
//!
//! Runs as its own process so that the core never holds a connection to a
//! cloud provider, and so that the check on what may leave the device sits
//! behind a process boundary rather than inside the code that wants to send.
//!
//! The ticket key is read from the environment, not the command line, so it
//! does not show up in the process table. The daemon generates it at startup
//! and passes it when spawning this process.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use genatrix_keys::TicketKey;
use genatrix_llm::config::Config;
use genatrix_llm::server::{Gateway, serve};

/// Environment variable carrying the hex-encoded ticket key.
const KEY_ENV: &str = "GENATRIX_TICKET_KEY";

#[derive(Debug, Parser)]
#[command(name = "genatrix-llm", about = "Genatrix model gateway")]
struct Args {
    /// Path to the gateway configuration.
    #[arg(long)]
    config: PathBuf,

    /// Check the configuration and exit.
    #[arg(long)]
    check: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let config = Config::load(&args.config)?;
    if args.check {
        println!(
            "configuration is valid: {} model(s), socket {}",
            config.registry.models.len(),
            config.socket.display()
        );
        for m in &config.registry.models {
            println!(
                "  {:<24} {:?}  {}",
                m.name,
                m.location(),
                m.purposes
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let key = match std::env::var(KEY_ENV) {
        Ok(hex) => TicketKey::from_hex(&hex)?,
        Err(_) => {
            anyhow::bail!(
                "{KEY_ENV} is not set. The gateway refuses every request without the \
                 ticket key it shares with the egress gate, so starting without it \
                 would serve nothing."
            );
        }
    };
    serve(Arc::new(Gateway::new(config, key))).await
}
