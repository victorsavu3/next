mod common;

use chrono::Local;
use next::cli::commands::{add, user};
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
// next user set / clear
// ---------------------------------------------------------------------------

#[test]
fn user_set_saves_active_users() {
    let mut env = common::setup();
    user::run(
        user::Args {
            subcommand: Some(user::UserSubcommand::Set(user::SetArgs {
                users: vec!["alice".into()],
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let state = env.ctx.store.get_state().unwrap();
    assert_eq!(state.active_users, vec!["alice".to_string()]);
}

#[test]
fn user_clear_removes_active_users() {
    let mut env = common::setup();
    user::run(
        user::Args {
            subcommand: Some(user::UserSubcommand::Set(user::SetArgs {
                users: vec!["alice".into()],
            })),
        },
        &mut env.ctx,
    )
    .unwrap();
    user::run(
        user::Args { subcommand: Some(user::UserSubcommand::Clear) },
        &mut env.ctx,
    )
    .unwrap();

    let state = env.ctx.store.get_state().unwrap();
    assert!(state.active_users.is_empty());
}

#[test]
fn user_show_does_not_error() {
    let mut env = common::setup();
    user::run(user::Args { subcommand: None }, &mut env.ctx).unwrap();
}

// ---------------------------------------------------------------------------
// next user list
// ---------------------------------------------------------------------------

#[test]
fn user_list_shows_assignees() {
    let mut env = common::setup();
    add::run(
        add::Args { assignee: Some("alice".into()), ..add_args("Alice task") },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args { assignee: Some("bob".into()), ..add_args("Bob task") },
        &mut env.ctx,
    )
    .unwrap();

    user::run(
        user::Args { subcommand: Some(user::UserSubcommand::List) },
        &mut env.ctx,
    )
    .unwrap();
}

#[test]
fn user_list_empty_when_no_assignees() {
    let mut env = common::setup();
    add::run(add_args("Unassigned task"), &mut env.ctx).unwrap();
    user::run(
        user::Args { subcommand: Some(user::UserSubcommand::List) },
        &mut env.ctx,
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Filter behaviour with active user
// ---------------------------------------------------------------------------

#[test]
fn user_filter_hides_other_users_tasks() {
    let mut env = common::setup();
    add::run(
        add::Args { assignee: Some("alice".into()), ..add_args("Alice task") },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args { assignee: Some("bob".into()), ..add_args("Bob task") },
        &mut env.ctx,
    )
    .unwrap();

    user::run(
        user::Args {
            subcommand: Some(user::UserSubcommand::Set(user::SetArgs {
                users: vec!["alice".into()],
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let titles = visible_titles(&mut env);
    assert!(titles.contains(&"Alice task".to_string()));
    assert!(!titles.contains(&"Bob task".to_string()));
}

#[test]
fn user_filter_keeps_unassigned_tasks() {
    let mut env = common::setup();
    add::run(
        add::Args { assignee: Some("alice".into()), ..add_args("Alice task") },
        &mut env.ctx,
    )
    .unwrap();
    add::run(add_args("Shared task"), &mut env.ctx).unwrap();

    user::run(
        user::Args {
            subcommand: Some(user::UserSubcommand::Set(user::SetArgs {
                users: vec!["alice".into()],
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let titles = visible_titles(&mut env);
    assert!(titles.contains(&"Alice task".to_string()), "assigned task must be visible");
    assert!(titles.contains(&"Shared task".to_string()), "unassigned task must always be visible");
}

#[test]
fn user_state_survives_store_reload() {
    let mut env = common::setup();
    user::run(
        user::Args {
            subcommand: Some(user::UserSubcommand::Set(user::SetArgs {
                users: vec!["carol".into()],
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    let (fresh_store, _) = next::storage::open(env.ctx.repo_root.clone()).unwrap();
    let state = fresh_store.get_state().unwrap();
    assert_eq!(state.active_users, vec!["carol".to_string()]);
}
