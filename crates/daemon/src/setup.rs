//! Adding and removing accounts, the same way from the terminal and from the
//! page.
//!
//! Design: `docs/design/05-connectors.md`, "接入" and "在界面上接入";
//! `docs/design/02-trust-boundary.md`, "凭据". A mailbox's password is tried
//! against its server first and kept in the keychain only when it worked; a
//! Telegram session goes into the encrypted store. Either way the account's
//! own address becomes a handle of the user's person. Removing an account
//! takes it out of the account list and its secret out of the keychain or
//! the store; what was fetched stays.

use genatrix_connector::capability::Host;
use genatrix_connector_imap::{Credentials, Imap};
use genatrix_connector_telegram::login::SignedIn;
use genatrix_model::{Confidence, Connector, Handle, HandleId, HandleKind};

use crate::accounts::{Account as MailAccount, Accounts};
use crate::keychain::Keychain;
use crate::system::System;

/// Add a mailbox, or sign in to one again. The password is tried first;
/// nothing is stored when the server refuses it.
pub async fn add_mail(
    system: &System,
    address: &str,
    imap_host: Option<String>,
    password: &str,
) -> anyhow::Result<MailAccount> {
    let path = system.config.accounts_path();
    let mut accounts = Accounts::load(&path)?;
    accounts.add_mail(address, imap_host)?;
    let account = accounts.mail[&address.trim().to_lowercase()].clone();
    let password = password.trim();
    if password.is_empty() {
        anyhow::bail!("no password given; nothing was changed");
    }
    let credentials = Credentials {
        account: account.address.clone(),
        password: password.to_owned(),
        imap: Host::new(&account.imap_host, account.imap_port),
    };
    match Imap::connect(&credentials).await {
        Ok(imap) => {
            let _ = imap.logout().await;
        }
        Err(fault) => anyhow::bail!("{} The password was not stored.", fault.detail),
    }
    Keychain::mail().store(&account.address, password)?;
    accounts.save(&path)?;
    claim_address(system, HandleKind::Email, &account.address, "")?;
    Ok(account)
}

/// An account's address is the user's own (design 01). With no self person
/// yet, the address's person becomes it. With one already, the address
/// joins it; and if the store had the address as somebody else, which is
/// what happens when a second mailbox has been writing to the first, that
/// somebody is folded into the user: their messages become the user's own,
/// with directions recomputed, and they leave the list of people. Returns
/// how many items changed hands.
pub fn claim_address(
    system: &System,
    kind: HandleKind,
    value: &str,
    name: &str,
) -> anyhow::Result<usize> {
    let Some(me) = system.store.self_person()? else {
        let person = system.store.person_for_handle(kind, value, name)?;
        system.store.set_self(person)?;
        return Ok(0);
    };
    match system.store.find_handle(kind, value)? {
        Some(existing) if existing.person_id == me.id => Ok(0),
        Some(existing) => {
            let moved = system.store.fold_person(existing.person_id, me.id, true)?;
            tracing::info!(
                items = moved,
                "an account's address was somebody else; folded into you"
            );
            // The people and groups lists counted them as somebody else.
            *system
                .people
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            *system
                .groups
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            Ok(moved)
        }
        None => {
            system.store.insert_handle(&Handle {
                id: HandleId::new(),
                person_id: me.id,
                kind,
                value: Handle::normalize(kind, value),
                confidence: Confidence::Confirmed,
            })?;
            Ok(0)
        }
    }
}

/// Keep a Telegram sign-in: the session in the encrypted store, the account
/// in the list, its user id as a handle of the user's own person.
pub fn keep_telegram(system: &System, phone: &str, signed_in: &SignedIn) -> anyhow::Result<()> {
    system.store.put_sync_cursor(
        Connector::Telegram,
        phone,
        "session",
        &serde_json::to_string(&signed_in.session)?,
    )?;
    let path = system.config.accounts_path();
    let mut accounts = Accounts::load(&path)?;
    accounts.add_telegram(phone, signed_in.user_id, &signed_in.name);
    accounts.save(&path)?;

    claim_address(
        system,
        HandleKind::TelegramId,
        &signed_in.user_id.to_string(),
        &signed_in.name,
    )?;
    Ok(())
}

/// Which kind of account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A mailbox, named by its address.
    Mail,
    /// A Telegram account, named by its phone number.
    Telegram,
}

/// Remove an account. Returns whether there was anything to remove. What it
/// fetched stays: that is the user's data, and removing an account is not
/// deleting a history.
pub async fn remove(system: &System, kind: Kind, id: &str) -> anyhow::Result<bool> {
    let path = system.config.accounts_path();
    let mut accounts = Accounts::load(&path)?;
    let id = id.trim();
    let removed = match kind {
        Kind::Mail => {
            let address = id.to_lowercase();
            let listed = accounts.mail.remove(&address).is_some();
            let had = Keychain::mail().forget(&address)?;
            listed || had
        }
        Kind::Telegram => {
            let listed = accounts.telegram.remove(id).is_some();
            sign_out_telegram(system, id).await;
            // The session is the secret; without it the connector cannot
            // sign in, and a new sign-in starts from nothing.
            let had = system
                .store
                .delete_sync_cursor(Connector::Telegram, id, "session")?;
            listed || had
        }
    };
    accounts.save(&path)?;
    Ok(removed)
}

/// End a Telegram account's session on Telegram's side, best effort and
/// within ten seconds, so the device leaves the account's session list.
pub async fn sign_out_telegram(system: &System, phone: &str) {
    let Ok(Some(stored)) = system
        .store
        .get_sync_cursor(Connector::Telegram, phone, "session")
    else {
        return;
    };
    let (Ok(snapshot), Ok(credentials)) = (
        serde_json::from_str(&stored.cursor),
        genatrix_connector_telegram::Credentials::find(),
    ) else {
        return;
    };
    let ending = genatrix_connector_telegram::login::sign_out(&credentials, snapshot);
    match tokio::time::timeout(std::time::Duration::from_secs(10), ending).await {
        Ok(Ok(())) => tracing::info!(%phone, "the Telegram session was ended"),
        Ok(Err(e)) => tracing::warn!(%phone, error = %e, "the Telegram session could not be ended"),
        Err(_) => tracing::warn!(%phone, "ending the Telegram session timed out"),
    }
}
