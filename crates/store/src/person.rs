//! Persons and handles.

use genatrix_model::{Confidence, Handle, HandleId, HandleKind, Person, PersonId};
use rusqlite::{OptionalExtension, Row, params};

use crate::error::{Error, Result, corrupt};
use crate::store::Store;

const T: &str = "person";
const H: &str = "handle";

fn handle_kind_to_col(k: HandleKind) -> &'static str {
    match k {
        HandleKind::Email => "email",
        HandleKind::TelegramId => "telegram_id",
        HandleKind::TelegramUsername => "telegram_username",
        HandleKind::Phone => "phone",
    }
}

fn handle_kind_from_col(s: &str) -> Result<HandleKind> {
    Ok(match s {
        "email" => HandleKind::Email,
        "telegram_id" => HandleKind::TelegramId,
        "telegram_username" => HandleKind::TelegramUsername,
        "phone" => HandleKind::Phone,
        other => return Err(corrupt(H, "kind", other)),
    })
}

fn confidence_to_col(c: Confidence) -> &'static str {
    match c {
        Confidence::Confirmed => "confirmed",
        Confidence::Inferred => "inferred",
    }
}

fn confidence_from_col(s: &str) -> Result<Confidence> {
    Ok(match s {
        "confirmed" => Confidence::Confirmed,
        "inferred" => Confidence::Inferred,
        other => return Err(corrupt(H, "confidence", other)),
    })
}

fn row_to_person(r: &Row<'_>) -> Result<Person> {
    let id: String = r.get("id")?;
    let merged: String = r.get("merged_from")?;
    Ok(Person {
        id: id.parse().map_err(|e| corrupt(T, "id", e))?,
        display_name: r.get("display_name")?,
        is_self: r.get::<_, i64>("is_self")? != 0,
        merged_from: serde_json::from_str(&merged).map_err(|e| corrupt(T, "merged_from", e))?,
    })
}

fn row_to_handle(r: &Row<'_>) -> Result<Handle> {
    let id: String = r.get("id")?;
    let person_id: String = r.get("person_id")?;
    let kind: String = r.get("kind")?;
    let confidence: String = r.get("confidence")?;
    Ok(Handle {
        id: id.parse().map_err(|e| corrupt(H, "id", e))?,
        person_id: person_id.parse().map_err(|e| corrupt(H, "person_id", e))?,
        kind: handle_kind_from_col(&kind)?,
        value: r.get("value")?,
        confidence: confidence_from_col(&confidence)?,
    })
}

impl Store {
    /// Insert or update a person.
    pub fn upsert_person(&self, person: &Person) -> Result<()> {
        self.conn().execute(
            "INSERT INTO person (id, display_name, is_self, merged_from)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                display_name = excluded.display_name,
                is_self = excluded.is_self,
                merged_from = excluded.merged_from",
            params![
                person.id.to_string(),
                person.display_name,
                i64::from(person.is_self),
                serde_json::to_string(&person.merged_from)?
            ],
        )?;
        Ok(())
    }

    /// Fetch a person by id.
    pub fn get_person(&self, id: PersonId) -> Result<Option<Person>> {
        self.conn()
            .query_row(
                "SELECT * FROM person WHERE id = ?1",
                [id.to_string()],
                |r| Ok(row_to_person(r)),
            )
            .optional()?
            .transpose()
    }

    /// The person marked as you, if any.
    pub fn self_person(&self) -> Result<Option<Person>> {
        self.conn()
            .query_row("SELECT * FROM person WHERE is_self = 1 LIMIT 1", [], |r| {
                Ok(row_to_person(r))
            })
            .optional()?
            .transpose()
    }

    /// Insert a handle. Fails if the same address is already attached to a
    /// person; use [`Store::find_handle`] first.
    pub fn insert_handle(&self, handle: &Handle) -> Result<()> {
        self.conn().execute(
            "INSERT INTO handle (id, person_id, kind, value, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                handle.id.to_string(),
                handle.person_id.to_string(),
                handle_kind_to_col(handle.kind),
                handle.value,
                confidence_to_col(handle.confidence)
            ],
        )?;
        Ok(())
    }

    /// Look up a handle by kind and normalized value.
    pub fn find_handle(&self, kind: HandleKind, value: &str) -> Result<Option<Handle>> {
        self.conn()
            .query_row(
                "SELECT * FROM handle WHERE kind = ?1 AND value = ?2",
                params![handle_kind_to_col(kind), value],
                |r| Ok(row_to_handle(r)),
            )
            .optional()?
            .transpose()
    }

    /// Resolve an address to a person, creating a new person with a single
    /// confirmed handle when the address has never been seen. This is the
    /// "a handle seen for the first time makes a person" rule of design 01.
    pub fn person_for_handle(
        &self,
        kind: HandleKind,
        raw_value: &str,
        display_name: &str,
    ) -> Result<PersonId> {
        let value = Handle::normalize(kind, raw_value);
        if let Some(h) = self.find_handle(kind, &value)? {
            return Ok(h.person_id);
        }
        let person = Person {
            id: PersonId::new(),
            display_name: if display_name.trim().is_empty() {
                value.clone()
            } else {
                display_name.trim().to_owned()
            },
            is_self: false,
            merged_from: Vec::new(),
        };
        let handle = Handle {
            id: HandleId::new(),
            person_id: person.id,
            kind,
            value,
            confidence: Confidence::Confirmed,
        };
        self.tx(|tx| {
            tx.execute(
                "INSERT INTO person (id, display_name, is_self, merged_from) VALUES (?1, ?2, 0, '[]')",
                params![person.id.to_string(), person.display_name],
            )?;
            tx.execute(
                "INSERT INTO handle (id, person_id, kind, value, confidence) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    handle.id.to_string(),
                    handle.person_id.to_string(),
                    handle_kind_to_col(handle.kind),
                    handle.value,
                    confidence_to_col(handle.confidence)
                ],
            )?;
            Ok(())
        })?;
        Ok(person.id)
    }

    /// Give a handle to another person: the account holder's own address
    /// found under a stray person, for instance. Says whether it moved.
    pub fn move_handle(&self, kind: HandleKind, value: &str, to: PersonId) -> Result<bool> {
        let value = Handle::normalize(kind, value);
        let n = self.conn().execute(
            "UPDATE handle SET person_id = ?3 WHERE kind = ?1 AND value = ?2 AND person_id <> ?3",
            params![handle_kind_to_col(kind), value, to.to_string()],
        )?;
        Ok(n > 0)
    }

    /// Items authored by one person become authored by another. Returns
    /// how many. Direction is not recomputed here; the caller re-derives.
    pub fn reassign_author(&self, from: PersonId, to: PersonId) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE item SET author = ?2 WHERE author = ?1",
            params![from.to_string(), to.to_string()],
        )?)
    }

    /// Handles of one person.
    pub fn handles_of(&self, person: PersonId) -> Result<Vec<Handle>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM handle WHERE person_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map([person.to_string()], |r| Ok(row_to_handle(r)))?;
        rows.map(|r| r?).collect()
    }

    /// Handles for several people at once, keyed by person.
    ///
    /// The People page draws a card per person and each card wants its
    /// handles. Asking one person at a time is one lock and one statement
    /// compiled per person, which is most of what that page used to cost.
    pub fn handles_for(
        &self,
        people: &[PersonId],
    ) -> Result<std::collections::BTreeMap<PersonId, Vec<Handle>>> {
        let mut out: std::collections::BTreeMap<PersonId, Vec<Handle>> =
            std::collections::BTreeMap::new();
        if people.is_empty() {
            return Ok(out);
        }
        let list = vec!["?"; people.len()].join(",");
        let ids: Vec<String> = people.iter().map(ToString::to_string).collect();
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT * FROM handle WHERE person_id IN ({list}) ORDER BY id"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            Ok(row_to_handle(r))
        })?;
        for row in rows {
            let handle = row??;
            out.entry(handle.person_id).or_default().push(handle);
        }
        Ok(out)
    }

    /// All persons. Used by export.
    pub fn all_persons(&self) -> Result<Vec<Person>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM person ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok(row_to_person(r)))?;
        rows.map(|r| r?).collect()
    }

    /// All handles. Used by export.
    pub fn all_handles(&self) -> Result<Vec<Handle>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM handle ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok(row_to_handle(r)))?;
        rows.map(|r| r?).collect()
    }

    /// Mark a person as you. Exactly one person may be self; marking a new
    /// one requires none to exist.
    pub fn set_self(&self, id: PersonId) -> Result<()> {
        self.tx(|tx| {
            let existing: Option<String> = tx
                .query_row("SELECT id FROM person WHERE is_self = 1", [], |r| r.get(0))
                .optional()?;
            if let Some(e) = existing
                && e != id.to_string()
            {
                return Err(Error::NotFound(format!(
                    "another person is already self: {e}"
                )));
            }
            let n = tx.execute(
                "UPDATE person SET is_self = 1 WHERE id = ?1",
                [id.to_string()],
            )?;
            if n == 0 {
                return Err(Error::NotFound(format!("person {id}")));
            }
            Ok(())
        })
    }
}
