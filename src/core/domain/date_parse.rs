use chrono::{Local, NaiveDate, NaiveTime, TimeZone};
use interim::{parse_date_string, Dialect};

use crate::core::error::{TaskError, Result};

/// Parses a date expression into a [`NaiveDate`].
///
/// Accepts ISO 8601 (`YYYY-MM-DD`) first, then falls back to natural-language
/// expressions understood by the `interim` crate (e.g. `"next monday"`,
/// `"in two weeks"`, `"tomorrow"`).
///
/// `today` is the reference date for relative expressions.
pub fn parse_date(expr: &str, today: NaiveDate) -> Result<NaiveDate> {
    if let Ok(date) = NaiveDate::parse_from_str(expr, "%Y-%m-%d") {
        return Ok(date);
    }

    // interim requires a timezone-aware DateTime; build one from `today` at midnight.
    let now = Local
        .from_local_datetime(&today.and_time(NaiveTime::MIN))
        .single()
        .ok_or_else(|| TaskError::Other(format!("cannot build datetime for {today}")))?;

    parse_date_string(expr, now, Dialect::Uk)
        .map(|dt| dt.date_naive())
        .map_err(|e| TaskError::Other(format!("cannot parse date {expr:?}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 5, 17).unwrap()
    }

    #[test]
    fn iso_date() {
        let d = parse_date("2026-12-31", base()).unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(2026, 12, 31).unwrap());
    }

    #[test]
    fn tomorrow() {
        let d = parse_date("tomorrow", base()).unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(2026, 5, 18).unwrap());
    }

    #[test]
    fn invalid_expression_errors() {
        assert!(parse_date("not a date at all xyz", base()).is_err());
    }
}
