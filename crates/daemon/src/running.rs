//! The connectors, running for the accounts as they are now, and started
//! again when that changes.
//!
//! Design: `docs/design/05-connectors.md`, "进程模型" and "在界面上接入". At
//! start, and after an account is added or removed from the page, the core
//! reads the account list, hands each connector its accounts and their
//! secrets, and keeps the tasks so the next change can stop them. Stopping
//! a supervising task kills its connector process; the cursors stay in the
//! store, so the new process resumes where the old one was.

use std::sync::Arc;

use genatrix_connector::SyncState;
use genatrix_connector::capability::Host;
use genatrix_connector_imap::{Credentials, Imap, run_account};

use crate::system::System;
use crate::{accounts, connectors, syncing};

/// Start every connector for the accounts in the list.
pub fn start(system: &Arc<System>) -> anyhow::Result<()> {
    let mut tasks = Vec::new();
    let config = &system.config;
    let accounts = accounts::Accounts::load(&config.accounts_path())?;
    let grant = accounts.grant();

    let mut assigned = Vec::new();
    for account in accounts.mail.values() {
        let status = system.accounts.track(&account.address);
        let secret = match password_for(&account.address) {
            Ok(Some(password)) => password,
            Ok(None) => {
                status.send_replace(SyncState::NeedsLogin {
                    detail: format!(
                        "no password stored; run `genatrix account --add {}`",
                        account.address
                    ),
                });
                continue;
            }
            Err(e) => {
                status.send_replace(SyncState::Retrying {
                    detail: format!("the keychain could not be read: {e}"),
                    attempt: 0,
                    next_in_secs: 0,
                });
                tracing::warn!(account = %account.address, error = %e, "keychain");
                continue;
            }
        };
        let host = Host::new(&account.imap_host, account.imap_port);
        let capability = match grant.account(&account.address) {
            Some(c) if c.may_reach(&host) => c.clone(),
            _ => {
                status.send_replace(SyncState::Stopped {
                    detail: format!("this account is not allowed to connect to {host}"),
                });
                continue;
            }
        };
        assigned.push(connectors::Assigned {
            capability,
            secret,
            status,
        });
    }

    tasks.extend(start_telegram_accounts(system, &accounts, &grant)?);

    if let Some(binary) = connectors::mail_binary() {
        tasks.extend(connectors::start_mail(system.clone(), assigned, binary)?);
    } else {
        {
            tracing::warn!(
                "genatrix-imap was not found beside this binary; running the mail connector \
                 inside the core, without a sandbox. `cargo build --workspace` builds it."
            );
            for connectors::Assigned {
                capability,
                secret,
                status,
            } in assigned
            {
                let Some(imap) = capability.hosts.iter().find(|h| h.port == 993).cloned() else {
                    status.send_replace(SyncState::Stopped {
                        detail: "no mail server to read from".to_owned(),
                    });
                    continue;
                };
                let credentials = Credentials {
                    account: capability.account.clone(),
                    password: secret,
                    imap,
                };
                let sink = syncing::StoreSink::new(Arc::clone(system), &capability.account);
                let run = run_account(
                    capability,
                    move || {
                        let credentials = credentials.clone();
                        async move { Imap::connect(&credentials).await }
                    },
                    sink,
                    status,
                );
                tasks.push(tokio::spawn(async move {
                    let _ = run.await;
                }));
            }
        }
    }
    remember(system, tasks);
    Ok(())
}

/// Stop every connector and start them again from the account list.
///
/// Waits for the old tasks to be gone before starting new ones: a stopped
/// supervising task is what kills its connector process, and two Telegram
/// connectors on one session at the same moment is a thing not to risk.
pub async fn restart(system: &Arc<System>) -> anyhow::Result<()> {
    let old: Vec<_> = system
        .connector_tasks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .drain(..)
        .collect();
    for task in &old {
        task.abort();
    }
    for task in old {
        let _ = task.await;
    }
    system.accounts.clear();
    start(system)
}

fn remember(system: &System, tasks: Vec<tokio::task::JoinHandle<()>>) {
    system
        .connector_tasks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend(tasks);
}

fn password_for(address: &str) -> anyhow::Result<Option<String>> {
    if let Ok(password) = std::env::var(crate::PASSWORD_ENV) {
        return Ok(Some(password));
    }
    crate::keychain::Keychain::mail().read(address)
}

fn start_telegram_accounts(
    system: &Arc<System>,
    accounts: &accounts::Accounts,
    grant: &genatrix_connector::capability::Grant,
) -> anyhow::Result<Vec<tokio::task::JoinHandle<()>>> {
    if accounts.telegram.is_empty() {
        return Ok(Vec::new());
    }
    let credentials = match genatrix_connector_telegram::Credentials::find() {
        Ok(c) => c,
        Err(e) => {
            for account in accounts.telegram.values() {
                system
                    .accounts
                    .track(&account.phone)
                    .send_replace(SyncState::Stopped {
                        detail: format!("no Telegram application credentials: {e}"),
                    });
            }
            return Ok(Vec::new());
        }
    };
    let secret = serde_json::to_string(&credentials)?;
    let assigned: Vec<connectors::Assigned> = accounts
        .telegram
        .values()
        .filter_map(|account| {
            let status = system.accounts.track(&account.phone);
            grant
                .account(&account.phone)
                .map(|capability| connectors::Assigned {
                    capability: capability.clone(),
                    secret: secret.clone(),
                    status,
                })
        })
        .collect();
    if let Some(binary) = connectors::binary_for("telegram") {
        return connectors::start_telegram(system.clone(), assigned, binary);
    }
    for a in &assigned {
        a.status.send_replace(SyncState::Stopped {
            detail:
                "genatrix-telegram is not beside this binary; `cargo build --release --workspace`"
                    .into(),
        });
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_restart_with_no_accounts_stops_nothing_and_starts_nothing() {
        let (_dir, system) = crate::system::test_system();
        start(&system).unwrap();
        restart(&system).await.unwrap();
        restart(&system).await.unwrap();
        assert!(system.accounts.snapshot().is_empty());
        assert!(system.connector_tasks.lock().unwrap().is_empty());
    }
}
