//! The single machine-local state file, `state.toml`.
//!
//! Three machine-local subsystems share one file under
//! `$XDG_STATE_HOME/task-manager/<hash>/state.toml`, guarded by one re-entrant
//! lock (`.state.toml.lock`):
//!
//! * the global runtime [`GlobalState`] (contexts, resources, users) at the top
//!   level,
//! * the plugin registry as a `[[plugin]]` array, and
//! * the [`SyncState`] (`last_pull` + per-plugin sync timestamps) under
//!   `[sync]`.
//!
//! [`load_machine_state`] reads the file (default when absent) and
//! [`update_machine_state`] is the read-modify-write primitive all three
//! subsystems use: it locks, reads, mutates, atomically rewrites, and returns —
//! so no subsystem can clobber another's section under concurrency.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::{
    domain::state::GlobalState,
    error::{Result, TaskError},
    plugin::registry::Plugin,
    storage,
    sync_state::SyncState,
};

/// On-disk layout of `state.toml`: [`GlobalState`] flattened at the top level,
/// followed by the plugin registry (`[[plugin]]`) and the sync state
/// (`[sync]`).
///
/// `GlobalState` stays exactly as it serialised before this consolidation, so
/// existing `state.toml` files (global fields only) load unchanged; the plugin
/// and sync sections are simply absent and default to empty.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MachineState {
    #[serde(flatten)]
    pub global: GlobalState,
    #[serde(default, rename = "plugin", skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<Plugin>,
    #[serde(default, skip_serializing_if = "is_default_sync")]
    pub sync: SyncState,
}

fn is_default_sync(sync: &SyncState) -> bool {
    *sync == SyncState::default()
}

/// Loads the combined machine-local state for the repo at `root`.
///
/// Returns [`MachineState::default`] when the file does not exist yet. Takes the
/// state lock for the read so it is consistent with a concurrent
/// [`update_machine_state`] (the lock is re-entrant, so calling this inside an
/// update does not deadlock).
///
/// Keyed off the repository root via [`storage::state_path_for_repo`], matching
/// the registry / sync-state public API which is also root-based. `TomlStore`,
/// which knows its state-file path directly (and may use a non-XDG path in
/// tests), goes through [`load_machine_state_at`].
pub(crate) fn load_machine_state(root: &Path) -> Result<MachineState> {
    load_machine_state_at(&storage::state_path_for_repo(root), &storage::state_lock_path_for_repo(root))
}

/// Like [`load_machine_state`] but with the state-file and lock paths given
/// explicitly (used by `TomlStore`, which owns its `state_path`).
pub(crate) fn load_machine_state_at(state_path: &Path, lock_path: &Path) -> Result<MachineState> {
    let _lock = acquire_lock(lock_path)?;
    read_machine_state(state_path)
}

/// Reads and parses `state.toml` at `path` without locking (default if absent).
fn read_machine_state(path: &Path) -> Result<MachineState> {
    if !path.exists() {
        return Ok(MachineState::default());
    }
    let content = std::fs::read_to_string(path)?;
    toml::from_str::<MachineState>(&content)
        .map_err(|e| TaskError::Other(format!("parse error in state.toml: {e}")))
}

/// Read-modify-write of the combined machine-local state under the state lock.
///
/// Acquires the (re-entrant, exclusive) state lock, loads the current
/// [`MachineState`], applies `f`, atomically rewrites `state.toml`, and returns
/// `f`'s value. Holding one lock across the whole cycle prevents any of the
/// three subsystems from losing another's concurrent update.
///
/// Root-based; see [`load_machine_state`] for why. `TomlStore` uses
/// [`update_machine_state_at`].
pub(crate) fn update_machine_state<F, T>(root: &Path, f: F) -> Result<T>
where
    F: FnOnce(&mut MachineState) -> Result<T>,
{
    update_machine_state_at(
        &storage::state_path_for_repo(root),
        &storage::state_lock_path_for_repo(root),
        f,
    )
}

/// Like [`update_machine_state`] but with explicit state-file and lock paths.
pub(crate) fn update_machine_state_at<F, T>(state_path: &Path, lock_path: &Path, f: F) -> Result<T>
where
    F: FnOnce(&mut MachineState) -> Result<T>,
{
    let _lock = acquire_lock(lock_path)?;
    let mut state = read_machine_state(state_path)?;
    let out = f(&mut state)?;
    let content = toml::to_string_pretty(&state)
        .map_err(|e| TaskError::Other(format!("serialize state.toml: {e}")))?;
    if let Some(parent) = state_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    storage::toml_store::atomic_write(state_path, &content)?;
    Ok(out)
}

/// Acquires the state lock at `lock_path`, creating its parent dir if needed.
fn acquire_lock(lock_path: &Path) -> Result<storage::FileLock> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| TaskError::Other(format!("create state dir: {e}")))?;
    }
    storage::FileLock::acquire(lock_path)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::core::sync_state::PluginSyncState;

    #[test]
    fn round_trip_all_three_sections() {
        let now = Utc::now();
        let mut m = MachineState::default();
        m.global.active_contexts = vec!["@work".into(), "@home".into()];
        m.global.excluded_contexts = vec!["@noise".into()];
        m.global.resources = HashMap::from([("printer".into(), false)]);
        m.global.active_users = vec!["alice".into()];
        m.plugins.push(Plugin {
            name: "forgejo".into(),
            command: vec!["next-forgejo".into(), "hook".into()],
            tasks: vec![Uuid::new_v4()],
            sync_command: vec!["next-forgejo".into(), "sync".into()],
            default_sync_interval_secs: None,
            sync_interval_secs: Some(3600),
            enabled: true,
        });
        m.sync.last_pull = Some(now);
        m.sync.plugins = BTreeMap::from([(
            "forgejo".to_owned(),
            PluginSyncState { last_sync: Some(now) },
        )]);

        // The toml crate errors if a plain value is emitted after a table; this
        // asserts the flattened global scalars + resources table + plugin array
        // + sync tables all serialise in a valid order.
        let s = toml::to_string_pretty(&m).expect("must serialise without ordering error");
        let back: MachineState = toml::from_str(&s).expect("must parse back");
        assert_eq!(back, m, "all three sections must round-trip losslessly");
    }

    #[test]
    fn global_only_file_loads_unchanged() {
        // A pre-consolidation state.toml with only global fields must load with
        // empty plugin/sync sections.
        let toml = "active_contexts = [\"@work\"]\n\n[resources]\nprinter = false\n";
        let m: MachineState = toml::from_str(toml).unwrap();
        assert_eq!(m.global.active_contexts, vec!["@work"]);
        assert_eq!(m.global.resources.get("printer"), Some(&false));
        assert!(m.plugins.is_empty());
        assert_eq!(m.sync, SyncState::default());
    }

    #[test]
    fn empty_sections_are_not_serialised() {
        let m = MachineState::default();
        let s = toml::to_string_pretty(&m).unwrap();
        assert!(!s.contains("[[plugin]]"), "empty plugin list omitted");
        assert!(!s.contains("[sync]"), "default sync omitted");
    }
}
