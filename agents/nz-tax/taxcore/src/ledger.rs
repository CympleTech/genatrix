use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Result, TaxError};
use crate::gst::GstTreatment;
use crate::ids::{AccountCode, EntryId, PostingId};
use crate::money::{Currency, Money};
use crate::taxyear::TaxYear;

/// The original amount and the exact rate used to bring it into base currency.
/// Kept as a rational so the conversion can be re-derived and audited rather
/// than trusted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignAmount {
    pub original: Money,
    pub rate_numerator: i64,
    pub rate_denominator: i64,
    pub rate_date: NaiveDate,
    /// Where the rate came from, e.g. "ird-monthly-average" or "xe.com".
    pub rate_source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posting {
    pub id: PostingId,
    pub account: AccountCode,
    /// Positive is a debit, negative a credit. Always in the ledger's base
    /// currency; anything foreign carries its origin in `foreign`.
    pub amount: Money,
    pub gst_treatment: GstTreatment,
    /// The GST content of `amount`, when it has been determined. Stored rather
    /// than recomputed so a historical return never shifts under a rate change.
    pub gst_amount: Option<Money>,
    pub foreign: Option<ForeignAmount>,
    pub memo: Option<String>,
}

impl Posting {
    pub fn new(account: AccountCode, amount: Money, gst_treatment: GstTreatment) -> Self {
        Posting {
            id: PostingId::new(),
            account,
            amount,
            gst_treatment,
            gst_amount: None,
            foreign: None,
            memo: None,
        }
    }

    pub fn with_gst_amount(mut self, gst: Money) -> Self {
        self.gst_amount = Some(gst);
        self
    }

    pub fn with_foreign(mut self, foreign: ForeignAmount) -> Self {
        self.foreign = Some(foreign);
        self
    }

    pub fn with_memo(mut self, memo: impl Into<String>) -> Self {
        self.memo = Some(memo.into());
        self
    }

    pub fn is_debit(&self) -> bool {
        self.amount.cents > 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    /// Proposed but not part of any return yet. The review queue works here.
    Draft,
    Posted,
    /// Rejected in review, never posted. Kept because the proposal itself is
    /// part of history — deleting it would hide what the agent suggested.
    Voided,
    /// Superseded by a reversing entry. The original row is never mutated.
    Reversed,
}

/// Who or what created an entry. Recorded because "the agent decided this" and
/// "I decided this" carry different weight when a number is questioned later.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EntrySource {
    /// An agent proposed it, naming the model that did the work.
    Agent {
        model: String,
    },
    Human,
    /// Mechanically derived from an import, e.g. a bank statement row.
    Import {
        batch: String,
    },
    /// Produced by a deterministic rule, e.g. a home-office apportionment.
    Rule {
        rule_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: EntryId,
    pub date: NaiveDate,
    pub narration: String,
    pub postings: Vec<Posting>,
    pub status: EntryStatus,
    pub source: EntrySource,
    pub created_at: DateTime<Utc>,
    /// Set when this entry exists to cancel an earlier one.
    pub reverses: Option<EntryId>,
}

impl Entry {
    pub fn tax_year(&self) -> TaxYear {
        TaxYear::containing(self.date)
    }

    pub fn base_currency(&self) -> Currency {
        self.postings
            .first()
            .map(|p| p.amount.currency)
            .unwrap_or(Currency::NZD)
    }

    pub fn residual(&self) -> Result<Money> {
        let currency = self.base_currency();
        self.postings
            .iter()
            .try_fold(Money::zero(currency), |acc, p| acc.add(p.amount))
    }

    pub fn is_balanced(&self) -> bool {
        self.residual().map(|r| r.is_zero()).unwrap_or(false)
    }

    /// Total GST recorded across the entry's postings.
    pub fn total_gst(&self) -> Result<Money> {
        let currency = self.base_currency();
        self.postings
            .iter()
            .filter_map(|p| p.gst_amount)
            .try_fold(Money::zero(currency), |acc, g| acc.add(g))
    }

    /// A mirror-image entry that cancels this one. Corrections are made by
    /// reversal rather than edit so the audit trail stays append-only.
    pub fn reversal(&self, date: NaiveDate, source: EntrySource) -> Entry {
        let postings = self
            .postings
            .iter()
            .map(|p| Posting {
                id: PostingId::new(),
                account: p.account.clone(),
                amount: p.amount.neg(),
                gst_treatment: p.gst_treatment,
                gst_amount: p.gst_amount.map(|g| g.neg()),
                foreign: p.foreign.clone().map(|f| ForeignAmount {
                    original: f.original.neg(),
                    ..f
                }),
                memo: p.memo.clone(),
            })
            .collect();

        Entry {
            id: EntryId::new(),
            date,
            narration: format!("Reversal of: {}", self.narration),
            postings,
            status: EntryStatus::Posted,
            source,
            created_at: Utc::now(),
            reverses: Some(self.id),
        }
    }
}

pub struct EntryBuilder {
    date: NaiveDate,
    narration: String,
    postings: Vec<Posting>,
    status: EntryStatus,
    source: EntrySource,
    base_currency: Currency,
}

impl EntryBuilder {
    pub fn new(date: NaiveDate, narration: impl Into<String>, source: EntrySource) -> Self {
        EntryBuilder {
            date,
            narration: narration.into(),
            postings: Vec::new(),
            status: EntryStatus::Draft,
            source,
            base_currency: Currency::NZD,
        }
    }

    pub fn base_currency(mut self, currency: Currency) -> Self {
        self.base_currency = currency;
        self
    }

    pub fn status(mut self, status: EntryStatus) -> Self {
        self.status = status;
        self
    }

    pub fn posting(mut self, posting: Posting) -> Self {
        self.postings.push(posting);
        self
    }

    pub fn debit(self, account: AccountCode, amount: Money, gst: GstTreatment) -> Self {
        self.posting(Posting::new(account, amount.abs(), gst))
    }

    pub fn credit(self, account: AccountCode, amount: Money, gst: GstTreatment) -> Self {
        self.posting(Posting::new(account, amount.abs().neg(), gst))
    }

    pub fn build(self) -> Result<Entry> {
        if self.postings.len() < 2 {
            return Err(TaxError::TooFewPostings(self.postings.len()));
        }
        for posting in &self.postings {
            if posting.amount.currency != self.base_currency {
                return Err(TaxError::NotBaseCurrency {
                    base: self.base_currency.to_string(),
                    found: posting.amount.currency.to_string(),
                });
            }
        }

        let residual = self
            .postings
            .iter()
            .try_fold(Money::zero(self.base_currency), |acc, p| acc.add(p.amount))?;
        if !residual.is_zero() {
            return Err(TaxError::Unbalanced {
                residual: residual.to_string(),
            });
        }

        Ok(Entry {
            id: EntryId::new(),
            date: self.date,
            narration: self.narration,
            postings: self.postings,
            status: self.status,
            source: self.source,
            created_at: Utc::now(),
            reverses: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(s: &str) -> AccountCode {
        AccountCode::new(s).unwrap()
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2025, 8, 14).unwrap()
    }

    fn agent() -> EntrySource {
        EntrySource::Agent {
            model: "test".into(),
        }
    }

    fn office_supplies_entry() -> Entry {
        EntryBuilder::new(date(), "Officeworks paper", agent())
            .debit(
                code("6100-office"),
                Money::nzd(11500),
                GstTreatment::Standard,
            )
            .credit(
                code("1010-bank"),
                Money::nzd(11500),
                GstTreatment::NotSubject,
            )
            .build()
            .unwrap()
    }

    #[test]
    fn balanced_entry_builds() {
        let entry = office_supplies_entry();
        assert!(entry.is_balanced());
        assert_eq!(entry.tax_year(), TaxYear(2026));
    }

    #[test]
    fn unbalanced_entry_is_rejected() {
        let err = EntryBuilder::new(date(), "typo", agent())
            .debit(
                code("6100-office"),
                Money::nzd(11500),
                GstTreatment::Standard,
            )
            .credit(
                code("1010-bank"),
                Money::nzd(11000),
                GstTreatment::NotSubject,
            )
            .build()
            .unwrap_err();
        assert!(matches!(err, TaxError::Unbalanced { .. }));
    }

    #[test]
    fn single_posting_is_rejected() {
        let err = EntryBuilder::new(date(), "dangling", agent())
            .debit(code("6100-office"), Money::nzd(0), GstTreatment::Standard)
            .build()
            .unwrap_err();
        assert!(matches!(err, TaxError::TooFewPostings(1)));
    }

    #[test]
    fn foreign_currency_postings_are_rejected_in_the_ledger() {
        let err = EntryBuilder::new(date(), "usd invoice", agent())
            .debit(
                code("6100-office"),
                Money::new(11500, Currency::USD),
                GstTreatment::Standard,
            )
            .credit(
                code("1010-bank"),
                Money::nzd(11500),
                GstTreatment::NotSubject,
            )
            .build()
            .unwrap_err();
        assert!(matches!(err, TaxError::NotBaseCurrency { .. }));
    }

    #[test]
    fn reversal_cancels_the_original() {
        let entry = office_supplies_entry();
        let reversal = entry.reversal(date(), EntrySource::Human);

        assert_eq!(reversal.reverses, Some(entry.id));
        assert!(reversal.is_balanced());

        let combined = entry
            .residual()
            .unwrap()
            .add(reversal.residual().unwrap())
            .unwrap();
        assert!(combined.is_zero());

        for (original, reversed) in entry.postings.iter().zip(&reversal.postings) {
            assert_eq!(original.amount.neg(), reversed.amount);
            assert_ne!(original.id, reversed.id);
        }
    }

    #[test]
    fn gst_totals_only_count_postings_that_carry_it() {
        let entry = EntryBuilder::new(date(), "mixed", agent())
            .posting(
                Posting::new(
                    code("6100-office"),
                    Money::nzd(11500),
                    GstTreatment::Standard,
                )
                .with_gst_amount(Money::nzd(1500)),
            )
            .credit(
                code("1010-bank"),
                Money::nzd(11500),
                GstTreatment::NotSubject,
            )
            .build()
            .unwrap();
        assert_eq!(entry.total_gst().unwrap(), Money::nzd(1500));
    }
}
