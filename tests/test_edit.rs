mod common;

use next::cli::commands::{add, edit};

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

fn base_edit(id: &str) -> edit::Args {
    edit::Args {
        id: id.to_string(),
        title: None,
        due: None,
        start: None,
        priority: None,
        slug: None,
        assignee: None,
        clear_assignee: false,
        tags: vec![],
        remove_tags: vec![],
        parent: None,
        blocked_by: vec![],
        description: None,
        clear_description: false,
        url: None,
        clear_url: false,
        notes: None,
        recur_schedule: None,
        recur_completion: None,
        recur_snap: None,
        clear_recurrence: false,
        long_term: false,
        adjust: None,
        clear_due: false,
        clear_start: false,
        clear_parent: false,
        clear_blocked_by: false,
        json: false,
        tag_tokens: vec![],
    }
}

#[test]
fn edit_title() {
    let mut env = common::setup();
    let a = add::Args {
        slug: Some("my-task".to_string()),
        ..add_args("Old title")
    };
    add::run(a, &mut env.ctx).unwrap();

    edit::run(
        edit::Args {
            title: Some("New title".to_string()),
            ..base_edit("my-task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert_eq!(task.title, "New title");
}

#[test]
fn edit_add_tag_via_flag() {
    let mut env = common::setup();
    let a = add::Args {
        slug: Some("tag-task".to_string()),
        ..add_args("Tagged task")
    };
    add::run(a, &mut env.ctx).unwrap();

    edit::run(
        edit::Args {
            tags: vec!["@home".to_string()],
            ..base_edit("tag-task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(task.tags.contains(&"@home".to_string()));
}

#[test]
fn edit_add_tag_via_trailing_plus() {
    let mut env = common::setup();
    let a = add::Args {
        slug: Some("trailing-task".to_string()),
        ..add_args("Trailing tag task")
    };
    add::run(a, &mut env.ctx).unwrap();

    edit::run(
        edit::Args {
            tag_tokens: vec!["+@work".to_string()],
            ..base_edit("trailing-task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(task.tags.contains(&"@work".to_string()));
}

#[test]
fn edit_remove_tag_via_trailing_minus() {
    let mut env = common::setup();
    let a = add::Args {
        slug: Some("remove-tag-task".to_string()),
        tags: vec!["@work".to_string(), "@home".to_string()],
        ..add_args("Has tags")
    };
    add::run(a, &mut env.ctx).unwrap();

    edit::run(
        edit::Args {
            tag_tokens: vec!["-@work".to_string()],
            ..base_edit("remove-tag-task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(!task.tags.contains(&"@work".to_string()));
    assert!(task.tags.contains(&"@home".to_string()));
}

#[test]
fn edit_remove_tag_via_flag() {
    let mut env = common::setup();
    let a = add::Args {
        slug: Some("flag-remove-task".to_string()),
        tags: vec!["urgent".to_string(), "review".to_string()],
        ..add_args("Flagged task")
    };
    add::run(a, &mut env.ctx).unwrap();

    edit::run(
        edit::Args {
            remove_tags: vec!["urgent".to_string()],
            ..base_edit("flag-remove-task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(!task.tags.contains(&"urgent".to_string()));
    assert!(task.tags.contains(&"review".to_string()));
}

#[test]
fn edit_clear_due_date() {
    let mut env = common::setup();
    let a = add::Args {
        slug: Some("due-task".to_string()),
        due: Some("2026-12-31".to_string()),
        ..add_args("Due task")
    };
    add::run(a, &mut env.ctx).unwrap();

    {
        let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
        assert!(task.due.is_some(), "due date should be set after add");
    }

    edit::run(
        edit::Args {
            clear_due: true,
            ..base_edit("due-task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(task.due.is_none());
}

#[test]
fn edit_invalid_trailing_token_rejected() {
    let mut env = common::setup();
    let a = add::Args {
        slug: Some("bad-token-task".to_string()),
        ..add_args("Bad token task")
    };
    add::run(a, &mut env.ctx).unwrap();

    let err = edit::run(
        edit::Args {
            tag_tokens: vec!["notavalidtoken".to_string()],
            ..base_edit("bad-token-task")
        },
        &mut env.ctx,
    )
    .unwrap_err();

    assert!(
        err.to_string().contains("unrecognised trailing argument"),
        "unexpected error: {err}"
    );
}

#[test]
fn edit_sets_description() {
    let mut env = common::setup();
    add::run(
        add::Args { slug: Some("desc-task".to_string()), ..add_args("Described task") },
        &mut env.ctx,
    )
    .unwrap();

    edit::run(
        edit::Args {
            description: Some("This is the detail.".to_string()),
            ..base_edit("desc-task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert_eq!(task.description.as_deref(), Some("This is the detail."));
}

#[test]
fn edit_clears_description() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("clear-desc-task".to_string()),
            description: Some("Initial description.".to_string()),
            ..add_args("Clear desc task")
        },
        &mut env.ctx,
    )
    .unwrap();

    edit::run(
        edit::Args { clear_description: true, ..base_edit("clear-desc-task") },
        &mut env.ctx,
    )
    .unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(task.description.is_none());
}

#[test]
fn edit_sets_url() {
    let mut env = common::setup();
    add::run(
        add::Args { slug: Some("url-task".to_string()), ..add_args("URL task") },
        &mut env.ctx,
    )
    .unwrap();

    edit::run(
        edit::Args {
            url: Some("https://example.com/docs".to_string()),
            ..base_edit("url-task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert_eq!(task.url.as_deref(), Some("https://example.com/docs"));
}

#[test]
fn edit_clears_url() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("clear-url-task".to_string()),
            url: Some("https://example.com".to_string()),
            ..add_args("Clear URL task")
        },
        &mut env.ctx,
    )
    .unwrap();

    edit::run(
        edit::Args { clear_url: true, ..base_edit("clear-url-task") },
        &mut env.ctx,
    )
    .unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(task.url.is_none());
}

#[test]
fn edit_rejects_invalid_url() {
    let mut env = common::setup();
    add::run(
        add::Args { slug: Some("bad-url-task".to_string()), ..add_args("Bad URL task") },
        &mut env.ctx,
    )
    .unwrap();

    let err = edit::run(
        edit::Args {
            url: Some("not-a-url".to_string()),
            ..base_edit("bad-url-task")
        },
        &mut env.ctx,
    )
    .unwrap_err();

    assert!(
        err.to_string().contains("http://") || err.to_string().contains("https://"),
        "unexpected error: {err}"
    );
    // URL on the task must remain unset.
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(task.url.is_none());
}

#[test]
fn edit_tag_deduplicates() {
    let mut env = common::setup();
    let a = add::Args {
        slug: Some("dedup-task".to_string()),
        tags: vec!["@work".to_string()],
        ..add_args("Dedup task")
    };
    add::run(a, &mut env.ctx).unwrap();

    // Add @work again — should remain a single entry.
    edit::run(
        edit::Args {
            tags: vec!["@work".to_string()],
            ..base_edit("dedup-task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    let work_count = task.tags.iter().filter(|t| t.as_str() == "@work").count();
    assert_eq!(work_count, 1);
}
