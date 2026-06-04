//! Re-entrant, cross-process advisory file lock.
//!
//! `next`, `next-mcp`, and (in future) external plugin processes all mutate the
//! same data in parallel.  [`FileLock`] is a generic advisory `flock(2)` over a
//! given lock file, used for two *separate* locks: the repository lock
//! (`<repo>/.next.lock`, guarding tasks/tags and all git operations) and the
//! machine-local state lock (`.state.toml.lock`).  Each lock file is an
//! independent instance — they never block one another.
//!
//! Within one process the lock must be:
//!
//! * **mutually exclusive across threads** — two threads (two MCP requests, or
//!   the test-suite's process simulations) must not both believe they hold it;
//! * **re-entrant within a thread** — a transaction acquires the lock once at
//!   the top of a read-modify-write(-commit) and then calls `save_task` /
//!   `commit` / `save_state`, which acquire it again; the nested calls must not
//!   deadlock.
//!
//! `flock` alone cannot provide re-entrancy: locks are associated with the open
//! file description, so a second `flock` from the same thread on a freshly
//! opened descriptor would block forever waiting on the first.  We therefore
//! layer an in-process gate (owner thread + recursion depth) over the OS lock
//! and share one gate per lock-file path through a process-global registry.
//!
//! Lock ordering: when both are taken, the repo lock is always acquired
//! *before* the state lock, never the other way round, so the two cannot
//! deadlock.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::ThreadId;

use fs4::FileExt;

use crate::core::error::{TaskError, Result};

/// In-process gate guarding one lock file's OS lock.
struct Gate {
    state: Mutex<GateState>,
    cond: Condvar,
}

struct GateState {
    /// Thread currently holding the lock, if any.
    owner: Option<ThreadId>,
    /// Recursion depth of the owning thread (0 when unheld).
    depth: usize,
    /// The held OS lock file; `Some` exactly while `depth > 0`.  Dropping it
    /// releases the `flock`.
    file: Option<File>,
}

/// Process-global map from canonical lock-file path to its shared gate.
fn registry() -> &'static Mutex<HashMap<PathBuf, Arc<Gate>>> {
    static REG: OnceLock<Mutex<HashMap<PathBuf, Arc<Gate>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Canonicalises the lock path so different spellings of the same lock file
/// share one gate.  The lock file itself need not exist yet, so we canonicalise
/// the parent directory and re-attach the file name.
fn canonical_key(lock_path: &Path) -> PathBuf {
    match (lock_path.parent(), lock_path.file_name()) {
        (Some(parent), Some(name)) => parent
            .canonicalize()
            .unwrap_or_else(|_| parent.to_path_buf())
            .join(name),
        _ => lock_path.to_path_buf(),
    }
}

fn gate_for(lock_path: &Path) -> Arc<Gate> {
    let key = canonical_key(lock_path);
    let mut reg = registry().lock().expect("lock registry poisoned");
    Arc::clone(reg.entry(key).or_insert_with(|| {
        Arc::new(Gate {
            state: Mutex::new(GateState {
                owner: None,
                depth: 0,
                file: None,
            }),
            cond: Condvar::new(),
        })
    }))
}

fn open_and_flock(lock_path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|e| TaskError::Other(format!("open lock file {}: {e}", lock_path.display())))?;
    file.lock_exclusive()
        .map_err(|e| TaskError::Other(format!("acquire lock {}: {e}", lock_path.display())))?;
    Ok(file)
}

/// An acquired advisory file lock.
///
/// Holds the lock until dropped.  Re-entrant acquisitions on the same thread
/// share one underlying OS lock and release it only when the outermost guard is
/// dropped.
pub struct FileLock {
    gate: Arc<Gate>,
}

impl FileLock {
    /// Acquires the exclusive lock on `lock_path`, blocking until it is free.
    ///
    /// Blocks while another *thread* in this process or another *process* holds
    /// it; returns immediately (bumping the recursion depth) when the current
    /// thread already holds it.
    pub fn acquire(lock_path: &Path) -> Result<FileLock> {
        let gate = gate_for(lock_path);
        let me = std::thread::current().id();

        let mut state = gate.state.lock().expect("gate poisoned");
        loop {
            match state.owner {
                // Already ours on this thread — re-entrant acquisition.
                Some(owner) if owner == me => {
                    state.depth += 1;
                    return Ok(FileLock {
                        gate: Arc::clone(&gate),
                    });
                }
                // Held by another thread in this process — wait for release.
                Some(_) => {
                    state = gate.cond.wait(state).expect("gate poisoned");
                }
                // Free: claim ownership, then take the OS lock.  We must not
                // hold the in-process mutex while blocking on `flock`, so we
                // mark ownership first and drop the guard during the (possibly
                // blocking) acquire.  Other threads that arrive meanwhile see
                // us as the owner and wait on the condvar.
                None => {
                    state.owner = Some(me);
                    state.depth = 1;
                    drop(state);

                    match open_and_flock(lock_path) {
                        Ok(file) => {
                            gate.state.lock().expect("gate poisoned").file = Some(file);
                            return Ok(FileLock {
                                gate: Arc::clone(&gate),
                            });
                        }
                        Err(e) => {
                            // Roll back ownership and wake the next waiter.
                            let mut s = gate.state.lock().expect("gate poisoned");
                            s.owner = None;
                            s.depth = 0;
                            gate.cond.notify_one();
                            return Err(e);
                        }
                    }
                }
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let mut state = self.gate.state.lock().expect("gate poisoned");
        state.depth -= 1;
        if state.depth == 0 {
            // Dropping the file releases the flock for other processes; clearing
            // the owner and notifying lets another in-process thread proceed.
            state.file = None;
            state.owner = None;
            self.gate.cond.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    };
    use std::time::Duration;

    use super::*;

    fn lock_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".next.lock");
        (dir, path)
    }

    #[test]
    fn reentrant_same_thread_does_not_deadlock() {
        let (_dir, path) = lock_path();
        let outer = FileLock::acquire(&path).unwrap();
        // Nested acquisition on the same thread must return immediately.
        let inner = FileLock::acquire(&path).unwrap();
        let innermost = FileLock::acquire(&path).unwrap();
        drop(innermost);
        drop(inner);
        drop(outer);
        // Lock is now free; a fresh acquire succeeds.
        let _again = FileLock::acquire(&path).unwrap();
    }

    #[test]
    fn different_threads_are_mutually_exclusive() {
        let (_dir, path) = lock_path();
        let counter = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        const THREADS: usize = 8;
        let barrier = Arc::new(Barrier::new(THREADS));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let path = path.clone();
                let counter = Arc::clone(&counter);
                let max_seen = Arc::clone(&max_seen);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..50 {
                        let _lock = FileLock::acquire(&path).unwrap();
                        // Inside the critical section the count must never
                        // exceed 1 if the lock is truly exclusive.
                        let now = counter.fetch_add(1, Ordering::SeqCst) + 1;
                        max_seen.fetch_max(now, Ordering::SeqCst);
                        std::thread::yield_now();
                        counter.fetch_sub(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(
            max_seen.load(Ordering::SeqCst),
            1,
            "at most one thread may hold the lock at a time"
        );
    }

    #[test]
    fn lock_releases_after_outermost_drop() {
        let (_dir, path) = lock_path();
        let held = Arc::new(AtomicUsize::new(0));

        let outer = FileLock::acquire(&path).unwrap();
        let held2 = Arc::clone(&held);
        let path2 = path.clone();
        let waiter = std::thread::spawn(move || {
            let _lock = FileLock::acquire(&path2).unwrap();
            held2.store(1, Ordering::SeqCst);
        });

        // Give the waiter a chance to block on the held lock.
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(held.load(Ordering::SeqCst), 0, "waiter must block while held");

        drop(outer);
        waiter.join().unwrap();
        assert_eq!(held.load(Ordering::SeqCst), 1, "waiter proceeds after release");
    }
}
