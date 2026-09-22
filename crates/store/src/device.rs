//! Paired devices: which browsers may reach the interface from another
//! address.
//!
//! Design: `docs/design/06-interface.md`, "形态"; `docs/design/08-storage.md`,
//! the network boundary. The store keeps a hash of each device's secret and
//! never the secret; a revoked device stays on record with the time.

use rusqlite::{OptionalExtension, params};

use crate::error::Result;
use crate::store::Store;

/// One paired device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// Identifier, public: it is in the cookie beside the secret.
    pub id: String,
    /// What the user called it, or what the browser said it was.
    pub name: String,
    /// SHA-256 of the secret, hex.
    pub secret_hash: String,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339, when it last made a request.
    pub last_seen: Option<String>,
    /// RFC 3339, when the user revoked it. A revoked device gets nothing.
    pub revoked_at: Option<String>,
}

impl Device {
    /// Whether requests from it are still accepted.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

fn row_to_device(r: &rusqlite::Row<'_>) -> rusqlite::Result<Device> {
    Ok(Device {
        id: r.get("id")?,
        name: r.get("name")?,
        secret_hash: r.get("secret_hash")?,
        created_at: r.get("created_at")?,
        last_seen: r.get("last_seen")?,
        revoked_at: r.get("revoked_at")?,
    })
}

impl Store {
    /// Record a newly paired device.
    pub fn insert_device(&self, device: &Device) -> Result<()> {
        self.conn().execute(
            "INSERT INTO device (id, name, secret_hash, created_at, last_seen, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                device.id,
                device.name,
                device.secret_hash,
                device.created_at,
                device.last_seen,
                device.revoked_at,
            ],
        )?;
        Ok(())
    }

    /// One device, revoked or not.
    pub fn get_device(&self, id: &str) -> Result<Option<Device>> {
        self.conn()
            .query_row(
                "SELECT id, name, secret_hash, created_at, last_seen, revoked_at
                 FROM device WHERE id = ?1",
                params![id],
                row_to_device,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Every device, newest first.
    pub fn all_devices(&self) -> Result<Vec<Device>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, secret_hash, created_at, last_seen, revoked_at
             FROM device ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_device)?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }

    /// Note that a device made a request.
    pub fn touch_device(&self, id: &str, at: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE device SET last_seen = ?2 WHERE id = ?1",
            params![id, at],
        )?;
        Ok(())
    }

    /// Revoke a device. Returns whether one was revoked by this call.
    pub fn revoke_device(&self, id: &str, at: &str) -> Result<bool> {
        let changed = self.conn().execute(
            "UPDATE device SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, at],
        )?;
        Ok(changed == 1)
    }
}
