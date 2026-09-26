//! The rule files, compiled in. A new year is a new file here and a new
//! line below; a file is never edited once returns have used it.

use chrono::NaiveDate;
use taxcore::TaxYear;
use taxrules::RuleSet;

const FILES: [(i32, &str); 2] = [
    (2026, include_str!("../../rules/nz/2025-26.yaml")),
    (2027, include_str!("../../rules/nz/2026-27.yaml")),
];

/// The rules for the tax year a date falls in, and whether they are a copy
/// not yet checked against IRD. `None` when there is no file for that year.
#[must_use]
pub fn for_date(date: NaiveDate) -> Option<(RuleSet, bool)> {
    for_year(TaxYear::containing(date))
}

/// The rules for a tax year (named by the year it ends in).
#[must_use]
pub fn for_year(year: TaxYear) -> Option<(RuleSet, bool)> {
    let (_, text) = FILES.iter().find(|(y, _)| *y == year.0)?;
    let rules = RuleSet::from_yaml(text).ok()?;
    let copied = rules.meta.copied_from.is_some();
    Some((rules, copied))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_file_loads_for_its_own_year() {
        for (year, _) in FILES {
            let (rules, _) = for_year(TaxYear(year)).expect("loads");
            assert_eq!(rules.meta.tax_year().unwrap(), TaxYear(year));
        }
        assert!(for_year(TaxYear(2024)).is_none());
    }
}
