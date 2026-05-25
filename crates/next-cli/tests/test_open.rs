mod common;

use next_cli::cli::commands::{add, open};

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
fn open_errors_when_task_has_no_url() {
    let mut env = common::setup();
    add::run(
        add::Args { slug: Some("no-url".to_string()), ..add_args("No URL task") },
        &mut env.ctx,
    )
    .unwrap();

    let err = open::run(open::Args { id: "no-url".to_string() }, &mut env.ctx).unwrap_err();
    assert!(
        err.to_string().contains("no URL"),
        "unexpected error: {err}"
    );
}

#[test]
fn open_errors_on_nonexistent_task() {
    let mut env = common::setup();
    let err = open::run(
        open::Args { id: "does-not-exist".to_string() },
        &mut env.ctx,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("does-not-exist") || err.to_string().contains("not found"),
        "unexpected error: {err}"
    );
}
