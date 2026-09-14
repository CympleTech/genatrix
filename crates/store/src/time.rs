//! Time column helpers. Timestamps are stored as RFC 3339 text; the source's
//! own offset is preserved for items, UTC everywhere else.

use chrono::{DateTime, FixedOffset, Utc};

use crate::error::{Result, corrupt};

pub(crate) fn utc_to_col(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

pub(crate) fn utc_from_col(
    table: &'static str,
    column: &'static str,
    s: &str,
) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| corrupt(table, column, e))
}

pub(crate) fn offset_to_col(t: DateTime<FixedOffset>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, false)
}

pub(crate) fn offset_from_col(
    table: &'static str,
    column: &'static str,
    s: &str,
) -> Result<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(s).map_err(|e| corrupt(table, column, e))
}

pub(crate) fn opt_utc_to_col(t: Option<DateTime<Utc>>) -> Option<String> {
    t.map(utc_to_col)
}

pub(crate) fn opt_utc_from_col(
    table: &'static str,
    column: &'static str,
    s: Option<String>,
) -> Result<Option<DateTime<Utc>>> {
    s.map(|s| utc_from_col(table, column, &s)).transpose()
}
