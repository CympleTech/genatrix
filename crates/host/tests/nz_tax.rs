//! The NZ tax agent (design 11, "第一个 agent") end to end in the sandbox:
//! a real component and a real encrypted space; the mail, the attachment's
//! text and the model's answers are stand-ins.

use std::sync::{Arc, LazyLock, Mutex};

use genatrix_host::manifest::Purpose;
use genatrix_host::wit::{
    BlobRef, Card, FieldValue, Filter, Item, Level as WitLevel, Message, Value,
};
use genatrix_host::{Doors, Invocation, Package, Run, Runner};
use genatrix_model::Level;
use genatrix_store::{Space, SpaceValue};

static RUNNER: LazyLock<Runner> = LazyLock::new(|| Runner::new().unwrap());

/// 2026-09-26, in the August-September GST period of the 2026-27 year.
const NOW_MS: i64 = 1_790_400_000_000;

const INVOICE: &str = "TAX INVOICE\nPower Co Ltd  GST 123-456-789\nInvoice 42  Date 3 Sep 2026\n\
                       Electricity August\nSubtotal 100.00\nGST 15.00\nTotal 115.00";

fn package() -> Package {
    let path = format!("{}/tests/fixtures/nz-tax.wasm", env!("CARGO_MANIFEST_DIR"));
    Package::read(std::fs::read(path).unwrap()).unwrap()
}

fn mail(id: &str, blob: &str) -> Item {
    Item {
        id: id.into(),
        connector: "imap".into(),
        kind: "mail".into(),
        direction: "inbound".into(),
        occurred_ms: NOW_MS,
        author: Some("Power Co".into()),
        title: Some(format!("Your invoice {id}")),
        text: "Your invoice is attached.".into(),
        blobs: vec![BlobRef {
            id: blob.into(),
            name: Some("invoice.pdf".into()),
            mime: "application/pdf".into(),
            size: 1000,
        }],
        level: WitLevel::Secret,
    }
}

#[derive(Default)]
struct Seen {
    proposals: Vec<(String, Card, String)>,
    model_levels: Vec<Level>,
}

struct Fake {
    items: Vec<Item>,
    /// What the model says when it reads a document: the extraction.
    extraction: String,
    space: Arc<Space>,
    seen: Arc<Mutex<Seen>>,
}

fn to_space(v: Value) -> SpaceValue {
    match v {
        Value::Null => SpaceValue::Null,
        Value::Integer(i) => SpaceValue::Integer(i),
        Value::Real(f) => SpaceValue::Real(f),
        Value::Text(s) => SpaceValue::Text(s),
        Value::Bytes(b) => SpaceValue::Bytes(b),
    }
}

fn from_space(v: SpaceValue) -> Value {
    match v {
        SpaceValue::Null => Value::Null,
        SpaceValue::Integer(i) => Value::Integer(i),
        SpaceValue::Real(f) => Value::Real(f),
        SpaceValue::Text(s) => Value::Text(s),
        SpaceValue::Bytes(b) => Value::Bytes(b),
    }
}

impl Doors for Fake {
    fn query(&mut self, _filter: &Filter) -> Vec<Item> {
        self.items.clone()
    }
    fn get(&mut self, id: &str) -> Option<Item> {
        self.items.iter().find(|i| i.id == id).cloned()
    }
    fn blob_text(&mut self, _id: &str) -> Result<(String, Level), String> {
        Ok((INVOICE.into(), Level::Secret))
    }
    fn call_model(
        &mut self,
        purpose: Purpose,
        _m: Vec<Message>,
        level: Level,
    ) -> Result<String, String> {
        self.seen.lock().unwrap().model_levels.push(level);
        Ok(match purpose {
            Purpose::Extract => self.extraction.clone(),
            Purpose::Classify => "<think></think>6200-utilities".into(),
            _ => String::new(),
        })
    }
    fn execute(&mut self, sql: &str, params: Vec<Value>) -> Result<u64, String> {
        let params: Vec<SpaceValue> = params.into_iter().map(to_space).collect();
        self.space.execute(sql, &params)
    }
    fn query_space(&mut self, sql: &str, params: Vec<Value>) -> Result<Vec<Vec<Value>>, String> {
        let params: Vec<SpaceValue> = params.into_iter().map(to_space).collect();
        self.space.query(sql, &params).map(|rows| {
            rows.into_iter()
                .map(|r| r.into_iter().map(from_space).collect())
                .collect()
        })
    }
    fn propose(
        &mut self,
        kind: &str,
        card: Card,
        payload: String,
        _level: Level,
    ) -> Result<String, String> {
        let mut seen = self.seen.lock().unwrap();
        seen.proposals.push((kind.into(), card, payload));
        Ok(format!("action-{}", seen.proposals.len()))
    }
}

const GOOD: &str = r#"{"is_invoice": true, "supplier_name": "Power Co Ltd", "supplier_gst_number": "123-456-789",
    "invoice_number": "42", "invoice_date": "2026-09-03", "currency": "NZD",
    "subtotal_cents": 10000, "gst_cents": 1500, "total_cents": 11500, "confidence": 0.95}"#;

struct Books {
    space: Arc<Space>,
    seen: Arc<Mutex<Seen>>,
    items: Vec<Item>,
}

impl Books {
    fn new(items: Vec<Item>) -> Self {
        Self {
            space: Arc::new(
                Space::open_in_memory(&genatrix_keys::DbKey::from_bytes([5; 32]), 50).unwrap(),
            ),
            seen: Arc::new(Mutex::new(Seen::default())),
            items,
        }
    }

    fn run(
        &self,
        invocation: Invocation,
        extraction: &str,
    ) -> Result<Option<String>, genatrix_host::RunError> {
        let doors = Box::new(Fake {
            items: self.items.clone(),
            extraction: extraction.into(),
            space: Arc::clone(&self.space),
            seen: Arc::clone(&self.seen),
        });
        let run = Run {
            invocation,
            now_ms: NOW_MS,
            seed: 1,
            space_level: Level::Public,
        };
        RUNNER.run(&package(), run, doors).result
    }

    fn ask(&self, text: &str) -> String {
        self.run(Invocation::Message(text.into()), GOOD)
            .unwrap()
            .unwrap()
    }
}

#[test]
fn an_invoice_becomes_a_proposal_then_an_entry_then_a_return() {
    let books = Books::new(vec![mail("i1", "b1")]);
    books
        .run(Invocation::Items(vec!["i1".into()]), GOOD)
        .unwrap();

    let (kind, card, payload) = books.seen.lock().unwrap().proposals[0].clone();
    assert_eq!(kind, "record_entry");
    assert_eq!(card.title, "Record Power Co Ltd 42");
    assert_eq!(card.evidence, vec!["i1".to_owned()]);
    let total = card.fields.iter().find(|f| f.label == "Total").unwrap();
    assert!(matches!(&total.value, FieldValue::Money(m) if m.cents == 11_500));
    let account = card.fields.iter().find(|f| f.label == "Account").unwrap();
    assert!(matches!(&account.value, FieldValue::Text(t) if t.starts_with("6200-utilities")));
    // What it read was secret, so every model call went out as secret.
    assert!(
        books
            .seen
            .lock()
            .unwrap()
            .model_levels
            .iter()
            .all(|l| *l == Level::Secret)
    );

    // Read once: the same mail again proposes nothing more.
    books
        .run(Invocation::Items(vec!["i1".into()]), GOOD)
        .unwrap();
    assert_eq!(books.seen.lock().unwrap().proposals.len(), 1);

    // Before approval the books are empty; after it, the entry counts.
    assert!(books.ask("gst").contains("0 entries"));
    books
        .run(
            Invocation::Apply {
                kind: "record_entry".into(),
                payload,
            },
            GOOD,
        )
        .unwrap();
    let gst = books.ask("gst");
    assert!(
        gst.contains("GST period 2026-08-01 to 2026-09-30 (1 entries), due 2026-10-28"),
        "{gst}"
    );
    assert!(gst.contains("Box 12 GST credit: NZD 15.00"), "{gst}");
    assert!(gst.contains("refund: NZD 15.00"), "{gst}");
    assert!(gst.contains("not yet checked against IRD"), "{gst}");
    let zh = books.ask("本期 GST");
    assert!(zh.contains("第 12 栏 可抵扣的 GST：NZD 15.00"), "{zh}");
    let income = books.ask("income tax");
    assert!(
        income.contains("Expenses excl. GST: NZD 100.00"),
        "{income}"
    );
}

#[test]
fn the_ledger_cannot_be_rewritten() {
    let books = Books::new(vec![mail("i1", "b1")]);
    books
        .run(Invocation::Items(vec!["i1".into()]), GOOD)
        .unwrap();
    let payload = books.seen.lock().unwrap().proposals[0].2.clone();
    books
        .run(
            Invocation::Apply {
                kind: "record_entry".into(),
                payload,
            },
            GOOD,
        )
        .unwrap();
    assert!(
        books
            .space
            .execute("UPDATE entry SET date = '2020-01-01'", &[])
            .is_err()
    );
    assert!(books.space.execute("DELETE FROM entry", &[]).is_err());
}

#[test]
fn figures_that_do_not_add_up_go_to_review_not_to_the_books() {
    // Subtotal and GST do not make the total: a misreading.
    let wrong = GOOD.replace("\"gst_cents\": 1500", "\"gst_cents\": 2000");
    let books = Books::new(vec![mail("i2", "b2")]);
    books
        .run(Invocation::Items(vec!["i2".into()]), &wrong)
        .unwrap();
    assert!(books.seen.lock().unwrap().proposals.is_empty());
    let review = books.ask("待审");
    assert!(review.contains("Your invoice i2"), "{review}");
}

#[test]
fn not_an_invoice_is_set_aside() {
    let books = Books::new(vec![mail("i3", "b3")]);
    books
        .run(
            Invocation::Items(vec!["i3".into()]),
            r#"{"is_invoice": false}"#,
        )
        .unwrap();
    assert!(books.seen.lock().unwrap().proposals.is_empty());
    assert!(books.ask("status").contains("0 entries recorded"));
}

#[test]
fn the_day_after_a_period_ends_its_return_is_proposed_once() {
    let books = Books::new(vec![mail("i1", "b1")]);
    books
        .run(Invocation::Items(vec!["i1".into()]), GOOD)
        .unwrap();
    let payload = books.seen.lock().unwrap().proposals[0].2.clone();
    books
        .run(
            Invocation::Apply {
                kind: "record_entry".into(),
                payload,
            },
            GOOD,
        )
        .unwrap();
    let on = |ms: i64| {
        let doors = Box::new(Fake {
            items: Vec::new(),
            extraction: String::new(),
            space: Arc::clone(&books.space),
            seen: Arc::clone(&books.seen),
        });
        let run = Run {
            invocation: Invocation::Schedule("gst_period".into()),
            now_ms: ms,
            seed: 1,
            space_level: Level::Public,
        };
        RUNNER.run(&package(), run, doors).result.unwrap();
    };
    // Mid-period: nothing.
    on(NOW_MS);
    assert_eq!(books.seen.lock().unwrap().proposals.len(), 1);
    // 1 October 2026: August-September has ended.
    let first_of_october = NOW_MS + 5 * 86_400_000;
    on(first_of_october);
    let (kind, card, payload) = books.seen.lock().unwrap().proposals[1].clone();
    assert_eq!(kind, "gst_return");
    assert_eq!(card.title, "GST return for 2026-08-01 to 2026-09-30");
    let refund = card
        .fields
        .iter()
        .find(|f| f.label == "Box 15 refund")
        .unwrap();
    assert!(matches!(&refund.value, FieldValue::Money(m) if m.cents == 1_500));
    // Acknowledged, it is not proposed again.
    books
        .run(
            Invocation::Apply {
                kind: "gst_return".into(),
                payload,
            },
            GOOD,
        )
        .unwrap();
    on(first_of_october);
    assert_eq!(books.seen.lock().unwrap().proposals.len(), 2);
}
