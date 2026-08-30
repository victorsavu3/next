mod common;

use chrono::{Datelike, NaiveDate, Weekday};

use next::cli::commands::{add, done};
use next::core::domain::task::{Recurrence, Status};
use next::core::recurrence::project_series;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

fn add_args(title: &str) -> add::Args {
    add::Args {
        title: title.to_string(),
        due: None,
        start: None,
        priority: None,
        slug: None,
        assignee: None,
        tags: vec![],
        parent: None,
        blocked_by: vec![],
        description: None,
        url: None,
        notes: None,
        recur_schedule: None,
        recur_completion: None,
        recur_snap: None,
        recur_snap_leeway: None,
        quiet: false,
        long_term: false,
        adjust: None,
        json: false,
    }
}

// ── UNTIL: task stops spawning after the UNTIL date ─────────────────────────

#[test]
fn until_rule_accepted_at_add_time() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("weekly-until".into()),
            due: Some("2026-05-04".into()),
            recur_schedule: Some("FREQ=WEEKLY;BYDAY=MO;UNTIL=20260525T000000Z".into()),
            ..add_args("Until task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = env.ctx.repo.store.list_tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert!(
        matches!(&task.recurrence, Some(Recurrence::Schedule { rrule, .. }) if rrule.contains("UNTIL"))
    );
}

#[test]
fn until_rule_stops_spawning_after_last_occurrence() {
    let mut env = common::setup();
    // Three occurrences: May 4, May 11, May 18, May 25. After completing May 18,
    // the next occurrence is May 25. Completing May 25 should NOT spawn a new task.
    add::run(
        add::Args {
            slug: Some("until-stop".into()),
            due: Some("2026-05-04".into()),
            recur_schedule: Some("FREQ=WEEKLY;BYDAY=MO;UNTIL=20260525T000000Z".into()),
            ..add_args("Until stop task")
        },
        &mut env.ctx,
    )
    .unwrap();

    // Complete May 4 → spawns May 11.
    done::run(
        done::Args {
            id: "until-stop".into(),
            completed_at: Some("2026-05-04".into()),
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();
    let tasks: Vec<_> = env
        .ctx
        .repo
        .store
        .list_tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.status == Status::Open || t.status == Status::Started)
        .collect();
    assert_eq!(tasks.len(), 1);
    let next_task = &tasks[0];
    let next_due = next_task.due.unwrap();
    assert_eq!(next_due, d(2026, 5, 11));

    // Complete the remaining occurrences up through May 25.
    done::run(
        done::Args {
            id: next_task.id.to_string(),
            completed_at: Some("2026-05-11".into()),
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();
    // Find the May 18 task and complete it.
    let open: Vec<_> = env
        .ctx
        .repo
        .store
        .list_tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.status == Status::Open)
        .collect();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].due.unwrap(), d(2026, 5, 18));

    done::run(
        done::Args {
            id: open[0].id.to_string(),
            completed_at: Some("2026-05-18".into()),
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    // Find May 25 task (last allowed by UNTIL) and complete it.
    let open: Vec<_> = env
        .ctx
        .repo
        .store
        .list_tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.status == Status::Open)
        .collect();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].due.unwrap(), d(2026, 5, 25));

    done::run(
        done::Args {
            id: open[0].id.to_string(),
            completed_at: Some("2026-05-25".into()),
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    // No more open tasks — UNTIL was reached.
    let open: Vec<_> = env
        .ctx
        .repo
        .store
        .list_tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.status == Status::Open)
        .collect();
    assert!(
        open.is_empty(),
        "no tasks should remain after UNTIL: {open:?}"
    );
}

// ── COUNT: task stops spawning after N total occurrences ─────────────────────

#[test]
fn count_rule_accepted_at_add_time() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("count-task".into()),
            due: Some("2026-05-04".into()),
            recur_schedule: Some("FREQ=WEEKLY;BYDAY=MO;COUNT=3".into()),
            ..add_args("Count task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = env.ctx.repo.store.list_tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(
        matches!(&tasks[0].recurrence, Some(Recurrence::Schedule { rrule, .. }) if rrule.contains("COUNT"))
    );
}

#[test]
fn count_rule_stops_spawning_after_n_completions() {
    // COUNT=2: two occurrences total (May 4, May 11). After completing both, no new task.
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("count-2".into()),
            due: Some("2026-05-04".into()),
            recur_schedule: Some("FREQ=WEEKLY;BYDAY=MO;COUNT=2".into()),
            ..add_args("Count=2 task")
        },
        &mut env.ctx,
    )
    .unwrap();

    // Complete first occurrence (May 4) → spawns second (May 11).
    done::run(
        done::Args {
            id: "count-2".into(),
            completed_at: Some("2026-05-04".into()),
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    let open: Vec<_> = env
        .ctx
        .repo
        .store
        .list_tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.status == Status::Open)
        .collect();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].due.unwrap(), d(2026, 5, 11));

    // Complete second (and last) occurrence (May 11) → no new task.
    done::run(
        done::Args {
            id: open[0].id.to_string(),
            completed_at: Some("2026-05-11".into()),
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    let open: Vec<_> = env
        .ctx
        .repo
        .store
        .list_tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.status == Status::Open)
        .collect();
    assert!(
        open.is_empty(),
        "no tasks should remain after COUNT=2: {open:?}"
    );
}

// ── Positional BYDAY (1MO, -1FR) ────────────────────────────────────────────

#[test]
fn first_monday_of_month_rule_accepted_and_spawns_correctly() {
    let mut env = common::setup();
    // First Monday of May 2026 is May 4.
    add::run(
        add::Args {
            slug: Some("first-monday".into()),
            due: Some("2026-05-04".into()),
            recur_schedule: Some("FREQ=MONTHLY;BYDAY=1MO".into()),
            ..add_args("First Monday")
        },
        &mut env.ctx,
    )
    .unwrap();

    done::run(
        done::Args {
            id: "first-monday".into(),
            completed_at: Some("2026-05-04".into()),
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    let open: Vec<_> = env
        .ctx
        .repo
        .store
        .list_tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.status == Status::Open)
        .collect();
    assert_eq!(open.len(), 1);
    let next_due = open[0].due.unwrap();
    // First Monday of June 2026 is June 1.
    assert_eq!(
        next_due.weekday(),
        Weekday::Mon,
        "next due must be a Monday"
    );
    assert!(
        next_due > d(2026, 5, 31),
        "next due must be in June or later"
    );
    assert!(
        next_due <= d(2026, 6, 7),
        "next due must be within the first week of June"
    );
}

#[test]
fn last_friday_of_month_rule_accepted_and_spawns_correctly() {
    let mut env = common::setup();
    // Last Friday of May 2026 is May 29.
    add::run(
        add::Args {
            slug: Some("last-friday".into()),
            due: Some("2026-05-29".into()),
            recur_schedule: Some("FREQ=MONTHLY;BYDAY=-1FR".into()),
            ..add_args("Last Friday")
        },
        &mut env.ctx,
    )
    .unwrap();

    done::run(
        done::Args {
            id: "last-friday".into(),
            completed_at: Some("2026-05-29".into()),
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    let open: Vec<_> = env
        .ctx
        .repo
        .store
        .list_tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.status == Status::Open)
        .collect();
    assert_eq!(open.len(), 1);
    let next_due = open[0].due.unwrap();
    // Last Friday of June 2026 is June 26.
    assert_eq!(
        next_due.weekday(),
        Weekday::Fri,
        "next due must be a Friday"
    );
    assert!(
        next_due >= d(2026, 6, 24),
        "must be within last week of June"
    );
    assert!(next_due <= d(2026, 6, 30));
}

// ── project_series respects UNTIL and COUNT ──────────────────────────────────

#[test]
fn project_series_until_stops_within_horizon() {
    let anchor = d(2026, 5, 4);
    let mut task = next::core::domain::task::Task::new("Until series");
    task.due = Some(anchor);
    task.recurrence = Some(Recurrence::Schedule {
        rrule: "FREQ=WEEKLY;BYDAY=MO;UNTIL=20260525T000000Z".into(),
        anchor,
        snap: None,
        snap_leeway: None,
    });
    // Horizon extends to July; UNTIL cuts the series at May 25.
    let dates = project_series(&task, d(2026, 5, 4), d(2026, 7, 1));
    assert_eq!(dates, vec![d(2026, 5, 11), d(2026, 5, 18), d(2026, 5, 25)]);
}

#[test]
fn project_series_count_stops_at_limit() {
    let anchor = d(2026, 5, 4);
    let mut task = next::core::domain::task::Task::new("Count series");
    task.due = Some(anchor);
    task.recurrence = Some(Recurrence::Schedule {
        rrule: "FREQ=WEEKLY;BYDAY=MO;COUNT=3".into(),
        anchor,
        snap: None,
        snap_leeway: None,
    });
    // COUNT=3 means May 4, May 11, May 18. The current instance (May 4) is covered
    // by the task itself; projected occurrences are May 11 and May 18.
    let dates = project_series(&task, d(2026, 5, 4), d(2026, 7, 1));
    assert_eq!(dates, vec![d(2026, 5, 11), d(2026, 5, 18)]);
}

// ── BYMONTH: annual recurrence on a specific month ───────────────────────────

#[test]
fn yearly_bymonth_bymonthday_rule_accepted_and_spawns_correctly() {
    let mut env = common::setup();
    // Recur every year on March 15.
    add::run(
        add::Args {
            slug: Some("annual-march15".into()),
            due: Some("2026-03-15".into()),
            recur_schedule: Some("FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=15".into()),
            ..add_args("Annual March 15")
        },
        &mut env.ctx,
    )
    .unwrap();

    done::run(
        done::Args {
            id: "annual-march15".into(),
            completed_at: Some("2026-03-15".into()),
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    let open: Vec<_> = env
        .ctx
        .repo
        .store
        .list_tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.status == Status::Open)
        .collect();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].due.unwrap(), d(2027, 3, 15));
}
