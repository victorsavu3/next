use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Mutex, OwnedSemaphorePermit, Semaphore};

use crate::core::store::PullResult;
use crate::TaskRepository;

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
pub fn do_sync(ctx: &mut TaskRepository) -> anyhow::Result<()> {
    match ctx.vcs.pull()? {
        PullResult::Clean => {
            tracing::info!(cmd = "sync", "pull: clean");
        }
        PullResult::Conflicts(paths) => {
            let names: Vec<_> = paths.iter().map(|p| p.display().to_string()).collect();
            tracing::error!(cmd = "sync", "pull conflicts: {}", names.join(", "));
            anyhow::bail!("merge conflicts: {}", names.join(", "));
        }
    }
    let new_head = ctx.vcs.head_hash()?;
    ctx.store.after_pull(&new_head)?;
    ctx.vcs.push()?;
    tracing::info!(cmd = "sync", "push: ok");
    Ok(())
}

/// Spawns the background task that manages deferred syncs.
/// Returns a `SyncScheduler` handle that callers use to schedule or cancel.
pub fn spawn_deferred_sync(ctx: Arc<Mutex<TaskRepository>>, delay: Duration) -> SyncScheduler {
    let (tx, rx) = mpsc::channel(32);
    let semaphore = Arc::new(Semaphore::new(1));
    tokio::spawn(deferred_sync_task(rx, ctx, delay, Arc::clone(&semaphore)));
    SyncScheduler { tx, semaphore }
}

async fn deferred_sync_task(
    mut rx: mpsc::Receiver<DeferredSyncMsg>,
    ctx: Arc<Mutex<TaskRepository>>,
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
async fn run_sync_background(ctx: &Arc<Mutex<TaskRepository>>, semaphore: &Arc<Semaphore>) {
    let permit = match Arc::clone(semaphore).try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            tracing::info!(cmd = "sync", "background sync skipped: explicit sync already in progress");
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
            tracing::error!(cmd = "sync", "background sync error: {e}");
        }
        Err(e) => {
            tracing::error!(cmd = "sync", "background sync task panic: {e}");
        }
    }
}

/// Spawns a periodic sync task that runs every `interval`.
/// Shares the scheduler's semaphore so periodic syncs don't race with explicit ones.
pub fn spawn_periodic_sync(
    ctx: Arc<Mutex<TaskRepository>>,
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
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use super::*;
    use crate::core::error::{TaskError, Result};
    use crate::core::store::{PullResult, Store, VcsBackend};
    use crate::TaskRepository;
    use crate::core::domain::state::GlobalState;
    use crate::core::domain::tag::TagMeta;
    use crate::core::domain::task::Task;
    use uuid::Uuid;

    struct FakeStore;

    impl Store for FakeStore {
        fn get_task(&self, _id: Uuid) -> Result<Task> { unimplemented!() }
        fn get_task_by_slug(&self, _slug: &str) -> Result<Option<Task>> { unimplemented!() }
        fn find_tasks_by_prefix(&self, _prefix: &str) -> Result<Vec<Task>> { unimplemented!() }
        fn list_tasks(&self) -> Result<Vec<Task>> { unimplemented!() }
        fn save_task(&mut self, _task: &Task) -> Result<()> { unimplemented!() }
        fn delete_task(&mut self, _id: Uuid) -> Result<()> { unimplemented!() }
        fn get_state(&self) -> Result<GlobalState> { unimplemented!() }
        fn save_state(&mut self, _state: &GlobalState) -> Result<()> { unimplemented!() }
        fn get_tag_meta(&self, _tag: &str) -> Result<Option<TagMeta>> { unimplemented!() }
        fn set_tag_meta(&mut self, _tag: &str, _meta: TagMeta) -> Result<()> { unimplemented!() }
        fn delete_tag_meta(&mut self, _tag: &str) -> Result<()> { unimplemented!() }
        fn list_tag_metas(&self) -> Result<HashMap<String, TagMeta>> { unimplemented!() }
    }

    #[derive(Clone)]
    struct FakeVcs {
        sync_count: Arc<AtomicUsize>,
        fail: bool,
    }

    impl FakeVcs {
        fn new() -> Self {
            Self { sync_count: Arc::new(AtomicUsize::new(0)), fail: false }
        }

        fn new_failing() -> Self {
            Self { sync_count: Arc::new(AtomicUsize::new(0)), fail: true }
        }
    }

    impl VcsBackend for FakeVcs {
        fn commit(&self, _paths: &[std::path::PathBuf], _message: &str) -> Result<()> { Ok(()) }
        fn head_hash(&self) -> Result<String> { Ok("0000000000000000000000000000000000000000".to_owned()) }

        fn pull(&self) -> Result<PullResult> {
            if self.fail {
                return Err(TaskError::Other("fake pull error".to_owned()));
            }
            self.sync_count.fetch_add(1, Ordering::SeqCst);
            Ok(PullResult::Clean)
        }

        fn push(&self) -> Result<()> {
            if self.fail {
                return Err(TaskError::Other("fake push error".to_owned()));
            }
            Ok(())
        }
    }

    fn make_ctx(vcs: FakeVcs) -> TaskRepository {
        TaskRepository::with_parts(Box::new(FakeStore), Box::new(vcs), std::path::PathBuf::from("/tmp"))
    }

    #[test]
    fn scheduler_size_check() {
        let _ = std::mem::size_of::<SyncScheduler>();
    }

    #[test]
    fn semaphore_blocks_concurrent_sync() {
        let scheduler = SyncScheduler::new_for_test();
        let _permit = scheduler.try_acquire().expect("first acquire must succeed");
        assert!(scheduler.try_acquire().is_none(), "second acquire must fail while first is held");
    }

    #[test]
    fn semaphore_released_after_permit_drop() {
        let scheduler = SyncScheduler::new_for_test();
        {
            let _permit = scheduler.try_acquire().expect("first acquire must succeed");
        }
        assert!(scheduler.try_acquire().is_some(), "must succeed after permit is dropped");
    }

    #[tokio::test]
    async fn cancel_prevents_deferred_sync() {
        tokio::time::pause();

        let vcs = FakeVcs::new();
        let counter = Arc::clone(&vcs.sync_count);
        let ctx = Arc::new(Mutex::new(make_ctx(vcs)));

        let delay = Duration::from_secs(5);
        let scheduler = spawn_deferred_sync(ctx, delay);

        scheduler.schedule_deferred();
        // yield so the background task can receive the Schedule message
        tokio::task::yield_now().await;

        scheduler.cancel();
        // yield so the background task can receive the Cancel message
        tokio::task::yield_now().await;

        tokio::time::advance(delay * 2).await;
        tokio::task::yield_now().await;

        assert_eq!(counter.load(Ordering::SeqCst), 0, "cancelled sync must not run");
    }

    #[tokio::test]
    async fn deferred_sync_fires_after_delay() {
        tokio::time::pause();

        let vcs = FakeVcs::new();
        let counter = Arc::clone(&vcs.sync_count);
        let ctx = Arc::new(Mutex::new(make_ctx(vcs)));

        let delay = Duration::from_secs(5);
        let scheduler = spawn_deferred_sync(ctx, delay);

        scheduler.schedule_deferred();
        tokio::task::yield_now().await;

        tokio::time::advance(delay + Duration::from_millis(1)).await;
        // spawn_blocking runs on a dedicated thread pool; give it time to finish
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(counter.load(Ordering::SeqCst), 1, "sync must run after delay");
    }

    #[tokio::test]
    async fn reschedule_resets_timer() {
        tokio::time::pause();

        let vcs = FakeVcs::new();
        let counter = Arc::clone(&vcs.sync_count);
        let ctx = Arc::new(Mutex::new(make_ctx(vcs)));

        let delay = Duration::from_secs(5);
        let scheduler = spawn_deferred_sync(ctx, delay);

        scheduler.schedule_deferred();
        tokio::task::yield_now().await;

        // advance most of the way, then reschedule — timer must reset
        tokio::time::advance(Duration::from_secs(4)).await;
        tokio::task::yield_now().await;

        scheduler.schedule_deferred();
        tokio::task::yield_now().await;

        // advancing by the original delay should NOT fire (timer was reset)
        tokio::time::advance(Duration::from_secs(2)).await;
        tokio::task::yield_now().await;

        assert_eq!(counter.load(Ordering::SeqCst), 0, "sync must not fire before reset delay elapses");

        // now advance past the full reset delay
        tokio::time::advance(Duration::from_secs(4)).await;
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(counter.load(Ordering::SeqCst), 1, "sync must fire after reset delay elapses");
    }

    #[tokio::test]
    async fn semaphore_held_skips_background_sync() {
        tokio::time::pause();

        let vcs = FakeVcs::new();
        let counter = Arc::clone(&vcs.sync_count);
        let ctx = Arc::new(Mutex::new(make_ctx(vcs)));

        let delay = Duration::from_secs(1);
        let scheduler = spawn_deferred_sync(ctx, delay);

        // hold the semaphore to simulate an explicit sync in progress
        let _permit = scheduler.try_acquire().expect("must acquire");

        scheduler.schedule_deferred();
        tokio::task::yield_now().await;

        tokio::time::advance(delay + Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(counter.load(Ordering::SeqCst), 0, "background sync must be skipped when semaphore is held");
    }

    #[tokio::test]
    async fn do_sync_calls_pull_and_push() {
        let vcs = FakeVcs::new();
        let counter = Arc::clone(&vcs.sync_count);
        let mut ctx = make_ctx(vcs);
        do_sync(&mut ctx).unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn do_sync_propagates_pull_error() {
        let vcs = FakeVcs::new_failing();
        let mut ctx = make_ctx(vcs);
        assert!(do_sync(&mut ctx).is_err());
    }
}
