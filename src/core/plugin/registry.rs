//! Machine-local plugin registry.
//!
//! Plugins subscribe to individual tasks; when a subscribed task is updated,
//! `next` notifies the plugin (see [`super::notify`]).  The registry is the
//! `[[plugin]]` section of the combined machine-local `state.toml` under
//! `$XDG_STATE_HOME` (never committed to git — plugin binaries are
//! per-machine), persisted via the shared
//! [`crate::core::storage::machine_state`] helpers under the single state lock
//! (`.state.toml.lock`).

use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::{
    error::{TaskError, Result},
    storage::{self, machine_state::update_machine_state},
};

/// The full set of registered plugins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRegistry {
    /// Serialised as a TOML `[[plugin]]` array.
    #[serde(default, rename = "plugin")]
    pub plugins: Vec<Plugin>,
}

fn default_enabled() -> bool { true }

/// A single registered plugin and the tasks it watches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plugin {
    /// Unique plugin name; also used as the loop-guard origin token.
    pub name: String,
    /// Export-hook command to spawn, as argv (`command[0]` is the program).
    /// Never shell-parsed. May be empty for a sync-only (import-only) plugin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// Task ids this plugin is subscribed to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<Uuid>,
    /// Periodic-sync (import) command, as argv — distinct from the export
    /// `command`. Empty means this plugin has no periodic sync.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sync_command: Vec<String>,
    /// The plugin's advertised default sync interval (seconds). Middle priority
    /// in SYSTEM → PLUGIN → USER.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_sync_interval_secs: Option<u64>,
    /// The user's override of the sync interval (seconds). Highest priority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_interval_secs: Option<u64>,
    /// Whether the plugin's periodic sync is enabled. Defaults to `true`.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

impl Plugin {
    /// A new plugin with the given name, enabled, and everything else empty.
    fn new(name: &str) -> Self {
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
}

impl PluginRegistry {
    /// Creates or replaces the export command for `name`, preserving everything else.
    pub fn set_command(&mut self, name: &str, command: Vec<String>) {
        if let Some(p) = self.plugins.iter_mut().find(|p| p.name == name) {
            p.command = command;
        } else {
            let mut p = Plugin::new(name);
            p.command = command;
            self.plugins.push(p);
        }
    }

    /// Creates or replaces the periodic-sync command for `name` (upserts the
    /// plugin if absent), preserving everything else.
    pub fn set_sync_command(&mut self, name: &str, sync_command: Vec<String>) {
        if let Some(p) = self.plugins.iter_mut().find(|p| p.name == name) {
            p.sync_command = sync_command;
        } else {
            let mut p = Plugin::new(name);
            p.sync_command = sync_command;
            self.plugins.push(p);
        }
    }

    /// Sets the plugin-advertised default sync interval. Errors if unknown.
    pub fn set_default_sync_interval(&mut self, name: &str, secs: Option<u64>) -> Result<()> {
        self.find_mut(name)?.default_sync_interval_secs = secs;
        Ok(())
    }

    /// Sets the user sync-interval override. Errors if the plugin is unknown.
    pub fn set_sync_interval(&mut self, name: &str, secs: Option<u64>) -> Result<()> {
        self.find_mut(name)?.sync_interval_secs = secs;
        Ok(())
    }

    /// Enables or disables a plugin's periodic sync. Errors if unknown.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> Result<()> {
        self.find_mut(name)?.enabled = enabled;
        Ok(())
    }

    fn find_mut(&mut self, name: &str) -> Result<&mut Plugin> {
        self.plugins
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| TaskError::Other(format!("unknown plugin {name:?}")))
    }

    /// Subscribes `name` to `task_id`. Errors if the plugin is not registered.
    pub fn watch(&mut self, name: &str, task_id: Uuid) -> Result<()> {
        let plugin = self
            .plugins
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| TaskError::Other(format!("unknown plugin {name:?} — register it first")))?;
        if !plugin.tasks.contains(&task_id) {
            plugin.tasks.push(task_id);
        }
        Ok(())
    }

    /// Unsubscribes `name` from `task_id`. Errors if the plugin is not registered.
    pub fn unwatch(&mut self, name: &str, task_id: Uuid) -> Result<()> {
        let plugin = self
            .plugins
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| TaskError::Other(format!("unknown plugin {name:?}")))?;
        plugin.tasks.retain(|t| *t != task_id);
        Ok(())
    }

    /// Removes a plugin entirely. Returns whether anything was removed.
    pub fn unregister(&mut self, name: &str) -> bool {
        let before = self.plugins.len();
        self.plugins.retain(|p| p.name != name);
        self.plugins.len() != before
    }

    /// Removes `task_id` from every plugin's subscriptions (used after a task
    /// is deleted).
    pub fn prune_task(&mut self, task_id: Uuid) {
        for plugin in &mut self.plugins {
            plugin.tasks.retain(|t| *t != task_id);
        }
    }

    /// Iterates the plugins subscribed to `task_id`.
    pub fn subscribers(&self, task_id: Uuid) -> impl Iterator<Item = &Plugin> {
        self.plugins.iter().filter(move |p| p.tasks.contains(&task_id))
    }
}

// ── Persistence via the combined machine state (the public API) ────────────────
//
// Each operation runs inside `update_machine_state`, which holds the single
// state lock across the whole load → modify → save so concurrent edits to any
// machine-local section cannot lose updates.

/// Loads the registry from the combined `state.toml` (empty if no file yet).
pub fn load(root: &Path) -> Result<PluginRegistry> {
    let machine = storage::load_machine_state(root)?;
    Ok(PluginRegistry { plugins: machine.plugins })
}

/// Mutates the plugin section in place, preserving the other state sections.
fn modify<F, T>(root: &Path, f: F) -> Result<T>
where
    F: FnOnce(&mut PluginRegistry) -> Result<T>,
{
    update_machine_state(root, |machine| {
        let mut reg = PluginRegistry {
            plugins: std::mem::take(&mut machine.plugins),
        };
        let out = f(&mut reg);
        machine.plugins = reg.plugins;
        out
    })
}

pub fn register(root: &Path, name: &str, command: Vec<String>) -> Result<()> {
    modify(root, |reg| {
        reg.set_command(name, command);
        Ok(())
    })
}

pub fn watch(root: &Path, name: &str, task_id: Uuid) -> Result<()> {
    modify(root, |reg| reg.watch(name, task_id))
}

pub fn unwatch(root: &Path, name: &str, task_id: Uuid) -> Result<()> {
    modify(root, |reg| reg.unwatch(name, task_id))
}

pub fn unregister(root: &Path, name: &str) -> Result<bool> {
    modify(root, |reg| Ok(reg.unregister(name)))
}

pub fn prune_task(root: &Path, task_id: Uuid) -> Result<()> {
    modify(root, |reg| {
        reg.prune_task(task_id);
        Ok(())
    })
}

pub fn set_sync_command(root: &Path, name: &str, sync_command: Vec<String>) -> Result<()> {
    modify(root, |reg| {
        reg.set_sync_command(name, sync_command);
        Ok(())
    })
}

pub fn set_default_sync_interval(root: &Path, name: &str, secs: Option<u64>) -> Result<()> {
    modify(root, |reg| reg.set_default_sync_interval(name, secs))
}

pub fn set_sync_interval(root: &Path, name: &str, secs: Option<u64>) -> Result<()> {
    modify(root, |reg| reg.set_sync_interval(name, secs))
}

pub fn set_enabled(root: &Path, name: &str, enabled: bool) -> Result<()> {
    modify(root, |reg| reg.set_enabled(name, enabled))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn round_trip_via_toml() {
        let mut reg = PluginRegistry::default();
        reg.set_command("forgejo", argv("next-forgejo sync"));
        let id = Uuid::new_v4();
        reg.watch("forgejo", id).unwrap();

        let s = toml::to_string_pretty(&reg).unwrap();
        let loaded: PluginRegistry = toml::from_str(&s).unwrap();
        assert_eq!(loaded, reg);
        assert_eq!(loaded.plugins[0].command, vec!["next-forgejo", "sync"]);
        assert_eq!(loaded.plugins[0].tasks, vec![id]);
    }

    #[test]
    fn round_trip_with_sync_fields() {
        let mut reg = PluginRegistry::default();
        reg.set_sync_command("forgejo", argv("next-forgejo sync"));
        reg.set_default_sync_interval("forgejo", Some(43200)).unwrap();
        reg.set_sync_interval("forgejo", Some(7200)).unwrap();
        reg.set_enabled("forgejo", false).unwrap();

        let loaded: PluginRegistry = toml::from_str(&toml::to_string_pretty(&reg).unwrap()).unwrap();
        let p = &loaded.plugins[0];
        assert_eq!(p.sync_command, vec!["next-forgejo", "sync"]);
        assert_eq!(p.default_sync_interval_secs, Some(43200));
        assert_eq!(p.sync_interval_secs, Some(7200));
        assert!(!p.enabled);
    }

    #[test]
    fn pre_req_b_plugin_loads_with_defaults() {
        // A `[[plugin]]` written before Req B (no sync fields, no `enabled`).
        let toml = "[[plugin]]\nname = \"forgejo\"\ncommand = [\"next-forgejo\", \"hook\"]\n";
        let reg: PluginRegistry = toml::from_str(toml).unwrap();
        let p = &reg.plugins[0];
        assert!(p.enabled, "enabled must default to true for legacy plugins");
        assert!(p.sync_command.is_empty());
        assert!(p.sync_interval_secs.is_none());
        assert!(p.default_sync_interval_secs.is_none());
    }

    #[test]
    fn set_enabled_unknown_plugin_errors() {
        let mut reg = PluginRegistry::default();
        assert!(reg.set_enabled("ghost", false).is_err());
    }

    #[test]
    fn register_replace_keeps_tasks() {
        let mut reg = PluginRegistry::default();
        reg.set_command("p", argv("old-cmd"));
        let id = Uuid::new_v4();
        reg.watch("p", id).unwrap();

        reg.set_command("p", argv("new-cmd --flag"));
        assert_eq!(reg.plugins.len(), 1);
        assert_eq!(reg.plugins[0].command, vec!["new-cmd", "--flag"]);
        assert_eq!(reg.plugins[0].tasks, vec![id], "subscriptions preserved on re-register");
    }

    #[test]
    fn watch_unknown_plugin_errors() {
        let mut reg = PluginRegistry::default();
        let err = reg.watch("ghost", Uuid::new_v4()).unwrap_err();
        assert!(err.to_string().contains("unknown plugin"));
        assert!(reg.unwatch("ghost", Uuid::new_v4()).is_err());
    }

    #[test]
    fn watch_is_idempotent() {
        let mut reg = PluginRegistry::default();
        reg.set_command("p", argv("c"));
        let id = Uuid::new_v4();
        reg.watch("p", id).unwrap();
        reg.watch("p", id).unwrap();
        assert_eq!(reg.plugins[0].tasks, vec![id]);
    }

    #[test]
    fn unwatch_and_unregister() {
        let mut reg = PluginRegistry::default();
        reg.set_command("p", argv("c"));
        let id = Uuid::new_v4();
        reg.watch("p", id).unwrap();
        reg.unwatch("p", id).unwrap();
        assert!(reg.plugins[0].tasks.is_empty());

        assert!(reg.unregister("p"));
        assert!(reg.plugins.is_empty());
        assert!(!reg.unregister("p"), "removing a missing plugin returns false");
    }

    #[test]
    fn subscribers_and_prune() {
        let mut reg = PluginRegistry::default();
        reg.set_command("a", argv("a"));
        reg.set_command("b", argv("b"));
        let id = Uuid::new_v4();
        let other = Uuid::new_v4();
        reg.watch("a", id).unwrap();
        reg.watch("b", id).unwrap();
        reg.watch("b", other).unwrap();

        let mut names: Vec<&str> = reg.subscribers(id).map(|p| p.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
        assert_eq!(reg.subscribers(other).count(), 1);

        reg.prune_task(id);
        assert_eq!(reg.subscribers(id).count(), 0);
        assert_eq!(reg.subscribers(other).count(), 1, "unrelated subscription kept");
    }

    #[test]
    fn lock_wrapped_ops_round_trip() {
        // Exercises the public lock-based API end to end against a real repo dir.
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        register(root, "p", argv("cmd")).unwrap();
        let id = Uuid::new_v4();
        watch(root, "p", id).unwrap();

        let reg = load(root).unwrap();
        assert_eq!(reg.plugins.len(), 1);
        assert_eq!(reg.subscribers(id).count(), 1);

        unwatch(root, "p", id).unwrap();
        assert_eq!(load(root).unwrap().subscribers(id).count(), 0);
        assert!(unregister(root, "p").unwrap());
        assert!(load(root).unwrap().plugins.is_empty());
    }
}
