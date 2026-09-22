//! Reading Telegram into the shapes the core stores.
//!
//! Design: `docs/design/05-connectors.md`, "拉取" and "归一". Dialogs first,
//! which gives every conversation; then each conversation's history from
//! the newest message back, one at a time so that this week's chats arrive
//! first; and the live update stream alongside, sequence-numbered by the
//! library so gaps are filled. Every message's raw structure is serialized
//! and travels with it.
//!
//! The library keeps one connection per datacenter behind its pool, and
//! Telegram's limits are the pool's to respect: its default retry policy
//! sleeps through short floods. This code asks for history in pages of a
//! hundred, which is what Telegram hands out anyway.

use chrono::{DateTime, Utc};
use genatrix_connector::protocol::{ChatMedia, ChatMessage, ChatSender};
use grammers_client::Client;
use grammers_client::media::Media;
use grammers_client::message::Message;
use grammers_client::peer::{Dialog, Peer};
use grammers_tl_types::Serializable;

/// One conversation, as far as the connector needs to know it.
#[derive(Clone, Debug)]
pub struct Conversation {
    /// Stable identifier across the account: the dialog id Telegram's Bot
    /// API would use, negative for groups and channels.
    pub id: i64,
    /// `direct`, `group` or `channel`.
    pub kind: &'static str,
    /// Its name.
    pub title: String,
    /// How to ask Telegram about it.
    pub peer: grammers_session::types::PeerRef,
    /// Whether the user has archived it. Telegram keeps archived chats in
    /// folder 1; an archived chat is one the user has chosen not to look
    /// at, and the connector leaves it alone (design 05).
    pub archived: bool,
}

/// The conversation's key within the account.
#[must_use]
pub fn thread_key(id: i64) -> String {
    format!("chat:{id}")
}

/// A message's key within the account.
#[must_use]
pub fn external_id(chat: i64, message: i32) -> String {
    format!("chat:{chat}/msg:{message}")
}

/// Every dialog the account has.
pub async fn conversations(
    client: &Client,
) -> Result<Vec<Conversation>, grammers_client::InvocationError> {
    let mut out = Vec::new();
    let mut dialogs = client.iter_dialogs();
    while let Some(dialog) = dialogs.next().await? {
        out.push(conversation_of(&dialog));
    }
    Ok(out)
}

fn conversation_of(dialog: &Dialog) -> Conversation {
    let peer = dialog.peer();
    let kind = match peer {
        Peer::User(_) => "direct",
        Peer::Group(_) => "group",
        Peer::Channel(_) => "channel",
    };
    let archived = match &dialog.raw {
        grammers_tl_types::enums::Dialog::Dialog(d) => d.folder_id == Some(1),
        grammers_tl_types::enums::Dialog::Folder(_) => false,
    };
    Conversation {
        id: peer.id().bot_api_dialog_id_unchecked(),
        kind,
        title: peer.name().unwrap_or("").to_owned(),
        peer: dialog.peer_ref(),
        archived,
    }
}

/// One page of a conversation's history, newest first, older than
/// `before` (a message id) when given. Returns the messages and the oldest
/// id seen, or an empty page at the beginning of the conversation.
pub async fn history_page(
    client: &Client,
    conversation: &Conversation,
    before: Option<i32>,
    limit: usize,
) -> Result<(Vec<ChatMessage>, Option<i32>), grammers_client::InvocationError> {
    let mut iter = client.iter_messages(conversation.peer).limit(limit);
    if let Some(before) = before {
        iter = iter.offset_id(before);
    }
    let mut out = Vec::new();
    let mut oldest = None;
    while let Some(message) = iter.next().await? {
        oldest = Some(oldest.map_or(message.id(), |o: i32| o.min(message.id())));
        if let Some(chat) = to_wire(conversation, &message) {
            out.push(chat);
        }
    }
    Ok((out, oldest))
}

/// A message from the live stream, in a conversation this connector may
/// not have listed yet.
#[must_use]
pub fn from_update(message: &Message) -> Option<ChatMessage> {
    let peer = message.peer()?;
    let kind = match peer {
        Peer::User(_) => "direct",
        Peer::Group(_) => "group",
        Peer::Channel(_) => "channel",
    };
    let conversation = Conversation {
        id: peer.id().bot_api_dialog_id_unchecked(),
        kind,
        title: peer.name().unwrap_or("").to_owned(),
        peer: grammers_session::types::PeerRef {
            id: peer.id(),
            auth: grammers_session::types::PeerAuth::default(),
        },
        archived: false,
    };
    to_wire(&conversation, message)
}

/// The message as the core stores it. Service messages (someone joined,
/// the photo changed) are not correspondence and yield nothing.
fn to_wire(conversation: &Conversation, message: &Message) -> Option<ChatMessage> {
    if message.action().is_some() {
        return None;
    }
    let sender = message.sender().map(|peer| ChatSender {
        id: peer.id().bot_api_dialog_id_unchecked(),
        username: peer.username().map(str::to_owned),
        name: match peer {
            Peer::User(user) => user.full_name(),
            other => other.name().unwrap_or("").to_owned(),
        },
    });
    let date: DateTime<Utc> = message.date();
    let media = message.media().into_iter().map(describe_media).collect();
    Some(ChatMessage {
        external_id: external_id(conversation.id, message.id()),
        thread_key: thread_key(conversation.id),
        thread_kind: conversation.kind.to_owned(),
        thread_title: conversation.title.clone(),
        sender,
        outgoing: message.outgoing(),
        date: date.to_rfc3339(),
        text: message.text().to_owned(),
        reply_to: message
            .reply_to_message_id()
            .map(|id| external_id(conversation.id, id)),
        forwarded_from: message.forward_header().and_then(forwarded_name),
        edited: message.edit_date().is_some(),
        raw: message.raw.to_bytes(),
        media,
    })
}

fn forwarded_name(header: grammers_tl_types::enums::MessageFwdHeader) -> Option<String> {
    let grammers_tl_types::enums::MessageFwdHeader::Header(h) = header;
    h.from_name.or(h.post_author)
}

fn describe_media(media: Media) -> ChatMedia {
    match media {
        Media::Photo(p) => ChatMedia {
            kind: "photo".into(),
            name: None,
            mime: Some("image/jpeg".into()),
            size: p.size().map(|s| s as u64),
        },
        Media::Document(d) => ChatMedia {
            kind: "document".into(),
            name: d.name().map(str::to_owned),
            mime: d.mime_type().map(str::to_owned),
            size: d.size().map(|s| s as u64),
        },
        Media::Sticker(_) => ChatMedia {
            kind: "sticker".into(),
            name: None,
            mime: None,
            size: None,
        },
        Media::Contact(_) => ChatMedia {
            kind: "contact".into(),
            name: None,
            mime: None,
            size: None,
        },
        Media::Poll(_) => ChatMedia {
            kind: "poll".into(),
            name: None,
            mime: None,
            size: None,
        },
        Media::Geo(_) | Media::Venue(_) => ChatMedia {
            kind: "location".into(),
            name: None,
            mime: None,
            size: None,
        },
        _ => ChatMedia {
            kind: "other".into(),
            name: None,
            mime: None,
            size: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_carry_the_conversation_so_message_ids_never_collide() {
        assert_eq!(external_id(-1_001_234, 77), "chat:-1001234/msg:77");
        assert_eq!(external_id(42, 77), "chat:42/msg:77");
        assert_eq!(thread_key(42), "chat:42");
    }
}
