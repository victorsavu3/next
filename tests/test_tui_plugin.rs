//! Regression tests for the TUI's plugin export hooks.
//!
//! The CLI (`src/cli/main.rs`) and the MCP server (`src/mcp/tools/mod.rs`)
//! both drain `TaskRepository::take_task_events()` after a successful mutation
//! and forward them to `next::core::plugin::notify`, pruning the plugin
//! registry on delete. The TUI records events the same way
//! (`self.repo.record_task_event(...)`) but never drained or dispatched them —
//! so a TUI-driven mutation silently skipped plugin notifications, and a
//! TUI-driven delete never pruned the task from plugin registries. These
//! tests exercise the TUI's public `App`/`Action` surface exactly the way
//! `tests/test_plugin.rs` exercises the CLI, and must observe the same
//! plugin side effects.

#![cfg(feature = "tui")]

mod common;

use next::core::domain::task::Task;
use next::core::plugin::registry;
use next::tui::app::{Action, App};
use next::tui::config::ConfigSource;
use uuid::Uuid;

/// A plugin command that copies the event payload (delivered on stdin) into
/// `$NEXT_REPO/plugin_out.json`, so the test can inspect what the plugin saw.
/// Identical to the fixture in `tests/test_plugin.rs`.
fn capture_command() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        "cat > \"$NEXT_REPO/plugin_out.json\"".into(),
    ]
}

fn read_output(repo: &std::path::Path) -> Option<serde_json::Value> {
    let path = repo.join("plugin_out.json");
    if !path.exists() {
        return None;
    }
    Some(serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap())
}

/// Builds an `App` over `repo`, seeded with `task` (saved directly to the
/// store, then reloaded so it is selected) — the same fixture shape as the
/// `App` unit tests' own `app_with_repo_tasks` helper in `src/tui/app.rs`.
fn app_with_task(mut repo: next::TaskRepository, task: &Task) -> App {
    repo.store.save_task(task).unwrap();
    let today = chrono::Local::now().date_naive();
    let mut app = App::new(next::Config::default(), repo, ConfigSource::Default, today);
    app.reload().unwrap();
    app
}

#[test]
fn tui_done_notifies_subscribed_plugin() {
    std::env::set_var("NEXT_WAIT_PLUGINS", "1");

    let env = common::setup();
    let repo_root = env.ctx.repo.repo_root.clone();
    let task = Task::new("Watch me");
    let id = task.id;

    registry::register(&repo_root, "demo", capture_command()).unwrap();
    registry::watch(&repo_root, "demo", id).unwrap();

    let mut app = app_with_task(env.ctx.repo, &task);
    assert_eq!(app.selected_task().map(|t| t.id), Some(id));

    app.update(Action::Done);

    let out = read_output(&repo_root)
        .expect("a TUI-driven `done` mutation must notify the subscribed plugin");
    assert_eq!(out["event"].as_str(), Some("done"));
    assert_eq!(out["task_id"].as_str(), Some(id.to_string().as_str()));
}

#[test]
fn tui_delete_notifies_and_prunes_registry() {
    std::env::set_var("NEXT_WAIT_PLUGINS", "1");

    let env = common::setup();
    let repo_root = env.ctx.repo.repo_root.clone();
    let task = Task::new("Delete me");
    let id = task.id;

    registry::register(&repo_root, "demo", capture_command()).unwrap();
    registry::watch(&repo_root, "demo", id).unwrap();

    let mut app = app_with_task(env.ctx.repo, &task);
    assert_eq!(app.selected_task().map(|t| t.id), Some(id));

    app.update(Action::OpenDelete);
    app.update(Action::ConfirmDelete);

    let out = read_output(&repo_root)
        .expect("a TUI-driven `delete` mutation must notify the subscribed plugin");
    assert_eq!(out["event"].as_str(), Some("delete"));
    assert_eq!(out["task_id"].as_str(), Some(id.to_string().as_str()));

    let reg = registry::load(&repo_root).unwrap();
    assert_eq!(
        reg.subscribers(id).count(),
        0,
        "a TUI-driven delete must prune the task from plugin registries, like the CLI/MCP do"
    );
}

/// Sanity check on the harness itself: an unrelated task id must not be
/// notified, so the two tests above are actually exercising the loop-guarded
/// per-task subscription path rather than something looser.
#[test]
fn tui_mutation_on_unwatched_task_does_not_notify() {
    std::env::set_var("NEXT_WAIT_PLUGINS", "1");

    let env = common::setup();
    let repo_root = env.ctx.repo.repo_root.clone();
    let task = Task::new("Not watched");

    registry::register(&repo_root, "demo", capture_command()).unwrap();
    registry::watch(&repo_root, "demo", Uuid::new_v4()).unwrap();

    let mut app = app_with_task(env.ctx.repo, &task);
    app.update(Action::Done);

    assert!(
        read_output(&repo_root).is_none(),
        "no notification for a task nobody subscribed to"
    );
}
