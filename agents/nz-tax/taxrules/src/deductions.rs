use std::collections::BTreeMap;

use serde::Deserialize;
use taxcore::{Money, Rounding};

use crate::{Ratio, Result, invalid};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deductions {
    pub entertainment: Entertainment,
    pub home_office: HomeOffice,
    pub motor_vehicle: MotorVehicle,
}

impl Deductions {
    pub(crate) fn validate(&self) -> Result<()> {
        if !self.entertainment.deductible.is_proper_fraction() {
            return Err(invalid(
                "deductions.entertainment.deductible must be strictly between 0 and 1",
            ));
        }
        if self.home_office.square_metre_rate_nzd <= 0.0 {
            return Err(invalid("deductions.home_office rate must be positive"));
        }
        if self.motor_vehicle.kilometre_rates_nzd.is_empty() {
            return Err(invalid("deductions.motor_vehicle has no kilometre rates"));
        }
        for (vehicle, rates) in &self.motor_vehicle.kilometre_rates_nzd {
            if rates.tier_one <= 0.0 || rates.tier_two <= 0.0 {
                return Err(invalid(format!(
                    "deductions.motor_vehicle rate for {vehicle} must be positive"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entertainment {
    pub deductible: Ratio,
}

impl Entertainment {
    /// The deductible share of an entertainment expense, outside the statutory
    /// exceptions. Whether an expense is an exception is a review decision,
    /// never decided here.
    pub fn deductible_portion(&self, amount: Money) -> Result<Money> {
        self.deductible.apply(amount, Rounding::HalfUp)
    }
}

/// Rates published to the cent are stored in YAML as decimal dollars and read
/// to the cent once, on use — never carried as floats into arithmetic.
fn cents(rate_nzd: f64) -> i64 {
    (rate_nzd * 100.0).round() as i64
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeOffice {
    pub method: String,
    pub square_metre_rate_nzd: f64,
    #[serde(default)]
    pub square_metre_rate_note: Option<String>,
}

impl HomeOffice {
    pub fn square_metre_rate(&self) -> Money {
        Money::nzd(cents(self.square_metre_rate_nzd))
    }

    /// The square-metre-rate claim for a measured office area. Covers utilities
    /// only; premises costs are claimed separately on the floor-area share.
    pub fn square_metre_claim(&self, office_square_metres: u32) -> Result<Money> {
        let rate = self.square_metre_rate();
        let claim = rate
            .cents
            .checked_mul(office_square_metres as i64)
            .ok_or(taxcore::TaxError::AmountOverflow)?;
        Ok(Money::nzd(claim))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotorVehicle {
    pub method: String,
    pub tier_one_limit_km: u32,
    pub kilometre_rates_nzd: BTreeMap<String, VehicleRates>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub needs_verification: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VehicleRates {
    pub tier_one: f64,
    pub tier_two: f64,
}

impl MotorVehicle {
    /// The kilometre-rate claim. Tier 1 covers the first `tier_one_limit_km`
    /// of *total* annual travel; the business share (from a logbook) is then
    /// applied to the whole figure.
    pub fn kilometre_claim(
        &self,
        vehicle: &str,
        total_annual_km: u32,
        business_share: Ratio,
    ) -> Result<Money> {
        let rates = self.kilometre_rates_nzd.get(vehicle).ok_or_else(|| {
            invalid(format!(
                "no kilometre rate for vehicle type {vehicle:?}; known types: {}",
                self.kilometre_rates_nzd
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        if !business_share.is_proper_fraction()
            && business_share.numerator != business_share.denominator
        {
            return Err(invalid("business share must be between 0 and 1"));
        }
        let tier_one_km = total_annual_km.min(self.tier_one_limit_km) as i64;
        let tier_two_km = total_annual_km.saturating_sub(self.tier_one_limit_km) as i64;
        let full =
            Money::nzd(tier_one_km * cents(rates.tier_one) + tier_two_km * cents(rates.tier_two));
        business_share.apply(full, Rounding::HalfUp)
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

    #[test]
    fn entertainment_is_half_deductible() {
        let portion = rules()
            .deductions
            .entertainment
            .deductible_portion(Money::nzd(10_001))
            .unwrap();
        assert_eq!(portion, Money::nzd(5_001)); // 100.01 -> 50.005 -> 50.01 half-up
    }

    #[test]
    fn the_square_metre_rate_reads_to_the_cent() {
        let deductions = &rules().deductions;
        assert_eq!(
            deductions.home_office.square_metre_rate(),
            Money::nzd(6_115)
        );
        assert_eq!(
            deductions.home_office.square_metre_claim(10).unwrap(),
            Money::nzd(61_150)
        );
    }

    #[test]
    fn kilometre_claim_splits_across_the_tiers() {
        // 20,000 km petrol: 14,000 @ 1.17 + 6,000 @ 0.37 = 16,380 + 2,220 = 18,600.
        // At 100% business use.
        let claim = rules()
            .deductions
            .motor_vehicle
            .kilometre_claim(
                "petrol_or_diesel",
                20_000,
                Ratio {
                    numerator: 1,
                    denominator: 1,
                },
            )
            .unwrap();
        assert_eq!(claim, Money::nzd(1_860_000));
    }

    #[test]
    fn kilometre_claim_applies_the_business_share_last() {
        // Same travel at 25% business use: 18,600 / 4 = 4,650.
        let claim = rules()
            .deductions
            .motor_vehicle
            .kilometre_claim(
                "petrol_or_diesel",
                20_000,
                Ratio {
                    numerator: 1,
                    denominator: 4,
                },
            )
            .unwrap();
        assert_eq!(claim, Money::nzd(465_000));
    }

    #[test]
    fn an_unknown_vehicle_type_is_an_error_not_a_default() {
        assert!(
            rules()
                .deductions
                .motor_vehicle
                .kilometre_claim(
                    "hovercraft",
                    1_000,
                    Ratio {
                        numerator: 1,
                        denominator: 1
                    }
                )
                .is_err()
        );
    }
}
