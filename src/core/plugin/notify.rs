//! Post-mutation plugin notification (the export hook).
//!
//! After a task mutation completes and the repository lock has been released,
//! the chokepoints in `src/main.rs` (CLI) and `src/mcp/tools/mod.rs` (MCP) call
//! [`notify`] with the buffered events.  For each event, every plugin
//! subscribed to the task is spawned **fire-and-forget** with the event on its
//! stdin (and in `NEXT_PLUGIN_EVENT`).
//!
//! Spawning happens only here — never while the repo lock is held — because a
//! plugin typically calls back into `next` and would otherwise deadlock on
//! `.next.lock`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use uuid::Uuid;

use super::registry::{self, Plugin};

/// A task mutation worth notifying plugins about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskEvent {
    /// The mutation verb (`add`, `start`, `done`, `edit`, `delete`, …).
    pub verb: &'static str,
    pub task_id: Uuid,
}

impl TaskEvent {
    pub fn new(verb: &'static str, task_id: Uuid) -> Self {
        Self { verb, task_id }
    }
}

/// Notifies subscribed plugins of the given task events.
///
/// `origin` is the current process's `NEXT_PLUGIN_ORIGIN` (the plugin that
/// caused these changes, if any): that plugin is skipped so it is never
/// notified of its own work.  Never fails — registry/spawn errors are logged.
pub fn notify(repo_root: &Path, events: &[TaskEvent], origin: Option<&str>) {
    if events.is_empty() {
        return;
    }
    let reg = match registry::load(repo_root) {
        Ok(reg) if !reg.plugins.is_empty() => reg,
        Ok(_) => return, // no plugins registered
        Err(e) => {
            tracing::warn!("plugin notify: registry load failed: {e:#}");
            return;
        }
    };

    // In tests (and when a caller wants synchronous behaviour) wait for each
    // child instead of detaching, so the effect is observable.
    let wait = std::env::var_os("NEXT_WAIT_PLUGINS").is_some();
    let timestamp = chrono::Local::now().to_rfc3339();

    for ev in events {
        for plugin in reg.subscribers(ev.task_id) {
            if Some(plugin.name.as_str()) == origin {
                continue; // loop guard: don't notify a plugin of its own changes
            }
            spawn_plugin(plugin, ev, repo_root, &timestamp, wait);
        }
    }
}

fn spawn_plugin(plugin: &Plugin, ev: &TaskEvent, repo: &Path, timestamp: &str, wait: bool) {
    let Some(program) = plugin.command.first() else {
        tracing::warn!("plugin {:?}: empty command, skipping", plugin.name);
        return;
    };

    let payload = serde_json::json!({
        "event": ev.verb,
        "task_id": ev.task_id.to_string(),
        "repo": repo.display().to_string(),
        "timestamp": timestamp,
    })
    .to_string();

    let mut cmd = Command::new(program);
    cmd.args(&plugin.command[1..])
        .current_dir(repo)
        .env("NEXT_REPO", repo)
        .env("NEXT_PLUGIN_EVENT", &payload)
        .env("NEXT_PLUGIN_ORIGIN", &plugin.name)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            tracing::warn!("plugin {:?}: spawn failed: {e}", plugin.name);
            return;
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.as_bytes());
        // `stdin` is dropped here, closing the pipe so the child sees EOF.
    }

    if wait {
        let _ = child.wait();
    } else {
        // Fire-and-forget, but reap the child so the long-lived MCP server does
        // not accumulate zombies. The helper thread blocks, not the caller.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}
