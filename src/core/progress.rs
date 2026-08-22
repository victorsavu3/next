//! Progress reporting — the abstraction, with no renderer attached.
//!
//! Several core operations are slow enough to look like a hang: the cache
//! rebuild on a fresh clone, the archive pass, a sync, a tag rename across
//! every tier. They need to say what they are doing, but *core* must not know
//! how it is shown — an interactive CLI draws a bar, the TUI forwards events
//! to its event loop, and `next-mcp` / `next-forgejo` / a library consumer must
//! stay completely silent because their stdout is a protocol.
//!
//! So core reports through a [`ProgressSink`] it is handed, and the default is
//! [`NoProgress`]: a binary that installs nothing is silent *by construction*,
//! not by remembering to pass a `quiet` flag down every call. The sink is
//! carried on [`TaskRepository`](crate::core::TaskRepository) (and pushed into
//! the store and the VCS backend by
//! [`with_progress`](crate::core::TaskRepository::with_progress)) rather than
//! threaded through every signature, because the operations that need it sit
//! several layers below the code that knows whether a terminal is attached.
//!
//! Both traits are object-safe and the sink is `Send + Sync`: a renderer may
//! live on another thread and receive events over a channel.

use std::sync::atomic::{AtomicBool, Ordering};

/// A destination for progress reports — a bar, a TUI channel, or nothing.
///
/// Implementations must tolerate concurrent use (`Send + Sync`) and must never
/// fail: a report is not something an operation can meaningfully handle, so
/// there is no `Result` anywhere in this module.
pub trait ProgressSink: Send + Sync {
    /// Begins a unit of work.
    ///
    /// `total = None` means the size is not known yet — a spinner — and
    /// `Some(n)` a determinate bar. `label` names the operation for the user
    /// ("Rebuilding cache", "Archiving"). The returned task reports into this
    /// sink until it is finished or dropped.
    fn begin(&self, label: &str, total: Option<u64>) -> Box<dyn ProgressTask>;

    /// Whether this sink discards everything, i.e. is [`NoProgress`].
    ///
    /// Lets a caller skip work it would only do in order to report — counting
    /// a corpus up front just to have a total, for instance — when nothing
    /// will render the answer. Never required for correctness: an operation
    /// that ignores this stays correct, only slightly slower under the no-op
    /// sink.
    fn is_noop(&self) -> bool {
        false
    }
}

/// One unit of work in progress, obtained from [`ProgressSink::begin`].
///
/// Every method takes `&self` so a task can be shared with a worker while the
/// operation keeps ownership, and none of them return anything: reporting is
/// fire-and-forget.
///
/// **Implementations must finish on `Drop`.** Operations propagate errors with
/// `?`, and an early return that leaves a bar on screen forever is exactly the
/// kind of cleanup no call site remembers. Since a task is used as
/// `Box<dyn ProgressTask>`, the `Drop` has to belong to the *implementing
/// type* (the vtable runs it when the box is dropped) — the trait cannot
/// provide it. The `Drop` impl must report at most one finish in total: an
/// explicit [`finish`](Self::finish) followed by the drop is one finish, not
/// two, which in practice means guarding on an internal flag such as an
/// [`AtomicBool`].
pub trait ProgressTask: Send {
    /// Advances the completed count by `delta`.
    fn inc(&self, delta: u64);

    /// Sets (or replaces) the expected total, turning a spinner into a bar
    /// once the count becomes known — the common shape for an operation that
    /// has to scan before it can say how much there is.
    fn set_total(&self, total: u64);

    /// Replaces the detail line shown next to the label (the current file, the
    /// current segment). The label itself does not change.
    fn set_message(&self, msg: &str);

    /// Marks the work complete, optionally replacing the label with a summary
    /// ("Rebuilt 12 480 tasks"). Calling this more than once, or calling it and
    /// then dropping the task, reports exactly one finish.
    fn finish(&self, msg: Option<&str>);
}

/// The default sink: every report is discarded.
///
/// This is what a [`TaskRepository`](crate::core::TaskRepository) carries until
/// someone installs something else, so the MCP server, the Forgejo plugin, and
/// any library consumer are silent without opting out of anything.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn begin(&self, _label: &str, _total: Option<u64>) -> Box<dyn ProgressTask> {
        Box::new(NoTask)
    }

    fn is_noop(&self) -> bool {
        true
    }
}

/// The task handed out by [`NoProgress`]. No state, so no `Drop` impl: the
/// "finish on drop" contract is satisfied vacuously by a finish that does
/// nothing.
struct NoTask;

impl ProgressTask for NoTask {
    fn inc(&self, _delta: u64) {}
    fn set_total(&self, _total: u64) {}
    fn set_message(&self, _msg: &str) {}
    fn finish(&self, _msg: Option<&str>) {}
}

/// A [`ProgressTask`] flag that makes the "at most one finish" rule a
/// three-line implementation rather than a thing each renderer re-derives.
///
/// `claim` returns `true` for the first caller only, so both the explicit
/// `finish` and the `Drop` impl can call it and exactly one wins.
#[derive(Debug, Default)]
pub struct FinishOnce(AtomicBool);

impl FinishOnce {
    /// Returns `true` if this call is the one that should report the finish.
    pub fn claim(&self) -> bool {
        !self.0.swap(true, Ordering::SeqCst)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! A recording sink for unit tests.
    //!
    //! Instrumented operations live all over the crate (cache rebuild,
    //! archiver, tag rename, git backend) and each is tested from its own
    //! module, so this is `pub(crate)` rather than local to one `mod tests`.

    use std::sync::{Arc, Mutex};

    use super::{FinishOnce, ProgressSink, ProgressTask};

    /// One recorded call, carrying the label of the task it belongs to so
    /// several concurrent tasks can be told apart.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum ProgressEvent {
        Begin { label: String, total: Option<u64> },
        Inc { label: String, delta: u64 },
        SetTotal { label: String, total: u64 },
        Message { label: String, msg: String },
        Finish { label: String, msg: Option<String> },
    }

    impl ProgressEvent {
        /// The label of the task this event belongs to.
        pub(crate) fn label(&self) -> &str {
            match self {
                Self::Begin { label, .. }
                | Self::Inc { label, .. }
                | Self::SetTotal { label, .. }
                | Self::Message { label, .. }
                | Self::Finish { label, .. } => label,
            }
        }
    }

    /// A sink that records everything it is told, for assertions.
    #[derive(Debug, Default)]
    pub(crate) struct RecordingSink {
        events: Arc<Mutex<Vec<ProgressEvent>>>,
    }

    impl RecordingSink {
        /// A sink with no recorded events. Wrap it in an `Arc` to install it:
        /// `repo.with_progress(Arc::new(RecordingSink::new()))`.
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// Every event, in the order it was reported.
        pub(crate) fn events(&self) -> Vec<ProgressEvent> {
            self.events.lock().expect("recording sink poisoned").clone()
        }

        /// The labels of the tasks that were begun, in order — the cheapest
        /// assertion for "this operation reported at all, under this name".
        pub(crate) fn labels(&self) -> Vec<String> {
            self.events()
                .iter()
                .filter_map(|e| match e {
                    ProgressEvent::Begin { label, .. } => Some(label.clone()),
                    _ => None,
                })
                .collect()
        }

        /// The events for `label`, folded into the shape a test usually wants
        /// to assert on. `None` when no task by that label was begun.
        pub(crate) fn task(&self, label: &str) -> Option<RecordedTask> {
            let events = self.events();
            let mut found = false;
            let mut rec = RecordedTask::default();
            for event in events.iter().filter(|e| e.label() == label) {
                match event {
                    ProgressEvent::Begin { total, .. } => {
                        found = true;
                        rec.total = *total;
                    }
                    ProgressEvent::Inc { delta, .. } => rec.progressed += delta,
                    ProgressEvent::SetTotal { total, .. } => rec.total = Some(*total),
                    ProgressEvent::Message { msg, .. } => rec.messages.push(msg.clone()),
                    ProgressEvent::Finish { msg, .. } => {
                        rec.finishes += 1;
                        if rec.finish_message.is_none() {
                            rec.finish_message.clone_from(msg);
                        }
                    }
                }
            }
            found.then_some(rec)
        }
    }

    /// The folded view of one task's events: what it was begun with, how far
    /// it got, and whether it ended.
    #[derive(Debug, Default, PartialEq, Eq)]
    pub(crate) struct RecordedTask {
        /// The latest total the task knew — from `begin`, or a later
        /// `set_total`.
        pub(crate) total: Option<u64>,
        /// The sum of every `inc`.
        pub(crate) progressed: u64,
        /// Every `set_message`, in order.
        pub(crate) messages: Vec<String>,
        /// How many finishes were reported. The contract says exactly one.
        pub(crate) finishes: usize,
        /// The message of the first finish.
        pub(crate) finish_message: Option<String>,
    }

    impl RecordedTask {
        /// "Began with a known total, advanced exactly that far, finished
        /// once" — the assertion instrumented loops are meant to satisfy.
        pub(crate) fn completed(&self) -> bool {
            self.finishes == 1 && self.total == Some(self.progressed)
        }
    }

    impl ProgressSink for RecordingSink {
        fn begin(&self, label: &str, total: Option<u64>) -> Box<dyn ProgressTask> {
            let task = RecordingTask {
                label: label.to_owned(),
                events: Arc::clone(&self.events),
                finished: FinishOnce::default(),
            };
            task.record(ProgressEvent::Begin {
                label: label.to_owned(),
                total,
            });
            Box::new(task)
        }
    }

    struct RecordingTask {
        label: String,
        events: Arc<Mutex<Vec<ProgressEvent>>>,
        finished: FinishOnce,
    }

    impl RecordingTask {
        fn record(&self, event: ProgressEvent) {
            self.events
                .lock()
                .expect("recording sink poisoned")
                .push(event);
        }
    }

    impl ProgressTask for RecordingTask {
        fn inc(&self, delta: u64) {
            self.record(ProgressEvent::Inc {
                label: self.label.clone(),
                delta,
            });
        }

        fn set_total(&self, total: u64) {
            self.record(ProgressEvent::SetTotal {
                label: self.label.clone(),
                total,
            });
        }

        fn set_message(&self, msg: &str) {
            self.record(ProgressEvent::Message {
                label: self.label.clone(),
                msg: msg.to_owned(),
            });
        }

        fn finish(&self, msg: Option<&str>) {
            if self.finished.claim() {
                self.record(ProgressEvent::Finish {
                    label: self.label.clone(),
                    msg: msg.map(str::to_owned),
                });
            }
        }
    }

    /// The `Drop` half of the finish contract — see [`ProgressTask`].
    impl Drop for RecordingTask {
        fn drop(&mut self) {
            self.finish(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::testing::{ProgressEvent, RecordingSink};
    use super::{NoProgress, ProgressSink};

    #[test]
    fn no_progress_discards_everything() {
        let sink = NoProgress;
        assert!(sink.is_noop());
        let task = sink.begin("rebuild", Some(3));
        task.inc(1);
        task.set_total(9);
        task.set_message("tasks/foo.toml");
        task.finish(Some("done"));
        drop(task);
    }

    #[test]
    fn no_progress_is_usable_as_a_shared_sink() {
        let sink: Arc<dyn ProgressSink> = Arc::new(NoProgress);
        let handle = Arc::clone(&sink);
        std::thread::spawn(move || handle.begin("worker", None).inc(1))
            .join()
            .expect("worker panicked");
        assert!(sink.is_noop());
    }

    #[test]
    fn recording_sink_records_a_full_determinate_run() {
        let sink = RecordingSink::new();
        assert!(!sink.is_noop());
        {
            let task = sink.begin("archive", Some(2));
            task.set_message("2024/01-000.toml");
            task.inc(1);
            task.inc(1);
            task.finish(Some("archived 2"));
        }
        assert_eq!(sink.labels(), vec!["archive".to_owned()]);
        let rec = sink.task("archive").expect("task was begun");
        assert!(rec.completed(), "{rec:?}");
        assert_eq!(rec.total, Some(2));
        assert_eq!(rec.progressed, 2);
        assert_eq!(rec.messages, vec!["2024/01-000.toml".to_owned()]);
        assert_eq!(rec.finish_message.as_deref(), Some("archived 2"));
        assert!(sink.task("nothing-by-this-name").is_none());
    }

    #[test]
    fn recording_sink_keeps_events_in_order_and_per_label() {
        let sink = RecordingSink::new();
        let outer = sink.begin("outer", None);
        let inner = sink.begin("inner", Some(1));
        inner.inc(1);
        drop(inner);
        outer.set_total(1);
        outer.inc(1);
        drop(outer);

        let events = sink.events();
        let labels: Vec<&str> = events.iter().map(ProgressEvent::label).collect();
        assert_eq!(
            labels,
            ["outer", "inner", "inner", "inner", "outer", "outer", "outer"],
            "events interleave in call order, each tagged with its task"
        );
        assert!(matches!(
            events.first(),
            Some(ProgressEvent::Begin { label, total: None }) if label == "outer"
        ));
        // A spinner that learns its total behaves like a bar from then on.
        assert!(sink.task("outer").expect("begun").completed());
        assert!(sink.task("inner").expect("begun").completed());
    }

    #[test]
    fn dropping_without_finishing_reports_one_finish() {
        let sink = RecordingSink::new();
        drop(sink.begin("rebuild", None));
        let rec = sink.task("rebuild").expect("task was begun");
        assert_eq!(rec.finishes, 1, "drop must finish: {rec:?}");
        assert_eq!(rec.finish_message, None);
    }

    #[test]
    fn explicit_finish_then_drop_reports_one_finish() {
        let sink = RecordingSink::new();
        {
            let task = sink.begin("rebuild", None);
            task.finish(Some("done"));
        }
        let rec = sink.task("rebuild").expect("task was begun");
        assert_eq!(rec.finishes, 1, "drop must not double-report: {rec:?}");
        assert_eq!(rec.finish_message.as_deref(), Some("done"));
    }

    #[test]
    fn repeated_explicit_finishes_report_once() {
        let sink = RecordingSink::new();
        let task = sink.begin("sync", None);
        task.finish(Some("first"));
        task.finish(Some("second"));
        drop(task);
        let rec = sink.task("sync").expect("task was begun");
        assert_eq!(rec.finishes, 1);
        assert_eq!(rec.finish_message.as_deref(), Some("first"));
    }
}
