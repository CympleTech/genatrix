use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::gst::GstRate;
use crate::ids::{DocumentId, ExtractionId};
use crate::money::{Currency, Money, Rounding};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DocumentSource {
    Email {
        from: String,
        subject: String,
        message_id: String,
    },
    Upload,
    Photo,
    BankStatement {
        account: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentStatus {
    /// Stored, waiting for an agent to read it.
    PendingExtraction,
    /// Extracted and validated cleanly.
    Extracted,
    /// Extracted but something failed validation; a human has to look.
    NeedsReview,
    /// Deliberately excluded — a duplicate, a personal receipt, spam.
    Ignored,
}

/// A stored source document. The engine never interprets the bytes; it stores
/// them, hashes them for dedup, and hands them to whoever can read them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    pub id: DocumentId,
    /// Content hash. The same receipt arriving by email and by photo collapses
    /// to one row only if the bytes match, so this is a cheap first pass —
    /// perceptual matching happens later against the extracted fields.
    pub sha256: String,
    pub source: DocumentSource,
    pub mime: String,
    pub byte_len: u64,
    /// Path relative to the data directory. Absolute paths would break the
    /// "back up the folder and it still works" property.
    pub stored_path: String,
    pub original_filename: Option<String>,
    pub received_at: DateTime<Utc>,
    pub status: DocumentStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineItem {
    pub description: String,
    pub quantity: Option<String>,
    pub amount: Money,
}

/// The structured reading of a document. Produced outside the engine — by an
/// agent looking at the image — and handed back here to be checked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedInvoice {
    pub supplier_name: Option<String>,
    pub supplier_gst_number: Option<String>,
    pub invoice_number: Option<String>,
    pub invoice_date: Option<NaiveDate>,
    pub currency: Currency,
    /// Tax-exclusive amount, when the document states one.
    pub subtotal: Option<Money>,
    pub gst: Option<Money>,
    pub total: Money,
    pub lines: Vec<LineItem>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Blocks automatic posting.
    Error,
    /// Posted, but flagged for the reviewer.
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub severity: Severity,
    pub code: String,
    pub message: String,
}

impl ValidationIssue {
    fn error(code: &str, message: impl Into<String>) -> Self {
        ValidationIssue {
            severity: Severity::Error,
            code: code.to_string(),
            message: message.into(),
        }
    }

    fn warning(code: &str, message: impl Into<String>) -> Self {
        ValidationIssue {
            severity: Severity::Warning,
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// How far the stated GST may drift from the computed figure before it counts
/// as wrong. Per-line rounding on a long invoice legitimately accumulates a
/// few cents; anything larger is a misreading.
const GST_TOLERANCE_CENTS: i64 = 5;

impl ExtractedInvoice {
    /// Recompute everything the document claims and report the disagreements.
    ///
    /// This is the whole reason extraction lives outside the engine: the model
    /// reports fields, and arithmetic decides whether to believe them.
    pub fn validate(&self, rate: GstRate, today: NaiveDate) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();

        if self.total.currency != self.currency {
            issues.push(ValidationIssue::error(
                "currency_mismatch",
                format!(
                    "total is in {} but the invoice is stated in {}",
                    self.total.currency, self.currency
                ),
            ));
        }

        if self.total.is_zero() {
            issues.push(ValidationIssue::error(
                "zero_total",
                "invoice total is zero",
            ));
        }

        if !self.lines.is_empty() {
            match Money::sum(
                &self.lines.iter().map(|l| l.amount).collect::<Vec<_>>(),
                self.currency,
            ) {
                Ok(line_sum) => {
                    let reference = self.subtotal.unwrap_or(self.total);
                    let label = if self.subtotal.is_some() {
                        "subtotal"
                    } else {
                        "total"
                    };
                    if let Ok(diff) = line_sum.sub(reference)
                        && diff.cents.abs() > GST_TOLERANCE_CENTS
                    {
                        issues.push(ValidationIssue::error(
                            "lines_do_not_sum",
                            format!("line items sum to {line_sum} but the {label} is {reference}"),
                        ));
                    }
                }
                Err(e) => issues.push(ValidationIssue::error(
                    "line_currency_mismatch",
                    e.to_string(),
                )),
            }
        }

        if let (Some(subtotal), Some(gst)) = (self.subtotal, self.gst)
            && let Ok(sum) = subtotal.add(gst)
            && let Ok(diff) = sum.sub(self.total)
            && diff.cents != 0
        {
            let total = self.total;
            issues.push(ValidationIssue::error(
                "parts_do_not_sum",
                format!("{subtotal} + {gst} = {sum}, but the stated total is {total}"),
            ));
        }

        if let Some(stated_gst) = self.gst
            && !stated_gst.is_zero()
            && let Ok(computed) = rate.extract_from_inclusive(self.total, Rounding::HalfUp)
            && let Ok(diff) = stated_gst.sub(computed)
            && diff.cents.abs() > GST_TOLERANCE_CENTS
        {
            issues.push(ValidationIssue::warning(
                "gst_off_rate",
                format!(
                    "stated GST {stated_gst} differs from {computed} computed at the standard rate \
                     — the supply may be partly zero-rated or exempt"
                ),
            ));
        }

        match self.invoice_date {
            None => issues.push(ValidationIssue::error(
                "missing_date",
                "no invoice date could be read",
            )),
            Some(date) => {
                if date > today {
                    issues.push(ValidationIssue::error(
                        "future_date",
                        format!("invoice date {date} is in the future"),
                    ));
                } else if (today - date).num_days() > 7 * 365 {
                    issues.push(ValidationIssue::warning(
                        "very_old_date",
                        format!("invoice date {date} is beyond the 7-year retention window"),
                    ));
                }
            }
        }

        if self
            .supplier_name
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            issues.push(ValidationIssue::error(
                "missing_supplier",
                "no supplier name could be read",
            ));
        }

        if self.gst.is_some_and(|g| !g.is_zero()) && self.supplier_gst_number.is_none() {
            issues.push(ValidationIssue::warning(
                "gst_without_supplier_number",
                "GST is claimed but no supplier GST number was found — an input credit needs \
                 taxable supply information",
            ));
        }

        issues
    }
}

/// One agent's reading of one document. Immutable: re-reading a document adds a
/// new version rather than overwriting, so changing model or prompt is a
/// comparison rather than a loss.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Extraction {
    pub id: ExtractionId,
    pub document_id: DocumentId,
    pub version: u32,
    /// The model, or "human" when someone typed the numbers in.
    pub extracted_by: String,
    pub extracted_at: DateTime<Utc>,
    pub payload: ExtractedInvoice,
    /// The agent's own confidence. Advisory only — `issues` decides.
    pub reported_confidence: Option<f32>,
    pub issues: Vec<ValidationIssue>,
    pub superseded: bool,
}

impl Extraction {
    pub fn new(
        document_id: DocumentId,
        version: u32,
        extracted_by: impl Into<String>,
        payload: ExtractedInvoice,
        rate: GstRate,
        today: NaiveDate,
    ) -> Self {
        let issues = payload.validate(rate, today);
        Extraction {
            id: ExtractionId::new(),
            document_id,
            version,
            extracted_by: extracted_by.into(),
            extracted_at: Utc::now(),
            payload,
            reported_confidence: None,
            issues,
            superseded: false,
        }
    }

    pub fn with_reported_confidence(mut self, confidence: f32) -> Self {
        self.reported_confidence = Some(confidence);
        self
    }

    pub fn has_errors(&self) -> bool {
        self.issues.iter().any(|i| i.severity == Severity::Error)
    }

    /// A stated confidence can only lower the outcome, never raise it: an
    /// extraction that fails arithmetic goes to review however sure the model is.
    pub fn resulting_status(&self, confidence_floor: f32) -> DocumentStatus {
        if self.has_errors() {
            return DocumentStatus::NeedsReview;
        }
        match self.reported_confidence {
            Some(c) if c < confidence_floor => DocumentStatus::NeedsReview,
            _ => DocumentStatus::Extracted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NZ_15: GstRate = GstRate::new(15, 100);

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 31).unwrap()
    }

    fn clean_invoice() -> ExtractedInvoice {
        ExtractedInvoice {
            supplier_name: Some("Officeworks".into()),
            supplier_gst_number: Some("123-456-789".into()),
            invoice_number: Some("INV-8842".into()),
            invoice_date: Some(NaiveDate::from_ymd_opt(2026, 5, 2).unwrap()),
            currency: Currency::NZD,
            subtotal: Some(Money::nzd(10000)),
            gst: Some(Money::nzd(1500)),
            total: Money::nzd(11500),
            lines: vec![LineItem {
                description: "A4 paper".into(),
                quantity: Some("2".into()),
                amount: Money::nzd(10000),
            }],
        }
    }

    #[test]
    fn a_consistent_invoice_raises_nothing() {
        assert!(clean_invoice().validate(NZ_15, today()).is_empty());
    }

    #[test]
    fn parts_that_do_not_sum_are_an_error() {
        let mut invoice = clean_invoice();
        invoice.total = Money::nzd(12500);
        invoice.lines.clear();
        let issues = invoice.validate(NZ_15, today());
        assert!(issues.iter().any(|i| i.code == "parts_do_not_sum"));
        assert!(issues.iter().any(|i| i.severity == Severity::Error));
    }

    #[test]
    fn misread_line_items_are_caught() {
        let mut invoice = clean_invoice();
        invoice.lines[0].amount = Money::nzd(1000);
        let issues = invoice.validate(NZ_15, today());
        assert!(issues.iter().any(|i| i.code == "lines_do_not_sum"));
    }

    #[test]
    fn a_zero_rated_supply_warns_rather_than_fails() {
        let mut invoice = clean_invoice();
        invoice.subtotal = Some(Money::nzd(11000));
        invoice.gst = Some(Money::nzd(500));
        invoice.lines[0].amount = Money::nzd(11000);
        let issues = invoice.validate(NZ_15, today());
        assert!(issues.iter().any(|i| i.code == "gst_off_rate"));
        assert!(issues.iter().all(|i| i.severity == Severity::Warning));
    }

    #[test]
    fn a_future_date_is_an_error() {
        let mut invoice = clean_invoice();
        invoice.invoice_date = Some(NaiveDate::from_ymd_opt(2027, 1, 1).unwrap());
        let issues = invoice.validate(NZ_15, today());
        assert!(issues.iter().any(|i| i.code == "future_date"));
    }

    #[test]
    fn arithmetic_overrides_a_confident_model() {
        let mut invoice = clean_invoice();
        invoice.total = Money::nzd(99900);
        invoice.lines.clear();
        let extraction =
            Extraction::new(DocumentId::new(), 1, "some-model", invoice, NZ_15, today())
                .with_reported_confidence(0.99);

        assert!(extraction.has_errors());
        assert_eq!(
            extraction.resulting_status(0.8),
            DocumentStatus::NeedsReview
        );
    }

    #[test]
    fn a_clean_but_unsure_extraction_still_goes_to_review() {
        let extraction = Extraction::new(
            DocumentId::new(),
            1,
            "some-model",
            clean_invoice(),
            NZ_15,
            today(),
        )
        .with_reported_confidence(0.4);

        assert!(!extraction.has_errors());
        assert_eq!(
            extraction.resulting_status(0.8),
            DocumentStatus::NeedsReview
        );
    }
}
