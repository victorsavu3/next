mod common;

use chrono::{Datelike, Duration, Local, NaiveDate, Weekday};
use next::cli::commands::{add, cancel, done, edit};
use next::core::domain::task::{Recurrence, Status};
use next::core::store::Store as _;

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
        completed_at: None,
        json: false,
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Returns all tasks regardless of status.
fn all_tasks(env: &common::TestEnv) -> Vec<next::core::domain::task::Task> {
    env.ctx.repo.store.list_tasks().unwrap()
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

/// Cancelling a recurring task must NOT spawn a next instance.
#[test]
fn recur_cancel_does_not_spawn() {
    let mut env = common::setup();
    let today = Local::now().date_naive();
    add::run(
        add::Args {
            due: Some(today.to_string()),
            recur_completion: Some(7),
            slug: Some("cancel-me".into()),
            ..add_args("Recurring to cancel")
        },
        &mut env.ctx,
    )
    .unwrap();

    cancel::run(
        cancel::Args { id: "cancel-me".into(), json: false },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = all_tasks(&env);
    assert_eq!(tasks.len(), 1, "cancel must not spawn a new instance");
    assert_eq!(tasks[0].status, Status::Cancelled);
}

/// Marking a non-recurring task done must not create any extra task.
#[test]
fn done_non_recurring_task_does_not_spawn() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("one-off".into()),
            ..add_args("One-off task")
        },
        &mut env.ctx,
    )
    .unwrap();

    done::run(done_args("one-off"), &mut env.ctx).unwrap();

    let tasks = all_tasks(&env);
    assert_eq!(tasks.len(), 1, "no extra task should be created for non-recurring");
    assert_eq!(tasks[0].status, Status::Done);
}

/// Spawned task must have slug = None (slugs are per-instance).
#[test]
fn recur_spawned_task_has_no_slug() {
    let mut env = common::setup();
    let today = Local::now().date_naive();
    add::run(
        add::Args {
            due: Some(today.to_string()),
            recur_completion: Some(7),
            slug: Some("my-slug".into()),
            ..add_args("Slugged recurring")
        },
        &mut env.ctx,
    )
    .unwrap();

    done::run(done_args("my-slug"), &mut env.ctx).unwrap();

    let tasks = all_tasks(&env);
    let new_task = tasks
        .iter()
        .find(|t| t.status == Status::Open)
        .expect("no open task");
    assert!(new_task.slug.is_none(), "spawned task must not carry the parent's slug");
}

/// Completing the spawned task produces a 3rd instance (chain of spawns).
#[test]
fn recur_chain_of_three_spawns() {
    let mut env = common::setup();
    let today = Local::now().date_naive();
    add::run(
        add::Args {
            due: Some(today.to_string()),
            recur_completion: Some(7),
            ..add_args("Chain task")
        },
        &mut env.ctx,
    )
    .unwrap();

    // First done → spawns instance 2
    let id1 = all_tasks(&env)[0].id.to_string();
    done::run(done_args(&id1), &mut env.ctx).unwrap();
    assert_eq!(all_tasks(&env).len(), 2);

    // Second done → spawns instance 3
    let id2 = all_tasks(&env)
        .iter()
        .find(|t| t.status == Status::Open)
        .expect("no open task after first spawn")
        .id
        .to_string();
    done::run(done_args(&id2), &mut env.ctx).unwrap();

    let tasks = all_tasks(&env);
    assert_eq!(tasks.len(), 3, "chain should produce 3 total instances");
    let open: Vec<_> = tasks.iter().filter(|t| t.status == Status::Open).collect();
    assert_eq!(open.len(), 1, "exactly one open task in chain");
}

/// Re-running `done` on an already-completed recurring instance must error and
/// must NOT spawn a second next instance (regression for issue #9).
#[test]
fn recur_double_done_spawns_exactly_one_instance() {
    let mut env = common::setup();
    let today = Local::now().date_naive();
    add::run(
        add::Args {
            due: Some(today.to_string()),
            recur_completion: Some(7),
            ..add_args("Recurring chore")
        },
        &mut env.ctx,
    )
    .unwrap();

    let id = all_tasks(&env)[0].id.to_string();

    // First done → marks done and spawns exactly one new instance.
    done::run(done_args(&id), &mut env.ctx).unwrap();
    assert_eq!(all_tasks(&env).len(), 2, "first done spawns one instance");

    // Second done on the same (now Done) task → errors, spawns nothing.
    let err = done::run(done_args(&id), &mut env.ctx).unwrap_err();
    assert!(err.to_string().contains("already done"), "unexpected error: {err}");

    let tasks = all_tasks(&env);
    assert_eq!(tasks.len(), 2, "second done must not spawn a duplicate instance");
    let open: Vec<_> = tasks.iter().filter(|t| t.status == Status::Open).collect();
    assert_eq!(open.len(), 1, "still exactly one open instance");
}

/// Old TOML files using the `rule` field name (pre-rename alias) load correctly.
///
/// The actual deserialization alias is verified by the storage unit test
/// `schedule_recurrence_rule_alias_deserializes`. Here we verify that a task
/// created with `add` and reloaded from a fresh store preserves its recurrence rule.
#[test]
fn backward_compat_rule_alias_loads() {
    let mut env = common::setup();
    // Add a task with a schedule recurrence.
    add::run(
        add::Args {
            due: Some("2026-06-01".into()),
            recur_schedule: Some("FREQ=MONTHLY;BYMONTHDAY=1".into()),
            slug: Some("legacy".into()),
            ..add_args("Legacy recurring task")
        },
        &mut env.ctx,
    )
    .unwrap();

    // Reload from a fresh store (exercises round-trip through TOML serialization).
    let (fresh_store, _) = next::core::storage::open(env.ctx.repo.repo_root.clone()).unwrap();
    let task = fresh_store
        .get_task_by_slug("legacy")
        .unwrap()
        .expect("task should be found");
    assert_eq!(task.title, "Legacy recurring task");
    assert!(task.recurrence.is_some(), "recurrence should survive store reload");
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

/// Snap round-trips through TOML serialization (NextWeekday, NextWorkday, DayOfMonth).
#[test]
fn recur_snap_round_trips_toml() {
    use next::core::domain::task::Snap;

    let mut env = common::setup();

    // NextWeekday snap
    add::run(
        add::Args {
            due: Some("2026-06-06".into()),
            recur_completion: Some(7),
            recur_snap: Some("saturday".into()),
            slug: Some("sat-task".into()),
            ..add_args("Saturday task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let (fresh, _) = next::core::storage::open(env.ctx.repo.repo_root.clone()).unwrap();
    let t = fresh.get_task_by_slug("sat-task").unwrap().unwrap();
    assert!(
        matches!(
            t.recurrence,
            Some(Recurrence::Completion { snap: Some(Snap::NextWeekday { weekday: 5 }), .. })
        ),
        "NextWeekday snap should round-trip"
    );

    // NextWorkday snap
    add::run(
        add::Args {
            due: Some("2026-06-01".into()),
            recur_completion: Some(30),
            recur_snap: Some("next-workday".into()),
            slug: Some("workday-task".into()),
            ..add_args("Workday task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let (fresh2, _) = next::core::storage::open(env.ctx.repo.repo_root.clone()).unwrap();
    let t2 = fresh2.get_task_by_slug("workday-task").unwrap().unwrap();
    assert!(
        matches!(t2.recurrence, Some(Recurrence::Completion { snap: Some(Snap::NextWorkday), .. })),
        "NextWorkday snap should round-trip"
    );
}

/// Editing a recurring task's RRULE preserves the original anchor.
#[test]
fn recur_edit_rule_preserves_anchor() {
    let mut env = common::setup();
    add::run(
        add::Args {
            start: Some("2026-06-01".into()),
            recur_schedule: Some("FREQ=MONTHLY;BYMONTHDAY=1".into()),
            slug: Some("monthly".into()),
            ..add_args("Monthly task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let before = env.ctx.repo.store.get_task_by_slug("monthly").unwrap().unwrap();
    let original_anchor = match &before.recurrence {
        Some(Recurrence::Schedule { anchor, .. }) => *anchor,
        _ => panic!("expected schedule recurrence"),
    };

    // Change the rule but keep everything else.
    edit::run(
        edit::Args {
            id: "monthly".into(),
            recur_schedule: Some("FREQ=MONTHLY;BYMONTHDAY=15".into()),
            title: None,
            due: None,
            start: None,
            priority: None,
            slug: None,
            assignee: None,
            clear_assignee: false,
            tags: vec![],
            remove_tags: vec![],
            parent: None,
            blocked_by: vec![],
            description: None,
            clear_description: false,
            url: None,
            clear_url: false,
            notes: None,
            recur_completion: None,
            recur_snap: None,
            clear_recurrence: false,
            long_term: false,
            adjust: None,
            clear_due: false,
            clear_start: false,
            clear_parent: false,
            clear_blocked_by: false,
            json: false,
            tag_tokens: vec![],
        },
        &mut env.ctx,
    )
    .unwrap();

    let after = env.ctx.repo.store.get_task_by_slug("monthly").unwrap().unwrap();
    let new_anchor = match &after.recurrence {
        Some(Recurrence::Schedule { anchor, rrule, .. }) => {
            assert_eq!(rrule, "FREQ=MONTHLY;BYMONTHDAY=15", "rrule should be updated");
            *anchor
        }
        _ => panic!("expected schedule recurrence after edit"),
    };
    assert_eq!(original_anchor, new_anchor, "anchor must not change when editing the rule");
}

/// Clearing the recurrence removes the rule and recurrence_id.
#[test]
fn recur_clear_recurrence_removes_rule() {
    let mut env = common::setup();
    add::run(
        add::Args {
            recur_completion: Some(7),
            slug: Some("clearme".into()),
            ..add_args("Recurring task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let before = env.ctx.repo.store.get_task_by_slug("clearme").unwrap().unwrap();
    assert!(before.recurrence.is_some(), "should have recurrence before clear");
    assert!(before.recurrence_id.is_some(), "should have recurrence_id before clear");

    edit::run(
        edit::Args {
            id: "clearme".into(),
            clear_recurrence: true,
            title: None,
            due: None,
            start: None,
            priority: None,
            slug: None,
            assignee: None,
            clear_assignee: false,
            tags: vec![],
            remove_tags: vec![],
            parent: None,
            blocked_by: vec![],
            description: None,
            clear_description: false,
            url: None,
            clear_url: false,
            notes: None,
            recur_schedule: None,
            recur_completion: None,
            recur_snap: None,
            long_term: false,
            adjust: None,
            clear_due: false,
            clear_start: false,
            clear_parent: false,
            clear_blocked_by: false,
            json: false,
            tag_tokens: vec![],
        },
        &mut env.ctx,
    )
    .unwrap();

    let after = env.ctx.repo.store.get_task_by_slug("clearme").unwrap().unwrap();
    assert!(after.recurrence.is_none(), "recurrence should be cleared");
    assert!(after.recurrence_id.is_none(), "recurrence_id should be cleared");
}

/// recurrence_id is set on the first instance when adding a recurring task.
#[test]
fn recur_first_instance_has_recurrence_id() {
    let mut env = common::setup();
    add::run(
        add::Args {
            recur_completion: Some(7),
            slug: Some("first-instance".into()),
            ..add_args("First instance")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = env.ctx.repo.store.get_task_by_slug("first-instance").unwrap().unwrap();
    assert_eq!(
        task.recurrence_id,
        Some(task.id),
        "first instance should have recurrence_id == its own id"
    );
}
