use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::money::{Money, Rounding};

/// How a line is treated for GST. The distinction between `ZeroRated` and
/// `Exempt` matters: zero-rated supplies still count toward total sales and
/// still allow input credits, exempt supplies do neither.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GstTreatment {
    /// Standard-rated supply, GST inclusive in the recorded amount.
    Standard,
    /// Taxable at 0% — exports, going-concern sales, most land transactions.
    ZeroRated,
    /// Outside the GST net — residential rent, financial services, donations.
    Exempt,
    /// Not a supply at all — drawings, capital contributions, wages, transfers.
    NotSubject,
}

impl GstTreatment {
    pub fn attracts_gst(&self) -> bool {
        matches!(self, GstTreatment::Standard)
    }

    /// Whether the amount belongs in the total sales / total purchases boxes of
    /// a GST return. Exempt and non-supply amounts are excluded.
    pub fn included_in_return_totals(&self) -> bool {
        matches!(self, GstTreatment::Standard | GstTreatment::ZeroRated)
    }
}

/// A GST rate as an exact rational, e.g. 15% is `15/100`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct GstRate {
    pub numerator: i64,
    pub denominator: i64,
}

impl GstRate {
    pub const fn new(numerator: i64, denominator: i64) -> Self {
        GstRate {
            numerator,
            denominator,
        }
    }

    /// The GST content of a tax-inclusive amount. At 15% this is the familiar
    /// 3/23 fraction, computed exactly rather than via 0.13043478.
    pub fn extract_from_inclusive(&self, inclusive: Money, rounding: Rounding) -> Result<Money> {
        inclusive.mul_ratio(self.numerator, self.denominator + self.numerator, rounding)
    }

    /// The GST to add on top of a tax-exclusive amount.
    pub fn add_to_exclusive(&self, exclusive: Money, rounding: Rounding) -> Result<Money> {
        exclusive.mul_ratio(self.numerator, self.denominator, rounding)
    }

    pub fn exclusive_part_of(&self, inclusive: Money, rounding: Rounding) -> Result<Money> {
        let gst = self.extract_from_inclusive(inclusive, rounding)?;
        inclusive.sub(gst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NZ_15: GstRate = GstRate::new(15, 100);

    #[test]
    fn extracts_gst_from_an_inclusive_price() {
        let gst = NZ_15
            .extract_from_inclusive(Money::nzd(11500), Rounding::HalfUp)
            .unwrap();
        assert_eq!(gst, Money::nzd(1500));
    }

    #[test]
    fn inclusive_splits_back_into_its_parts() {
        let incl = Money::nzd(4999);
        let gst = NZ_15
            .extract_from_inclusive(incl, Rounding::HalfUp)
            .unwrap();
        let excl = NZ_15.exclusive_part_of(incl, Rounding::HalfUp).unwrap();
        assert_eq!(excl.add(gst).unwrap(), incl);
    }

    #[test]
    fn adding_then_extracting_round_trips() {
        let excl = Money::nzd(10000);
        let gst = NZ_15.add_to_exclusive(excl, Rounding::HalfUp).unwrap();
        assert_eq!(gst, Money::nzd(1500));
        let incl = excl.add(gst).unwrap();
        assert_eq!(
            NZ_15
                .extract_from_inclusive(incl, Rounding::HalfUp)
                .unwrap(),
            gst
        );
    }

    #[test]
    fn a_rate_change_needs_no_code_change() {
        let gst_10 = GstRate::new(10, 100);
        assert_eq!(
            gst_10
                .extract_from_inclusive(Money::nzd(11000), Rounding::HalfUp)
                .unwrap(),
            Money::nzd(1000)
        );
    }
}
