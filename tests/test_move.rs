mod common;

use next::cli::commands::{add, move_cmd};

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
        long_term: false,
        adjust: None,
        json: false,
    }
}

#[test]
fn move_sets_parent() {
    let mut env = common::setup();
    add::run(
        add::Args { slug: Some("parent".into()), ..add_args("Parent task") },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args { slug: Some("child".into()), ..add_args("Child task") },
        &mut env.ctx,
    )
    .unwrap();

    move_cmd::run(
        move_cmd::Args { id: "child".into(), parent: Some("parent".into()), json: false },
        &mut env.ctx,
    )
    .unwrap();

    let parent = env.ctx.store.get_task_by_slug("parent").unwrap().unwrap();
    let child = env.ctx.store.get_task_by_slug("child").unwrap().unwrap();
    assert_eq!(child.parent_id, Some(parent.id));
}

#[test]
fn move_none_clears_parent() {
    let mut env = common::setup();
    add::run(
        add::Args { slug: Some("root".into()), ..add_args("Root") },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            slug: Some("child".into()),
            parent: Some("root".into()),
            ..add_args("Child")
        },
        &mut env.ctx,
    )
    .unwrap();

    let child_before = env.ctx.store.get_task_by_slug("child").unwrap().unwrap();
    assert!(child_before.parent_id.is_some());

    move_cmd::run(
        move_cmd::Args { id: "child".into(), parent: Some("none".into()), json: false },
        &mut env.ctx,
    )
    .unwrap();

    let child_after = env.ctx.store.get_task_by_slug("child").unwrap().unwrap();
    assert!(child_after.parent_id.is_none());
}

#[test]
fn move_no_parent_arg_is_noop() {
    let mut env = common::setup();
    add::run(
        add::Args { slug: Some("task-a".into()), ..add_args("Task A") },
        &mut env.ctx,
    )
    .unwrap();

    move_cmd::run(
        move_cmd::Args { id: "task-a".into(), parent: None, json: false },
        &mut env.ctx,
    )
    .unwrap();

    let task = env.ctx.store.get_task_by_slug("task-a").unwrap().unwrap();
    assert!(task.parent_id.is_none());
}

#[test]
fn move_nonexistent_task_errors() {
    let mut env = common::setup();
    let err = move_cmd::run(
        move_cmd::Args {
            id: "00000000-0000-0000-0000-000000000000".into(),
            parent: None,
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

#[test]
fn move_json_output() {
    let mut env = common::setup();
    add::run(
        add::Args { slug: Some("json-task".into()), ..add_args("JSON task") },
        &mut env.ctx,
    )
    .unwrap();

    move_cmd::run(
        move_cmd::Args { id: "json-task".into(), parent: None, json: true },
        &mut env.ctx,
    )
    .unwrap();
}
