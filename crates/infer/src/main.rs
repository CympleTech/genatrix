//! `genatrix-infer`: local inference process, sandboxed, no network.
//!
//! Design: `docs/design/04-model-layer.md`. Verified: `docs/plan/spikes/01-sandbox.md`.
//!
//! One model, one Unix socket, one dialect. The process loads a single MLX
//! model and answers OpenAI-style chat completion requests over a Unix
//! domain socket. It is meant to run under a macOS sandbox profile that
//! removes all network access; `--print-sandbox-profile` emits that profile
//! for the given socket directory, and the daemon launches this binary with
//! `sandbox-exec -f <profile>`.
//!
//! Nothing here downloads anything. Weights must already be on disk.

#![forbid(unsafe_code)]

mod sandbox;
mod server;

use std::path::PathBuf;

use clap::Parser;

/// Command line.
#[derive(Debug, Parser)]
#[command(name = "genatrix-infer", about = "Genatrix local inference process")]
struct Args {
    /// Directory holding the MLX model (config.json, weights, tokenizer).
    #[arg(long)]
    model_dir: PathBuf,

    /// Name the model is served under. Requests for any other model are refused.
    #[arg(long)]
    model_name: String,

    /// Unix socket to listen on. Its parent directory must be short: macOS
    /// caps socket paths at 104 bytes.
    #[arg(long)]
    socket: PathBuf,

    /// Unload the model after this many idle seconds. 0 keeps it resident.
    #[arg(long, default_value_t = 0)]
    idle_timeout: u64,

    /// Print the macOS sandbox profile for these paths and exit.
    #[arg(long)]
    print_sandbox_profile: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.print_sandbox_profile {
        print!("{}", sandbox::profile(&args.socket, &args.model_dir)?);
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,crabllm_mlx=warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    server::run(
        &args.model_dir,
        &args.model_name,
        &args.socket,
        args.idle_timeout,
    )
    .await
}
