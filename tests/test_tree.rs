mod common;

use next::cli::commands::{add, done, tag, tree};

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

fn tree_args(all: bool) -> tree::Args {
    tree::Args {
        all,
        closed: false,
        json: false,
        format: None,
        count: false,
        fields: vec![],
        tokens: vec![],
    }
}

/// A filtered tree keeps a match's ancestors, or matching subtasks would be
/// left dangling with no visible parent — which is not a tree.
#[test]
fn tree_filter_keeps_the_ancestors_of_a_match() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("proj".into()),
            ..add_args("Parent project")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            parent: Some("proj".into()),
            tags: vec!["#rust".into()],
            ..add_args("Tagged subtask")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(add_args("Unrelated task"), &mut env.ctx).unwrap();

    let mut buf: Vec<u8> = Vec::new();
    tree::run_with_writer(
        tree::Args {
            tokens: vec!["+#rust".into()],
            ..tree_args(true)
        },
        &env.ctx,
        &mut buf,
    )
    .unwrap();
    let out = String::from_utf8(buf).unwrap();

    assert!(out.contains("Tagged subtask"), "the match itself: {out}");
    assert!(
        out.contains("Parent project"),
        "its parent comes along so the match has somewhere to hang: {out}"
    );
    assert!(
        !out.contains("Unrelated task"),
        "everything else is filtered out: {out}"
    );
}

#[test]
fn tree_count_prints_only_a_number() {
    let mut env = common::setup();
    add::run(add_args("One"), &mut env.ctx).unwrap();
    add::run(add_args("Two"), &mut env.ctx).unwrap();

    let mut buf: Vec<u8> = Vec::new();
    tree::run_with_writer(
        tree::Args {
            count: true,
            ..tree_args(false)
        },
        &env.ctx,
        &mut buf,
    )
    .unwrap();
    assert_eq!(String::from_utf8(buf).unwrap().trim(), "2");
}

/// Runs `tree` with the given args and returns the trimmed output.
fn capture_tree_args(env: &common::TestEnv, args: tree::Args) -> String {
    let mut buf: Vec<u8> = Vec::new();
    tree::run_with_writer(args, &env.ctx, &mut buf).unwrap();
    String::from_utf8(buf).unwrap().trim().to_owned()
}

/// A parent with one tagged subtask and one unrelated task alongside.
fn count_env() -> common::TestEnv {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("proj".into()),
            ..add_args("Parent project")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            parent: Some("proj".into()),
            tags: vec!["#rust".into()],
            ..add_args("Tagged subtask")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(add_args("Unrelated task"), &mut env.ctx).unwrap();
    env
}

/// `--count` asks a question no output format has an opinion about, so it is
/// answered before the format is chosen — `--json --count` is still a number.
#[test]
fn tree_count_wins_over_the_json_format() {
    let env = count_env();

    for args in [
        tree::Args {
            count: true,
            json: true,
            ..tree_args(false)
        },
        tree::Args {
            count: true,
            format: Some(next::cli::commands::OutputFormat::Json),
            ..tree_args(false)
        },
    ] {
        assert_eq!(capture_tree_args(&env, args), "3");
    }
}

/// The ancestors a filtered tree keeps for shape did not match the query, so
/// they are not counted — otherwise `tree --count` and `list --count` would
/// disagree about the same query.
#[test]
fn tree_count_excludes_the_ancestors_kept_for_shape() {
    let env = count_env();

    // The parent is printed to keep the subtree connected...
    let rendered = capture_tree_args(
        &env,
        tree::Args {
            tokens: vec!["+#rust".into()],
            ..tree_args(false)
        },
    );
    assert!(rendered.contains("Parent project"), "{rendered}");
    assert!(rendered.contains("Tagged subtask"), "{rendered}");

    // ...but only the subtask matched.
    for args in [
        tree::Args {
            count: true,
            tokens: vec!["+#rust".into()],
            ..tree_args(false)
        },
        tree::Args {
            count: true,
            json: true,
            tokens: vec!["+#rust".into()],
            ..tree_args(false)
        },
    ] {
        assert_eq!(capture_tree_args(&env, args), "1");
    }
}

/// With no query there is nothing to distinguish a match from a bystander, so
/// the count is simply the visible set — including tasks `--all` reveals.
#[test]
fn tree_count_without_a_query_is_the_visible_set() {
    let mut env = count_env();
    done::run(
        done::Args {
            id: "proj".into(),
            completed_at: None,
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    assert_eq!(
        capture_tree_args(
            &env,
            tree::Args {
                count: true,
                ..tree_args(false)
            }
        ),
        "2",
        "the closed parent drops out of the active set"
    );
    assert_eq!(
        capture_tree_args(
            &env,
            tree::Args {
                count: true,
                ..tree_args(true)
            }
        ),
        "3",
        "--all counts everything it would print"
    );
}

fn capture_tree_closed(env: &common::TestEnv) -> String {
    let mut buf: Vec<u8> = Vec::new();
    tree::run_with_writer(
        tree::Args {
            all: false,
            closed: true,
            json: false,
            format: None,
            count: false,
            fields: vec![],
            tokens: vec![],
        },
        &env.ctx,
        &mut buf,
    )
    .unwrap();
    String::from_utf8(buf).unwrap()
}

/// Run the tree command and capture its output as a String.
fn capture_tree(env: &common::TestEnv, all: bool) -> String {
    let mut buf: Vec<u8> = Vec::new();
    tree::run_with_writer(tree_args(all), &env.ctx, &mut buf).unwrap();
    String::from_utf8(buf).unwrap()
}

// ---------------------------------------------------------------------------
// Basic tree rendering
// ---------------------------------------------------------------------------

#[test]
fn tree_empty_when_no_tasks() {
    let env = common::setup();
    tree::run(tree_args(false), &env.ctx).unwrap();
}

#[test]
fn tree_shows_top_level_tasks() {
    let mut env = common::setup();
    add::run(add_args("Task A"), &mut env.ctx).unwrap();
    add::run(add_args("Task B"), &mut env.ctx).unwrap();
    // Must not error; two tasks, no parent.
    tree::run(tree_args(false), &env.ctx).unwrap();
}

#[test]
fn tree_shows_children_under_parent() {
    let mut env = common::setup();

    add::run(
        add::Args {
            slug: Some("parent-task".into()),
            ..add_args("Parent")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            parent: Some("parent-task".into()),
            ..add_args("Child")
        },
        &mut env.ctx,
    )
    .unwrap();

    // No error; child is nested under parent.
    tree::run(tree_args(false), &env.ctx).unwrap();

    // Verify structure: parent has one child.
    let all = env.ctx.repo.store.list_tasks().unwrap();
    let parent = all.iter().find(|t| t.title == "Parent").unwrap();
    let child = all.iter().find(|t| t.title == "Child").unwrap();
    assert_eq!(child.parent_id, Some(parent.id));
}

#[test]
fn tree_excludes_done_tasks_by_default() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("done-task".into()),
            ..add_args("Done task")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(add_args("Open task"), &mut env.ctx).unwrap();

    done::run(
        done::Args {
            id: "done-task".into(),
            completed_at: None,
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    let all = env.ctx.repo.store.list_tasks().unwrap();
    let open_count = all
        .iter()
        .filter(|t| t.status == next::core::domain::task::Status::Open)
        .count();
    assert_eq!(open_count, 1);
    // Default tree should not error even with mixed statuses.
    tree::run(tree_args(false), &env.ctx).unwrap();
}

#[test]
fn tree_all_includes_done_tasks() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("done-task".into()),
            ..add_args("Done task")
        },
        &mut env.ctx,
    )
    .unwrap();
    done::run(
        done::Args {
            id: "done-task".into(),
            completed_at: None,
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    // --all must not error and includes done tasks.
    tree::run(tree_args(true), &env.ctx).unwrap();

    let all = env.ctx.repo.store.list_tasks().unwrap();
    assert_eq!(all.len(), 1);
}

#[test]
fn tree_json_output() {
    let mut env = common::setup();
    add::run(add_args("Task JSON"), &mut env.ctx).unwrap();
    tree::run(
        tree::Args {
            all: false,
            closed: false,
            json: true,
            format: None,
            count: false,
            fields: vec![],
            tokens: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}

/// `--fields` trims every task in the array. The tree's JSON is the largest
/// payload the tool emits, so projection matters most here.
#[test]
fn tree_json_projects_the_requested_fields() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("proj".into()),
            description: Some("A long description nobody asked for".into()),
            ..add_args("Parent project")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            parent: Some("proj".into()),
            ..add_args("Subtask")
        },
        &mut env.ctx,
    )
    .unwrap();

    let out = capture_tree_args(
        &env,
        tree::Args {
            json: true,
            fields: vec!["id".into(), "title".into()],
            ..tree_args(false)
        },
    );
    let items: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
    assert_eq!(items.len(), 2, "{out}");
    for item in &items {
        let keys: Vec<&str> = item.as_object().unwrap().keys().map(|k| &**k).collect();
        assert_eq!(keys, ["id", "title"], "{item}");
    }
}

/// An unknown field name is refused before the repository is read, not after.
#[test]
fn tree_rejects_an_unknown_field_name() {
    let env = common::setup();
    let err = tree::run_with_writer(
        tree::Args {
            json: true,
            fields: vec!["nosuchfield".into()],
            ..tree_args(false)
        },
        &env.ctx,
        &mut Vec::new(),
    )
    .expect_err("an unknown field must be refused")
    .to_string();
    assert!(err.contains("unknown field"), "{err}");
}

/// The tree is a fixed rendering, so `--fields` has nothing to do there.
/// Silently ignoring a flag the user typed is worse than refusing it.
#[test]
fn tree_fields_without_json_is_a_usage_error() {
    let mut env = common::setup();
    add::run(add_args("A task"), &mut env.ctx).unwrap();

    let err = tree::run_with_writer(
        tree::Args {
            fields: vec!["id".into()],
            ..tree_args(false)
        },
        &env.ctx,
        &mut Vec::new(),
    )
    .expect_err("`tree --fields` without --json must be refused")
    .to_string();
    assert!(err.contains("--json"), "the message names the fix: {err}");
}

// ---------------------------------------------------------------------------
// Context grouping — output assertions via run_with_writer
// ---------------------------------------------------------------------------

/// A task with a single context tag appears under that context's section.
#[test]
fn context_single_tag_appears_in_section() {
    let mut env = common::setup();
    add::run(
        add::Args {
            tags: vec!["@work".into()],
            ..add_args("Work task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let out = capture_tree(&env, false);
    assert!(
        out.contains("── @work ──"),
        "expected @work section, got:\n{out}"
    );
    assert!(
        out.contains("Work task"),
        "expected task title, got:\n{out}"
    );
    assert!(
        !out.contains("No context"),
        "unexpected No context section:\n{out}"
    );
}

/// A task with no context tag appears under "No context".
#[test]
fn no_context_task_appears_in_no_context_section() {
    let mut env = common::setup();
    add::run(add_args("Plain task"), &mut env.ctx).unwrap();

    let out = capture_tree(&env, false);
    assert!(
        out.contains("── No context ──"),
        "expected No context section, got:\n{out}"
    );
    assert!(
        out.contains("Plain task"),
        "expected task title, got:\n{out}"
    );
}

/// A task with a nested context tag (@work/frontend) appears under @work/frontend
/// only, NOT also under @work.
#[test]
fn nested_context_most_specific_wins() {
    let mut env = common::setup();
    add::run(
        add::Args {
            tags: vec!["@work/frontend".into()],
            ..add_args("Frontend task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let out = capture_tree(&env, false);
    assert!(
        out.contains("── @work/frontend ──"),
        "expected @work/frontend section, got:\n{out}"
    );
    // Must NOT have a standalone @work section.
    assert!(
        !out.contains("── @work ──"),
        "unexpected @work section (task should only be in @work/frontend):\n{out}"
    );
    assert!(
        out.contains("Frontend task"),
        "expected task title, got:\n{out}"
    );
}

/// A task with both @work and @work/frontend appears only in @work/frontend.
#[test]
fn deeper_tag_overrides_shallower_tag_on_same_task() {
    let mut env = common::setup();
    add::run(
        add::Args {
            // Both tags present; @work/frontend is deeper.
            tags: vec!["@work".into(), "@work/frontend".into()],
            ..add_args("Mixed depth task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let out = capture_tree(&env, false);
    assert!(
        out.contains("── @work/frontend ──"),
        "expected @work/frontend section, got:\n{out}"
    );
    assert!(
        !out.contains("── @work ──"),
        "standalone @work section must not appear:\n{out}"
    );
}

/// A task with two same-depth context tags (@home and @work) appears in both
/// sections; the second occurrence carries "(also in @home)".
#[test]
fn same_depth_multi_context_appears_in_both_sections() {
    let mut env = common::setup();
    add::run(
        add::Args {
            // Alphabetically: @home < @work, so @home is primary.
            tags: vec!["@work".into(), "@home".into()],
            ..add_args("Shared task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let out = capture_tree(&env, false);

    assert!(
        out.contains("── @home ──"),
        "expected @home section, got:\n{out}"
    );
    assert!(
        out.contains("── @work ──"),
        "expected @work section, got:\n{out}"
    );

    // The task appears in both sections.
    let home_pos = out.find("── @home ──").unwrap();
    let work_pos = out.find("── @work ──").unwrap();

    // In @home it must appear WITHOUT a note (it's the primary).
    let home_section = &out[home_pos..work_pos];
    assert!(
        home_section.contains("Shared task"),
        "task should appear in @home section:\n{home_section}"
    );
    assert!(
        !home_section.contains("(also in"),
        "@home section must not have 'also in' note:\n{home_section}"
    );

    // In @work it must appear WITH "(also in @home)".
    let work_section = &out[work_pos..];
    assert!(
        work_section.contains("Shared task"),
        "task should appear in @work section:\n{work_section}"
    );
    assert!(
        work_section.contains("(also in @home)"),
        "@work section must contain '(also in @home)':\n{work_section}"
    );
}

/// Child tasks follow their parent into the parent's context section and are
/// NOT repeated in secondary sections for multi-context parents.
#[test]
fn children_follow_parent_context_section() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("ctx-parent".into()),
            tags: vec!["@work".into()],
            ..add_args("Context parent")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            parent: Some("ctx-parent".into()),
            ..add_args("Context child")
        },
        &mut env.ctx,
    )
    .unwrap();

    let out = capture_tree(&env, false);

    assert!(
        out.contains("── @work ──"),
        "expected @work section, got:\n{out}"
    );
    let work_pos = out.find("── @work ──").unwrap();
    let work_section = &out[work_pos..];
    assert!(
        work_section.contains("Context parent"),
        "parent should appear in @work section:\n{work_section}"
    );
    assert!(
        work_section.contains("Context child"),
        "child should follow parent in @work section:\n{work_section}"
    );
    // The child should NOT appear under No context since the parent has a tag.
    assert!(
        !out.contains("No context"),
        "No context section should not appear:\n{out}"
    );
}

/// Children are printed only once — under their parent's primary section —
/// and not duplicated in secondary sections of a multi-context parent.
#[test]
fn children_not_repeated_in_secondary_context_section() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("multi-ctx".into()),
            // @home is primary (alphabetically first), @work is secondary.
            tags: vec!["@home".into(), "@work".into()],
            ..add_args("Multi-context parent")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            parent: Some("multi-ctx".into()),
            ..add_args("Child of multi")
        },
        &mut env.ctx,
    )
    .unwrap();

    let out = capture_tree(&env, false);

    // Split at the @work section boundary.
    let home_pos = out.find("── @home ──").unwrap();
    let work_pos = out.find("── @work ──").unwrap();

    let home_section = &out[home_pos..work_pos];
    let work_section = &out[work_pos..];

    // Child must appear in @home (primary).
    assert!(
        home_section.contains("Child of multi"),
        "child should appear in @home section:\n{home_section}"
    );
    // Child must NOT appear in @work (secondary).
    assert!(
        !work_section.contains("Child of multi"),
        "child must not be repeated in @work section:\n{work_section}"
    );
}

/// "No context" section always comes after all named context sections.
#[test]
fn no_context_section_comes_last() {
    let mut env = common::setup();
    add::run(
        add::Args {
            tags: vec!["@work".into()],
            ..add_args("Work task")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(add_args("Plain task"), &mut env.ctx).unwrap();

    let out = capture_tree(&env, false);
    let work_pos = out.find("── @work ──").expect("@work section missing");
    let no_ctx_pos = out
        .find("── No context ──")
        .expect("No context section missing");
    assert!(
        work_pos < no_ctx_pos,
        "@work must appear before No context:\n{out}"
    );
}

/// `tree --closed` must respect the active context: only closed tasks whose
/// context matches the active context are shown.
#[test]
fn tree_closed_respects_context() {
    let mut env = common::setup();

    tag::run(
        tag::Args {
            subcommand: Some(tag::TagSubcommand::Require(tag::TagStateArgs {
                tags: vec!["@work".into()],
            })),
        },
        &mut env.ctx,
    )
    .unwrap();

    add::run(
        add::Args {
            slug: Some("work-done".into()),
            tags: vec!["@work".into()],
            ..add_args("Work done task")
        },
        &mut env.ctx,
    )
    .unwrap();
    done::run(
        done::Args {
            id: "work-done".into(),
            completed_at: None,
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    add::run(
        add::Args {
            slug: Some("home-done".into()),
            tags: vec!["@home".into()],
            ..add_args("Home done task")
        },
        &mut env.ctx,
    )
    .unwrap();
    done::run(
        done::Args {
            id: "home-done".into(),
            completed_at: None,
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    let out = capture_tree_closed(&env);
    assert!(
        out.contains("Work done task"),
        "expected @work task in output:\n{out}"
    );
    assert!(
        !out.contains("Home done task"),
        "unexpected @home task while @work context is active:\n{out}"
    );
}

/// Named context sections appear in alphabetical order.
#[test]
fn named_sections_are_sorted_alphabetically() {
    let mut env = common::setup();
    add::run(
        add::Args {
            tags: vec!["@work".into()],
            ..add_args("Work task")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            tags: vec!["@home".into()],
            ..add_args("Home task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let out = capture_tree(&env, false);
    let home_pos = out.find("── @home ──").expect("@home section missing");
    let work_pos = out.find("── @work ──").expect("@work section missing");
    assert!(
        home_pos < work_pos,
        "@home must appear before @work:\n{out}"
    );
}
