//! Where each item belongs (design 01, "分类"): personal, transactional,
//! newsletter or promotion. Rules only, no model: the signs of bulk mail are
//! in its headers and its sender, and whether the user has ever written to
//! the sender says the rest. The user's word about a sender beats them all.
//!
//! Hiding is all a category does. Nothing here deletes anything.

use std::collections::HashMap;

use genatrix_model::{
    Annotation, AnnotationKind, Category, Direction, Item, Payload, PersonId, Producer,
};
use genatrix_store::Store;

/// Items judged in one pass. Rules are cheap; the first pass over an old
/// mailbox takes a few of these.
const PER_PASS: u32 = 5000;
/// The rule set's version, recorded on every judgement.
const VERSION: &str = "1";

/// Words that make bulk mail a receipt, a bill or a notice about the user's
/// own affairs rather than an advertisement.
const TRANSACTIONAL: &[&str] = &[
    "receipt",
    "invoice",
    "order confirmation",
    "your order",
    "order #",
    "order number",
    "payment",
    "statement",
    "your bill",
    "billing",
    "verification code",
    "security code",
    "one-time",
    "password reset",
    "reset your password",
    "sign-in",
    "new login",
    "your account",
    "shipped",
    "shipping confirmation",
    "delivered",
    "tracking",
    "booking",
    "reservation",
    "itinerary",
    "e-ticket",
    "renewal",
    "refund",
    "发票",
    "收据",
    "订单",
    "账单",
    "付款",
    "支付",
    "验证码",
    "发货",
    "快递",
    "物流",
    "预订",
    "行程",
    "退款",
    "对账单",
];

/// Words that make bulk mail an advertisement.
const PROMOTION: &[&str] = &[
    "% off",
    "sale",
    "discount",
    "deal",
    "offer",
    "coupon",
    "promo",
    "free shipping",
    "limited time",
    "last chance",
    "clearance",
    "black friday",
    "cyber monday",
    "shop now",
    "buy now",
    "new arrivals",
    "exclusive",
    "促销",
    "优惠",
    "折扣",
    "特价",
    "限时",
    "打折",
    "满减",
    "秒杀",
    "抢购",
    "新品",
];

/// Bulk mail services, as they show in `Return-Path`.
const SENDERS: &[&str] = &[
    "sendgrid",
    "mailchimp",
    "mcsv.net",
    "mcdlv.net",
    "amazonses",
    "mailgun",
    "sparkpost",
    "sendinblue",
    "brevo",
    "klaviyo",
    "hubspot",
    "constantcontact",
    "mailerlite",
    "cmail",
    "createsend",
    "mandrillapp",
    "exacttarget",
    "salesforce",
    "marketo",
];

/// Sender addresses no person writes from.
const MACHINES: &[&str] = &[
    "noreply",
    "no-reply",
    "donotreply",
    "do-not-reply",
    "newsletter",
    "news@",
    "marketing",
    "promo",
    "offers@",
    "deals@",
    "notifications@",
    "notification@",
    "mailer",
    "updates@",
];

/// What one pass did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Items judged.
    pub judged: usize,
    /// Of them, newsletters or promotions.
    pub quiet: usize,
}

/// Judge the items nobody has judged yet.
pub fn run(store: &Store) -> genatrix_store::Result<Report> {
    let mut report = Report::default();
    let mut written: HashMap<PersonId, bool> = HashMap::new();
    let mut said: HashMap<PersonId, Option<Category>> = HashMap::new();
    for item in store.items_without_category(PER_PASS)? {
        let (category, producer, reason) = judge(store, &item, &mut written, &mut said)?;
        store.insert_annotation(&Annotation::new(
            item.id,
            producer,
            AnnotationKind::Category {
                category,
                reason: reason.to_owned(),
            },
        ))?;
        store.set_item_category(item.id, category)?;
        report.judged += 1;
        report.quiet += usize::from(category.is_quiet());
    }
    Ok(report)
}

/// The user's word about a sender: every item they wrote takes this
/// category, and so does whatever they send later.
pub fn set_sender(
    store: &Store,
    person: PersonId,
    category: Category,
) -> genatrix_store::Result<usize> {
    store.set_person_category(person, Some(category))?;
    let items = store.items_by(person)?;
    for id in &items {
        store.insert_annotation(&Annotation::new(
            *id,
            Producer::User,
            AnnotationKind::Category {
                category,
                reason: "you said so".into(),
            },
        ))?;
        store.set_item_category(*id, category)?;
    }
    Ok(items.len())
}

fn rule() -> Producer {
    Producer::Rule {
        rule: "categories".into(),
        version: VERSION.into(),
    }
}

fn judge(
    store: &Store,
    item: &Item,
    written: &mut HashMap<PersonId, bool>,
    said: &mut HashMap<PersonId, Option<Category>>,
) -> genatrix_store::Result<(Category, Producer, &'static str)> {
    if let Some(author) = item.author {
        let word = if let Some(w) = said.get(&author) {
            *w
        } else {
            let w = store.person_category(author)?;
            said.insert(author, w);
            w
        };
        if let Some(category) = word {
            return Ok((category, Producer::User, "you said so about this sender"));
        }
    }
    let Payload::Mail {
        subject,
        from,
        headers,
        ..
    } = &item.payload
    else {
        return Ok((Category::Personal, rule(), "not mail"));
    };
    if matches!(item.direction, Direction::Outbound | Direction::Internal) {
        return Ok((Category::Personal, rule(), "you wrote it"));
    }
    let header = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.to_lowercase())
    };
    let unsubscribe = header("list-unsubscribe").is_some();
    let list = header("list-id").is_some();
    let precedence =
        header("precedence").is_some_and(|v| matches!(v.trim(), "bulk" | "list" | "junk"));
    let automatic = header("auto-submitted").is_some_and(|v| v.trim() != "no")
        || header("x-autoreply").is_some();
    let service = header("return-path").is_some_and(|v| SENDERS.iter().any(|s| v.contains(s)));
    let from = from.to_lowercase();
    let machine = MACHINES.iter().any(|m| from.contains(m));

    let bulk = unsubscribe || list || precedence || automatic || service || machine;
    if !bulk {
        return Ok((Category::Personal, rule(), "between people"));
    }
    let known = match item.author {
        Some(author) => {
            if let Some(k) = written.get(&author) {
                *k
            } else {
                let k = store.has_written_to(author)?;
                written.insert(author, k);
                k
            }
        }
        None => false,
    };
    if known && !unsubscribe && !list {
        return Ok((Category::Personal, rule(), "someone you write to"));
    }
    let words: String = format!(
        "{subject} {}",
        item.text.chars().take(600).collect::<String>()
    )
    .to_lowercase();
    if TRANSACTIONAL.iter().any(|w| words.contains(w)) {
        return Ok((
            Category::Transactional,
            rule(),
            "bulk, about your own affairs",
        ));
    }
    if PROMOTION.iter().any(|w| words.contains(w)) {
        return Ok((Category::Promotion, rule(), "bulk, selling something"));
    }
    if unsubscribe || list {
        return Ok((Category::Newsletter, rule(), "a list you are on"));
    }
    Ok((Category::Transactional, rule(), "an automatic notice"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use genatrix_model::{
        Connector, HandleKind, ItemId, Level, Raw, Source, Thread, ThreadId, ThreadKind,
    };

    fn store() -> Store {
        Store::open_in_memory(&genatrix_keys::DbKey::from_bytes([3; 32])).unwrap()
    }

    fn mail(
        store: &Store,
        from: &str,
        subject: &str,
        headers: &[(&str, &str)],
        direction: Direction,
    ) -> Item {
        let ext = ulid::Ulid::new().to_string();
        let source = Source::new(Connector::Imap, "me@example.com", &ext);
        let raw = Raw::describe(source.clone(), "message/rfc822", ext.as_bytes());
        store.insert_raw(&raw).unwrap();
        let author = store
            .person_for_handle(HandleKind::Email, from, from)
            .unwrap();
        let thread = store
            .upsert_thread(&Thread {
                id: ThreadId::new(),
                kind: ThreadKind::MailThread,
                source: Source::new(Connector::Imap, "me@example.com", format!("t{ext}")),
                title: None,
                members: vec![],
                first_at: None,
                last_at: None,
            })
            .unwrap();
        let item = Item {
            id: ItemId::new(),
            source,
            raw_id: raw.id,
            supersedes: None,
            thread_id: thread,
            occurred_at: Utc::now().fixed_offset(),
            ingested_at: Utc::now(),
            direction,
            author: Some(author),
            recipients: vec![],
            text: "body".into(),
            blobs: vec![],
            sensitivity: Level::Personal,
            tombstoned: false,
            payload: Payload::Mail {
                subject: subject.into(),
                from: from.into(),
                to: vec![],
                cc: vec![],
                message_id: None,
                in_reply_to: None,
                references: vec![],
                labels: vec![],
                headers: headers
                    .iter()
                    .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                    .collect(),
            },
        };
        store.insert_item(&item).unwrap();
        item
    }

    #[test]
    fn bulk_mail_is_sorted_and_people_are_not() {
        let s = store();
        let unsub = [("list-unsubscribe", "<mailto:u@x.com>")];
        let friend = mail(
            &s,
            "ann@example.com",
            "lunch on friday?",
            &[],
            Direction::Inbound,
        );
        let ad = mail(
            &s,
            "shop@brand.com",
            "50% off everything this weekend",
            &unsub,
            Direction::Inbound,
        );
        let receipt = mail(
            &s,
            "orders@shop.com",
            "Your order receipt",
            &unsub,
            Direction::Inbound,
        );
        let news = mail(
            &s,
            "editor@weekly.com",
            "This week in Rust",
            &unsub,
            Direction::Inbound,
        );
        let notice = mail(
            &s,
            "noreply@bank.co.nz",
            "We noticed something",
            &[],
            Direction::Inbound,
        );
        let sent = mail(&s, "me@example.com", "deal?", &unsub, Direction::Outbound);
        let report = run(&s).unwrap();
        assert_eq!(report.judged, 6);
        assert_eq!(report.quiet, 2);
        let cat = |i: &Item| s.item_category(i.id).unwrap();
        assert_eq!(cat(&friend), Category::Personal);
        assert_eq!(cat(&ad), Category::Promotion);
        assert_eq!(cat(&receipt), Category::Transactional);
        assert_eq!(cat(&news), Category::Newsletter);
        assert_eq!(cat(&notice), Category::Transactional);
        assert_eq!(cat(&sent), Category::Personal);
        // Judged once.
        assert_eq!(run(&s).unwrap().judged, 0);
    }

    #[test]
    fn the_users_word_about_a_sender_wins_now_and_later() {
        let s = store();
        let unsub = [("list-unsubscribe", "<mailto:u@x.com>")];
        let first = mail(
            &s,
            "editor@weekly.com",
            "This week in Rust",
            &unsub,
            Direction::Inbound,
        );
        run(&s).unwrap();
        assert_eq!(s.item_category(first.id).unwrap(), Category::Newsletter);
        let author = first.author.unwrap();
        assert_eq!(set_sender(&s, author, Category::Personal).unwrap(), 1);
        assert_eq!(s.item_category(first.id).unwrap(), Category::Personal);
        let later = mail(
            &s,
            "editor@weekly.com",
            "Next week in Rust",
            &unsub,
            Direction::Inbound,
        );
        run(&s).unwrap();
        assert_eq!(s.item_category(later.id).unwrap(), Category::Personal);
    }
}
