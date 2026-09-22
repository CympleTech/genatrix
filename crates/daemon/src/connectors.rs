//! Connector processes: started, sandboxed, listened to, restarted.
//!
//! Design: `docs/design/05-connectors.md`, "进程模型", and
//! `docs/design/02-trust-boundary.md`. The core is the server. It starts each
//! connector under a sandbox built from the union of its accounts'
//! capabilities, hands it a fresh token in the environment, and accepts one
//! hello with that token per start. The connector gets its accounts and
//! their secrets in the answer, and from then on only asks: store this
//! batch, where was I, I am here now, this is how the account is doing.
//!
//! What the socket is protected by, said plainly: it lives in the data
//! directory, readable by this user only, and a caller without the current
//! token is answered with a refusal and nothing else. That keeps another
//! process of the same user from collecting passwords by connecting; it
//! does not, and cannot, defend against one that has already read this
//! process's memory (design 08 puts that outside the threat model).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use genatrix_connector::capability::AccountCapability;
use genatrix_connector::protocol::{
    self, Assign, Assignment, Body, Envelope, Failure, TOKEN_ENV, read_frame, write_frame,
};
use genatrix_connector::{Fault, SyncState};
use genatrix_connector_imap::watch::Sink as _;
use genatrix_connector_imap::{ipc, sandbox};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, watch};

use crate::syncing::StoreSink;
use crate::system::System;

/// One account the mail connector is to look after.
pub struct Assigned {
    /// What it may reach and do.
    pub capability: AccountCapability,
    /// How it signs in.
    pub secret: String,
    /// Where its state is reported.
    pub status: watch::Sender<SyncState>,
}

/// A connector binary, if it can be found: named by the environment
/// (`GENATRIX_IMAP_BIN`, `GENATRIX_TELEGRAM_BIN`), or beside this
/// executable as `genatrix-<kind>`.
#[must_use]
pub fn binary_for(kind: &str) -> Option<PathBuf> {
    let env = format!("GENATRIX_{}_BIN", kind.to_uppercase());
    if let Some(path) = std::env::var_os(&env) {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let beside = std::env::current_exe()
        .ok()?
        .parent()?
        .join(format!("genatrix-{kind}"));
    beside.is_file().then_some(beside)
}

/// The mail connector binary, if it can be found.
#[must_use]
pub fn mail_binary() -> Option<PathBuf> {
    binary_for("imap")
}

struct Server {
    system: Arc<System>,
    /// `imap` or `telegram`: what the hello must say.
    kind: String,
    connector: genatrix_model::Connector,
    accounts: BTreeMap<String, Assigned>,
    /// The token the next hello has to carry. Taken on use, replaced on
    /// every start of the process.
    token: Mutex<Option<String>>,
}

impl Server {
    fn sink(&self, account: &str) -> Option<StoreSink> {
        self.accounts
            .contains_key(account)
            .then(|| StoreSink::for_connector(Arc::clone(&self.system), self.connector, account))
    }

    fn everyone(&self, state: &SyncState) {
        for assigned in self.accounts.values() {
            assigned.status.send_replace(state.clone());
        }
    }
}

/// Start the mail connector for these accounts and keep it running.
///
/// Returns once the socket is listening and the process has been started;
/// the accepting and the supervising carry on in the background for the
/// life of the daemon.
pub fn start_mail(
    system: Arc<System>,
    assigned: Vec<Assigned>,
    binary: PathBuf,
) -> anyhow::Result<()> {
    start_connector(
        system,
        "imap",
        genatrix_model::Connector::Imap,
        assigned,
        binary,
    )
}

/// Start the Telegram connector for these accounts and keep it running.
pub fn start_telegram(
    system: Arc<System>,
    assigned: Vec<Assigned>,
    binary: PathBuf,
) -> anyhow::Result<()> {
    start_connector(
        system,
        "telegram",
        genatrix_model::Connector::Telegram,
        assigned,
        binary,
    )
}

/// Start one kind of connector for these accounts and keep it running.
///
/// Each kind has its own socket, `run/<kind>.sock`, its own sandbox profile
/// and its own token. Returns once the socket is listening and the process
/// has been started; the accepting and the supervising carry on in the
/// background for the life of the daemon.
fn start_connector(
    system: Arc<System>,
    kind: &str,
    connector: genatrix_model::Connector,
    assigned: Vec<Assigned>,
    binary: PathBuf,
) -> anyhow::Result<()> {
    if assigned.is_empty() {
        return Ok(());
    }
    let run_dir = system.config.data_dir.join("run");
    std::fs::create_dir_all(&run_dir)?;
    let socket = run_dir.join(format!("{kind}.sock"));
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    restrict(&socket)?;

    let ports: BTreeSet<u16> = assigned
        .iter()
        .flat_map(|a| a.capability.hosts.iter().map(|h| h.port))
        .collect();
    let profile_path = run_dir.join(format!("{kind}.sb"));
    std::fs::write(
        &profile_path,
        sandbox::profile(&sandbox::Confinement {
            ports: &ports,
            data_dir: &system.config.data_dir,
            run_dir: &run_dir,
            binary: &binary,
        })?,
    )?;

    let server = Arc::new(Server {
        system,
        kind: kind.to_owned(),
        connector,
        accounts: assigned
            .into_iter()
            .map(|a| (a.capability.account.clone(), a))
            .collect(),
        token: Mutex::new(None),
    });

    let accepting = Arc::clone(&server);
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let server = Arc::clone(&accepting);
                    tokio::spawn(async move {
                        if let Err(e) = handle(server, stream).await {
                            tracing::warn!(error = %e, "connector connection ended");
                        }
                    });
                }
                Err(e) => {
                    tracing::error!(error = %e, "accepting a connector failed");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
    });

    tokio::spawn(supervise(server, binary, profile_path, socket));
    Ok(())
}

/// Only this user reads or writes the socket.
fn restrict(socket: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
}

/// Start the process, wait for it to end, say so, start it again.
async fn supervise(server: Arc<Server>, binary: PathBuf, profile: PathBuf, socket: PathBuf) {
    let mut attempt: u32 = 0;
    loop {
        let token = fresh_token();
        *server.token.lock().await = Some(token.clone());
        server.everyone(&SyncState::Connecting);

        let mut child = match spawn(&binary, &profile, &socket, &token) {
            Ok(child) => child,
            Err(e) => {
                tracing::error!(error = %e, "could not start the connector");
                attempt = attempt.saturating_add(1);
                let wait = Fault::backoff(attempt);
                server.everyone(&SyncState::Retrying {
                    detail: format!("the connector could not be started: {e}"),
                    attempt,
                    next_in_secs: wait.as_secs(),
                });
                tokio::time::sleep(wait).await;
                continue;
            }
        };
        tracing::info!(pid = child.id(), "connector started");
        let started = std::time::Instant::now();
        let status = child.wait().await;
        tracing::warn!(?status, "connector stopped");

        // A process that ran for a while before stopping has earned a
        // quick restart; one that keeps dying at once is backed off.
        attempt = if started.elapsed().as_secs() > 60 {
            0
        } else {
            attempt.saturating_add(1)
        };
        let wait = Fault::backoff(attempt);
        server.everyone(&SyncState::Retrying {
            detail: "the connector stopped".to_owned(),
            attempt,
            next_in_secs: wait.as_secs(),
        });
        tokio::time::sleep(wait).await;
    }
}

fn fresh_token() -> String {
    format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new())
}

/// The connector under the sandbox, with its token in the environment and
/// nothing else of ours. Dies with the daemon.
fn spawn(
    binary: &Path,
    profile: &Path,
    socket: &Path,
    token: &str,
) -> std::io::Result<tokio::process::Child> {
    let sandbox_exec = Path::new("/usr/bin/sandbox-exec");
    let mut command = if sandbox_exec.exists() {
        let mut c = tokio::process::Command::new(sandbox_exec);
        c.arg("-f").arg(profile).arg(binary);
        c
    } else {
        tracing::warn!("no sandbox-exec on this system; the mail connector runs unconfined");
        tokio::process::Command::new(binary)
    };
    command
        .arg("--core-socket")
        .arg(socket)
        .env(TOKEN_ENV, token)
        .env_remove("GENATRIX_IMAP_PASSWORD")
        .env_remove("GENATRIX_TICKET_KEY")
        .kill_on_drop(true)
        .spawn()
}

/// One connection from a connector, hello to close.
async fn handle(server: Arc<Server>, mut stream: UnixStream) -> anyhow::Result<()> {
    let Some(first) = read_frame(&mut stream).await? else {
        return Ok(());
    };
    let Some(Body::Hello(hello)) = first.body else {
        refuse(&mut stream, first.id, "say hello first").await?;
        return Ok(());
    };
    let expected = server.token.lock().await.take();
    if hello.connector != server.kind || expected.as_deref() != Some(hello.token.as_str()) {
        tracing::warn!(
            connector = %hello.connector,
            pid = hello.pid,
            "a connector connected without the current token"
        );
        refuse(&mut stream, first.id, "not the connector this core started").await?;
        return Ok(());
    }
    tracing::info!(kind = %server.kind, pid = hello.pid, "connector connected");

    let assign = Assign {
        accounts: server
            .accounts
            .values()
            .map(|a| {
                Ok(Assignment {
                    capability_json: serde_json::to_string(&a.capability)?,
                    secret: a.secret.clone(),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
    };
    write_frame(
        &mut stream,
        &Envelope {
            id: first.id,
            body: Some(Body::Assign(assign)),
        },
    )
    .await?;

    while let Some(frame) = read_frame(&mut stream).await? {
        let reply = match frame.body {
            Some(body) => answer(&server, body).await,
            None => Body::Failure(Failure {
                detail: "an empty frame".into(),
            }),
        };
        write_frame(
            &mut stream,
            &Envelope {
                id: frame.id,
                body: Some(reply),
            },
        )
        .await?;
    }
    Ok(())
}

async fn refuse(stream: &mut UnixStream, id: u64, detail: &str) -> std::io::Result<()> {
    write_frame(
        stream,
        &Envelope {
            id,
            body: Some(Body::Failure(Failure {
                detail: detail.to_owned(),
            })),
        },
    )
    .await
}

/// One request, one answer. A failure is an answer too.
async fn answer(server: &Server, body: Body) -> Body {
    let failure = |detail: String| Body::Failure(Failure { detail });
    match body {
        Body::Store(store) => {
            let Some(sink) = server.sink(&store.account) else {
                return failure(format!("{} is not an account of yours", store.account));
            };
            let batch: Vec<_> = store
                .messages
                .into_iter()
                .map(|m| ipc::from_wire(&store.account, m))
                .collect();
            let mail = match sink.store(&batch).await {
                Ok(new) => new,
                Err(fault) => return failure(fault.detail),
            };
            let chats = match sink.store_chats(&store.chats) {
                Ok(new) => new,
                Err(fault) => return failure(fault.detail),
            };
            Body::Stored(protocol::Stored {
                new: u32::try_from(mail + chats).unwrap_or(u32::MAX),
            })
        }
        Body::LoadCursor(load) => {
            let Some(sink) = server.sink(&load.account) else {
                return failure(format!("{} is not an account of yours", load.account));
            };
            match sink.load_json(&load.scope) {
                Ok(cursor_json) => Body::CursorLoaded(protocol::CursorLoaded { cursor_json }),
                Err(fault) => failure(fault.detail),
            }
        }
        Body::SaveCursor(save) => {
            let Some(sink) = server.sink(&save.account) else {
                return failure(format!("{} is not an account of yours", save.account));
            };
            match sink.save_json(&save.scope, &save.cursor_json) {
                Ok(()) => Body::Saved(protocol::Saved {}),
                Err(fault) => failure(fault.detail),
            }
        }
        Body::Status(status) => {
            let Some(assigned) = server.accounts.get(&status.account) else {
                return failure(format!("{} is not an account of yours", status.account));
            };
            match serde_json::from_str::<SyncState>(&status.state_json) {
                Ok(state) => {
                    assigned.status.send_replace(state);
                    Body::Ack(protocol::Ack {})
                }
                Err(e) => failure(format!("unreadable state: {e}")),
            }
        }
        Body::Hello(_) => failure("already said hello".into()),
        _ => failure("that is the core's line, not yours".into()),
    }
}

#[cfg(test)]
mod tests {
    use genatrix_connector::capability::Host;
    use genatrix_connector_imap::{Incoming, normalize};
    use genatrix_keys::TicketKey;
    use genatrix_model::Connector;

    use super::*;
    use crate::config::Config;

    /// A whole core in a temporary directory, with the gateway
    /// configuration it insists on.
    fn system() -> (tempfile::TempDir, Arc<System>) {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::under(dir.path());
        config.create_dirs().unwrap();
        let run = dir.path().join("run");
        std::fs::write(
            config.gateway_config_path(),
            format!(
                r#"socket = "{}"

[[models]]
name = "local"
model = "test"
context_length = 4096
purposes = ["classify", "extract", "embed", "identity_suggestion", "summarize", "draft", "translate", "search_rewrite", "plan"]
endpoint = {{ kind = "local_socket", path = "{}" }}
"#,
                run.join("g.sock").display(),
                run.join("i.sock").display()
            ),
        )
        .unwrap();
        let system = System::open(config, TicketKey::generate().unwrap()).unwrap();
        (dir, Arc::new(system))
    }

    fn server(system: Arc<System>) -> (Arc<Server>, watch::Receiver<SyncState>) {
        let (status, rx) = watch::channel(SyncState::Starting);
        let capability = AccountCapability::new(Connector::Imap, "me@example.com")
            .with_host(Host::new("imap.example.com", 993));
        let server = Server {
            system,
            kind: "imap".to_owned(),
            connector: Connector::Imap,
            accounts: [(
                "me@example.com".to_owned(),
                Assigned {
                    capability,
                    secret: "s3cret".to_owned(),
                    status,
                },
            )]
            .into_iter()
            .collect(),
            token: Mutex::new(Some("tok".to_owned())),
        };
        (Arc::new(server), rx)
    }

    fn incoming(n: u32) -> Incoming {
        let raw = format!(
            "From: Ann <ann@example.com>\r\nTo: me@example.com\r\nSubject: {n}\r\n\
             Date: Mon, 1 Sep 2026 10:00:00 +0800\r\nMessage-ID: <{n}@example.com>\r\n\
             Content-Type: text/plain\r\n\r\nbody {n}\r\n"
        )
        .into_bytes();
        let mail = normalize(&raw).unwrap();
        Incoming {
            account: "me@example.com".into(),
            external_id: format!("mid:{n}@example.com"),
            thread_key: format!("mid:{n}@example.com"),
            raw,
            mail,
        }
    }

    #[test]
    fn tokens_are_long_and_never_repeat() {
        let a = fresh_token();
        let b = fresh_token();
        assert!(a.len() >= 40);
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn a_batch_is_stored_once_and_the_cursors_round_trip() {
        let (_dir, system) = system();
        let (server, _) = server(Arc::clone(&system));

        let store = |n: u32| {
            Body::Store(protocol::Store {
                account: "me@example.com".into(),
                messages: vec![ipc::to_wire(&incoming(n))],
                chats: vec![],
            })
        };
        assert!(matches!(
            answer(&server, store(1)).await,
            Body::Stored(protocol::Stored { new: 1 })
        ));
        assert!(
            matches!(
                answer(&server, store(1)).await,
                Body::Stored(protocol::Stored { new: 0 })
            ),
            "the same message again is not new"
        );
        assert_eq!(system.store.count_items().unwrap(), 1);

        let load = || {
            Body::LoadCursor(protocol::LoadCursor {
                account: "me@example.com".into(),
                scope: "INBOX".into(),
            })
        };
        assert!(matches!(
            answer(&server, load()).await,
            Body::CursorLoaded(protocol::CursorLoaded { cursor_json: None })
        ));
        assert!(matches!(
            answer(
                &server,
                Body::SaveCursor(protocol::SaveCursor {
                    account: "me@example.com".into(),
                    scope: "INBOX".into(),
                    cursor_json: "{\"cursor\":\"imap_uid\",\"uidvalidity\":1,\"highest\":9}".into(),
                })
            )
            .await,
            Body::Saved(_)
        ));
        match answer(&server, load()).await {
            Body::CursorLoaded(protocol::CursorLoaded {
                cursor_json: Some(json),
            }) => assert!(json.contains("\"highest\":9")),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_reported_state_reaches_the_interface() {
        let (_dir, system) = system();
        let (server, rx) = server(system);
        let state = SyncState::Live {
            synced_at: chrono::Utc::now(),
        };
        let reply = answer(
            &server,
            Body::Status(protocol::Status {
                account: "me@example.com".into(),
                state_json: serde_json::to_string(&state).unwrap(),
            }),
        )
        .await;
        assert!(matches!(reply, Body::Ack(_)));
        assert!(matches!(*rx.borrow(), SyncState::Live { .. }));
    }

    #[tokio::test]
    async fn an_account_that_was_not_assigned_is_refused() {
        let (_dir, system) = system();
        let (server, _) = server(system);
        let reply = answer(
            &server,
            Body::Store(protocol::Store {
                account: "someone-else@example.com".into(),
                messages: vec![],
                chats: vec![],
            }),
        )
        .await;
        assert!(matches!(reply, Body::Failure(_)));
    }

    #[tokio::test]
    async fn only_the_process_with_the_token_gets_the_secrets() {
        let (_dir, system) = system();
        let (server, _) = server(system);

        // Wrong token: refused, and the token is spent either way.
        let (mine, theirs) = UnixStream::pair().unwrap();
        let handling = tokio::spawn(handle(Arc::clone(&server), theirs));
        let mut client = protocol::Client::over(mine);
        let err = client.hello("imap", "wrong").await.unwrap_err();
        assert!(matches!(err, protocol::CallError::Refused(_)));
        handling.await.unwrap().unwrap();

        // Right token, but it has been spent.
        *server.token.lock().await = Some("tok".to_owned());
        let (mine, theirs) = UnixStream::pair().unwrap();
        let handling = tokio::spawn(handle(Arc::clone(&server), theirs));
        let mut client = protocol::Client::over(mine);
        let assign = client.hello("imap", "tok").await.unwrap();
        assert_eq!(assign.accounts.len(), 1);
        assert_eq!(assign.accounts[0].secret, "s3cret");
        assert!(
            assign.accounts[0]
                .capability_json
                .contains("imap.example.com")
        );
        drop(client);
        handling.await.unwrap().unwrap();

        let (mine, theirs) = UnixStream::pair().unwrap();
        let handling = tokio::spawn(handle(Arc::clone(&server), theirs));
        let mut client = protocol::Client::over(mine);
        assert!(
            client.hello("imap", "tok").await.is_err(),
            "a token is good for one hello"
        );
        handling.await.unwrap().unwrap();
    }
}
