mod common;

use next::cli::commands::{add, edit};
use next::core::domain::task::{Recurrence, Snap, SnapLeeway};

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
        clear_recur_snap: false,
        recur_snap_leeway: None,
        clear_recur_snap_leeway: false,
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
fn edit_typoed_flag_in_trailing_tokens_rejected() {
    // `--clear-du` (a typo of `--clear-due`) is swallowed into the trailing
    // var-arg by clap; it must error loudly instead of silently no-oping.
    let mut env = common::setup();
    let a = add::Args {
        slug: Some("typo-flag-task".to_string()),
        due: Some("2026-12-31".to_string()),
        ..add_args("Typo flag task")
    };
    add::run(a, &mut env.ctx).unwrap();

    let err = edit::run(
        edit::Args {
            tag_tokens: vec!["--clear-du".to_string()],
            ..base_edit("typo-flag-task")
        },
        &mut env.ctx,
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("unrecognised flag"), "unexpected error: {msg}");
    assert!(
        msg.contains("--clear-du"),
        "error must name the token: {msg}"
    );
    assert!(msg.contains("--help"), "error must point at --help: {msg}");

    // The task is untouched.
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(task.due.is_some(), "due date must survive the failed edit");
}

#[test]
fn edit_invalid_trailing_remove_token_rejected() {
    // Removal tokens go through the same tag validation as add tokens, so a
    // malformed `-tag` errors instead of no-oping.
    let mut env = common::setup();
    let a = add::Args {
        slug: Some("bad-remove-task".to_string()),
        tags: vec!["@work".to_string()],
        ..add_args("Bad remove task")
    };
    add::run(a, &mut env.ctx).unwrap();

    let err = edit::run(
        edit::Args {
            tag_tokens: vec!["-bad!tag".to_string()],
            ..base_edit("bad-remove-task")
        },
        &mut env.ctx,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("bad!tag"),
        "unexpected error: {err}"
    );

    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    assert!(task.tags.contains(&"@work".to_string()));
}

#[test]
fn edit_invalid_remove_tag_flag_rejected() {
    // --remove-tag values are validated too: an invalid tag can never be on a
    // task, so removing one would otherwise silently do nothing.
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("bad-flag-remove".to_string()),
            ..add_args("Bad flag remove")
        },
        &mut env.ctx,
    )
    .unwrap();

    let err = edit::run(
        edit::Args {
            remove_tags: vec!["bad!tag".to_string()],
            ..base_edit("bad-flag-remove")
        },
        &mut env.ctx,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("bad!tag"),
        "unexpected error: {err}"
    );
}

#[test]
fn edit_sets_description() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("desc-task".to_string()),
            ..add_args("Described task")
        },
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
        edit::Args {
            clear_description: true,
            ..base_edit("clear-desc-task")
        },
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
        add::Args {
            slug: Some("url-task".to_string()),
            ..add_args("URL task")
        },
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
        edit::Args {
            clear_url: true,
            ..base_edit("clear-url-task")
        },
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
        add::Args {
            slug: Some("bad-url-task".to_string()),
            ..add_args("Bad URL task")
        },
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

#[test]
fn title_edit_commits_the_file_rename() {
    // Editing the title of a slug-less task renames its file; the old path
    // must be committed as a deletion, or it stays tracked in git and
    // resurfaces as a duplicate task on other machines after their next pull.
    let mut env = common::setup();
    add::run(add_args("Original name"), &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    let root = env.ctx.repo.repo_root.clone();
    let old_file = next::core::storage::task_path(&root, &task);
    assert!(old_file.exists());

    edit::run(
        edit::Args {
            title: Some("Renamed completely".to_string()),
            ..base_edit(&task.id.to_string())
        },
        &mut env.ctx,
    )
    .unwrap();

    assert!(
        !old_file.exists(),
        "old file must be gone from the working tree"
    );
    // No tracked-file changes may remain: the rename (delete + add) was
    // committed atomically with the edit.
    let mut cmd = std::process::Command::new("git");
    cmd.args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(&root);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
    ] {
        cmd.env_remove(var);
    }
    let out = cmd.output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "",
        "the rename must leave no uncommitted tracked changes"
    );
}

// ── recurrence snap edits ───────────────────────────────────────────────────

/// Adds a task with a completion rule and a `dom:1` snap, returning its slug.
fn add_snapped_task(env: &mut common::TestEnv, slug: &str) -> String {
    add::run(
        add::Args {
            slug: Some(slug.to_owned()),
            recur_completion: Some(7),
            recur_snap: Some("dom:1".to_owned()),
            ..add_args("Pay the rent")
        },
        &mut env.ctx,
    )
    .unwrap();
    slug.to_owned()
}

fn recurrence_of(env: &mut common::TestEnv, slug: &str) -> Option<Recurrence> {
    env.ctx
        .repo
        .store
        .get_task_by_slug(slug)
        .unwrap()
        .unwrap()
        .recurrence
}

#[test]
fn edit_recur_completion_keeps_existing_snap() {
    // Changing only the interval must not wipe the snap the task already had.
    let mut env = common::setup();
    let slug = add_snapped_task(&mut env, "rent");

    edit::run(
        edit::Args {
            recur_completion: Some(31),
            ..base_edit(&slug)
        },
        &mut env.ctx,
    )
    .unwrap();

    match recurrence_of(&mut env, &slug) {
        Some(Recurrence::Completion {
            interval_days,
            snap,
            ..
        }) => {
            assert_eq!(interval_days, 31, "the interval must be updated");
            assert_eq!(
                snap,
                Some(Snap::DayOfMonth { day: 1 }),
                "the snap must survive a bare --recur-completion"
            );
        }
        other => panic!("expected a completion rule, got {other:?}"),
    }
}

#[test]
fn edit_recur_schedule_keeps_existing_snap() {
    // The same carry-forward applies when switching to a schedule rule.
    let mut env = common::setup();
    let slug = add_snapped_task(&mut env, "rent-schedule");

    edit::run(
        edit::Args {
            recur_schedule: Some("FREQ=MONTHLY;BYMONTHDAY=28".to_owned()),
            ..base_edit(&slug)
        },
        &mut env.ctx,
    )
    .unwrap();

    match recurrence_of(&mut env, &slug) {
        Some(Recurrence::Schedule { rrule, snap, .. }) => {
            assert_eq!(rrule, "FREQ=MONTHLY;BYMONTHDAY=28");
            assert_eq!(snap, Some(Snap::DayOfMonth { day: 1 }));
        }
        other => panic!("expected a schedule rule, got {other:?}"),
    }
}

#[test]
fn edit_recur_snap_still_wins_over_the_carried_one() {
    let mut env = common::setup();
    let slug = add_snapped_task(&mut env, "rent-replace");

    edit::run(
        edit::Args {
            recur_completion: Some(14),
            recur_snap: Some("monday".to_owned()),
            ..base_edit(&slug)
        },
        &mut env.ctx,
    )
    .unwrap();

    match recurrence_of(&mut env, &slug) {
        Some(Recurrence::Completion {
            interval_days,
            snap,
            ..
        }) => {
            assert_eq!(interval_days, 14);
            assert_eq!(snap, Some(Snap::NextWeekday { weekday: 0 }));
        }
        other => panic!("expected a completion rule, got {other:?}"),
    }
}

#[test]
fn edit_clear_recur_snap_standalone_keeps_the_rule() {
    let mut env = common::setup();
    let slug = add_snapped_task(&mut env, "rent-clear");

    edit::run(
        edit::Args {
            clear_recur_snap: true,
            ..base_edit(&slug)
        },
        &mut env.ctx,
    )
    .unwrap();

    match recurrence_of(&mut env, &slug) {
        Some(Recurrence::Completion {
            interval_days,
            snap,
            ..
        }) => {
            assert_eq!(interval_days, 7, "the rule itself must be untouched");
            assert_eq!(snap, None, "--clear-recur-snap must drop the snap");
        }
        other => panic!("expected a completion rule, got {other:?}"),
    }
}

#[test]
fn edit_clear_recur_snap_alongside_a_rule_change() {
    let mut env = common::setup();
    let slug = add_snapped_task(&mut env, "rent-both");

    edit::run(
        edit::Args {
            recur_completion: Some(31),
            clear_recur_snap: true,
            ..base_edit(&slug)
        },
        &mut env.ctx,
    )
    .unwrap();

    match recurrence_of(&mut env, &slug) {
        Some(Recurrence::Completion {
            interval_days,
            snap,
            ..
        }) => {
            assert_eq!(interval_days, 31);
            assert_eq!(snap, None);
        }
        other => panic!("expected a completion rule, got {other:?}"),
    }
}

#[test]
fn edit_recur_snap_and_clear_recur_snap_conflict() {
    let mut env = common::setup();
    let slug = add_snapped_task(&mut env, "rent-conflict");

    let err = edit::run(
        edit::Args {
            recur_snap: Some("monday".to_owned()),
            clear_recur_snap: true,
            ..base_edit(&slug)
        },
        &mut env.ctx,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("--recur-snap and --clear-recur-snap are mutually exclusive"),
        "unexpected error: {err}"
    );
}

#[test]
fn edit_clear_recur_snap_without_recurrence_errors() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("plain".to_owned()),
            ..add_args("Not recurring")
        },
        &mut env.ctx,
    )
    .unwrap();

    let err = edit::run(
        edit::Args {
            clear_recur_snap: true,
            ..base_edit("plain")
        },
        &mut env.ctx,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("--clear-recur-snap requires an existing recurrence rule"),
        "unexpected error: {err}"
    );
}

// ── recurrence snap leeway edits ────────────────────────────────────────────

/// Adds `Pay the rent`: 30 days after completion, snapped to the 1st, with
/// three days of tolerance either way — the worked example throughout.
fn add_leeway_task(env: &mut common::TestEnv, slug: &str) -> String {
    add::run(
        add::Args {
            slug: Some(slug.to_owned()),
            recur_completion: Some(30),
            recur_snap: Some("dom:1".to_owned()),
            recur_snap_leeway: Some("3".to_owned()),
            ..add_args("Pay the rent")
        },
        &mut env.ctx,
    )
    .unwrap();
    slug.to_owned()
}

fn leeway_of(env: &mut common::TestEnv, slug: &str) -> Option<SnapLeeway> {
    recurrence_of(env, slug)
        .as_ref()
        .and_then(Recurrence::snap_leeway)
        .cloned()
}

/// T11, extended: the interval change must carry the snap **and** the leeway.
#[test]
fn edit_recur_completion_keeps_existing_snap_and_leeway() {
    let mut env = common::setup();
    let slug = add_leeway_task(&mut env, "rent");

    edit::run(
        edit::Args {
            recur_completion: Some(31),
            ..base_edit(&slug)
        },
        &mut env.ctx,
    )
    .unwrap();

    match recurrence_of(&mut env, &slug) {
        Some(Recurrence::Completion {
            interval_days,
            snap,
            snap_leeway,
        }) => {
            assert_eq!(interval_days, 31);
            assert_eq!(snap, Some(Snap::DayOfMonth { day: 1 }));
            assert_eq!(
                snap_leeway,
                Some(SnapLeeway {
                    back: 3,
                    forward: Some(3)
                }),
                "the leeway must survive a bare --recur-completion"
            );
        }
        other => panic!("expected a completion rule, got {other:?}"),
    }
}

#[test]
fn edit_recur_snap_leeway_standalone_replaces_it() {
    let mut env = common::setup();
    let slug = add_leeway_task(&mut env, "rent-replace");

    edit::run(
        edit::Args {
            recur_snap_leeway: Some("5,0".to_owned()),
            ..base_edit(&slug)
        },
        &mut env.ctx,
    )
    .unwrap();

    assert_eq!(
        leeway_of(&mut env, &slug),
        Some(SnapLeeway {
            back: 5,
            forward: Some(0)
        })
    );
    assert_eq!(
        recurrence_of(&mut env, &slug)
            .as_ref()
            .and_then(Recurrence::snap),
        Some(&Snap::DayOfMonth { day: 1 }),
        "a leeway-only edit must leave the snap alone"
    );
}

#[test]
fn edit_clear_recur_snap_leeway_keeps_the_snap() {
    let mut env = common::setup();
    let slug = add_leeway_task(&mut env, "rent-clear-leeway");

    edit::run(
        edit::Args {
            clear_recur_snap_leeway: true,
            ..base_edit(&slug)
        },
        &mut env.ctx,
    )
    .unwrap();

    assert_eq!(leeway_of(&mut env, &slug), None);
    assert_eq!(
        recurrence_of(&mut env, &slug)
            .as_ref()
            .and_then(Recurrence::snap),
        Some(&Snap::DayOfMonth { day: 1 })
    );
}

#[test]
fn edit_clear_recur_snap_takes_the_leeway_with_it() {
    // A leeway with no snap is invalid (V1), so dropping the snap must drop
    // the tolerance that only described it.
    let mut env = common::setup();
    let slug = add_leeway_task(&mut env, "rent-clear-snap");

    edit::run(
        edit::Args {
            clear_recur_snap: true,
            ..base_edit(&slug)
        },
        &mut env.ctx,
    )
    .unwrap();

    assert_eq!(leeway_of(&mut env, &slug), None);
    assert_eq!(
        recurrence_of(&mut env, &slug)
            .as_ref()
            .and_then(Recurrence::snap),
        None
    );
}

/// T13, CLI half: the carry-forward path reaches the same checks. The stored
/// `back = 3` never passes through the leeway parser, so only
/// `validate_recurrence` can catch it against the new interval.
#[test]
fn edit_rejects_an_interval_the_carried_leeway_would_stall() {
    let mut env = common::setup();
    let slug = add_leeway_task(&mut env, "rent-stall");

    let err = edit::run(
        edit::Args {
            recur_completion: Some(3),
            ..base_edit(&slug)
        },
        &mut env.ctx,
    )
    .unwrap_err();

    assert_eq!(
        err.to_string(),
        "backward leeway (3d) must be less than the completion interval (3d), \
         or the series would not advance"
    );
    assert_eq!(
        recurrence_of(&mut env, &slug),
        Some(Recurrence::Completion {
            interval_days: 30,
            snap: Some(Snap::DayOfMonth { day: 1 }),
            snap_leeway: Some(SnapLeeway {
                back: 3,
                forward: Some(3)
            }),
        }),
        "a rejected edit must leave the stored rule untouched"
    );
}

#[test]
fn edit_recur_snap_leeway_and_clear_conflict() {
    let mut env = common::setup();
    let slug = add_leeway_task(&mut env, "rent-conflict");

    let err = edit::run(
        edit::Args {
            recur_snap_leeway: Some("3".to_owned()),
            clear_recur_snap_leeway: true,
            ..base_edit(&slug)
        },
        &mut env.ctx,
    )
    .unwrap_err();

    assert_eq!(
        err.to_string(),
        "--recur-snap-leeway and --clear-recur-snap-leeway are mutually exclusive"
    );
}

#[test]
fn edit_recur_snap_leeway_without_recurrence_errors() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("plain-leeway".to_owned()),
            ..add_args("Not recurring")
        },
        &mut env.ctx,
    )
    .unwrap();

    let err = edit::run(
        edit::Args {
            recur_snap_leeway: Some("3".to_owned()),
            ..base_edit("plain-leeway")
        },
        &mut env.ctx,
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("--recur-snap-leeway requires an existing recurrence rule"),
        "unexpected error: {err}"
    );
}
