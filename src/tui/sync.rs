//! Background sync plumbing for the TUI.
//!
//! A network sync (git pull/push) must not block the UI thread. So the manual
//! sync trigger spawns a `std::thread` that opens a *second* repository handle
//! (via [`bootstrap::open_repository`](crate::core::bootstrap::open_repository))
//! on the same root — locking is cross-handle/cross-process safe, the same way
//! the CLI and MCP server coexist — runs [`core::sync::sync`](crate::core::sync),
//! and sends the `anyhow::Result<SyncOutcome>` back over an mpsc channel.
//!
//! The event loop drains that channel each tick and feeds the result into
//! [`SyncMsg`], which the [`App`](super::app::App) handler turns into a status
//! line + a reload request. The handler logic lives here (pure) so it can be
//! unit-tested without spawning a real sync against a remote.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

use crate::core::{bootstrap, sync, SyncOutcome};
use crate::Config;

/// The message a sync worker sends back when it finishes: the raw sync result.
pub type SyncMsg = anyhow::Result<SyncOutcome>;

/// The outcome of folding a [`SyncMsg`] into UI state: a status line to show
/// and whether the task list should be reloaded (to pick up pulled changes).
///
/// Separated from [`App`](super::app::App) so the message handling is a pure
/// function over the worker result, unit-testable without a real network sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncResult {
    /// The footer status message describing the outcome.
    pub status: String,
    /// Whether the caller should `reload()` (true on a clean sync — a pull may
    /// have brought in new tasks; false on conflict/error).
    pub reload: bool,
}

impl SyncResult {
    /// Maps a finished worker message to a status + reload decision, mirroring
    /// the CLI's reporting:
    /// * `Clean` → "sync: up to date" + reload.
    /// * `Conflicts(paths)` → the CLI's "Merge conflicts — resolve manually: …"
    ///   message, no reload (the working tree holds conflict markers).
    /// * `Err(e)` → "sync error: …", no reload.
    pub fn from_msg(msg: SyncMsg) -> Self {
        match msg {
            Ok(SyncOutcome::Clean) => SyncResult {
                status: "sync: up to date / pushed".to_owned(),
                reload: true,
            },
            Ok(SyncOutcome::Conflicts(paths)) => {
                let names: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                SyncResult {
                    status: format!("Merge conflicts — resolve manually: {}", names.join(", ")),
                    reload: false,
                }
            }
            Err(e) => SyncResult {
                status: format!("sync error: {e}"),
                reload: false,
            },
        }
    }
}

/// Spawns a background sync worker.
///
/// Opens a second repository handle on `root` (applying `config.sync`), runs a
/// full push+pull sync, and sends the result over `tx`. Any error from opening
/// the handle is forwarded as the worker result, so the UI always gets exactly
/// one message back per spawn. The thread is detached; the receiver lives on the
/// [`App`](super::app::App) and is drained each tick.
pub fn spawn(root: PathBuf, config: Config, tx: Sender<SyncMsg>) {
    thread::spawn(move || {
        let result = run(root, &config);
        // The receiver may be gone if the app quit mid-sync; ignore send errors.
        let _ = tx.send(result);
    });
}

/// The worker body: open a fresh handle and run the sync. Factored out so the
/// spawn closure stays trivial.
fn run(root: PathBuf, config: &Config) -> SyncMsg {
    let mut repo = bootstrap::open_repository(root, config)?;
    sync::sync(&mut repo, false, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_requests_reload() {
        let r = SyncResult::from_msg(Ok(SyncOutcome::Clean));
        assert!(r.reload);
        assert!(r.status.contains("up to date"));
    }

    #[test]
    fn conflicts_report_paths_no_reload() {
        let paths = vec![PathBuf::from("tasks/a.toml"), PathBuf::from("tasks/b.toml")];
        let r = SyncResult::from_msg(Ok(SyncOutcome::Conflicts(paths)));
        assert!(!r.reload);
        assert!(r.status.starts_with("Merge conflicts — resolve manually:"));
        assert!(r.status.contains("tasks/a.toml"));
        assert!(r.status.contains("tasks/b.toml"));
    }

    #[test]
    fn error_reports_message_no_reload() {
        let r = SyncResult::from_msg(Err(anyhow::anyhow!("boom")));
        assert!(!r.reload);
        assert!(r.status.contains("sync error"));
        assert!(r.status.contains("boom"));
    }
}
