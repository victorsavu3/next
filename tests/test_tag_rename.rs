//! Integration tests for `next tag rename`.

mod common;

use chrono::NaiveDate;
use next::core::archiver::run_archive_pass;
use next::core::domain::task::Task;
use next::core::store::{Store as _, TaskQuery};
use next::core::tag_rename::rename_tag;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

/// Saves a task and commits it, mimicking a normal mutation.
fn add_committed(env: &mut common::TestEnv, task: &Task) {
    env.ctx
        .repo
        .transaction(|store, vcs, root| {
            store.save_task(task)?;
            vcs.commit(
                &[next::core::storage::task_path(root, task)],
                &format!("next: add {:?}", task.title),
            )?;
            Ok(())
        })
        .unwrap();
}

fn tagged(title: &str, tags: &[&str]) -> Task {
    let mut t = Task::new(title);
    t.tags = tags.iter().map(|s| s.to_string()).collect();
    t
}

/// Tags of the task with `id`, whichever tier it lives in.
fn tags_of(env: &common::TestEnv, id: uuid::Uuid) -> Vec<String> {
    env.ctx.repo.store().get_task(id).unwrap().tags
}

/// Tracked-file status: everything the rename touched must be committed.
/// Untracked files are ignored — a bare test repo has no `.gitignore` for
/// the cache and lock files a real `next init` would exclude.
fn git_status(root: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn renames_active_tasks_descendants_and_metadata() {
    let mut env = common::setup();

    let parent = tagged("Parent tag", &["@ai", "python"]);
    let child = tagged("Child tag", &["@ai/task-manager"]);
    let grandchild = tagged("Grandchild tag", &["@ai/task-manager/mcp"]);
    let unrelated = tagged("Unrelated", &["@aim", "@ai-other"]);
    for t in [&parent, &child, &grandchild, &unrelated] {
        add_committed(&mut env, t);
    }

    env.ctx
        .repo
        .transaction(|store, vcs, root| {
            store.set_tag_description("@ai/task-manager", "The task manager")?;
            vcs.commit(
                &[next::core::storage::tag_meta_path(root, "@ai/task-manager")],
                "next: tag describe",
            )?;
            Ok(())
        })
        .unwrap();

    let outcome = rename_tag(&mut env.ctx.repo, "@ai/task-manager", "@ai/next", false).unwrap();
    assert_eq!(outcome.active_tasks, 2, "the tag itself and its descendant");
    assert_eq!(outcome.archived_tasks, 0);
    assert_eq!(
        outcome.metas_moved,
        vec![("@ai/task-manager".to_string(), "@ai/next".to_string())]
    );

    assert_eq!(tags_of(&env, child.id), vec!["@ai/next"]);
    assert_eq!(tags_of(&env, grandchild.id), vec!["@ai/next/mcp"]);
    assert_eq!(
        tags_of(&env, parent.id),
        vec!["@ai", "python"],
        "ancestor untouched"
    );
    assert_eq!(
        tags_of(&env, unrelated.id),
        vec!["@aim", "@ai-other"],
        "only whole segments match"
    );

    // Metadata moved, with the description preserved.
    let store = env.ctx.repo.store();
    assert_eq!(
        store.get_tag_description("@ai/next").unwrap().as_deref(),
        Some("The task manager")
    );
    assert!(store.get_tag_meta("@ai/task-manager").unwrap().is_none());
    let root = env.ctx.repo.repo_root.clone();
    assert!(!root.join("tags/__context__ai/task-manager.toml").exists());
    assert!(root.join("tags/__context__ai/next.toml").exists());

    // Queries follow the new name, and the rename committed everything.
    let by_new = store
        .query_tasks(&TaskQuery {
            required_tags: vec!["@ai/next".into()],
            ..TaskQuery::unpaginated()
        })
        .unwrap();
    assert_eq!(by_new.total, 2, "parent filter matches the descendant too");
    assert_eq!(git_status(&root), "", "rename leaves a clean tree");
}

#[test]
fn renames_archived_tasks_in_warm_segments() {
    let mut env = common::setup();
    let today = d(2026, 7, 15);
    let root = env.ctx.repo.repo_root.clone();

    let mut archived = tagged("Long done", &["@ai/task-manager", "python"]);
    archived.slug = Some("long-done".into());
    archived.mark_done(d(2025, 11, 20));
    let active = tagged("Still open", &["@ai/task-manager"]);
    add_committed(&mut env, &archived);
    add_committed(&mut env, &active);

    let pass = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(pass.archived, 1);

    let outcome = rename_tag(&mut env.ctx.repo, "@ai/task-manager", "@ai/next", false).unwrap();
    assert_eq!(outcome.active_tasks, 1);
    assert_eq!(outcome.archived_tasks, 1);
    assert_eq!(outcome.segments, vec!["archive/2025/11-001.toml"]);
    assert!(outcome.restored_segments.is_empty());

    // The segment file on disk carries the new tag…
    let segment = std::fs::read_to_string(root.join("archive/2025/11-001.toml")).unwrap();
    assert!(
        segment.contains("@ai/next"),
        "segment not rewritten: {segment}"
    );
    assert!(!segment.contains("task-manager"));

    // …and so does the cache, for both direct lookups and archived queries.
    assert_eq!(tags_of(&env, archived.id), vec!["@ai/next", "python"]);
    let hits = env
        .ctx
        .repo
        .store()
        .query_tasks(&TaskQuery {
            archived: true,
            required_tags: vec!["@ai/next".into()],
            ..TaskQuery::unpaginated()
        })
        .unwrap();
    assert_eq!(hits.total, 1);
    let stale = env
        .ctx
        .repo
        .store()
        .query_tasks(&TaskQuery {
            archived: true,
            required_tags: vec!["@ai/task-manager".into()],
            ..TaskQuery::unpaginated()
        })
        .unwrap();
    assert_eq!(stale.total, 0, "no archived task keeps the old tag");
    assert_eq!(git_status(&root), "");

    // A rebuilt cache (fresh clone) agrees with what was written.
    let (store2, _vcs2) = next::core::storage::open(root.clone()).unwrap();
    assert_eq!(
        store2.get_task(archived.id).unwrap().tags,
        vec!["@ai/next", "python"]
    );
}

#[test]
fn renames_archived_tasks_in_pruned_cold_segments() {
    let mut env = common::setup();
    let today = d(2026, 7, 15);
    let root = env.ctx.repo.repo_root.clone();

    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/archive.toml"),
        "archive_after_days = 180\nprune_after_days = 400\n",
    )
    .unwrap();

    let mut ancient = tagged("Ancient", &["@ai/task-manager"]);
    ancient.mark_done(d(2025, 3, 1));
    add_committed(&mut env, &ancient);

    let pass = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(pass.pruned, vec!["archive/2025/03-001.toml"]);
    assert!(!root.join("archive/2025/03-001.toml").exists());

    let outcome = rename_tag(&mut env.ctx.repo, "@ai/task-manager", "@ai/next", false).unwrap();
    assert_eq!(outcome.archived_tasks, 1);
    assert_eq!(
        outcome.restored_segments,
        vec!["archive/2025/03-001.toml"],
        "a cold segment can only be rewritten by restoring it"
    );

    let segment = std::fs::read_to_string(root.join("archive/2025/03-001.toml")).unwrap();
    assert!(segment.contains("@ai/next"));
    assert_eq!(tags_of(&env, ancient.id), vec!["@ai/next"]);
    assert_eq!(git_status(&root), "");

    // The restored segment re-prunes on the next pass, as after a
    // resurrection — the rename does not permanently un-prune it.
    let outcome2 = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(outcome2.pruned, vec!["archive/2025/03-001.toml"]);
}

#[test]
fn updates_machine_local_state() {
    use next::core::domain::state::TagState;

    let mut env = common::setup();
    add_committed(&mut env, &tagged("Tagged", &["@ai/task-manager"]));

    env.ctx
        .repo
        .state_transaction(|store| {
            let mut state = store.get_state()?;
            state.set_state("@ai/task-manager", Some(TagState::Included));
            state.set_state("@ai/task-manager/mcp", Some(TagState::Excluded));
            state.set_state("@work", Some(TagState::Excluded));
            store.save_state(&state)?;
            Ok(())
        })
        .unwrap();

    let outcome = rename_tag(&mut env.ctx.repo, "@ai/task-manager", "@ai/next", false).unwrap();
    assert_eq!(outcome.state_fields, vec!["tags"]);

    let state = env.ctx.repo.store().get_state().unwrap();
    assert_eq!(
        state.state_of("@ai/next"),
        Some(TagState::Included),
        "the included tag is not orphaned"
    );
    assert_eq!(state.state_of("@ai/next/mcp"), Some(TagState::Excluded));
    assert_eq!(
        state.state_of("@work"),
        Some(TagState::Excluded),
        "an unrelated entry is untouched"
    );
    assert!(!state.tags.contains_key("@ai/task-manager"));
}

#[test]
fn renames_state_entries_for_every_tag_kind() {
    use next::core::domain::state::TagState;

    // Unification means the state map is keyed by the full tag, sigil
    // included, so a resource renames exactly like a context — there is no
    // bare-name special case left to get wrong.
    let mut env = common::setup();
    add_committed(&mut env, &tagged("Print", &["#office/printer"]));

    env.ctx
        .repo
        .state_transaction(|store| {
            let mut state = store.get_state()?;
            state.set_state("#office/printer", Some(TagState::Excluded));
            state.set_state("#garage", Some(TagState::Excluded));
            store.save_state(&state)?;
            Ok(())
        })
        .unwrap();

    let outcome = rename_tag(&mut env.ctx.repo, "#office", "#hq", false).unwrap();
    assert_eq!(outcome.state_fields, vec!["tags"]);

    let state = env.ctx.repo.store().get_state().unwrap();
    assert_eq!(state.state_of("#hq/printer"), Some(TagState::Excluded));
    assert_eq!(
        state.state_of("#garage"),
        Some(TagState::Excluded),
        "unrelated key kept"
    );
    assert!(!state.tags.contains_key("#office/printer"));
}

#[test]
fn rejects_existing_destination_without_merge() {
    let mut env = common::setup();
    add_committed(&mut env, &tagged("Old", &["@old"]));
    add_committed(&mut env, &tagged("New", &["@new"]));

    let err = rename_tag(&mut env.ctx.repo, "@old", "@new", false).unwrap_err();
    assert!(
        err.to_string().contains("already exists"),
        "unexpected error: {err}"
    );
    assert!(
        err.to_string().contains("--merge"),
        "error should point at the way out"
    );
    assert_eq!(
        git_status(&env.ctx.repo.repo_root),
        "",
        "a refused rename changes nothing"
    );
}

#[test]
fn merge_folds_tag_into_existing_destination() {
    let mut env = common::setup();
    let both = tagged("Carries both", &["@new", "@old", "python"]);
    let only_old = tagged("Carries old", &["@old"]);
    add_committed(&mut env, &both);
    add_committed(&mut env, &only_old);

    env.ctx
        .repo
        .transaction(|store, vcs, root| {
            store.set_tag_description("@old", "the old one")?;
            store.set_tag_description("@new", "the new one")?;
            vcs.commit(
                &[
                    next::core::storage::tag_meta_path(root, "@old"),
                    next::core::storage::tag_meta_path(root, "@new"),
                ],
                "next: tag describe",
            )?;
            Ok(())
        })
        .unwrap();

    let outcome = rename_tag(&mut env.ctx.repo, "@old", "@new", true).unwrap();
    assert_eq!(outcome.active_tasks, 2);
    assert_eq!(outcome.metas_dropped, vec!["@old".to_string()]);
    assert!(outcome.metas_moved.is_empty());

    assert_eq!(
        tags_of(&env, both.id),
        vec!["@new", "python"],
        "no duplicate tag"
    );
    assert_eq!(tags_of(&env, only_old.id), vec!["@new"]);
    let store = env.ctx.repo.store();
    assert_eq!(
        store.get_tag_description("@new").unwrap().as_deref(),
        Some("the new one"),
        "the destination's own metadata survives"
    );
    assert!(store.get_tag_meta("@old").unwrap().is_none());
    assert_eq!(git_status(&env.ctx.repo.repo_root), "");
}

#[test]
fn rejects_kind_changes_and_self_renames() {
    let mut env = common::setup();
    add_committed(&mut env, &tagged("Tagged", &["@work"]));

    for (old, new, expected) in [
        ("@work", "#work", "leading character"),
        ("@work", "work", "leading character"),
        ("@work", "@work", "already named"),
        ("@work", "@work/sub", "own subtree"),
    ] {
        let err = rename_tag(&mut env.ctx.repo, old, new, false).unwrap_err();
        assert!(
            err.to_string().contains(expected),
            "renaming {old} to {new}: unexpected error: {err}"
        );
    }
    assert_eq!(
        tags_of(&env, env.ctx.repo.store().list_tasks().unwrap()[0].id),
        vec!["@work"]
    );
}

#[test]
fn cli_subcommand_renames() {
    use next::cli::commands::tag::{self, RenameArgs, TagSubcommand};

    let mut env = common::setup();
    let task = tagged("Tagged", &["@ai/task-manager"]);
    add_committed(&mut env, &task);

    tag::run(
        tag::Args {
            subcommand: Some(TagSubcommand::Rename(RenameArgs {
                old: "@ai/task-manager".into(),
                new: "@ai/next".into(),
                merge: false,
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    assert_eq!(tags_of(&env, task.id), vec!["@ai/next"]);
}

#[test]
fn unknown_tag_is_a_no_op() {
    let mut env = common::setup();
    add_committed(&mut env, &tagged("Tagged", &["@work"]));

    let outcome = rename_tag(&mut env.ctx.repo, "@nothing", "@something", false).unwrap();
    assert!(outcome.is_empty());
    assert_eq!(git_status(&env.ctx.repo.repo_root), "", "no empty commit");
}
