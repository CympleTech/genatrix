use serde::{Deserialize, Serialize};

use crate::error::{Result, TaxError};
use crate::ids::{BankTxnId, DocumentId, EntryId, ExtractionId};
use crate::money::Money;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum SourceRef {
    Document(DocumentId),
    Extraction(ExtractionId),
    BankTxn(BankTxnId),
    /// Someone asserted it with no document behind it. Legitimate for things
    /// like a mileage claim, but it should be visible that nothing backs it.
    Manual,
}

/// What a journal entry was derived from. Every entry has one; an entry with an
/// empty source list is a defect, not a special case.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provenance {
    pub entry: EntryId,
    pub sources: Vec<SourceRef>,
    pub note: Option<String>,
}

impl Provenance {
    pub fn new(entry: EntryId, sources: Vec<SourceRef>) -> Self {
        Provenance {
            entry,
            sources,
            note: None,
        }
    }

    pub fn is_backed_by_document(&self) -> bool {
        self.sources
            .iter()
            .any(|s| matches!(s, SourceRef::Document(_) | SourceRef::Extraction(_)))
    }
}

/// One entry's contribution to one line of a return.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Contribution {
    pub entry: EntryId,
    pub amount: Money,
    pub narration: String,
    pub sources: Vec<SourceRef>,
}

/// A single figure on a return, carrying everything that produced it.
///
/// The engine's actual product is not the number — it is being able to answer
/// "where did this come from" years later, which is what an IRD query asks.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReturnLine {
    /// Stable identifier from the rule file, e.g. "gst101.box5".
    pub code: String,
    pub label: String,
    pub amount: Money,
    pub contributions: Vec<Contribution>,
}

impl ReturnLine {
    pub fn new(
        code: impl Into<String>,
        label: impl Into<String>,
        amount: Money,
        contributions: Vec<Contribution>,
    ) -> Self {
        ReturnLine {
            code: code.into(),
            label: label.into(),
            amount,
            contributions,
        }
    }

    /// The figure must equal the sum of what is behind it. Checked on every
    /// generated return: a line that cannot be explained is not shipped.
    pub fn verify(&self) -> Result<()> {
        let summed = Money::sum(
            &self
                .contributions
                .iter()
                .map(|c| c.amount)
                .collect::<Vec<_>>(),
            self.amount.currency,
        )?;
        if summed != self.amount {
            let residual = summed.sub(self.amount)?;
            return Err(TaxError::Unbalanced {
                residual: format!("{} on line {}", residual, self.code),
            });
        }
        Ok(())
    }

    pub fn unbacked_contributions(&self) -> Vec<&Contribution> {
        self.contributions
            .iter()
            .filter(|c| {
                !c.sources
                    .iter()
                    .any(|s| matches!(s, SourceRef::Document(_) | SourceRef::Extraction(_)))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contribution(cents: i64) -> Contribution {
        Contribution {
            entry: EntryId::new(),
            amount: Money::nzd(cents),
            narration: "something".into(),
            sources: vec![SourceRef::Document(DocumentId::new())],
        }
    }

    #[test]
    fn a_line_that_sums_verifies() {
        let line = ReturnLine::new(
            "gst101.box5",
            "Total sales and income",
            Money::nzd(432000),
            vec![contribution(400000), contribution(32000)],
        );
        assert!(line.verify().is_ok());
    }

    #[test]
    fn a_line_that_does_not_sum_is_rejected() {
        let line = ReturnLine::new(
            "gst101.box5",
            "Total sales and income",
            Money::nzd(432000),
            vec![contribution(400000)],
        );
        assert!(line.verify().is_err());
    }

    #[test]
    fn unbacked_contributions_are_surfaced() {
        let mut manual = contribution(5000);
        manual.sources = vec![SourceRef::Manual];
        let line = ReturnLine::new(
            "ir3.expenses",
            "Expenses",
            Money::nzd(15000),
            vec![contribution(10000), manual],
        );
        assert!(line.verify().is_ok());
        assert_eq!(line.unbacked_contributions().len(), 1);
    }
}
