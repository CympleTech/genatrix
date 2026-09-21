//! Passwords in the macOS keychain.
//!
//! Design: `docs/design/08-storage.md` and `docs/design/05-connectors.md`,
//! "接入": credentials go into the keychain, never into a file beside the
//! data.
//!
//! This goes through `/usr/bin/security` rather than the Security framework,
//! on purpose. A keychain item remembers which program may read it, and for
//! an unsigned binary that is the binary's hash, so every rebuild during
//! development would bring a prompt. The `security` tool is signed by Apple
//! and never changes, so an item it created it can read, quietly, from any
//! build. The signed application (design 09) can move to the framework
//! without changing what is stored.
//!
//! The secret goes to the tool on its command line. That is a compromise,
//! named here: for the moment the process runs, another process of the same
//! user could read it from the process table. The alternative, the tool's
//! interactive mode over standard input, disables the user interaction the
//! keychain needs to add an item, and fails. Design 08 puts a malicious
//! process of the same user outside the threat model; the signed
//! application will use the framework and close this gap.

use std::process::{Command, Stdio};

/// What `security` reports when there is no such item.
const NOT_FOUND: i32 = 44;

/// One keychain service: a namespace for accounts.
#[derive(Clone, Debug)]
pub struct Keychain {
    service: String,
}

impl Keychain {
    /// Where mail passwords live.
    #[must_use]
    pub fn mail() -> Self {
        Self::named("Genatrix mail")
    }

    /// A keychain service by name.
    #[must_use]
    pub fn named(service: &str) -> Self {
        Self {
            service: service.to_owned(),
        }
    }

    /// Store a secret for an account, replacing any earlier one.
    pub fn store(&self, account: &str, secret: &str) -> anyhow::Result<()> {
        let out = Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-U",
                "-a",
                account,
                "-s",
                &self.service,
                "-w",
                secret,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()?;
        if !out.status.success() {
            anyhow::bail!(
                "the keychain refused to store the password: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// Read an account's secret, if one is stored.
    pub fn read(&self, account: &str) -> anyhow::Result<Option<String>> {
        let out = Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-a",
                account,
                "-s",
                &self.service,
                "-w",
            ])
            .stdin(Stdio::null())
            .output()?;
        if out.status.code() == Some(NOT_FOUND) {
            return Ok(None);
        }
        if !out.status.success() {
            anyhow::bail!(
                "the keychain could not be read: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let text = String::from_utf8(out.stdout)?;
        Ok(Some(text.trim_end_matches(['\n', '\r']).to_owned()))
    }

    /// Remove an account's secret. Says whether there was one.
    pub fn forget(&self, account: &str) -> anyhow::Result<bool> {
        let out = Command::new("/usr/bin/security")
            .args([
                "delete-generic-password",
                "-a",
                account,
                "-s",
                &self.service,
            ])
            .stdin(Stdio::null())
            .output()?;
        if out.status.code() == Some(NOT_FOUND) {
            return Ok(false);
        }
        if !out.status.success() {
            anyhow::bail!(
                "the keychain could not be changed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Touches the login keychain, so it is not part of the ordinary run:
    /// `cargo test -p genatrix-daemon -- --ignored keychain`.
    #[test]
    #[ignore = "writes to the login keychain"]
    fn a_password_goes_in_comes_back_and_goes_away() {
        let service = format!("Genatrix test {}", std::process::id());
        let chain = Keychain::named(&service);
        let account = "test@example.com";
        assert_eq!(chain.read(account).unwrap(), None);
        chain.store(account, "first \"quoted\" 密码").unwrap();
        assert_eq!(
            chain.read(account).unwrap().as_deref(),
            Some("first \"quoted\" 密码")
        );
        chain.store(account, "second").unwrap();
        assert_eq!(chain.read(account).unwrap().as_deref(), Some("second"));
        assert!(chain.forget(account).unwrap());
        assert!(!chain.forget(account).unwrap());
        assert_eq!(chain.read(account).unwrap(), None);
    }
}
