//! End-to-end tests for the plugin export hook.

mod common;

use std::path::Path;

use next::core::service::{create_task, CreateTaskParams};
use next::core::plugin::{notify, registry, TaskEvent};
use tempfile::TempDir;
use uuid::Uuid;

/// A plugin command that copies the event payload (delivered on stdin) into
/// `$NEXT_REPO/plugin_out.json`, so the test can inspect what the plugin saw.
fn capture_command() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        "cat > \"$NEXT_REPO/plugin_out.json\"".into(),
    ]
}

fn read_output(repo: &Path) -> Option<serde_json::Value> {
    let path = repo.join("plugin_out.json");
    if !path.exists() {
        return None;
    }
    Some(serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap())
}

#[test]
fn notify_spawns_subscribed_plugin_with_payload() {
    // Wait for the spawned child so the output file is present synchronously.
    std::env::set_var("NEXT_WAIT_PLUGINS", "1");

    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let id = Uuid::new_v4();
    registry::register(root, "demo", capture_command()).unwrap();
    registry::watch(root, "demo", id).unwrap();

    notify(root, &[TaskEvent::new("start", id)], None);

    let out = read_output(root).expect("subscribed plugin should have run");
    assert_eq!(out["event"].as_str(), Some("start"));
    assert_eq!(out["task_id"].as_str(), Some(id.to_string().as_str()));
    assert_eq!(out["repo"].as_str(), Some(root.display().to_string().as_str()));
    assert!(out["timestamp"].is_string(), "payload carries a timestamp");
}

#[test]
fn notify_skips_origin_plugin() {
    std::env::set_var("NEXT_WAIT_PLUGINS", "1");

    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let id = Uuid::new_v4();
    registry::register(root, "demo", capture_command()).unwrap();
    registry::watch(root, "demo", id).unwrap();

    // The change originated from "demo" itself — it must not be notified.
    notify(root, &[TaskEvent::new("edit", id)], Some("demo"));

    assert!(
        read_output(root).is_none(),
        "loop guard: a plugin must not be notified of changes it caused"
    );
}

#[test]
fn notify_ignores_unsubscribed_tasks() {
    std::env::set_var("NEXT_WAIT_PLUGINS", "1");

    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let watched = Uuid::new_v4();
    let other = Uuid::new_v4();
    registry::register(root, "demo", capture_command()).unwrap();
    registry::watch(root, "demo", watched).unwrap();

    notify(root, &[TaskEvent::new("edit", other)], None);
    assert!(read_output(root).is_none(), "no notification for an unwatched task");

    notify(root, &[TaskEvent::new("edit", watched)], None);
    assert!(read_output(root).is_some(), "notification fires for the watched task");
}

#[test]
fn cli_mutation_handler_records_event() {
    // The CLI handlers record (verb, id) on the context; main.rs drains and
    // dispatches at the post-mutation chokepoint. Here we assert the recording.
    let mut env = common::setup();
    let today = chrono::Local::now().date_naive();
    let task = create_task(
        "Watch me".into(),
        CreateTaskParams::default(),
        today,
        &env.ctx.repo.repo_root.clone(),
        &mut *env.ctx.repo.store,
        &*env.ctx.repo.vcs,
    )
    .unwrap();
    // create_task is the shared service fn (no AppContext) — it records nothing.
    assert!(env.ctx.take_task_events().is_empty());

    next::cli::commands::start::run(
        next::cli::commands::start::Args { id: task.id.to_string(), json: false },
        &mut env.ctx,
    )
    .unwrap();

    let events = env.ctx.take_task_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].verb, "start");
    assert_eq!(events[0].task_id, task.id);
}
