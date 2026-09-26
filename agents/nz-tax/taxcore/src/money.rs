use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Result, TaxError};

/// ISO 4217 code, stored inline so `Money` stays `Copy`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Currency([u8; 3]);

impl Currency {
    pub const NZD: Currency = Currency(*b"NZD");
    pub const AUD: Currency = Currency(*b"AUD");
    pub const USD: Currency = Currency(*b"USD");

    pub fn new(code: &str) -> Result<Self> {
        let bytes = code.as_bytes();
        if bytes.len() != 3 || !bytes.iter().all(|b| b.is_ascii_alphabetic()) {
            return Err(TaxError::BadCurrency(code.to_string()));
        }
        let mut buf = [0u8; 3];
        for (slot, b) in buf.iter_mut().zip(bytes) {
            *slot = b.to_ascii_uppercase();
        }
        Ok(Currency(buf))
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("validated ascii on construction")
    }
}

impl TryFrom<String> for Currency {
    type Error = TaxError;
    fn try_from(s: String) -> Result<Self> {
        Currency::new(&s)
    }
}

impl From<Currency> for String {
    fn from(c: Currency) -> String {
        c.as_str().to_string()
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Currency({})", self.as_str())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rounding {
    /// 0.5 away from zero. IRD's default expectation for GST amounts.
    HalfUp,
    HalfEven,
    TowardZero,
}

/// A minor-unit amount. Never a float: every cent in a return has to reconcile.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    pub cents: i64,
    pub currency: Currency,
}

impl Money {
    pub const fn new(cents: i64, currency: Currency) -> Self {
        Money { cents, currency }
    }

    pub const fn nzd(cents: i64) -> Self {
        Money::new(cents, Currency::NZD)
    }

    pub const fn zero(currency: Currency) -> Self {
        Money::new(0, currency)
    }

    pub const fn is_zero(&self) -> bool {
        self.cents == 0
    }

    pub const fn abs(&self) -> Self {
        Money::new(self.cents.abs(), self.currency)
    }

    pub const fn neg(&self) -> Self {
        Money::new(-self.cents, self.currency)
    }

    fn same_currency(&self, other: &Money) -> Result<()> {
        if self.currency != other.currency {
            return Err(TaxError::CurrencyMismatch {
                left: self.currency.to_string(),
                right: other.currency.to_string(),
            });
        }
        Ok(())
    }

    pub fn add(&self, other: Money) -> Result<Money> {
        self.same_currency(&other)?;
        let cents = self
            .cents
            .checked_add(other.cents)
            .ok_or(TaxError::AmountOverflow)?;
        Ok(Money::new(cents, self.currency))
    }

    pub fn sub(&self, other: Money) -> Result<Money> {
        self.same_currency(&other)?;
        let cents = self
            .cents
            .checked_sub(other.cents)
            .ok_or(TaxError::AmountOverflow)?;
        Ok(Money::new(cents, self.currency))
    }

    pub fn sum(amounts: &[Money], currency: Currency) -> Result<Money> {
        amounts
            .iter()
            .try_fold(Money::zero(currency), |acc, m| acc.add(*m))
    }

    /// Exact rational scaling. Tax fractions are rationals, not decimals:
    /// the GST component of a 15% inclusive price is `x * 3 / 23`, and going
    /// via f64 loses cents on large invoices.
    pub fn mul_ratio(&self, numerator: i64, denominator: i64, rounding: Rounding) -> Result<Money> {
        if denominator == 0 {
            return Err(TaxError::DivideByZero);
        }
        let scaled = (self.cents as i128)
            .checked_mul(numerator as i128)
            .ok_or(TaxError::AmountOverflow)?;
        let den = denominator as i128;
        let quotient = scaled / den;
        let remainder = scaled % den;
        if remainder == 0 {
            return Ok(Money::new(
                i64::try_from(quotient).map_err(|_| TaxError::AmountOverflow)?,
                self.currency,
            ));
        }

        let sign = if (scaled < 0) != (den < 0) { -1i128 } else { 1 };
        let twice = (remainder * 2).abs();
        let den_abs = den.abs();
        let bump = match rounding {
            Rounding::TowardZero => false,
            Rounding::HalfUp => twice >= den_abs,
            Rounding::HalfEven => twice > den_abs || (twice == den_abs && quotient % 2 != 0),
        };

        let cents = if bump { quotient + sign } else { quotient };
        Ok(Money::new(
            i64::try_from(cents).map_err(|_| TaxError::AmountOverflow)?,
            self.currency,
        ))
    }

    /// Split into `n` parts that sum back to the original exactly, distributing
    /// the leftover cents one each to the earliest parts.
    pub fn allocate(&self, n: usize) -> Result<Vec<Money>> {
        if n == 0 {
            return Err(TaxError::DivideByZero);
        }
        let n_i64 = n as i64;
        let base = self.cents / n_i64;
        let mut remainder = self.cents % n_i64;
        let step = if self.cents < 0 { -1 } else { 1 };
        let mut parts = Vec::with_capacity(n);
        for _ in 0..n {
            let extra = if remainder != 0 {
                remainder -= step;
                step
            } else {
                0
            };
            parts.push(Money::new(base + extra, self.currency));
        }
        Ok(parts)
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.cents < 0 { "-" } else { "" };
        let abs = self.cents.unsigned_abs();
        write!(
            f,
            "{}{}.{:02} {}",
            sign,
            abs / 100,
            abs % 100,
            self.currency
        )
    }
}

impl fmt::Debug for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_cross_currency_arithmetic() {
        let nzd = Money::nzd(100);
        let usd = Money::new(100, Currency::USD);
        assert!(nzd.add(usd).is_err());
    }

    #[test]
    fn gst_component_is_exact_at_15_percent() {
        // 3/23 of a GST-inclusive amount. 115.00 incl -> 15.00 GST.
        let incl = Money::nzd(11500);
        assert_eq!(incl.mul_ratio(3, 23, Rounding::HalfUp).unwrap().cents, 1500);
    }

    #[test]
    fn gst_rounding_is_half_up_not_truncating() {
        // 10.00 incl -> 1.304347... -> 1.30
        assert_eq!(
            Money::nzd(1000)
                .mul_ratio(3, 23, Rounding::HalfUp)
                .unwrap()
                .cents,
            130
        );
        // 19.99 incl -> 2.6073913... -> 2.61
        assert_eq!(
            Money::nzd(1999)
                .mul_ratio(3, 23, Rounding::HalfUp)
                .unwrap()
                .cents,
            261
        );
    }

    #[test]
    fn negative_amounts_round_away_from_zero() {
        assert_eq!(
            Money::nzd(-1000)
                .mul_ratio(3, 23, Rounding::HalfUp)
                .unwrap()
                .cents,
            -130
        );
    }

    #[test]
    fn allocate_preserves_the_total() {
        let parts = Money::nzd(1000).allocate(3).unwrap();
        assert_eq!(parts.iter().map(|m| m.cents).sum::<i64>(), 1000);
        assert_eq!(parts[0].cents, 334);
        assert_eq!(parts[2].cents, 333);
    }

    #[test]
    fn allocate_preserves_negative_totals() {
        let parts = Money::nzd(-1000).allocate(3).unwrap();
        assert_eq!(parts.iter().map(|m| m.cents).sum::<i64>(), -1000);
    }
}
