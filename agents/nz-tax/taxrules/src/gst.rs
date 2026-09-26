use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use taxcore::{GstFrequency, Money};

use crate::{Ratio, Result, invalid, nzd_dollars};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GstRules {
    pub rate: Ratio,
    pub registration_threshold_nzd: i64,
    pub registration_window_days: u32,
    pub filing_frequencies: Vec<FilingFrequency>,
    pub due_date: DueDateRule,
}

impl GstRules {
    pub fn registration_threshold(&self) -> Result<Money> {
        nzd_dollars(self.registration_threshold_nzd)
    }

    pub fn default_frequency(&self) -> &FilingFrequency {
        self.filing_frequencies
            .iter()
            .find(|f| f.is_default)
            .expect("validated: exactly one default frequency")
    }

    pub fn frequency(&self, id: &str) -> Option<&FilingFrequency> {
        self.filing_frequencies.iter().find(|f| f.id == id)
    }

    pub fn due_date(&self, period_end: NaiveDate) -> NaiveDate {
        self.due_date.for_period_end(period_end)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if !self.rate.is_proper_fraction() {
            return Err(invalid("gst.rate must be strictly between 0 and 1"));
        }
        let defaults = self
            .filing_frequencies
            .iter()
            .filter(|f| f.is_default)
            .count();
        if defaults != 1 {
            return Err(invalid(format!(
                "gst.filing_frequencies must have exactly one default, found {defaults}"
            )));
        }
        for freq in &self.filing_frequencies {
            freq.frequency()?;
        }
        self.due_date.validate()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilingFrequency {
    pub id: String,
    pub months_per_period: u32,
    pub anchor_end_month: u32,
    #[serde(default, rename = "default")]
    pub is_default: bool,
    /// Compulsory above this turnover.
    #[serde(default)]
    pub required_above_nzd: Option<i64>,
    /// Only available below this turnover.
    #[serde(default)]
    pub available_below_nzd: Option<i64>,
}

impl FilingFrequency {
    pub fn frequency(&self) -> Result<GstFrequency> {
        Ok(GstFrequency::new(
            self.months_per_period,
            self.anchor_end_month,
        )?)
    }
}

/// Returns are due on a fixed day some months after the period ends, with a
/// short list of exceptions. The exceptions are data because they exist for
/// human reasons (the Christmas and year-end crush) and could change.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DueDateRule {
    pub months_after_period_end: u32,
    pub day: u32,
    #[serde(default)]
    pub exceptions: Vec<DueDateException>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DueDateException {
    pub when_period_ends_in_month: u32,
    pub due_month: u32,
    pub due_day: u32,
    pub due_year_offset: i32,
}

impl DueDateRule {
    pub fn for_period_end(&self, period_end: NaiveDate) -> NaiveDate {
        for ex in &self.exceptions {
            if ex.when_period_ends_in_month == period_end.month() {
                return NaiveDate::from_ymd_opt(
                    period_end.year() + ex.due_year_offset,
                    ex.due_month,
                    ex.due_day,
                )
                .expect("validated: due day is at most 28");
            }
        }
        let month_index = period_end.year() * 12 + period_end.month() as i32 - 1
            + self.months_after_period_end as i32;
        NaiveDate::from_ymd_opt(
            month_index.div_euclid(12),
            month_index.rem_euclid(12) as u32 + 1,
            self.day,
        )
        .expect("validated: due day is at most 28")
    }

    fn validate(&self) -> Result<()> {
        // Capped at 28 so the due date exists in every month.
        if !(1..=28).contains(&self.day) {
            return Err(invalid("gst.due_date.day must be between 1 and 28"));
        }
        for ex in &self.exceptions {
            if !(1..=12).contains(&ex.when_period_ends_in_month)
                || !(1..=12).contains(&ex.due_month)
                || !(1..=28).contains(&ex.due_day)
            {
                return Err(invalid("gst.due_date exception has an impossible date"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuleSet;
    use std::path::PathBuf;
    use taxcore::TaxYear;

    fn rules() -> RuleSet {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rules");
        RuleSet::for_year(&dir, "NZ", TaxYear(2026)).unwrap()
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn ordinary_periods_are_due_on_the_28th_of_the_next_month() {
        assert_eq!(rules().gst.due_date(d(2025, 5, 31)), d(2025, 6, 28));
        assert_eq!(rules().gst.due_date(d(2025, 7, 31)), d(2025, 8, 28));
    }

    #[test]
    fn a_december_period_end_crosses_the_year_boundary() {
        assert_eq!(rules().gst.due_date(d(2025, 12, 31)), d(2026, 1, 28));
    }

    #[test]
    fn the_march_period_gets_the_year_end_extension() {
        assert_eq!(rules().gst.due_date(d(2026, 3, 31)), d(2026, 5, 7));
    }

    #[test]
    fn the_november_period_gets_the_christmas_extension() {
        assert_eq!(rules().gst.due_date(d(2025, 11, 30)), d(2026, 1, 15));
    }

    #[test]
    fn the_default_frequency_is_two_monthly() {
        let rules = rules();
        let freq = rules.gst.default_frequency();
        assert_eq!(freq.id, "two_monthly");
        assert_eq!(freq.frequency().unwrap().months_per_period, 2);
    }

    #[test]
    fn the_registration_threshold_is_sixty_thousand() {
        assert_eq!(
            rules().gst.registration_threshold().unwrap(),
            Money::nzd(6_000_000)
        );
    }
}
