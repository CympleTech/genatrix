//! The books: the chart of accounts, an entry from a checked invoice, and
//! the returns computed from posted entries. No I/O; the guest reads and
//! writes the space and hands the rows in.
//!
//! The model reports the figures on a document. Everything here is
//! arithmetic on those figures and on the rule file, the way the finance
//! engine it comes from did it.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use taxcore::{
    Account, AccountCode, AccountKind, Currency, Entry, EntryBuilder, EntrySource,
    ExtractedInvoice, GstPeriod, GstTreatment, Money, Posting, Rounding, TaxYear,
};
use taxrules::RuleSet;

/// The account every purchase is paid from.
pub const FUNDING: &str = "1010-bank";
/// Where a purchase goes when nothing better is known.
pub const OTHER: &str = "6900-other";

fn code(c: &str) -> AccountCode {
    AccountCode::new(c).expect("the chart's codes are valid")
}

/// The chart a sole trader in New Zealand starts with. Each expense account
/// carries the GST treatment its purchases usually have; a default is a
/// starting point the user approves, never a silent decision.
#[must_use]
pub fn chart() -> Vec<Account> {
    use AccountKind::{Asset, Expense, Income};
    use GstTreatment::{Exempt, Standard};
    vec![
        Account::new(code(FUNDING), "Bank", Asset),
        Account::new(code("4000-sales"), "Sales", Income).with_gst(Standard),
        Account::new(
            code("6100-office"),
            "Office, software and equipment",
            Expense,
        )
        .with_gst(Standard),
        Account::new(code("6200-utilities"), "Power, phone and internet", Expense)
            .with_gst(Standard),
        Account::new(code("6300-travel"), "Travel and vehicle", Expense).with_gst(Standard),
        Account::new(code("6400-meals"), "Meals and entertainment", Expense).with_gst(Standard),
        Account::new(
            code("6500-professional"),
            "Accounting, legal and other services",
            Expense,
        )
        .with_gst(Standard),
        Account::new(code("6600-insurance"), "Insurance", Expense).with_gst(Standard),
        Account::new(code("6700-bank-fees"), "Bank fees and interest", Expense).with_gst(Exempt),
        Account::new(code(OTHER), "Other expenses", Expense).with_gst(Standard),
    ]
}

/// The expense accounts, for the model to choose among.
#[must_use]
pub fn expense_accounts() -> Vec<Account> {
    chart()
        .into_iter()
        .filter(|a| a.kind == AccountKind::Expense)
        .collect()
}

/// The account with this code, if the chart has it.
#[must_use]
pub fn account(c: &str) -> Option<Account> {
    chart().into_iter().find(|a| a.code.as_str() == c)
}

/// A purchase entry from an invoice that passed its checks: the expense
/// debited with the GST the invoice states (or the rule rate's share of the
/// total when it states none), the bank credited. A draft; approval posts it.
pub fn purchase(
    invoice: &ExtractedInvoice,
    expense: &Account,
    rules: &RuleSet,
    model: &str,
) -> Result<Entry, String> {
    if invoice.currency != Currency::NZD {
        return Err(format!("only NZD is kept here, not {}", invoice.currency));
    }
    let date = invoice.invoice_date.ok_or("the invoice has no date")?;
    let treatment = expense
        .default_gst_treatment
        .unwrap_or(GstTreatment::Standard);
    let gst = if treatment.attracts_gst() {
        Some(match invoice.gst {
            Some(stated) if !stated.is_zero() => stated,
            _ => rules
                .gst_rate()
                .extract_from_inclusive(invoice.total, Rounding::HalfUp)
                .map_err(|e| e.to_string())?,
        })
    } else {
        None
    };
    let supplier = invoice
        .supplier_name
        .as_deref()
        .unwrap_or("Unknown supplier");
    let narration = match &invoice.invoice_number {
        Some(n) => format!("{supplier} {n}"),
        None => supplier.to_owned(),
    };
    let mut debit = Posting::new(expense.code.clone(), invoice.total.abs(), treatment);
    if let Some(gst) = gst {
        debit = debit.with_gst_amount(gst);
    }
    EntryBuilder::new(
        date,
        narration,
        EntrySource::Agent {
            model: model.to_owned(),
        },
    )
    .posting(debit)
    .credit(code(FUNDING), invoice.total, GstTreatment::NotSubject)
    .build()
    .map_err(|e| e.to_string())
}

/// What the posted entries in a span add up to, by the kinds a return asks
/// for. Signed, so a reversal pair nets to nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Totals {
    /// Income with GST, standard and zero-rated.
    pub sales_incl: Money,
    /// The zero-rated part of it.
    pub zero_rated: Money,
    /// GST on sales.
    pub output_gst: Money,
    /// Standard-rated purchases with GST.
    pub purchases_incl: Money,
    /// GST on those purchases.
    pub input_gst: Money,
    /// Income without GST, what income tax sees.
    pub income_excl: Money,
    /// Expenses without GST.
    pub expenses_excl: Money,
    /// Entries counted.
    pub entries: usize,
}

/// Add up the entries dated `from` to `to`, both included.
pub fn totals(
    entries: &[Entry],
    from: NaiveDate,
    to: NaiveDate,
) -> Result<(Totals, Vec<String>), String> {
    let chart: BTreeMap<String, Account> = chart()
        .into_iter()
        .map(|a| (a.code.to_string(), a))
        .collect();
    let zero = Money::zero(Currency::NZD);
    let mut t = Totals {
        sales_incl: zero,
        zero_rated: zero,
        output_gst: zero,
        purchases_incl: zero,
        input_gst: zero,
        income_excl: zero,
        expenses_excl: zero,
        entries: 0,
    };
    let mut warnings = Vec::new();
    let e = |r: taxcore::Result<Money>| r.map_err(|e| e.to_string());
    for entry in entries.iter().filter(|x| x.date >= from && x.date <= to) {
        t.entries += 1;
        for posting in &entry.postings {
            let Some(account) = chart.get(posting.account.as_str()) else {
                warnings.push(format!(
                    "{}: unknown account {}",
                    entry.narration, posting.account
                ));
                continue;
            };
            let gst = posting.gst_amount.unwrap_or(zero);
            let excl = e(posting.amount.sub(gst))?;
            match account.kind {
                AccountKind::Income => {
                    if posting.gst_treatment.included_in_return_totals() {
                        t.sales_incl = e(t.sales_incl.sub(posting.amount))?;
                    }
                    if posting.gst_treatment == GstTreatment::ZeroRated {
                        t.zero_rated = e(t.zero_rated.sub(posting.amount))?;
                    }
                    t.output_gst = e(t.output_gst.sub(gst))?;
                    t.income_excl = e(t.income_excl.sub(excl))?;
                }
                AccountKind::Expense => {
                    if posting.gst_treatment == GstTreatment::Standard {
                        t.purchases_incl = e(t.purchases_incl.add(posting.amount))?;
                        if posting.gst_amount.is_none() {
                            // Never invent a credit.
                            warnings.push(format!(
                                "{}: no GST recorded, so none is claimed",
                                entry.narration
                            ));
                        }
                        t.input_gst = e(t.input_gst.add(gst))?;
                    }
                    t.expenses_excl = e(t.expenses_excl.add(excl))?;
                }
                AccountKind::Asset | AccountKind::Liability | AccountKind::Equity => {}
            }
        }
    }
    Ok((t, warnings))
}

/// A GST return for one period, box by box (GST101).
#[derive(Clone, Debug)]
pub struct Gst101 {
    /// The period.
    pub period: GstPeriod,
    /// When it is due.
    pub due: NaiveDate,
    /// Boxes 5 to 15; 9 and 13, adjustments, are zero here.
    pub boxes: BTreeMap<u8, Money>,
    /// Entries counted.
    pub entries: usize,
    /// What the figures do not cover, or doubts about them.
    pub warnings: Vec<String>,
}

impl Gst101 {
    /// Box 15: positive is GST to pay, negative a refund.
    #[must_use]
    pub fn to_pay(&self) -> Money {
        self.boxes[&15]
    }
}

/// The GST return for `period` from the posted entries.
pub fn gst101(entries: &[Entry], rules: &RuleSet, period: GstPeriod) -> Result<Gst101, String> {
    let (t, mut warnings) = totals(entries, period.start, period.end)?;
    let e = |r: taxcore::Result<Money>| r.map_err(|e| e.to_string());
    let zero = Money::zero(Currency::NZD);
    let mut boxes = BTreeMap::new();
    boxes.insert(5, t.sales_incl);
    boxes.insert(6, t.zero_rated);
    boxes.insert(7, e(t.sales_incl.sub(t.zero_rated))?);
    boxes.insert(8, t.output_gst);
    boxes.insert(9, zero);
    boxes.insert(10, t.output_gst);
    boxes.insert(11, t.purchases_incl);
    boxes.insert(12, t.input_gst);
    boxes.insert(13, zero);
    boxes.insert(14, t.input_gst);
    boxes.insert(15, e(t.output_gst.sub(t.input_gst))?);
    if let Some(from) = &rules.meta.copied_from {
        warnings.push(format!(
            "the rules for {} are copied from {from} and not yet checked against IRD",
            rules.meta.tax_year
        ));
    }
    Ok(Gst101 {
        period,
        due: rules.gst.due_date(period.end),
        boxes,
        entries: t.entries,
        warnings,
    })
}

/// A sole trader's year for IR3: income, expenses, profit, and the tax on it.
#[derive(Clone, Debug)]
pub struct Ir3 {
    /// The year.
    pub year: TaxYear,
    /// Income without GST.
    pub income: Money,
    /// Expenses without GST.
    pub expenses: Money,
    /// The difference.
    pub profit: Money,
    /// Tax on the profit, band by band.
    pub tax: Money,
    /// When a self-filed return is due.
    pub due: NaiveDate,
    /// Entries counted.
    pub entries: usize,
    /// What the figures do not cover.
    pub notes: Vec<String>,
}

/// The IR3 picture for `year` from the posted entries.
pub fn ir3(entries: &[Entry], rules: &RuleSet, year: TaxYear) -> Result<Ir3, String> {
    let (t, mut notes) = totals(entries, year.start(), year.end())?;
    let profit = t
        .income_excl
        .sub(t.expenses_excl)
        .map_err(|e| e.to_string())?;
    let tax = rules
        .income_tax
        .tax_on(profit)
        .map_err(|e| e.to_string())?
        .total;
    notes
        .push("entertainment (50%), home office and vehicle apportionments are not applied".into());
    if let Some(from) = &rules.meta.copied_from {
        notes.push(format!(
            "the rules for {} are copied from {from} and not yet checked against IRD",
            rules.meta.tax_year
        ));
    }
    Ok(Ir3 {
        year,
        income: t.income_excl,
        expenses: t.expenses_excl,
        profit,
        tax,
        due: rules.income_tax.self_filed_due(year),
        entries: t.entries,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::for_date;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn invoice(total: i64, gst: Option<i64>, date: NaiveDate) -> ExtractedInvoice {
        ExtractedInvoice {
            supplier_name: Some("Power Co".into()),
            supplier_gst_number: Some("123-456-789".into()),
            invoice_number: Some("42".into()),
            invoice_date: Some(date),
            currency: Currency::NZD,
            subtotal: gst.map(|g| Money::nzd(total - g)),
            gst: gst.map(Money::nzd),
            total: Money::nzd(total),
            lines: Vec::new(),
        }
    }

    #[test]
    fn a_purchase_debits_the_expense_with_its_gst_and_credits_the_bank() {
        let (rules, _) = for_date(d(2025, 10, 1)).unwrap();
        let e = purchase(
            &invoice(11_500, Some(1_500), d(2025, 10, 1)),
            &account("6200-utilities").unwrap(),
            &rules,
            "m",
        )
        .unwrap();
        assert!(e.is_balanced());
        assert_eq!(e.postings[0].amount, Money::nzd(11_500));
        assert_eq!(e.postings[0].gst_amount, Some(Money::nzd(1_500)));
        assert_eq!(e.postings[1].amount, Money::nzd(-11_500));
        // No GST stated: the rule rate's share of the total, 3/23.
        let e = purchase(
            &invoice(11_500, None, d(2025, 10, 1)),
            &account("6100-office").unwrap(),
            &rules,
            "m",
        )
        .unwrap();
        assert_eq!(e.postings[0].gst_amount, Some(Money::nzd(1_500)));
        // An exempt account claims nothing.
        let e = purchase(
            &invoice(1_000, None, d(2025, 10, 1)),
            &account("6700-bank-fees").unwrap(),
            &rules,
            "m",
        )
        .unwrap();
        assert_eq!(e.postings[0].gst_amount, None);
    }

    #[test]
    fn a_return_adds_up_the_period_and_nothing_else() {
        let (rules, _) = for_date(d(2025, 10, 1)).unwrap();
        let power = account("6200-utilities").unwrap();
        let entries = vec![
            purchase(
                &invoice(11_500, Some(1_500), d(2025, 8, 3)),
                &power,
                &rules,
                "m",
            )
            .unwrap(),
            purchase(
                &invoice(23_000, Some(3_000), d(2025, 9, 30)),
                &power,
                &rules,
                "m",
            )
            .unwrap(),
            purchase(
                &invoice(5_750, Some(750), d(2025, 10, 1)),
                &power,
                &rules,
                "m",
            )
            .unwrap(),
        ];
        let period =
            taxcore::GstFrequency::two_monthly_ending_march().period_containing(d(2025, 9, 1));
        let r = gst101(&entries, &rules, period).unwrap();
        assert_eq!(r.entries, 2);
        assert_eq!(r.boxes[&11], Money::nzd(34_500));
        assert_eq!(r.boxes[&12], Money::nzd(4_500));
        assert_eq!(r.to_pay(), Money::nzd(-4_500), "purchases only: a refund");
        assert_eq!(r.due, d(2025, 10, 28));
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn a_year_under_copied_rules_says_so() {
        let (rules, copied) = for_date(d(2026, 9, 1)).unwrap();
        assert!(copied);
        let r = ir3(&[], &rules, TaxYear(2027)).unwrap();
        assert!(r.notes.iter().any(|n| n.contains("not yet checked")));
        assert_eq!(r.tax, Money::nzd(0));
    }
}
