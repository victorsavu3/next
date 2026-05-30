mod common;

use chrono::{Datelike, Duration, Local, NaiveDate, Weekday};
use next::cli::commands::{add, done};
use next::domain::task::Status;

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

fn done_args(id: &str) -> done::Args {
    done::Args {
        id: id.to_string(),
        json: false,
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Returns all tasks regardless of status.
fn all_tasks(env: &common::TestEnv) -> Vec<next::domain::task::Task> {
    env.ctx.store.list_tasks().unwrap()
}

// ── tests ────────────────────────────────────────────────────────────────────

/// 1. Recurring weekday task spawns a next instance whose due/start is Mon-Fri.
#[test]
fn recur_every_workday_spawns_next_on_workday() {
    let mut env = common::setup();
    let anchor = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(); // Monday
    add::run(
        add::Args {
            due: Some("2026-06-01".to_string()),
            recur_schedule: Some("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".to_string()),
            ..add_args("Daily standup")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = all_tasks(&env).remove(0);
    let id = task.id.to_string();
    done::run(done_args(&id), &mut env.ctx).unwrap();

    let tasks = all_tasks(&env);
    assert_eq!(tasks.len(), 2, "expected 2 tasks (done + new)");

    let new_task = tasks.iter().find(|t| t.status == Status::Open).expect("no open task");
    let date = new_task.due.or(new_task.start).expect("no date on new task");
    assert!(
        !matches!(date.weekday(), Weekday::Sat | Weekday::Sun),
        "spawned task is on a weekend: {date}"
    );
    // Ensure it's after the anchor
    assert!(date > anchor, "spawned task not after anchor");
}

/// 2. Monthly task: start on 1st, due on 3rd → next instance has same day offset.
#[test]
fn recur_monthly_start_1st_due_3rd() {
    let mut env = common::setup();
    add::run(
        add::Args {
            start: Some("2026-06-01".to_string()),
            due: Some("2026-06-03".to_string()),
            recur_schedule: Some("FREQ=MONTHLY;BYMONTHDAY=1".to_string()),
            ..add_args("Monthly review")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = all_tasks(&env).remove(0);
    let id = task.id.to_string();
    done::run(done_args(&id), &mut env.ctx).unwrap();

    let tasks = all_tasks(&env);
    let new_task = tasks
        .iter()
        .find(|t| t.status == Status::Open)
        .expect("no open task");

    let start = new_task.start.expect("new task has no start date");
    let due = new_task.due.expect("new task has no due date");
    assert_eq!(start.day(), 1, "start day should be 1st");
    assert_eq!(due.day(), 3, "due day should be 3rd");
    assert_eq!((due - start).num_days(), 2, "start-to-due offset should be 2 days");
    // Should be in a future month
    assert!(
        start > NaiveDate::from_ymd_opt(2026, 6, 3).unwrap(),
        "new start should be after original due"
    );
}

/// 3. Completion-based 1-week interval with Saturday snap.
#[test]
fn recur_completion_1_week_saturday_snap() {
    let mut env = common::setup();
    let today = Local::now().date_naive();
    add::run(
        add::Args {
            due: Some(today.to_string()),
            recur_completion: Some(7),
            recur_snap: Some("saturday".to_string()),
            ..add_args("Weekly chore")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = all_tasks(&env).remove(0);
    let id = task.id.to_string();
    done::run(done_args(&id), &mut env.ctx).unwrap();

    let tasks = all_tasks(&env);
    let new_task = tasks
        .iter()
        .find(|t| t.status == Status::Open)
        .expect("no open task");

    let date = new_task.due.or(new_task.start).expect("no date on new task");
    assert_eq!(date.weekday(), Weekday::Sat, "expected Saturday, got {}", date.weekday());
    assert!(date >= today + Duration::days(7), "due date should be >= today + 7d");
}

/// 4. Completion-based 2-week interval with Saturday snap.
#[test]
fn recur_completion_2_weeks_saturday_snap() {
    let mut env = common::setup();
    let today = Local::now().date_naive();
    add::run(
        add::Args {
            due: Some(today.to_string()),
            recur_completion: Some(14),
            recur_snap: Some("saturday".to_string()),
            ..add_args("Biweekly chore")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = all_tasks(&env).remove(0);
    let id = task.id.to_string();
    done::run(done_args(&id), &mut env.ctx).unwrap();

    let tasks = all_tasks(&env);
    let new_task = tasks
        .iter()
        .find(|t| t.status == Status::Open)
        .expect("no open task");

    let date = new_task.due.or(new_task.start).expect("no date on new task");
    assert_eq!(date.weekday(), Weekday::Sat, "expected Saturday, got {}", date.weekday());
    assert!(date >= today + Duration::days(14), "due date should be >= today + 14d");
}

/// 5. Quarterly recurring task on 1st of the month.
#[test]
fn recur_quarterly_on_1st() {
    let mut env = common::setup();
    add::run(
        add::Args {
            start: Some("2026-01-01".to_string()),
            recur_schedule: Some("FREQ=MONTHLY;INTERVAL=3;BYMONTHDAY=1".to_string()),
            ..add_args("Quarterly report")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = all_tasks(&env).remove(0);
    let id = task.id.to_string();
    done::run(done_args(&id), &mut env.ctx).unwrap();

    let tasks = all_tasks(&env);
    let new_task = tasks
        .iter()
        .find(|t| t.status == Status::Open)
        .expect("no open task");

    let start = new_task.start.expect("new task has no start date");
    assert_eq!(start.day(), 1, "start day should be 1st");
    // Quarterly months after Jan: Apr(4), Jul(7), Oct(10)
    assert!(
        [4u32, 7, 10].contains(&start.month()),
        "expected quarterly month (4/7/10), got {}",
        start.month()
    );
}

/// 6. The original completed task must have status Done; new task must be Open.
#[test]
fn recur_done_task_stays_done() {
    let mut env = common::setup();
    let today = Local::now().date_naive();
    add::run(
        add::Args {
            due: Some(today.to_string()),
            recur_completion: Some(7),
            ..add_args("Recurring task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = all_tasks(&env).remove(0);
    let id = task.id.to_string();
    done::run(done_args(&id), &mut env.ctx).unwrap();

    let tasks = all_tasks(&env);
    assert_eq!(tasks.len(), 2);
    let done_tasks: Vec<_> = tasks.iter().filter(|t| t.status == Status::Done).collect();
    let open_tasks: Vec<_> = tasks.iter().filter(|t| t.status == Status::Open).collect();
    assert_eq!(done_tasks.len(), 1, "expected exactly 1 done task");
    assert_eq!(open_tasks.len(), 1, "expected exactly 1 open task");
}

/// 7. After completing, both tasks share the same recurrence_id.
#[test]
fn recur_series_linked_by_recurrence_id() {
    let mut env = common::setup();
    let today = Local::now().date_naive();
    add::run(
        add::Args {
            due: Some(today.to_string()),
            recur_completion: Some(7),
            ..add_args("Linked recurring task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = all_tasks(&env).remove(0);
    let original_id = task.id;
    let id = original_id.to_string();
    done::run(done_args(&id), &mut env.ctx).unwrap();

    let tasks = all_tasks(&env);
    assert_eq!(tasks.len(), 2);

    // The new open task must have recurrence_id set (to original task's id or its own recurrence_id)
    let new_task = tasks
        .iter()
        .find(|t| t.status == Status::Open)
        .expect("no open task");
    let rec_id = new_task.recurrence_id.expect("new task has no recurrence_id");
    // recurrence_id should point to the original task (since it had none, it uses its own id)
    assert_eq!(rec_id, original_id, "recurrence_id should equal the original task's id");
}

/// 8. 30-day completion interval, no snap — due date is exactly today + 30.
#[test]
fn recur_completion_no_snap_interval_days() {
    let mut env = common::setup();
    let today = Local::now().date_naive();
    add::run(
        add::Args {
            due: Some(today.to_string()),
            recur_completion: Some(30),
            ..add_args("Monthly chore")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = all_tasks(&env).remove(0);
    let id = task.id.to_string();
    done::run(done_args(&id), &mut env.ctx).unwrap();

    let tasks = all_tasks(&env);
    let new_task = tasks
        .iter()
        .find(|t| t.status == Status::Open)
        .expect("no open task");

    let date = new_task.due.or(new_task.start).expect("no date on new task");
    assert_eq!(date, today + Duration::days(30), "due should be today + 30");
}

/// 9. Any spawned schedule-based task has start or due strictly after today.
#[test]
fn recur_schedule_next_occurrence_is_in_future() {
    let mut env = common::setup();
    let today = Local::now().date_naive();
    // Use a well-past anchor so the next occurrence is definitely in the future.
    add::run(
        add::Args {
            due: Some("2026-01-01".to_string()),
            start: Some("2026-01-01".to_string()),
            recur_schedule: Some("FREQ=MONTHLY;BYMONTHDAY=1".to_string()),
            ..add_args("Monthly first")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = all_tasks(&env).remove(0);
    let id = task.id.to_string();
    done::run(done_args(&id), &mut env.ctx).unwrap();

    let tasks = all_tasks(&env);
    let new_task = tasks
        .iter()
        .find(|t| t.status == Status::Open)
        .expect("no open task");

    let date = new_task.due.or(new_task.start).expect("no date on new task");
    assert!(date > today, "spawned schedule task's date {date} should be after today {today}");
}
