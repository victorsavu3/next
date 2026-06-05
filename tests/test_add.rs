mod common;

use next::cli::commands::add;

fn args(title: &str) -> add::Args {
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

#[test]
fn add_basic_task() {
    let mut env = common::setup();
    add::run(args("Buy milk"), &mut env.ctx).unwrap();

    let tasks = env.ctx.repo.store.list_tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "Buy milk");
}

#[test]
fn add_with_slug_resolves_by_slug() {
    let mut env = common::setup();
    let a = add::Args {
        slug: Some("water-plants".to_string()),
        ..args("Water plants")
    };
    add::run(a, &mut env.ctx).unwrap();

    let found = env
        .ctx
        .repo
        .store
        .get_task_by_slug("water-plants")
        .unwrap()
        .expect("task not found by slug");
    assert_eq!(found.title, "Water plants");
}

#[test]
fn add_with_due_date() {
    let mut env = common::setup();
    let a = add::Args {
        due: Some("2026-12-31".to_string()),
        ..args("End of year")
    };
    add::run(a, &mut env.ctx).unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(task.due.is_some());
    assert_eq!(task.due.unwrap().to_string(), "2026-12-31");
}

#[test]
fn add_with_context_tag() {
    let mut env = common::setup();
    let a = add::Args {
        tags: vec!["@work".to_string()],
        ..args("Write report")
    };
    add::run(a, &mut env.ctx).unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(task.tags.contains(&"@work".to_string()));
}

#[test]
fn add_with_resource_tag() {
    let mut env = common::setup();
    let a = add::Args {
        tags: vec!["#printer".to_string()],
        ..args("Print document")
    };
    add::run(a, &mut env.ctx).unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(task.tags.contains(&"#printer".to_string()));
}

#[test]
fn add_with_freeform_tag() {
    let mut env = common::setup();
    let a = add::Args {
        tags: vec!["urgent".to_string()],
        ..args("Fix bug")
    };
    add::run(a, &mut env.ctx).unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(task.tags.contains(&"urgent".to_string()));
}

#[test]
fn add_with_invalid_tag_is_rejected() {
    let mut env = common::setup();
    let a = add::Args {
        tags: vec!["123invalid".to_string()],
        ..args("Bad tag")
    };
    let err = add::run(a, &mut env.ctx).unwrap_err();
    assert!(
        err.to_string().contains("must start with a letter"),
        "unexpected error: {err}"
    );

    // No task should have been saved.
    assert!(env.ctx.repo.store.list_tasks().unwrap().is_empty());
}

#[test]
fn add_with_parent() {
    let mut env = common::setup();
    let parent_args = add::Args {
        slug: Some("parent-task".to_string()),
        ..args("Parent")
    };
    add::run(parent_args, &mut env.ctx).unwrap();

    let child_args = add::Args {
        parent: Some("parent-task".to_string()),
        ..args("Child")
    };
    add::run(child_args, &mut env.ctx).unwrap();

    let all = env.ctx.repo.store.list_tasks().unwrap();
    assert_eq!(all.len(), 2);

    let parent = env
        .ctx
        .repo
        .store
        .get_task_by_slug("parent-task")
        .unwrap()
        .unwrap();
    let child = all.iter().find(|t| t.title == "Child").unwrap();
    assert_eq!(child.parent_id, Some(parent.id));
}

#[test]
fn add_with_description() {
    let mut env = common::setup();
    let a = add::Args {
        description: Some("Pick up 2% milk from the corner store.".to_string()),
        ..args("Buy milk")
    };
    add::run(a, &mut env.ctx).unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert_eq!(
        task.description.as_deref(),
        Some("Pick up 2% milk from the corner store.")
    );
}

#[test]
fn add_with_url() {
    let mut env = common::setup();
    let a = add::Args {
        url: Some("https://example.com/ticket/42".to_string()),
        ..args("Fix ticket")
    };
    add::run(a, &mut env.ctx).unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert_eq!(task.url.as_deref(), Some("https://example.com/ticket/42"));
}

#[test]
fn add_with_invalid_url_is_rejected() {
    let mut env = common::setup();
    let a = add::Args {
        url: Some("ftp://bad-scheme.example.com".to_string()),
        ..args("Bad URL task")
    };
    let err = add::run(a, &mut env.ctx).unwrap_err();
    assert!(
        err.to_string().contains("http://") || err.to_string().contains("https://"),
        "unexpected error: {err}"
    );
    assert!(env.ctx.repo.store.list_tasks().unwrap().is_empty());
}

#[test]
fn multiple_tasks_stored_independently() {
    let mut env = common::setup();
    add::run(args("Task A"), &mut env.ctx).unwrap();
    add::run(args("Task B"), &mut env.ctx).unwrap();
    add::run(args("Task C"), &mut env.ctx).unwrap();

    let all = env.ctx.repo.store.list_tasks().unwrap();
    assert_eq!(all.len(), 3);

    let titles: std::collections::HashSet<&str> = all.iter().map(|t| t.title.as_str()).collect();
    assert!(titles.contains("Task A"));
    assert!(titles.contains("Task B"));
    assert!(titles.contains("Task C"));
}

// ---------------------------------------------------------------------------
// Auto-apply active context tags
// ---------------------------------------------------------------------------

#[test]
fn add_auto_applies_active_context_when_no_context_tag() {
    use next::core::domain::state::GlobalState;
    let mut env = common::setup();

    // Set @work as the active context.
    let state = GlobalState { active_contexts: vec!["@work".to_string()], ..Default::default() };
    env.ctx.repo.store.save_state(&state).unwrap();

    add::run(args("No-context task"), &mut env.ctx).unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(
        task.tags.contains(&"@work".to_string()),
        "active context @work should be auto-applied: {:?}",
        task.tags
    );
}

#[test]
fn add_does_not_auto_apply_when_context_tag_already_present() {
    use next::core::domain::state::GlobalState;
    let mut env = common::setup();

    let state = GlobalState { active_contexts: vec!["@work".to_string()], ..Default::default() };
    env.ctx.repo.store.save_state(&state).unwrap();

    add::run(
        add::Args { tags: vec!["@home".to_string()], ..args("Home task") },
        &mut env.ctx,
    ).unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(task.tags.contains(&"@home".to_string()), "explicit tag kept");
    assert!(!task.tags.contains(&"@work".to_string()), "@work must not be auto-applied when user already has a context tag");
}

#[test]
fn add_no_auto_apply_when_no_active_context() {
    let mut env = common::setup();
    add::run(args("Plain task"), &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(
        task.tags.iter().all(|t| !t.starts_with('@')),
        "no context tags should be added when no active context is set"
    );
}
