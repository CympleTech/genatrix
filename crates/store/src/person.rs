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

    /// Fold one person into another, in one transaction: every item they
    /// wrote or received, every commitment either way, and the direction of
    /// each item recomputed when `into` is the user. The absorbed person stays as
    /// a row with no items, named in `merged_from`, so the merge can be
    /// undone (design 01). Returns how many items changed.
    ///
    /// Used when an address the store had as somebody else turns out to be
    /// one of the user's own, because the user added it as an account.
    pub fn fold_person(&self, from: PersonId, into: PersonId, into_is_self: bool) -> Result<usize> {
        if from == into {
            return Ok(0);
        }
        let (f, t) = (from.to_string(), into.to_string());
        let quoted_from = format!("\"{f}\"");
        let quoted_into = format!("\"{t}\"");
        self.tx(|tx| {
            // The items touched, before anything moves, with what the
            // direction depends on.
            let mut stmt = tx.prepare(
                "SELECT id, author, recipients, direction FROM item
                 WHERE author = ?1 OR recipients LIKE ?2",
            )?;
            let touched: Vec<(String, Option<String>, String, String)> = stmt
                .query_map(params![f, format!("%{quoted_from}%")], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<std::result::Result<_, _>>()?;
            drop(stmt);
            for (id, author, recipients, direction) in &touched {
                let author = author
                    .as_deref()
                    .map(|a| if a == f { t.as_str() } else { a });
                let recipients = recipients.replace(&quoted_from, &quoted_into);
                // A message the source gives no direction to keeps none.
                let direction = if direction == "neutral" || !into_is_self {
                    direction.clone()
                } else {
                    let from_self = author == Some(t.as_str());
                    let to_self = recipients.contains(&quoted_into);
                    match (from_self, to_self) {
                        (true, true) => "internal",
                        (true, false) => "outbound",
                        (false, _) => "inbound",
                    }
                    .to_owned()
                };
                tx.execute(
                    "UPDATE item SET author = ?2, recipients = ?3, direction = ?4 WHERE id = ?1",
                    params![id, author, recipients, direction],
                )?;
            }
            tx.execute(
                "UPDATE commitment SET from_person = ?2 WHERE from_person = ?1",
                params![f, t],
            )?;
            tx.execute(
                "UPDATE commitment SET to_person = ?2 WHERE to_person = ?1",
                params![f, t],
            )?;
            tx.execute(
                "UPDATE handle SET person_id = ?2 WHERE person_id = ?1",
                params![f, t],
            )?;
            let merged: String = tx.query_row(
                "SELECT merged_from FROM person WHERE id = ?1",
                params![t],
                |r| r.get(0),
            )?;
            let mut merged: Vec<String> = serde_json::from_str(&merged).unwrap_or_default();
            if !merged.contains(&f) {
                merged.push(f.clone());
            }
            tx.execute(
                "UPDATE person SET merged_from = ?2 WHERE id = ?1",
                params![t, serde_json::to_string(&merged)?],
            )?;
            Ok(touched.len())
        })
    }

    /// Items authored by one person become authored by another. Returns
    /// how many. Direction is not recomputed here; the caller re-derives.
    pub fn reassign_author(&self, from: PersonId, to: PersonId) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE item SET author = ?2 WHERE author = ?1",
            params![from.to_string(), to.to_string()],
        )?)
    }

    /// Whether the user has ever written to this person: an outbound item
    /// with them among its recipients. What "a known contact" means when an
    /// agent's manifest limits whom it may write to (design 11).
    pub fn has_written_to(&self, person: PersonId) -> Result<bool> {
        let found: Option<i64> = self
            .conn()
            .query_row(
                "SELECT 1 FROM item i WHERE i.direction IN ('outbound', 'internal')
                   AND EXISTS (SELECT 1 FROM json_each(i.recipients) WHERE value = ?1)
                 LIMIT 1",
                [person.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
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
