mod common;

use chrono::Local;
use next::cli::commands::{add, done, start};
use next::cli::filter::FilterArgs;
use next::domain::{filter, scoring};
use next::domain::scoring::ScoredTask;
use next::domain::tag::TagMeta;
use next::domain::task::Priority;

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

/// Run the full pipeline: list tasks from the store, apply the default filter,
/// score and sort. Tag metadata is loaded from the store.
fn score_all(env: &mut common::TestEnv) -> Vec<ScoredTask> {
    let today = Local::now().date_naive();
    let filter_set = FilterArgs::default().to_filter_set().unwrap();
    let state = env.ctx.store.get_state().unwrap();
    let all = env.ctx.store.list_tasks().unwrap();
    let filtered = filter::apply(all.clone(), &filter_set, &state, today);
    let tag_metas = env.ctx.store.list_tag_metas().unwrap();
    scoring::score_and_sort(filtered, &all, today, &env.ctx.config.scoring, &tag_metas)
}

fn titles(tasks: &[ScoredTask]) -> Vec<&str> {
    tasks.iter().map(|t| t.task.title.as_str()).collect()
}

// ---------------------------------------------------------------------------
// Priority
// ---------------------------------------------------------------------------

#[test]
fn high_priority_ranks_above_medium() {
    let mut env = common::setup();
    add::run(add_args("Medium task"), &mut env.ctx).unwrap();
    add::run(
        add::Args { priority: Some("high".into()), ..add_args("High task") },
        &mut env.ctx,
    ).unwrap();

    let ranked = score_all(&mut env);
    let t = titles(&ranked);
    let hi = t.iter().position(|&s| s == "High task").unwrap();
    let md = t.iter().position(|&s| s == "Medium task").unwrap();
    assert!(hi < md, "high priority should outrank medium");
}

#[test]
fn low_priority_ranks_below_medium() {
    let mut env = common::setup();
    add::run(add_args("Medium task"), &mut env.ctx).unwrap();
    add::run(
        add::Args { priority: Some("low".into()), ..add_args("Low task") },
        &mut env.ctx,
    ).unwrap();

    let ranked = score_all(&mut env);
    let t = titles(&ranked);
    let lo = t.iter().position(|&s| s == "Low task").unwrap();
    let md = t.iter().position(|&s| s == "Medium task").unwrap();
    assert!(lo > md, "low priority should rank below medium");
}

// ---------------------------------------------------------------------------
// Due date
// ---------------------------------------------------------------------------

#[test]
fn due_today_ranks_above_no_due_date() {
    let mut env = common::setup();
    add::run(add_args("No due"), &mut env.ctx).unwrap();
    let today = Local::now().date_naive().to_string();
    add::run(
        add::Args { due: Some(today), ..add_args("Due today") },
        &mut env.ctx,
    ).unwrap();

    let ranked = score_all(&mut env);
    let t = titles(&ranked);
    let due_pos = t.iter().position(|&s| s == "Due today").unwrap();
    let no_due_pos = t.iter().position(|&s| s == "No due").unwrap();
    assert!(due_pos < no_due_pos, "task due today should outrank task with no due date");
}

#[test]
fn overdue_task_ranks_above_due_tomorrow() {
    let mut env = common::setup();
    use chrono::Duration;
    let yesterday = (Local::now().date_naive() - Duration::days(1)).to_string();
    let tomorrow = (Local::now().date_naive() + Duration::days(1)).to_string();

    add::run(add::Args { due: Some(tomorrow), ..add_args("Due tomorrow") }, &mut env.ctx).unwrap();
    add::run(add::Args { due: Some(yesterday), ..add_args("Overdue") }, &mut env.ctx).unwrap();

    let ranked = score_all(&mut env);
    let t = titles(&ranked);
    let ov = t.iter().position(|&s| s == "Overdue").unwrap();
    let tm = t.iter().position(|&s| s == "Due tomorrow").unwrap();
    assert!(ov < tm, "overdue task should outrank task due tomorrow");
}

// ---------------------------------------------------------------------------
// Started bonus
// ---------------------------------------------------------------------------

#[test]
fn started_task_ranks_above_equivalent_open() {
    let mut env = common::setup();
    add::run(add::Args { slug: Some("open".into()), ..add_args("Open task") }, &mut env.ctx).unwrap();
    add::run(add::Args { slug: Some("started".into()), ..add_args("Started task") }, &mut env.ctx).unwrap();
    start::run(start::Args { id: "started".into(), json: false }, &mut env.ctx).unwrap();

    let ranked = score_all(&mut env);
    let t = titles(&ranked);
    let st = t.iter().position(|&s| s == "Started task").unwrap();
    let op = t.iter().position(|&s| s == "Open task").unwrap();
    assert!(st < op, "started task should outrank equivalent open task");
}

// ---------------------------------------------------------------------------
// Parent priority factor
// ---------------------------------------------------------------------------

#[test]
fn child_of_high_priority_parent_outranks_child_of_low_priority_parent() {
    let mut env = common::setup();

    // High-priority parent — its children are hidden until we mark it done,
    // but we want to test child scoring, so use started status to make it
    // a leaf (no children yet when we run score_all).
    add::run(
        add::Args { slug: Some("hi-parent".into()), priority: Some("high".into()), ..add_args("High parent") },
        &mut env.ctx,
    ).unwrap();
    let hi = env.ctx.store.get_task_by_slug("hi-parent").unwrap().unwrap();
    add::run(
        add::Args { slug: Some("hi-child".into()), parent: Some(hi.id.to_string()), ..add_args("Child of high") },
        &mut env.ctx,
    ).unwrap();

    add::run(
        add::Args { slug: Some("lo-parent".into()), priority: Some("low".into()), ..add_args("Low parent") },
        &mut env.ctx,
    ).unwrap();
    let lo = env.ctx.store.get_task_by_slug("lo-parent").unwrap().unwrap();
    add::run(
        add::Args { slug: Some("lo-child".into()), parent: Some(lo.id.to_string()), ..add_args("Child of low") },
        &mut env.ctx,
    ).unwrap();

    // Both parents are hidden (they have open children). Score only the children.
    let ranked = score_all(&mut env);
    let t = titles(&ranked);
    assert!(!t.contains(&"High parent"), "parent hidden while children open");
    assert!(!t.contains(&"Low parent"), "parent hidden while children open");

    let hi_pos = t.iter().position(|&s| s == "Child of high").unwrap();
    let lo_pos = t.iter().position(|&s| s == "Child of low").unwrap();
    assert!(hi_pos < lo_pos, "child of high-priority parent should outrank child of low-priority parent");
}

// ---------------------------------------------------------------------------
// Tag priority factor
// ---------------------------------------------------------------------------

#[test]
fn high_priority_tag_boosts_task_rank() {
    let mut env = common::setup();

    add::run(add_args("Plain task"), &mut env.ctx).unwrap();
    add::run(
        add::Args { tags: vec!["urgent".into()], ..add_args("Urgent task") },
        &mut env.ctx,
    ).unwrap();

    // Give the tag high priority in the store.
    let mut meta = TagMeta::default();
    meta.priority = Some(Priority::High);
    env.ctx.store.set_tag_meta("urgent", meta).unwrap();

    let ranked = score_all(&mut env);
    let t = titles(&ranked);
    let urg = t.iter().position(|&s| s == "Urgent task").unwrap();
    let plain = t.iter().position(|&s| s == "Plain task").unwrap();
    assert!(urg < plain, "task with high-priority tag should outrank plain task");
}

#[test]
fn low_priority_tag_penalises_task_rank() {
    let mut env = common::setup();

    add::run(add_args("Plain task"), &mut env.ctx).unwrap();
    add::run(
        add::Args { tags: vec!["someday".into()], ..add_args("Someday task") },
        &mut env.ctx,
    ).unwrap();

    let mut meta = TagMeta::default();
    meta.priority = Some(Priority::Low);
    env.ctx.store.set_tag_meta("someday", meta).unwrap();

    let ranked = score_all(&mut env);
    let t = titles(&ranked);
    let sm = t.iter().position(|&s| s == "Someday task").unwrap();
    let plain = t.iter().position(|&s| s == "Plain task").unwrap();
    assert!(sm > plain, "task with low-priority tag should rank below plain task");
}

// ---------------------------------------------------------------------------
// Score adjustment
// ---------------------------------------------------------------------------

#[test]
fn positive_score_adjustment_moves_task_up() {
    let mut env = common::setup();

    add::run(add_args("Normal task"), &mut env.ctx).unwrap();
    add::run(
        add::Args { adjust: Some(10.0), ..add_args("Boosted task") },
        &mut env.ctx,
    ).unwrap();

    let ranked = score_all(&mut env);
    let t = titles(&ranked);
    let boosted = t.iter().position(|&s| s == "Boosted task").unwrap();
    let normal = t.iter().position(|&s| s == "Normal task").unwrap();
    assert!(boosted < normal, "task with positive score_adjustment should outrank a normal task");
}

#[test]
fn negative_score_adjustment_moves_task_down() {
    let mut env = common::setup();

    add::run(add_args("Normal task"), &mut env.ctx).unwrap();
    add::run(
        add::Args { adjust: Some(-10.0), ..add_args("Penalised task") },
        &mut env.ctx,
    ).unwrap();

    let ranked = score_all(&mut env);
    let t = titles(&ranked);
    let penalised = t.iter().position(|&s| s == "Penalised task").unwrap();
    let normal = t.iter().position(|&s| s == "Normal task").unwrap();
    assert!(penalised > normal, "task with large negative adjustment should rank below a normal task");
}

// ---------------------------------------------------------------------------
// Done task excluded from scored list
// ---------------------------------------------------------------------------

#[test]
fn done_task_does_not_appear_in_scored_list() {
    let mut env = common::setup();

    add::run(add::Args { slug: Some("open".into()), ..add_args("Open task") }, &mut env.ctx).unwrap();
    add::run(add::Args { slug: Some("finished".into()), ..add_args("Done task") }, &mut env.ctx).unwrap();
    done::run(done::Args { id: "finished".into(), json: false }, &mut env.ctx).unwrap();

    let ranked = score_all(&mut env);
    let t = titles(&ranked);
    assert!(t.contains(&"Open task"));
    assert!(!t.contains(&"Done task"), "done task must not appear in default scored list");
}
