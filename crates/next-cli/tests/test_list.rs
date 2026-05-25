mod common;

use chrono::Local;
use next::domain::{filter, scoring::ScoredTask, scoring};
use next_cli::cli::commands::add;
use next_cli::cli::filter::FilterArgs;

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
        data: vec![],
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

fn apply_filter(env: &mut common::TestEnv, tokens: Vec<String>) -> Vec<ScoredTask> {
    let today = Local::now().date_naive();
    let filter_args = FilterArgs::parse(tokens);
    let filter_set = filter_args.to_filter_set().unwrap();
    let state = env.ctx.store.get_state().unwrap();
    let all = env.ctx.store.list_tasks().unwrap();
    let filtered = filter::apply(all.clone(), &filter_set, &state, today);
    scoring::score_and_sort(filtered, &all, today, &env.ctx.config.scoring)
}

#[test]
fn list_returns_open_tasks() {
    let mut env = common::setup();
    add::run(add_args("Task one"), &mut env.ctx).unwrap();
    add::run(add_args("Task two"), &mut env.ctx).unwrap();

    let tasks = apply_filter(&mut env, vec![]);
    assert_eq!(tasks.len(), 2);
}

#[test]
fn list_filters_by_required_tag() {
    let mut env = common::setup();
    let with_tag = add::Args {
        tags: vec!["@work".to_string()],
        ..add_args("Work task")
    };
    add::run(with_tag, &mut env.ctx).unwrap();
    add::run(add_args("Home task"), &mut env.ctx).unwrap();

    let tasks = apply_filter(&mut env, vec!["+@work".to_string()]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task.title, "Work task");
}

#[test]
fn list_bare_token_acts_as_required_tag() {
    let mut env = common::setup();
    let with_tag = add::Args {
        tags: vec!["urgent".to_string()],
        ..add_args("Urgent task")
    };
    add::run(with_tag, &mut env.ctx).unwrap();
    add::run(add_args("Normal task"), &mut env.ctx).unwrap();

    // Bare "urgent" should behave like "+urgent".
    let tasks = apply_filter(&mut env, vec!["urgent".to_string()]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task.title, "Urgent task");
}

#[test]
fn list_excludes_by_negative_tag() {
    let mut env = common::setup();
    let with_tag = add::Args {
        tags: vec!["@work".to_string()],
        ..add_args("Work task")
    };
    add::run(with_tag, &mut env.ctx).unwrap();
    add::run(add_args("Home task"), &mut env.ctx).unwrap();

    let tasks = apply_filter(&mut env, vec!["-@work".to_string()]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task.title, "Home task");
}

#[test]
fn list_filters_by_multiple_tags() {
    let mut env = common::setup();

    let both = add::Args {
        tags: vec!["@work".to_string(), "urgent".to_string()],
        ..add_args("Urgent work task")
    };
    add::run(both, &mut env.ctx).unwrap();

    let only_work = add::Args {
        tags: vec!["@work".to_string()],
        ..add_args("Routine work task")
    };
    add::run(only_work, &mut env.ctx).unwrap();

    add::run(add_args("Unrelated"), &mut env.ctx).unwrap();

    // Require both @work AND urgent.
    let tasks = apply_filter(&mut env, vec!["+@work".to_string(), "+urgent".to_string()]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task.title, "Urgent work task");
}

#[test]
fn list_empty_when_no_tasks() {
    let mut env = common::setup();
    let tasks = apply_filter(&mut env, vec![]);
    assert!(tasks.is_empty());
}

#[test]
fn list_excludes_done_tasks_by_default() {
    let mut env = common::setup();
    add::run(add_args("Open task"), &mut env.ctx).unwrap();

    let done_args = add::Args {
        slug: Some("done-task".to_string()),
        ..add_args("Done task")
    };
    add::run(done_args, &mut env.ctx).unwrap();

    // Mark second task done.
    use next_cli::cli::commands::done;
    done::run(
        done::Args {
            id: "done-task".to_string(),
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = apply_filter(&mut env, vec![]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task.title, "Open task");
}
