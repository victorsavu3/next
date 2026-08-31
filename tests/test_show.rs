mod common;

use chrono::NaiveDate;
use next::cli::commands::{add, show};

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

#[test]
fn show_by_id() {
    let mut env = common::setup();
    add::run(add_args("My task"), &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    show::run(
        show::Args {
            id: task.id.to_string(),
            json: false,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}

#[test]
fn show_by_slug() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("my-slug".into()),
            ..add_args("Slugged task")
        },
        &mut env.ctx,
    )
    .unwrap();
    show::run(
        show::Args {
            id: "my-slug".into(),
            json: false,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}

#[test]
fn show_by_id_prefix() {
    let mut env = common::setup();
    add::run(add_args("Prefix task"), &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    let prefix = task.id.to_string().replace('-', "")[..8].to_string();
    show::run(
        show::Args {
            id: prefix,
            json: false,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}

#[test]
fn show_json_output() {
    let mut env = common::setup();
    add::run(add_args("JSON task"), &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    show::run(
        show::Args {
            id: task.id.to_string(),
            json: true,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}

#[test]
fn show_nonexistent_errors() {
    let env = common::setup();
    let err = show::run(
        show::Args {
            id: "00000000-0000-0000-0000-000000000000".into(),
            json: false,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("not found") || err.to_string().contains("00000000"),
        "unexpected: {err}"
    );
}

#[test]
fn show_task_with_all_optional_fields() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("full-task".into()),
            due: Some("2026-12-31".into()),
            priority: Some("high".into()),
            tags: vec!["@work".into(), "#laptop".into()],
            description: Some("A description.".into()),
            url: Some("https://example.com".into()),
            notes: Some("Some notes.".into()),
            assignee: Some("alice".into()),
            adjust: Some(1.5),
            ..add_args("Full task")
        },
        &mut env.ctx,
    )
    .unwrap();
    show::run(
        show::Args {
            id: "full-task".into(),
            json: false,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}

#[test]
fn show_task_with_parent() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("parent".into()),
            ..add_args("Parent task")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            slug: Some("child".into()),
            parent: Some("parent".into()),
            ..add_args("Child task")
        },
        &mut env.ctx,
    )
    .unwrap();
    show::run(
        show::Args {
            id: "child".into(),
            json: false,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}

#[test]
fn show_task_with_blocker() {
    let mut env = common::setup();
    add::run(
        add::Args {
            slug: Some("blocker".into()),
            ..add_args("Blocking task")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            slug: Some("blocked".into()),
            blocked_by: vec!["blocker".into()],
            ..add_args("Blocked task")
        },
        &mut env.ctx,
    )
    .unwrap();
    show::run(
        show::Args {
            id: "blocked".into(),
            json: false,
            fields: vec![],
        },
        &env.ctx,
    )
    .unwrap();
}

// ── the recurrence block ─────────────────────────────────────────────────────

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 6, 1).unwrap()
}

/// The recurrence lines `show` would print for a task added with these flags.
fn lines_for(env: &mut common::TestEnv, a: add::Args) -> Vec<String> {
    add::run(a, &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);
    show::recurrence_lines(&task, today())
}

#[test]
fn show_spells_out_a_configured_leeway() {
    let mut env = common::setup();
    let lines = lines_for(
        &mut env,
        add::Args {
            due: Some("2026-06-01".into()),
            recur_completion: Some(30),
            recur_snap: Some("dom:1".into()),
            recur_snap_leeway: Some("3".into()),
            ..add_args("Pay the rent")
        },
    );

    assert_eq!(lines[0], "Recur:    30d after completion");
    assert_eq!(
        lines[1],
        "Snap:     day 1 of month, leeway 3d back / 3d forward"
    );
    // Completed today, the raw date is 2026-07-01 — already on the boundary.
    assert_eq!(lines[2], "Next:     2026-07-01  (if completed today)");
}

/// The parenthetical is the point: it turns a default nobody chose into
/// something visible next to the snap it silently governs.
#[test]
fn show_names_the_default_when_no_leeway_is_set() {
    let mut env = common::setup();
    let lines = lines_for(
        &mut env,
        add::Args {
            due: Some("2026-06-01".into()),
            recur_completion: Some(30),
            recur_snap: Some("dom:1".into()),
            ..add_args("Pay the rent")
        },
    );

    assert_eq!(
        lines[1],
        "Snap:     day 1 of month, no leeway (always moves later)"
    );
}

#[test]
fn show_reports_an_asymmetric_leeway_in_both_directions() {
    let mut env = common::setup();
    let lines = lines_for(
        &mut env,
        add::Args {
            due: Some("2026-06-01".into()),
            recur_completion: Some(30),
            recur_snap: Some("monday".into()),
            recur_snap_leeway: Some("5,0".into()),
            ..add_args("Weekly chore")
        },
    );

    assert_eq!(lines[1], "Snap:     Monday, leeway 5d back / 0d forward");
}

/// The anchor decides which dates a schedule can produce, so it belongs on the
/// line — `show` used to destructure it away entirely.
#[test]
fn show_reports_the_schedule_anchor_and_an_absent_snap() {
    let mut env = common::setup();
    let lines = lines_for(
        &mut env,
        add::Args {
            due: Some("2026-05-04".into()),
            recur_schedule: Some("FREQ=WEEKLY;BYDAY=MO".into()),
            ..add_args("Weekly review")
        },
    );

    assert_eq!(
        lines[0],
        "Recur:    schedule (FREQ=WEEKLY;BYDAY=MO), anchor 2026-05-04"
    );
    assert_eq!(lines[1], "Snap:     none");
}

#[test]
fn show_prints_nothing_for_a_task_without_a_rule() {
    let mut env = common::setup();
    assert!(lines_for(&mut env, add_args("Plain task")).is_empty());
}
