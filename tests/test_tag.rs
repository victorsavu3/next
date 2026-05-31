mod common;

use next::store::Store as _;
use next::cli::commands::{add, tag};
use next::domain::task::Priority;

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

    let (fresh_store, _) = next::storage::open(env.ctx.repo_root.clone()).unwrap();
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

// ---------------------------------------------------------------------------
// next tag show
// ---------------------------------------------------------------------------

#[test]
fn tag_show_empty_meta_does_not_error() {
    let mut env = common::setup();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::Show(tag::ShowArgs {
                tag: "@work".into(),
                json: false,
            })),
        },
        &mut env.ctx,
    )
    .unwrap();
}

#[test]
fn tag_show_json_output() {
    let mut env = common::setup();
    tag::run(describe("@work", "Office"), &mut env.ctx).unwrap();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::Show(tag::ShowArgs {
                tag: "@work".into(),
                json: true,
            })),
        },
        &mut env.ctx,
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// next tag set-url / clear-url
// ---------------------------------------------------------------------------

#[test]
fn tag_set_url_stores_url() {
    let mut env = common::setup();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::SetUrl(tag::SetUrlArgs {
                tag: "@work".into(),
                url: "https://example.com/work".into(),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let meta = env.ctx.store.get_tag_meta("@work").unwrap().unwrap();
    assert_eq!(meta.url.as_deref(), Some("https://example.com/work"));
}

#[test]
fn tag_set_url_preserves_description() {
    let mut env = common::setup();
    tag::run(describe("@work", "Office work"), &mut env.ctx).unwrap();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::SetUrl(tag::SetUrlArgs {
                tag: "@work".into(),
                url: "https://example.com".into(),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let meta = env.ctx.store.get_tag_meta("@work").unwrap().unwrap();
    assert_eq!(meta.description.as_deref(), Some("Office work"));
    assert_eq!(meta.url.as_deref(), Some("https://example.com"));
}

#[test]
fn tag_clear_url_removes_url() {
    let mut env = common::setup();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::SetUrl(tag::SetUrlArgs {
                tag: "python".into(),
                url: "https://python.org".into(),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::ClearUrl(tag::ClearUrlArgs {
                tag: "python".into(),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let meta = env.ctx.store.get_tag_meta("python").unwrap();
    assert!(meta.map(|m| m.url.is_none()).unwrap_or(true));
}

// ---------------------------------------------------------------------------
// next tag set-priority / clear-priority
// ---------------------------------------------------------------------------

#[test]
fn tag_set_priority_stores_priority() {
    let mut env = common::setup();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::SetPriority(tag::SetPriorityArgs {
                tag: "@work".into(),
                priority: "high".into(),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let meta = env.ctx.store.get_tag_meta("@work").unwrap().unwrap();
    assert_eq!(meta.priority, Some(Priority::High));
}

#[test]
fn tag_set_priority_invalid_errors() {
    let mut env = common::setup();
    let result = tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::SetPriority(tag::SetPriorityArgs {
                tag: "@work".into(),
                priority: "critical".into(),
            })),
        },
        &mut env.ctx,
    );
    assert!(result.is_err());
}

#[test]
fn tag_clear_priority_removes_priority() {
    let mut env = common::setup();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::SetPriority(tag::SetPriorityArgs {
                tag: "python".into(),
                priority: "low".into(),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::ClearPriority(tag::ClearPriorityArgs {
                tag: "python".into(),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let meta = env.ctx.store.get_tag_meta("python").unwrap();
    assert!(meta.map(|m| m.priority.is_none()).unwrap_or(true));
}

// ---------------------------------------------------------------------------
// next tag data set/get/unset/list
// ---------------------------------------------------------------------------

#[test]
fn tag_data_set_stores_value() {
    let mut env = common::setup();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::Data(tag::DataArgs {
                subcommand: tag::DataSubcommand::Set(tag::DataSetArgs {
                    tag: "@work".into(),
                    key: "team".into(),
                    value: "engineering".into(),
                }),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let meta = env.ctx.store.get_tag_meta("@work").unwrap().unwrap();
    assert_eq!(
        meta.data.get("team"),
        Some(&serde_json::Value::String("engineering".into()))
    );
}

#[test]
fn tag_data_set_json_value() {
    let mut env = common::setup();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::Data(tag::DataArgs {
                subcommand: tag::DataSubcommand::Set(tag::DataSetArgs {
                    tag: "python".into(),
                    key: "count".into(),
                    value: "42".into(),
                }),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let meta = env.ctx.store.get_tag_meta("python").unwrap().unwrap();
    assert_eq!(
        meta.data.get("count"),
        Some(&serde_json::Value::Number(42.into()))
    );
}

#[test]
fn tag_data_get_returns_value() {
    let mut env = common::setup();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::Data(tag::DataArgs {
                subcommand: tag::DataSubcommand::Set(tag::DataSetArgs {
                    tag: "@home".into(),
                    key: "color".into(),
                    value: "blue".into(),
                }),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::Data(tag::DataArgs {
                subcommand: tag::DataSubcommand::Get(tag::DataGetArgs {
                    tag: "@home".into(),
                    key: "color".into(),
                }),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();
}

#[test]
fn tag_data_get_missing_key_errors() {
    let mut env = common::setup();
    let result = tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::Data(tag::DataArgs {
                subcommand: tag::DataSubcommand::Get(tag::DataGetArgs {
                    tag: "@work".into(),
                    key: "missing".into(),
                }),
            })),
        },
        &mut env.ctx,
    );
    assert!(result.is_err());
}

#[test]
fn tag_data_unset_removes_key() {
    let mut env = common::setup();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::Data(tag::DataArgs {
                subcommand: tag::DataSubcommand::Set(tag::DataSetArgs {
                    tag: "rust".into(),
                    key: "edition".into(),
                    value: "2021".into(),
                }),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::Data(tag::DataArgs {
                subcommand: tag::DataSubcommand::Unset(tag::DataUnsetArgs {
                    tag: "rust".into(),
                    key: "edition".into(),
                }),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let meta = env.ctx.store.get_tag_meta("rust").unwrap();
    assert!(meta.map(|m| m.data.is_empty()).unwrap_or(true));
}

#[test]
fn tag_data_list_does_not_error() {
    let mut env = common::setup();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::Data(tag::DataArgs {
                subcommand: tag::DataSubcommand::Set(tag::DataSetArgs {
                    tag: "@work".into(),
                    key: "owner".into(),
                    value: "alice".into(),
                }),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::Data(tag::DataArgs {
                subcommand: tag::DataSubcommand::List(tag::DataListArgs {
                    tag: "@work".into(),
                    json: false,
                }),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();
}

#[test]
fn tag_meta_all_fields_survives_reload() {
    let mut env = common::setup();
    // Set description, url, priority, and data.
    tag::run(describe("@work", "Office"), &mut env.ctx).unwrap();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::SetUrl(tag::SetUrlArgs {
                tag: "@work".into(),
                url: "https://example.com".into(),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::SetPriority(tag::SetPriorityArgs {
                tag: "@work".into(),
                priority: "high".into(),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();
    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::Data(tag::DataArgs {
                subcommand: tag::DataSubcommand::Set(tag::DataSetArgs {
                    tag: "@work".into(),
                    key: "team".into(),
                    value: "eng".into(),
                }),
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let (fresh_store, _) = next::storage::open(env.ctx.repo_root.clone()).unwrap();
    let meta = fresh_store.get_tag_meta("@work").unwrap().unwrap();
    assert_eq!(meta.description.as_deref(), Some("Office"));
    assert_eq!(meta.url.as_deref(), Some("https://example.com"));
    assert_eq!(meta.priority, Some(Priority::High));
    assert_eq!(
        meta.data.get("team"),
        Some(&serde_json::Value::String("eng".into()))
    );
}

// ---------------------------------------------------------------------------
// Backward compatibility: files written with description-only format
// ---------------------------------------------------------------------------

#[test]
fn legacy_description_file_reads_as_tag_meta() {
    use std::fs;
    let env = common::setup();
    // Write a legacy single-field TOML file directly (using encoded path).
    let tags_dir = env.ctx.repo_root.join("tags");
    fs::create_dir_all(&tags_dir).unwrap();
    fs::write(tags_dir.join("__context__work.toml"), "description = \"Office tasks\"\n").unwrap();

    let meta = env.ctx.store.get_tag_meta("@work").unwrap().unwrap();
    assert_eq!(meta.description.as_deref(), Some("Office tasks"));
    assert!(meta.url.is_none());
    assert!(meta.data.is_empty());
    assert!(meta.priority.is_none());
}

