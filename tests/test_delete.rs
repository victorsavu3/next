mod common;

use next::cli::commands::{add, delete};

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

#[test]
fn delete_removes_task_with_yes() {
    let mut env = common::setup();
    add::run(add_args("Temp task"), &mut env.ctx).unwrap();
    assert_eq!(env.ctx.repo.store.list_tasks().unwrap().len(), 1);

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    delete::run(
        delete::Args {
            id: task.id.to_string(),
            yes: true,
        },
        &mut env.ctx,
    )
    .unwrap();

    assert!(env.ctx.repo.store.list_tasks().unwrap().is_empty());
}

#[test]
fn delete_without_yes_errors() {
    let mut env = common::setup();
    add::run(add_args("Protected task"), &mut env.ctx).unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    let err = delete::run(
        delete::Args {
            id: task.id.to_string(),
            yes: false,
        },
        &mut env.ctx,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("aborted"),
        "unexpected error: {err}"
    );

    // Task must still exist.
    assert_eq!(env.ctx.repo.store.list_tasks().unwrap().len(), 1);
}

#[test]
fn delete_by_slug() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("delete-me".into()),
            ..add_args("Deletable task")
        },
        &mut env.ctx,
    )
    .unwrap();

    delete::run(
        delete::Args {
            id: "delete-me".into(),
            yes: true,
        },
        &mut env.ctx,
    )
    .unwrap();
    assert!(env.ctx.repo.store.list_tasks().unwrap().is_empty());
}

#[test]
fn delete_nonexistent_errors() {
    let mut env = common::setup();
    let err = delete::run(
        delete::Args {
            id: "00000000-0000-0000-0000-000000000000".into(),
            yes: true,
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
fn delete_by_id_prefix() {
    let mut env = common::setup();
    add::run(add_args("Prefix delete"), &mut env.ctx).unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    let prefix = task.id.to_string().replace('-', "")[..8].to_string();

    delete::run(
        delete::Args {
            id: prefix,
            yes: true,
        },
        &mut env.ctx,
    )
    .unwrap();
    assert!(env.ctx.repo.store.list_tasks().unwrap().is_empty());
}
