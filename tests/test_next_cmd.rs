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
        format: None,
        fields: vec![],
        tokens: vec![],
    }
}

/// Runs `next next …` and returns what it wrote, so the JSON can be asserted
/// rather than merely produced.
fn run_next(env: &common::TestEnv, args: next_cmd::Args) -> anyhow::Result<String> {
    let mut buf: Vec<u8> = Vec::new();
    next_cmd::run_with_writer(args, &env.ctx, &mut buf)?;
    Ok(String::from_utf8(buf).expect("utf-8 output"))
}

#[test]
fn next_shows_open_tasks() {
    let mut env = common::setup();
    add::run(add_args("Task one"), &mut env.ctx).unwrap();
    add::run(add_args("Task two"), &mut env.ctx).unwrap();
    next_cmd::run(next_args(None), &env.ctx).unwrap();
}

#[test]
fn next_respects_count_arg() {
    let mut env = common::setup();
    for i in 1..=5 {
        add::run(add_args(&format!("task {i}")), &mut env.ctx).unwrap();
    }
    // Count of 2 must not error; all 5 tasks remain in the store.
    next_cmd::run(next_args(Some(2)), &env.ctx).unwrap();
    assert_eq!(env.ctx.repo.store.list_tasks().unwrap().len(), 5);
}

#[test]
fn next_default_count_from_config() {
    let mut env = common::setup();
    for i in 1..=15 {
        add::run(add_args(&format!("task {i}")), &mut env.ctx).unwrap();
    }
    env.ctx.config.next_count = 3;
    // With 15 tasks and next_count = 3, must not error.
    next_cmd::run(next_args(None), &env.ctx).unwrap();
    assert_eq!(env.ctx.repo.store.list_tasks().unwrap().len(), 15);
}

#[test]
fn next_excludes_done_tasks() {
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
        done::Args {
            id: "done-task".into(),
            completed_at: None,
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    // next only shows open tasks
    next_cmd::run(next_args(None), &env.ctx).unwrap();
    let all = env.ctx.repo.store.list_tasks().unwrap();
    let open_count = all
        .iter()
        .filter(|t| t.status == next::core::domain::task::Status::Open)
        .count();
    assert_eq!(open_count, 1);
}

#[test]
fn next_json_output() {
    let mut env = common::setup();
    add::run(add_args("JSON task"), &mut env.ctx).unwrap();
    next_cmd::run(
        next_cmd::Args {
            json: true,
            ..next_args(None)
        },
        &env.ctx,
    )
    .unwrap();
}

/// The flag has to exist on the command line, not only on the struct — a test
/// that builds `Args` by hand would pass with no `--fields` flag at all.
#[test]
fn next_accepts_fields_on_the_command_line() {
    use clap::Parser as _;
    let cli = next::cli::Cli::try_parse_from(["next", "next", "--json", "--fields", "id,title"])
        .expect("`next --fields` must parse");
    match cli.command.expect("a subcommand") {
        next::cli::Command::Next(args) => {
            assert_eq!(args.fields, vec!["id".to_string(), "title".to_string()]);
        }
        other => panic!("expected `next`, got {other:?}"),
    }
}

#[test]
fn next_json_returns_every_field_by_default() {
    let mut env = common::setup();
    add::run(add_args("JSON task"), &mut env.ctx).unwrap();

    let out = run_next(
        &env,
        next_cmd::Args {
            json: true,
            ..next_args(None)
        },
    )
    .unwrap();
    let items: serde_json::Value = serde_json::from_str(&out).unwrap();
    let row = &items[0];
    assert!(row.get("score").is_some(), "{out}");
    assert!(
        row["task"].get("status").is_some() && row["task"].get("priority").is_some(),
        "no projection means every field the task has: {out}"
    );
}

#[test]
fn next_json_projects_only_the_named_fields() {
    let mut env = common::setup();
    add::run(add_args("JSON task"), &mut env.ctx).unwrap();

    let out = run_next(
        &env,
        next_cmd::Args {
            json: true,
            fields: vec!["id".into(), "title".into(), "score".into()],
            ..next_args(None)
        },
    )
    .unwrap();
    let items: serde_json::Value = serde_json::from_str(&out).unwrap();
    let items = items.as_array().expect("next --json is an array");
    assert_eq!(items.len(), 1, "{out}");
    for row in items {
        assert!(row.get("score").is_some(), "score was asked for: {out}");
        let keys: Vec<&str> = row["task"]
            .as_object()
            .expect("a task object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["id", "title"], "{out}");
    }
}

#[test]
fn next_json_drops_the_score_unless_it_is_named() {
    let mut env = common::setup();
    add::run(add_args("JSON task"), &mut env.ctx).unwrap();

    let out = run_next(
        &env,
        next_cmd::Args {
            json: true,
            fields: vec!["title".into()],
            ..next_args(None)
        },
    )
    .unwrap();
    let items: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(items[0].get("score").is_none(), "{out}");
}

#[test]
fn next_fields_without_json_is_a_usage_error() {
    let mut env = common::setup();
    add::run(add_args("JSON task"), &mut env.ctx).unwrap();

    let err = run_next(
        &env,
        next_cmd::Args {
            fields: vec!["id".into()],
            ..next_args(None)
        },
    )
    .expect_err("--fields with table output must be refused, not ignored")
    .to_string();
    assert!(err.contains("--json"), "the message names the fix: {err}");
}

#[test]
fn next_rejects_an_unknown_field_name() {
    let env = common::setup();
    let err = run_next(
        &env,
        next_cmd::Args {
            json: true,
            fields: vec!["nosuchfield".into()],
            ..next_args(None)
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("unknown field"), "{err}");
}

#[test]
fn next_empty_list_does_not_error() {
    let env = common::setup();
    next_cmd::run(next_args(None), &env.ctx).unwrap();
}

#[test]
fn next_typoed_flag_in_filter_tokens_rejected() {
    // An unknown `--flag` swallowed into the trailing filter tokens must
    // error clearly instead of being misread as a tag exclusion.
    let env = common::setup();
    let err = next_cmd::run(
        next_cmd::Args {
            tokens: vec!["--al".to_string()],
            ..next_args(None)
        },
        &env.ctx,
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("unrecognised flag"), "unexpected error: {msg}");
    assert!(msg.contains("--al"), "error must name the token: {msg}");
}
