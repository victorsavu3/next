mod common;

use chrono::Local;
use next::cli::commands::{add, context};
use next::core::store::Store as _;
use next::core::{domain::filter, scoring};
use next::core::FilterArgs;

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

fn visible_titles(env: &mut common::TestEnv) -> Vec<String> {
    let today = Local::now().date_naive();
    let state = env.ctx.repo.store.get_state().unwrap();
    let all = env.ctx.repo.store.list_tasks().unwrap();
    let filtered = filter::apply(all.clone(), &FilterArgs::parse(vec![]).to_filter_set().unwrap(), &state, today);
    let scored = scoring::score_and_sort(filtered, &all, today, &env.ctx.config.scoring, &std::collections::HashMap::new());
    scored.into_iter().map(|s| s.task.title).collect()
}

// ---------------------------------------------------------------------------
// next context set / clear
// ---------------------------------------------------------------------------

#[test]
fn context_set_saves_active_contexts() {
    let mut env = common::setup();
    context::run(
        context::Args {
            subcommand: Some(context::ContextSubcommand::Set(context::ContextTagArgs {
                tags: vec!["@work".into()],
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let state = env.ctx.repo.store.get_state().unwrap();
    assert_eq!(state.active_contexts, vec!["@work".to_string()]);
}

#[test]
fn context_set_validates_at_prefix() {
    let mut env = common::setup();
    let err = context::run(
        context::Args {
            subcommand: Some(context::ContextSubcommand::Set(context::ContextTagArgs {
                tags: vec!["work".into()],
            })),
        },
        &mut env.ctx,
    )
    .unwrap_err();
    assert!(err.to_string().contains("'@'"), "unexpected: {err}");
}

#[test]
fn context_clear_removes_active_contexts() {
    let mut env = common::setup();
    context::run(
        context::Args {
            subcommand: Some(context::ContextSubcommand::Set(context::ContextTagArgs {
                tags: vec!["@work".into()],
            })),
        },
        &mut env.ctx,
    )
    .unwrap();
    context::run(
        context::Args { subcommand: Some(context::ContextSubcommand::Clear) },
        &mut env.ctx,
    )
    .unwrap();

    let state = env.ctx.repo.store.get_state().unwrap();
    assert!(state.active_contexts.is_empty());
}

#[test]
fn context_show_does_not_error() {
    let mut env = common::setup();
    context::run(context::Args { subcommand: None }, &mut env.ctx).unwrap();
}

// ---------------------------------------------------------------------------
// Filter behaviour with active context
// ---------------------------------------------------------------------------

#[test]
fn context_filter_hides_wrong_context_tasks() {
    let mut env = common::setup();
    add::run(
        add::Args { tags: vec!["@work".into()], ..add_args("Work task") },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args { tags: vec!["@home".into()], ..add_args("Home task") },
        &mut env.ctx,
    )
    .unwrap();

    context::run(
        context::Args {
            subcommand: Some(context::ContextSubcommand::Set(context::ContextTagArgs {
                tags: vec!["@work".into()],
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let titles = visible_titles(&mut env);
    assert!(titles.contains(&"Work task".to_string()));
    assert!(!titles.contains(&"Home task".to_string()));
}

#[test]
fn context_filter_keeps_context_neutral_tasks() {
    let mut env = common::setup();
    add::run(
        add::Args { tags: vec!["@work".into()], ..add_args("Work task") },
        &mut env.ctx,
    )
    .unwrap();
    add::run(add_args("No-context task"), &mut env.ctx).unwrap();

    context::run(
        context::Args {
            subcommand: Some(context::ContextSubcommand::Set(context::ContextTagArgs {
                tags: vec!["@work".into()],
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let titles = visible_titles(&mut env);
    assert!(titles.contains(&"Work task".to_string()), "work task must be visible");
    assert!(
        titles.contains(&"No-context task".to_string()),
        "context-neutral task must always be visible"
    );
}

#[test]
fn context_filter_shows_all_when_no_context_active() {
    let mut env = common::setup();
    add::run(
        add::Args { tags: vec!["@work".into()], ..add_args("Work task") },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args { tags: vec!["@home".into()], ..add_args("Home task") },
        &mut env.ctx,
    )
    .unwrap();

    let titles = visible_titles(&mut env);
    assert_eq!(titles.len(), 2);
}

#[test]
fn context_state_survives_store_reload() {
    let mut env = common::setup();
    context::run(
        context::Args {
            subcommand: Some(context::ContextSubcommand::Set(context::ContextTagArgs {
                tags: vec!["@home".into()],
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let (fresh_store, _) = next::core::storage::open(env.ctx.repo.repo_root.clone()).unwrap();
    let state = fresh_store.get_state().unwrap();
    assert_eq!(state.active_contexts, vec!["@home".to_string()]);
}
