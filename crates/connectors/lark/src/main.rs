//! `genatrix-lark`: the Lark connector, in its own process.
//!
//! Design: `docs/design/05-connectors.md`, "飞书（Lark）：你负责的任务".
//! Started by the core under a sandbox with a token in the environment: it
//! connects to the core's socket, learns its accounts, and for each one
//! loads the sign-in the core kept and reads the user's tasks every
//! [`EVERY`], sending what changed back over the socket. It renews the
//! sign-in itself, and hands the new one to the core before using it. It
//! never touches the store, the keychain, or any host but its own open
//! platform.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use genatrix_connector::capability::{AccountCapability, Host};
use genatrix_connector::protocol::{self, Body, CallError, Client as CoreClient, TOKEN_ENV};
use genatrix_connector::{Fault, SyncState};
use genatrix_connector_lark::api::{ApiError, Http};
use genatrix_connector_lark::auth::{self, AuthError};
use genatrix_connector_lark::normalize::Names;
use genatrix_connector_lark::sync::{self, Position};
use genatrix_connector_lark::{SIGN_IN_SCOPE, Secret, TASKS_SCOPE, Token};
use tokio::sync::Mutex;

#[derive(Debug, Parser)]
#[command(name = "genatrix-lark", about = "Genatrix Lark connector")]
struct Args {
    /// The core's socket.
    #[arg(long)]
    core_socket: PathBuf,
}

/// How often the tasks are read.
const EVERY: Duration = Duration::from_secs(120);
/// How many records go in one store call.
const BATCH: usize = 50;
/// Whether what the sign-in was granted has been said.
static GRANTED: std::sync::Once = std::sync::Once::new();

/// The core, for one account.
#[derive(Clone)]
struct Core {
    client: Arc<Mutex<CoreClient>>,
    account: String,
}

impl Core {
    async fn call(&self, body: Body) -> Result<Body, Fault> {
        match self.client.lock().await.call(body).await {
            Ok(body) => Ok(body),
            Err(CallError::Refused(detail)) => Err(Fault::transient(&self.account, detail)),
            Err(e) => {
                tracing::error!(error = %e, "the core connection is gone; leaving");
                std::process::exit(2);
            }
        }
    }

    async fn status(&self, state: &SyncState) {
        if let Ok(state_json) = serde_json::to_string(state) {
            let _ = self
                .call(Body::Status(protocol::Status {
                    account: self.account.clone(),
                    state_json,
                }))
                .await;
        }
    }

    async fn load_json(&self, scope: &str) -> Result<Option<String>, Fault> {
        match self
            .call(Body::LoadCursor(protocol::LoadCursor {
                account: self.account.clone(),
                scope: scope.to_owned(),
            }))
            .await?
        {
            Body::CursorLoaded(c) => Ok(c.cursor_json),
            _ => Err(Fault::transient(
                &self.account,
                "the core answered out of turn",
            )),
        }
    }

    async fn save_json(&self, scope: &str, json: String) -> Result<(), Fault> {
        self.call(Body::SaveCursor(protocol::SaveCursor {
            account: self.account.clone(),
            scope: scope.to_owned(),
            cursor_json: json,
        }))
        .await
        .map(|_| ())
    }

    async fn store(
        &self,
        tasks: Vec<protocol::TaskRecord>,
        comments: Vec<protocol::TaskComment>,
    ) -> Result<(), Fault> {
        self.call(Body::Store(protocol::Store {
            account: self.account.clone(),
            tasks,
            comments,
            ..Default::default()
        }))
        .await
        .map(|_| ())
    }
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
    let token = std::env::var(TOKEN_ENV).map_err(|_| {
        anyhow::anyhow!("{TOKEN_ENV} is not set; this process is started by the core")
    })?;

    let parent = std::os::unix::process::parent_id();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if std::os::unix::process::parent_id() != parent {
                tracing::warn!("the core is gone; leaving");
                std::process::exit(3);
            }
        }
    });

    let mut client = CoreClient::connect(&args.core_socket).await?;
    let assign = client.hello("lark", &token).await?;
    let client = Arc::new(Mutex::new(client));
    tracing::info!(accounts = assign.accounts.len(), "assigned");

    let mut tasks = Vec::new();
    for assignment in assign.accounts {
        let capability: AccountCapability = serde_json::from_str(&assignment.capability_json)?;
        let secret: Secret = serde_json::from_str(&assignment.secret)?;
        let core = Core {
            client: Arc::clone(&client),
            account: capability.account.clone(),
        };
        // Its own open platform, and only that: the sandbox lets port 443
        // go anywhere, so the host is checked here (design 05).
        let host = Host::new(secret.brand.open_host(), 443);
        if !capability.may_reach(&host) {
            core.status(&SyncState::Stopped {
                detail: format!("this account is not allowed to connect to {host}"),
            })
            .await;
            continue;
        }
        tasks.push(tokio::spawn(run_account(core, secret)));
    }
    for task in tasks {
        if let Ok(fault) = task.await {
            tracing::warn!(%fault, "account stopped");
        }
    }
    std::future::pending::<()>().await;
    Ok(())
}

/// Keep one account up to date until a fault only the user can fix.
async fn run_account(core: Core, secret: Secret) -> Fault {
    let mut attempt: u32 = 0;
    let mut names = Names::default();
    loop {
        match once(&core, &secret, &mut names).await {
            Ok(()) => {
                attempt = 0;
                core.status(&SyncState::Live {
                    synced_at: chrono::Utc::now(),
                })
                .await;
                tokio::time::sleep(EVERY).await;
            }
            Err(fault) if !fault.retryable() => {
                core.status(&SyncState::after(&fault, attempt)).await;
                return fault;
            }
            Err(fault) => {
                attempt = attempt.saturating_add(1);
                core.status(&SyncState::after(&fault, attempt)).await;
                tokio::time::sleep(Fault::backoff(attempt).max(EVERY / 4)).await;
            }
        }
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The sign-in the core kept, renewed first if it is about to run out. A
/// renewed sign-in is saved before it is used: the old refresh token is
/// spent the moment Lark answers.
async fn signed_in(core: &Core, secret: &Secret, force: bool) -> Result<Token, Fault> {
    let account = core.account.clone();
    let Some(json) = core.load_json(SIGN_IN_SCOPE).await? else {
        return Err(Fault::needs_user(
            account,
            "not signed in to Lark; run `genatrix account --add-lark <app id>`",
        ));
    };
    let token: Token = serde_json::from_str(&json)
        .map_err(|e| Fault::needs_user(&account, format!("the Lark sign-in is unreadable: {e}")))?;
    if !force && !token.stale(now_ms()) {
        return Ok(token);
    }
    let renewing = secret.clone();
    let renewed = tokio::task::spawn_blocking(move || auth::refresh(&renewing, &token))
        .await
        .map_err(|e| Fault::transient(&account, e.to_string()))?;
    let token = match renewed {
        Ok(token) => token,
        Err(AuthError::SignInAgain(detail)) => return Err(Fault::needs_user(&account, detail)),
        Err(AuthError::Transient(detail)) => return Err(Fault::transient(&account, detail)),
    };
    let json =
        serde_json::to_string(&token).map_err(|e| Fault::transient(&account, e.to_string()))?;
    if let Err(fault) = core.save_json(SIGN_IN_SCOPE, json).await {
        // The old refresh token is spent and the new one is not kept: the
        // next start will need a sign-in. Say so rather than carry on as if
        // nothing happened.
        tracing::error!(%fault, "a renewed Lark sign-in could not be kept");
        return Err(Fault::needs_user(
            &account,
            "the renewed Lark sign-in could not be saved; sign in again",
        ));
    }
    tracing::info!("renewed the Lark sign-in");
    Ok(token)
}

/// One round: sign in, read what changed, store it, then move the cursor.
async fn once(core: &Core, secret: &Secret, names: &mut Names) -> Result<(), Fault> {
    let account = core.account.clone();
    let before: Position = if let Some(json) = core.load_json(TASKS_SCOPE).await? {
        serde_json::from_str(&json).unwrap_or_default()
    } else {
        core.status(&SyncState::Connecting).await;
        Position::default()
    };

    let mut force = false;
    let round = loop {
        let token = signed_in(core, secret, force).await?;
        // What a sign-in may do is fixed when it is granted: a permission
        // turned on in the console afterwards needs a new sign-in. Said once.
        GRANTED.call_once(|| {
            let missing: Vec<&str> = auth::NAME_SCOPES
                .into_iter()
                .filter(|wanted| !token.scope.split(' ').any(|s| s == *wanted))
                .collect();
            if missing.is_empty() {
                tracing::info!(scope = %token.scope, "Lark sign-in granted");
            } else {
                tracing::warn!(
                    scope = %token.scope,
                    missing = %missing.join(" "),
                    "the Lark sign-in lacks a permission for names, so people show as ids; sign in again"
                );
            }
        });
        let brand = secret.brand;
        let position = before.clone();
        let mut known = std::mem::take(names);
        let (result, back) = tokio::task::spawn_blocking(move || {
            let remote = Http::new(brand, &token);
            let result = sync::round(&remote, &mut known, &position, now_ms());
            (result, known)
        })
        .await
        .map_err(|e| Fault::transient(&account, e.to_string()))?;
        *names = back;
        match result {
            Ok(round) => break round,
            Err(ApiError::Unauthorized(detail)) if !force => {
                tracing::info!(%detail, "Lark refused the sign-in; renewing it");
                force = true;
            }
            Err(ApiError::Unauthorized(detail)) => {
                return Err(Fault::needs_user(
                    &account,
                    format!("Lark does not accept the sign-in ({detail}); sign in again"),
                ));
            }
            Err(ApiError::Forbidden(detail)) => return Err(Fault::needs_user(&account, detail)),
            Err(ApiError::Transient(detail)) => return Err(Fault::transient(&account, detail)),
        }
    };

    let (tasks, comments) = (round.tasks, round.comments);
    if !tasks.is_empty() || !comments.is_empty() {
        tracing::info!(tasks = tasks.len(), comments = comments.len(), "storing");
    }
    // Tasks before comments: a comment's thread is its task's.
    for chunk in tasks.chunks(BATCH) {
        core.store(chunk.to_vec(), Vec::new()).await?;
    }
    for chunk in comments.chunks(BATCH) {
        core.store(Vec::new(), chunk.to_vec()).await?;
    }
    if round.position != before {
        let json = serde_json::to_string(&round.position)
            .map_err(|e| Fault::transient(&account, e.to_string()))?;
        core.save_json(TASKS_SCOPE, json).await?;
    }
    Ok(())
}
