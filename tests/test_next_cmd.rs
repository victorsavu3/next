mod common;

use next::cli::commands::{add, done, next_cmd};

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

fn next_args(count: Option<usize>) -> next_cmd::Args {
    next_cmd::Args {
        count,
        future: false,
        all: false,
        all_users: false,
        json: false,
        tokens: vec![],
    }
}

#[test]
fn next_shows_open_tasks() {
    let mut env = common::setup();
    add::run(add_args("Task one"), &mut env.ctx).unwrap();
    add::run(add_args("Task two"), &mut env.ctx).unwrap();
    next_cmd::run(next_args(None), &mut env.ctx).unwrap();
}

#[test]
fn next_respects_count_arg() {
    let mut env = common::setup();
    for i in 1..=5 {
        add::run(add_args(&format!("task {i}")), &mut env.ctx).unwrap();
    }
    // Count of 2 must not error; all 5 tasks remain in the store.
    next_cmd::run(next_args(Some(2)), &mut env.ctx).unwrap();
    assert_eq!(env.ctx.store.list_tasks().unwrap().len(), 5);
}

#[test]
fn next_default_count_from_config() {
    let mut env = common::setup();
    for i in 1..=15 {
        add::run(add_args(&format!("task {i}")), &mut env.ctx).unwrap();
    }
    env.ctx.config.next_count = 3;
    // With 15 tasks and next_count = 3, must not error.
    next_cmd::run(next_args(None), &mut env.ctx).unwrap();
    assert_eq!(env.ctx.store.list_tasks().unwrap().len(), 15);
}

#[test]
fn next_excludes_done_tasks() {
    let mut env = common::setup();
    add::run(
        add::Args { slug: Some("done-task".into()), ..add_args("Done task") },
        &mut env.ctx,
    )
    .unwrap();
    add::run(add_args("Open task"), &mut env.ctx).unwrap();

    done::run(done::Args { id: "done-task".into(), json: false }, &mut env.ctx).unwrap();

    // next only shows open tasks
    next_cmd::run(next_args(None), &mut env.ctx).unwrap();
    let all = env.ctx.store.list_tasks().unwrap();
    let open_count = all
        .iter()
        .filter(|t| t.status == next::domain::task::Status::Open)
        .count();
    assert_eq!(open_count, 1);
}

#[test]
fn next_json_output() {
    let mut env = common::setup();
    add::run(add_args("JSON task"), &mut env.ctx).unwrap();
    next_cmd::run(
        next_cmd::Args { json: true, ..next_args(None) },
        &mut env.ctx,
    )
    .unwrap();
}

#[test]
fn next_empty_list_does_not_error() {
    let mut env = common::setup();
    next_cmd::run(next_args(None), &mut env.ctx).unwrap();
}
