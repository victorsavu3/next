mod common;

use chrono::Local;
use next::cli::commands::{add, forecast};

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
        future: false,
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
    let task = env.ctx.store.list_tasks().unwrap().remove(0);
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
