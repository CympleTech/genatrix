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
use genatrix_host::manifest::{Effect, Manifest, Proposal, Purpose, TargetRule, Targets, ceiling};
use genatrix_host::{Doors, wit};
use genatrix_llm::ticket::Purpose as GatePurpose;
use genatrix_model::{Connector, Direction, HandleKind, Item, ItemId, Kind, Level, Payload};
use genatrix_store::{AgentState, ItemQuery, Space, SpaceValue, StoredAgent};

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
    /// Proposals made in this run, not yet in the run record.
    proposed: u64,
    /// In a trial, where proposals go instead of the approvals.
    trial: Option<Arc<std::sync::Mutex<Vec<Tried>>>>,
}

/// A proposal a trial run would have made.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Tried {
    /// The manifest's label for its kind.
    pub label: String,
    /// Whom a mail would reach, with its subject; `None` for the agent's
    /// own space. The page says which in its own language.
    pub to: Option<String>,
    /// The card, for an agent's own kind.
    pub card: Option<genatrix_agent::Card>,
    /// A mail's body, for an outward one.
    pub draft: Option<String>,
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
            proposed: 0,
            trial: None,
        }
    }

    /// The read scope as a query, narrowed by the agent's filter.
    pub(super) fn scoped(&self, filter: &wit::Filter) -> Option<ItemQuery> {
        scope(&self.manifest, self.now, filter)
    }

    /// Hold proposals instead of storing them: a trial run before install.
    pub(super) fn trial(mut self, sink: Arc<std::sync::Mutex<Vec<Tried>>>) -> Self {
        self.trial = Some(sink);
        self
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

/// A manifest's read scope as a query, narrowed by the agent's filter.
/// `None` when the manifest reads nothing.
pub(super) fn scope(
    manifest: &Manifest,
    now: DateTime<Utc>,
    filter: &wit::Filter,
) -> Option<ItemQuery> {
    let r = &manifest.reads;
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
    let floor = r.days.map(|d| now - Duration::days(i64::from(d)));
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
        kind: &str,
        card: wit::Card,
        payload: String,
        _level: Level,
    ) -> Result<String, String> {
        let spec = self
            .manifest
            .proposal(kind)
            .cloned()
            .ok_or_else(|| format!("`{kind}` is not declared in the manifest"))?;
        if self.trial.is_none() {
            self.within_cap()?;
        }
        let read: std::collections::HashSet<ItemId> = self.read.iter().copied().collect();
        // Evidence is what the agent actually read in this run, nothing it
        // merely names.
        let evidence: Vec<ItemId> = card
            .evidence
            .iter()
            .filter_map(|e| e.parse().ok())
            .filter(|e| read.contains(e))
            .collect();
        let rationale = format!("{} · {}", self.agent.name, spec.label);
        let (effect, draft) = match spec.effect {
            Effect::Own => (
                genatrix_agent::Effect::Agent {
                    agent: self.agent.id.clone(),
                    name: self.agent.name.clone(),
                    version: self.agent.version.clone(),
                    agent_kind: kind.to_owned(),
                    label: spec.label.clone(),
                    card: card_of(card),
                },
                payload,
            ),
            Effect::SendMail => self.outward_mail(&spec, &payload, &read)?,
            Effect::SendMessage | Effect::CreateEvent => {
                return Err("this kind of effect is not open to agents yet".into());
            }
        };
        if let Some(sink) = &self.trial {
            let (to, card, draft) = match &effect {
                genatrix_agent::Effect::Agent { card, .. } => (None, Some(card.clone()), None),
                genatrix_agent::Effect::SendMail { to, subject, .. } => (
                    Some(format!("{} · {subject}", to.join(", "))),
                    None,
                    Some(draft),
                ),
                other => (Some(other.kind().to_owned()), None, None),
            };
            let mut sink = sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            sink.push(Tried {
                label: spec.label.clone(),
                to,
                card,
                draft,
            });
            return Ok(format!("trial-{}", sink.len()));
        }
        let action = self
            .system
            .actions
            .propose(
                &self.system.store,
                &self.system.ledger,
                &self.run,
                effect,
                draft,
                rationale,
                evidence,
            )
            .map_err(|e| e.to_string())?;
        self.proposed += 1;
        Ok(action.id)
    }
}

impl StoreDoors {
    /// The daily limits: the manifest's for this agent, the core's for all
    /// of them. Going over pauses the agent until the user looks
    /// (design 11, ruling 8).
    fn within_cap(&mut self) -> Result<(), String> {
        let since = (self.now - Duration::days(1)).timestamp_millis();
        let store = &self.system.store;
        let mine = store
            .agent_proposals_since(Some(&self.agent.id), since)
            .map_err(|e| e.to_string())?
            + self.proposed;
        let everyone = store
            .agent_proposals_since(None, since)
            .map_err(|e| e.to_string())?
            + self.proposed;
        let limit = u64::from(self.manifest.quota.proposals_per_day);
        if mine >= limit || everyone >= u64::from(ceiling::PROPOSALS_PER_DAY) {
            let _ = store.set_agent_state(&self.agent.id, AgentState::Paused);
            tracing::warn!(agent = %self.agent.id, "an agent reached its daily proposals and was paused");
            return Err("the daily limit on proposals is reached; the agent is paused".into());
        }
        Ok(())
    }

    /// A mail, to whom the manifest allows and no one else (design 02,
    /// invariant 21). Refused here, before anything reaches the approvals.
    fn outward_mail(
        &self,
        spec: &Proposal,
        payload: &str,
        read: &std::collections::HashSet<ItemId>,
    ) -> Result<(genatrix_agent::Effect, String), String> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Mail {
            to: Vec<String>,
            subject: String,
            body: String,
            #[serde(default)]
            reply_to: Option<String>,
        }
        let mail: Mail = serde_json::from_str(payload)
            .map_err(|e| format!("a mail is {{to, subject, body, reply_to?}}: {e}"))?;
        let to: Vec<String> = mail.to.iter().map(|a| bare(a)).collect();
        if to.is_empty() || to.iter().any(|a| !a.contains('@')) {
            return Err("a mail needs at least one address".into());
        }
        let store = &self.system.store;
        let replied = match &mail.reply_to {
            None => None,
            Some(id) => {
                let id: ItemId = id
                    .parse()
                    .map_err(|_| "reply_to is not an item".to_owned())?;
                if !read.contains(&id) {
                    return Err("reply_to must be an item read in this run".into());
                }
                store.get_item(id).map_err(|e| e.to_string())?
            }
        };
        let allowed = match &spec.targets {
            Some(Targets::Addresses(list)) => {
                let list: Vec<String> = list.iter().map(|a| bare(a)).collect();
                to.iter().all(|a| list.contains(a))
            }
            Some(Targets::Rule(TargetRule::SameThread)) => {
                let Some(Item {
                    payload:
                        Payload::Mail {
                            from, to: t, cc, ..
                        },
                    ..
                }) = &replied
                else {
                    return Err("same_thread needs reply_to on a mail".into());
                };
                let thread: Vec<String> = std::iter::once(from)
                    .chain(t)
                    .chain(cc)
                    .map(|a| bare(a))
                    .collect();
                to.iter().all(|a| thread.contains(a))
            }
            Some(Targets::Rule(TargetRule::KnownContacts)) => to.iter().all(|a| {
                store
                    .find_handle(HandleKind::Email, a)
                    .ok()
                    .flatten()
                    .is_some_and(|h| store.has_written_to(h.person_id).unwrap_or(false))
            }),
            None => false,
        };
        if !allowed {
            return Err("an address is outside what the manifest allows".into());
        }
        let (account, in_reply_to, references) = if let Some(Item {
            source,
            payload:
                Payload::Mail {
                    message_id,
                    references,
                    ..
                },
            ..
        }) = &replied
        {
            let mut chain = references.clone();
            chain.extend(message_id.clone());
            (source.account.clone(), message_id.clone(), chain)
        } else {
            let accounts = crate::accounts::Accounts::load(&self.system.config.accounts_path())
                .map_err(|e| e.to_string())?;
            let first = accounts
                .mail
                .keys()
                .next()
                .cloned()
                .ok_or_else(|| "there is no mail account to send from".to_owned())?;
            (first, None, Vec::new())
        };
        Ok((
            genatrix_agent::Effect::SendMail {
                account,
                to,
                subject: mail.subject,
                in_reply_to,
                references,
            },
            mail.body,
        ))
    }
}

/// The address in `Name <address>`, lowercased.
fn bare(address: &str) -> String {
    let a = match (address.find('<'), address.rfind('>')) {
        (Some(open), Some(close)) if open < close => &address[open + 1..close],
        _ => address,
    };
    a.trim().to_lowercase()
}

fn card_of(card: wit::Card) -> genatrix_agent::Card {
    use genatrix_agent::{CardField, CardValue};
    genatrix_agent::Card {
        title: card.title.chars().take(200).collect(),
        fields: card
            .fields
            .into_iter()
            .take(40)
            .map(|f| CardField {
                label: f.label.chars().take(80).collect(),
                value: match f.value {
                    wit::FieldValue::Text(text) => CardValue::Text {
                        text: text.chars().take(2000).collect(),
                    },
                    wit::FieldValue::Money(m) => CardValue::Money {
                        cents: m.cents,
                        currency: m.currency.chars().take(3).collect(),
                    },
                    wit::FieldValue::Date(date) => CardValue::Date {
                        date: date.chars().take(10).collect(),
                    },
                },
            })
            .collect(),
    }
}
