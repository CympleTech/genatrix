//! Turning what a connector fetched into what the store holds.
//!
//! Design: `docs/design/01-data-model.md`.
//!
//! This is where identity is decided, which is why it is here and not in the
//! connector: saying that an address belongs to a person is an assertion
//! about the world, and only the core has the rest of the world to check it
//! against.
//!
//! Idempotent throughout. The same message arriving twice stops at the raw
//! record, because `Source` is unique, and nothing after that runs. That is
//! what lets a connector refetch freely whenever it is unsure.

use chrono::Utc;
use genatrix_connector_imap::sync::Incoming;
use genatrix_model::{
    Blob, Connector, HandleKind, Item, ItemId, Level, Payload, Raw, Source, Thread, ThreadId,
    ThreadKind,
};
use genatrix_store::{FileStore, Store};

/// What happened to one message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ingested {
    /// It is new; here is the item.
    Added(ItemId),
    /// It was already here, byte for byte.
    AlreadyHad,
}

/// Store one fetched message.
pub fn mail(
    store: &Store,
    raw_files: &FileStore,
    blob_files: &FileStore,
    incoming: &Incoming,
) -> anyhow::Result<Ingested> {
    let source = Source::new(
        Connector::Imap,
        incoming.account.clone(),
        incoming.external_id.clone(),
    );
    let raw = Raw::describe(source.clone(), "message/rfc822", &incoming.raw);
    if !store.insert_raw(&raw)? {
        return Ok(Ingested::AlreadyHad);
    }
    raw_files.put(&incoming.raw)?;

    let mail = &incoming.mail;

    // Everyone this message names. A first sighting of an address makes a
    // person; the core never guesses that two addresses are one person.
    let author = match &mail.from {
        Some(from) => Some(store.person_for_handle(
            HandleKind::Email,
            &from.address,
            &from.name.clone().unwrap_or_default(),
        )?),
        None => None,
    };
    let mut recipients = Vec::new();
    for mailbox in mail.to.iter().chain(&mail.cc) {
        recipients.push(store.person_for_handle(
            HandleKind::Email,
            &mailbox.address,
            &mailbox.name.clone().unwrap_or_default(),
        )?);
    }

    let me = store.self_person()?.map(|p| p.id);
    let is_self = |person| me == Some(person);
    let direction = genatrix_model::item::direction_of(author, &recipients, is_self, true);

    let thread_id = store.upsert_thread(&Thread {
        id: ThreadId::new(),
        kind: ThreadKind::MailThread,
        source: Source::new(
            Connector::Imap,
            incoming.account.clone(),
            incoming.thread_key.clone(),
        ),
        title: Some(mail.subject.clone()),
        members: author
            .into_iter()
            .chain(recipients.iter().copied())
            .collect(),
        first_at: mail.date.map(|d| d.to_utc()),
        last_at: mail.date.map(|d| d.to_utc()),
    })?;

    let mut blobs = Vec::with_capacity(mail.attachments.len());
    for attachment in &mail.attachments {
        let hash = blob_files.put(&attachment.bytes)?;
        store.upsert_blob(&Blob {
            hash,
            mime: attachment.mime.clone(),
            size: attachment.bytes.len() as u64,
            name_hint: attachment.name.clone(),
        })?;
        blobs.push(hash);
    }

    let item = Item {
        id: ItemId::new(),
        source,
        raw_id: raw.id,
        supersedes: None,
        thread_id,
        // A message with no date at all is dated when we first saw it, which
        // is wrong but knowable, rather than being dropped or dated zero.
        occurred_at: mail.date.unwrap_or_else(|| Utc::now().into()),
        ingested_at: Utc::now(),
        direction,
        author,
        recipients,
        text: mail.text.clone(),
        blobs,
        // Every new item starts private and stays there until something
        // judges it. Design 02: the default fails safe.
        sensitivity: Level::default(),
        tombstoned: false,
        payload: Payload::Mail {
            subject: mail.subject.clone(),
            from: mail
                .from
                .as_ref()
                .map(|f| f.address.clone())
                .unwrap_or_default(),
            to: mail.to.iter().map(|m| m.address.clone()).collect(),
            cc: mail.cc.iter().map(|m| m.address.clone()).collect(),
            message_id: mail.message_id.clone(),
            in_reply_to: mail.in_reply_to.clone(),
            references: mail.references.clone(),
            labels: vec![],
            headers: mail.headers.clone(),
        },
    };
    store.insert_item(&item)?;
    Ok(Ingested::Added(item.id))
}
