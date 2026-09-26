//! The agent inside the sandbox: its space, its triggers, its answers.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use genatrix_agent_sdk::types::{
    Card, Field, FieldValue, Filter, Item, Money as WitMoney, Purpose, Value,
};
use genatrix_agent_sdk::{
    Guest, actions, blobs, export_agent, host, items, log, model, space, system, user,
};
use serde::{Deserialize, Serialize};
use taxcore::{Entry, EntryStatus, GstFrequency, GstPeriod, Money, TaxYear};

use crate::books::{self, Gst101};
use crate::reading::{self, Reading};
use crate::rules;
use crate::words::{self, chinese};

/// Documents read in one run; the rest wait for the next.
const PER_RUN: usize = 6;
/// Below this confidence (in percent) a reading goes to review.
const CONFIDENCE_FLOOR: u8 = 80;
/// Recorded as the source of every entry the agent proposes.
const MODEL: &str = "genatrix local model";

const SCHEMA: [&str; 9] = [
    "CREATE TABLE IF NOT EXISTS document (
        blob TEXT PRIMARY KEY, item TEXT NOT NULL, subject TEXT, status TEXT NOT NULL,
        detail TEXT, seen_ms INTEGER NOT NULL)",
    "CREATE TABLE IF NOT EXISTS extraction (
        blob TEXT NOT NULL, reading TEXT NOT NULL, issues TEXT NOT NULL,
        confidence INTEGER NOT NULL, at_ms INTEGER NOT NULL)",
    "CREATE TABLE IF NOT EXISTS entry (
        id TEXT PRIMARY KEY, date TEXT NOT NULL, body TEXT NOT NULL,
        blob TEXT, item TEXT, posted_ms INTEGER NOT NULL)",
    // The ledger is append-only: a mistake is corrected by a reversing
    // entry, never by changing or removing the one it corrects.
    "CREATE TRIGGER IF NOT EXISTS entry_kept BEFORE UPDATE ON entry
        BEGIN SELECT RAISE(ABORT, 'entries are append-only'); END",
    "CREATE TRIGGER IF NOT EXISTS entry_stays BEFORE DELETE ON entry
        BEGIN SELECT RAISE(ABORT, 'entries are append-only'); END",
    "CREATE TRIGGER IF NOT EXISTS extraction_kept BEFORE UPDATE ON extraction
        BEGIN SELECT RAISE(ABORT, 'readings are append-only'); END",
    "CREATE TRIGGER IF NOT EXISTS extraction_stays BEFORE DELETE ON extraction
        BEGIN SELECT RAISE(ABORT, 'readings are append-only'); END",
    "CREATE TABLE IF NOT EXISTS setting (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS gst_return (
        period TEXT PRIMARY KEY, body TEXT NOT NULL, acknowledged_ms INTEGER NOT NULL)",
];

fn ensure() -> Result<(), String> {
    for statement in SCHEMA {
        space::execute(statement, &[])?;
    }
    Ok(())
}

fn text(s: impl Into<String>) -> Value {
    Value::Text(s.into())
}

fn int(n: i64) -> Value {
    Value::Integer(n)
}

fn today() -> NaiveDate {
    DateTime::<Utc>::from_timestamp_millis(host::now_ms())
        .unwrap_or_default()
        .date_naive()
}

fn one_string(sql: &str, params: &[Value]) -> Result<Option<String>, String> {
    Ok(space::query(sql, params)?
        .into_iter()
        .next()
        .and_then(|r| r.into_iter().next())
        .and_then(|v| match v {
            Value::Text(t) => Some(t),
            Value::Integer(i) => Some(i.to_string()),
            _ => None,
        }))
}

fn count(sql: &str) -> Result<i64, String> {
    Ok(one_string(sql, &[])?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

fn seen(blob: &str) -> Result<bool, String> {
    Ok(one_string("SELECT status FROM document WHERE blob = ?1", &[text(blob)])?.is_some())
}

fn note(item: &Item, blob: &str, status: &str, detail: &str) -> Result<(), String> {
    space::execute(
        "INSERT OR REPLACE INTO document (blob, item, subject, status, detail, seen_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        &[
            text(blob),
            text(item.id.clone()),
            text(item.title.clone().unwrap_or_default()),
            text(status),
            text(detail),
            int(host::now_ms()),
        ],
    )?;
    Ok(())
}

fn entries() -> Result<Vec<Entry>, String> {
    space::query("SELECT body FROM entry ORDER BY date, id", &[])?
        .into_iter()
        .filter_map(|r| match r.into_iter().next() {
            Some(Value::Text(t)) => serde_json::from_str(&t).ok(),
            _ => None,
        })
        .map(Ok)
        .collect()
}

fn frequency() -> Result<GstFrequency, String> {
    let months = one_string("SELECT value FROM setting WHERE key = 'gst_months'", &[])?
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(2);
    GstFrequency::new(months, 3).map_err(|e| e.to_string())
}

fn wit_money(m: Money) -> FieldValue {
    FieldValue::Money(WitMoney {
        cents: m.cents,
        currency: m.currency.to_string(),
    })
}

fn field(label: &str, value: FieldValue) -> Field {
    Field {
        label: label.to_owned(),
        value,
    }
}

/// What approving a `record_entry` hands back to `apply`.
#[derive(Serialize, Deserialize)]
struct Proposed {
    blob: String,
    item: String,
    entry: Entry,
}

/// What approving a `gst_return` hands back to `apply`.
#[derive(Serialize, Deserialize)]
struct Acknowledged {
    period: String,
    to_pay_cents: i64,
    due: NaiveDate,
}

/// What happened to the documents one run looked at.
#[derive(Default)]
struct Tally {
    proposed: usize,
    review: usize,
    other: usize,
    waiting: usize,
}

/// Read the PDFs one item carries, up to what is left of this run's share.
fn read_item(item: &Item, tally: &mut Tally) -> Result<(), String> {
    for blob in &item.blobs {
        if !blob.mime.starts_with("application/pdf") || seen(&blob.id)? {
            continue;
        }
        if tally.proposed + tally.review + tally.other >= PER_RUN {
            tally.waiting += 1;
            continue;
        }
        match read_document(item, &blob.id)? {
            Outcome::Proposed => tally.proposed += 1,
            Outcome::Review => tally.review += 1,
            Outcome::Other => tally.other += 1,
            Outcome::Later => tally.waiting += 1,
        }
    }
    Ok(())
}

enum Outcome {
    Proposed,
    Review,
    Other,
    /// Not decided; tried again on the next scan.
    Later,
}

fn read_document(item: &Item, blob: &str) -> Result<Outcome, String> {
    let body = match blobs::text(blob) {
        Ok(t) if t.trim().chars().count() >= 20 => t,
        Ok(_) => {
            note(
                item,
                blob,
                "unreadable",
                "no text in it; a scan or a photo needs OCR",
            )?;
            return Ok(Outcome::Other);
        }
        Err(e) => {
            note(item, blob, "unreadable", &e)?;
            return Ok(Outcome::Other);
        }
    };
    let subject = item.title.clone().unwrap_or_default();
    let from = item.author.clone().unwrap_or_default();
    let reply = match model::call(
        Purpose::Extract,
        &[
            system(reading::SYSTEM),
            user(reading::prompt(&subject, &from, &body)),
        ],
    ) {
        Ok(r) => r,
        Err(e) => {
            log(format!("the model did not answer for {blob}: {e}"));
            return Ok(Outcome::Later);
        }
    };
    let (invoice, confidence) = match reading::parse(&reply) {
        Reading::NotInvoice => {
            note(item, blob, "not_invoice", "")?;
            return Ok(Outcome::Other);
        }
        Reading::Unusable(why) => {
            note(item, blob, "needs_review", &why)?;
            return Ok(Outcome::Review);
        }
        Reading::Invoice(inv, c) => (inv, c),
    };
    let Some((rules, _)) = rules::for_date(invoice.invoice_date.unwrap_or_else(today)) else {
        note(
            item,
            blob,
            "needs_review",
            "no rules for the tax year of this invoice",
        )?;
        return Ok(Outcome::Review);
    };
    let issues = invoice.validate(rules.gst_rate(), today());
    space::execute(
        "INSERT INTO extraction (blob, reading, issues, confidence, at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
        &[
            text(blob),
            text(serde_json::to_string(&invoice).map_err(|e| e.to_string())?),
            text(serde_json::to_string(&issues).map_err(|e| e.to_string())?),
            int(i64::from(confidence)),
            int(host::now_ms()),
        ],
    )?;
    let errors: Vec<&str> = issues
        .iter()
        .filter(|i| i.severity == taxcore::document::Severity::Error)
        .map(|i| i.message.as_str())
        .collect();
    if !errors.is_empty() || confidence < CONFIDENCE_FLOOR {
        let why = if errors.is_empty() {
            format!("the reading is only {confidence}% sure")
        } else {
            errors.join("; ")
        };
        note(item, blob, "needs_review", &why)?;
        return Ok(Outcome::Review);
    }

    let supplier = invoice.supplier_name.clone().unwrap_or_default();
    let accounts: Vec<(String, String)> = books::expense_accounts()
        .into_iter()
        .map(|a| (a.code.to_string(), a.name))
        .collect();
    let codes: Vec<String> = accounts.iter().map(|(c, _)| c.clone()).collect();
    let chosen = model::call(
        Purpose::Classify,
        &[user(reading::classify_prompt(&supplier, &body, &accounts))],
    )
    .ok()
    .and_then(|r| reading::chosen(&r, &codes).map(str::to_owned))
    .unwrap_or_else(|| books::OTHER.to_owned());
    let expense = books::account(&chosen)
        .unwrap_or_else(|| books::account(books::OTHER).expect("the chart has an other account"));
    let entry = match books::purchase(&invoice, &expense, &rules, MODEL) {
        Ok(e) => e,
        Err(why) => {
            note(item, blob, "needs_review", &why)?;
            return Ok(Outcome::Review);
        }
    };

    let gst = entry.postings[0].gst_amount.unwrap_or(Money::nzd(0));
    let mut fields = vec![
        field("Supplier", FieldValue::Text(supplier)),
        field(
            "Invoice",
            FieldValue::Text(invoice.invoice_number.clone().unwrap_or_default()),
        ),
        field("Date", FieldValue::Date(entry.date.to_string())),
        field("Total", wit_money(invoice.total)),
        field("GST", wit_money(gst)),
        field(
            "Account",
            FieldValue::Text(format!("{} {}", expense.code, expense.name)),
        ),
    ];
    if let Some(n) = &invoice.supplier_gst_number {
        fields.push(field("Supplier GST number", FieldValue::Text(n.clone())));
    }
    let warnings: Vec<&str> = issues.iter().map(|i| i.message.as_str()).collect();
    if !warnings.is_empty() {
        fields.push(field("Check", FieldValue::Text(warnings.join("; "))));
    }
    let card = Card {
        title: format!("Record {}", entry.narration),
        fields,
        evidence: vec![item.id.clone()],
    };
    let payload = serde_json::to_string(&Proposed {
        blob: blob.to_owned(),
        item: item.id.clone(),
        entry,
    })
    .map_err(|e| e.to_string())?;
    match actions::propose("record_entry", &card, &payload) {
        Ok(_) => {
            note(item, blob, "proposed", "")?;
            Ok(Outcome::Proposed)
        }
        Err(e) => {
            log(format!("could not propose {blob}: {e}"));
            Ok(Outcome::Later)
        }
    }
}

fn scan() -> Result<Tally, String> {
    let mut tally = Tally::default();
    let found = items::query(&Filter {
        since_ms: None,
        until_ms: None,
        text: None,
        limit: 200,
    });
    for item in &found {
        read_item(item, &mut tally)?;
    }
    Ok(tally)
}

fn tally_words(t: &Tally, zh: bool) -> String {
    if zh {
        let mut s = format!(
            "看了 {} 份单据：{} 份提议记账，{} 份需要你看一眼，{} 份不是发票或读不出来。",
            t.proposed + t.review + t.other,
            t.proposed,
            t.review,
            t.other
        );
        if t.waiting > 0 {
            s.push_str(&format!("还有 {} 份等着，再说一次\"扫描\"。", t.waiting));
        }
        s
    } else {
        let mut s = format!(
            "Looked at {} documents: {} proposed, {} need your eye, {} not invoices or unreadable.",
            t.proposed + t.review + t.other,
            t.proposed,
            t.review,
            t.other
        );
        if t.waiting > 0 {
            s.push_str(&format!(
                " {} more are waiting; say \"scan\" again.",
                t.waiting
            ));
        }
        s
    }
}

fn period_return(period: GstPeriod) -> Result<Gst101, String> {
    let (rules, _) = rules::for_date(period.end)
        .ok_or_else(|| format!("no rules for the tax year of {}", period.end))?;
    books::gst101(&entries()?, &rules, period)
}

fn review(zh: bool) -> Result<String, String> {
    let rows = space::query(
        "SELECT subject, detail FROM document WHERE status = 'needs_review' ORDER BY seen_ms DESC LIMIT 10",
        &[],
    )?;
    if rows.is_empty() {
        return Ok(if zh {
            "没有需要你看的单据。".into()
        } else {
            "Nothing needs your eye.".into()
        });
    }
    let mut out = String::from(if zh {
        "需要你看一眼的单据："
    } else {
        "Documents that need your eye:"
    });
    for row in rows {
        let cell = |v: &Value| match v {
            Value::Text(t) => t.clone(),
            _ => String::new(),
        };
        out.push_str(&format!("\n- {}: {}", cell(&row[0]), cell(&row[1])));
    }
    Ok(out)
}

fn status(zh: bool) -> Result<String, String> {
    let recorded = count("SELECT count(*) FROM entry")?;
    let proposed = count("SELECT count(*) FROM document WHERE status = 'proposed'")?;
    let review = count("SELECT count(*) FROM document WHERE status = 'needs_review'")?;
    let period = frequency()?.period_containing(today());
    let so_far = period_return(period)
        .map(|r| words::money(r.to_pay()))
        .unwrap_or_else(|e| e);
    Ok(if zh {
        format!(
            "账上已记 {recorded} 笔；{proposed} 笔等你批准；{review} 份单据需要你看一眼。\
             本期（{} 至 {}）目前应缴 GST：{so_far}。\n\n{}",
            period.start,
            period.end,
            words::help(true)
        )
    } else {
        format!(
            "{recorded} entries recorded; {proposed} waiting for your approval; {review} documents need your eye. \
             GST this period ({} to {}) so far: {so_far}.\n\n{}",
            period.start,
            period.end,
            words::help(false)
        )
    })
}

fn answer(said: &str) -> Result<String, String> {
    let zh = chinese(said);
    let s = said.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| s.contains(w));
    if has(&["scan", "扫描"]) {
        return Ok(tally_words(&scan()?, zh));
    }
    if has(&["frequency", "频率"]) {
        let months = if has(&["six", "6", "六"]) {
            6
        } else if has(&["month", "每月", "1"]) && !has(&["two", "2", "两", "二"]) {
            1
        } else {
            2
        };
        space::execute(
            "INSERT OR REPLACE INTO setting (key, value) VALUES ('gst_months', ?1)",
            &[text(months.to_string())],
        )?;
        return Ok(if zh {
            format!("GST 申报频率设为每 {months} 个月一期。")
        } else {
            format!("GST is now filed every {months} month(s).")
        });
    }
    if has(&["gst", "消费税"]) {
        let freq = frequency()?;
        let mut period = freq.period_containing(today());
        if has(&["last", "previous", "上"]) {
            period = freq.period_containing(period.start - Duration::days(1));
        }
        return Ok(words::gst101(&period_return(period)?, zh));
    }
    if has(&["income", "ir3", "所得税", "利润"]) {
        let year = TaxYear::containing(today());
        let (rules, _) = rules::for_year(year).ok_or("no rules for this tax year")?;
        return Ok(words::ir3(&books::ir3(&entries()?, &rules, year)?, zh));
    }
    if has(&["review", "待审", "检查"]) {
        return review(zh);
    }
    status(zh)
}

struct NzTax;

impl Guest for NzTax {
    fn on_items(ids: Vec<String>) -> Result<(), String> {
        ensure()?;
        let mut tally = Tally::default();
        for id in ids {
            if let Some(item) = items::get(&id) {
                read_item(&item, &mut tally)?;
            }
        }
        log(tally_words(&tally, false));
        Ok(())
    }

    fn on_message(said: String) -> Result<String, String> {
        ensure()?;
        answer(&said)
    }

    fn on_schedule(name: String) -> Result<(), String> {
        ensure()?;
        if name != "gst_period" {
            return Ok(());
        }
        // The day after a period ends, its return is ready to look at.
        let yesterday = today() - Duration::days(1);
        let period = frequency()?.period_containing(yesterday);
        if period.end != yesterday {
            return Ok(());
        }
        let label = period.label();
        if one_string(
            "SELECT period FROM gst_return WHERE period = ?1",
            &[text(label.clone())],
        )?
        .is_some()
        {
            return Ok(());
        }
        let r = period_return(period)?;
        let to_pay = r.to_pay();
        let mut fields = vec![
            field("Period", FieldValue::Text(label.clone())),
            field("Due", FieldValue::Date(r.due.to_string())),
            field("Box 5 sales incl. GST", wit_money(r.boxes[&5])),
            field("Box 8 GST on sales", wit_money(r.boxes[&8])),
            field("Box 11 purchases incl. GST", wit_money(r.boxes[&11])),
            field("Box 12 GST credit", wit_money(r.boxes[&12])),
            field(
                if to_pay.cents >= 0 {
                    "Box 15 to pay"
                } else {
                    "Box 15 refund"
                },
                wit_money(to_pay.abs()),
            ),
        ];
        for w in &r.warnings {
            fields.push(field("Note", FieldValue::Text(w.clone())));
        }
        let card = Card {
            title: format!("GST return for {label}"),
            fields,
            evidence: Vec::new(),
        };
        let payload = serde_json::to_string(&Acknowledged {
            period: label,
            to_pay_cents: to_pay.cents,
            due: r.due,
        })
        .map_err(|e| e.to_string())?;
        actions::propose("gst_return", &card, &payload).map(|_| ())
    }

    fn apply(kind: String, payload: String) -> Result<(), String> {
        ensure()?;
        match kind.as_str() {
            "record_entry" => {
                let Proposed {
                    blob,
                    item,
                    mut entry,
                } = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                if !entry.is_balanced() {
                    return Err("the entry does not balance".into());
                }
                // Approval is what posts it.
                entry.status = EntryStatus::Posted;
                space::execute(
                    "INSERT OR IGNORE INTO entry (id, date, body, blob, item, posted_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    &[
                        text(entry.id.to_string()),
                        text(entry.date.to_string()),
                        text(serde_json::to_string(&entry).map_err(|e| e.to_string())?),
                        text(blob.clone()),
                        text(item),
                        int(host::now_ms()),
                    ],
                )?;
                space::execute(
                    "UPDATE document SET status = 'recorded' WHERE blob = ?1",
                    &[text(blob)],
                )?;
                Ok(())
            }
            "gst_return" => {
                let a: Acknowledged = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                space::execute(
                    "INSERT OR IGNORE INTO gst_return (period, body, acknowledged_ms) VALUES (?1, ?2, ?3)",
                    &[text(a.period.clone()), text(payload), int(host::now_ms())],
                )?;
                Ok(())
            }
            other => Err(format!("unknown kind {other}")),
        }
    }
}

export_agent!(NzTax with_types_in genatrix_agent_sdk);
