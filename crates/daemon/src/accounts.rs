//! The accounts this copy of Genatrix reads, and what each may reach.
//!
//! Design: `docs/design/05-connectors.md` and
//! `docs/design/09-install-recover-migrate.md`.
//!
//! What is stored here is deliberately not secret: an address, a server, a
//! port. The password is not written down at all in this phase. Design 08
//! puts it in the keychain, and until that exists the honest arrangement is
//! to read it from the environment each run, so there is no password file to
//! leak in the meantime.

use std::collections::BTreeMap;
use std::path::Path;

use genatrix_connector::capability::{AccountCapability, Grant, Host};
use genatrix_model::Connector;
use serde::{Deserialize, Serialize};

/// One mailbox.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    /// The address, which is also the login name everywhere that matters.
    pub address: String,
    /// Where to read mail.
    pub imap_host: String,
    /// Port. 993 everywhere in practice.
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
    /// Where to submit mail. Absent until sending is built.
    #[serde(default)]
    pub smtp_host: Option<String>,
    /// Submission port.
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
}

const fn default_imap_port() -> u16 {
    993
}

const fn default_smtp_port() -> u16 {
    587
}

/// One Telegram account. The session itself is in the encrypted store, not
/// here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TelegramAccount {
    /// The phone number, international form, which is the account's name.
    pub phone: String,
    /// The account's own Telegram user id.
    pub user_id: i64,
    /// The account's display name at sign-in.
    pub name: String,
}

/// Telegram's production datacenters. The connector's sandbox is built from
/// these: with the port filter the OS offers, that means port 443 to
/// anywhere, which is weaker than for mail and is said so in design 05.
pub const TELEGRAM_HOSTS: &[&str] = &[
    "149.154.175.53",
    "149.154.167.51",
    "149.154.175.100",
    "149.154.167.91",
    "149.154.171.5",
];

/// Every account, as stored.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Accounts {
    /// Mail accounts, keyed by address.
    #[serde(default)]
    pub mail: BTreeMap<String, Account>,
    /// Telegram accounts, keyed by phone number.
    #[serde(default)]
    pub telegram: BTreeMap<String, TelegramAccount>,
}

impl Accounts {
    /// Read the account list, or an empty one if there is none yet.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        Ok(toml::from_str(&std::fs::read_to_string(path)?)?)
    }

    /// Write it back.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Add or replace a mail account, filling in the servers when they can be
    /// worked out from the address.
    pub fn add_mail(&mut self, address: &str, imap_host: Option<String>) -> anyhow::Result<()> {
        let address = address.trim().to_lowercase();
        let known = imap_host.or_else(|| known_servers(&address).map(|(imap, _)| imap.to_owned()));
        let Some(imap_host) = known else {
            anyhow::bail!(
                "could not work out the mail server for {address}. Pass --imap-host; \
                 your provider's help pages call it the IMAP server."
            );
        };
        let smtp_host = known_servers(&address).map(|(_, smtp)| smtp.to_owned());
        self.mail.insert(
            address.clone(),
            Account {
                address,
                imap_host,
                imap_port: default_imap_port(),
                smtp_host,
                smtp_port: default_smtp_port(),
            },
        );
        Ok(())
    }

    /// Add or replace a Telegram account.
    pub fn add_telegram(&mut self, phone: &str, user_id: i64, name: &str) {
        let phone = phone.trim().to_owned();
        self.telegram.insert(
            phone.clone(),
            TelegramAccount {
                phone,
                user_id,
                name: name.trim().to_owned(),
            },
        );
    }

    /// What each account is allowed to reach and do.
    #[must_use]
    pub fn grant(&self) -> Grant {
        let mail = self.mail.values().map(|account| {
            let mut capability = AccountCapability::new(Connector::Imap, &account.address)
                .with_host(Host::new(&account.imap_host, account.imap_port));
            if let Some(smtp) = &account.smtp_host {
                capability = capability
                    .with_host(Host::new(smtp, account.smtp_port))
                    .with_effect("send_mail");
            }
            capability
        });
        let telegram = self.telegram.values().map(|account| {
            let mut capability = AccountCapability::new(Connector::Telegram, &account.phone)
                .with_effect("send_message");
            for host in TELEGRAM_HOSTS {
                capability = capability.with_host(Host::new(host, 443));
            }
            capability
        });
        Grant::of(mail.chain(telegram).collect())
    }
}

/// Servers for the providers most people use, so the question never has to be
/// asked. Design 00's measure is someone who does not use a terminal, and
/// "what is your IMAP server" is a question they cannot answer.
pub(crate) fn known_servers(address: &str) -> Option<(&'static str, &'static str)> {
    let domain = address.split_once('@')?.1;
    Some(match domain {
        "gmail.com" | "googlemail.com" => ("imap.gmail.com", "smtp.gmail.com"),
        "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => {
            ("outlook.office365.com", "smtp-mail.outlook.com")
        }
        "icloud.com" | "me.com" | "mac.com" => ("imap.mail.me.com", "smtp.mail.me.com"),
        "yahoo.com" => ("imap.mail.yahoo.com", "smtp.mail.yahoo.com"),
        "qq.com" => ("imap.qq.com", "smtp.qq.com"),
        "163.com" => ("imap.163.com", "smtp.163.com"),
        "126.com" => ("imap.126.com", "smtp.126.com"),
        "fastmail.com" | "fastmail.fm" => ("imap.fastmail.com", "smtp.fastmail.com"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_provider_needs_only_an_address() {
        let mut accounts = Accounts::default();
        accounts.add_mail("Someone@Gmail.com", None).unwrap();
        let account = &accounts.mail["someone@gmail.com"];
        assert_eq!(account.imap_host, "imap.gmail.com");
        assert_eq!(account.smtp_host.as_deref(), Some("smtp.gmail.com"));
        assert_eq!(account.imap_port, 993);
    }

    #[test]
    fn an_unknown_provider_asks_rather_than_guessing() {
        let mut accounts = Accounts::default();
        let err = accounts
            .add_mail("me@small-company.example", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--imap-host"), "{err}");
        assert!(err.contains("IMAP server"), "and says what to look for");

        accounts
            .add_mail(
                "me@small-company.example",
                Some("mail.small-company.example".into()),
            )
            .unwrap();
        assert_eq!(accounts.mail.len(), 1);
    }

    #[test]
    fn the_grant_gives_each_account_only_its_own_servers() {
        let mut accounts = Accounts::default();
        accounts.add_mail("a@gmail.com", None).unwrap();
        accounts.add_mail("b@qq.com", None).unwrap();
        let grant = accounts.grant();

        assert!(grant.may_reach("a@gmail.com", &Host::new("imap.gmail.com", 993)));
        assert!(
            !grant.may_reach("b@qq.com", &Host::new("imap.gmail.com", 993)),
            "one account's server is not another's"
        );
        assert!(grant.may_do("a@gmail.com", "send_mail"));
        assert_eq!(grant.hosts().len(), 4, "two servers each");
    }

    #[test]
    fn an_account_with_no_submission_server_cannot_send() {
        let mut accounts = Accounts::default();
        accounts
            .add_mail("me@small.example", Some("mail.small.example".into()))
            .unwrap();
        let grant = accounts.grant();
        assert!(grant.may_reach("me@small.example", &Host::new("mail.small.example", 993)));
        assert!(
            !grant.may_do("me@small.example", "send_mail"),
            "reading is not sending"
        );
    }

    #[test]
    fn accounts_survive_a_round_trip_and_hold_no_secret() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.toml");
        let mut accounts = Accounts::default();
        accounts.add_mail("me@gmail.com", None).unwrap();
        accounts.save(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("imap.gmail.com"));
        assert!(
            !text.to_lowercase().contains("password"),
            "no password is written down: {text}"
        );
        assert_eq!(Accounts::load(&path).unwrap().mail.len(), 1);
    }

    #[test]
    fn a_missing_file_is_an_empty_list_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            Accounts::load(&dir.path().join("nope.toml"))
                .unwrap()
                .mail
                .is_empty()
        );
    }
}
