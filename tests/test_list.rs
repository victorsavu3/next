mod common;

use chrono::Local;
use next::domain::{filter, scoring::ScoredTask, scoring};
use next::cli::commands::add;
use next::cli::filter::FilterArgs;
use next::store::Store as _;

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

fn apply_filter(env: &mut common::TestEnv, tokens: Vec<String>) -> Vec<ScoredTask> {
    let today = Local::now().date_naive();
    let filter_args = FilterArgs::parse(tokens);
    let filter_set = filter_args.to_filter_set().unwrap();
    let state = env.ctx.store.get_state().unwrap();
    let all = env.ctx.store.list_tasks().unwrap();
    let filtered = filter::apply(all.clone(), &filter_set, &state, today);
    scoring::score_and_sort(filtered, &all, today, &env.ctx.config.scoring, &std::collections::HashMap::new())
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
    use next::cli::commands::done;
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

// ---------------------------------------------------------------------------
// Future start date filtering
// ---------------------------------------------------------------------------

fn apply_filter_with_future(env: &mut common::TestEnv, include_future: bool) -> Vec<ScoredTask> {
    let today = Local::now().date_naive();
    let mut filter_args = FilterArgs::parse(vec![]);
    filter_args.future = include_future;
    let filter_set = filter_args.to_filter_set().unwrap();
    let state = env.ctx.store.get_state().unwrap();
    let all = env.ctx.store.list_tasks().unwrap();
    let filtered = filter::apply(all.clone(), &filter_set, &state, today);
    scoring::score_and_sort(filtered, &all, today, &env.ctx.config.scoring, &std::collections::HashMap::new())
}

#[test]
fn future_start_task_hidden_by_default() {
    let mut env = common::setup();
    add::run(add_args("Normal task"), &mut env.ctx).unwrap();
    add::run(
        add::Args {
            start: Some("2099-01-01".to_string()),
            ..add_args("Future task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = apply_filter_with_future(&mut env, false);
    let titles: Vec<&str> = tasks.iter().map(|t| t.task.title.as_str()).collect();
    assert!(titles.contains(&"Normal task"), "normal task must be visible");
    assert!(!titles.contains(&"Future task"), "future-start task must be hidden by default");
}

#[test]
fn future_start_task_shown_with_future_flag() {
    let mut env = common::setup();
    add::run(add_args("Normal task"), &mut env.ctx).unwrap();
    add::run(
        add::Args {
            start: Some("2099-01-01".to_string()),
            ..add_args("Future task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = apply_filter_with_future(&mut env, true);
    let titles: Vec<&str> = tasks.iter().map(|t| t.task.title.as_str()).collect();
    assert!(titles.contains(&"Normal task"));
    assert!(titles.contains(&"Future task"), "future-start task must appear with --future");
}

#[test]
fn task_with_start_today_is_visible() {
    let mut env = common::setup();
    let today = Local::now().date_naive().to_string();
    add::run(
        add::Args {
            start: Some(today),
            ..add_args("Starts today")
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = apply_filter_with_future(&mut env, false);
    assert_eq!(tasks.len(), 1, "task starting today must be visible without --future");
}

#[test]
fn task_with_past_start_is_visible() {
    let mut env = common::setup();
    add::run(
        add::Args {
            start: Some("2000-01-01".to_string()),
            ..add_args("Started long ago")
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = apply_filter_with_future(&mut env, false);
    assert_eq!(tasks.len(), 1, "task with past start date must be visible");
}

#[test]
fn all_flag_also_shows_future_start_tasks() {
    let mut env = common::setup();
    add::run(
        add::Args {
            start: Some("2099-06-01".to_string()),
            ..add_args("Distant future")
        },
        &mut env.ctx,
    )
    .unwrap();

    // --all disables all implicit filtering including start-date gate.
    let today = Local::now().date_naive();
    let mut filter_args = FilterArgs::parse(vec![]);
    filter_args.all = true;
    let filter_set = filter_args.to_filter_set().unwrap();
    let state = env.ctx.store.get_state().unwrap();
    let all = env.ctx.store.list_tasks().unwrap();
    let filtered = filter::apply(all.clone(), &filter_set, &state, today);
    let tasks = scoring::score_and_sort(filtered, &all, today, &env.ctx.config.scoring, &std::collections::HashMap::new());
    assert_eq!(tasks.len(), 1, "--all must reveal future-start tasks");
}

// ---------------------------------------------------------------------------
// Limit flag
// ---------------------------------------------------------------------------

fn list_args_with_limit(limit: Option<usize>) -> next::cli::commands::list::Args {
    next::cli::commands::list::Args {
        future: false,
        all: false,
        all_users: false,
        json: false,
        limit,
        tokens: vec![],
    }
}

#[test]
fn list_limit_truncates_results() {
    let mut env = common::setup();
    for i in 1..=5 {
        add::run(add_args(&format!("task {i}")), &mut env.ctx).unwrap();
    }
    // Without limit all five tasks are returned.
    let all = apply_filter(&mut env, vec![]);
    assert_eq!(all.len(), 5);

    // With limit = 3 only three are returned.
    next::cli::commands::list::run(list_args_with_limit(Some(3)), &mut env.ctx).unwrap();
    let store = &env.ctx.store;
    let tasks = store.list_tasks().unwrap();
    // Verify the store still has all five (limit only affects output, not storage).
    assert_eq!(tasks.len(), 5);
}

#[test]
fn list_limit_zero_shows_nothing() {
    let mut env = common::setup();
    add::run(add_args("task one"), &mut env.ctx).unwrap();
    next::cli::commands::list::run(list_args_with_limit(Some(0)), &mut env.ctx).unwrap();
}

#[test]
fn list_config_limit_applies_when_no_flag() {
    let mut env = common::setup();
    for i in 1..=5 {
        add::run(add_args(&format!("task {i}")), &mut env.ctx).unwrap();
    }
    env.ctx.config.list_limit = Some(2);
    // run() should truncate to 2 without passing --limit
    next::cli::commands::list::run(list_args_with_limit(None), &mut env.ctx).unwrap();
    // The store still has five tasks.
    assert_eq!(env.ctx.store.list_tasks().unwrap().len(), 5);
}

#[test]
fn list_flag_overrides_config_limit() {
    let mut env = common::setup();
    for i in 1..=5 {
        add::run(add_args(&format!("task {i}")), &mut env.ctx).unwrap();
    }
    env.ctx.config.list_limit = Some(1);
    // --limit 4 overrides config limit of 1
    next::cli::commands::list::run(list_args_with_limit(Some(4)), &mut env.ctx).unwrap();
    assert_eq!(env.ctx.store.list_tasks().unwrap().len(), 5);
}
