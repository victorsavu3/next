mod common;

use next_cli::cli::commands::{add, done, tree};

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

fn tree_args(all: bool) -> tree::Args {
    tree::Args { all, json: false }
}

// ---------------------------------------------------------------------------
// Basic tree rendering
// ---------------------------------------------------------------------------

#[test]
fn tree_empty_when_no_tasks() {
    let mut env = common::setup();
    tree::run(tree_args(false), &mut env.ctx).unwrap();
}

#[test]
fn tree_shows_top_level_tasks() {
    let mut env = common::setup();
    add::run(add_args("Task A"), &mut env.ctx).unwrap();
    add::run(add_args("Task B"), &mut env.ctx).unwrap();
    // Must not error; two tasks, no parent.
    tree::run(tree_args(false), &mut env.ctx).unwrap();
}

#[test]
fn tree_shows_children_under_parent() {
    let mut env = common::setup();

    add::run(
        add::Args {
            slug: Some("parent-task".into()),
            ..add_args("Parent")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            parent: Some("parent-task".into()),
            ..add_args("Child")
        },
        &mut env.ctx,
    )
    .unwrap();

    // No error; child is nested under parent.
    tree::run(tree_args(false), &mut env.ctx).unwrap();

    // Verify structure: parent has one child.
    let all = env.ctx.store.list_tasks().unwrap();
    let parent = all.iter().find(|t| t.title == "Parent").unwrap();
    let child = all.iter().find(|t| t.title == "Child").unwrap();
    assert_eq!(child.parent_id, Some(parent.id));
}

#[test]
fn tree_excludes_done_tasks_by_default() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("done-task".into()),
            ..add_args("Done task")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(add_args("Open task"), &mut env.ctx).unwrap();

    done::run(
        done::Args { id: "done-task".into(), json: false },
        &mut env.ctx,
    )
    .unwrap();

    let all = env.ctx.store.list_tasks().unwrap();
    let open_count = all
        .iter()
        .filter(|t| t.status == next::domain::task::Status::Open)
        .count();
    assert_eq!(open_count, 1);
    // Default tree should not error even with mixed statuses.
    tree::run(tree_args(false), &mut env.ctx).unwrap();
}

#[test]
fn tree_all_includes_done_tasks() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("done-task".into()),
            ..add_args("Done task")
        },
        &mut env.ctx,
    )
    .unwrap();
    done::run(
        done::Args { id: "done-task".into(), json: false },
        &mut env.ctx,
    )
    .unwrap();

    // --all must not error and includes done tasks.
    tree::run(tree_args(true), &mut env.ctx).unwrap();

    let all = env.ctx.store.list_tasks().unwrap();
    assert_eq!(all.len(), 1);
}

#[test]
fn tree_json_output() {
    let mut env = common::setup();
    add::run(add_args("Task JSON"), &mut env.ctx).unwrap();
    tree::run(tree::Args { all: false, json: true }, &mut env.ctx).unwrap();
}
