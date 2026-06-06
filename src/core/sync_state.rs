//! Machine-local sync state.
//!
//! Tracks when the local repository last pulled from its remote (`last_pull`)
//! and, for Req B, when each plugin last synced (`plugins`).  Stored as
//! `sync_state.toml` beside the state and plugin files under `$XDG_STATE_HOME`
//! (never committed to git — it is per-machine), guarded by its own re-entrant
//! lock (`.sync_state.toml.lock`) via [`crate::core::storage::lock_sync_state`].
//!
//! This mirrors the plugin-registry pattern in [`crate::core::plugin::registry`]:
//! path-based `load_from`/`save_to` helpers plus a lock-wrapped public API.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::{
    error::{Result, TaskError},
    storage,
};

/// Machine-local sync bookkeeping for a single repository.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    /// When the local repository last pulled cleanly from its remote.
    ///
    /// `None` means "never pulled on this machine" — which is treated as stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pull: Option<DateTime<Utc>>,

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

// ── Path-based I/O (no locking; for the lock-wrapped fns and tests) ────────────

pub(crate) fn load_from(path: &Path) -> Result<SyncState> {
    if !path.exists() {
        return Ok(SyncState::default());
    }
    let content = fs::read_to_string(path)?;
    toml::from_str::<SyncState>(&content)
        .map_err(|e| TaskError::Other(format!("parse sync_state.toml: {e}")))
}

pub(crate) fn save_to(path: &Path, state: &SyncState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(state)
        .map_err(|e| TaskError::Other(format!("serialize sync_state.toml: {e}")))?;
    storage::toml_store::atomic_write(path, &content)
}

// ── Lock-wrapped operations (the public API) ───────────────────────────────────
//
// Each holds the sync-state lock for the whole load → modify → save so
// concurrent edits cannot lose updates.

/// Loads the sync state (default/empty if no file exists yet).
pub fn load(root: &Path) -> Result<SyncState> {
    let _lock = storage::lock_sync_state(root)?;
    load_from(&storage::sync_state_path_for_repo(root))
}

/// Records a successful pull at `now`, preserving the plugins map.
pub fn record_pull(root: &Path, now: DateTime<Utc>) -> Result<()> {
    let _lock = storage::lock_sync_state(root)?;
    let path = storage::sync_state_path_for_repo(root);
    let mut state = load_from(&path)?;
    state.last_pull = Some(now);
    save_to(&path, &state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_via_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sync_state.toml");

        let now = Utc::now();
        let mut state = SyncState {
            last_pull: Some(now),
            plugins: BTreeMap::new(),
        };
        state
            .plugins
            .insert("forgejo".to_owned(), PluginSyncState { last_sync: Some(now) });
        save_to(&path, &state).unwrap();

        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded, state, "DateTime<Utc> must round-trip through TOML");
        assert_eq!(loaded.last_pull, Some(now));
        assert_eq!(loaded.plugins["forgejo"].last_sync, Some(now));
    }

    #[test]
    fn absent_file_loads_default() {
        let dir = tempfile::TempDir::new().unwrap();
        let state = load_from(&dir.path().join("nope.toml")).unwrap();
        assert_eq!(state, SyncState::default());
        assert!(state.last_pull.is_none());
        assert!(state.plugins.is_empty(), "plugins map defaults to empty");
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
    fn record_pull_preserves_plugins() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let path = storage::sync_state_path_for_repo(root);

        // Seed a plugins entry (as Req B would), then record a pull.
        let mut seed = SyncState::default();
        seed.plugins
            .insert("forgejo".to_owned(), PluginSyncState { last_sync: Some(Utc::now()) });
        save_to(&path, &seed).unwrap();

        let now = Utc::now();
        record_pull(root, now).unwrap();

        let loaded = load(root).unwrap();
        assert_eq!(loaded.last_pull, Some(now));
        assert!(loaded.plugins.contains_key("forgejo"), "record_pull must preserve plugins");
    }
}
