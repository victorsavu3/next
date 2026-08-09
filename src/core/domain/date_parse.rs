use chrono::{Datelike, Days, Local, Months, NaiveDate, NaiveTime, TimeZone};
use interim::{parse_date_string, Dialect};

use crate::core::error::{Result, TaskError};

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

/// Parses the compact date forms a filter expression writes unquoted.
///
/// These are the forms that survive a shell without quoting, which is why the
/// filter grammar reaches for them before it reaches for [`parse_date`]:
///
/// - ISO 8601 — `2026-08-10`
/// - a signed offset — `+7d`, `-2w`, `+3m`, `-1y` (days, weeks, months, years)
/// - a named day — `today`, `tomorrow`, `yesterday`
/// - an end-of-period — `eow` (the coming Sunday), `eom`, `eoy`
///
/// Returns `None` when `expr` is none of them, leaving the caller to decide
/// whether to fall back to natural language or to report the value as bad.
/// Names are matched case-insensitively.
pub fn parse_compact(expr: &str, today: NaiveDate) -> Option<NaiveDate> {
    if let Ok(date) = NaiveDate::parse_from_str(expr, "%Y-%m-%d") {
        return Some(date);
    }
    match expr.to_ascii_lowercase().as_str() {
        "today" => return Some(today),
        "tomorrow" => return today.checked_add_days(Days::new(1)),
        "yesterday" => return today.checked_sub_days(Days::new(1)),
        // The UK dialect used for natural language puts Sunday at the end of
        // the week, so `eow` follows suit rather than inventing a second
        // convention for the same tool.
        "eow" => {
            let to_sunday = 6 - today.weekday().num_days_from_monday() as u64;
            return today.checked_add_days(Days::new(to_sunday));
        }
        "eom" => return end_of_month(today),
        "eoy" => return NaiveDate::from_ymd_opt(today.year(), 12, 31),
        _ => {}
    }
    parse_offset(expr, today)
}

/// `+7d` / `-2w` / `+3m` / `-1y`, relative to `today`.
fn parse_offset(expr: &str, today: NaiveDate) -> Option<NaiveDate> {
    let (forward, rest) = match expr.as_bytes().first()? {
        b'+' => (true, &expr[1..]),
        b'-' => (false, &expr[1..]),
        _ => return None,
    };
    // Split on the last CHARACTER, not the last byte. `split_at` takes a byte
    // index and panics when it lands inside a multi-byte character, and this
    // value comes straight from the user: `due<+7é` would abort the CLI and
    // take down an MCP server thread.
    let (unit, digits) = {
        let mut chars = rest.chars();
        let unit = chars.next_back()?;
        (unit, chars.as_str())
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let magnitude: u64 = digits.parse().ok()?;
    let (days, months) = match unit.to_ascii_lowercase() {
        'd' => (magnitude, 0),
        'w' => (magnitude.checked_mul(7)?, 0),
        'm' => (0, u32::try_from(magnitude).ok()?),
        'y' => (0, u32::try_from(magnitude).ok()?.checked_mul(12)?),
        _ => return None,
    };
    if months > 0 {
        let months = Months::new(months);
        return if forward {
            today.checked_add_months(months)
        } else {
            today.checked_sub_months(months)
        };
    }
    let days = Days::new(days);
    if forward {
        today.checked_add_days(days)
    } else {
        today.checked_sub_days(days)
    }
}

/// The last day of `date`'s month.
fn end_of_month(date: NaiveDate) -> Option<NaiveDate> {
    let first = NaiveDate::from_ymd_opt(date.year(), date.month(), 1)?;
    first
        .checked_add_months(Months::new(1))?
        .checked_sub_days(Days::new(1))
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

    // ── Compact forms ────────────────────────────────────────────────────────

    fn compact(expr: &str) -> NaiveDate {
        parse_compact(expr, base()).unwrap_or_else(|| panic!("{expr} did not parse"))
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn compact_iso_and_named_days() {
        assert_eq!(compact("2026-12-31"), date(2026, 12, 31));
        assert_eq!(compact("today"), base());
        assert_eq!(compact("TODAY"), base());
        assert_eq!(compact("tomorrow"), date(2026, 5, 18));
        assert_eq!(compact("yesterday"), date(2026, 5, 16));
    }

    #[test]
    fn compact_offsets() {
        assert_eq!(compact("+7d"), date(2026, 5, 24));
        assert_eq!(compact("-2w"), date(2026, 5, 3));
        assert_eq!(compact("+3m"), date(2026, 8, 17));
        assert_eq!(compact("-1y"), date(2025, 5, 17));
        assert_eq!(compact("+0d"), base());
    }

    #[test]
    fn compact_end_of_period() {
        // 2026-05-17 is a Sunday, so it is already the end of its week.
        assert_eq!(base().weekday(), chrono::Weekday::Sun);
        assert_eq!(compact("eow"), base());
        assert_eq!(
            parse_compact("eow", date(2026, 5, 11)).unwrap(),
            date(2026, 5, 17),
            "a Monday runs to the coming Sunday"
        );
        assert_eq!(compact("eom"), date(2026, 5, 31));
        assert_eq!(compact("eoy"), date(2026, 12, 31));
        assert_eq!(
            parse_compact("eom", date(2028, 2, 3)).unwrap(),
            date(2028, 2, 29),
            "a leap February ends on the 29th"
        );
    }

    #[test]
    fn a_multibyte_unit_does_not_panic() {
        // `split_at` takes a BYTE index. `rest.len() - 1` lands inside the
        // last character whenever it is multi-byte, and `str::split_at` panics
        // rather than erroring. Reachable straight from `next list 'due<+7é'`
        // and from an MCP filter_tokens value, so it aborts the CLI and takes
        // down a server thread.
        for expr in ["+7é", "-2ü", "+1日", "+7🎉", "é", "+é", "-é", "+7\u{0301}"] {
            assert!(parse_compact(expr, base()).is_none(), "{expr:?}");
        }
    }

    #[test]
    fn compact_rejects_what_it_does_not_own() {
        // Natural language is `parse_date`'s job; the compact parser says so by
        // returning None rather than guessing.
        for expr in [
            "next monday",
            "",
            "+d",
            "7d",
            "+7",
            "+7x",
            "+-7d",
            "++7d",
            "-",
            "+9999999999999999999999d",
        ] {
            assert!(parse_compact(expr, base()).is_none(), "{expr:?}");
        }
    }
}
