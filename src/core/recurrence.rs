use anyhow::Context as _;
use chrono::{Datelike, Duration, NaiveDate, Weekday};

use crate::core::domain::task::{Recurrence, Snap, SnapLeeway, Task};

// ─── rrule helpers ─────────────────────────────────────────────────────────

fn build_rrule_str(rrule: &str, anchor: NaiveDate) -> String {
    format!(
        "DTSTART:{}\nRRULE:{}",
        anchor.format("%Y%m%dT000000Z"),
        rrule
    )
}

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
                anyhow::bail!("unknown snap {s:?} — try: next-workday, monday … sunday, dom:N")
            }
        }
    }
}

impl Snap {
    /// Whether `date` already sits on a boundary of this snap.
    pub fn qualifies(&self, date: NaiveDate) -> bool {
        match self {
            Snap::NextWeekday { weekday } => {
                date.weekday().num_days_from_monday() as u8 == *weekday % 7
            }
            Snap::NextWorkday => !matches!(date.weekday(), Weekday::Sat | Weekday::Sun),
            Snap::DayOfMonth { day } => date.day() == *day as u32,
        }
    }

    /// The least qualifying date `>= date`.
    ///
    /// Total for every snap `parse_snap` can produce: weekday and workday
    /// boundaries are dense mod 7, and `dom:N` is capped at 1–28 so day *N*
    /// exists in every month. A hand-edited file carrying `dom:29`–`dom:31`
    /// falls back to the last day of a month that is too short, matching the
    /// clamping [`apply_snap`] has always done there.
    pub fn next_boundary(&self, date: NaiveDate) -> NaiveDate {
        match self {
            Snap::NextWeekday { weekday } => {
                let current = date.weekday().num_days_from_monday() as u8;
                date + Duration::days(((weekday + 7 - current) % 7) as i64)
            }
            Snap::NextWorkday => match date.weekday() {
                Weekday::Sat => date + Duration::days(2),
                Weekday::Sun => date + Duration::days(1),
                _ => date,
            },
            Snap::DayOfMonth { day } => {
                let d = *day as u32;
                if let Some(candidate) = NaiveDate::from_ymd_opt(date.year(), date.month(), d) {
                    if candidate >= date {
                        return candidate;
                    }
                }
                let (y, m) = if date.month() == 12 {
                    (date.year() + 1, 1u32)
                } else {
                    (date.year(), date.month() + 1)
                };
                clamped_date(y, m, d)
            }
        }
    }

    /// The greatest qualifying date `<= date`. The mirror of [`Snap::next_boundary`].
    pub fn prev_boundary(&self, date: NaiveDate) -> NaiveDate {
        match self {
            Snap::NextWeekday { weekday } => {
                let current = date.weekday().num_days_from_monday() as u8;
                date - Duration::days(((current + 7 - weekday) % 7) as i64)
            }
            Snap::NextWorkday => match date.weekday() {
                Weekday::Sat => date - Duration::days(1),
                Weekday::Sun => date - Duration::days(2),
                _ => date,
            },
            Snap::DayOfMonth { day } => {
                let d = *day as u32;
                if let Some(candidate) = NaiveDate::from_ymd_opt(date.year(), date.month(), d) {
                    if candidate <= date {
                        return candidate;
                    }
                }
                let (y, m) = if date.month() == 1 {
                    (date.year() - 1, 12u32)
                } else {
                    (date.year(), date.month() - 1)
                };
                clamped_date(y, m, d)
            }
        }
    }
}

/// Move `raw` to a snap boundary, but only as far as `leeway` allows.
///
/// `floor` is the date `raw` was computed from — the completion date for a
/// `Completion` rule, the `after` bound for a `Schedule` one. The result is
/// always strictly greater than it, so a series cannot stall.
///
/// The four outcomes, in the order the code checks them:
///
/// 1. `raw` already sits on a boundary — return it untouched.
/// 2. A boundary is within `leeway.back` and stays above `floor` — pull back.
/// 3. A boundary is within `leeway.forward` (`None` = unbounded) — push forward.
/// 4. Neither is in range — keep `raw`, preserving the interval exactly.
///
/// When both directions qualify the nearer boundary wins, and a tie goes
/// forward: forward can never breach the floor, so the tie rule needs no
/// extra guard, and a symmetric leeway therefore never produces an earlier
/// date than the forward-only [`apply_snap`] would have.
///
/// With [`SnapLeeway::DEFAULT`] this reproduces [`apply_snap`] exactly — the
/// backward branch needs `db <= 0`, which case 1 has already returned on.
pub fn snap_with_leeway(
    raw: NaiveDate,
    snap: &Snap,
    leeway: &SnapLeeway,
    floor: NaiveDate,
) -> NaiveDate {
    if snap.qualifies(raw) {
        return raw;
    }

    let back = snap.prev_boundary(raw);
    let fwd = snap.next_boundary(raw);
    let db = (raw - back).num_days();
    let df = (fwd - raw).num_days();

    // A backward pull must never reach the date the series is stepping from,
    // or the next instance would be due before the one just completed.
    // Validation makes this unreachable through `parse_recurrence`; the check
    // is what makes a hand-edited or git-merged file safe too.
    let back_ok = db <= i64::from(leeway.back) && back > floor;
    let fwd_ok = leeway.forward.is_none_or(|f| df <= i64::from(f));

    match (back_ok, fwd_ok) {
        (true, true) => {
            if db < df {
                back
            } else {
                fwd
            }
        }
        (true, false) => back,
        (false, true) => fwd,
        (false, false) => raw,
    }
}

/// Apply a [`Snap`] to move `date` forward to the nearest qualifying date.
///
/// The pre-leeway rule, kept as the reference [`snap_with_leeway`] is measured
/// against: it is exactly that function under [`SnapLeeway::DEFAULT`], which is
/// what an absent `snap_leeway` means and why no stored task changes date.
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

fn last_day_of_month(year: i32, month: u32) -> NaiveDate {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(ny, nm, 1).unwrap() - Duration::days(1)
}

fn clamped_date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap_or_else(|| last_day_of_month(year, month))
}

// ─── RRULE validation and iteration (via the `rrule` crate, RFC 5545) ───────

/// Parse recurrence arguments into a [`Recurrence`] value.
///
/// `schedule` and `completion` are mutually exclusive; passing both returns an
/// error.  If neither is provided the function returns `Ok(None)`.
///
/// A `completion` interval must be at least one day, mirroring the `INTERVAL
/// >= 1` rule [`validate_rrule`] enforces for schedules.
///
/// `anchor` is the date used as the starting point for schedule-based rules.
/// The caller is responsible for supplying the appropriate anchor (e.g. the
/// task's start/due date or today for `add`, and the existing anchor when
/// editing a task that already has a schedule rule).
pub fn parse_recurrence(
    schedule: Option<String>,
    completion: Option<u32>,
    snap: Option<&str>,
    anchor: NaiveDate,
) -> anyhow::Result<Option<Recurrence>> {
    let snap_val = snap.map(parse_snap).transpose()?;

    match (schedule, completion) {
        (Some(_), Some(_)) => {
            anyhow::bail!("--recur-schedule and --recur-completion are mutually exclusive");
        }
        (Some(rule), None) => {
            // Validate the rule up front so a malformed or unsupported RRULE is
            // rejected at add/edit time rather than failing later on `done`.
            validate_rrule(&rule)
                .map_err(|e| anyhow::anyhow!("invalid recurrence rule {rule:?}: {e}"))?;
            Ok(Some(Recurrence::Schedule {
                rrule: rule,
                anchor,
                snap: snap_val,
                snap_leeway: None,
            }))
        }
        (None, Some(interval)) => {
            // A zero-day interval never advances: every spawned instance would
            // be due the day it was created, forever. Reject it here, where
            // every completion rule is built, rather than letting the series
            // stall long after the rule was accepted.
            anyhow::ensure!(
                interval >= 1,
                "completion interval must be >= 1 day, got {interval}"
            );
            Ok(Some(Recurrence::Completion {
                interval_days: interval,
                snap: snap_val,
                snap_leeway: None,
            }))
        }
        (None, None) => Ok(None),
    }
}

/// Validate a schedule recurrence rule (RRULE) string.
///
/// Parses the rule via the RFC 5545 `rrule` crate, returning an error if the
/// string is malformed (unknown `FREQ`, bad `UNTIL` format, etc.).
///
/// Also rejects `INTERVAL=0` explicitly: the crate accepts it without error
/// but RFC 5545 requires INTERVAL >= 1 and a zero-interval rule never advances.
pub fn validate_rrule(rrule: &str) -> anyhow::Result<()> {
    for part in rrule.split(';') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("INTERVAL=") {
            let n: i64 = val
                .parse()
                .map_err(|_| anyhow::anyhow!("INTERVAL must be a positive integer"))?;
            anyhow::ensure!(n >= 1, "INTERVAL must be >= 1, got {n}");
        }
        if let Some(vals) = part.strip_prefix("BYMONTHDAY=") {
            for token in vals.split(',') {
                let n: i64 = token
                    .trim()
                    .parse()
                    .map_err(|_| anyhow::anyhow!("BYMONTHDAY must be a non-zero integer"))?;
                anyhow::ensure!(
                    n != 0,
                    "BYMONTHDAY=0 is invalid (RFC 5545 requires non-zero)"
                );
            }
        }
    }
    let dummy = NaiveDate::from_ymd_opt(2000, 1, 1).unwrap();
    build_rrule_str(rrule, dummy)
        .parse::<rrule::RRuleSet>()
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Returns the first occurrence of the rule strictly after `after`.
///
/// Per RFC 5545 months/years that have no matching day (e.g. `BYMONTHDAY=31`
/// in February) are **skipped**, not clamped. `UNTIL` and `COUNT` clauses are
/// fully honoured; if the rule is exhausted before a date `> after` is found
/// an error is returned.
pub fn next_occurrence(
    rrule: &str,
    anchor: NaiveDate,
    after: NaiveDate,
) -> anyhow::Result<NaiveDate> {
    let set: rrule::RRuleSet = build_rrule_str(rrule, anchor)
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid RRULE: {e}"))?;

    set.into_iter()
        .find(|dt| dt.naive_utc().date() > after)
        .map(|dt| dt.naive_utc().date())
        .ok_or_else(|| anyhow::anyhow!("RRULE has no occurrence after {after}"))
}

// ─── project_series ──────────────────────────────────────────────────────────

/// Hard cap on the number of dates a single series contributes to a forecast.
///
/// The horizon is the real bound; this keeps one daily series from filling a
/// long forecast on its own. It counts *emitted* dates: a snap that collapses a
/// run of raw occurrences onto one boundary spends one slot, not one per raw
/// step, so a snapped series reaches as far into the horizon as an unsnapped
/// one does.
const MAX_PROJECTED_PER_SERIES: usize = 366;

/// Hard cap on the raw steps taken while filling those dates.
///
/// This is what guarantees termination, and why the emitted cap alone cannot:
/// snapping can collapse arbitrarily many raw occurrences onto one date, so a
/// walk could keep stepping without ever emitting its 366th date. Every step
/// strictly advances the raw date instead, so the walk covers at most this many
/// occurrences of the series and then stops regardless of the horizon. It sits
/// far above the emitted cap so only a pathological rule-and-horizon pair —
/// a daily rule snapped to a monthly boundary, forecast decades out — ever
/// reaches it.
const MAX_SERIES_STEPS: usize = 10_000;

/// Appends `date` unless it repeats the one before it.
///
/// Snapping maps whole runs of raw dates onto the same boundary — a daily rule
/// snapped to Monday hits that Monday five times over — and a forecast wants
/// each date once. Comparing against the last entry removes exactly what
/// `Vec::dedup` would remove at the end of the walk, because both walks emit a
/// non-decreasing series. Doing it as dates are added is what lets
/// [`MAX_PROJECTED_PER_SERIES`] count emitted dates rather than raw steps.
///
/// Leeway does not cost us that ordering, even though it can now pull a date
/// *backward*. The completion walk steps from what it emitted, so it is
/// strictly increasing by construction. In the schedule walk each step's floor
/// is the previous raw date, so a backward pull can only reach a boundary
/// *above* that date — and any such boundary is at or after the forward
/// boundary the previous step could have chosen. Either way the result never
/// goes below its predecessor. `snap_is_monotone_over_the_sweep` checks the
/// underlying property directly.
fn push_deduped(dates: &mut Vec<NaiveDate>, date: NaiveDate) {
    if dates.last() != Some(&date) {
        dates.push(date);
    }
}

/// Enumerate the projected (not-yet-spawned) future occurrences of `task`'s
/// recurrence series with dates `> today` (and `> the current instance`) up to
/// and including `cutoff`.
///
/// Pure and timezone-free: `today` and `cutoff` are passed in so the function is
/// directly testable. Returns an empty list when the task is not an active
/// recurring task.
///
/// - **Schedule** (`RRULE`): dates are deterministic; projected exactly.
/// - **Completion** (`interval_days`): assumes each instance is completed on its
///   due date ("as soon as possible"), so the next due is always
///   `prev_due + interval_days`. This gives a best-case projection.
///
/// The returned dates are strictly increasing: when a snap lands several raw
/// occurrences on the same boundary, that boundary is reported once.
///
/// Shared by both the CLI `forecast` command and the MCP `get_forecast` tool so
/// their projections cannot drift.
pub fn project_series(task: &Task, today: NaiveDate, cutoff: NaiveDate) -> Vec<NaiveDate> {
    if !task.is_active() {
        return Vec::new();
    }

    // The concrete task covers its own due date; start projecting strictly after.
    let base = [task.due, task.start, Some(today)]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(today);

    let leeway = task
        .recurrence
        .as_ref()
        .map_or(&SnapLeeway::DEFAULT, Recurrence::effective_snap_leeway);

    match task.recurrence.as_ref() {
        Some(Recurrence::Schedule {
            rrule,
            anchor,
            snap,
            ..
        }) => {
            // Walk the raw (un-snapped) series so each call strictly advances;
            // snap is applied only to the emitted date. The RRULE *defines* the
            // raw series here, so snapping is a display transform over it — the
            // completion branch below is the one that has to walk what it emits.
            let mut after = base;
            let mut dates = Vec::new();
            for _ in 0..MAX_SERIES_STEPS {
                if dates.len() >= MAX_PROJECTED_PER_SERIES {
                    break;
                }
                let raw = match next_occurrence(rrule, *anchor, after) {
                    Ok(d) => d,
                    Err(_) => break,
                };
                // `next_occurrence` guarantees raw > after, so the walk terminates.
                let occurrence = snap
                    .as_ref()
                    .map_or(raw, |s| snap_with_leeway(raw, s, leeway, after));
                after = raw;
                if occurrence > cutoff {
                    break;
                }
                // Snapping can move a date backwards; only keep strictly-future dates.
                if occurrence > today {
                    push_deduped(&mut dates, occurrence);
                }
            }
            dates
        }
        Some(Recurrence::Completion {
            interval_days,
            snap,
            ..
        }) => {
            // Assume each occurrence is completed on its due date — which is the
            // *snapped* date, since that is what `spawn_next` writes and what
            // the user then completes. So the next step is measured from the
            // date last emitted, not from an un-snapped shadow series that
            // drifts off the boundary and runs a whole period behind reality.
            //
            // The walk still advances strictly: `prev + interval_days` is pulled
            // back by at most `back`, which validation holds below
            // `interval_days`, and `snap_with_leeway`'s floor enforces it for
            // rules that reached the store some other way.
            let mut prev = base;
            let mut dates = Vec::new();
            for _ in 0..MAX_SERIES_STEPS {
                if dates.len() >= MAX_PROJECTED_PER_SERIES {
                    break;
                }
                let raw = prev + Duration::days(*interval_days as i64);
                let occurrence = snap
                    .as_ref()
                    .map_or(raw, |s| snap_with_leeway(raw, s, leeway, prev));
                prev = occurrence;
                if occurrence > cutoff {
                    break;
                }
                if occurrence > today {
                    push_deduped(&mut dates, occurrence);
                }
            }
            dates
        }
        None => Vec::new(),
    }
}

// ─── spawn_next ────────────────────────────────────────────────────────────

/// Creates the next recurring task instance from a completed task.
///
/// Returns `Ok(None)` if the task has no recurrence rule or if the rule is
/// exhausted (UNTIL date passed, COUNT reached). Returns `Err` only for
/// unexpected failures (storage, etc.).
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

    // `after` is also the floor the snap may not pull the new date back to or
    // past: for a completion rule it is the completion date, for a schedule
    // rule the current instance's own date.
    let leeway = recurrence.effective_snap_leeway();

    let occurrence = match recurrence {
        Recurrence::Schedule {
            rrule,
            anchor,
            snap,
            ..
        } => {
            // A "no occurrence" error means the rule is exhausted (UNTIL/COUNT),
            // not a programming error — treat as "nothing to spawn".
            let raw = match next_occurrence(rrule, *anchor, after) {
                Ok(d) => d,
                Err(_) => return Ok(None),
            };
            snap.as_ref()
                .map_or(raw, |s| snap_with_leeway(raw, s, leeway, after))
        }
        Recurrence::Completion {
            interval_days,
            snap,
            ..
        } => {
            let raw = today + Duration::days(*interval_days as i64);
            snap.as_ref()
                .map_or(raw, |s| snap_with_leeway(raw, s, leeway, after))
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

    // ── parse_recurrence ────────────────────────────────────────────────────

    #[test]
    fn parse_recurrence_both_schedule_and_completion_is_error() {
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(Some("FREQ=DAILY".into()), Some(7), None, anchor);
        assert!(
            result.is_err(),
            "should error when both schedule and completion are given"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("mutually exclusive"),
            "error message should mention mutual exclusion: {msg}"
        );
    }

    #[test]
    fn parse_recurrence_neither_returns_none() {
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(None, None, None, anchor).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_recurrence_schedule_no_snap() {
        let anchor = d(2026, 5, 1);
        let result =
            parse_recurrence(Some("FREQ=WEEKLY;BYDAY=MO".into()), None, None, anchor).unwrap();
        match result {
            Some(Recurrence::Schedule {
                rrule,
                anchor: a,
                snap,
                ..
            }) => {
                assert_eq!(rrule, "FREQ=WEEKLY;BYDAY=MO");
                assert_eq!(a, anchor);
                assert!(snap.is_none());
            }
            other => panic!("expected Schedule, got {other:?}"),
        }
    }

    #[test]
    fn parse_recurrence_schedule_with_snap() {
        let anchor = d(2026, 5, 4);
        let result = parse_recurrence(
            Some("FREQ=WEEKLY;BYDAY=MO".into()),
            None,
            Some("friday"),
            anchor,
        )
        .unwrap();
        match result {
            Some(Recurrence::Schedule {
                snap: Some(snap), ..
            }) => {
                assert_eq!(snap, Snap::NextWeekday { weekday: 4 });
            }
            other => panic!("expected Schedule with snap, got {other:?}"),
        }
    }

    #[test]
    fn parse_recurrence_completion_no_snap() {
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(None, Some(14), None, anchor).unwrap();
        match result {
            Some(Recurrence::Completion {
                interval_days,
                snap,
                ..
            }) => {
                assert_eq!(interval_days, 14);
                assert!(snap.is_none());
            }
            other => panic!("expected Completion, got {other:?}"),
        }
    }

    #[test]
    fn parse_recurrence_completion_with_snap() {
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(None, Some(7), Some("next-workday"), anchor).unwrap();
        match result {
            Some(Recurrence::Completion {
                snap: Some(snap), ..
            }) => {
                assert_eq!(snap, Snap::NextWorkday);
            }
            other => panic!("expected Completion with snap, got {other:?}"),
        }
    }

    #[test]
    fn parse_recurrence_completion_zero_interval_is_error() {
        // interval_days: 0 produces a series that never advances.
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(None, Some(0), None, anchor);
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("completion interval must be >= 1 day, got 0"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn parse_recurrence_completion_one_day_is_accepted() {
        // The bound is inclusive: a daily chore is a legitimate rule.
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(None, Some(1), None, anchor).unwrap();
        assert!(matches!(
            result,
            Some(Recurrence::Completion {
                interval_days: 1,
                ..
            })
        ));
    }

    #[test]
    fn parse_recurrence_invalid_snap_value_is_error() {
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(None, Some(7), Some("not-a-snap"), anchor);
        assert!(result.is_err(), "invalid snap should return error");
    }

    #[test]
    fn parse_recurrence_snap_dom_valid() {
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(None, Some(30), Some("dom:15"), anchor).unwrap();
        match result {
            Some(Recurrence::Completion {
                snap: Some(snap), ..
            }) => {
                assert_eq!(snap, Snap::DayOfMonth { day: 15 });
            }
            other => panic!("expected Completion with dom snap, got {other:?}"),
        }
    }

    #[test]
    fn parse_recurrence_snap_dom_out_of_range_is_error() {
        let anchor = d(2026, 5, 1);
        // dom:29 is outside 1–28
        let result = parse_recurrence(None, Some(30), Some("dom:29"), anchor);
        assert!(result.is_err(), "dom:29 should be rejected");
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
        let result =
            next_occurrence("FREQ=MONTHLY;INTERVAL=3;BYMONTHDAY=1", anchor, after).unwrap();
        assert_eq!(result, d(2026, 4, 1));
    }

    #[test]
    fn monthly_interval3_far_future() {
        // anchor=Jan 1, after=Apr 5 → Jul 1
        let anchor = d(2026, 1, 1);
        let after = d(2026, 4, 5);
        let result =
            next_occurrence("FREQ=MONTHLY;INTERVAL=3;BYMONTHDAY=1", anchor, after).unwrap();
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
    fn yearly_bymonthday_without_bymonth_applies_to_all_months() {
        // RFC 5545: FREQ=YEARLY;BYMONTHDAY=15 (no BYMONTH) fires on the 15th of
        // EVERY month each year. Use BYMONTH=3;BYMONTHDAY=15 to mean "March 15".
        let anchor = d(2026, 3, 1);
        // After Mar 5 → Mar 15 (within the same month)
        assert_eq!(
            next_occurrence("FREQ=YEARLY;BYMONTHDAY=15", anchor, d(2026, 3, 5)).unwrap(),
            d(2026, 3, 15)
        );
        // After Mar 20 → Apr 15 (next month's 15th, not March 15 next year)
        assert_eq!(
            next_occurrence("FREQ=YEARLY;BYMONTHDAY=15", anchor, d(2026, 3, 20)).unwrap(),
            d(2026, 4, 15)
        );
    }

    #[test]
    fn yearly_bymonth_bymonthday_for_specific_annual_date() {
        // Correct rule for "March 15 every year": FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=15
        let anchor = d(2026, 3, 15);
        assert_eq!(
            next_occurrence(
                "FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=15",
                anchor,
                d(2026, 3, 20)
            )
            .unwrap(),
            d(2027, 3, 15)
        );
        assert_eq!(
            next_occurrence("FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=15", anchor, d(2026, 3, 5)).unwrap(),
            d(2026, 3, 15)
        );
    }

    // ── month-end skipping (RFC 5545) ───────────────────────────────────────
    //
    // RFC 5545: a BYMONTHDAY value that doesn't exist in a given month causes
    // that month to be skipped entirely (not clamped to the last day).

    #[test]
    fn monthly_jan31_skips_short_months() {
        // FREQ=MONTHLY, anchor Jan 31: February (no day 31) and April (no day 31)
        // are skipped; March 31 and May 31 are returned.
        let anchor = d(2026, 1, 31);
        let result = next_occurrence("FREQ=MONTHLY", anchor, d(2026, 1, 31)).unwrap();
        assert_eq!(result, d(2026, 3, 31)); // Feb skipped
        let result = next_occurrence("FREQ=MONTHLY", anchor, d(2026, 3, 31)).unwrap();
        assert_eq!(result, d(2026, 5, 31)); // Apr skipped
    }

    #[test]
    fn monthly_jan31_skips_february_even_in_leap_year() {
        // February has at most 29 days; day 31 never exists, so Feb is always skipped.
        let anchor = d(2028, 1, 31);
        let result = next_occurrence("FREQ=MONTHLY", anchor, d(2028, 1, 31)).unwrap();
        assert_eq!(result, d(2028, 3, 31)); // Feb 2028 (leap, 29 days) still has no day 31
    }

    #[test]
    fn monthly_bymonthday31_skips_30day_month() {
        // BYMONTHDAY=31 skips April (30 days) → next is May 31.
        let anchor = d(2026, 1, 31);
        let result = next_occurrence("FREQ=MONTHLY;BYMONTHDAY=31", anchor, d(2026, 4, 1)).unwrap();
        assert_eq!(result, d(2026, 5, 31));
    }

    #[test]
    fn monthly_bymonthday_30_31_skips_february() {
        // BYMONTHDAY=30,31: February has neither day, so Feb is skipped entirely.
        // After Jan 31 the next qualifying month is March, yielding Mar 30 (earliest).
        let anchor = d(2026, 1, 30);
        let result =
            next_occurrence("FREQ=MONTHLY;BYMONTHDAY=30,31", anchor, d(2026, 1, 31)).unwrap();
        assert_eq!(result, d(2026, 3, 30));
        let result =
            next_occurrence("FREQ=MONTHLY;BYMONTHDAY=30,31", anchor, d(2026, 2, 28)).unwrap();
        assert_eq!(result, d(2026, 3, 30));
    }

    #[test]
    fn yearly_feb29_skips_non_leap_years() {
        // FREQ=YEARLY, anchor Feb 29 2024: non-leap years have no Feb 29 → skipped.
        // Next after Feb 29 2024 is Feb 29 2028 (next leap year).
        let anchor = d(2024, 2, 29);
        let result = next_occurrence("FREQ=YEARLY", anchor, d(2024, 2, 29)).unwrap();
        assert_eq!(result, d(2028, 2, 29));
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
            snap_leeway: None,
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
            snap_leeway: None,
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
            snap_leeway: None,
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
            snap_leeway: None,
        });
        let today = d(2026, 5, 10);
        let next = spawn_next(&task, today).unwrap().unwrap();
        assert!(
            next.start.is_some(),
            "spawned task should have a start date"
        );
        assert!(
            next.due.is_none(),
            "spawned task should not have a due date"
        );
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
            snap_leeway: None,
        });
        let today = d(2026, 5, 10);
        let next = spawn_next(&task, today).unwrap().unwrap();
        assert!(next.due.is_some(), "spawned task should have a due date");
        assert!(
            next.start.is_none(),
            "spawned task should not have a start date"
        );
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
            snap_leeway: None,
        });
        let today = d(2026, 5, 8); // Friday
        let next = spawn_next(&task, today).unwrap().unwrap();
        assert!(
            next.start.is_some(),
            "spawned task should have a start date"
        );
        assert!(
            next.due.is_none(),
            "spawned task should not have a due date"
        );
        assert!(!matches!(
            next.start.unwrap().weekday(),
            Weekday::Sat | Weekday::Sun
        ));
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
            snap_leeway: None,
        });
        let next = spawn_next(&task, d(2026, 5, 5)).unwrap().unwrap();
        assert!(next.slug.is_none(), "spawned task must not copy the slug");
    }

    // ── new unit tests for review findings ──────────────────────────────────

    #[test]
    fn weekly_without_byday_defaults_to_anchor_weekday() {
        // FREQ=WEEKLY without BYDAY should recur on the same weekday as anchor.
        let anchor = d(2026, 5, 4); // Monday
        let after = d(2026, 5, 4); // same day as anchor
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
    fn byday_positional_prefix_is_supported() {
        // "1MO" (first Monday of month) is now fully supported via the rrule crate.
        // First Monday of May 2026 is May 4 (anchor); first Monday of June 2026 is June 1.
        let anchor = d(2026, 5, 4);
        let after = d(2026, 5, 4);
        let result = next_occurrence("FREQ=MONTHLY;BYDAY=1MO", anchor, after).unwrap();
        assert_eq!(result, d(2026, 6, 1), "first Monday of June 2026");
    }

    #[test]
    fn byday_last_weekday_of_month_supported() {
        // "-1FR" = last Friday of month.
        // Last Friday of June 2026 is June 26; anchor=May 30 (last Friday of May).
        let anchor = d(2026, 5, 29);
        let after = d(2026, 5, 29);
        let result = next_occurrence("FREQ=MONTHLY;BYDAY=-1FR", anchor, after).unwrap();
        // Last Friday of June 2026
        assert_eq!(result.weekday(), Weekday::Fri);
        assert!(result > after);
    }

    #[test]
    fn spawn_next_copies_data_but_not_time_log() {
        // Custom data keys are preserved; time_log is dropped.
        let mut task = Task::new("Reviewed task");
        task.due = Some(d(2026, 5, 1));
        task.data
            .insert("ticket".into(), serde_json::Value::String("JIRA-99".into()));
        task.data
            .insert("time_log".into(), serde_json::json!([{"event": "start"}]));
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 7,
            snap: None,
            snap_leeway: None,
        });
        let next = spawn_next(&task, d(2026, 5, 5)).unwrap().unwrap();
        assert_eq!(
            next.data.get("ticket").and_then(|v| v.as_str()),
            Some("JIRA-99")
        );
        assert!(
            !next.data.contains_key("time_log"),
            "time_log must not be copied"
        );
    }

    // ── UNTIL / COUNT / new RFC 5545 features ───────────────────────────────

    #[test]
    fn rrule_until_limits_occurrences() {
        // UNTIL=20260525T000000Z: last valid occurrence is May 25; no occurrence after.
        let anchor = d(2026, 5, 4); // Monday
        let result = next_occurrence(
            "FREQ=WEEKLY;BYDAY=MO;UNTIL=20260525T000000Z",
            anchor,
            d(2026, 5, 18),
        )
        .unwrap();
        assert_eq!(result, d(2026, 5, 25));

        let result = next_occurrence(
            "FREQ=WEEKLY;BYDAY=MO;UNTIL=20260525T000000Z",
            anchor,
            d(2026, 5, 25),
        );
        assert!(result.is_err(), "no occurrence after UNTIL date");
    }

    #[test]
    fn rrule_count_limits_occurrences() {
        // COUNT=3: occurrences are May 4, May 11, May 18; exhausted after May 18.
        let anchor = d(2026, 5, 4);
        let result =
            next_occurrence("FREQ=WEEKLY;BYDAY=MO;COUNT=3", anchor, d(2026, 5, 11)).unwrap();
        assert_eq!(result, d(2026, 5, 18));

        let result = next_occurrence("FREQ=WEEKLY;BYDAY=MO;COUNT=3", anchor, d(2026, 5, 18));
        assert!(result.is_err(), "no occurrence after COUNT is exhausted");
    }

    #[test]
    fn validate_rrule_accepts_until_clause() {
        assert!(validate_rrule("FREQ=WEEKLY;BYDAY=MO;UNTIL=20261231T000000Z").is_ok());
    }

    #[test]
    fn validate_rrule_accepts_count_clause() {
        assert!(validate_rrule("FREQ=DAILY;COUNT=10").is_ok());
    }

    #[test]
    fn project_series_stops_at_until() {
        // UNTIL=20260525T000000Z: series ends after May 25; cutoff is later.
        let anchor = d(2026, 5, 4);
        let mut task = Task::new("Until test");
        task.due = Some(anchor);
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=WEEKLY;BYDAY=MO;UNTIL=20260525T000000Z".into(),
            anchor,
            snap: None,
            snap_leeway: None,
        });
        let dates = project_series(&task, d(2026, 5, 4), d(2026, 7, 1));
        assert_eq!(dates, vec![d(2026, 5, 11), d(2026, 5, 18), d(2026, 5, 25)]);
    }

    #[test]
    fn project_series_stops_at_count() {
        // COUNT=3: total occurrences May 4/11/18; projected (after today=May 4) are May 11/18.
        let anchor = d(2026, 5, 4);
        let mut task = Task::new("Count test");
        task.due = Some(anchor);
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=WEEKLY;BYDAY=MO;COUNT=3".into(),
            anchor,
            snap: None,
            snap_leeway: None,
        });
        let dates = project_series(&task, d(2026, 5, 4), d(2026, 7, 1));
        assert_eq!(dates, vec![d(2026, 5, 11), d(2026, 5, 18)]);
    }

    // ── project_series ──────────────────────────────────────────────────────

    fn weekly_monday_task(anchor: NaiveDate) -> Task {
        let mut task = Task::new("Weekly review");
        task.due = Some(anchor);
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=WEEKLY;BYDAY=MO".into(),
            anchor,
            snap: None,
            snap_leeway: None,
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
            snap_leeway: None,
        });
        let dates = project_series(&task, d(2026, 5, 4), d(2026, 5, 31));
        // Mondays May 11/18/25 snap to Saturdays May 16/23/30.
        assert_eq!(dates, vec![d(2026, 5, 16), d(2026, 5, 23), d(2026, 5, 30)]);
        assert!(dates.iter().all(|d| d.weekday() == Weekday::Sat));
    }

    #[test]
    fn project_series_completion_type_projects_assuming_done_asap() {
        // Task due May 1, every 7 days. Assuming completed on May 4 (today),
        // projected occurrences: May 11, May 18, ... up to horizon.
        let mut task = Task::new("Water plants");
        task.due = Some(d(2026, 5, 1));
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 7,
            snap: None,
            snap_leeway: None,
        });
        let today = d(2026, 5, 4);
        let cutoff = d(2026, 5, 26);
        let dates = project_series(&task, today, cutoff);
        // base = max(due=May1, today=May4) = May4
        // May4+7=May11, May11+7=May18, May18+7=May25 (≤ May26), May25+7=Jun1 (> cutoff)
        assert_eq!(dates, vec![d(2026, 5, 11), d(2026, 5, 18), d(2026, 5, 25)]);
    }

    #[test]
    fn project_series_completion_type_respects_horizon() {
        let mut task = Task::new("Exercise");
        task.due = Some(d(2026, 6, 1));
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 30,
            snap: None,
            snap_leeway: None,
        });
        let today = d(2026, 6, 1);
        let cutoff = d(2026, 6, 30);
        let dates = project_series(&task, today, cutoff);
        // base = June 1; next = July 1 which is > cutoff June 30 → empty.
        assert!(
            dates.is_empty(),
            "single 30-day interval should exceed 29-day horizon"
        );
    }

    #[test]
    fn project_series_completion_type_overdue_uses_today_as_base() {
        // Task due Apr 1 (past), today is May 4.  Base = max(Apr1, May4) = May4.
        let mut task = Task::new("Overdue chore");
        task.due = Some(d(2026, 4, 1));
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 14,
            snap: None,
            snap_leeway: None,
        });
        let today = d(2026, 5, 4);
        let cutoff = d(2026, 5, 20);
        let dates = project_series(&task, today, cutoff);
        // base = May4; May4+14=May18 (≤ May20); May18+14=Jun1 (> cutoff).
        assert_eq!(dates, vec![d(2026, 5, 18)]);
    }

    #[test]
    fn project_series_snap_collapsing_many_raw_dates_emits_each_once() {
        // A daily schedule snapped to Monday: the seven raw dates of a week all
        // land on the same Monday, which belongs in the forecast once.
        let anchor = d(2026, 5, 4); // Monday
        let mut task = Task::new("Daily, snapped to Monday");
        task.due = Some(anchor);
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=DAILY".into(),
            anchor,
            snap: Some(Snap::NextWeekday { weekday: 0 }),
            snap_leeway: None,
        });
        let dates = project_series(&task, anchor, d(2026, 5, 31));
        assert_eq!(dates, vec![d(2026, 5, 11), d(2026, 5, 18), d(2026, 5, 25)]);
    }

    #[test]
    fn project_series_completion_snap_collapsing_emits_each_once() {
        // Same collapse on the completion branch: a daily chore snapped to the
        // 1st of the month yields one date per month, not one per day.
        let mut task = Task::new("Daily, snapped to the 1st");
        task.due = Some(d(2026, 5, 4));
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 1,
            snap: Some(Snap::DayOfMonth { day: 1 }),
            snap_leeway: None,
        });
        let dates = project_series(&task, d(2026, 5, 4), d(2026, 8, 15));
        assert_eq!(dates, vec![d(2026, 6, 1), d(2026, 7, 1), d(2026, 8, 1)]);
    }

    #[test]
    fn project_series_cap_counts_emitted_dates() {
        // A daily series over a horizon far beyond the cap stops at exactly
        // MAX_PROJECTED_PER_SERIES distinct dates.
        let today = d(2026, 5, 4);
        let mut task = Task::new("Daily chore");
        task.due = Some(today);
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 1,
            snap: None,
            snap_leeway: None,
        });
        let dates = project_series(&task, today, today + Duration::days(3000));
        assert_eq!(dates.len(), MAX_PROJECTED_PER_SERIES);
        assert!(
            dates.windows(2).all(|w| w[0] < w[1]),
            "projected dates must be strictly increasing"
        );
        assert_eq!(dates.last(), Some(&(today + Duration::days(366))));
    }

    #[test]
    fn project_series_collapsing_snap_terminates_on_a_huge_horizon() {
        // The pathological case the step guard exists for: every raw date
        // collapses onto a monthly boundary, so the emitted cap alone would
        // never stop the walk. It must still return, capped and deduplicated.
        let today = d(2026, 5, 4);
        let mut task = Task::new("Daily, snapped to the 1st");
        task.due = Some(today);
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 1,
            snap: Some(Snap::DayOfMonth { day: 1 }),
            snap_leeway: None,
        });
        let dates = project_series(&task, today, d(2126, 1, 1));
        assert!(
            !dates.is_empty() && dates.len() <= MAX_PROJECTED_PER_SERIES,
            "expected a bounded non-empty projection, got {}",
            dates.len()
        );
        assert!(dates.windows(2).all(|w| w[0] < w[1]));
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
            snap_leeway: None,
        });
        let today = d(2026, 5, 30);
        let next = spawn_next(&task, today).unwrap().unwrap();
        let date = next.due.or(next.start).unwrap();
        assert!(
            date > today,
            "spawned date {date} should be after today {today}"
        );
    }

    // ── snap leeway ─────────────────────────────────────────────────────────

    /// Every snap a user can actually configure: `parse_snap` accepts exactly
    /// the seven weekdays, the workday snap, and `dom:1`–`dom:28`.
    fn every_snap() -> Vec<Snap> {
        let mut snaps = vec![Snap::NextWorkday];
        snaps.extend((0..7).map(|weekday| Snap::NextWeekday { weekday }));
        snaps.extend((1..=28).map(|day| Snap::DayOfMonth { day }));
        snaps
    }

    fn leeway(back: u16, forward: Option<u16>) -> SnapLeeway {
        SnapLeeway { back, forward }
    }

    /// A completion-based task with a snap and a leeway, due on `due`.
    fn completion_task(interval_days: u32, snap: Snap, lee: SnapLeeway, due: NaiveDate) -> Task {
        let mut task = Task::new("Pay rent");
        task.due = Some(due);
        task.recurrence = Some(Recurrence::Completion {
            interval_days,
            snap: Some(snap),
            snap_leeway: Some(lee),
        });
        task
    }

    /// T1 — the claim that lets an absent `snap_leeway` need no migration.
    #[test]
    fn default_leeway_reproduces_apply_snap_exactly() {
        // Well below any date under test, so the floor never masks a difference.
        let floor = d(2000, 1, 1);
        let start = d(2024, 1, 1);
        let end = d(2030, 1, 1);
        let snaps = every_snap();

        let mut cases = 0usize;
        let mut date = start;
        while date <= end {
            for snap in &snaps {
                let old = apply_snap(date, snap);
                let new = snap_with_leeway(date, snap, &SnapLeeway::DEFAULT, floor);
                assert_eq!(
                    old, new,
                    "{snap:?} on {date}: apply_snap gave {old}, snap_with_leeway gave {new}"
                );
                cases += 1;
            }
            date += Duration::days(1);
        }
        assert_eq!(
            cases,
            snaps.len() * 2193,
            "the sweep should cover every date/snap pair"
        );
    }

    /// T2 — `project_series` deduplicates adjacent dates, which is only sound
    /// while the snapped series cannot go backwards as the raw one advances.
    #[test]
    fn snap_is_monotone_over_the_sweep() {
        let floor = d(2000, 1, 1);
        let backs = [0u16, 1, 2, 3, 5, 10, 14];
        let forwards = [
            Some(0u16),
            Some(1),
            Some(2),
            Some(3),
            Some(5),
            Some(10),
            Some(14),
            None,
        ];
        let start = d(2026, 1, 1);
        let end = d(2027, 6, 1); // 17 months

        let mut pairs = 0usize;
        for snap in every_snap() {
            for back in backs {
                for forward in forwards {
                    let lee = leeway(back, forward);
                    let mut date = start;
                    let mut prev = snap_with_leeway(date, &snap, &lee, floor);
                    while date < end {
                        date += Duration::days(1);
                        let cur = snap_with_leeway(date, &snap, &lee, floor);
                        assert!(
                            cur >= prev,
                            "{snap:?} {lee:?}: {date} mapped to {cur}, below the previous {prev}"
                        );
                        prev = cur;
                        pairs += 1;
                    }
                }
            }
        }
        assert!(pairs > 200_000, "expected a wide sweep, got {pairs} pairs");
    }

    /// T3 — the floor is what stops a hand-edited file stalling a series.
    #[test]
    fn result_is_always_above_the_floor() {
        let backs = [0u16, 1, 3, 10, 400];
        let start = d(2026, 1, 1);
        let end = d(2027, 1, 1);
        for snap in every_snap() {
            for back in backs {
                for forward in [Some(0u16), Some(3), None] {
                    let lee = leeway(back, forward);
                    let mut raw = start;
                    while raw < end {
                        // The tightest floor a real caller can pass: the raw
                        // date is one day past it.
                        let floor = raw - Duration::days(1);
                        let got = snap_with_leeway(raw, &snap, &lee, floor);
                        assert!(
                            got > floor,
                            "{snap:?} {lee:?}: {raw} snapped to {got} <= {floor}"
                        );
                        raw += Duration::days(1);
                    }
                }
            }
        }
    }

    /// T3, second half — a leeway that validation would reject still cannot
    /// stall the series, because the runtime floor catches it.
    #[test]
    fn back_leeway_wider_than_the_interval_still_advances() {
        // back = 10 against a 3-day interval: unreachable through
        // `parse_recurrence`, reachable by hand-editing a task file.
        let mut task = completion_task(
            3,
            Snap::DayOfMonth { day: 1 },
            leeway(10, Some(10)),
            d(2026, 6, 1),
        );
        let mut today = d(2026, 6, 1);
        for _ in 0..40 {
            let next = spawn_next(&task, today).unwrap().unwrap();
            let due = next.due.unwrap();
            assert!(due > today, "series stalled: {due} is not after {today}");
            today = due;
            task = next;
        }
    }

    /// T4 — the acceptance criterion. Completing one day late must not double
    /// the period.
    #[test]
    fn cadence_holds_across_the_drift_table() {
        // interval 30, snap dom:1. (completed, today's due, leeway 3/3 due)
        let rows = [
            (d(2026, 6, 1), d(2026, 7, 1), d(2026, 7, 1)),
            (d(2026, 6, 2), d(2026, 8, 1), d(2026, 7, 1)),
            (d(2026, 6, 5), d(2026, 8, 1), d(2026, 7, 5)),
            (d(2026, 6, 15), d(2026, 8, 1), d(2026, 7, 15)),
            (d(2026, 6, 30), d(2026, 8, 1), d(2026, 8, 1)),
        ];

        for (completed, legacy_due, leeway_due) in rows {
            let bare = completion_task(
                30,
                Snap::DayOfMonth { day: 1 },
                SnapLeeway::DEFAULT,
                d(2026, 6, 1),
            );
            let got = spawn_next(&bare, completed).unwrap().unwrap().due.unwrap();
            assert_eq!(got, legacy_due, "default leeway, completed {completed}");

            let toleranced = completion_task(
                30,
                Snap::DayOfMonth { day: 1 },
                leeway(3, Some(3)),
                d(2026, 6, 1),
            );
            let got = spawn_next(&toleranced, completed)
                .unwrap()
                .unwrap()
                .due
                .unwrap();
            assert_eq!(got, leeway_due, "leeway 3/3, completed {completed}");
            // The property the table exists to protect: the gap never exceeds
            // the configured interval plus the backward tolerance.
            let gap = (got - completed).num_days();
            assert!(
                gap <= 30 + 3,
                "completed {completed} gave a {gap}-day gap, past interval + back"
            );
        }
    }

    /// T5 — the same failure at a weekly period, over every completion weekday.
    #[test]
    fn weekly_monday_snap_with_two_day_leeway() {
        // May 4 2026 is a Monday, so the sweep starts on one.
        let expected = [
            (d(2026, 5, 4), 7),  // Mon — already on the boundary
            (d(2026, 5, 5), 6),  // Tue — pulled back
            (d(2026, 5, 6), 5),  // Wed — pulled back to the limit
            (d(2026, 5, 7), 7),  // Thu — out of window both ways, kept
            (d(2026, 5, 8), 7),  // Fri — kept
            (d(2026, 5, 9), 9),  // Sat — pushed forward
            (d(2026, 5, 10), 8), // Sun — pushed forward
        ];
        for (completed, gap) in expected {
            let task = completion_task(
                7,
                Snap::NextWeekday { weekday: 0 },
                leeway(2, Some(2)),
                d(2026, 5, 4),
            );
            let due = spawn_next(&task, completed).unwrap().unwrap().due.unwrap();
            assert_eq!(
                (due - completed).num_days(),
                gap,
                "completed {} ({:?}) → {due}",
                completed,
                completed.weekday()
            );
        }
    }

    /// Drives a completion series, always completing one day after the due
    /// date, and returns every due date spawned up to `until`.
    fn simulate_habitual_slip(lee: SnapLeeway, until: NaiveDate) -> Vec<NaiveDate> {
        let mut task = completion_task(30, Snap::DayOfMonth { day: 1 }, lee, d(2026, 6, 1));
        let mut due = d(2026, 6, 1);
        let mut spawned = Vec::new();
        for _ in 0..14 {
            let completed = due + Duration::days(1);
            let next = spawn_next(&task, completed).unwrap().unwrap();
            due = next.due.unwrap();
            if due > until {
                break;
            }
            spawned.push(due);
            task = next;
        }
        spawned
    }

    /// T6 — the headline claim: seven payments a year becomes twelve.
    #[test]
    fn twelve_months_of_habitual_one_day_slip() {
        let horizon = d(2027, 6, 1);

        let with_leeway = simulate_habitual_slip(leeway(3, Some(3)), horizon);
        assert_eq!(
            with_leeway.len(),
            12,
            "leeway 3/3 should hold the monthly cadence, got {with_leeway:?}"
        );
        assert!(
            with_leeway.iter().all(|d| d.day() == 1),
            "every date should stay on the 1st: {with_leeway:?}"
        );

        let bare = simulate_habitual_slip(SnapLeeway::DEFAULT, horizon);
        assert_eq!(
            bare,
            vec![
                d(2026, 8, 1),
                d(2026, 9, 1),
                d(2026, 11, 1),
                d(2027, 1, 1),
                d(2027, 2, 1),
                d(2027, 4, 1),
                d(2027, 6, 1),
            ],
            "the forward-only snap should still skip five cycles"
        );
    }

    /// T7 — D3: an exact tie resolves forward, the branch that can never
    /// breach the floor.
    #[test]
    fn equidistant_boundaries_resolve_forward() {
        let raw = d(2026, 6, 30);
        let snap = Snap::DayOfMonth { day: 15 };
        assert_eq!(snap.prev_boundary(raw), d(2026, 6, 15));
        assert_eq!(snap.next_boundary(raw), d(2026, 7, 15));
        let got = snap_with_leeway(raw, &snap, &leeway(20, Some(20)), d(2026, 6, 1));
        assert_eq!(got, d(2026, 7, 15), "a tie must go forward");
    }

    /// T8 — D5: the schedule arm honours leeway too, without disturbing the
    /// anchor or the raw series the RRULE defines.
    #[test]
    fn schedule_arm_honours_leeway() {
        let anchor = d(2026, 5, 4); // Monday
        let mut task = Task::new("Weekly, snapped to the 1st");
        task.due = Some(anchor);
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=WEEKLY;BYDAY=MO".into(),
            anchor,
            snap: Some(Snap::DayOfMonth { day: 1 }),
            snap_leeway: Some(leeway(2, Some(2))),
        });

        // Mondays after Jun 25 2026: Jun 29, Jul 6, Jul 13 …
        // Jun 29 is two days before Jul 1, so it snaps; the rest stand.
        let spawned = spawn_next(&task, d(2026, 6, 25)).unwrap().unwrap();
        assert_eq!(spawned.due, Some(d(2026, 7, 1)), "within two days → snaps");

        task.due = Some(d(2026, 6, 29));
        let spawned = spawn_next(&task, d(2026, 6, 29)).unwrap().unwrap();
        assert_eq!(
            spawned.due,
            Some(d(2026, 7, 6)),
            "five days from a boundary → the raw Monday stands"
        );

        // The anchor is carried unchanged, so INTERVAL parity is untouched.
        match spawned.recurrence {
            Some(Recurrence::Schedule { anchor: a, .. }) => assert_eq!(a, anchor),
            other => panic!("expected a schedule rule, got {other:?}"),
        }
    }

    /// T12 — D9: the completion forecast must predict what `spawn_next`
    /// actually produces for a punctual user.
    #[test]
    fn projection_agrees_with_spawning_under_a_snap() {
        let base = d(2026, 6, 2);
        let cutoff = d(2026, 12, 31);
        let mut task = completion_task(30, Snap::DayOfMonth { day: 1 }, SnapLeeway::DEFAULT, base);

        let projected = project_series(&task, base, cutoff);

        // The same series, driven for real: complete each instance on its due
        // date, which is the assumption `project_series` documents.
        let mut spawned = Vec::new();
        let mut due = base;
        while spawned.len() < projected.len() {
            let next = spawn_next(&task, due).unwrap().unwrap();
            due = next.due.unwrap();
            if due > cutoff {
                break;
            }
            spawned.push(due);
            task = next;
        }

        assert_eq!(
            projected, spawned,
            "the forecast must match what completing on the due date produces"
        );
        assert_eq!(
            projected,
            vec![
                d(2026, 8, 1),
                d(2026, 9, 1),
                d(2026, 10, 1),
                d(2026, 11, 1),
                d(2026, 12, 1),
            ],
            "the raw walk used to emit 08-01 twice and then run a month behind"
        );
    }

    /// R4 — every boundary primitive is total only because `dom:N` is capped
    /// at a day that exists in every month. Lifting the cap needs
    /// `prev_boundary` revisited, so pin it here.
    #[test]
    fn day_of_month_snap_is_capped_at_28() {
        assert!(parse_snap("dom:28").is_ok());
        assert!(parse_snap("dom:29").is_err());
        assert!(parse_snap("dom:0").is_err());
        // February 2027 has 28 days, so the cap is exactly the bound that makes
        // day N exist in every month.
        for day in 1..=28u8 {
            let snap = Snap::DayOfMonth { day };
            let date = d(2027, 2, 14);
            assert!(snap.qualifies(snap.next_boundary(date)));
            assert!(snap.qualifies(snap.prev_boundary(date)));
        }
    }

    /// The primitives must bracket their input, or `snap_with_leeway`'s
    /// distance arithmetic is meaningless.
    #[test]
    fn boundaries_bracket_their_input() {
        for snap in every_snap() {
            let mut date = d(2026, 1, 1);
            while date < d(2027, 1, 1) {
                let prev = snap.prev_boundary(date);
                let next = snap.next_boundary(date);
                assert!(prev <= date, "{snap:?}: prev {prev} above {date}");
                assert!(next >= date, "{snap:?}: next {next} below {date}");
                assert!(snap.qualifies(prev), "{snap:?}: {prev} is not a boundary");
                assert!(snap.qualifies(next), "{snap:?}: {next} is not a boundary");
                if snap.qualifies(date) {
                    assert_eq!(prev, date);
                    assert_eq!(next, date);
                }
                date += Duration::days(1);
            }
        }
    }
}
