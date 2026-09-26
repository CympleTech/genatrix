use std::fmt;
use std::str::FromStr;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::error::{Result, TaxError};

/// A New Zealand income tax year, identified by the calendar year it ends in:
/// `TaxYear(2026)` runs 1 April 2025 to 31 March 2026 and is written "2025-26".
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaxYear(pub i32);

impl TaxYear {
    pub fn containing(date: NaiveDate) -> TaxYear {
        if date.month() >= 4 {
            TaxYear(date.year() + 1)
        } else {
            TaxYear(date.year())
        }
    }

    pub fn start(&self) -> NaiveDate {
        NaiveDate::from_ymd_opt(self.0 - 1, 4, 1).expect("1 April is always valid")
    }

    pub fn end(&self) -> NaiveDate {
        NaiveDate::from_ymd_opt(self.0, 3, 31).expect("31 March is always valid")
    }

    pub fn contains(&self, date: NaiveDate) -> bool {
        date >= self.start() && date <= self.end()
    }

    pub fn label(&self) -> String {
        format!("{}-{:02}", self.0 - 1, self.0 % 100)
    }

    pub fn previous(&self) -> TaxYear {
        TaxYear(self.0 - 1)
    }

    pub fn next(&self) -> TaxYear {
        TaxYear(self.0 + 1)
    }
}

impl FromStr for TaxYear {
    type Err = TaxError;

    /// Accepts either "2025-26" or the bare ending year "2026".
    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        if let Some((first, second)) = s.split_once('-') {
            let start: i32 = first
                .parse()
                .map_err(|_| TaxError::BadTaxYear(s.to_string()))?;
            let suffix: i32 = second
                .parse()
                .map_err(|_| TaxError::BadTaxYear(s.to_string()))?;
            let end = start + 1;
            if second.len() != 2 || end % 100 != suffix {
                return Err(TaxError::BadTaxYear(s.to_string()));
            }
            Ok(TaxYear(end))
        } else {
            s.parse()
                .map(TaxYear)
                .map_err(|_| TaxError::BadTaxYear(s.to_string()))
        }
    }
}

impl fmt::Display for TaxYear {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

impl fmt::Debug for TaxYear {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TaxYear({})", self.label())
    }
}

/// How often GST is filed, expressed as a period length plus the month a period
/// ends in. IRD's filing "categories" are just named instances of this — keeping
/// it parameterised means a category reshuffle is a config change, not a patch.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct GstFrequency {
    pub months_per_period: u32,
    /// Any month a period ends in; the rest follow by stepping backwards.
    pub anchor_end_month: u32,
}

impl GstFrequency {
    pub fn new(months_per_period: u32, anchor_end_month: u32) -> Result<Self> {
        if months_per_period == 0
            || 12 % months_per_period != 0
            || !(1..=12).contains(&anchor_end_month)
        {
            return Err(TaxError::BadTaxYear(format!(
                "{months_per_period}-month periods ending in month {anchor_end_month}"
            )));
        }
        Ok(GstFrequency {
            months_per_period,
            anchor_end_month,
        })
    }

    pub fn monthly() -> Self {
        GstFrequency {
            months_per_period: 1,
            anchor_end_month: 3,
        }
    }

    /// The common default for small businesses: two-monthly periods ending in
    /// odd months, so the final period closes with the 31 March tax year end.
    pub fn two_monthly_ending_march() -> Self {
        GstFrequency {
            months_per_period: 2,
            anchor_end_month: 3,
        }
    }

    pub fn six_monthly_ending_march() -> Self {
        GstFrequency {
            months_per_period: 6,
            anchor_end_month: 3,
        }
    }

    pub fn period_containing(&self, date: NaiveDate) -> GstPeriod {
        let months_from_anchor = (date.year() * 12 + date.month() as i32 - 1)
            - (date.year() * 12 + self.anchor_end_month as i32 - 1);
        let step = self.months_per_period as i32;
        // Whole periods from the anchor to the first period end on or after
        // this month. Rounding down picked the end before the date, so the
        // period did not contain it (and `periods_overlapping` never moved on).
        let offset = (months_from_anchor + step - 1).div_euclid(step);
        let end_month_index = (date.year() * 12 + self.anchor_end_month as i32 - 1) + offset * step;
        let end = last_day_of(month_index_to_ym(end_month_index));
        let start = first_day_of(month_index_to_ym(end_month_index - (step - 1)));
        GstPeriod { start, end }
    }

    /// Every period that overlaps the tax year, in chronological order. With an
    /// off-cycle anchor the first and last may straddle the year boundary.
    pub fn periods_overlapping(&self, year: TaxYear) -> Vec<GstPeriod> {
        let mut periods = Vec::new();
        let mut current = self.period_containing(year.start());
        while current.start <= year.end() {
            let next = self.period_containing(
                current
                    .end
                    .succ_opt()
                    .expect("tax dates are far from the calendar limits"),
            );
            periods.push(current);
            current = next;
        }
        periods
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct GstPeriod {
    pub start: NaiveDate,
    pub end: NaiveDate,
}

impl GstPeriod {
    pub fn contains(&self, date: NaiveDate) -> bool {
        date >= self.start && date <= self.end
    }

    pub fn label(&self) -> String {
        format!("{} to {}", self.start, self.end)
    }
}

impl fmt::Display for GstPeriod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

fn month_index_to_ym(index: i32) -> (i32, u32) {
    (index.div_euclid(12), index.rem_euclid(12) as u32 + 1)
}

fn first_day_of((year, month): (i32, u32)) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, 1).expect("month index is normalised")
}

fn last_day_of((year, month): (i32, u32)) -> NaiveDate {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .expect("month index is normalised")
        .pred_opt()
        .expect("tax dates are far from the calendar limits")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn tax_year_boundary_is_1_april() {
        assert_eq!(TaxYear::containing(d(2025, 3, 31)), TaxYear(2025));
        assert_eq!(TaxYear::containing(d(2025, 4, 1)), TaxYear(2026));
    }

    #[test]
    fn tax_year_label_matches_ird_convention() {
        assert_eq!(TaxYear(2026).label(), "2025-26");
        assert_eq!(TaxYear(2030).label(), "2029-30");
        assert_eq!(TaxYear(2100).label(), "2099-00");
    }

    #[test]
    fn tax_year_parses_both_written_forms() {
        assert_eq!("2025-26".parse::<TaxYear>().unwrap(), TaxYear(2026));
        assert_eq!("2026".parse::<TaxYear>().unwrap(), TaxYear(2026));
        assert!("2025-27".parse::<TaxYear>().is_err());
    }

    #[test]
    fn two_monthly_periods_align_with_the_tax_year() {
        let freq = GstFrequency::two_monthly_ending_march();
        let periods = freq.periods_overlapping(TaxYear(2026));
        assert_eq!(periods.len(), 6);
        assert_eq!(periods[0].start, d(2025, 4, 1));
        assert_eq!(periods[0].end, d(2025, 5, 31));
        assert_eq!(periods[5].start, d(2026, 2, 1));
        assert_eq!(periods[5].end, d(2026, 3, 31));
    }

    #[test]
    fn a_day_after_a_period_end_belongs_to_the_next_period() {
        // Rounding the wrong way put 1 April in the February-March period.
        let freq = GstFrequency::two_monthly_ending_march();
        let p = freq.period_containing(d(2026, 4, 1));
        assert_eq!((p.start, p.end), (d(2026, 4, 1), d(2026, 5, 31)));
        let p = freq.period_containing(d(2026, 9, 26));
        assert_eq!((p.start, p.end), (d(2026, 8, 1), d(2026, 9, 30)));
        let p = freq.period_containing(d(2027, 1, 15));
        assert_eq!((p.start, p.end), (d(2026, 12, 1), d(2027, 1, 31)));
    }

    #[test]
    fn off_cycle_periods_straddle_the_year_boundary() {
        let freq = GstFrequency::new(2, 4).unwrap();
        let periods = freq.periods_overlapping(TaxYear(2026));
        assert_eq!(periods[0].start, d(2025, 3, 1));
        assert!(periods.last().unwrap().end > TaxYear(2026).end());
    }

    #[test]
    fn period_containing_covers_every_day_of_the_year() {
        let freq = GstFrequency::two_monthly_ending_march();
        let mut day = TaxYear(2026).start();
        while day <= TaxYear(2026).end() {
            assert!(
                freq.period_containing(day).contains(day),
                "{day} not covered"
            );
            day = day.succ_opt().unwrap();
        }
    }

    #[test]
    fn monthly_periods_are_calendar_months() {
        let freq = GstFrequency::monthly();
        let p = freq.period_containing(d(2025, 11, 17));
        assert_eq!(p.start, d(2025, 11, 1));
        assert_eq!(p.end, d(2025, 11, 30));
    }
}
