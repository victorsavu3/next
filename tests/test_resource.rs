mod common;

use chrono::Local;
use next::cli::commands::{add, resource};
use next::store::Store as _;
use next::domain::{filter, scoring};
use next::cli::filter::FilterArgs;

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

fn visible_titles(env: &mut common::TestEnv) -> Vec<String> {
    let today = Local::now().date_naive();
    let state = env.ctx.store.get_state().unwrap();
    let all = env.ctx.store.list_tasks().unwrap();
    let filtered = filter::apply(all.clone(), &FilterArgs::parse(vec![]).to_filter_set().unwrap(), &state, today);
    let scored = scoring::score_and_sort(filtered, &all, today, &env.ctx.config.scoring, &std::collections::HashMap::new());
    scored.into_iter().map(|s| s.task.title).collect()
}

// ---------------------------------------------------------------------------
// next resource set
// ---------------------------------------------------------------------------

#[test]
fn resource_set_off_marks_unavailable() {
    let mut env = common::setup();
    resource::run(
        resource::Args {
            json: false,
            subcommand: Some(resource::ResourceSubcommand::Set(resource::SetArgs {
                resource: "#printer".into(),
                availability: resource::Availability::Off,
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let state = env.ctx.store.get_state().unwrap();
    assert_eq!(state.resources.get("printer"), Some(&false));
}

#[test]
fn resource_set_on_marks_available() {
    let mut env = common::setup();
    // Set off then back on.
    for avail in [resource::Availability::Off, resource::Availability::On] {
        resource::run(
            resource::Args {
                json: false,
                subcommand: Some(resource::ResourceSubcommand::Set(resource::SetArgs {
                    resource: "#laptop".into(),
                    availability: avail,
                })),
            },
            &mut env.ctx,
        )
        .unwrap();
    }

    let state = env.ctx.store.get_state().unwrap();
    assert_eq!(state.resources.get("laptop"), Some(&true));
}

#[test]
fn resource_set_missing_hash_prefix_errors() {
    let mut env = common::setup();
    let err = resource::run(
        resource::Args {
            json: false,
            subcommand: Some(resource::ResourceSubcommand::Set(resource::SetArgs {
                resource: "printer".into(),
                availability: resource::Availability::Off,
            })),
        },
        &mut env.ctx,
    )
    .unwrap_err();
    assert!(err.to_string().contains("'#'"), "unexpected: {err}");
}

#[test]
fn resource_set_off_hides_tagged_tasks() {
    let mut env = common::setup();
    add::run(
        add::Args { tags: vec!["#printer".into()], ..add_args("Print document") },
        &mut env.ctx,
    )
    .unwrap();
    add::run(add_args("Unrelated task"), &mut env.ctx).unwrap();

    resource::run(
        resource::Args {
            json: false,
            subcommand: Some(resource::ResourceSubcommand::Set(resource::SetArgs {
                resource: "#printer".into(),
                availability: resource::Availability::Off,
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let titles = visible_titles(&mut env);
    assert!(!titles.contains(&"Print document".to_string()));
    assert!(titles.contains(&"Unrelated task".to_string()));
}

#[test]
fn resource_set_on_restores_hidden_tasks() {
    let mut env = common::setup();
    add::run(
        add::Args { tags: vec!["#laptop".into()], ..add_args("Laptop task") },
        &mut env.ctx,
    )
    .unwrap();

    for avail in [resource::Availability::Off, resource::Availability::On] {
        resource::run(
            resource::Args {
                json: false,
                subcommand: Some(resource::ResourceSubcommand::Set(resource::SetArgs {
                    resource: "#laptop".into(),
                    availability: avail,
                })),
            },
            &mut env.ctx,
        )
        .unwrap();
    }

    let titles = visible_titles(&mut env);
    assert!(titles.contains(&"Laptop task".to_string()));
}

#[test]
fn resource_show_does_not_error() {
    let mut env = common::setup();
    resource::run(resource::Args { json: false, subcommand: None }, &mut env.ctx).unwrap();
}

#[test]
fn resource_state_survives_store_reload() {
    let mut env = common::setup();
    resource::run(
        resource::Args {
            json: false,
            subcommand: Some(resource::ResourceSubcommand::Set(resource::SetArgs {
                resource: "#vacation".into(),
                availability: resource::Availability::Off,
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let (fresh_store, _) = next::storage::open(env.ctx.repo_root.clone()).unwrap();
    let state = fresh_store.get_state().unwrap();
    assert_eq!(state.resources.get("vacation"), Some(&false));
}
