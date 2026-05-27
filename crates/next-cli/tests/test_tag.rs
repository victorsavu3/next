mod common;

use next::store::Store as _;
use next_cli::cli::commands::{add, tag};

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

fn describe(tag_str: &str, desc: &str) -> tag::Args {
    tag::Args {
        subcommand: Some(tag::TagSubcommand::Describe(tag::DescribeArgs {
            tag: tag_str.to_owned(),
            description: desc.to_owned(),
        })),
    }
}

fn clear_description(tag_str: &str) -> tag::Args {
    tag::Args {
        subcommand: Some(tag::TagSubcommand::ClearDescription(
            tag::ClearDescriptionArgs {
                tag: tag_str.to_owned(),
            },
        )),
    }
}

// ---------------------------------------------------------------------------
// next tag (list)
// ---------------------------------------------------------------------------

#[test]
fn tag_list_empty_when_no_tags() {
    let mut env = common::setup();
    tag::run(tag::Args { subcommand: None }, &mut env.ctx).unwrap();
}

#[test]
fn tag_list_shows_tags_from_tasks() {
    let mut env = common::setup();
    add::run(
        add::Args {
            tags: vec!["@work".into(), "#printer".into(), "python".into()],
            ..add_args("task one")
        },
        &mut env.ctx,
    )
    .unwrap();
    tag::run(tag::Args { subcommand: None }, &mut env.ctx).unwrap();
}

// ---------------------------------------------------------------------------
// next tag describe
// ---------------------------------------------------------------------------

#[test]
fn tag_describe_stores_description() {
    let mut env = common::setup();
    tag::run(describe("@work", "Tasks done at the office"), &mut env.ctx).unwrap();

    let desc = env.ctx.store.get_tag_description("@work").unwrap();
    assert_eq!(desc.as_deref(), Some("Tasks done at the office"));
}

#[test]
fn tag_describe_updates_existing_description() {
    let mut env = common::setup();
    tag::run(describe("@home", "first"), &mut env.ctx).unwrap();
    tag::run(describe("@home", "second"), &mut env.ctx).unwrap();

    let desc = env.ctx.store.get_tag_description("@home").unwrap();
    assert_eq!(desc.as_deref(), Some("second"));
}

#[test]
fn tag_describe_rejects_invalid_tag() {
    let mut env = common::setup();
    let result = tag::run(describe("bad tag!", "whatever"), &mut env.ctx);
    assert!(result.is_err(), "invalid tag should be rejected");
}

// ---------------------------------------------------------------------------
// next tag clear
// ---------------------------------------------------------------------------

#[test]
fn tag_clear_removes_description() {
    let mut env = common::setup();
    tag::run(describe("python", "Python tasks"), &mut env.ctx).unwrap();
    tag::run(clear_description("python"), &mut env.ctx).unwrap();

    let desc = env.ctx.store.get_tag_description("python").unwrap();
    assert!(desc.is_none());
}

#[test]
fn tag_clear_errors_when_no_description_exists() {
    let mut env = common::setup();
    let result = tag::run(clear_description("@nonexistent"), &mut env.ctx);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

#[test]
fn tag_description_survives_store_reload() {
    let mut env = common::setup();
    tag::run(describe("#vacation", "Away from keyboard"), &mut env.ctx).unwrap();

    let (fresh_store, _) = next_storage::open(env.ctx.repo_root.clone()).unwrap();
    let desc = fresh_store.get_tag_description("#vacation").unwrap();
    assert_eq!(desc.as_deref(), Some("Away from keyboard"));
}

// ---------------------------------------------------------------------------
// Hierarchical tags
// ---------------------------------------------------------------------------

#[test]
fn tag_describe_hierarchical_tag() {
    let mut env = common::setup();
    tag::run(describe("@home/kitchen", "Tasks in the kitchen"), &mut env.ctx).unwrap();

    let desc = env.ctx.store.get_tag_description("@home/kitchen").unwrap();
    assert_eq!(desc.as_deref(), Some("Tasks in the kitchen"));
}

#[test]
fn list_tag_descriptions_returns_all() {
    let mut env = common::setup();
    tag::run(describe("@work", "Work tasks"), &mut env.ctx).unwrap();
    tag::run(describe("#printer", "Office printer"), &mut env.ctx).unwrap();
    tag::run(describe("python", "Python projects"), &mut env.ctx).unwrap();

    let all = env.ctx.store.list_tag_descriptions().unwrap();
    assert_eq!(all.get("@work").map(String::as_str), Some("Work tasks"));
    assert_eq!(all.get("#printer").map(String::as_str), Some("Office printer"));
    assert_eq!(all.get("python").map(String::as_str), Some("Python projects"));
    assert_eq!(all.len(), 3);
}
