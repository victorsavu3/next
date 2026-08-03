mod common;

use next::cli::commands::{add, archive, maintenance};

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

#[test]
fn rebuild_cache_preserves_active_tasks() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("a".into()),
            ..add_args("Task A")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            slug: Some("b".into()),
            ..add_args("Task B")
        },
        &mut env.ctx,
    )
    .unwrap();

    assert_eq!(env.ctx.repo.store.list_tasks().unwrap().len(), 2);

    maintenance::run(
        maintenance::Args {
            command: maintenance::Command::RebuildCache(maintenance::RebuildCacheArgs {
                json: false,
            }),
        },
        &mut env.ctx,
    )
    .unwrap();

    // Every active task survives the rebuild and is still resolvable by slug.
    assert_eq!(
        env.ctx.repo.store.list_tasks().unwrap().len(),
        2,
        "rebuild must repopulate all active tasks from the TOML source"
    );
    assert!(env.ctx.repo.store.get_task_by_slug("a").unwrap().is_some());
    assert!(env.ctx.repo.store.get_task_by_slug("b").unwrap().is_some());
}

#[test]
fn maintenance_archive_dispatches_and_leaves_fresh_tasks() {
    let mut env = common::setup();
    add::run(add_args("Fresh task"), &mut env.ctx).unwrap();

    // Nothing is old enough to archive, but the sub-command must dispatch to
    // the archive pass and succeed rather than being an unknown command.
    maintenance::run(
        maintenance::Args {
            command: maintenance::Command::Archive(archive::Args { json: false }),
        },
        &mut env.ctx,
    )
    .unwrap();

    assert_eq!(
        env.ctx.repo.store.list_tasks().unwrap().len(),
        1,
        "a fresh open task must not be archived"
    );
}
