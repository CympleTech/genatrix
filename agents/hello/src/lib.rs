//! The smallest useful agent: it keeps a list of the mail it has seen in
//! its own space, and when you write to it, says how many and proposes a
//! note you can approve.

use genatrix_agent_sdk::types::{Card, Field, FieldValue, Value};
use genatrix_agent_sdk::{Guest, actions, export_agent, items, log, space};

struct Hello;

fn ensure_tables() -> Result<(), String> {
    space::execute(
        "CREATE TABLE IF NOT EXISTS seen (id TEXT PRIMARY KEY, title TEXT)",
        &[],
    )?;
    space::execute(
        "CREATE TABLE IF NOT EXISTS note (at INTEGER, text TEXT)",
        &[],
    )?;
    Ok(())
}

impl Guest for Hello {
    fn on_items(ids: Vec<String>) -> Result<(), String> {
        ensure_tables()?;
        for id in ids {
            let Some(item) = items::get(&id) else {
                continue;
            };
            let title = item.title.unwrap_or_default();
            log(format!("seen {title}"));
            space::execute(
                "INSERT OR IGNORE INTO seen (id, title) VALUES (?1, ?2)",
                &[Value::Text(item.id), Value::Text(title)],
            )?;
        }
        Ok(())
    }

    fn on_message(text: String) -> Result<String, String> {
        ensure_tables()?;
        let rows = space::query("SELECT count(*) FROM seen", &[])?;
        let seen = match rows.first().and_then(|r| r.first()) {
            Some(Value::Integer(n)) => *n,
            _ => 0,
        };
        let card = Card {
            title: "Keep a note".into(),
            fields: vec![Field {
                label: "Note".into(),
                value: FieldValue::Text(text.clone()),
            }],
            evidence: Vec::new(),
        };
        actions::propose("note", &card, &text)?;
        Ok(format!(
            "I have seen {seen} mails. I proposed keeping your note."
        ))
    }

    fn on_schedule(_name: String) -> Result<(), String> {
        Ok(())
    }

    fn apply(kind: String, payload: String) -> Result<(), String> {
        if kind != "note" {
            return Err(format!("unknown kind {kind}"));
        }
        ensure_tables()?;
        let now = genatrix_agent_sdk::host::now_ms();
        space::execute(
            "INSERT INTO note (at, text) VALUES (?1, ?2)",
            &[Value::Integer(now), Value::Text(payload)],
        )?;
        Ok(())
    }
}

export_agent!(Hello with_types_in genatrix_agent_sdk);
