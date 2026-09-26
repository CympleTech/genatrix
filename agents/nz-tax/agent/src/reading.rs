//! Asking the model what a document says, and checking what it answers.
//! It reports the figures printed on the document; it never computes one.

use chrono::NaiveDate;
use serde::Deserialize;
use taxcore::{Currency, ExtractedInvoice, Money};

/// The extraction prompt.
pub const SYSTEM: &str = "/no_think
You read the text of a document attached to an email. Decide whether it is \
a tax invoice or receipt for something the account holder bought or paid \
for. If it is, report its fields exactly as printed.

Report, never compute: copy the numbers the document shows. Write amounts \
in cents as whole numbers: NZD 115.00 is 11500. If the document does not \
show a field, use null. A statement, a quote, an order confirmation without \
a total paid, or a newsletter is not an invoice.

Output one JSON object and nothing else:
{\"is_invoice\": true or false, \"supplier_name\": string or null, \
\"supplier_gst_number\": string or null, \"invoice_number\": string or null, \
\"invoice_date\": \"YYYY-MM-DD\" or null, \"currency\": \"NZD\", \
\"subtotal_cents\": integer or null, \"gst_cents\": integer or null, \
\"total_cents\": integer or null, \"confidence\": number from 0 to 1}";

/// How much of the document the model sees.
pub const EXCERPT: usize = 6000;

/// The prompt for one document.
#[must_use]
pub fn prompt(subject: &str, from: &str, text: &str) -> String {
    let body: String = text.chars().take(EXCERPT).collect();
    format!("Email subject: {subject}\nFrom: {from}\n--- document text ---\n{body}\n--- end ---")
}

/// What the model made of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reading {
    /// Not an invoice.
    NotInvoice,
    /// An invoice, and how sure the model says it is (0 to 100).
    Invoice(ExtractedInvoice, u8),
    /// The answer could not be used.
    Unusable(String),
}

#[derive(Deserialize)]
struct Answer {
    is_invoice: bool,
    supplier_name: Option<String>,
    supplier_gst_number: Option<String>,
    invoice_number: Option<String>,
    invoice_date: Option<String>,
    currency: Option<String>,
    subtotal_cents: Option<i64>,
    gst_cents: Option<i64>,
    total_cents: Option<i64>,
    confidence: Option<f64>,
}

/// Read the model's answer. A missing total or an unknown currency makes
/// it unusable rather than guessed at.
#[must_use]
pub fn parse(reply: &str) -> Reading {
    let reply = match reply.rfind("</think>") {
        Some(i) => &reply[i + "</think>".len()..],
        None => reply,
    };
    let (Some(start), Some(end)) = (reply.find('{'), reply.rfind('}')) else {
        return Reading::Unusable("no JSON in the answer".into());
    };
    let Ok(a) = serde_json::from_str::<Answer>(&reply[start..=end]) else {
        return Reading::Unusable("the answer was not the JSON asked for".into());
    };
    if !a.is_invoice {
        return Reading::NotInvoice;
    }
    let Some(total) = a.total_cents else {
        return Reading::Unusable("no total".into());
    };
    let Ok(currency) = Currency::new(a.currency.as_deref().unwrap_or("NZD").trim()) else {
        return Reading::Unusable("an unknown currency".into());
    };
    let clean = |s: Option<String>| s.map(|x| x.trim().to_owned()).filter(|x| !x.is_empty());
    let money = |c: Option<i64>| c.map(|c| Money::new(c, currency));
    let confidence = a.confidence.unwrap_or(0.0).clamp(0.0, 1.0);
    Reading::Invoice(
        ExtractedInvoice {
            supplier_name: clean(a.supplier_name),
            supplier_gst_number: clean(a.supplier_gst_number),
            invoice_number: clean(a.invoice_number),
            invoice_date: a
                .invoice_date
                .and_then(|d| NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").ok()),
            currency,
            subtotal: money(a.subtotal_cents),
            gst: money(a.gst_cents),
            total: Money::new(total, currency),
            lines: Vec::new(),
        },
        // Whole percent: the floor is 80.
        u8::try_from((confidence * 100.0).round() as i64).unwrap_or(0),
    )
}

/// The prompt that picks an expense account.
#[must_use]
pub fn classify_prompt(supplier: &str, text: &str, accounts: &[(String, String)]) -> String {
    let list: Vec<String> = accounts.iter().map(|(c, n)| format!("{c}: {n}")).collect();
    let body: String = text.chars().take(1500).collect();
    format!(
        "/no_think\nWhich account does this purchase belong to? Answer with the code only.\n\
         Accounts:\n{}\n\nSupplier: {supplier}\nDocument:\n{body}",
        list.join("\n")
    )
}

/// The account code in the model's answer, if it names one of `codes`.
#[must_use]
pub fn chosen<'a>(reply: &str, codes: &'a [String]) -> Option<&'a str> {
    let reply = match reply.rfind("</think>") {
        Some(i) => &reply[i + "</think>".len()..],
        None => reply,
    };
    codes.iter().map(String::as_str).find(|c| reply.contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invoice_is_read_to_the_cent() {
        let r = parse(
            "<think></think>{\"is_invoice\": true, \"supplier_name\": \" Power Co \", \
             \"supplier_gst_number\": \"123-456-789\", \"invoice_number\": \"42\", \
             \"invoice_date\": \"2026-09-03\", \"currency\": \"NZD\", \"subtotal_cents\": 10000, \
             \"gst_cents\": 1500, \"total_cents\": 11500, \"confidence\": 0.93}",
        );
        let Reading::Invoice(inv, confidence) = r else {
            panic!("{r:?}")
        };
        assert_eq!(inv.supplier_name.as_deref(), Some("Power Co"));
        assert_eq!(inv.total, Money::nzd(11500));
        assert_eq!(inv.gst, Some(Money::nzd(1500)));
        assert_eq!(inv.invoice_date, NaiveDate::from_ymd_opt(2026, 9, 3));
        assert_eq!(confidence, 93);
    }

    #[test]
    fn what_is_not_an_invoice_or_not_usable_says_so() {
        assert_eq!(parse("{\"is_invoice\": false}"), Reading::NotInvoice);
        assert!(matches!(parse("no idea"), Reading::Unusable(_)));
        assert!(matches!(
            parse("{\"is_invoice\": true, \"total_cents\": null}"),
            Reading::Unusable(_)
        ));
        assert!(matches!(
            parse("{\"is_invoice\": true, \"total_cents\": 5, \"currency\": \"dollars\"}"),
            Reading::Unusable(_)
        ));
    }

    #[test]
    fn the_account_named_is_found() {
        let codes = vec!["6100-office".to_owned(), "6200-utilities".to_owned()];
        assert_eq!(
            chosen("<think>x</think> 6200-utilities", &codes),
            Some("6200-utilities")
        );
        assert_eq!(chosen("travel", &codes), None);
    }
}
