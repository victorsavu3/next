use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Mutex, OwnedSemaphorePermit, Semaphore};

use crate::store::PullResult;
use crate::AppContext;

#[derive(Debug)]
enum DeferredSyncMsg {
    /// Start (or reset) the deferred sync timer.
    Schedule,
    /// Cancel any pending deferred timer (explicit sync was just run externally).
    Cancel,
}

/// Cloneable handle for scheduling deferred syncs and acquiring the sync lock.
#[derive(Clone)]
pub struct SyncScheduler {
    tx: mpsc::Sender<DeferredSyncMsg>,
    /// At most one sync (explicit or deferred) runs at a time.
    /// Background tasks use `try_acquire`; explicit callers fail-fast if busy.
    semaphore: Arc<Semaphore>,
}

impl SyncScheduler {
    pub fn schedule_deferred(&self) {
        let _ = self.tx.try_send(DeferredSyncMsg::Schedule);
    }

    pub fn cancel(&self) {
        let _ = self.tx.try_send(DeferredSyncMsg::Cancel);
    }

    /// Try to acquire the sync lock without blocking.
    /// Returns `None` if a sync is already in progress.
    pub fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.semaphore).try_acquire_owned().ok()
    }

    #[cfg(test)]
    pub fn new_for_test() -> Self {
        let (tx, _rx) = mpsc::channel(1);
        Self {
            tx,
            semaphore: Arc::new(Semaphore::new(1)),
        }
    }
}

/// Performs a pull+push using the VCS backend inside `ctx`.
/// Errors are returned but not fatal — callers decide whether to surface them.
pub fn do_sync(ctx: &mut AppContext) -> anyhow::Result<()> {
    match ctx.vcs.pull()? {
        PullResult::Clean => {
            ctx.log.info("sync", "pull: clean");
        }
        PullResult::Conflicts(paths) => {
            let names: Vec<_> = paths.iter().map(|p| p.display().to_string()).collect();
            ctx.log
                .error("sync", &format!("pull conflicts: {}", names.join(", ")));
            anyhow::bail!("merge conflicts: {}", names.join(", "));
        }
    }
    ctx.vcs.push()?;
    ctx.log.info("sync", "push: ok");
    Ok(())
}

/// Spawns the background task that manages deferred syncs.
/// Returns a `SyncScheduler` handle that callers use to schedule or cancel.
pub fn spawn_deferred_sync(ctx: Arc<Mutex<AppContext>>, delay: Duration) -> SyncScheduler {
    let (tx, rx) = mpsc::channel(32);
    let semaphore = Arc::new(Semaphore::new(1));
    tokio::spawn(deferred_sync_task(rx, ctx, delay, Arc::clone(&semaphore)));
    SyncScheduler { tx, semaphore }
}

async fn deferred_sync_task(
    mut rx: mpsc::Receiver<DeferredSyncMsg>,
    ctx: Arc<Mutex<AppContext>>,
    delay: Duration,
    semaphore: Arc<Semaphore>,
) {
    loop {
        let msg = match rx.recv().await {
            Some(m) => m,
            None => return, // sender dropped — server shutting down
        };

        match msg {
            DeferredSyncMsg::Cancel => continue,
            DeferredSyncMsg::Schedule => {
                let sleep = tokio::time::sleep(delay);
                tokio::pin!(sleep);

                loop {
                    tokio::select! {
                        _ = &mut sleep => {
                            run_sync_background(&ctx, &semaphore).await;
                            break;
                        }
                        msg = rx.recv() => match msg {
                            None => return,
                            Some(DeferredSyncMsg::Schedule) => {
                                sleep.as_mut().reset(tokio::time::Instant::now() + delay);
                            }
                            Some(DeferredSyncMsg::Cancel) => break,
                        }
                    }
                }
            }
        }
    }
}

/// Run sync in a background thread, acquiring the semaphore first.
/// If the semaphore is already held (explicit sync in progress), skip silently.
async fn run_sync_background(ctx: &Arc<Mutex<AppContext>>, semaphore: &Arc<Semaphore>) {
    let permit = match Arc::clone(semaphore).try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            ctx.lock()
                .await
                .log
                .info("sync", "background sync skipped: explicit sync already in progress");
            return;
        }
    };
    let ctx_for_blocking = ctx.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut ctx = ctx_for_blocking.blocking_lock();
        let r = do_sync(&mut ctx);
        drop(permit);
        r
    })
    .await;

    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            ctx.lock()
                .await
                .log
                .error("sync", &format!("background sync error: {e}"));
        }
        Err(e) => {
            ctx.lock()
                .await
                .log
                .error("sync", &format!("background sync task panic: {e}"));
        }
    }
}

/// Spawns a periodic sync task that runs every `interval`.
/// Shares the scheduler's semaphore so periodic syncs don't race with explicit ones.
pub fn spawn_periodic_sync(
    ctx: Arc<Mutex<AppContext>>,
    interval: Duration,
    scheduler: &SyncScheduler,
) {
    let semaphore = Arc::clone(&scheduler.semaphore);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await; // skip the immediate first tick
        loop {
            ticker.tick().await;
            run_sync_background(&ctx, &semaphore).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_size_check() {
        let _ = std::mem::size_of::<SyncScheduler>();
    }
}
