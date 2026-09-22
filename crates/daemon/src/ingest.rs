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

/// Turn one fetched message into an item, unless it is already here.
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
    // A mail this account sent through a connector was kept under its own
    // `Message-ID` before the server ever saw it. When sync brings the
    // server's copy back under the server's identifier, it is the same
    // message, and design 05 has it skipped, not doubled.
    if !incoming.external_id.starts_with("mid:")
        && let Some(message_id) = &incoming.mail.message_id
        && store
            .current_item(&Source::new(
                Connector::Imap,
                incoming.account.clone(),
                format!("mid:{message_id}"),
            ))?
            .is_some()
    {
        return Ok(Ingested::AlreadyHad);
    }
    let raw = Raw::describe(source.clone(), "message/rfc822", &incoming.raw);
    if !store.insert_raw(&raw)? {
        return Ok(Ingested::AlreadyHad);
    }
    raw_files.put(&incoming.raw)?;

    let item = build(
        store,
        blob_files,
        source,
        raw.id,
        &incoming.mail,
        &incoming.thread_key,
        None,
    )?;
    store.insert_item(&item)?;
    Ok(Ingested::Added(item.id))
}

/// Turn one chat message into an item, unless it is already here. An edit
/// arrives as the same message with different bytes: a new raw record, and
/// a new item that supersedes the current one (design 01).
pub fn chat(
    store: &Store,
    raw_files: &FileStore,
    account: &str,
    message: &genatrix_connector::protocol::ChatMessage,
) -> anyhow::Result<Ingested> {
    use genatrix_model::PersonId;

    let source = Source::new(Connector::Telegram, account, message.external_id.clone());
    let raw = Raw::describe(
        source.clone(),
        "application/x-telegram-message+tl",
        &message.raw,
    );
    if !store.insert_raw(&raw)? {
        return Ok(Ingested::AlreadyHad);
    }
    raw_files.put(&message.raw)?;

    let me = store.self_person()?.map(|p| p.id);
    let author = chat_author(store, me, message)?;
    // In a direct chat the other party is the conversation itself: its key
    // is their Telegram id. So an outgoing message is to them, an incoming
    // one to the user.
    let recipients: Vec<PersonId> = match (message.thread_kind.as_str(), message.outgoing, me) {
        ("direct", false, Some(me)) => vec![me],
        ("direct", true, _) => direct_peer(store, &message.thread_key, &message.thread_title)?
            .into_iter()
            .collect(),
        _ => Vec::new(),
    };
    let is_self = |person| me == Some(person);
    let direction = genatrix_model::item::direction_of(author, &recipients, is_self, true);

    let kind = match message.thread_kind.as_str() {
        "direct" => ThreadKind::DirectChat,
        "channel" => ThreadKind::Channel,
        _ => ThreadKind::GroupChat,
    };
    let thread_id = store.upsert_thread(&Thread {
        id: ThreadId::new(),
        kind,
        source: Source::new(Connector::Telegram, account, message.thread_key.clone()),
        title: (!message.thread_title.is_empty()).then(|| message.thread_title.clone()),
        members: author
            .into_iter()
            .chain(recipients.iter().copied())
            .collect(),
        first_at: None,
        last_at: None,
    })?;

    let reply_to = match &message.reply_to {
        Some(external) => store
            .current_item(&Source::new(Connector::Telegram, account, external.clone()))?
            .map(|i| i.id),
        None => None,
    };
    let current = store.current_item(&source)?;
    let occurred_at =
        chrono::DateTime::parse_from_rfc3339(&message.date).unwrap_or_else(|_| Utc::now().into());

    let text = chat_text(message);

    let item = Item {
        id: ItemId::new(),
        source,
        raw_id: raw.id,
        supersedes: current.as_ref().map(|c| c.id),
        thread_id,
        occurred_at,
        ingested_at: Utc::now(),
        direction,
        author,
        recipients,
        text,
        blobs: vec![],
        sensitivity: Level::default(),
        tombstoned: false,
        payload: Payload::Message {
            reply_to,
            forwarded_from: None,
            edited: message.edited,
        },
    };
    store.insert_item(&item)?;
    Ok(Ingested::Added(item.id))
}

/// The person a direct chat is with, from its key (`chat:<telegram id>`).
fn direct_peer(
    store: &Store,
    thread_key: &str,
    title: &str,
) -> anyhow::Result<Option<genatrix_model::PersonId>> {
    use genatrix_model::HandleKind;
    let Some(id) = thread_key.strip_prefix("chat:") else {
        return Ok(None);
    };
    if id.starts_with('-') {
        return Ok(None);
    }
    Ok(Some(store.person_for_handle(
        HandleKind::TelegramId,
        id,
        title,
    )?))
}

/// Put right what earlier builds got wrong about chats: the account
/// holder's own Telegram identity filed under a separate person, outgoing
/// direct messages without a recipient, and every chat message marked
/// directionless. Returns how many items changed.
pub fn repair_chats(store: &Store, telegram_user_ids: &[i64]) -> anyhow::Result<usize> {
    use genatrix_model::{Connector, HandleKind, ThreadKind};

    let Some(me) = store.self_person()?.map(|p| p.id) else {
        return Ok(0);
    };
    // The user's own Telegram ids belong to the user's person.
    for id in telegram_user_ids {
        let value = id.to_string();
        if let Some(handle) = store.find_handle(HandleKind::TelegramId, &value)?
            && handle.person_id != me
        {
            store.move_handle(HandleKind::TelegramId, &value, me)?;
            store.reassign_author(handle.person_id, me)?;
        }
    }

    let mut changed = 0;
    for mut item in store.all_items()? {
        if item.source.connector != Connector::Telegram || item.tombstoned {
            continue;
        }
        let Some(thread) = store.get_thread(item.thread_id)? else {
            continue;
        };
        let from_me = item.author == Some(me);
        let mut recipients = item.recipients.clone();
        if thread.kind == ThreadKind::DirectChat {
            let title = thread.title.clone().unwrap_or_default();
            recipients = if from_me {
                direct_peer(store, &thread.source.external_id, &title)?
                    .into_iter()
                    .collect()
            } else {
                vec![me]
            };
        }
        let direction =
            genatrix_model::item::direction_of(item.author, &recipients, |p| p == me, true);
        if direction != item.direction || recipients != item.recipients {
            item.direction = direction;
            item.recipients = recipients;
            store.rederive_item(&item)?;
            changed += 1;
        }
    }
    Ok(changed)
}

/// Who wrote a chat message: the user for an outgoing one, else the person
/// behind the sender's Telegram id, created on first sight with the
/// username as a second handle.
fn chat_author(
    store: &Store,
    me: Option<genatrix_model::PersonId>,
    message: &genatrix_connector::protocol::ChatMessage,
) -> anyhow::Result<Option<genatrix_model::PersonId>> {
    use genatrix_model::{Confidence, Handle, HandleId, HandleKind};

    if message.outgoing {
        return Ok(me);
    }
    let Some(sender) = &message.sender else {
        return Ok(None);
    };
    let id =
        store.person_for_handle(HandleKind::TelegramId, &sender.id.to_string(), &sender.name)?;
    if let Some(username) = sender.username.as_deref().filter(|u| !u.is_empty())
        && store
            .find_handle(HandleKind::TelegramUsername, username)?
            .is_none()
    {
        store.insert_handle(&Handle {
            id: HandleId::new(),
            person_id: id,
            kind: HandleKind::TelegramUsername,
            value: username.to_owned(),
            confidence: Confidence::Confirmed,
        })?;
    }
    Ok(Some(id))
}

/// The text, with each attachment named on its own line, so a photo with no
/// caption is not an empty item.
fn chat_text(message: &genatrix_connector::protocol::ChatMessage) -> String {
    let mut text = message.text.clone();
    for media in &message.media {
        let described = match (&media.name, &media.mime) {
            (Some(name), _) => format!("[{}: {name}]", media.kind),
            (None, Some(mime)) => format!("[{}: {mime}]", media.kind),
            (None, None) => format!("[{}]", media.kind),
        };
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&described);
    }
    text
}

/// What reading a raw record again did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reprocessed {
    /// Not mail, or the item it would produce is the one already there.
    Unchanged,
    /// The item was brought up to date in place; or created, when a crash
    /// had left the raw record without one.
    Updated(ItemId),
    /// The bytes no longer parse. The old item stays.
    Unreadable(String),
}

/// Derive an item from a raw record again, with today's normalization.
///
/// Design 01: the item is what the core makes of the raw record, and a
/// better way of making it replaces the item in place. A version, one item
/// superseding another, is reserved for a change upstream, which arrives as
/// a new raw record. Sensitivity and identity stay; the text, the people,
/// the conversation and the attachments are taken from the bytes again.
/// This is how a better parser reaches mail fetched before it existed,
/// without fetching anything twice.
pub fn reprocess(
    store: &Store,
    raw_files: &FileStore,
    blob_files: &FileStore,
    raw: &Raw,
) -> anyhow::Result<Reprocessed> {
    if raw.content_type != "message/rfc822" {
        return Ok(Reprocessed::Unchanged);
    }
    let bytes = raw_files.get(&raw.hash)?;
    let mail = match genatrix_connector_imap::normalize(&bytes) {
        Ok(mail) => mail,
        Err(e) => return Ok(Reprocessed::Unreadable(e.to_string())),
    };
    let current = store.current_item(&raw.source)?;
    let thread_key = genatrix_connector_imap::thread_key(&mail, None);
    let mut item = build(
        store,
        blob_files,
        raw.source.clone(),
        raw.id,
        &mail,
        &thread_key,
        None,
    )?;
    let Some(current) = current else {
        store.insert_item(&item)?;
        return Ok(Reprocessed::Updated(item.id));
    };
    if current.text == item.text && current.payload == item.payload && current.blobs == item.blobs {
        return Ok(Reprocessed::Unchanged);
    }
    item.id = current.id;
    store.rederive_item(&item)?;
    Ok(Reprocessed::Updated(item.id))
}

/// The item a normalized message becomes: its people, its conversation,
/// its attachments, its text. Idempotent in everything it touches besides
/// the item itself, which the caller inserts.
fn build(
    store: &Store,
    blob_files: &FileStore,
    source: Source,
    raw_id: genatrix_model::RawId,
    mail: &genatrix_connector_imap::Mail,
    thread_key: &str,
    supersedes: Option<ItemId>,
) -> anyhow::Result<Item> {
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
            source.account.clone(),
            thread_key.to_owned(),
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

    Ok(Item {
        id: ItemId::new(),
        source,
        raw_id,
        supersedes,
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
    })
}

#[cfg(test)]
mod tests {
    use genatrix_keys::{DbKey, MasterKey};
    use genatrix_store::ItemQuery;

    use super::*;

    const RAW: &[u8] = b"From: Ann <ann@example.com>\r\n\
To: me@example.com\r\n\
Subject: lunch\r\n\
Date: Mon, 1 Sep 2026 10:00:00 +0800\r\n\
Message-ID: <a1@example.com>\r\n\
Content-Type: text/plain\r\n\
\r\n\
Friday?\r\n";

    fn stores() -> (tempfile::TempDir, Store, FileStore, FileStore) {
        let dir = tempfile::tempdir().unwrap();
        let master = MasterKey::from_bytes([9; 32]);
        let store = Store::open_in_memory(&DbKey::from_bytes([1; 32])).unwrap();
        let raw_files = FileStore::open(dir.path().join("raw"), master.clone()).unwrap();
        let blob_files = FileStore::open(dir.path().join("blobs"), master).unwrap();
        (dir, store, raw_files, blob_files)
    }

    fn incoming(text: &str) -> Incoming {
        let mut mail = genatrix_connector_imap::normalize(RAW).unwrap();
        // As an older normalization might have left it.
        mail.text = text.to_owned();
        Incoming {
            account: "me@example.com".into(),
            external_id: "mid:a1@example.com".into(),
            thread_key: "mid:a1@example.com".into(),
            raw: RAW.to_vec(),
            mail,
        }
    }

    #[test]
    fn reprocessing_a_clean_item_changes_nothing() {
        let (_dir, store, raw_files, blob_files) = stores();
        let clean = incoming("Friday?");
        assert!(matches!(
            mail(&store, &raw_files, &blob_files, &clean).unwrap(),
            Ingested::Added(_)
        ));
        let raw = &store.all_raw().unwrap()[0];
        assert_eq!(
            reprocess(&store, &raw_files, &blob_files, raw).unwrap(),
            Reprocessed::Unchanged
        );
        assert_eq!(store.all_items().unwrap().len(), 1);
    }

    #[test]
    fn a_better_parser_brings_the_item_up_to_date_in_place() {
        let (_dir, store, raw_files, blob_files) = stores();
        let dirty = incoming(":root { color-scheme: light } Friday?");
        let Ingested::Added(id) = mail(&store, &raw_files, &blob_files, &dirty).unwrap() else {
            panic!("first sight");
        };
        store
            .set_item_sensitivity(id, Level::Secret)
            .expect("a judgement, to see that it survives");
        let raw = &store.all_raw().unwrap()[0];
        assert_eq!(
            reprocess(&store, &raw_files, &blob_files, raw).unwrap(),
            Reprocessed::Updated(id),
            "the same item, brought up to date"
        );

        let current = store
            .query_items(&ItemQuery {
                limit: 10,
                version: genatrix_store::ItemVersion::Current,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].id, id);
        assert_eq!(current[0].text, "Friday?");
        assert_eq!(current[0].sensitivity, Level::Secret, "the judgement stays");
        assert_eq!(store.all_items().unwrap().len(), 1, "no version was made");
        assert_eq!(store.all_raw().unwrap().len(), 1, "raw is never touched");
        assert_eq!(
            store.search_items("Friday", 10).unwrap().len(),
            1,
            "the search index followed the text"
        );

        assert_eq!(
            reprocess(&store, &raw_files, &blob_files, raw).unwrap(),
            Reprocessed::Unchanged,
            "and doing it again is a no-op"
        );
    }

    #[test]
    fn a_raw_record_without_an_item_gets_one() {
        let (_dir, store, raw_files, blob_files) = stores();
        // As a crash between storing the raw record and its item leaves it.
        let clean = incoming("Friday?");
        let source = Source::new(Connector::Imap, "me@example.com", "mid:a1@example.com");
        let raw = Raw::describe(source, "message/rfc822", &clean.raw);
        assert!(store.insert_raw(&raw).unwrap());
        raw_files.put(&clean.raw).unwrap();
        assert!(matches!(
            reprocess(&store, &raw_files, &blob_files, &raw).unwrap(),
            Reprocessed::Updated(_)
        ));
        assert_eq!(store.all_items().unwrap().len(), 1);
    }
}
