use chrono::NaiveDate;

use crate::core::{
    domain::task::Recurrence,
    recurrence::{parse_snap, validate_rrule},
};

/// Parse recurrence arguments into a [`Recurrence`] value.
///
/// `schedule` and `completion` are mutually exclusive; passing both returns an
/// error.  If neither is provided the function returns `Ok(None)`.
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
            anyhow::bail!(
                "--recur-schedule and --recur-completion are mutually exclusive"
            );
        }
        (Some(rule), None) => {
            // Validate the rule up front so a malformed or unsupported RRULE is
            // rejected at add/edit time rather than failing later on `done`.
            validate_rrule(&rule).map_err(|e| {
                anyhow::anyhow!("invalid recurrence rule {rule:?}: {e}")
            })?;
            Ok(Some(Recurrence::Schedule {
                rrule: rule,
                anchor,
                snap: snap_val,
            }))
        }
        (None, Some(interval)) => Ok(Some(Recurrence::Completion {
            interval_days: interval,
            snap: snap_val,
        })),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn both_schedule_and_completion_is_error() {
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(
            Some("FREQ=DAILY".into()),
            Some(7),
            None,
            anchor,
        );
        assert!(result.is_err(), "should error when both schedule and completion are given");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("mutually exclusive"), "error message should mention mutual exclusion: {msg}");
    }

    #[test]
    fn neither_returns_none() {
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(None, None, None, anchor).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn schedule_no_snap() {
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(
            Some("FREQ=WEEKLY;BYDAY=MO".into()),
            None,
            None,
            anchor,
        )
        .unwrap();
        match result {
            Some(Recurrence::Schedule { rrule, anchor: a, snap }) => {
                assert_eq!(rrule, "FREQ=WEEKLY;BYDAY=MO");
                assert_eq!(a, anchor);
                assert!(snap.is_none());
            }
            other => panic!("expected Schedule, got {other:?}"),
        }
    }

    #[test]
    fn schedule_with_snap() {
        let anchor = d(2026, 5, 4);
        let result = parse_recurrence(
            Some("FREQ=WEEKLY;BYDAY=MO".into()),
            None,
            Some("friday"),
            anchor,
        )
        .unwrap();
        match result {
            Some(Recurrence::Schedule { snap: Some(snap), .. }) => {
                use crate::core::domain::task::Snap;
                assert_eq!(snap, Snap::NextWeekday { weekday: 4 });
            }
            other => panic!("expected Schedule with snap, got {other:?}"),
        }
    }

    #[test]
    fn completion_no_snap() {
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(None, Some(14), None, anchor).unwrap();
        match result {
            Some(Recurrence::Completion { interval_days, snap }) => {
                assert_eq!(interval_days, 14);
                assert!(snap.is_none());
            }
            other => panic!("expected Completion, got {other:?}"),
        }
    }

    #[test]
    fn completion_with_snap() {
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(None, Some(7), Some("next-workday"), anchor).unwrap();
        match result {
            Some(Recurrence::Completion { snap: Some(snap), .. }) => {
                use crate::core::domain::task::Snap;
                assert_eq!(snap, Snap::NextWorkday);
            }
            other => panic!("expected Completion with snap, got {other:?}"),
        }
    }

    #[test]
    fn invalid_snap_value_is_error() {
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(None, Some(7), Some("not-a-snap"), anchor);
        assert!(result.is_err(), "invalid snap should return error");
    }

    #[test]
    fn snap_dom_valid() {
        let anchor = d(2026, 5, 1);
        let result = parse_recurrence(None, Some(30), Some("dom:15"), anchor).unwrap();
        match result {
            Some(Recurrence::Completion { snap: Some(snap), .. }) => {
                use crate::core::domain::task::Snap;
                assert_eq!(snap, Snap::DayOfMonth { day: 15 });
            }
            other => panic!("expected Completion with dom snap, got {other:?}"),
        }
    }

    #[test]
    fn snap_dom_out_of_range_is_error() {
        let anchor = d(2026, 5, 1);
        // dom:29 is outside 1–28
        let result = parse_recurrence(None, Some(30), Some("dom:29"), anchor);
        assert!(result.is_err(), "dom:29 should be rejected");
    }
}
