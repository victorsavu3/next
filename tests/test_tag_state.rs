//! Integration tests for the unified tag state (`next tag require|exclude|
//! accept|clear-state`), which replaced `next context` and `next resource`.
//!
//! The theme throughout: a `@context`, a `#resource` and a freeform label are
//! the same thing to the state model. Where a test could be written for one
//! kind, it is written for all three instead.

mod common;

use chrono::Local;
use next::cli::commands::{add, tag};
use next::core::domain::state::TagState;
use next::core::store::Store as _;
use next::core::FilterArgs;
use next::core::{domain::filter, scoring};

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
        recur_snap_leeway: None,
        quiet: false,
        long_term: false,
        adjust: None,
        json: false,
    }
}

fn add_tagged(env: &mut common::TestEnv, title: &str, tags: &[&str]) {
    add::run(
        add::Args {
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..add_args(title)
        },
        &mut env.ctx,
    )
    .unwrap();
}

/// Runs the default listing pipeline and returns the visible titles.
fn visible_titles(env: &mut common::TestEnv) -> Vec<String> {
    let today = Local::now().date_naive();
    let state = env.ctx.repo.store.get_state().unwrap();
    let all = env.ctx.repo.store.list_tasks().unwrap();
    let filtered = filter::apply(
        all.clone(),
        &FilterArgs::parse(vec![]).unwrap().to_filter_set().unwrap(),
        &state,
        today,
        &std::collections::HashMap::new(),
    );
    let scored = scoring::score_and_sort(
        filtered,
        &all,
        today,
        &env.ctx.repo.scoring,
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
    );
    scored.into_iter().map(|s| s.task.title).collect()
}

fn run_state(env: &mut common::TestEnv, sub: tag::TagSubcommand) {
    tag::run(
        tag::Args {
            subcommand: Some(sub),
        },
        &mut env.ctx,
    )
    .unwrap();
}

fn state_args(tags: &[&str]) -> tag::TagStateArgs {
    tag::TagStateArgs {
        tags: tags.iter().map(|s| s.to_string()).collect(),
    }
}

// ---------------------------------------------------------------------------
// Setting state
// ---------------------------------------------------------------------------

#[test]
fn include_exclude_and_default_apply_to_every_tag_kind() {
    let mut env = common::setup();

    run_state(
        &mut env,
        tag::TagSubcommand::Require(state_args(&["@work", "#laptop", "urgent"])),
    );
    let state = env.ctx.repo.store.get_state().unwrap();
    for t in ["@work", "#laptop", "urgent"] {
        assert_eq!(state.state_of(t), Some(TagState::Required), "{t}");
    }

    run_state(
        &mut env,
        tag::TagSubcommand::Exclude(state_args(&["@work", "#laptop", "urgent"])),
    );
    let state = env.ctx.repo.store.get_state().unwrap();
    for t in ["@work", "#laptop", "urgent"] {
        assert_eq!(state.state_of(t), Some(TagState::Excluded), "{t}");
    }
}

#[test]
fn clear_state_drops_entries_and_clears_all_with_no_arguments() {
    let mut env = common::setup();
    run_state(
        &mut env,
        tag::TagSubcommand::Require(state_args(&["@work", "@home"])),
    );

    run_state(
        &mut env,
        tag::TagSubcommand::ClearState(tag::ClearStateArgs {
            tags: vec!["@work".into()],
        }),
    );
    let state = env.ctx.repo.store.get_state().unwrap();
    assert!(!state.tags.contains_key("@work"));
    assert_eq!(state.state_of("@home"), Some(TagState::Required));

    run_state(
        &mut env,
        tag::TagSubcommand::ClearState(tag::ClearStateArgs { tags: vec![] }),
    );
    assert!(env.ctx.repo.store.get_state().unwrap().tags.is_empty());
}

#[test]
fn a_malformed_tag_is_rejected() {
    let mut env = common::setup();
    let err = tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::Require(state_args(&["1bad"]))),
        },
        &mut env.ctx,
    )
    .unwrap_err();
    assert!(err.to_string().contains("must start with a letter"));
}

// ---------------------------------------------------------------------------
// How state filters the listing
// ---------------------------------------------------------------------------

#[test]
fn excluding_hides_tasks_of_any_tag_kind() {
    let mut env = common::setup();
    add_tagged(&mut env, "Work thing", &["@work"]);
    add_tagged(&mut env, "Printing", &["#printer"]);
    add_tagged(&mut env, "Errand", &["errand"]);
    add_tagged(&mut env, "Untagged", &[]);

    run_state(
        &mut env,
        tag::TagSubcommand::Exclude(state_args(&["#printer", "errand"])),
    );

    let titles = visible_titles(&mut env);
    assert!(titles.contains(&"Work thing".to_string()));
    assert!(titles.contains(&"Untagged".to_string()));
    assert!(!titles.contains(&"Printing".to_string()));
    assert!(!titles.contains(&"Errand".to_string()));
}

#[test]
fn including_a_resource_narrows_the_list_to_it() {
    // Impossible under the old model, where a resource could only be marked
    // unavailable: "show me only what I can do at the printer".
    let mut env = common::setup();
    add_tagged(&mut env, "Printing", &["#printer"]);
    add_tagged(&mut env, "Work thing", &["@work"]);
    add_tagged(&mut env, "Untagged", &[]);

    run_state(
        &mut env,
        tag::TagSubcommand::Require(state_args(&["#printer"])),
    );

    assert_eq!(visible_titles(&mut env), vec!["Printing".to_string()]);
}

#[test]
fn including_hides_untagged_tasks_too() {
    // The context-neutral exemption is gone: including a tag means the list is
    // that tag's tasks.
    let mut env = common::setup();
    add_tagged(&mut env, "Work thing", &["@work"]);
    add_tagged(&mut env, "Untagged", &[]);

    run_state(
        &mut env,
        tag::TagSubcommand::Require(state_args(&["@work"])),
    );

    assert_eq!(visible_titles(&mut env), vec!["Work thing".to_string()]);
}

#[test]
fn state_is_inherited_by_descendant_tags() {
    let mut env = common::setup();
    add_tagged(&mut env, "Kitchen", &["@home/kitchen"]);
    add_tagged(&mut env, "Garden", &["@home/garden"]);
    add_tagged(&mut env, "Work thing", &["@work"]);

    run_state(
        &mut env,
        tag::TagSubcommand::Exclude(state_args(&["@home"])),
    );

    let titles = visible_titles(&mut env);
    assert_eq!(titles, vec!["Work thing".to_string()]);
}

#[test]
fn a_pinned_default_opts_a_child_out_of_its_parents_state() {
    let mut env = common::setup();
    add_tagged(&mut env, "Kitchen", &["@home/kitchen"]);
    add_tagged(&mut env, "Garden", &["@home/garden"]);

    run_state(
        &mut env,
        tag::TagSubcommand::Exclude(state_args(&["@home"])),
    );
    run_state(
        &mut env,
        tag::TagSubcommand::Accept(state_args(&["@home/kitchen"])),
    );

    assert_eq!(visible_titles(&mut env), vec!["Kitchen".to_string()]);
}

#[test]
fn including_several_tags_is_a_disjunction() {
    let mut env = common::setup();
    add_tagged(&mut env, "Work thing", &["@work"]);
    add_tagged(&mut env, "Home thing", &["@home"]);
    add_tagged(&mut env, "Errand", &["errand"]);

    run_state(
        &mut env,
        tag::TagSubcommand::Require(state_args(&["@work", "@home"])),
    );

    let mut titles = visible_titles(&mut env);
    titles.sort();
    assert_eq!(
        titles,
        vec!["Home thing".to_string(), "Work thing".to_string()]
    );
}

#[test]
fn exclusion_wins_over_inclusion() {
    let mut env = common::setup();
    add_tagged(&mut env, "Needs printer", &["@work", "#printer"]);
    add_tagged(&mut env, "Plain work", &["@work"]);

    run_state(
        &mut env,
        tag::TagSubcommand::Require(state_args(&["@work"])),
    );
    run_state(
        &mut env,
        tag::TagSubcommand::Exclude(state_args(&["#printer"])),
    );

    assert_eq!(visible_titles(&mut env), vec!["Plain work".to_string()]);
}

#[test]
fn state_survives_a_reopen() {
    let mut env = common::setup();
    run_state(
        &mut env,
        tag::TagSubcommand::Exclude(state_args(&["#printer"])),
    );

    let root = env.ctx.repo.repo_root.clone();
    let (store, _vcs) = next::core::storage::open(root).unwrap();
    assert_eq!(
        store.get_state().unwrap().state_of("#printer"),
        Some(TagState::Excluded)
    );
}
