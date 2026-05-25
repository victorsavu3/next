mod common;

use next_cli::cli::commands::{add, data};

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
        stage: None,
        wait_for: None,
        recur_schedule: None,
        recur_completion: None,
        long_term: false,
        adjust: None,
        json: false,
    }
}

fn set(id: &str, key: &str, value: &str) -> data::Args {
    data::Args {
        subcommand: data::DataSubcommand::Set(data::SetArgs {
            id: id.to_string(),
            key: key.to_string(),
            value: value.to_string(),
        }),
    }
}

fn unset(id: &str, key: &str) -> data::Args {
    data::Args {
        subcommand: data::DataSubcommand::Unset(data::UnsetArgs {
            id: id.to_string(),
            key: key.to_string(),
        }),
    }
}

fn get(id: &str, key: &str) -> data::Args {
    data::Args {
        subcommand: data::DataSubcommand::Get(data::GetArgs {
            id: id.to_string(),
            key: key.to_string(),
        }),
    }
}

#[test]
fn data_set_string_value() {
    let mut env = common::setup();
    add::run(add::Args { slug: Some("t".to_string()), ..add_args("Task") }, &mut env.ctx).unwrap();

    data::run(set("t", "source", "github"), &mut env.ctx).unwrap();

    let task = env.ctx.store.list_tasks().unwrap().remove(0);
    assert_eq!(task.data["source"], serde_json::json!("github"));
}

#[test]
fn data_set_number_value() {
    let mut env = common::setup();
    add::run(add::Args { slug: Some("t".to_string()), ..add_args("Task") }, &mut env.ctx).unwrap();

    data::run(set("t", "score", "42"), &mut env.ctx).unwrap();

    let task = env.ctx.store.list_tasks().unwrap().remove(0);
    assert_eq!(task.data["score"], serde_json::json!(42));
}

#[test]
fn data_set_bool_value() {
    let mut env = common::setup();
    add::run(add::Args { slug: Some("t".to_string()), ..add_args("Task") }, &mut env.ctx).unwrap();

    data::run(set("t", "urgent", "true"), &mut env.ctx).unwrap();

    let task = env.ctx.store.list_tasks().unwrap().remove(0);
    assert_eq!(task.data["urgent"], serde_json::json!(true));
}

#[test]
fn data_set_updates_existing_key() {
    let mut env = common::setup();
    add::run(add::Args { slug: Some("t".to_string()), ..add_args("Task") }, &mut env.ctx).unwrap();

    data::run(set("t", "phase", "alpha"), &mut env.ctx).unwrap();
    data::run(set("t", "phase", "beta"), &mut env.ctx).unwrap();

    let task = env.ctx.store.list_tasks().unwrap().remove(0);
    assert_eq!(task.data["phase"], serde_json::json!("beta"));
    assert_eq!(task.data.len(), 1);
}

#[test]
fn data_set_multiple_independent_keys() {
    let mut env = common::setup();
    add::run(add::Args { slug: Some("t".to_string()), ..add_args("Task") }, &mut env.ctx).unwrap();

    data::run(set("t", "key1", "a"), &mut env.ctx).unwrap();
    data::run(set("t", "key2", "b"), &mut env.ctx).unwrap();

    let task = env.ctx.store.list_tasks().unwrap().remove(0);
    assert_eq!(task.data["key1"], serde_json::json!("a"));
    assert_eq!(task.data["key2"], serde_json::json!("b"));
}

#[test]
fn data_unset_removes_key() {
    let mut env = common::setup();
    add::run(add::Args { slug: Some("t".to_string()), ..add_args("Task") }, &mut env.ctx).unwrap();

    data::run(set("t", "remove-me", "yes"), &mut env.ctx).unwrap();
    data::run(set("t", "keep-me", "yes"), &mut env.ctx).unwrap();
    data::run(unset("t", "remove-me"), &mut env.ctx).unwrap();

    let task = env.ctx.store.list_tasks().unwrap().remove(0);
    assert!(!task.data.contains_key("remove-me"));
    assert!(task.data.contains_key("keep-me"));
}

#[test]
fn data_unset_missing_key_errors() {
    let mut env = common::setup();
    add::run(add::Args { slug: Some("t".to_string()), ..add_args("Task") }, &mut env.ctx).unwrap();

    let err = data::run(unset("t", "no-such-key"), &mut env.ctx).unwrap_err();
    assert!(
        err.to_string().contains("no-such-key"),
        "unexpected error: {err}"
    );
}

#[test]
fn data_get_returns_value() {
    let mut env = common::setup();
    add::run(add::Args { slug: Some("t".to_string()), ..add_args("Task") }, &mut env.ctx).unwrap();
    data::run(set("t", "color", "blue"), &mut env.ctx).unwrap();

    // get succeeds (output goes to stdout — just assert it doesn't error).
    data::run(get("t", "color"), &mut env.ctx).unwrap();
}

#[test]
fn data_get_missing_key_errors() {
    let mut env = common::setup();
    add::run(add::Args { slug: Some("t".to_string()), ..add_args("Task") }, &mut env.ctx).unwrap();

    let err = data::run(get("t", "missing"), &mut env.ctx).unwrap_err();
    assert!(
        err.to_string().contains("missing"),
        "unexpected error: {err}"
    );
}

#[test]
fn data_set_errors_on_nonexistent_task() {
    let mut env = common::setup();
    let err = data::run(set("ghost", "k", "v"), &mut env.ctx).unwrap_err();
    assert!(
        err.to_string().contains("ghost") || err.to_string().contains("not found"),
        "unexpected error: {err}"
    );
}

#[test]
fn data_set_rejects_null_json() {
    let mut env = common::setup();
    add::run(add::Args { slug: Some("t".to_string()), ..add_args("Task") }, &mut env.ctx).unwrap();

    let err = data::run(set("t", "k", "null"), &mut env.ctx).unwrap_err();
    assert!(
        err.to_string().contains("null"),
        "unexpected error: {err}"
    );
}
