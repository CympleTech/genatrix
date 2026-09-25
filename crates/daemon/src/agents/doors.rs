//! The doors, against the real store, gate and spaces.
//!
//! Every read goes through [`StoreDoors::scoped`], which builds the query
//! from the manifest's read scope and only narrows it by what the agent
//! asked. The agent's filter has no field that could widen it
//! (design 02, invariant 14).

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use genatrix_agent::run::ModelCaller;
use genatrix_gate::gate::{Initiator, Message, Request};
use genatrix_host::manifest::{Manifest, Purpose};
use genatrix_host::{Doors, wit};
use genatrix_llm::ticket::Purpose as GatePurpose;
use genatrix_model::{Connector, Direction, Item, ItemId, Kind, Level, Payload};
use genatrix_store::{ItemQuery, Space, SpaceValue, StoredAgent};

use crate::system::System;

/// One run's doors.
pub(super) struct StoreDoors {
    system: Arc<System>,
    agent: StoredAgent,
    manifest: Manifest,
    space: Arc<Space>,
    run: String,
    now: DateTime<Utc>,
    handle: tokio::runtime::Handle,
    /// Items handed to the agent so far, for the egress record's provenance.
    read: Vec<ItemId>,
}

impl StoreDoors {
    pub(super) fn new(
        system: Arc<System>,
        agent: StoredAgent,
        manifest: Manifest,
        space: Arc<Space>,
        run: String,
        now: DateTime<Utc>,
        handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            system,
            agent,
            manifest,
            space,
            run,
            now,
            handle,
            read: Vec::new(),
        }
    }

    /// The read scope as a query, narrowed by the agent's filter. `None`
    /// when the manifest reads nothing.
    pub(super) fn scoped(&self, filter: &wit::Filter) -> Option<ItemQuery> {
        let r = &self.manifest.reads;
        let connectors: Vec<Connector> = r
            .connectors
            .iter()
            .filter_map(|c| match c.as_str() {
                "imap" => Some(Connector::Imap),
                "telegram" => Some(Connector::Telegram),
                _ => None,
            })
            .collect();
        if connectors.is_empty() {
            return None;
        }
        let kinds = r
            .kinds
            .iter()
            .filter_map(|k| match k.as_str() {
                "mail" => Some(Kind::Mail),
                "message" => Some(Kind::Message),
                "event" => Some(Kind::Event),
                _ => None,
            })
            .collect();
        let floor = r.days.map(|d| self.now - Duration::days(i64::from(d)));
        let asked = filter.since_ms.and_then(DateTime::from_timestamp_millis);
        let since = match (floor, asked) {
            (Some(f), Some(a)) => Some(f.max(a)),
            (f, a) => f.or(a),
        };
        Some(ItemQuery {
            since,
            until: filter.until_ms.and_then(DateTime::from_timestamp_millis),
            kinds,
            max_level: Some(r.max_level),
            connectors,
            with_attachment: r.with_attachment,
            attachment_mimes: r.mime.clone(),
            matching: r.matching.clone(),
            text: filter.text.clone(),
            limit: filter.limit.clamp(1, 200),
            ..ItemQuery::default()
        })
    }

    fn to_wit(&self, item: &Item) -> wit::Item {
        let store = &self.system.store;
        let author = item
            .author
            .and_then(|p| store.get_person(p).ok().flatten())
            .map(|p| p.display_name);
        let title = match &item.payload {
            Payload::Mail { subject, .. } => Some(subject.clone()),
            _ => store
                .get_thread(item.thread_id)
                .ok()
                .flatten()
                .and_then(|t| t.title),
        };
        let blobs = item
            .blobs
            .iter()
            .filter_map(|h| store.get_blob(h).ok().flatten())
            .map(|b| wit::BlobRef {
                id: b.hash.to_string(),
                name: b.name_hint,
                mime: b.mime,
                size: b.size,
            })
            .collect();
        wit::Item {
            id: item.id.to_string(),
            connector: item.source.connector.as_str().to_owned(),
            kind: match item.kind() {
                Kind::Mail => "mail",
                Kind::Message => "message",
                Kind::Event => "event",
                Kind::Note => "note",
                Kind::File => "file",
            }
            .to_owned(),
            direction: match item.direction {
                Direction::Inbound => "inbound",
                Direction::Outbound => "outbound",
                Direction::Internal => "internal",
                Direction::Neutral => "neutral",
            }
            .to_owned(),
            occurred_ms: item.occurred_at.timestamp_millis(),
            author,
            title,
            text: item.text.clone(),
            blobs,
            level: match item.sensitivity {
                Level::Public => wit::Level::Public,
                Level::Personal => wit::Level::Personal,
                Level::Secret => wit::Level::Secret,
            },
        }
    }

    fn found(&mut self, items: &[Item]) -> Vec<wit::Item> {
        let out = items.iter().map(|i| self.to_wit(i)).collect();
        self.read.extend(items.iter().map(|i| i.id));
        out
    }

    /// What goes to the gate for a model call.
    pub(super) fn request(
        &self,
        purpose: Purpose,
        messages: Vec<wit::Message>,
        level: Level,
    ) -> Request {
        Request {
            purpose: match purpose {
                Purpose::Classify => GatePurpose::Classify,
                Purpose::Extract => GatePurpose::Extract,
                Purpose::Summarize => GatePurpose::Summarize,
                Purpose::Draft => GatePurpose::Draft,
                Purpose::Translate => GatePurpose::Translate,
            },
            initiator: Initiator::Installed {
                agent: self.agent.id.clone(),
                version: self.agent.version.clone(),
                run: self.run.clone(),
                cloud: false,
            },
            items: self.read.clone(),
            level,
            identities: Vec::new(),
            messages: messages
                .into_iter()
                .map(|m| Message {
                    role: m.role,
                    content: m.content,
                })
                .collect(),
            max_tokens: Some(1024),
            temperature: None,
            stream: false,
        }
    }
}

fn space_value(v: wit::Value) -> SpaceValue {
    match v {
        wit::Value::Null => SpaceValue::Null,
        wit::Value::Integer(i) => SpaceValue::Integer(i),
        wit::Value::Real(f) => SpaceValue::Real(f),
        wit::Value::Text(s) => SpaceValue::Text(s),
        wit::Value::Bytes(b) => SpaceValue::Bytes(b),
    }
}

fn wit_value(v: SpaceValue) -> wit::Value {
    match v {
        SpaceValue::Null => wit::Value::Null,
        SpaceValue::Integer(i) => wit::Value::Integer(i),
        SpaceValue::Real(f) => wit::Value::Real(f),
        SpaceValue::Text(s) => wit::Value::Text(s),
        SpaceValue::Bytes(b) => wit::Value::Bytes(b),
    }
}

impl Doors for StoreDoors {
    fn query(&mut self, filter: &wit::Filter) -> Vec<wit::Item> {
        let Some(q) = self.scoped(filter) else {
            return Vec::new();
        };
        match self.system.store.query_items(&q) {
            Ok(items) => self.found(&items),
            Err(e) => {
                tracing::warn!(agent = %self.agent.id, error = %e, "an agent's query failed");
                Vec::new()
            }
        }
    }

    fn get(&mut self, id: &str) -> Option<wit::Item> {
        let id: ItemId = id.parse().ok()?;
        let whole = wit::Filter {
            since_ms: None,
            until_ms: None,
            text: None,
            limit: 1,
        };
        let mut q = self.scoped(&whole)?;
        q.ids = vec![id];
        let items = self.system.store.query_items(&q).ok()?;
        self.found(&items).into_iter().next()
    }

    fn blob_text(&mut self, _id: &str) -> Result<(String, Level), String> {
        Err("attachment text is not available yet".into())
    }

    fn call_model(
        &mut self,
        purpose: Purpose,
        messages: Vec<wit::Message>,
        level: Level,
    ) -> Result<String, String> {
        let request = self.request(purpose, messages, level);
        let caller = &self.system.caller;
        self.handle
            .block_on(caller.call(&request))
            .map(|called| called.reply.text)
            .map_err(|e| e.to_string())
    }

    fn execute(&mut self, sql: &str, params: Vec<wit::Value>) -> Result<u64, String> {
        let params: Vec<SpaceValue> = params.into_iter().map(space_value).collect();
        self.space.execute(sql, &params)
    }

    fn query_space(
        &mut self,
        sql: &str,
        params: Vec<wit::Value>,
    ) -> Result<Vec<Vec<wit::Value>>, String> {
        let params: Vec<SpaceValue> = params.into_iter().map(space_value).collect();
        self.space.query(sql, &params).map(|rows| {
            rows.into_iter()
                .map(|r| r.into_iter().map(wit_value).collect())
                .collect()
        })
    }

    fn propose(
        &mut self,
        _kind: &str,
        _card: wit::Card,
        _payload: String,
        _level: Level,
    ) -> Result<String, String> {
        Err("proposals are not available yet".into())
    }
}
