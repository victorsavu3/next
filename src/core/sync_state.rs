//! Machine-local sync state.
//!
//! Tracks when the local repository last pulled from its remote (`last_pull`)
//! and, for Req B, when each plugin last synced (`plugins`).  It is the
//! `[sync]` section of the combined machine-local `state.toml` under
//! `$XDG_STATE_HOME` (never committed to git — it is per-machine), persisted via
//! the shared [`crate::core::storage::machine_state`] helpers under the single
//! state lock (`.state.toml.lock`).

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::{
    error::Result,
    storage::{self, machine_state::update_machine_state},
};

/// Machine-local sync bookkeeping for a single repository.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    /// When the local repository last pulled cleanly from its remote.
    ///
    /// `None` means "never pulled on this machine" — which is treated as stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pull: Option<DateTime<Utc>>,

    /// When the automatic archive pass last ran on this machine. Throttles
    /// the sync-time pass to at most once per day; `next archive` ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_archive: Option<DateTime<Utc>>,

    /// Per-plugin sync state, keyed by plugin name.  Populated by Req B; this
    /// slice only reads/writes `last_pull` and preserves this map untouched.
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginSyncState>,
}

/// Per-plugin sync bookkeeping (used by Req B).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSyncState {
    /// When this plugin last completed a sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<DateTime<Utc>>,
}

// ── Persistence via the combined machine state (the public API) ────────────────
//
// Each operation runs inside `update_machine_state`, which holds the single
// state lock across the whole load → modify → save so concurrent edits to any
// machine-local section cannot lose updates.

/// Loads the sync state from the combined `state.toml` (default if no file yet).
pub fn load(root: &Path) -> Result<SyncState> {
    Ok(storage::load_machine_state(root)?.sync)
}

/// Records a successful pull at `now`, preserving the plugins map.
pub fn record_pull(root: &Path, now: DateTime<Utc>) -> Result<()> {
    update_machine_state(root, |machine| {
        machine.sync.last_pull = Some(now);
        Ok(())
    })
}

/// Records an automatic archive pass at `now`, preserving the other fields.
pub fn record_archive(root: &Path, now: DateTime<Utc>) -> Result<()> {
    update_machine_state(root, |machine| {
        machine.sync.last_archive = Some(now);
        Ok(())
    })
}

/// Records a successful periodic sync for `name` at `now`, preserving the rest.
pub fn record_plugin_sync(root: &Path, name: &str, now: DateTime<Utc>) -> Result<()> {
    update_machine_state(root, |machine| {
        machine
            .sync
            .plugins
            .entry(name.to_owned())
            .or_default()
            .last_sync = Some(now);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_via_toml() {
        let now = Utc::now();
        let mut state = SyncState {
            last_pull: Some(now),
            ..Default::default()
        };
        state.plugins.insert(
            "forgejo".to_owned(),
            PluginSyncState {
                last_sync: Some(now),
            },
        );

        let s = toml::to_string_pretty(&state).unwrap();
        let loaded: SyncState = toml::from_str(&s).unwrap();
        assert_eq!(loaded, state, "DateTime<Utc> must round-trip through TOML");
        assert_eq!(loaded.last_pull, Some(now));
        assert_eq!(loaded.plugins["forgejo"].last_sync, Some(now));
    }

    #[test]
    fn load_defaults_when_no_state_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let state = load(dir.path()).unwrap();
        assert_eq!(state, SyncState::default());
    }

    #[test]
    fn plugins_map_defaults_when_absent_in_toml() {
        // A file written before Req B (only last_pull) must still load, with an
        // empty plugins map.
        let now = Utc::now();
        let toml = format!("last_pull = \"{}\"\n", now.to_rfc3339());
        let state: SyncState = toml::from_str(&toml).unwrap();
        assert!(state.plugins.is_empty());
        assert!(state.last_pull.is_some());
    }

    #[test]
    fn record_plugin_sync_round_trips_and_preserves_others() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let earlier = Utc::now();
        record_pull(root, earlier).unwrap();

        let now = earlier + chrono::Duration::seconds(5);
        record_plugin_sync(root, "forgejo", now).unwrap();

        let loaded = load(root).unwrap();
        assert_eq!(loaded.plugins["forgejo"].last_sync, Some(now));
        assert_eq!(
            loaded.last_pull,
            Some(earlier),
            "record_plugin_sync must preserve last_pull"
        );
    }

    #[test]
    fn record_pull_preserves_plugins() {
        use crate::core::storage::machine_state::update_machine_state;

        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();

        // Seed a plugins entry (as Req B would) directly in the combined state,
        // then record a pull.
        update_machine_state(root, |machine| {
            machine.sync.plugins.insert(
                "forgejo".to_owned(),
                PluginSyncState {
                    last_sync: Some(Utc::now()),
                },
            );
            Ok(())
        })
        .unwrap();

        let now = Utc::now();
        record_pull(root, now).unwrap();

        let loaded = load(root).unwrap();
        assert_eq!(loaded.last_pull, Some(now));
        assert!(
            loaded.plugins.contains_key("forgejo"),
            "record_pull must preserve plugins"
        );
    }

    #[test]
    fn record_pull_preserves_global_and_plugin_sections() {
        use crate::core::storage::machine_state::{load_machine_state, update_machine_state};

        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();

        // Seed a global field and a plugin registration alongside sync state.
        update_machine_state(root, |machine| {
            machine.global.active_contexts = vec!["@work".into()];
            machine.plugins.push(crate::core::plugin::registry::Plugin {
                name: "forgejo".into(),
                command: vec!["next-forgejo".into()],
                tasks: vec![],
                sync_command: vec![],
                default_sync_interval_secs: None,
                sync_interval_secs: None,
                enabled: true,
            });
            Ok(())
        })
        .unwrap();

        record_pull(root, Utc::now()).unwrap();

        let machine = load_machine_state(root).unwrap();
        assert_eq!(machine.global.active_contexts, vec!["@work"]);
        assert_eq!(
            machine.plugins.len(),
            1,
            "record_pull must not drop plugins"
        );
        assert!(machine.sync.last_pull.is_some());
    }
}
