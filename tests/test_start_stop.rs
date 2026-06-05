mod common;

use next::cli::commands::{add, done, start, stop};
use next::core::domain::task::Status;

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
fn start_sets_started_status() {
    let mut env = common::setup();
    add::run(add_args("My task"), &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);

    start::run(start::Args { id: task.id.to_string(), json: false }, &mut env.ctx).unwrap();

    let updated = env.ctx.repo.store.get_task(task.id).unwrap();
    assert_eq!(updated.status, Status::Started);
}

#[test]
fn start_logs_time_event() {
    let mut env = common::setup();
    add::run(add_args("Timed task"), &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);

    start::run(start::Args { id: task.id.to_string(), json: false }, &mut env.ctx).unwrap();

    let updated = env.ctx.repo.store.get_task(task.id).unwrap();
    let log = updated.data.get("time_log").expect("time_log should exist");
    let arr = log.as_array().expect("time_log should be an array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["event"], "start");
    assert!(arr[0]["at"].is_string());
}

#[test]
fn stop_returns_to_open_and_logs_event() {
    let mut env = common::setup();
    add::run(add_args("Stoppable task"), &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);

    start::run(start::Args { id: task.id.to_string(), json: false }, &mut env.ctx).unwrap();
    stop::run(stop::Args { id: task.id.to_string(), json: false }, &mut env.ctx).unwrap();

    let updated = env.ctx.repo.store.get_task(task.id).unwrap();
    assert_eq!(updated.status, Status::Open);

    let arr = updated.data["time_log"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["event"], "start");
    assert_eq!(arr[1]["event"], "stop");
}

#[test]
fn started_task_appears_in_default_list() {
    let mut env = common::setup();
    add::run(add_args("Active task"), &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);

    start::run(start::Args { id: task.id.to_string(), json: false }, &mut env.ctx).unwrap();

    // Default list should include started tasks.
    let listed = env.ctx.repo.store.list_tasks().unwrap();
    assert!(listed.iter().any(|t| t.id == task.id && t.status == Status::Started));
}

#[test]
fn multiple_start_stop_cycles_accumulate_log() {
    let mut env = common::setup();
    add::run(add_args("Multi-cycle"), &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);

    start::run(start::Args { id: task.id.to_string(), json: false }, &mut env.ctx).unwrap();
    stop::run(stop::Args { id: task.id.to_string(), json: false }, &mut env.ctx).unwrap();
    start::run(start::Args { id: task.id.to_string(), json: false }, &mut env.ctx).unwrap();
    stop::run(stop::Args { id: task.id.to_string(), json: false }, &mut env.ctx).unwrap();

    let updated = env.ctx.repo.store.get_task(task.id).unwrap();
    let arr = updated.data["time_log"].as_array().unwrap();
    assert_eq!(arr.len(), 4);
    assert_eq!(arr[0]["event"], "start");
    assert_eq!(arr[1]["event"], "stop");
    assert_eq!(arr[2]["event"], "start");
    assert_eq!(arr[3]["event"], "stop");
}

#[test]
fn started_task_can_be_marked_done() {
    let mut env = common::setup();
    add::run(add_args("Finish me"), &mut env.ctx).unwrap();
    let task = env.ctx.repo.store.list_tasks().unwrap().remove(0);

    start::run(start::Args { id: task.id.to_string(), json: false }, &mut env.ctx).unwrap();
    done::run(done::Args { id: task.id.to_string(), completed_at: None, json: false }, &mut env.ctx).unwrap();

    let updated = env.ctx.repo.store.get_task(task.id).unwrap();
    assert_eq!(updated.status, Status::Done);
}
