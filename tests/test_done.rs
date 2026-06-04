mod common;

use next::core::domain::task::Status;
use next::cli::commands::{add, done};

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
fn done_marks_task_closed() {
    let mut env = common::setup();
    add::run(add_args("Finish report"), &mut env.ctx).unwrap();

    let task = env.ctx.store.list_tasks().unwrap().remove(0);
    let id = task.id.to_string();

    done::run(done::Args { id, completed_at: None, json: false }, &mut env.ctx).unwrap();

    let updated = env.ctx.store.list_tasks().unwrap().remove(0);
    assert_eq!(updated.status, Status::Done);
}

#[test]
fn done_by_id_prefix() {
    let mut env = common::setup();
    add::run(add_args("Clean desk"), &mut env.ctx).unwrap();

    let task = env.ctx.store.list_tasks().unwrap().remove(0);
    let prefix = task.id.to_string().replace('-', "")[..8].to_string();

    done::run(done::Args { id: prefix, completed_at: None, json: false }, &mut env.ctx).unwrap();

    let updated = env.ctx.store.list_tasks().unwrap().remove(0);
    assert_eq!(updated.status, Status::Done);
}

#[test]
fn done_by_slug() {
    let mut env = common::setup();
    let a = add::Args {
        slug: Some("daily-standup".to_string()),
        ..add_args("Daily standup")
    };
    add::run(a, &mut env.ctx).unwrap();

    done::run(
        done::Args {
            id: "daily-standup".to_string(),
            completed_at: None,
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    let updated = env.ctx.store.list_tasks().unwrap().remove(0);
    assert_eq!(updated.status, Status::Done);
}

#[test]
fn done_nonexistent_task_errors() {
    let mut env = common::setup();
    let err = done::run(
        done::Args {
            id: "00000000-0000-0000-0000-000000000000".to_string(),
            completed_at: None,
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("not found") || err.to_string().contains("00000000"),
        "unexpected error: {err}"
    );
}
