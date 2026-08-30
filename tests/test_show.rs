mod common;

use next::cli::commands::{add, show};

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
fn show_by_id() {
    let mut env = common::setup();
    add::run(add_args("My task"), &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    show::run(
        show::Args {
            id: task.id.to_string(),
            json: false,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}

#[test]
fn show_by_slug() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("my-slug".into()),
            ..add_args("Slugged task")
        },
        &mut env.ctx,
    )
    .unwrap();
    show::run(
        show::Args {
            id: "my-slug".into(),
            json: false,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}

#[test]
fn show_by_id_prefix() {
    let mut env = common::setup();
    add::run(add_args("Prefix task"), &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    let prefix = task.id.to_string().replace('-', "")[..8].to_string();
    show::run(
        show::Args {
            id: prefix,
            json: false,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}

#[test]
fn show_json_output() {
    let mut env = common::setup();
    add::run(add_args("JSON task"), &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    show::run(
        show::Args {
            id: task.id.to_string(),
            json: true,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}

#[test]
fn show_nonexistent_errors() {
    let env = common::setup();
    let err = show::run(
        show::Args {
            id: "00000000-0000-0000-0000-000000000000".into(),
            json: false,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("not found") || err.to_string().contains("00000000"),
        "unexpected: {err}"
    );
}

#[test]
fn show_task_with_all_optional_fields() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("full-task".into()),
            due: Some("2026-12-31".into()),
            priority: Some("high".into()),
            tags: vec!["@work".into(), "#laptop".into()],
            description: Some("A description.".into()),
            url: Some("https://example.com".into()),
            notes: Some("Some notes.".into()),
            assignee: Some("alice".into()),
            adjust: Some(1.5),
            ..add_args("Full task")
        },
        &mut env.ctx,
    )
    .unwrap();
    show::run(
        show::Args {
            id: "full-task".into(),
            json: false,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}

#[test]
fn show_task_with_parent() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("parent".into()),
            ..add_args("Parent task")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            slug: Some("child".into()),
            parent: Some("parent".into()),
            ..add_args("Child task")
        },
        &mut env.ctx,
    )
    .unwrap();
    show::run(
        show::Args {
            id: "child".into(),
            json: false,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}

#[test]
fn show_task_with_blocker() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("blocker".into()),
            ..add_args("Blocking task")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            slug: Some("blocked".into()),
            blocked_by: vec!["blocker".into()],
            ..add_args("Blocked task")
        },
        &mut env.ctx,
    )
    .unwrap();
    show::run(
        show::Args {
            id: "blocked".into(),
            json: false,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}
