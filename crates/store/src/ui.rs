//! Interfaces (design 15): which exist, their versions, and which one each
//! device uses. The files themselves live in the data directory, by hash;
//! the store keeps the record.

use rusqlite::{OptionalExtension, params};

use crate::error::Result;
use crate::store::Store;

/// One interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ui {
    /// Its name, as it is addressed.
    pub name: String,
    /// What it is called on the page.
    pub title: String,
    /// The approved version in use.
    pub current: Option<String>,
    /// The device that made it, or `local`.
    pub made_by: String,
    /// RFC 3339.
    pub created_at: String,
}

/// One version of an interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiVersion {
    /// The interface.
    pub ui: String,
    /// The hash of its manifest.
    pub hash: String,
    /// The manifest, JSON.
    pub manifest: String,
    /// What changed, in a line.
    pub note: String,
    /// RFC 3339.
    pub created_at: String,
    /// When it was approved, if it was.
    pub approved_at: Option<String>,
    /// Who approved it.
    pub approved_by: Option<String>,
    /// The source commit it was built from, when an executor built it.
    pub source_commit: Option<String>,
}

fn ui_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Ui> {
    Ok(Ui {
        name: r.get(0)?,
        title: r.get(1)?,
        current: r.get(2)?,
        made_by: r.get(3)?,
        created_at: r.get(4)?,
    })
}

fn version_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<UiVersion> {
    Ok(UiVersion {
        ui: r.get(0)?,
        hash: r.get(1)?,
        manifest: r.get(2)?,
        note: r.get(3)?,
        created_at: r.get(4)?,
        approved_at: r.get(5)?,
        approved_by: r.get(6)?,
        source_commit: r.get(7)?,
    })
}

const UI: &str = "name, title, current, made_by, created_at";
const VERSION: &str =
    "ui, hash, manifest, note, created_at, approved_at, approved_by, source_commit";

impl Store {
    /// Every interface, oldest first.
    pub fn uis(&self) -> Result<Vec<Ui>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!("SELECT {UI} FROM ui ORDER BY created_at, name"))?;
        let rows = stmt.query_map([], ui_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// One interface.
    pub fn ui(&self, name: &str) -> Result<Option<Ui>> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT {UI} FROM ui WHERE name = ?1"),
                [name],
                ui_row,
            )
            .optional()?)
    }

    /// Make an interface, with no version yet. False if the name is taken.
    pub fn create_ui(&self, name: &str, title: &str, made_by: &str, at: &str) -> Result<bool> {
        let n = self.conn().execute(
            "INSERT OR IGNORE INTO ui (name, title, made_by, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![name, title, made_by, at],
        )?;
        Ok(n == 1)
    }

    /// File a version, waiting for approval. Filing the same files again
    /// changes nothing.
    pub fn add_ui_version(
        &self,
        ui: &str,
        hash: &str,
        manifest: &str,
        note: &str,
        at: &str,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO ui_version (ui, hash, manifest, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ui, hash, manifest, note, at],
        )?;
        Ok(())
    }

    /// Say which source commit a version was built from.
    pub fn set_ui_version_commit(&self, ui: &str, hash: &str, commit: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE ui_version SET source_commit = ?3 WHERE ui = ?1 AND hash = ?2",
            params![ui, hash, commit],
        )?;
        Ok(())
    }

    /// An interface's versions, newest first.
    pub fn ui_versions(&self, ui: &str) -> Result<Vec<UiVersion>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {VERSION} FROM ui_version WHERE ui = ?1 ORDER BY created_at DESC, hash"
        ))?;
        let rows = stmt.query_map([ui], version_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// One version of an interface.
    pub fn ui_version(&self, ui: &str, hash: &str) -> Result<Option<UiVersion>> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT {VERSION} FROM ui_version WHERE ui = ?1 AND hash = ?2"),
                [ui, hash],
                version_row,
            )
            .optional()?)
    }

    /// Whether any interface has a version with these files.
    pub fn ui_hash_known(&self, hash: &str) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM ui_version WHERE hash = ?1 LIMIT 1",
                [hash],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The manifest of a version, by hash alone.
    pub fn ui_manifest(&self, hash: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT manifest FROM ui_version WHERE hash = ?1 LIMIT 1",
                [hash],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Approve a version and make it the one in use.
    pub fn approve_ui_version(&self, ui: &str, hash: &str, by: &str, at: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE ui_version SET approved_at = coalesce(approved_at, ?3),
                    approved_by = coalesce(approved_by, ?4)
             WHERE ui = ?1 AND hash = ?2",
            params![ui, hash, at, by],
        )?;
        tx.execute(
            "UPDATE ui SET current = ?2 WHERE name = ?1",
            params![ui, hash],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The interface a device uses, if it picked one.
    pub fn device_ui(&self, device: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT ui FROM device_ui WHERE device = ?1",
                [device],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Pick the interface a device uses.
    pub fn set_device_ui(&self, device: &str, ui: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO device_ui (device, ui) VALUES (?1, ?2)
             ON CONFLICT (device) DO UPDATE SET ui = excluded.ui",
            params![device, ui],
        )?;
        Ok(())
    }

    /// Which device uses which interface.
    pub fn device_uis(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT device, ui FROM device_ui ORDER BY device")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    #[test]
    fn an_interface_is_made_filed_approved_and_picked() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(
            dir.path().join("data.db"),
            &genatrix_keys::DbKey::from_bytes([1; 32]),
        )
        .unwrap();
        assert!(
            s.create_ui("phone", "Phone", "local", "2026-11-14T00:00:00Z")
                .unwrap()
        );
        assert!(
            !s.create_ui("phone", "Other", "d1", "2026-11-14T00:00:00Z")
                .unwrap()
        );
        s.add_ui_version("phone", "h1", "{}", "first", "2026-11-14T00:00:01Z")
            .unwrap();
        s.add_ui_version("phone", "h1", "{}", "again", "2026-11-14T00:00:02Z")
            .unwrap();
        assert_eq!(s.ui_versions("phone").unwrap().len(), 1);
        assert!(s.ui("phone").unwrap().unwrap().current.is_none());
        s.approve_ui_version("phone", "h1", "local", "2026-11-14T00:00:03Z")
            .unwrap();
        assert_eq!(
            s.ui("phone").unwrap().unwrap().current.as_deref(),
            Some("h1")
        );
        assert!(s.ui_hash_known("h1").unwrap());
        assert_eq!(s.device_ui("d1").unwrap(), None);
        s.set_device_ui("d1", "phone").unwrap();
        s.set_device_ui("d1", "default").unwrap();
        assert_eq!(s.device_ui("d1").unwrap().as_deref(), Some("default"));
        assert_eq!(s.device_uis().unwrap().len(), 1);
    }
}

/// One agent's view (design 11 v0.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentView {
    /// The agent's id.
    pub agent: String,
    /// The view's name.
    pub name: String,
    /// Its title, as the manifest gives it.
    pub title: String,
    /// The view, JSON.
    pub data: String,
    /// The level of the run that published it.
    pub level: String,
    /// RFC 3339.
    pub at: String,
}

impl Store {
    /// Keep a view, replacing the one before under its name.
    pub fn put_agent_view(&self, v: &AgentView) -> Result<()> {
        self.conn().execute(
            "INSERT INTO agent_view (agent, name, title, data, level, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (agent, name) DO UPDATE SET title = excluded.title,
                data = excluded.data, level = excluded.level, at = excluded.at",
            params![v.agent, v.name, v.title, v.data, v.level, v.at],
        )?;
        Ok(())
    }

    /// One view.
    pub fn agent_view(&self, agent: &str, name: &str) -> Result<Option<AgentView>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT agent, name, title, data, level, at FROM agent_view
                 WHERE agent = ?1 AND name = ?2",
                [agent, name],
                |r| {
                    Ok(AgentView {
                        agent: r.get(0)?,
                        name: r.get(1)?,
                        title: r.get(2)?,
                        data: r.get(3)?,
                        level: r.get(4)?,
                        at: r.get(5)?,
                    })
                },
            )
            .optional()?)
    }

    /// Every view, without its data.
    pub fn agent_views(&self) -> Result<Vec<AgentView>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT agent, name, title, level, at FROM agent_view ORDER BY agent, name")?;
        let rows = stmt.query_map([], |r| {
            Ok(AgentView {
                agent: r.get(0)?,
                name: r.get(1)?,
                title: r.get(2)?,
                data: String::new(),
                level: r.get(3)?,
                at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The agents that have published a view.
    pub fn agent_view_agents(&self) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT DISTINCT agent FROM agent_view ORDER BY agent")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Forget an agent's views, when it is uninstalled.
    pub fn delete_agent_views(&self, agent: &str) -> Result<usize> {
        Ok(self
            .conn()
            .execute("DELETE FROM agent_view WHERE agent = ?1", [agent])?)
    }
}
