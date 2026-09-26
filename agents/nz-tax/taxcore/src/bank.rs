use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::{BankTxnId, DocumentId, EntryId};
use crate::money::Money;

/// A row from a bank statement or feed. Statements are the completeness spine —
/// a receipt can go missing, a payment cannot — so these are imported first and
/// documents are matched onto them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BankTransaction {
    pub id: BankTxnId,
    pub account: String,
    pub date: NaiveDate,
    /// Positive is money in, negative money out, from the account holder's view.
    pub amount: Money,
    pub description: String,
    pub reference: Option<String>,
    pub import_batch: String,
    /// Stable across re-imports of overlapping date ranges, which is the normal
    /// case when someone exports "last 3 months" every month.
    pub dedup_hash: String,
    pub matched_document: Option<DocumentId>,
    pub entry: Option<EntryId>,
}

impl BankTransaction {
    pub fn new(
        account: impl Into<String>,
        date: NaiveDate,
        amount: Money,
        description: impl Into<String>,
        import_batch: impl Into<String>,
    ) -> Self {
        let account = account.into();
        let description = description.into();
        let dedup_hash = Self::compute_dedup_hash(&account, date, amount, &description);
        BankTransaction {
            id: BankTxnId::new(),
            account,
            date,
            amount,
            description,
            reference: None,
            import_batch: import_batch.into(),
            dedup_hash,
            matched_document: None,
            entry: None,
        }
    }

    pub fn compute_dedup_hash(
        account: &str,
        date: NaiveDate,
        amount: Money,
        description: &str,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(account.as_bytes());
        hasher.update([0]);
        hasher.update(date.to_string().as_bytes());
        hasher.update([0]);
        hasher.update(amount.cents.to_le_bytes());
        hasher.update(amount.currency.as_str().as_bytes());
        hasher.update([0]);
        hasher.update(description.trim().to_lowercase().as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub fn is_money_out(&self) -> bool {
        self.amount.cents < 0
    }
}

/// How closely a document lines up with a bank row. Deliberately coarse: the
/// engine proposes candidates and something else decides.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchStrength {
    /// Same amount, same day, supplier name found in the bank description.
    Exact,
    /// Same amount within the date window; supplier not confirmed.
    Probable,
    /// Amount matches but the date is at the edge of the window.
    Possible,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchCandidate {
    pub bank_txn: BankTxnId,
    pub document: DocumentId,
    pub strength: MatchStrength,
    pub day_gap: i64,
    pub reason: String,
}

/// Card settlement lags the receipt, so an exact-date match is the exception.
pub const DEFAULT_MATCH_WINDOW_DAYS: i64 = 3;

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn reimporting_the_same_row_produces_the_same_hash() {
        let a = BankTransaction::new(
            "asb-8842",
            d(2025, 8, 14),
            Money::nzd(-11500),
            "OFFICEWORKS",
            "jan",
        );
        let b = BankTransaction::new(
            "asb-8842",
            d(2025, 8, 14),
            Money::nzd(-11500),
            "officeworks ",
            "feb",
        );
        assert_eq!(a.dedup_hash, b.dedup_hash);
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn a_different_amount_is_a_different_row() {
        let a = BankTransaction::new(
            "asb-8842",
            d(2025, 8, 14),
            Money::nzd(-11500),
            "OFFICEWORKS",
            "jan",
        );
        let b = BankTransaction::new(
            "asb-8842",
            d(2025, 8, 14),
            Money::nzd(-11501),
            "OFFICEWORKS",
            "jan",
        );
        assert_ne!(a.dedup_hash, b.dedup_hash);
    }
}
