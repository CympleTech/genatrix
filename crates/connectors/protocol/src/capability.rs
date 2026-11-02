//! What one account is allowed to reach and do.
//!
//! Design: `docs/design/05-connectors.md`, "进程模型".
//!
//! Issued by the core when the user adds an account, and turned into a
//! sandbox profile for the connector process. A connector cannot widen its
//! own capability, because the capability is not its to write: the worst a
//! compromised IMAP connector can do is talk to the mail servers the user
//! approved and send mail as the accounts they added.

use std::collections::BTreeSet;
use std::fmt;

use genatrix_model::Connector;
use serde::{Deserialize, Serialize};

/// One host and port a connector may open a connection to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Host {
    /// Host name, lowercase.
    pub name: String,
    /// Port.
    pub port: u16,
}

impl Host {
    /// A host and port.
    pub fn new(name: impl AsRef<str>, port: u16) -> Self {
        Self {
            name: name.as_ref().trim().to_lowercase(),
            port,
        }
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.name, self.port)
    }
}

/// What a connector may do on behalf of one account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountCapability {
    /// Which connector this is for.
    pub connector: Connector,
    /// The account, as `Source::account` names it.
    pub account: String,
    /// Every host it may connect to. Reading and sending are separate hosts
    /// for mail, and both have to be here: leaving the submission host out
    /// would produce a connector that can read but silently fails to send.
    pub hosts: BTreeSet<Host>,
    /// Which approved actions it may carry out.
    pub effects: BTreeSet<String>,
}

impl AccountCapability {
    /// A capability for an account with no permissions yet.
    pub fn new(connector: Connector, account: impl Into<String>) -> Self {
        Self {
            connector,
            account: account.into(),
            hosts: BTreeSet::new(),
            effects: BTreeSet::new(),
        }
    }

    /// Allow a host.
    #[must_use]
    pub fn with_host(mut self, host: Host) -> Self {
        self.hosts.insert(host);
        self
    }

    /// Allow an effect, named as [`genatrix_agent::action::Effect::kind`]
    /// names it.
    #[must_use]
    pub fn with_effect(mut self, effect: impl Into<String>) -> Self {
        self.effects.insert(effect.into());
        self
    }

    /// Whether this capability permits reaching a host.
    #[must_use]
    pub fn may_reach(&self, host: &Host) -> bool {
        self.hosts.contains(host)
    }

    /// Whether this capability permits carrying out an effect.
    #[must_use]
    pub fn may_do(&self, effect: &str) -> bool {
        self.effects.contains(effect)
    }
}

/// Everything one connector process is allowed to do: the union over its
/// accounts, which is what the sandbox is built from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// One entry per account.
    pub accounts: Vec<AccountCapability>,
}

impl Grant {
    /// Collect capabilities into a grant.
    #[must_use]
    pub fn of(accounts: Vec<AccountCapability>) -> Self {
        Self { accounts }
    }

    /// Every host any of its accounts may reach.
    #[must_use]
    pub fn hosts(&self) -> BTreeSet<Host> {
        self.accounts
            .iter()
            .flat_map(|a| a.hosts.iter().cloned())
            .collect()
    }

    /// The capability for one account, if it has one.
    #[must_use]
    pub fn account(&self, account: &str) -> Option<&AccountCapability> {
        self.accounts.iter().find(|a| a.account == account)
    }

    /// Whether an account may reach a host. Unknown accounts may reach
    /// nothing, which is the answer that fails safe.
    #[must_use]
    pub fn may_reach(&self, account: &str, host: &Host) -> bool {
        self.account(account).is_some_and(|a| a.may_reach(host))
    }

    /// Whether an account may carry out an effect.
    #[must_use]
    pub fn may_do(&self, account: &str, effect: &str) -> bool {
        self.account(account).is_some_and(|a| a.may_do(effect))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail_account() -> AccountCapability {
        AccountCapability::new(Connector::Imap, "me@example.com")
            .with_host(Host::new("imap.gmail.com", 993))
            .with_host(Host::new("smtp.gmail.com", 587))
            .with_effect("send_mail")
    }

    #[test]
    fn a_capability_permits_what_it_lists_and_nothing_else() {
        let cap = mail_account();
        assert!(cap.may_reach(&Host::new("imap.gmail.com", 993)));
        assert!(cap.may_reach(&Host::new("smtp.gmail.com", 587)));
        assert!(
            !cap.may_reach(&Host::new("imap.gmail.com", 143)),
            "a port is part of it"
        );
        assert!(!cap.may_reach(&Host::new("evil.example", 993)));
        assert!(cap.may_do("send_mail"));
        assert!(!cap.may_do("send_message"));
    }

    #[test]
    fn host_names_are_normalized_so_case_cannot_slip_past() {
        assert_eq!(
            Host::new("  IMAP.Gmail.COM ", 993),
            Host::new("imap.gmail.com", 993)
        );
        assert!(mail_account().may_reach(&Host::new("IMAP.GMAIL.COM", 993)));
    }

    #[test]
    fn one_account_does_not_borrow_anothers_permission() {
        let grant = Grant::of(vec![
            mail_account(),
            AccountCapability::new(Connector::Imap, "work@example.org")
                .with_host(Host::new("mail.example.org", 993)),
        ]);
        assert!(grant.may_reach("me@example.com", &Host::new("imap.gmail.com", 993)));
        assert!(
            !grant.may_reach("work@example.org", &Host::new("imap.gmail.com", 993)),
            "sharing a process does not mean sharing a permission"
        );
        assert!(
            !grant.may_do("work@example.org", "send_mail"),
            "and an account with no send permission cannot send"
        );
    }

    #[test]
    fn an_account_nobody_granted_can_do_nothing() {
        let grant = Grant::of(vec![mail_account()]);
        assert!(!grant.may_reach("stranger@example.com", &Host::new("imap.gmail.com", 993)));
        assert!(!grant.may_do("stranger@example.com", "send_mail"));
        assert!(grant.account("stranger@example.com").is_none());
    }

    #[test]
    fn the_process_wide_host_set_is_the_union() {
        let grant = Grant::of(vec![
            mail_account(),
            AccountCapability::new(Connector::Imap, "work@example.org")
                .with_host(Host::new("mail.example.org", 993)),
        ]);
        let hosts = grant.hosts();
        assert_eq!(hosts.len(), 3);
        assert!(hosts.contains(&Host::new("mail.example.org", 993)));
    }
}
