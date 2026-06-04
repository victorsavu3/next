mod common;

use next::cli::commands::{add, cancel};
use next::core::domain::task::Status;

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

#[test]
fn cancel_marks_task_cancelled() {
    let mut env = common::setup();
    add::run(add_args("Unwanted task"), &mut env.ctx).unwrap();

    let task = env.ctx.store.list_tasks().unwrap().remove(0);
    cancel::run(cancel::Args { id: task.id.to_string(), json: false }, &mut env.ctx).unwrap();

    let updated = env.ctx.store.list_tasks().unwrap().remove(0);
    assert_eq!(updated.status, Status::Cancelled);
}

#[test]
fn cancel_by_id_prefix() {
    let mut env = common::setup();
    add::run(add_args("Clean desk"), &mut env.ctx).unwrap();

    let task = env.ctx.store.list_tasks().unwrap().remove(0);
    let prefix = task.id.to_string().replace('-', "")[..8].to_string();

    cancel::run(cancel::Args { id: prefix, json: false }, &mut env.ctx).unwrap();

    let updated = env.ctx.store.list_tasks().unwrap().remove(0);
    assert_eq!(updated.status, Status::Cancelled);
}

#[test]
fn cancel_by_slug() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("my-task".into()),
            ..add_args("Slugged task")
        },
        &mut env.ctx,
    )
    .unwrap();

    cancel::run(cancel::Args { id: "my-task".into(), json: false }, &mut env.ctx).unwrap();

    let updated = env.ctx.store.list_tasks().unwrap().remove(0);
    assert_eq!(updated.status, Status::Cancelled);
}

#[test]
fn cancel_nonexistent_task_errors() {
    let mut env = common::setup();
    let err = cancel::run(
        cancel::Args { id: "00000000-0000-0000-0000-000000000000".into(), json: false },
        &mut env.ctx,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("not found") || err.to_string().contains("00000000"),
        "unexpected error: {err}"
    );
}

#[test]
fn cancel_json_output() {
    let mut env = common::setup();
    add::run(add_args("JSON task"), &mut env.ctx).unwrap();
    let task = env.ctx.store.list_tasks().unwrap().remove(0);
    cancel::run(cancel::Args { id: task.id.to_string(), json: true }, &mut env.ctx).unwrap();
}
