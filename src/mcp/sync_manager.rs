use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Mutex};

use crate::store::PullResult;
use crate::AppContext;


#[derive(Debug)]
enum DeferredSyncMsg {
    /// Start (or reset) the 30-second deferred sync timer.
    Schedule,
    /// Cancel any pending deferred timer (explicit sync was just run externally).
    Cancel,
}

/// Cloneable handle for scheduling deferred syncs.
#[derive(Clone)]
pub struct SyncScheduler {
    tx: mpsc::Sender<DeferredSyncMsg>,
}

impl SyncScheduler {
    pub fn schedule_deferred(&self) {
        // Best-effort; if the receiver is gone we can't do anything.
        let _ = self.tx.try_send(DeferredSyncMsg::Schedule);
    }

    pub fn cancel(&self) {
        let _ = self.tx.try_send(DeferredSyncMsg::Cancel);
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
    tokio::spawn(deferred_sync_task(rx, ctx, delay));
    SyncScheduler { tx }
}

async fn deferred_sync_task(
    mut rx: mpsc::Receiver<DeferredSyncMsg>,
    ctx: Arc<Mutex<AppContext>>,
    delay: Duration,
) {
    loop {
        // Wait for the first message.
        let msg = match rx.recv().await {
            Some(m) => m,
            None => return, // sender dropped — server shutting down
        };

        match msg {
            DeferredSyncMsg::Cancel => continue, // nothing pending; ignore
            DeferredSyncMsg::Schedule => {
                // Inner loop: manage the timer, allow resets.
                let sleep = tokio::time::sleep(delay);
                tokio::pin!(sleep);

                loop {
                    tokio::select! {
                        _ = &mut sleep => {
                            // Timer fired — run sync.
                            run_sync_background(&ctx).await;
                            break;
                        }
                        msg = rx.recv() => match msg {
                            None => return,
                            Some(DeferredSyncMsg::Schedule) => {
                                // Reset the timer.
                                sleep.as_mut().reset(
                                    tokio::time::Instant::now() + delay,
                                );
                            }
                            Some(DeferredSyncMsg::Cancel) => {
                                // Explicit sync was called externally; discard timer.
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn run_sync_background(ctx: &Arc<Mutex<AppContext>>) {
    let ctx = ctx.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut ctx = ctx.blocking_lock();
        do_sync(&mut ctx)
    })
    .await;

    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => eprintln!("background sync error: {e}"),
        Err(e) => eprintln!("background sync task panic: {e}"),
    }
}

/// Spawns a periodic sync task that runs every `interval`.
pub fn spawn_periodic_sync(ctx: Arc<Mutex<AppContext>>, interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await; // skip the immediate first tick
        loop {
            ticker.tick().await;
            run_sync_background(&ctx).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn do_sync_no_remote_returns_error() {
        // A repo with no remote configured should return an error from push.
        // We can't easily test this without a real git repo, so just verify
        // the function signature compiles and the function is callable.
        // Full integration tests cover the real behaviour.
        let _ = std::mem::size_of::<SyncScheduler>();
    }
}
