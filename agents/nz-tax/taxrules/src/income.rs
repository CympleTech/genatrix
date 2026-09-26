use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use taxcore::{Currency, Money, Rounding, TaxYear};

use crate::{Ratio, Result, invalid, nzd_dollars};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncomeTaxRules {
    /// Applied progressively, lowest band first. The last bracket is open-ended.
    pub brackets: Vec<Bracket>,
    pub return_due: ReturnDue,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bracket {
    pub up_to_nzd: Option<i64>,
    pub rate: Ratio,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReturnDue {
    pub self_filed: MonthDay,
    pub with_tax_agent: MonthDay,
}

/// A date relative to the calendar year a tax year ends in. `year_offset` is 1
/// for dates that fall in the following calendar year, like an agent's
/// extension of time.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonthDay {
    pub month: u32,
    pub day: u32,
    #[serde(default)]
    pub year_offset: i32,
}

impl MonthDay {
    pub fn date_in(&self, year: TaxYear) -> NaiveDate {
        NaiveDate::from_ymd_opt(year.0 + self.year_offset, self.month, self.day)
            .expect("validated: day exists in this month in every year")
    }
}

/// One band's slice of a progressive tax computation. Kept so a total can be
/// shown band by band — a figure that cannot be broken down is not explained.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandTax {
    pub floor: Money,
    /// `None` on the open-ended top band.
    pub ceiling: Option<Money>,
    pub rate: Ratio,
    pub taxable_in_band: Money,
    pub tax: Money,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomeTaxBreakdown {
    pub taxable_income: Money,
    pub bands: Vec<BandTax>,
    pub total: Money,
}

impl IncomeTaxRules {
    /// Progressive tax on a taxable income. Each band is rounded half-up on
    /// its own, matching how the bands are stated dollar-for-dollar.
    /// Negative input is treated as zero taxable income.
    pub fn tax_on(&self, taxable: Money) -> Result<IncomeTaxBreakdown> {
        if taxable.currency != Currency::NZD {
            return Err(invalid(format!(
                "income tax is computed in NZD, got {}",
                taxable.currency
            )));
        }
        let taxable_cents = taxable.cents.max(0);

        let mut bands = Vec::new();
        let mut total = Money::zero(Currency::NZD);
        let mut floor_cents: i64 = 0;
        for bracket in &self.brackets {
            let ceiling_cents = match bracket.up_to_nzd {
                Some(dollars) => Some(nzd_dollars(dollars)?.cents),
                None => None,
            };
            let upper = ceiling_cents.unwrap_or(i64::MAX).min(taxable_cents);
            let in_band = (upper - floor_cents).max(0);
            if in_band > 0 {
                let taxable_in_band = Money::nzd(in_band);
                let tax = bracket.rate.apply(taxable_in_band, Rounding::HalfUp)?;
                total = total.add(tax)?;
                bands.push(BandTax {
                    floor: Money::nzd(floor_cents),
                    ceiling: ceiling_cents.map(Money::nzd),
                    rate: bracket.rate,
                    taxable_in_band,
                    tax,
                });
            }
            match ceiling_cents {
                Some(c) if c < taxable_cents => floor_cents = c,
                _ => break,
            }
        }

        Ok(IncomeTaxBreakdown {
            taxable_income: Money::nzd(taxable_cents),
            bands,
            total,
        })
    }

    pub fn self_filed_due(&self, year: TaxYear) -> NaiveDate {
        self.return_due.self_filed.date_in(year)
    }

    pub fn with_tax_agent_due(&self, year: TaxYear) -> NaiveDate {
        self.return_due.with_tax_agent.date_in(year)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.brackets.is_empty() {
            return Err(invalid("income_tax.brackets is empty"));
        }
        let mut previous: Option<i64> = None;
        for (i, bracket) in self.brackets.iter().enumerate() {
            let is_last = i == self.brackets.len() - 1;
            if !bracket.rate.is_proper_fraction() {
                return Err(invalid(format!(
                    "income_tax.brackets[{i}].rate must be strictly between 0 and 1"
                )));
            }
            match (bracket.up_to_nzd, is_last) {
                (None, false) => {
                    return Err(invalid(format!(
                        "income_tax.brackets[{i}] is open-ended but not last"
                    )));
                }
                (Some(_), true) => {
                    return Err(invalid("the last income tax bracket must be open-ended"));
                }
                (Some(up_to), false) => {
                    if up_to <= 0 || up_to > 1_000_000_000_000 {
                        return Err(invalid(format!(
                            "income_tax.brackets[{i}].up_to_nzd is out of range"
                        )));
                    }
                    if let Some(prev) = previous
                        && up_to <= prev
                    {
                        return Err(invalid(format!("income_tax.brackets[{i}] does not ascend")));
                    }
                    previous = Some(up_to);
                }
                (None, true) => {}
            }
        }
        for month_day in [self.return_due.self_filed, self.return_due.with_tax_agent] {
            // The month is fixed, so the day only has to exist in that month —
            // except February, capped at 28 so the date exists in every year.
            let max_day = match month_day.month {
                1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
                4 | 6 | 9 | 11 => 30,
                2 => 28,
                _ => return Err(invalid("income_tax.return_due has an impossible date")),
            };
            if !(1..=max_day).contains(&month_day.day) {
                return Err(invalid("income_tax.return_due has an impossible date"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisionalTax {
    pub residual_income_tax_threshold_nzd: i64,
    pub standard_option: StandardOption,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardOption {
    pub uplift_prior_year: Ratio,
    pub uplift_two_years_prior: Ratio,
}

impl ProvisionalTax {
    pub fn threshold(&self) -> Result<Money> {
        nzd_dollars(self.residual_income_tax_threshold_nzd)
    }

    /// Provisional tax applies once residual income tax passes the threshold.
    pub fn applies_to(&self, residual_income_tax: Money) -> Result<bool> {
        Ok(residual_income_tax.cents > self.threshold()?.cents)
    }

    /// The standard option: 105% of last year's RIT when that return exists,
    /// otherwise 110% of the year before.
    pub fn standard_uplift(&self, prior_rit: Money, years_back: u8) -> Result<Money> {
        let uplift = match years_back {
            1 => self.standard_option.uplift_prior_year,
            2 => self.standard_option.uplift_two_years_prior,
            n => return Err(invalid(format!("no uplift is defined for {n} years back"))),
        };
        uplift.apply(prior_rit, Rounding::HalfUp)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.residual_income_tax_threshold_nzd < 0 {
            return Err(invalid("provisional_tax threshold cannot be negative"));
        }
        if !self.standard_option.uplift_prior_year.is_uplift()
            || !self.standard_option.uplift_two_years_prior.is_uplift()
        {
            return Err(invalid("provisional_tax uplifts must be at least 1"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuleSet;
    use std::path::PathBuf;

    fn rules() -> RuleSet {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rules");
        RuleSet::for_year(&dir, "NZ", TaxYear(2026)).unwrap()
    }

    #[test]
    fn tax_on_sixty_thousand_matches_the_published_bands() {
        // 15,600 @ 10.5% + 37,900 @ 17.5% + 6,500 @ 30% = 1,638 + 6,632.50 + 1,950
        let breakdown = rules().income_tax.tax_on(Money::nzd(6_000_000)).unwrap();
        assert_eq!(breakdown.total, Money::nzd(1_022_050));
        assert_eq!(breakdown.bands.len(), 3);
        assert_eq!(breakdown.bands[0].tax, Money::nzd(163_800));
        assert_eq!(breakdown.bands[1].tax, Money::nzd(663_250));
        assert_eq!(breakdown.bands[2].tax, Money::nzd(195_000));
    }

    #[test]
    fn there_is_no_tax_free_threshold() {
        let breakdown = rules().income_tax.tax_on(Money::nzd(100)).unwrap();
        // 1.00 at 10.5% rounds to 0.11 (half up).
        assert_eq!(breakdown.total, Money::nzd(11));
    }

    #[test]
    fn income_in_the_top_band_uses_the_open_bracket() {
        let breakdown = rules().income_tax.tax_on(Money::nzd(20_000_000)).unwrap();
        assert_eq!(breakdown.bands.len(), 5);
        let top = breakdown.bands.last().unwrap();
        assert_eq!(top.ceiling, None);
        assert_eq!(top.taxable_in_band, Money::nzd(2_000_000));
        assert_eq!(top.tax, Money::nzd(780_000));
    }

    #[test]
    fn a_loss_owes_nothing() {
        let breakdown = rules().income_tax.tax_on(Money::nzd(-500_000)).unwrap();
        assert!(breakdown.total.is_zero());
        assert!(breakdown.bands.is_empty());
    }

    #[test]
    fn band_boundaries_are_exact() {
        // Exactly 15,600 stays entirely in the first band.
        let breakdown = rules().income_tax.tax_on(Money::nzd(1_560_000)).unwrap();
        assert_eq!(breakdown.bands.len(), 1);
        assert_eq!(breakdown.total, Money::nzd(163_800));
    }

    #[test]
    fn return_due_dates_follow_the_tax_year() {
        let rules = rules();
        let year = TaxYear(2026);
        assert_eq!(
            rules.income_tax.self_filed_due(year),
            NaiveDate::from_ymd_opt(2026, 7, 7).unwrap()
        );
        assert_eq!(
            rules.income_tax.with_tax_agent_due(year),
            NaiveDate::from_ymd_opt(2027, 3, 31).unwrap()
        );
    }

    #[test]
    fn provisional_tax_threshold_and_uplift() {
        let rules = rules();
        assert!(
            !rules
                .provisional_tax
                .applies_to(Money::nzd(500_000))
                .unwrap()
        );
        assert!(
            rules
                .provisional_tax
                .applies_to(Money::nzd(500_100))
                .unwrap()
        );
        assert_eq!(
            rules
                .provisional_tax
                .standard_uplift(Money::nzd(1_000_000), 1)
                .unwrap(),
            Money::nzd(1_050_000)
        );
        assert_eq!(
            rules
                .provisional_tax
                .standard_uplift(Money::nzd(1_000_000), 2)
                .unwrap(),
            Money::nzd(1_100_000)
        );
    }
}
