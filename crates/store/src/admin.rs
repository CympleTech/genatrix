//! The admin passphrase and the count of wrong tries.
//!
//! Design: `docs/design/06-interface.md` v0.7, "设备分两种"; design 02,
//! invariant 12. The store keeps a slow hash of the passphrase, made by the
//! core, never the passphrase. Five wrong tries in a row lock remote
//! administration until this machine unlocks it.

use rusqlite::params;

use crate::error::Result;
use crate::store::Store;

/// Wrong tries in a row that lock remote administration.
pub const ADMIN_TRIES: u32 = 5;

/// Where remote administration stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdminState {
    /// The passphrase's slow hash, as the core wrote it. None until set.
    pub passphrase: Option<String>,
    /// Wrong tries since the last right one or the last unlock.
    pub failures: u32,
    /// RFC 3339, when the fifth wrong try locked it.
    pub locked_at: Option<String>,
}

impl AdminState {
    /// Whether remote high-privilege requests are refused outright.
    #[must_use]
    pub const fn is_locked(&self) -> bool {
        self.locked_at.is_some()
    }
}

impl Store {
    /// Where remote administration stands.
    pub fn admin_state(&self) -> Result<AdminState> {
        self.conn()
            .query_row(
                "SELECT passphrase, failures, locked_at FROM admin WHERE id = 1",
                [],
                |r| {
                    Ok(AdminState {
                        passphrase: r.get(0)?,
                        failures: r.get(1)?,
                        locked_at: r.get(2)?,
                    })
                },
            )
            .map_err(Into::into)
    }

    /// Set or change the passphrase's hash. The count starts again; a lock
    /// stays until it is lifted on its own.
    pub fn set_admin_passphrase(&self, hash: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE admin SET passphrase = ?1, failures = 0 WHERE id = 1",
            params![hash],
        )?;
        Ok(())
    }

    /// Count a wrong try, locking at the fifth. Returns the count.
    pub fn admin_failed(&self, at: &str) -> Result<u32> {
        let conn = self.conn();
        conn.execute(
            "UPDATE admin SET failures = failures + 1,
                 locked_at = CASE WHEN failures + 1 >= ?1 AND locked_at IS NULL THEN ?2 ELSE locked_at END
             WHERE id = 1",
            params![ADMIN_TRIES, at],
        )?;
        conn.query_row("SELECT failures FROM admin WHERE id = 1", [], |r| r.get(0))
            .map_err(Into::into)
    }

    /// A right try: the count starts again.
    pub fn admin_succeeded(&self) -> Result<()> {
        self.conn()
            .execute("UPDATE admin SET failures = 0 WHERE id = 1", [])?;
        Ok(())
    }

    /// Lift the lock. This machine only; the caller checks.
    pub fn unlock_admin(&self) -> Result<()> {
        self.conn().execute(
            "UPDATE admin SET failures = 0, locked_at = NULL WHERE id = 1",
            [],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::store::Store;
    use crate::{DbKey, Device};

    use super::ADMIN_TRIES;

    #[test]
    fn five_wrong_tries_lock_until_unlocked() {
        let store = Store::open_in_memory(&DbKey::from_bytes([3; 32])).unwrap();
        let fresh = store.admin_state().unwrap();
        assert!(fresh.passphrase.is_none() && !fresh.is_locked());
        store.set_admin_passphrase("h").unwrap();
        for n in 1..ADMIN_TRIES {
            assert_eq!(store.admin_failed("t").unwrap(), n);
            assert!(!store.admin_state().unwrap().is_locked());
        }
        store.admin_failed("t").unwrap();
        assert!(store.admin_state().unwrap().is_locked());
        // A new passphrase does not lift it; only an unlock does.
        store.set_admin_passphrase("h2").unwrap();
        assert!(store.admin_state().unwrap().is_locked());
        store.unlock_admin().unwrap();
        let after = store.admin_state().unwrap();
        assert!(!after.is_locked() && after.failures == 0);
        assert_eq!(after.passphrase.as_deref(), Some("h2"));
    }

    #[test]
    fn a_revoked_device_is_no_longer_an_admin() {
        let store = Store::open_in_memory(&DbKey::from_bytes([3; 32])).unwrap();
        store
            .insert_device(&Device {
                id: "d".into(),
                name: "laptop".into(),
                secret_hash: "x".into(),
                created_at: "t".into(),
                last_seen: None,
                revoked_at: None,
                admin: false,
            })
            .unwrap();
        assert!(store.set_device_admin("d", true).unwrap());
        assert!(store.get_device("d").unwrap().unwrap().admin);
        assert!(store.revoke_device("d", "t2").unwrap());
        assert!(!store.get_device("d").unwrap().unwrap().admin);
        assert!(
            !store.set_device_admin("d", true).unwrap(),
            "not once revoked"
        );
    }
}
