//! Synthetic items, so the rest of the system has something to work on
//! before a connector exists.
//!
//! These are invented, but not arbitrary: the mix of languages, senders and
//! sensitivities is the one the local-model spike measured against, so what
//! the pipeline does with them can be compared to a number that already
//! exists. They go in through the same path a connector will use, which is
//! most of the point.

use chrono::{DateTime, Duration, FixedOffset, Utc};
use genatrix_model::{
    Connector, Direction, HandleKind, Item, ItemId, Payload, Raw, Source, Thread, ThreadId,
    ThreadKind,
};
use genatrix_store::Store;

struct Sample {
    connector: Connector,
    account: &'static str,
    external_id: &'static str,
    /// Who wrote it: an address for mail, a numeric id for Telegram.
    from: &'static str,
    from_name: &'static str,
    thread: &'static str,
    thread_kind: ThreadKind,
    thread_title: &'static str,
    subject: &'static str,
    /// Headers, as a connector would hand them over.
    headers: &'static [(&'static str, &'static str)],
    text: &'static str,
    /// Hours before now.
    hours_ago: i64,
}

const SAMPLES: &[Sample] = &[
    Sample {
        connector: Connector::Imap,
        account: "me@example.com",
        external_id: "gm:1001",
        from: "maria@contoso.example",
        from_name: "Maria Alvarez",
        thread: "gm:t100",
        thread_kind: ThreadKind::MailThread,
        thread_title: "Revised proposal",
        subject: "Re: revised proposal",
        headers: &[],
        text: "Hi, following up on our call yesterday. The team is broadly happy with the scope. Two things remain open: we would like to shift milestone two by two weeks, and the support terms in section 5 mention 24/7 coverage when we only need business hours. If you can send an updated version by Friday we can take it to the steering committee on Monday.",
        hours_ago: 3,
    },
    Sample {
        connector: Connector::Imap,
        account: "me@example.com",
        external_id: "gm:1002",
        from: "digest@newsletter.example",
        from_name: "The Weekly Digest",
        thread: "gm:t101",
        thread_kind: ThreadKind::MailThread,
        thread_title: "Weekly digest",
        subject: "Ten stories you might like",
        headers: &[
            ("List-Unsubscribe", "<https://newsletter.example/u/abc>"),
            ("Precedence", "bulk"),
        ],
        text: "This week: a new Rust release, a deep dive into SQLite internals, and why your laptop fan is loud. Unsubscribe at any time.",
        hours_ago: 9,
    },
    Sample {
        connector: Connector::Imap,
        account: "me@example.com",
        external_id: "gm:1003",
        from: "no-reply@chase.com",
        from_name: "Account Services",
        thread: "gm:t102",
        thread_kind: ThreadKind::MailThread,
        thread_title: "Statement ready",
        subject: "Your statement is ready",
        headers: &[("List-Unsubscribe", "<https://chase.com/u>")],
        text: "Your monthly statement for the account ending 9032 is ready to view. Balance carried forward: 4,210.55.",
        hours_ago: 20,
    },
    Sample {
        connector: Connector::Imap,
        account: "me@example.com",
        external_id: "gm:1004",
        from: "security@accounts.example",
        from_name: "Accounts",
        thread: "gm:t103",
        thread_kind: ThreadKind::MailThread,
        thread_title: "Sign-in",
        subject: "New sign-in",
        headers: &[],
        text: "Your verification code is 482913. It expires in 10 minutes. Do not share this code with anyone.",
        hours_ago: 26,
    },
    Sample {
        connector: Connector::Telegram,
        account: "1",
        external_id: "7:2001",
        from: "2",
        from_name: "Alice Chen",
        thread: "chat:7",
        thread_kind: ThreadKind::DirectChat,
        thread_title: "Alice Chen",
        subject: "",
        headers: &[],
        text: "周四能不能改到下午三点？上午我要去接孩子。",
        hours_ago: 5,
    },
    Sample {
        connector: Connector::Telegram,
        account: "1",
        external_id: "7:2002",
        from: "1",
        from_name: "Neo",
        thread: "chat:7",
        thread_kind: ThreadKind::DirectChat,
        thread_title: "Alice Chen",
        subject: "",
        headers: &[],
        text: "可以，那就周四下午三点。我把报价周五之前发给你。",
        hours_ago: 4,
    },
    Sample {
        connector: Connector::Telegram,
        account: "1",
        external_id: "9:2003",
        from: "3",
        from_name: "Rust 中文频道",
        thread: "chat:9",
        thread_kind: ThreadKind::Channel,
        thread_title: "Rust 中文频道",
        subject: "",
        headers: &[],
        text: "本周四晚八点直播分享 Rust 异步编程实践，欢迎大家准时收看。",
        hours_ago: 11,
    },
    Sample {
        connector: Connector::Telegram,
        account: "1",
        external_id: "11:2004",
        from: "4",
        from_name: "Dr Lin's office",
        thread: "chat:11",
        thread_kind: ThreadKind::DirectChat,
        thread_title: "Dr Lin's office",
        subject: "",
        headers: &[],
        text: "Your lab results are ready. HbA1c came back at 7.1%. Please book a follow-up this month to talk through it.",
        hours_ago: 30,
    },
];

/// How many items the seed puts in.
#[must_use]
pub const fn count() -> usize {
    SAMPLES.len()
}

/// Put the samples in, skipping any that are already there.
///
/// Returns how many were new. Running it twice adds nothing, because it goes
/// through the same idempotence a connector relies on.
pub fn run(store: &Store) -> anyhow::Result<usize> {
    let me = store.person_for_handle(HandleKind::Email, "me@example.com", "Neo")?;
    if store.self_person()?.is_none() {
        store.set_self(me)?;
    }
    let me_telegram = store.person_for_handle(HandleKind::TelegramId, "1", "Neo")?;

    let now = Utc::now();
    let mut added = 0;
    for sample in SAMPLES {
        let source = Source::new(sample.connector, sample.account, sample.external_id);
        let raw = Raw::describe(
            source.clone(),
            content_type(sample.connector),
            sample.text.as_bytes(),
        );
        if !store.insert_raw(&raw)? {
            continue;
        }

        let thread_source = Source::new(sample.connector, sample.account, sample.thread);
        let thread_id = store.upsert_thread(&Thread {
            id: ThreadId::new(),
            kind: sample.thread_kind,
            source: thread_source,
            title: Some(sample.thread_title.to_owned()),
            members: vec![],
            first_at: None,
            last_at: None,
        })?;

        let author = match sample.connector {
            Connector::Imap => {
                store.person_for_handle(HandleKind::Email, sample.from, sample.from_name)?
            }
            Connector::Telegram => {
                store.person_for_handle(HandleKind::TelegramId, sample.from, sample.from_name)?
            }
        };
        let self_person = match sample.connector {
            Connector::Imap => me,
            Connector::Telegram => me_telegram,
        };
        let outbound = author == self_person;
        let recipients = if outbound { vec![] } else { vec![self_person] };

        let occurred = at(now - Duration::hours(sample.hours_ago));
        let payload = match sample.connector {
            Connector::Imap => Payload::Mail {
                subject: sample.subject.to_owned(),
                from: sample.from.to_owned(),
                to: vec![sample.account.to_owned()],
                cc: vec![],
                message_id: Some(format!("<{}@example>", sample.external_id)),
                in_reply_to: None,
                references: vec![],
                labels: vec!["INBOX".to_owned()],
            },
            Connector::Telegram => Payload::Message {
                reply_to: None,
                forwarded_from: None,
                edited: false,
            },
        };

        store.insert_item(&Item {
            id: ItemId::new(),
            source,
            raw_id: raw.id,
            supersedes: None,
            thread_id,
            occurred_at: occurred,
            ingested_at: Utc::now(),
            direction: if outbound {
                Direction::Outbound
            } else {
                Direction::Inbound
            },
            author: Some(author),
            recipients,
            text: sample.text.to_owned(),
            blobs: vec![],
            sensitivity: genatrix_model::Level::default(),
            tombstoned: false,
            payload,
        })?;
        added += 1;
    }
    Ok(added)
}

/// Headers for an item, as the classifier needs them. A connector will get
/// these from the source; the seed keeps them beside the sample.
#[must_use]
pub fn headers_for(external_id: &str) -> Vec<(String, String)> {
    SAMPLES
        .iter()
        .find(|s| s.external_id == external_id)
        .map(|s| {
            s.headers
                .iter()
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// The sender's domain for an item, when it has one.
#[must_use]
pub fn sender_domain_for(external_id: &str) -> Option<String> {
    SAMPLES
        .iter()
        .find(|s| s.external_id == external_id)
        .and_then(|s| s.from.split_once('@'))
        .map(|(_, domain)| domain.to_lowercase())
}

const fn content_type(connector: Connector) -> &'static str {
    match connector {
        Connector::Imap => "message/rfc822",
        Connector::Telegram => "application/x-telegram-message+json",
    }
}

fn at(t: DateTime<Utc>) -> DateTime<FixedOffset> {
    t.with_timezone(&FixedOffset::east_opt(8 * 3600).expect("a valid offset"))
}
