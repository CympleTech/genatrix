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

    async fn pull_actions(&self) -> Result<Vec<protocol::ActionToDo>, Fault> {
        match self
            .call(Body::PullActions(protocol::PullActions {
                account: self.account.clone(),
            }))
            .await?
        {
            Body::Actions(a) => Ok(a.actions),
            _ => Err(Fault::transient(
                &self.account,
                "the core answered out of turn",
            )),
        }
    }

    async fn report(&self, report: protocol::Report) -> Result<(), Fault> {
        self.call(Body::Report(report)).await.map(|_| ())
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
        tasks.push(tokio::spawn(run_account(core, credentials, capability)));
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
async fn run_account(core: Core, credentials: Credentials, capability: AccountCapability) -> Fault {
    let mut attempt: u32 = 0;
    loop {
        core.status(&SyncState::Connecting).await;
        let fault = match once(&core, &credentials, &capability).await {
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
#[allow(clippy::too_many_lines)] // one life, told in order
async fn once(
    core: &Core,
    credentials: &Credentials,
    capability: &AccountCapability,
) -> Result<(), Fault> {
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

    // Approved replies, alongside the reading (design 05, "动作执行").
    let conversations = Arc::new(conversations);
    let executing = tokio::spawn(execute::run(
        core.clone(),
        client.clone(),
        capability.clone(),
        Arc::clone(&conversations),
    ));

    let outcome = backfill(core, &client, &conversations).await;
    save_session(core, &session).await?;
    if let Err(fault) = outcome {
        live.abort();
        executing.abort();
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
                executing.abort();
                runner.abort();
                return Ok(());
            }
        }
    }
}

/// Carrying out approved `send_message` actions.
///
/// The core has checked status, expiry, token and version before handing
/// one out. This side checks what only it can see, sends once, and
/// reports once. No retry: a message sent twice is worse than one that
/// failed (design 05).
mod execute {
    use std::sync::Arc;
    use std::time::Duration;

    use genatrix_connector::capability::AccountCapability;
    use genatrix_connector::protocol::{ActionToDo, Report, outcome};
    use genatrix_connector_telegram::sync::{self, Conversation};
    use grammers_client::Client;
    use grammers_client::InvocationError;
    use grammers_client::message::InputMessage;
    use serde::Deserialize;
    use sha2::{Digest, Sha256};

    use super::Core;

    /// How often the core is asked.
    const PULL_EVERY: Duration = Duration::from_secs(15);

    /// The `send_message` effect as the core serializes it.
    #[derive(Debug, Deserialize)]
    struct SendMessage {
        account: String,
        /// The thread key, `chat:<id>`.
        chat: String,
    }

    pub async fn run(
        core: Core,
        client: Client,
        capability: AccountCapability,
        conversations: Arc<Vec<Conversation>>,
    ) {
        loop {
            match core.pull_actions().await {
                Ok(actions) => {
                    for action in actions {
                        let report =
                            Box::pin(carry_out(&client, &capability, &conversations, action)).await;
                        if let Err(e) = core.report(report).await {
                            tracing::warn!(error = %e, "the core did not take the report");
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "could not ask for approved actions"),
            }
            tokio::time::sleep(PULL_EVERY).await;
        }
    }

    async fn carry_out(
        client: &Client,
        capability: &AccountCapability,
        conversations: &[Conversation],
        action: ActionToDo,
    ) -> Report {
        let mut report = Report {
            account: capability.account.clone(),
            action_id: action.id.clone(),
            token: action.token.clone(),
            outcome: outcome::FAILED.to_owned(),
            detail: String::new(),
            message: None,
            chat: None,
            message_id: None,
        };
        let conversation = match check(capability, conversations, &action) {
            Ok(c) => c,
            Err(detail) => {
                tracing::warn!(action = %action.id, %detail, "not sent");
                report.detail = detail;
                return report;
            }
        };
        let reply_to = action
            .reply_to_external_id
            .as_deref()
            .and_then(|id| id.rsplit_once("/msg:"))
            .and_then(|(_, n)| n.parse::<i32>().ok());
        let input = InputMessage::new()
            .text(action.payload.as_str())
            .reply_to(reply_to);
        match Box::pin(client.send_message(conversation.peer, input)).await {
            Ok(message) => {
                tracing::info!(action = %action.id, "sent");
                outcome::EXECUTED.clone_into(&mut report.outcome);
                report.chat = sync::sent(conversation, &message);
            }
            Err(InvocationError::Rpc(e)) => {
                // Telegram answered, and the answer was no: nothing went out.
                report.detail = format!("Telegram refused: {e}");
                tracing::warn!(action = %action.id, detail = %report.detail, "not sent");
            }
            Err(e) => {
                // The request left and the answer did not come back.
                outcome::UNKNOWN.clone_into(&mut report.outcome);
                report.detail = format!("the connection failed before Telegram answered: {e}");
                tracing::warn!(action = %action.id, detail = %report.detail, "outcome unknown");
            }
        }
        report
    }

    fn check<'c>(
        capability: &AccountCapability,
        conversations: &'c [Conversation],
        action: &ActionToDo,
    ) -> Result<&'c Conversation, String> {
        if action.kind != "send_message" {
            return Err(format!(
                "the Telegram connector does not do {}",
                action.kind
            ));
        }
        if !capability.may_do(&action.kind) {
            return Err(format!(
                "{} was not granted {}",
                capability.account, action.kind
            ));
        }
        if hex::encode(Sha256::digest(action.payload.as_bytes())) != action.payload_hash {
            return Err("the words do not match the approved version".to_owned());
        }
        let effect: SendMessage = serde_json::from_str(&action.effect_json)
            .map_err(|e| format!("the effect could not be read: {e}"))?;
        if effect.account != capability.account {
            return Err(format!(
                "the action is for {}, and this is {}",
                effect.account, capability.account
            ));
        }
        let id: i64 = effect
            .chat
            .strip_prefix("chat:")
            .unwrap_or(&effect.chat)
            .parse()
            .map_err(|_| format!("{} is not a conversation", effect.chat))?;
        conversations
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| "the conversation is not one this account has open".to_owned())
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
