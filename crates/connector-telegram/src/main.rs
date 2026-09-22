//! `genatrix-telegram`: the Telegram connector, in its own process.
//!
//! Design: `docs/design/05-connectors.md`. Started by the core under a
//! sandbox with a token in the environment, like the mail connector: it
//! connects to the core's socket, learns its accounts, and for each one
//! loads the session the core kept, reads the dialogs, walks each history
//! newest first, and takes live updates, sending everything back over the
//! socket. It never touches the store, the keychain, or any host but
//! Telegram's.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use genatrix_connector::backfill::Progress;
use genatrix_connector::capability::AccountCapability;
use genatrix_connector::protocol::{self, Body, CallError, Client as CoreClient, TOKEN_ENV};
use genatrix_connector::{Cursor, Fault, SyncState};
use genatrix_connector_telegram::sync::{self, Conversation};
use genatrix_connector_telegram::{Credentials, JsonSession, Snapshot};
use grammers_client::client::UpdatesConfiguration;
use grammers_client::update::Update;
use grammers_client::{Client, SenderPool};
use tokio::sync::Mutex;

#[derive(Debug, Parser)]
#[command(name = "genatrix-telegram", about = "Genatrix Telegram connector")]
struct Args {
    /// The core's socket.
    #[arg(long)]
    core_socket: PathBuf,
}

/// Scope the session is kept under in the core's cursors.
const SESSION_SCOPE: &str = "session";
/// History pages.
const PAGE: usize = 100;
/// How far back groups and channels are read. Direct chats are read whole:
/// they are the user's own correspondence. A group or channel is a place
/// the user is in, not a conversation they had, and its history is read
/// only this far; everything from now on arrives live (design 05).
const GROUP_HISTORY_DAYS: i64 = 30;

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

    async fn store(&self, chats: Vec<protocol::ChatMessage>) -> Result<usize, Fault> {
        if chats.is_empty() {
            return Ok(0);
        }
        match self
            .call(Body::Store(protocol::Store {
                account: self.account.clone(),
                messages: vec![],
                chats,
            }))
            .await?
        {
            Body::Stored(s) => Ok(s.new as usize),
            _ => Err(Fault::transient(
                &self.account,
                "the core answered out of turn",
            )),
        }
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
    let assign = client.hello("telegram", &token).await?;
    let client = Arc::new(Mutex::new(client));
    tracing::info!(accounts = assign.accounts.len(), "assigned");

    let mut tasks = Vec::new();
    for assignment in assign.accounts {
        let capability: AccountCapability = serde_json::from_str(&assignment.capability_json)?;
        let credentials: Credentials = serde_json::from_str(&assignment.secret)?;
        let core = Core {
            client: Arc::clone(&client),
            account: capability.account.clone(),
        };
        tasks.push(tokio::spawn(run_account(core, credentials)));
    }
    for task in tasks {
        if let Ok(fault) = task.await {
            tracing::warn!(%fault, "account stopped");
        }
    }
    std::future::pending::<()>().await;
    Ok(())
}

/// Keep one account up to date across reconnections, until a fault only
/// the user can fix.
async fn run_account(core: Core, credentials: Credentials) -> Fault {
    let mut attempt: u32 = 0;
    loop {
        core.status(&SyncState::Connecting).await;
        let fault = match once(&core, &credentials).await {
            Ok(()) => Fault::transient(&core.account, "the update stream ended"),
            Err(fault) => fault,
        };
        if !fault.retryable() {
            core.status(&SyncState::after(&fault, attempt)).await;
            return fault;
        }
        attempt = attempt.saturating_add(1);
        core.status(&SyncState::after(&fault, attempt)).await;
        tokio::time::sleep(Fault::backoff(attempt)).await;
    }
}

/// One connected life: session in, dialogs, history alongside updates.
async fn once(core: &Core, credentials: &Credentials) -> Result<(), Fault> {
    let snapshot: Option<Snapshot> = match core.load_json(SESSION_SCOPE).await? {
        Some(json) => serde_json::from_str(&json).ok(),
        None => None,
    };
    if !snapshot.as_ref().is_some_and(Snapshot::signed_in) {
        return Err(Fault::needs_user(
            &core.account,
            "not signed in to Telegram; run `genatrix account --add-telegram <phone>`",
        ));
    }
    let session = Arc::new(JsonSession::from_snapshot(snapshot));
    let pool = SenderPool::new(Arc::clone(&session), credentials.api_id);
    let updates_rx = pool.updates;
    let client = Client::new(pool.handle.clone());
    let runner = tokio::spawn(pool.runner.run());

    let transient = |e: grammers_client::InvocationError| {
        Fault::transient(&core.account, format!("Telegram: {e}"))
    };
    match client.is_authorized().await {
        Ok(true) => {}
        Ok(false) => {
            runner.abort();
            return Err(Fault::needs_user(
                &core.account,
                "the Telegram session was revoked; sign in again with `genatrix account --add-telegram <phone>`",
            ));
        }
        Err(e) => {
            runner.abort();
            return Err(transient(e));
        }
    }

    let all = sync::conversations(&client).await.map_err(transient)?;
    let archived: std::collections::BTreeSet<i64> =
        all.iter().filter(|c| c.archived).map(|c| c.id).collect();
    let conversations: Vec<Conversation> = all.into_iter().filter(|c| !c.archived).collect();
    tracing::info!(
        conversations = conversations.len(),
        archived = archived.len(),
        "dialogs listed; archived ones are left alone"
    );
    save_session(core, &session).await?;

    // Live updates in their own task; history below, one page at a time.
    let mut stream = client
        .stream_updates(
            updates_rx,
            UpdatesConfiguration {
                catch_up: true,
                update_queue_limit: Some(2000),
            },
        )
        .await
        .map_err(|e| Fault::transient(&core.account, format!("update stream: {e}")))?;
    let live_core = core.clone();
    let mut live = tokio::spawn(async move {
        loop {
            match stream.next().await {
                Ok(Update::NewMessage(m) | Update::MessageEdited(m)) => {
                    if let Some(chat) = sync::from_update(&m)
                        && !archived.contains(&chat_id(&chat))
                        && let Err(e) = live_core.store(vec![chat]).await
                    {
                        tracing::warn!(error = %e, "could not store an update");
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "update stream failed");
                    return;
                }
            }
        }
    });

    let outcome = backfill(core, &client, &conversations).await;
    save_session(core, &session).await?;
    if let Err(fault) = outcome {
        live.abort();
        runner.abort();
        return Err(fault);
    }
    core.status(&SyncState::Live {
        synced_at: chrono::Utc::now(),
    })
    .await;

    // History is in. Stay for the updates; save the session now and then so
    // the update position survives a restart.
    loop {
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(300)) => {
                save_session(core, &session).await?;
                core.status(&SyncState::Live { synced_at: chrono::Utc::now() }).await;
            }
            _ = &mut live => {
                runner.abort();
                return Ok(());
            }
        }
    }
}

/// The conversation a wire message belongs to, from its thread key.
fn chat_id(chat: &protocol::ChatMessage) -> i64 {
    chat.thread_key
        .strip_prefix("chat:")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

async fn save_session(core: &Core, session: &JsonSession) -> Result<(), Fault> {
    let snapshot = session
        .snapshot()
        .map_err(|e| Fault::transient(&core.account, e.to_string()))?;
    let json = serde_json::to_string(&snapshot)
        .map_err(|e| Fault::transient(&core.account, format!("session: {e}")))?;
    core.save_json(SESSION_SCOPE, json).await
}

/// Every conversation's history, newest first, resumable per conversation.
async fn backfill(
    core: &Core,
    client: &Client,
    conversations: &[Conversation],
) -> Result<(), Fault> {
    let mut progress = Progress::default();
    for (n, conversation) in conversations.iter().enumerate() {
        let scope = format!("{}#history", sync::thread_key(conversation.id));
        let mut oldest: Option<i32> = None;
        if let Some(json) = core.load_json(&scope).await? {
            match serde_json::from_str::<Cursor>(&json) {
                Ok(Cursor::History { complete: true, .. }) => continue,
                Ok(Cursor::History {
                    oldest: Some(id), ..
                }) => oldest = id.parse().ok(),
                _ => {}
            }
        }
        let horizon = (conversation.kind != "direct")
            .then(|| chrono::Utc::now() - chrono::Duration::days(GROUP_HISTORY_DAYS));
        loop {
            let (mut page, page_oldest) = sync::history_page(client, conversation, oldest, PAGE)
                .await
                .map_err(|e| Fault::transient(&core.account, format!("Telegram: {e}")))?;
            let mut complete = page_oldest.is_none();
            if let Some(horizon) = horizon {
                let before = page.len();
                page.retain(|m| {
                    chrono::DateTime::parse_from_rfc3339(&m.date).is_ok_and(|d| d >= horizon)
                });
                if page.len() < before {
                    // The page crossed the horizon: what is older stays
                    // where it is, and this conversation is done.
                    complete = true;
                }
            }
            progress.done += page.len() as u64;
            core.store(page).await?;
            if let Some(id) = page_oldest {
                oldest = Some(id);
            }
            let cursor = Cursor::History {
                oldest: oldest.map(|o| o.to_string()),
                complete,
            };
            core.save_json(
                &scope,
                serde_json::to_string(&cursor)
                    .map_err(|e| Fault::transient(&core.account, e.to_string()))?,
            )
            .await?;
            progress.reached = Some(format!(
                "{} of {} conversations, in {}",
                n + 1,
                conversations.len(),
                if conversation.title.is_empty() {
                    "a chat".to_owned()
                } else {
                    conversation.title.clone()
                }
            ));
            core.status(&SyncState::Backfilling {
                progress: progress.clone(),
            })
            .await;
            if complete {
                break;
            }
        }
    }
    progress.complete = true;
    Ok(())
}
