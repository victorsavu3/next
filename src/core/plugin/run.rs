//! Periodic plugin sync (the import trigger).
//!
//! When a normal sync happens, each registered plugin that declares a
//! `sync_command` and is *due* (more than its resolved interval since its last
//! successful sync) is run to completion. This is how the import direction
//! (e.g. `next-forgejo sync`) is driven on a schedule rather than manually.
//!
//! Unlike the export hook ([`super::notify`], which is fire-and-forget), a
//! periodic sync is run synchronously and its exit status is checked: only a
//! successful run advances the plugin's `last_sync` in the machine-local sync
//! state, so a failure is retried on the next sync rather than suppressed for a
//! whole interval.
//!
//! Callers invoke this AFTER a normal [`sync`](crate::core::sync::sync) has
//! returned — i.e. with no repository lock held — so the plugin child (which
//! opens its own repository handle) cannot deadlock on the repo lock.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use chrono::{DateTime, Utc};

use super::registry::Plugin;
use crate::core::sync_state;

/// Resolves a plugin's sync interval using the SYSTEM → PLUGIN → USER
/// precedence: the user override wins, else the plugin's advertised default,
/// else the system default.
pub fn resolve_sync_interval(plugin: &Plugin, system_default_secs: u64) -> Duration {
    let secs = plugin
        .sync_interval_secs
        .or(plugin.default_sync_interval_secs)
        .unwrap_or(system_default_secs);
    Duration::from_secs(secs)
}

/// Runs the periodic sync of every enabled plugin that is due.
///
/// Best-effort: per-plugin failures are logged and isolated (one failing plugin
/// never stops the others), and the function never returns an error or panics —
/// the caller's own sync must not be affected.
pub fn run_due_syncs(repo_root: &Path, system_default_secs: u64, now: DateTime<Utc>) {
    let registry = match super::registry::load(repo_root) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("plugin sync: failed to load registry: {e:#}");
            return;
        }
    };
    let sync = sync_state::load(repo_root).unwrap_or_default();

    for plugin in &registry.plugins {
        if !plugin.enabled || plugin.sync_command.is_empty() {
            continue;
        }
        let interval = resolve_sync_interval(plugin, system_default_secs);
        let last_sync = sync.plugins.get(&plugin.name).and_then(|p| p.last_sync);
        let due = match last_sync {
            Some(last) => now
                .signed_duration_since(last)
                .to_std()
                .map(|elapsed| elapsed >= interval)
                .unwrap_or(true), // negative elapsed (clock skew) → treat as due
            None => true,
        };
        if due {
            run_one(repo_root, plugin, now);
        }
    }
}

/// Runs one plugin's `sync_command` to completion and records `last_sync` on
/// success. Never panics; logs and returns on any error.
fn run_one(repo_root: &Path, plugin: &Plugin, now: DateTime<Utc>) {
    let Some(program) = plugin.sync_command.first() else {
        return; // guarded by the caller, but be defensive
    };

    let mut cmd = Command::new(program);
    cmd.args(&plugin.sync_command[1..])
        .current_dir(repo_root)
        .env("NEXT_REPO", repo_root)
        // Loop guard: the plugin's own task writes carry this origin, so its
        // export hook is never re-triggered by its own import.
        .env("NEXT_PLUGIN_ORIGIN", &plugin.name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let status = match cmd.status() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("plugin {:?} sync: spawn failed: {e}", plugin.name);
            return;
        }
    };

    if !status.success() {
        tracing::warn!("plugin {:?} sync: exited with {status}", plugin.name);
        return; // do not record — retried on the next sync
    }

    tracing::info!("plugin {:?} sync: ok", plugin.name);
    if let Err(e) = sync_state::record_plugin_sync(repo_root, &plugin.name, now) {
        tracing::warn!("plugin {:?} sync: failed to record last_sync: {e:#}", plugin.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugin::registry;
    use crate::core::TaskRepository;

    fn temp_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        {
            let mut cfg = repo.config().unwrap();
            cfg.set_str("user.name", "Test").unwrap();
            cfg.set_str("user.email", "test@test.com").unwrap();
        }
        // Open once so the machine-state dir exists.
        let _ = TaskRepository::open(dir.path().to_path_buf()).unwrap();
        let root = dir.path().to_path_buf();
        (dir, root)
    }

    fn mk(name: &str) -> Plugin {
        Plugin {
            name: name.to_owned(),
            command: Vec::new(),
            tasks: Vec::new(),
            sync_command: Vec::new(),
            default_sync_interval_secs: None,
            sync_interval_secs: None,
            enabled: true,
        }
    }

    #[test]
    fn resolve_precedence_user_over_plugin_over_system() {
        let mut p = mk("p");
        // System default only.
        assert_eq!(resolve_sync_interval(&p, 100), Duration::from_secs(100));
        // Plugin default beats system.
        p.default_sync_interval_secs = Some(50);
        assert_eq!(resolve_sync_interval(&p, 100), Duration::from_secs(50));
        // User override beats both.
        p.sync_interval_secs = Some(7);
        assert_eq!(resolve_sync_interval(&p, 100), Duration::from_secs(7));
    }

    #[test]
    fn due_plugin_runs_and_records_last_sync() {
        let (_dir, root) = temp_repo();
        registry::set_sync_command(&root, "ok", vec!["true".to_owned()]).unwrap();

        let now = Utc::now();
        run_due_syncs(&root, 3600, now);

        let st = sync_state::load(&root).unwrap();
        assert!(
            st.plugins.get("ok").and_then(|p| p.last_sync).is_some(),
            "a successful sync must record last_sync"
        );
    }

    #[test]
    fn recently_synced_plugin_is_skipped() {
        let (_dir, root) = temp_repo();
        registry::set_sync_command(&root, "ok", vec!["true".to_owned()]).unwrap();

        let earlier = Utc::now();
        sync_state::record_plugin_sync(&root, "ok", earlier).unwrap();

        // A long interval + a recent last_sync → not due → last_sync unchanged.
        run_due_syncs(&root, 86400, earlier + chrono::Duration::seconds(5));
        let st = sync_state::load(&root).unwrap();
        assert_eq!(st.plugins["ok"].last_sync, Some(earlier), "must not re-run when fresh");
    }

    #[test]
    fn failing_plugin_does_not_record() {
        let (_dir, root) = temp_repo();
        registry::set_sync_command(&root, "bad", vec!["false".to_owned()]).unwrap();

        run_due_syncs(&root, 3600, Utc::now());
        let st = sync_state::load(&root).unwrap();
        assert!(
            st.plugins.get("bad").and_then(|p| p.last_sync).is_none(),
            "a failing sync must not record last_sync (so it retries)"
        );
    }

    #[test]
    fn disabled_or_commandless_plugins_are_skipped() {
        let (_dir, root) = temp_repo();
        // Disabled, with a sync command.
        registry::set_sync_command(&root, "off", vec!["true".to_owned()]).unwrap();
        registry::set_enabled(&root, "off", false).unwrap();
        // Enabled, but no sync command.
        registry::register(&root, "nosync", vec!["next-forgejo".to_owned(), "hook".to_owned()]).unwrap();

        run_due_syncs(&root, 3600, Utc::now());
        let st = sync_state::load(&root).unwrap();
        assert!(st.plugins.get("off").and_then(|p| p.last_sync).is_none());
        assert!(st.plugins.get("nosync").and_then(|p| p.last_sync).is_none());
    }
}
