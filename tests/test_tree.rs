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
    }
}

fn capture_tree_closed(env: &common::TestEnv) -> String {
    let mut buf: Vec<u8> = Vec::new();
    tree::run_with_writer(
        tree::Args {
            all: false,
            closed: true,
            json: false,
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
        },
        &env.ctx,
    )
    .unwrap();
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
            subcommand: Some(tag::TagSubcommand::Include(tag::TagStateArgs {
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
