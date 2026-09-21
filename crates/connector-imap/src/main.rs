//! `genatrix-imap`: the mail connector, in its own process.
//!
//! Design: `docs/design/05-connectors.md`, "进程模型".
//!
//! Started by the core under a sandbox, with a token in the environment.
//! It connects to the core's socket, says hello, learns which accounts it is
//! responsible for, and keeps each one up to date for as long as it runs.
//! It never touches the keychain, the store, or the network beyond the
//! hosts it was granted; everything it fetches goes back over the socket.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use genatrix_connector::capability::AccountCapability;
use genatrix_connector::protocol::{Client, TOKEN_ENV};
use genatrix_connector::{Fault, SyncState};
use genatrix_connector_imap::ipc::IpcSink;
use genatrix_connector_imap::{Credentials, Imap, run_account};
use tokio::sync::{Mutex, watch};

#[derive(Debug, Parser)]
#[command(name = "genatrix-imap", about = "Genatrix mail connector")]
struct Args {
    /// The core's socket.
    #[arg(long)]
    core_socket: PathBuf,
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

    // The token is single-use on the core's side: it accepts one hello per
    // start. Leaving it in this process's environment therefore gives
    // nothing away, and this process starts no children to inherit it.
    let token = std::env::var(TOKEN_ENV).map_err(|_| {
        anyhow::anyhow!("{TOKEN_ENV} is not set; this process is started by the core")
    })?;

    let mut client = Client::connect(&args.core_socket).await?;
    let assign = client.hello("imap", &token).await?;
    let client = Arc::new(Mutex::new(client));
    tracing::info!(accounts = assign.accounts.len(), "assigned");

    let mut tasks = Vec::new();
    for assignment in assign.accounts {
        let capability: AccountCapability = serde_json::from_str(&assignment.capability_json)?;
        let account = capability.account.clone();
        let sink = IpcSink::new(Arc::clone(&client), &account);

        // The state goes to the core as it changes; the core shows it.
        let (status, mut changes) = watch::channel(SyncState::Starting);
        let reporter = sink.clone();
        tokio::spawn(async move {
            while changes.changed().await.is_ok() {
                let state = changes.borrow_and_update().clone();
                if let Err(e) = reporter.status(&state).await {
                    tracing::warn!(error = %e, "could not report the account state");
                }
            }
        });

        let imap_host = capability
            .hosts
            .iter()
            .find(|h| h.port == 993 || h.port == 143)
            .cloned();
        let secret = assignment.secret;
        let connect_capability = capability.clone();
        let connect = move || {
            let host = imap_host.clone();
            let capability = connect_capability.clone();
            let credentials = host.clone().map(|imap| Credentials {
                account: capability.account.clone(),
                password: secret.clone(),
                imap,
            });
            async move {
                let Some(host) = host else {
                    return Err(Fault::permanent(
                        &capability.account,
                        "this account was granted no mail server to read from",
                    ));
                };
                // The inner of the two checks (design 05). The sandbox is
                // the outer one.
                if !capability.may_reach(&host) {
                    return Err(Fault::permanent(
                        &capability.account,
                        format!("this account is not allowed to connect to {host}"),
                    ));
                }
                let credentials = credentials.expect("a host means credentials");
                Imap::connect(&credentials).await
            }
        };
        tasks.push(tokio::spawn(run_account(capability, connect, sink, status)));
    }

    for task in tasks {
        match task.await {
            Ok(fault) => tracing::warn!(%fault, "account stopped"),
            Err(e) => tracing::error!(error = %e, "account task failed"),
        }
    }
    // Every account has stopped for a reason only the user can fix. Stay
    // up so the core does not restart this process in a loop; it will be
    // replaced when the user signs in again.
    std::future::pending::<()>().await;
    Ok(())
}
