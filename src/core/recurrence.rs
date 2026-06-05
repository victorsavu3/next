use anyhow::Context as _;
use chrono::{Datelike, Duration, NaiveDate, Weekday};

use crate::core::domain::task::{Recurrence, Snap, Task};

// ─── snap ──────────────────────────────────────────────────────────────────

/// Parse a CLI snap string into a [`Snap`] value.
pub fn parse_snap(s: &str) -> anyhow::Result<Snap> {
    match s.to_lowercase().as_str() {
        "next-workday" | "workday" => Ok(Snap::NextWorkday),
        "mon" | "monday" => Ok(Snap::NextWeekday { weekday: 0 }),
        "tue" | "tuesday" => Ok(Snap::NextWeekday { weekday: 1 }),
        "wed" | "wednesday" => Ok(Snap::NextWeekday { weekday: 2 }),
        "thu" | "thursday" => Ok(Snap::NextWeekday { weekday: 3 }),
        "fri" | "friday" => Ok(Snap::NextWeekday { weekday: 4 }),
        "sat" | "saturday" => Ok(Snap::NextWeekday { weekday: 5 }),
        "sun" | "sunday" => Ok(Snap::NextWeekday { weekday: 6 }),
        other => {
            if let Some(n) = other
                .strip_prefix("dom:")
                .or_else(|| other.strip_prefix("day-of-month:"))
            {
                let day: u8 = n.parse().context("invalid day-of-month")?;
                anyhow::ensure!((1..=28).contains(&day), "day-of-month must be 1–28");
                Ok(Snap::DayOfMonth { day })
            } else {
                anyhow::bail!(
                    "unknown snap {s:?} — try: next-workday, monday … sunday, dom:N"
                )
            }
        }
    }
}

/// Apply a [`Snap`] to move `date` forward to the nearest qualifying date.
pub fn apply_snap(mut date: NaiveDate, snap: &Snap) -> NaiveDate {
    match snap {
        Snap::NextWeekday { weekday } => {
            let current = date.weekday().num_days_from_monday() as u8;
            let days = (weekday + 7 - current) % 7;
            date + Duration::days(days as i64)
        }
        Snap::NextWorkday => {
            while matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
                date += Duration::days(1);
            }
            date
        }
        Snap::DayOfMonth { day } => {
            let d = *day as u32;
            if let Some(candidate) = NaiveDate::from_ymd_opt(date.year(), date.month(), d) {
                if candidate >= date {
                    return candidate;
                }
            }
            // Move to next month
            let (y, m) = if date.month() == 12 {
                (date.year() + 1, 1u32)
            } else {
                (date.year(), date.month() + 1)
            };
            // day doesn't exist in month m — clamp to the last day of that month
            clamped_date(y, m, d)
        }
    }
}

/// Last calendar day of `(year, month)` (e.g. 28/29 for February, 30/31 else).
///
/// Computed as the first of the following month minus one day, which handles
/// leap years and the December → January wrap without a lookup table.
fn last_day_of_month(year: i32, month: u32) -> NaiveDate {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(ny, nm, 1).unwrap() - Duration::days(1)
}

/// Build the candidate date for `day` in `(year, month)`, clamping a
/// nonexistent target day (e.g. the 31st of a 30-day month, or Feb 29 in a
/// non-leap year) to the last day of that month rather than skipping it.
fn clamped_date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap_or_else(|| last_day_of_month(year, month))
}

// ─── RRULE parser ──────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug)]
struct RRule {
    freq: Freq,
    interval: i64,
    by_day: Vec<u8>,        // 0=Mon … 6=Sun
    by_month_day: Vec<u32>, // positive day-of-month values
}

fn parse_rrule(rrule: &str) -> anyhow::Result<RRule> {
    let mut freq: Option<Freq> = None;
    let mut interval: i64 = 1;
    let mut by_day: Vec<u8> = Vec::new();
    let mut by_month_day: Vec<u32> = Vec::new();

    for part in rrule.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, value) = part
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("invalid RRULE part: {part:?}"))?;
        match key.to_uppercase().as_str() {
            "FREQ" => {
                freq = Some(match value.to_uppercase().as_str() {
                    "DAILY" => Freq::Daily,
                    "WEEKLY" => Freq::Weekly,
                    "MONTHLY" => Freq::Monthly,
                    "YEARLY" => Freq::Yearly,
                    other => anyhow::bail!("unsupported FREQ={other}"),
                });
            }
            "INTERVAL" => {
                interval = value
                    .parse::<i64>()
                    .context("INTERVAL must be a positive integer")?;
                anyhow::ensure!(interval >= 1, "INTERVAL must be >= 1");
            }
            "BYDAY" => {
                for token in value.split(',') {
                    let token = token.trim();
                    // Reject positional prefixes (e.g. "1MO", "-1FR") — not supported.
                    let has_prefix = token.starts_with(|c: char| c.is_ascii_digit() || c == '+' || c == '-');
                    anyhow::ensure!(
                        !has_prefix,
                        "positional BYDAY values (e.g. \"1MO\", \"-1FR\") are not supported"
                    );
                    let wd = match token.to_uppercase().as_str() {
                        "MO" => 0u8,
                        "TU" => 1,
                        "WE" => 2,
                        "TH" => 3,
                        "FR" => 4,
                        "SA" => 5,
                        "SU" => 6,
                        other => anyhow::bail!("unknown weekday abbreviation: {other}"),
                    };
                    by_day.push(wd);
                }
            }
            "BYMONTHDAY" => {
                for token in value.split(',') {
                    let n: i64 = token
                        .trim()
                        .parse()
                        .context("BYMONTHDAY value must be an integer")?;
                    anyhow::ensure!(n >= 1, "only positive BYMONTHDAY values are supported");
                    by_month_day.push(n as u32);
                }
            }
            // Silently ignore unsupported fields (UNTIL, COUNT, etc.)
            _ => {}
        }
    }

    Ok(RRule {
        freq: freq.ok_or_else(|| anyhow::anyhow!("RRULE missing FREQ"))?,
        interval,
        by_day,
        by_month_day,
    })
}

/// Validate a schedule recurrence rule (RRULE) string.
///
/// Runs the rule through the authoritative parser ([`parse_rrule`]) and returns
/// an error if it is malformed or uses an unsupported feature (missing `FREQ`,
/// `INTERVAL` < 1, non-positive `BYMONTHDAY`, unknown `FREQ`, positional
/// `BYDAY`, etc.). Used to reject invalid rules up front at `add`/`edit` time
/// instead of failing later when the task is completed.
pub fn validate_rrule(rrule: &str) -> anyhow::Result<()> {
    parse_rrule(rrule).map(|_| ())
}

/// Returns the first occurrence of the rule strictly after `after`.
pub fn next_occurrence(
    rrule: &str,
    anchor: NaiveDate,
    after: NaiveDate,
) -> anyhow::Result<NaiveDate> {
    let rule = parse_rrule(rrule)?;

    match rule.freq {
        Freq::Weekly => {
            // When BYDAY is absent, default to the anchor's weekday (per RFC 5545).
            let anchor_wd = anchor.weekday().num_days_from_monday() as u8;
            let effective_by_day: &[u8] = if rule.by_day.is_empty() {
                std::slice::from_ref(&anchor_wd)
            } else {
                &rule.by_day
            };
            // Walk day by day; limit extended to cover large INTERVAL values.
            let limit = (rule.interval as usize) * 7 + 14;
            let anchor_monday =
                anchor - Duration::days(anchor.weekday().num_days_from_monday() as i64);
            let mut d = after + Duration::days(1);
            for _ in 0..limit {
                let wd = d.weekday().num_days_from_monday() as u8;
                if effective_by_day.contains(&wd) {
                    let d_monday =
                        d - Duration::days(d.weekday().num_days_from_monday() as i64);
                    let weeks = (d_monday - anchor_monday).num_days() / 7;
                    if weeks >= 0 && weeks % rule.interval == 0 {
                        return Ok(d);
                    }
                }
                d += Duration::days(1);
            }
            anyhow::bail!("no weekly occurrence found within {} days", limit)
        }

        Freq::Daily => {
            // When BYDAY is present, an interval-aligned day may only coincide
            // with an allowed weekday every lcm(interval, 7) days (up to 7×interval
            // when interval and 7 are coprime). Mirror the WEEKLY bound so such
            // rules resolve instead of erroring; for plain DAILY this is still
            // ample (the first aligned day is interval days out).
            let limit = (rule.interval as usize) * 7 + 14;
            let mut d = after + Duration::days(1);
            for _ in 0..limit {
                let wd = d.weekday().num_days_from_monday() as u8;
                let in_by_day = rule.by_day.is_empty() || rule.by_day.contains(&wd);
                if in_by_day {
                    let days_from_anchor = (d - anchor).num_days();
                    if days_from_anchor >= 0 && days_from_anchor % rule.interval == 0 {
                        return Ok(d);
                    }
                }
                d += Duration::days(1);
            }
            anyhow::bail!("no daily occurrence found within {} days", limit)
        }

        Freq::Monthly => {
            // Use 0-based month index
            let month_idx = |y: i32, m: u32| -> i64 { y as i64 * 12 + (m as i64 - 1) };
            let anchor_idx = month_idx(anchor.year(), anchor.month());
            let mut candidate_idx = month_idx(after.year(), after.month());
            let limit = anchor_idx + 50 * 12;

            // When BYMONTHDAY lists several days, the earliest valid date `> after`
            // may be any of them, so sort ascending and pick the minimum match.
            let mut days: Vec<u32> = if rule.by_month_day.is_empty() {
                vec![anchor.day()]
            } else {
                rule.by_month_day.clone()
            };
            days.sort_unstable();

            loop {
                let rel = candidate_idx - anchor_idx;
                if rel >= 0 && rel % rule.interval == 0 {
                    let year = (candidate_idx / 12) as i32;
                    let month = (candidate_idx % 12 + 1) as u32;

                    // Pick the earliest valid date `> after` among all BYMONTHDAY
                    // values in this month. A target day that doesn't exist in
                    // this month (e.g. the 31st of a 30-day month, or Feb 29 in
                    // a non-leap year) is CLAMPED to the month's last day rather
                    // than skipped (issue #12). Several days can clamp to the
                    // same date (e.g. 30 and 31 both → Feb 28), so `min` over the
                    // clamped candidates naturally dedupes and still yields the
                    // earliest valid date `> after` (issue #11).
                    if let Some(d) = days
                        .iter()
                        .map(|&day| clamped_date(year, month, day))
                        .filter(|&d| d > after)
                        .min()
                    {
                        return Ok(d);
                    }
                }
                candidate_idx += 1;
                if candidate_idx > limit {
                    anyhow::bail!("no monthly occurrence found within 50 years");
                }
            }
        }

        Freq::Yearly => {
            // Yearly is treated as FREQ=MONTHLY;INTERVAL=12*interval
            let effective_interval = rule.interval * 12;
            let month_idx = |y: i32, m: u32| -> i64 { y as i64 * 12 + (m as i64 - 1) };
            let anchor_idx = month_idx(anchor.year(), anchor.month());
            let mut candidate_idx = month_idx(after.year(), after.month());
            let limit = anchor_idx + 50 * 12;

            // Honour every BYMONTHDAY value (not just the first) and pick the
            // earliest valid date `> after` within the qualifying month (issue #11).
            let mut days: Vec<u32> = if rule.by_month_day.is_empty() {
                vec![anchor.day()]
            } else {
                rule.by_month_day.clone()
            };
            days.sort_unstable();

            loop {
                let rel = candidate_idx - anchor_idx;
                if rel >= 0 && rel % effective_interval == 0 {
                    let year = (candidate_idx / 12) as i32;
                    let month = (candidate_idx % 12 + 1) as u32;

                    // Clamp a nonexistent target day to the month's last day
                    // rather than skipping the year (issue #12); `min` over the
                    // clamped candidates dedupes and picks the earliest > after.
                    if let Some(d) = days
                        .iter()
                        .map(|&day| clamped_date(year, month, day))
                        .filter(|&d| d > after)
                        .min()
                    {
                        return Ok(d);
                    }
                }
                candidate_idx += 1;
                if candidate_idx > limit {
                    anyhow::bail!("no yearly occurrence found within 50 years");
                }
            }
        }
    }
}

// ─── project_series ──────────────────────────────────────────────────────────

/// Hard cap on the number of projected occurrences generated per series, as a
/// safety net against a misbehaving rule that fails to advance. The horizon is
/// the real bound; this only guards against pathological cases.
const MAX_PROJECTED_PER_SERIES: usize = 366;

/// Enumerate the projected (not-yet-spawned) future occurrences of `task`'s
/// recurrence series with dates `> today` (and `> the current instance`) up to
/// and including `cutoff`.
///
/// Pure and timezone-free: `today` and `cutoff` are passed in so the function is
/// directly testable. Returns an empty list when the task is not an active
/// schedule-type recurring task. Completion-type recurrence is intentionally not
/// projected: the next date is `completion_date + interval_days`, and future
/// completion dates are unknown, so no deterministic series exists to forecast.
///
/// Shared by both the CLI `forecast` command and the MCP `get_forecast` tool so
/// their projections cannot drift.
pub fn project_series(task: &Task, today: NaiveDate, cutoff: NaiveDate) -> Vec<NaiveDate> {
    if !task.is_active() {
        return Vec::new();
    }
    let Some(Recurrence::Schedule { rrule, anchor, snap }) = task.recurrence.as_ref() else {
        return Vec::new();
    };

    // Walk the raw (un-snapped) series so each `next_occurrence` call strictly
    // advances; the snap is applied only to the emitted date. The concrete task
    // already covers its own `due`, so start projecting strictly after it.
    let mut after = [task.due, task.start, Some(today)]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(today);

    let mut dates = Vec::new();
    for _ in 0..MAX_PROJECTED_PER_SERIES {
        let raw = match next_occurrence(rrule, *anchor, after) {
            Ok(d) => d,
            Err(_) => break,
        };
        // `next_occurrence` guarantees raw > after, so the walk terminates.
        after = raw;
        let occurrence = snap.as_ref().map_or(raw, |s| apply_snap(raw, s));
        if occurrence > cutoff {
            break;
        }
        // Snapping can move a date backwards to a prior emitted one or onto the
        // current instance; only keep strictly-future, in-horizon dates.
        if occurrence > today {
            dates.push(occurrence);
        }
    }
    dates
}

// ─── spawn_next ────────────────────────────────────────────────────────────

/// Creates the next recurring task instance from a completed task.
///
/// Returns `Ok(None)` if the task has no recurrence rule, `Err` if a
/// schedule rule fails to produce a valid next date.
pub fn spawn_next(task: &Task, today: NaiveDate) -> anyhow::Result<Option<Task>> {
    let recurrence = match task.recurrence.as_ref() {
        Some(r) => r,
        None => return Ok(None),
    };

    // For fixed schedule, don't go back before the current task's due/start date.
    let after = match recurrence {
        Recurrence::Schedule { .. } => [task.due, task.start, Some(today)]
            .iter()
            .filter_map(|d| *d)
            .max()
            .unwrap_or(today),
        Recurrence::Completion { .. } => today,
    };

    let occurrence = match recurrence {
        Recurrence::Schedule { rrule, anchor, snap } => {
            let raw = next_occurrence(rrule, *anchor, after)?;
            snap.as_ref().map_or(raw, |s| apply_snap(raw, s))
        }
        Recurrence::Completion { interval_days, snap } => {
            let raw = today + Duration::days(*interval_days as i64);
            snap.as_ref().map_or(raw, |s| apply_snap(raw, s))
        }
    };

    let mut next = Task::new(task.title.clone());
    next.recurrence = task.recurrence.clone();
    next.recurrence_id = Some(task.recurrence_id.unwrap_or(task.id));
    next.priority = task.priority.clone();
    next.tags = task.tags.clone();
    next.assignee = task.assignee.clone();
    next.parent_id = task.parent_id;
    next.description = task.description.clone();
    next.url = task.url.clone();
    next.notes = task.notes.clone();
    next.long_term = task.long_term;
    next.score_adjustment = task.score_adjustment;
    // Copy user/tool data but not the instance-specific time_log.
    next.data = task
        .data
        .iter()
        .filter(|(k, _)| k.as_str() != "time_log")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    match (task.start, task.due) {
        (Some(s), Some(d)) => {
            let offset = d - s;
            next.start = Some(occurrence);
            next.due = Some(occurrence + offset);
        }
        (Some(_), None) => next.start = Some(occurrence),
        (None, Some(_)) => next.due = Some(occurrence),
        (None, None) => next.start = Some(occurrence),
    }

    Ok(Some(next))
}

// ─── tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    // ── apply_snap ──────────────────────────────────────────────────────────

    #[test]
    fn snap_next_weekday_already_on_day() {
        // Saturday (weekday=5) on a Saturday → keep it
        let date = d(2026, 5, 2); // Saturday
        assert_eq!(date.weekday(), Weekday::Sat);
        let result = apply_snap(date, &Snap::NextWeekday { weekday: 5 });
        assert_eq!(result, date);
    }

    #[test]
    fn snap_next_weekday_advances() {
        // Wednesday + Saturday snap → next Saturday
        let date = d(2026, 4, 29); // Wednesday
        assert_eq!(date.weekday(), Weekday::Wed);
        let result = apply_snap(date, &Snap::NextWeekday { weekday: 5 }); // Saturday
        assert_eq!(result, d(2026, 5, 2));
    }

    #[test]
    fn snap_next_workday_on_saturday() {
        let date = d(2026, 5, 2); // Saturday
        let result = apply_snap(date, &Snap::NextWorkday);
        assert_eq!(result, d(2026, 5, 4)); // Monday
    }

    #[test]
    fn snap_next_workday_on_friday() {
        let date = d(2026, 5, 1); // Friday
        let result = apply_snap(date, &Snap::NextWorkday);
        assert_eq!(result, date); // keep Friday
    }

    #[test]
    fn snap_day_of_month_before_target() {
        // May 3 + dom:10 → May 10
        let date = d(2026, 5, 3);
        let result = apply_snap(date, &Snap::DayOfMonth { day: 10 });
        assert_eq!(result, d(2026, 5, 10));
    }

    #[test]
    fn snap_day_of_month_after_target() {
        // May 15 + dom:10 → Jun 10
        let date = d(2026, 5, 15);
        let result = apply_snap(date, &Snap::DayOfMonth { day: 10 });
        assert_eq!(result, d(2026, 6, 10));
    }

    #[test]
    fn snap_day_of_month_on_target() {
        // May 10 + dom:10 → May 10
        let date = d(2026, 5, 10);
        let result = apply_snap(date, &Snap::DayOfMonth { day: 10 });
        assert_eq!(result, d(2026, 5, 10));
    }

    // ── next_occurrence ─────────────────────────────────────────────────────

    #[test]
    fn workday_rule_skips_weekend() {
        // FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR; after a Friday → returns next Monday
        let anchor = d(2026, 4, 27); // Monday
        let after = d(2026, 5, 1); // Friday
        let result = next_occurrence("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR", anchor, after).unwrap();
        assert_eq!(result.weekday(), Weekday::Mon);
        assert!(result > after);
    }

    #[test]
    fn monthly_bymonthday_returns_next_month() {
        // After May 5, BYMONTHDAY=1 → Jun 1
        let anchor = d(2026, 4, 1);
        let after = d(2026, 5, 5);
        let result = next_occurrence("FREQ=MONTHLY;BYMONTHDAY=1", anchor, after).unwrap();
        assert_eq!(result, d(2026, 6, 1));
    }

    #[test]
    fn monthly_interval3_respects_anchor() {
        // anchor=Jan 1, after=Jan 15, INTERVAL=3 → Apr 1
        let anchor = d(2026, 1, 1);
        let after = d(2026, 1, 15);
        let result = next_occurrence("FREQ=MONTHLY;INTERVAL=3;BYMONTHDAY=1", anchor, after).unwrap();
        assert_eq!(result, d(2026, 4, 1));
    }

    #[test]
    fn monthly_interval3_far_future() {
        // anchor=Jan 1, after=Apr 5 → Jul 1
        let anchor = d(2026, 1, 1);
        let after = d(2026, 4, 5);
        let result = next_occurrence("FREQ=MONTHLY;INTERVAL=3;BYMONTHDAY=1", anchor, after).unwrap();
        assert_eq!(result, d(2026, 7, 1));
    }

    #[test]
    fn monthly_multiple_bymonthday_sorted_and_unsorted_match() {
        // FREQ=MONTHLY;BYMONTHDAY=1,15 and the unsorted 15,1 must agree.
        // From mid-month (May 10) the next is the upcoming 15th (May 15).
        let anchor = d(2026, 5, 1);
        let after = d(2026, 5, 10);
        let sorted = next_occurrence("FREQ=MONTHLY;BYMONTHDAY=1,15", anchor, after).unwrap();
        let unsorted = next_occurrence("FREQ=MONTHLY;BYMONTHDAY=15,1", anchor, after).unwrap();
        assert_eq!(sorted, d(2026, 5, 15));
        assert_eq!(unsorted, d(2026, 5, 15));
        assert_eq!(sorted, unsorted);
    }

    #[test]
    fn monthly_multiple_bymonthday_picks_earliest_same_month() {
        // OLD BUG: with unsorted BYMONTHDAY=15,1, after the 5th the code returned
        // the FIRST candidate > after (the 15th) only by luck; but after a day
        // before both, it must still pick the 15th (the earliest > after) here.
        // After May 5, valid days this month are the 15th (1st already passed),
        // so next is May 15 — earliest, not whatever comes first in the list.
        let anchor = d(2026, 5, 1);
        let after = d(2026, 5, 5);
        let result = next_occurrence("FREQ=MONTHLY;BYMONTHDAY=15,1", anchor, after).unwrap();
        assert_eq!(result, d(2026, 5, 15));
    }

    #[test]
    fn monthly_multiple_bymonthday_rolls_to_next_month() {
        // After May 20, both the 1st and 15th have passed this month, so the
        // next occurrence is the 1st of next month (June 1) — the earliest
        // qualifying day across the wrap, with unsorted input.
        let anchor = d(2026, 5, 1);
        let after = d(2026, 5, 20);
        let result = next_occurrence("FREQ=MONTHLY;BYMONTHDAY=15,1", anchor, after).unwrap();
        assert_eq!(result, d(2026, 6, 1));
    }

    #[test]
    fn monthly_single_bymonthday_unchanged_regression() {
        // Single-value BYMONTHDAY=15 must behave exactly as before.
        let anchor = d(2026, 5, 1);
        // After the 10th → upcoming 15th.
        assert_eq!(
            next_occurrence("FREQ=MONTHLY;BYMONTHDAY=15", anchor, d(2026, 5, 10)).unwrap(),
            d(2026, 5, 15)
        );
        // After the 20th → 15th of next month.
        assert_eq!(
            next_occurrence("FREQ=MONTHLY;BYMONTHDAY=15", anchor, d(2026, 5, 20)).unwrap(),
            d(2026, 6, 15)
        );
    }

    #[test]
    fn yearly_multiple_bymonthday_picks_earliest() {
        // FREQ=YEARLY;BYMONTHDAY=1,15 — anchor in March so the qualifying month
        // is March. From Feb 1 the next is Mar 1 (earliest of the two days).
        // This proves YEARLY no longer ignores the extra (15) value.
        let anchor = d(2026, 3, 1);
        let after = d(2026, 2, 1);
        let result = next_occurrence("FREQ=YEARLY;BYMONTHDAY=1,15", anchor, after).unwrap();
        assert_eq!(result, d(2026, 3, 1));
    }

    #[test]
    fn yearly_multiple_bymonthday_second_value_in_same_month() {
        // From Mar 5 the 1st has passed, so the next is Mar 15 — the SECOND
        // listed value. With the old code (by_month_day[0] only) this would
        // have skipped the 15th and jumped to next year's Mar 1.
        let anchor = d(2026, 3, 1);
        let after = d(2026, 3, 5);
        let result = next_occurrence("FREQ=YEARLY;BYMONTHDAY=1,15", anchor, after).unwrap();
        assert_eq!(result, d(2026, 3, 15));
    }

    #[test]
    fn yearly_multiple_bymonthday_unsorted_matches_sorted() {
        // Unsorted 15,1 must agree with sorted 1,15.
        let anchor = d(2026, 3, 1);
        let after = d(2026, 3, 5);
        let sorted = next_occurrence("FREQ=YEARLY;BYMONTHDAY=1,15", anchor, after).unwrap();
        let unsorted = next_occurrence("FREQ=YEARLY;BYMONTHDAY=15,1", anchor, after).unwrap();
        assert_eq!(sorted, unsorted);
        assert_eq!(unsorted, d(2026, 3, 15));
    }

    #[test]
    fn yearly_single_bymonthday_unchanged_regression() {
        // Single-value BYMONTHDAY=15 YEARLY must behave as before.
        let anchor = d(2026, 3, 1);
        // After Mar 5 → Mar 15 same year.
        assert_eq!(
            next_occurrence("FREQ=YEARLY;BYMONTHDAY=15", anchor, d(2026, 3, 5)).unwrap(),
            d(2026, 3, 15)
        );
        // After Mar 20 → Mar 15 next year.
        assert_eq!(
            next_occurrence("FREQ=YEARLY;BYMONTHDAY=15", anchor, d(2026, 3, 20)).unwrap(),
            d(2027, 3, 15)
        );
    }

    // ── month-end clamping (issue #12) ──────────────────────────────────────

    #[test]
    fn monthly_clamps_jan31_to_feb_end_then_keeps_mar31() {
        // Anchor Jan 31, FREQ=MONTHLY. 2026 is not a leap year, so February has
        // 28 days: after Jan 31 → Feb 28 (clamped, not skipped to March).
        let anchor = d(2026, 1, 31);
        let result = next_occurrence("FREQ=MONTHLY", anchor, d(2026, 1, 31)).unwrap();
        assert_eq!(result, d(2026, 2, 28));
        // After Feb 28 → Mar 31: March is a full month, so the day-31 target is
        // kept exactly (clamp only applies to short months).
        let result = next_occurrence("FREQ=MONTHLY", anchor, d(2026, 2, 28)).unwrap();
        assert_eq!(result, d(2026, 3, 31));
    }

    #[test]
    fn monthly_clamps_jan31_to_feb29_in_leap_year() {
        // Anchor Jan 31 in a leap year (2028): after Jan 31 → Feb 29.
        let anchor = d(2028, 1, 31);
        let result = next_occurrence("FREQ=MONTHLY", anchor, d(2028, 1, 31)).unwrap();
        assert_eq!(result, d(2028, 2, 29));
    }

    #[test]
    fn monthly_bymonthday31_clamps_in_30day_month() {
        // BYMONTHDAY=31 MONTHLY across April (30 days) → Apr 30.
        let anchor = d(2026, 1, 31);
        let result = next_occurrence("FREQ=MONTHLY;BYMONTHDAY=31", anchor, d(2026, 4, 1)).unwrap();
        assert_eq!(result, d(2026, 4, 30));
    }

    #[test]
    fn monthly_bymonthday_30_31_dedup_in_february() {
        // BYMONTHDAY=30,31 in February (2026, 28 days): both clamp to Feb 28, so
        // the result is a single Feb 28 (no double counting), and after Feb 28
        // the next qualifying day is Mar 30 (the earliest of 30/31 in March).
        let anchor = d(2026, 1, 30);
        let result =
            next_occurrence("FREQ=MONTHLY;BYMONTHDAY=30,31", anchor, d(2026, 1, 31)).unwrap();
        assert_eq!(result, d(2026, 2, 28));
        let result =
            next_occurrence("FREQ=MONTHLY;BYMONTHDAY=30,31", anchor, d(2026, 2, 28)).unwrap();
        assert_eq!(result, d(2026, 3, 30));
    }

    #[test]
    fn yearly_clamps_feb29_to_feb28_in_non_leap_years() {
        // Anchor Feb 29 2024, FREQ=YEARLY: after Feb 29 2024 → Feb 28 2025
        // (clamped, year not skipped).
        let anchor = d(2024, 2, 29);
        let result = next_occurrence("FREQ=YEARLY", anchor, d(2024, 2, 29)).unwrap();
        assert_eq!(result, d(2025, 2, 28));
        // 2028 is a leap year, so Feb 29 2028 is kept exactly (not clamped).
        let result = next_occurrence("FREQ=YEARLY", anchor, d(2027, 3, 1)).unwrap();
        assert_eq!(result, d(2028, 2, 29));
    }

    #[test]
    fn weekly_interval2_respects_anchor() {
        // BYDAY=MO, INTERVAL=2, anchor=May 4 (Mon)
        // after=May 11 (Mon) → May 18 is the next even week from anchor
        let anchor = d(2026, 5, 4); // Monday
        let after = d(2026, 5, 11); // Monday, odd weeks from anchor (1 week after)
        // Week 0 = May 4, Week 1 = May 11, Week 2 = May 18
        let result = next_occurrence("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO", anchor, after).unwrap();
        assert_eq!(result, d(2026, 5, 18));
    }

    #[test]
    fn daily_interval5_byday_monday_aligns_at_lcm() {
        // FREQ=DAILY;INTERVAL=5;BYDAY=MO from a Monday anchor.
        // The first day that is BOTH a multiple of 5 from the anchor AND a Monday
        // is lcm(5, 7) = 35 days out. anchor = May 4 2026 (Mon) → June 8 2026 (Mon).
        let anchor = d(2026, 5, 4); // Monday
        assert_eq!(anchor.weekday(), Weekday::Mon);
        let after = d(2026, 5, 4); // same day as anchor
        let result = next_occurrence("FREQ=DAILY;INTERVAL=5;BYDAY=MO", anchor, after).unwrap();
        assert_eq!(result, d(2026, 6, 8));
        assert_eq!(result.weekday(), Weekday::Mon);
        assert_eq!((result - anchor).num_days() % 5, 0);
    }

    #[test]
    fn daily_interval3_byday_tuesday_aligns() {
        // FREQ=DAILY;INTERVAL=3;BYDAY=TU. lcm(3, 7) = 21.
        // anchor = May 5 2026 (Tue) → next Tuesday that is a multiple of 3 days out.
        let anchor = d(2026, 5, 5); // Tuesday
        assert_eq!(anchor.weekday(), Weekday::Tue);
        let after = d(2026, 5, 5);
        let result = next_occurrence("FREQ=DAILY;INTERVAL=3;BYDAY=TU", anchor, after).unwrap();
        // May 5 + 21 = May 26 (Tuesday); 21 / 3 = 7.
        assert_eq!(result, d(2026, 5, 26));
        assert_eq!(result.weekday(), Weekday::Tue);
        assert_eq!((result - anchor).num_days() % 3, 0);
    }

    #[test]
    fn daily_interval5_no_byday_regression() {
        // Plain FREQ=DAILY;INTERVAL=5 → next aligned day is interval days out.
        let anchor = d(2026, 5, 4);
        let after = d(2026, 5, 4);
        let result = next_occurrence("FREQ=DAILY;INTERVAL=5", anchor, after).unwrap();
        assert_eq!(result, d(2026, 5, 9));
    }

    #[test]
    fn daily_no_byday_regression() {
        // Plain FREQ=DAILY → next day.
        let anchor = d(2026, 5, 4);
        let after = d(2026, 5, 4);
        let result = next_occurrence("FREQ=DAILY", anchor, after).unwrap();
        assert_eq!(result, d(2026, 5, 5));
    }

    // ── spawn_next ──────────────────────────────────────────────────────────

    #[test]
    fn spawn_next_completion_no_snap() {
        // task with due=May 1, completion-based interval=7, today=May 5 → new task due=May 12
        let mut task = Task::new("Water plants");
        task.due = Some(d(2026, 5, 1));
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 7,
            snap: None,
        });
        let today = d(2026, 5, 5);
        let next = spawn_next(&task, today).unwrap().unwrap();
        assert_eq!(next.due, Some(d(2026, 5, 12)));
    }

    #[test]
    fn spawn_next_completion_with_saturday_snap() {
        // interval=7, snap=Sat, today=Wed May 6 → due=next Saturday after May 13
        let mut task = Task::new("Water plants");
        task.due = Some(d(2026, 5, 1));
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 7,
            snap: Some(Snap::NextWeekday { weekday: 5 }),
        });
        let today = d(2026, 5, 6); // Wednesday
        let next = spawn_next(&task, today).unwrap().unwrap();
        // May 6 + 7 = May 13 (Wednesday), snap to next Saturday = May 16
        let due = next.due.unwrap();
        assert_eq!(due.weekday(), Weekday::Sat);
        assert!(due >= today + Duration::days(7));
    }

    #[test]
    fn spawn_next_schedule_preserves_start_due_offset() {
        // task with start=May 1, due=May 3, schedule monthly 1st, today=May 10
        // → new start=Jun 1, due=Jun 3
        let anchor = d(2026, 5, 1);
        let mut task = Task::new("Monthly review");
        task.start = Some(d(2026, 5, 1));
        task.due = Some(d(2026, 5, 3));
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=MONTHLY;BYMONTHDAY=1".into(),
            anchor,
            snap: None,
        });
        let today = d(2026, 5, 10);
        let next = spawn_next(&task, today).unwrap().unwrap();
        assert_eq!(next.start, Some(d(2026, 6, 1)));
        assert_eq!(next.due, Some(d(2026, 6, 3)));
    }

    #[test]
    fn spawn_next_no_spawn_without_recurrence() {
        let task = Task::new("One-off task");
        assert!(spawn_next(&task, d(2026, 5, 10)).unwrap().is_none());
    }

    // ── apply_snap DayOfMonth edge cases ────────────────────────────────────

    #[test]
    fn snap_day_of_month_november_date_to_december_no_panic() {
        // date=Nov 15, dom:1 → Dec 1 (m+1 = 12, must not panic)
        let date = d(2026, 11, 15);
        let result = apply_snap(date, &Snap::DayOfMonth { day: 1 });
        assert_eq!(result, d(2026, 12, 1));
    }

    #[test]
    fn snap_day_of_month_november_end_to_december_no_panic() {
        // date=Nov 30, dom:28 → Dec 28 (next month is December)
        let date = d(2026, 11, 30);
        let result = apply_snap(date, &Snap::DayOfMonth { day: 28 });
        assert_eq!(result, d(2026, 12, 28));
    }

    #[test]
    fn snap_day_of_month_december_wraps_to_january() {
        // date=Dec 15, dom:1 → Jan 1 of next year (December branch)
        let date = d(2026, 12, 15);
        let result = apply_snap(date, &Snap::DayOfMonth { day: 1 });
        assert_eq!(result, d(2027, 1, 1));
    }

    #[test]
    fn snap_day_of_month_december_31_wraps_to_january() {
        // date=Dec 31, dom:1 → Jan 1 next year
        let date = d(2026, 12, 31);
        let result = apply_snap(date, &Snap::DayOfMonth { day: 1 });
        assert_eq!(result, d(2027, 1, 1));
    }

    // ── spawn_next with different date configurations ────────────────────────

    #[test]
    fn spawn_next_only_start_sets_start_on_new() {
        // task with only start, no due → spawned task also gets only start
        let mut task = Task::new("Morning run");
        task.start = Some(d(2026, 5, 1));
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 7,
            snap: None,
        });
        let today = d(2026, 5, 10);
        let next = spawn_next(&task, today).unwrap().unwrap();
        assert!(next.start.is_some(), "spawned task should have a start date");
        assert!(next.due.is_none(), "spawned task should not have a due date");
        assert_eq!(next.start, Some(today + Duration::days(7)));
    }

    #[test]
    fn spawn_next_only_due_sets_due_on_new() {
        // task with only due, no start → spawned task also gets only due
        let mut task = Task::new("Pay bills");
        task.due = Some(d(2026, 5, 1));
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 30,
            snap: None,
        });
        let today = d(2026, 5, 10);
        let next = spawn_next(&task, today).unwrap().unwrap();
        assert!(next.due.is_some(), "spawned task should have a due date");
        assert!(next.start.is_none(), "spawned task should not have a start date");
        assert_eq!(next.due, Some(today + Duration::days(30)));
    }

    #[test]
    fn spawn_next_no_dates_gets_start() {
        // task with no start and no due → spawned task gets a start date
        let mut task = Task::new("Daily standup");
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".into(),
            anchor: d(2026, 5, 4), // Monday
            snap: None,
        });
        let today = d(2026, 5, 8); // Friday
        let next = spawn_next(&task, today).unwrap().unwrap();
        assert!(next.start.is_some(), "spawned task should have a start date");
        assert!(next.due.is_none(), "spawned task should not have a due date");
        assert!(!matches!(next.start.unwrap().weekday(), Weekday::Sat | Weekday::Sun));
    }

    #[test]
    fn spawn_next_does_not_copy_slug() {
        // slug must be None on spawned tasks (per-instance, not series-wide)
        let mut task = Task::new("Weekly review");
        task.slug = Some("weekly-review".into());
        task.due = Some(d(2026, 5, 1));
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 7,
            snap: None,
        });
        let next = spawn_next(&task, d(2026, 5, 5)).unwrap().unwrap();
        assert!(next.slug.is_none(), "spawned task must not copy the slug");
    }

    // ── new unit tests for review findings ──────────────────────────────────

    #[test]
    fn weekly_without_byday_defaults_to_anchor_weekday() {
        // FREQ=WEEKLY without BYDAY should recur on the same weekday as anchor.
        let anchor = d(2026, 5, 4); // Monday
        let after = d(2026, 5, 4);  // same day as anchor
        let result = next_occurrence("FREQ=WEEKLY", anchor, after).unwrap();
        // Should return the NEXT Monday (May 11), not the next day (Tuesday)
        assert_eq!(result, d(2026, 5, 11));
        assert_eq!(result.weekday(), Weekday::Mon);
    }

    #[test]
    fn weekly_interval2_without_byday_respects_anchor_weekday() {
        // FREQ=WEEKLY;INTERVAL=2 without BYDAY → every other Monday starting from anchor.
        let anchor = d(2026, 5, 4); // Monday (week 0)
        let after = d(2026, 5, 11); // Monday (week 1, odd) → next is May 18 (week 2, even)
        let result = next_occurrence("FREQ=WEEKLY;INTERVAL=2", anchor, after).unwrap();
        assert_eq!(result, d(2026, 5, 18));
    }

    #[test]
    fn byday_positional_prefix_is_rejected() {
        // "1MO" (first Monday of month) is not supported — should return an error.
        let anchor = d(2026, 5, 4);
        let after = d(2026, 5, 4);
        let result = next_occurrence("FREQ=MONTHLY;BYDAY=1MO", anchor, after);
        assert!(result.is_err(), "positional BYDAY should be rejected");
    }

    #[test]
    fn spawn_next_copies_data_but_not_time_log() {
        // Custom data keys are preserved; time_log is dropped.
        let mut task = Task::new("Reviewed task");
        task.due = Some(d(2026, 5, 1));
        task.data.insert("ticket".into(), serde_json::Value::String("JIRA-99".into()));
        task.data.insert("time_log".into(), serde_json::json!([{"event": "start"}]));
        task.recurrence = Some(Recurrence::Completion { interval_days: 7, snap: None });
        let next = spawn_next(&task, d(2026, 5, 5)).unwrap().unwrap();
        assert_eq!(next.data.get("ticket").and_then(|v| v.as_str()), Some("JIRA-99"));
        assert!(!next.data.contains_key("time_log"), "time_log must not be copied");
    }

    // ── project_series ──────────────────────────────────────────────────────

    fn weekly_monday_task(anchor: NaiveDate) -> Task {
        let mut task = Task::new("Weekly review");
        task.due = Some(anchor);
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=WEEKLY;BYDAY=MO".into(),
            anchor,
            snap: None,
        });
        task
    }

    #[test]
    fn project_series_weekly_yields_expected_dates_in_horizon() {
        // Anchor Mon May 4 2026; today May 4; 21-day horizon → cutoff May 25.
        // The current instance (May 4) is excluded; projected Mondays strictly
        // after today and within the horizon are May 11, 18, 25.
        let anchor = d(2026, 5, 4);
        let task = weekly_monday_task(anchor);
        let dates = project_series(&task, d(2026, 5, 4), d(2026, 5, 25));
        assert_eq!(dates, vec![d(2026, 5, 11), d(2026, 5, 18), d(2026, 5, 25)]);
    }

    #[test]
    fn project_series_bounded_by_horizon() {
        // A short horizon yields a single projected occurrence.
        let anchor = d(2026, 5, 4);
        let task = weekly_monday_task(anchor);
        let dates = project_series(&task, d(2026, 5, 4), d(2026, 5, 12));
        assert_eq!(dates, vec![d(2026, 5, 11)]);
    }

    #[test]
    fn project_series_applies_snap() {
        // Weekly Monday schedule snapped to next Saturday: each projected Monday
        // is moved forward to the following Saturday.
        let anchor = d(2026, 5, 4); // Monday
        let mut task = weekly_monday_task(anchor);
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=WEEKLY;BYDAY=MO".into(),
            anchor,
            snap: Some(Snap::NextWeekday { weekday: 5 }), // Saturday
        });
        let dates = project_series(&task, d(2026, 5, 4), d(2026, 5, 31));
        // Mondays May 11/18/25 snap to Saturdays May 16/23/30.
        assert_eq!(dates, vec![d(2026, 5, 16), d(2026, 5, 23), d(2026, 5, 30)]);
        assert!(dates.iter().all(|d| d.weekday() == Weekday::Sat));
    }

    #[test]
    fn project_series_completion_type_yields_none() {
        let mut task = Task::new("Water plants");
        task.due = Some(d(2026, 5, 1));
        task.recurrence = Some(Recurrence::Completion { interval_days: 7, snap: None });
        assert!(project_series(&task, d(2026, 5, 4), d(2026, 7, 1)).is_empty());
    }

    #[test]
    fn project_series_done_and_cancelled_yield_none() {
        let anchor = d(2026, 5, 4);
        let mut done = weekly_monday_task(anchor);
        done.status = crate::core::domain::task::Status::Done;
        assert!(project_series(&done, d(2026, 5, 4), d(2026, 6, 1)).is_empty());

        let mut cancelled = weekly_monday_task(anchor);
        cancelled.status = crate::core::domain::task::Status::Cancelled;
        assert!(project_series(&cancelled, d(2026, 5, 4), d(2026, 6, 1)).is_empty());
    }

    #[test]
    fn project_series_non_recurring_yields_none() {
        let mut task = Task::new("One-off");
        task.due = Some(d(2026, 5, 10));
        assert!(project_series(&task, d(2026, 5, 1), d(2026, 7, 1)).is_empty());
    }

    #[test]
    fn spawn_next_late_completion_skips_past_dates() {
        // Task due Apr 1, completed May 30 (overdue by ~60 days).
        // Next occurrence should be after May 30, not after Apr 1.
        let anchor = d(2026, 4, 1);
        let mut task = Task::new("Monthly report");
        task.due = Some(d(2026, 4, 1));
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=MONTHLY;BYMONTHDAY=1".into(),
            anchor,
            snap: None,
        });
        let today = d(2026, 5, 30);
        let next = spawn_next(&task, today).unwrap().unwrap();
        let date = next.due.or(next.start).unwrap();
        assert!(date > today, "spawned date {date} should be after today {today}");
    }
}
