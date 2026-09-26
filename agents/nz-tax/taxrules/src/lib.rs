//! Typed access to the rule files under `rules/`.
//!
//! Rules are data, not code: a Budget that changes a rate adds a new file, and
//! this crate is the only place that reads them. Every file is validated in
//! full on load — a rule file that fails validation is unusable, never
//! partially usable, because a silently-skipped section would change a return.

mod deductions;
mod gst;
mod income;

pub use deductions::{Deductions, Entertainment, HomeOffice, MotorVehicle, VehicleRates};
pub use gst::{DueDateException, DueDateRule, FilingFrequency, GstRules};
pub use income::{
    BandTax, Bracket, IncomeTaxBreakdown, IncomeTaxRules, MonthDay, ProvisionalTax, ReturnDue,
    StandardOption,
};

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use taxcore::{Currency, GstRate, Money, Rounding, TaxYear};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, RulesError>;

#[derive(Debug, Error)]
pub enum RulesError {
    #[error("cannot read rule file: {0}")]
    Io(#[from] std::io::Error),

    #[error("cannot parse rule file: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("rule file is invalid: {0}")]
    Invalid(String),

    #[error(transparent)]
    Tax(#[from] taxcore::TaxError),

    #[error("no rule file for {jurisdiction} {year} under {dir}")]
    MissingYear {
        jurisdiction: String,
        year: String,
        dir: String,
    },
}

pub(crate) fn invalid(msg: impl Into<String>) -> RulesError {
    RulesError::Invalid(msg.into())
}

/// An exact rational, the only form a rate is ever written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ratio {
    pub numerator: i64,
    pub denominator: i64,
}

impl Ratio {
    pub fn apply(&self, amount: Money, rounding: Rounding) -> Result<Money> {
        Ok(amount.mul_ratio(self.numerator, self.denominator, rounding)?)
    }

    /// Strictly between zero and one — what a rate or a deductible share must be.
    pub fn is_proper_fraction(&self) -> bool {
        self.denominator > 0 && self.numerator > 0 && self.numerator < self.denominator
    }

    /// At least one — what an uplift must be.
    pub fn is_uplift(&self) -> bool {
        self.denominator > 0 && self.numerator >= self.denominator
    }
}

/// Whole NZD from a rule file into cents. Thresholds and brackets are written
/// in dollars because that is how legislation states them.
pub(crate) fn nzd_dollars(dollars: i64) -> Result<Money> {
    let cents = dollars
        .checked_mul(100)
        .ok_or(taxcore::TaxError::AmountOverflow)?;
    Ok(Money::nzd(cents))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub jurisdiction: String,
    pub tax_year: String,
    /// Bumped when the file is corrected; recorded on every computed return.
    pub version: u32,
    pub verified_on: NaiveDate,
    pub sources: Vec<String>,
    /// Set when the file is a copy of an earlier year's, not yet checked
    /// against the source: every figure computed under it says so.
    #[serde(default)]
    pub copied_from: Option<String>,
}

impl Meta {
    pub fn tax_year(&self) -> Result<TaxYear> {
        Ok(self.tax_year.parse()?)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSet {
    pub meta: Meta,
    pub gst: GstRules,
    pub income_tax: IncomeTaxRules,
    pub provisional_tax: ProvisionalTax,
    pub deductions: Deductions,
    /// Commonly misapplied treatments, used to seed account defaults and to
    /// warn during review — never to decide silently.
    #[serde(default)]
    pub gst_treatment_hints: BTreeMap<String, Vec<String>>,
}

impl RuleSet {
    pub fn from_yaml(text: &str) -> Result<Self> {
        let rules: RuleSet = serde_yaml::from_str(text)?;
        rules.validate()?;
        Ok(rules)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_yaml(&fs::read_to_string(path)?)
    }

    /// Load `{rules_dir}/{jurisdiction}/{year label}.yaml` and check the file
    /// agrees about what it is.
    pub fn for_year(rules_dir: &Path, jurisdiction: &str, year: TaxYear) -> Result<Self> {
        let path = rules_dir
            .join(jurisdiction.to_lowercase())
            .join(format!("{}.yaml", year.label()));
        if !path.exists() {
            return Err(RulesError::MissingYear {
                jurisdiction: jurisdiction.to_string(),
                year: year.label(),
                dir: rules_dir.display().to_string(),
            });
        }
        let rules = Self::load(&path)?;
        if rules.meta.tax_year()? != year {
            return Err(invalid(format!(
                "file {} declares tax year {}, expected {}",
                path.display(),
                rules.meta.tax_year,
                year.label()
            )));
        }
        if !rules.meta.jurisdiction.eq_ignore_ascii_case(jurisdiction) {
            return Err(invalid(format!(
                "file {} declares jurisdiction {}, expected {}",
                path.display(),
                rules.meta.jurisdiction,
                jurisdiction
            )));
        }
        Ok(rules)
    }

    pub fn gst_rate(&self) -> GstRate {
        GstRate::new(self.gst.rate.numerator, self.gst.rate.denominator)
    }

    fn validate(&self) -> Result<()> {
        if self.meta.jurisdiction.trim().is_empty() {
            return Err(invalid("meta.jurisdiction is empty"));
        }
        if self.meta.version == 0 {
            return Err(invalid("meta.version must be at least 1"));
        }
        self.meta.tax_year()?;
        self.gst.validate()?;
        self.income_tax.validate()?;
        self.provisional_tax.validate()?;
        self.deductions.validate()?;
        Ok(())
    }
}

pub const NZD: Currency = Currency::NZD;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn rules_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rules")
    }

    #[test]
    fn the_current_year_loads_and_says_it_is_unverified() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rules");
        let rules = RuleSet::for_year(&dir, "NZ", TaxYear(2027)).unwrap();
        assert_eq!(rules.meta.copied_from.as_deref(), Some("2025-26"));
        let verified = RuleSet::for_year(&dir, "NZ", TaxYear(2026)).unwrap();
        assert_eq!(verified.meta.copied_from, None);
    }

    #[test]
    fn the_shipped_nz_file_loads_and_validates() {
        let rules = RuleSet::for_year(&rules_dir(), "NZ", TaxYear(2026)).unwrap();
        assert_eq!(rules.meta.version, 1);
        assert_eq!(rules.gst_rate(), GstRate::new(15, 100));
        assert!(!rules.gst_treatment_hints.is_empty());
    }

    #[test]
    fn a_missing_year_is_reported_not_guessed() {
        let err = RuleSet::for_year(&rules_dir(), "NZ", TaxYear(1999)).unwrap_err();
        assert!(matches!(err, RulesError::MissingYear { .. }));
    }

    #[test]
    fn an_unknown_field_is_a_parse_error() {
        let rules = RuleSet::for_year(&rules_dir(), "NZ", TaxYear(2026)).unwrap();
        drop(rules);
        let text = fs::read_to_string(rules_dir().join("nz/2025-26.yaml")).unwrap();
        let with_typo = text.replace("registration_threshold_nzd", "registration_treshold_nzd");
        assert!(RuleSet::from_yaml(&with_typo).is_err());
    }
}
