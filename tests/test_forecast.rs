mod common;

use chrono::{Duration, Local, NaiveDate};
use next::cli::commands::{add, forecast};
use next::core::domain::task::{Recurrence, Snap, Status, Task};

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
        long_term: false,
        adjust: None,
        json: false,
    }
}

fn forecast_args(days: Option<u32>) -> forecast::Args {
    forecast::Args {
        days,
        all: false,
        all_users: false,
        json: false,
        tokens: vec![],
    }
}

fn due_in(n: i64) -> String {
    (Local::now().date_naive() + chrono::Duration::days(n))
        .format("%Y-%m-%d")
        .to_string()
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// The projected dates produced by the forecast for a given task title.
fn projected_dates(env: &common::TestEnv, days: Option<u32>, title: &str) -> Vec<NaiveDate> {
    let (entries, _) = forecast::build_entries(&forecast_args(days), &env.ctx).unwrap();
    let mut dates: Vec<NaiveDate> = entries
        .iter()
        .filter(|e| e.projected && e.title == title)
        .map(|e| e.date)
        .collect();
    dates.sort();
    dates
}

/// The concrete (non-projected) forecast entries' dates for a given task title.
fn concrete_dates(env: &common::TestEnv, days: Option<u32>, title: &str) -> Vec<NaiveDate> {
    let (entries, _) = forecast::build_entries(&forecast_args(days), &env.ctx).unwrap();
    entries
        .iter()
        .filter(|e| !e.projected && e.title == title)
        .map(|e| e.date)
        .collect()
}

/// Save a task directly into the store (used to set up recurring fixtures with
/// fully controlled dates/anchors, independent of `add`'s date parsing).
fn save(env: &mut common::TestEnv, task: &Task) {
    env.ctx.repo.store.save_task(task).unwrap();
}

// ---------------------------------------------------------------------------
// Basic forecast
// ---------------------------------------------------------------------------

#[test]
fn forecast_empty_when_no_tasks() {
    let env = common::setup();
    forecast::run(forecast_args(None), &env.ctx).unwrap();
}

#[test]
fn forecast_empty_when_no_due_dates() {
    let mut env = common::setup();
    add::run(add_args("No due date"), &mut env.ctx).unwrap();
    forecast::run(forecast_args(None), &env.ctx).unwrap();
}

#[test]
fn forecast_shows_tasks_due_within_horizon() {
    let mut env = common::setup();
    add::run(
        add::Args { due: Some(due_in(5)), ..add_args("Due soon") },
        &mut env.ctx,
    )
    .unwrap();
    forecast::run(forecast_args(Some(30)), &env.ctx).unwrap();
    // Task with due date within horizon must be in the store.
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(task.due.is_some());
}

#[test]
fn forecast_excludes_tasks_beyond_horizon() {
    let mut env = common::setup();
    add::run(
        add::Args { due: Some(due_in(200)), ..add_args("Far future task") },
        &mut env.ctx,
    )
    .unwrap();
    // With a 90-day horizon the task is beyond the cutoff — no error.
    forecast::run(forecast_args(Some(90)), &env.ctx).unwrap();
}

#[test]
fn forecast_json_output() {
    let mut env = common::setup();
    add::run(
        add::Args { due: Some(due_in(3)), ..add_args("JSON forecast task") },
        &mut env.ctx,
    )
    .unwrap();
    forecast::run(
        forecast::Args { json: true, ..forecast_args(Some(30)) },
        &env.ctx,
    )
    .unwrap();
}

#[test]
fn forecast_overdue_task_included() {
    let mut env = common::setup();
    add::run(
        add::Args { due: Some(due_in(-3)), ..add_args("Overdue task") },
        &mut env.ctx,
    )
    .unwrap();
    forecast::run(forecast_args(Some(90)), &env.ctx).unwrap();
}

// ---------------------------------------------------------------------------
// Unicode safety
// ---------------------------------------------------------------------------

#[test]
fn forecast_unicode_title_does_not_panic() {
    let mut env = common::setup();
    add::run(
        add::Args {
            due: Some(due_in(2)),
            ..add_args("タスク: 日本語のタイトルは長くなることがある — emoji 🎉🔥")
        },
        &mut env.ctx,
    )
    .unwrap();
    // This must not panic even though the title is > 45 bytes and contains multibyte chars.
    forecast::run(forecast_args(Some(30)), &env.ctx).unwrap();
}

// ---------------------------------------------------------------------------
// Recurrence-series projection (REQUIREMENTS §7.4)
// ---------------------------------------------------------------------------

/// Build an active schedule-recurring task with `due = today + due_offset` and
/// `anchor = due`, so projection is deterministic relative to today.
fn schedule_task(title: &str, rrule: &str, due_offset: i64, snap: Option<Snap>) -> Task {
    let due = today() + Duration::days(due_offset);
    let mut t = Task::new(title);
    t.due = Some(due);
    t.recurrence = Some(Recurrence::Schedule {
        rrule: rrule.into(),
        anchor: due,
        snap,
    });
    t
}

#[test]
fn weekly_schedule_projects_multiple_occurrences_in_horizon() {
    let mut env = common::setup();
    // due = today + 3; FREQ=WEEKLY anchored on that weekday.
    let task = schedule_task("Weekly review", "FREQ=WEEKLY", 3, None);
    save(&mut env, &task);

    let projected = projected_dates(&env, Some(90), "Weekly review");
    // due+3 is the concrete instance; projections are due+10, due+17, … up to
    // today+90. With base = today+3: today+10, +17, … +87 → floor(87/7)=12.
    assert_eq!(projected.len(), 12, "expected 12 projected weekly occurrences");
    // First two projected occurrences are exactly one and two weeks out from due.
    assert_eq!(projected[0], today() + Duration::days(10));
    assert_eq!(projected[1], today() + Duration::days(17));
    // The concrete instance itself is present and NOT projected.
    assert_eq!(concrete_dates(&env, Some(90), "Weekly review"), vec![today() + Duration::days(3)]);
    // All projected dates lie within the horizon.
    let cutoff = today() + Duration::days(90);
    assert!(projected.iter().all(|d| *d <= cutoff));
}

#[test]
fn monthly_schedule_projects_monthly_dates_in_horizon() {
    let mut env = common::setup();
    // Anchor on the 1st-of-month series; due = today + 2 for determinism of the
    // concrete instance, but the rule pins occurrences to BYMONTHDAY=1.
    let due = today() + Duration::days(2);
    let mut task = Task::new("Monthly bill");
    task.due = Some(due);
    task.recurrence = Some(Recurrence::Schedule {
        rrule: "FREQ=MONTHLY;BYMONTHDAY=1".into(),
        anchor: due,
        snap: None,
    });
    save(&mut env, &task);

    let projected = projected_dates(&env, Some(120), "Monthly bill");
    // Within a 120-day window there are 3 or 4 first-of-month occurrences after
    // `due`. They must all be the 1st of a month and strictly increasing.
    assert!(
        (3..=4).contains(&projected.len()),
        "expected 3–4 monthly occurrences, got {}",
        projected.len()
    );
    use chrono::Datelike;
    assert!(projected.iter().all(|d| d.day() == 1), "all on the 1st");
    for w in projected.windows(2) {
        assert!(w[1] > w[0], "monthly dates strictly increasing");
    }
    let cutoff = today() + Duration::days(120);
    assert!(projected.iter().all(|d| *d <= cutoff));
}

#[test]
fn projection_stops_at_horizon() {
    let mut env = common::setup();
    let task = schedule_task("Weekly stop", "FREQ=WEEKLY", 1, None);
    save(&mut env, &task);

    // A short 20-day horizon: base = today+1, projections at +8, +15 (not +22).
    let projected = projected_dates(&env, Some(20), "Weekly stop");
    let cutoff = today() + Duration::days(20);
    assert!(projected.iter().all(|d| *d <= cutoff), "nothing beyond the horizon");
    assert_eq!(projected, vec![today() + Duration::days(8), today() + Duration::days(15)]);
}

#[test]
fn completion_recurring_task_is_projected_assuming_done_asap() {
    let mut env = common::setup();
    let due = today() + Duration::days(5);
    let mut task = Task::new("Water plants");
    task.due = Some(due);
    task.recurrence = Some(Recurrence::Completion { interval_days: 7, snap: None });
    save(&mut env, &task);

    // The current instance shows as a concrete entry.
    assert_eq!(concrete_dates(&env, Some(90), "Water plants"), vec![due]);
    // Projected entries assume completion on the due date; first projection = due + 7.
    let projected = projected_dates(&env, Some(90), "Water plants");
    assert!(!projected.is_empty(), "completion series should now project");
    assert_eq!(projected[0], due + Duration::days(7));
    // Subsequent projections advance by interval_days each step.
    for w in projected.windows(2) {
        assert_eq!(w[1] - w[0], Duration::days(7));
    }
}

#[test]
fn non_recurring_due_task_appears_as_concrete_only() {
    let mut env = common::setup();
    add::run(
        add::Args { due: Some(due_in(10)), ..add_args("One-off") },
        &mut env.ctx,
    )
    .unwrap();

    assert!(projected_dates(&env, Some(90), "One-off").is_empty());
    assert_eq!(concrete_dates(&env, Some(90), "One-off"), vec![today() + Duration::days(10)]);
}

#[test]
fn done_recurring_task_is_not_projected() {
    let mut env = common::setup();
    let mut task = schedule_task("Done weekly", "FREQ=WEEKLY", 3, None);
    task.status = Status::Done;
    save(&mut env, &task);

    // A done (or cancelled) series is not active, so it is never projected.
    assert!(projected_dates(&env, Some(90), "Done weekly").is_empty());
}

#[test]
fn cancelled_recurring_task_is_not_projected() {
    let mut env = common::setup();
    let mut task = schedule_task("Cancelled weekly", "FREQ=WEEKLY", 3, None);
    task.status = Status::Cancelled;
    save(&mut env, &task);

    assert!(projected_dates(&env, Some(90), "Cancelled weekly").is_empty());
}

#[test]
fn projected_entries_are_marked_distinct_from_concrete() {
    let mut env = common::setup();
    let task = schedule_task("Marker check", "FREQ=WEEKLY", 2, None);
    save(&mut env, &task);

    let (entries, _) = forecast::build_entries(&forecast_args(Some(40)), &env.ctx).unwrap();
    let mine: Vec<_> = entries.iter().filter(|e| e.title == "Marker check").collect();
    // Exactly one concrete (the current instance) plus several projected ones.
    let concrete = mine.iter().filter(|e| !e.projected).count();
    let projected = mine.iter().filter(|e| e.projected).count();
    assert_eq!(concrete, 1, "one concrete current instance");
    assert!(projected >= 4, "several projected future occurrences");

    // The serialized JSON exposes the `projected` flag so callers can style it.
    let json = serde_json::to_string(&entries).unwrap();
    assert!(json.contains("\"projected\":true"));
    assert!(json.contains("\"projected\":false"));
}

#[test]
fn projection_respects_snap() {
    let mut env = common::setup();
    // Weekly series snapped to the next Monday. Every projected date must land
    // on a Monday regardless of the raw occurrence weekday.
    let task = schedule_task("Snapped weekly", "FREQ=WEEKLY", 1, Some(Snap::NextWeekday { weekday: 0 }));
    save(&mut env, &task);

    use chrono::{Datelike, Weekday};
    let projected = projected_dates(&env, Some(60), "Snapped weekly");
    assert!(!projected.is_empty());
    assert!(projected.iter().all(|d| d.weekday() == Weekday::Mon), "all snapped to Monday");
}

#[test]
fn forecast_typoed_flag_in_filter_tokens_rejected() {
    // An unknown `--flag` swallowed into the trailing filter tokens must
    // error clearly instead of being misread as a tag exclusion.
    let env = common::setup();
    let err = forecast::run(
        forecast::Args { tokens: vec!["--jsn".to_string()], ..forecast_args(None) },
        &env.ctx,
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("unrecognised flag"), "unexpected error: {msg}");
    assert!(msg.contains("--jsn"), "error must name the token: {msg}");
}
