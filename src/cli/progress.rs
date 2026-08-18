//! The CLI's progress renderer, and the decision of whether to render at all.
//!
//! Core reports *what* is happening through [`ProgressSink`]
//! ([`crate::core::progress`]); this module is the one place that turns those
//! reports into something on a screen. It draws with `indicatif` — the only
//! module that depends on it, and the reason the dependency is optional and
//! pulled in by the `cli` feature alone.
//!
//! Three things live here:
//!
//! * [`IndicatifSink`] — bars and spinners on **stderr**, because stdout
//!   carries results and a progress bar interleaved with them would corrupt a
//!   pipeline.
//! * [`ProgressGate`] — the pure predicate deciding whether to render, so every
//!   branch of it is testable without a terminal.
//! * [`machine_readable`] — whether the command about to run prints JSON.
//!
//! ## The 100 ms delay
//!
//! A task paints nothing for its first 100 ms, so the many operations that
//! finish instantly never flash a bar. It is a *timer*, not a "reveal on the
//! first update": the case that matters most — a stalled fetch against an
//! unreachable remote — never calls `inc` at all, so a lazily-revealed bar
//! would stay invisible for exactly the wait it exists to explain.
//!
//! Each begun task therefore gets a small thread that waits out the delay and
//! reveals the bar only if the task is still unfinished. The wait is a
//! `Condvar`, so finishing wakes the thread immediately rather than leaving it
//! sleeping, and the reveal and the finish take the same mutex — a task that
//! ends inside the window can never be painted afterwards by its own timer.
//! Tasks are few and sequential (a handful per run), so a thread each is
//! cheaper than a scheduler.

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle, TermLike};

use crate::cli::commands::OutputFormat;
use crate::cli::Command;
use crate::core::progress::{FinishOnce, ProgressSink, ProgressTask};

/// Environment variable that forces progress rendering on.
///
/// It overrides the automatic detection — no terminal on stderr, a
/// machine-readable command, `TERM=dumb` — and, so that a forced run always
/// paints something, it also **skips the 100 ms startup delay**. It does *not*
/// override `--quiet` or `--no-progress`: those are the user saying no, and an
/// environment variable does not outrank them.
///
/// Two uses: keeping progress while deliberately redirecting stderr to a log,
/// and making the rendering path testable without a pty.
pub const FORCE_ENV: &str = "NEXT_FORCE_PROGRESS";

/// How long a task stays invisible after it begins. See the module docs.
pub const REVEAL_DELAY: Duration = Duration::from_millis(100);

/// How often a revealed bar redraws itself without being told to — what makes a
/// spinner spin, and an elapsed counter tick, while the operation is blocked.
const STEADY_TICK: Duration = Duration::from_millis(120);

/// The two templates, deliberately side by side: they are the whole visual
/// design, and keeping them apart is how the spinner and the bar drift into
/// looking like two different programs.
///
/// Fields common to both:
///
/// * `{prefix}` — the label `begin` was called with ("Indexing tasks"),
/// * `{wide_msg}` — the detail line from `set_message` (a file or segment
///   path), given the slack so the fields after it stay right-aligned,
/// * `{elapsed}` — time since `begin`.
///
/// The determinate one adds `{bar}` (fixed 24 columns — `wide_msg` already
/// claims the flexible width, and indicatif allows only one such field),
/// `{pos}/{len}`, and `{eta}`, which reads well here because every determinate
/// phase is a loop over a known count of similar items.
const SPINNER_TEMPLATE: &str = "{spinner} {prefix} {wide_msg} ({elapsed})";
const BAR_TEMPLATE: &str = "{prefix} [{bar:24}] {pos}/{len} {wide_msg} ({elapsed}, eta {eta})";

/// Whether `command` prints machine-readable output on stdout.
///
/// An **exhaustive match with no wildcard arm**, so a command added later has
/// to make this choice deliberately rather than inherit "human" by default.
/// The answer is "did the invocation select JSON", which is exactly
/// [`OutputFormat::resolve`]'s question — the `--format`/`--json` precedence is
/// not re-derived here.
///
/// Progress goes to stderr and so cannot corrupt a JSON document on stdout;
/// the reason to stay quiet anyway is that JSON output means a script is
/// reading, and a script's stderr is usually a log or a `2>&1` merge.
pub fn machine_readable(command: &Command) -> bool {
    /// `--json` / `--format json` folded into one answer. Commands with no
    /// `--format` pass `None`, which is the same question with one fewer way
    /// to spell it.
    fn json(format: Option<OutputFormat>, json_alias: bool) -> bool {
        OutputFormat::resolve(format, json_alias).is_json()
    }

    use crate::cli::commands;
    match command {
        // Never reach the dispatch that consults this (handled before the
        // repository is opened), but the match is exhaustive on purpose.
        Command::Init(_) | Command::Tutorial(_) | Command::Config(_) => false,

        Command::List(a) => json(a.format, a.json),
        Command::Next(a) => json(a.format, a.json),
        Command::Show(a) => json(None, a.json),
        Command::Tree(a) => json(a.format, a.json),
        Command::Forecast(a) => json(a.format, a.json),
        Command::Add(a) => json(None, a.json),
        Command::Edit(a) => json(None, a.json),
        Command::Done(a) => json(None, a.json),
        Command::Cancel(a) => json(None, a.json),
        Command::Start(a) => json(None, a.json),
        Command::Stop(a) => json(None, a.json),
        Command::Move(a) => json(None, a.json),
        Command::Maintenance(a) => match &a.command {
            commands::maintenance::Command::RebuildCache(a) => json(None, a.json),
            commands::maintenance::Command::Archive(a) => json(None, a.json),
        },
        Command::Tag(a) => match &a.subcommand {
            Some(commands::tag::TagSubcommand::Show(a)) => json(None, a.json),
            Some(commands::tag::TagSubcommand::Data(a)) => match &a.subcommand {
                commands::tag::DataSubcommand::List(a) => json(None, a.json),
                commands::tag::DataSubcommand::Set(_)
                | commands::tag::DataSubcommand::Get(_)
                | commands::tag::DataSubcommand::Unset(_) => false,
            },
            // The metadata writers and the state commands print a one-line
            // confirmation for a person; `next tag` with no subcommand prints
            // the tag table.
            None
            | Some(
                commands::tag::TagSubcommand::Rename(_)
                | commands::tag::TagSubcommand::Describe(_)
                | commands::tag::TagSubcommand::ClearDescription(_)
                | commands::tag::TagSubcommand::SetUrl(_)
                | commands::tag::TagSubcommand::ClearUrl(_)
                | commands::tag::TagSubcommand::SetPriority(_)
                | commands::tag::TagSubcommand::ClearPriority(_)
                | commands::tag::TagSubcommand::SetNoTimeUrgency(_)
                | commands::tag::TagSubcommand::ClearNoTimeUrgency(_)
                | commands::tag::TagSubcommand::Require(_)
                | commands::tag::TagSubcommand::Exclude(_)
                | commands::tag::TagSubcommand::Accept(_)
                | commands::tag::TagSubcommand::ClearState(_),
            ) => false,
        },

        // No `--json` / `--format` at all: prose, a confirmation line, or a
        // single data value.
        Command::Open(_)
        | Command::Data(_)
        | Command::Delete(_)
        | Command::User(_)
        | Command::Sync(_)
        | Command::Plugin(_) => false,
    }
}

/// The inputs to "should this run draw progress?", so the decision is one pure
/// function rather than a condition spread over `main`.
///
/// The order the fields are resolved in is the order they win in: the two
/// explicit flags first, then the forced escape hatch, then the automatic
/// detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressGate {
    /// `std::io::IsTerminal` on **stderr** — where progress would be drawn.
    /// stdout is irrelevant: `next list --json > file` on a terminal still has
    /// a terminal to draw a spinner on.
    pub stderr_is_terminal: bool,
    /// `--quiet`.
    pub quiet: bool,
    /// `--no-progress`.
    pub no_progress: bool,
    /// [`machine_readable`] for the command about to run.
    pub machine_readable: bool,
    /// `TERM=dumb` — a terminal that cannot redraw a line.
    pub term_is_dumb: bool,
    /// [`FORCE_ENV`] is set to `1`.
    pub forced: bool,
}

impl ProgressGate {
    /// Reads the gate from the environment for `command`.
    pub fn detect(command: &Command, quiet: bool, no_progress: bool) -> Self {
        Self {
            stderr_is_terminal: io::stderr().is_terminal(),
            quiet,
            no_progress,
            machine_readable: machine_readable(command),
            term_is_dumb: std::env::var("TERM").is_ok_and(|t| t == "dumb"),
            forced: std::env::var(FORCE_ENV).is_ok_and(|v| v == "1"),
        }
    }

    /// The decision.
    pub fn should_render(self) -> bool {
        if self.quiet || self.no_progress {
            return false;
        }
        if self.forced {
            return true;
        }
        self.stderr_is_terminal && !self.machine_readable && !self.term_is_dumb
    }
}

/// Counts of the two events tests need to observe. Kept unconditionally (two
/// words) so the reveal path has no `cfg` in it; only the accessors are
/// test-only.
#[derive(Debug, Default)]
struct Counters {
    revealed: AtomicUsize,
    finished: AtomicUsize,
}

/// Draws progress with `indicatif`.
///
/// One [`MultiProgress`] owns the screen area, so concurrent tasks (core does
/// not currently begin two at once, but the trait allows it) stack instead of
/// overwriting each other.
pub struct IndicatifSink {
    multi: MultiProgress,
    /// How long a task stays invisible; zero under [`FORCE_ENV`].
    delay: Duration,
    counters: Arc<Counters>,
}

impl IndicatifSink {
    /// The sink the CLI installs: draws on stderr, `forced` meaning
    /// [`FORCE_ENV`] was set and therefore no startup delay.
    pub fn for_stderr(forced: bool) -> Self {
        let stderr = ProgressDrawTarget::stderr();
        // indicatif suppresses its own stderr target when stderr is not a
        // terminal (or `TERM` is dumb, or `NO_COLOR` is set) — sensible as a
        // default, but we only get here when the gate already decided to
        // render, which off a terminal means the user forced it. So fall back
        // to writing the frames ourselves rather than silently drawing
        // nothing.
        let target = if stderr.is_hidden() {
            ProgressDrawTarget::term_like_with_hz(Box::new(PlainStderr::new()), 20)
        } else {
            stderr
        };
        Self::with_target(target, if forced { Duration::ZERO } else { REVEAL_DELAY })
    }

    /// A sink drawing to an explicit target with an explicit startup delay —
    /// the seam the unit tests use to exercise the timer without a terminal.
    pub fn with_target(target: ProgressDrawTarget, delay: Duration) -> Self {
        Self {
            multi: MultiProgress::with_draw_target(target),
            delay,
            counters: Arc::new(Counters::default()),
        }
    }

    /// Runs `f` with the bars temporarily off the screen, for output that has
    /// to appear while an operation is still running.
    ///
    /// Today's informational notes are all printed *after* the operation that
    /// could have a bar up has returned (and a finished bar is cleared), so
    /// this is insurance rather than a fix — but it is the cheap kind: with no
    /// bar live it is a lock and a call.
    ///
    /// `tracing` output is deliberately **not** routed through here. The
    /// default level is `error`, i.e. a line printed at most once just before
    /// the process exits, and a custom `MakeWriter` plumbed into the subscriber
    /// to protect that is more machinery than the collision is worth.
    pub fn suspend<R>(&self, f: impl FnOnce() -> R) -> R {
        self.multi.suspend(f)
    }

    /// How many tasks were revealed (i.e. actually painted).
    #[cfg(test)]
    fn revealed(&self) -> usize {
        self.counters.revealed.load(Ordering::SeqCst)
    }

    /// How many finishes were reported — the contract is one per begun task.
    #[cfg(test)]
    fn finished(&self) -> usize {
        self.counters.finished.load(Ordering::SeqCst)
    }
}

impl ProgressSink for IndicatifSink {
    fn begin(&self, label: &str, total: Option<u64>) -> Box<dyn ProgressTask> {
        // Hidden at birth: the bar exists and accumulates position, but has
        // nowhere to draw until the timer joins it to the MultiProgress.
        let bar = ProgressBar::with_draw_target(total, ProgressDrawTarget::hidden());
        bar.set_style(style_for(total.is_some()));
        bar.set_prefix(label.to_owned());

        let shared = Arc::new(Shared {
            multi: self.multi.clone(),
            bar,
            state: Mutex::new(Reveal::Hidden),
            wake: Condvar::new(),
            counters: Arc::clone(&self.counters),
        });

        if self.delay.is_zero() {
            // Forced mode. Revealing here rather than from a thread that would
            // race the operation is what makes "forced progress paints" a fact
            // instead of a likelihood.
            shared.reveal();
        } else {
            let timer = Arc::clone(&shared);
            let delay = self.delay;
            std::thread::spawn(move || timer.reveal_after(delay));
        }

        Box::new(IndicatifTask {
            shared,
            finished: FinishOnce::default(),
        })
    }
}

/// Whether a task has been painted yet, and whether it is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reveal {
    /// Begun, inside the startup window, nothing on screen.
    Hidden,
    /// Painted; the screen has to be cleaned up on finish.
    Shown,
    /// Finished. The timer thread must not paint anything from here on.
    Done,
}

/// The half of a task the timer thread shares with the operation.
struct Shared {
    multi: MultiProgress,
    bar: ProgressBar,
    state: Mutex<Reveal>,
    wake: Condvar,
    counters: Arc<Counters>,
}

impl Shared {
    /// The state lock, unpoisoned: a panicking operation is already being
    /// reported, and refusing to clean up its bar on the way out would only
    /// leave the terminal broken as well.
    fn lock(&self) -> MutexGuard<'_, Reveal> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Waits out the startup window, then paints — unless the task ended
    /// first, in which case this returns without touching the terminal. The
    /// `Condvar` means a task that finishes in 2 ms also ends its thread in
    /// 2 ms rather than leaving it asleep for the rest of the window.
    fn reveal_after(&self, delay: Duration) {
        let state = self.lock();
        let (state, _timed_out) = self
            .wake
            .wait_timeout_while(state, delay, |s| *s == Reveal::Hidden)
            .unwrap_or_else(|e| e.into_inner());
        if *state == Reveal::Hidden {
            self.paint(state);
        }
    }

    /// Paints from a caller that holds no lock (the forced path).
    fn reveal(&self) {
        let state = self.lock();
        if *state == Reveal::Hidden {
            self.paint(state);
        }
    }

    /// Joins the bar to the `MultiProgress` — which is what gives it somewhere
    /// to draw — and starts it animating on its own, so a spinner keeps moving
    /// while the operation is blocked on the network. Called with the state
    /// lock held, so a concurrent `finish` cannot slip in between the two.
    fn paint(&self, mut state: MutexGuard<'_, Reveal>) {
        self.multi.add(self.bar.clone());
        self.bar.enable_steady_tick(STEADY_TICK);
        // Force the first frame out now: a phase that never calls `inc` (the
        // history-walk spinner) would otherwise show nothing until the steady
        // tick came round.
        self.bar.tick();
        *state = Reveal::Shown;
        self.counters.revealed.fetch_add(1, Ordering::SeqCst);
    }

    /// Ends the task, clearing whatever it drew.
    ///
    /// Always `finish_and_clear`: progress is transient and results are not,
    /// so stderr is left exactly as it was found. No summary is kept, because
    /// no core call site passes one — every `finish` in `core` passes `None` —
    /// and a formatting path nothing reaches is a path nothing keeps correct.
    fn finish(&self) {
        let mut state = self.lock();
        if *state == Reveal::Shown {
            self.bar.finish_and_clear();
            self.multi.remove(&self.bar);
        }
        *state = Reveal::Done;
        drop(state);
        // Wakes the timer thread so it exits now instead of at the end of the
        // window.
        self.wake.notify_all();
        self.counters.finished.fetch_add(1, Ordering::SeqCst);
    }
}

/// The handle handed to the operation. Owning it is what keeps the task alive:
/// dropping it finishes, whether the operation got there or a `?` did.
struct IndicatifTask {
    shared: Arc<Shared>,
    finished: FinishOnce,
}

impl ProgressTask for IndicatifTask {
    fn inc(&self, delta: u64) {
        self.shared.bar.inc(delta);
    }

    fn set_total(&self, total: u64) {
        // A spinner that learns its size becomes a bar, mid-flight: the fetch
        // phases do exactly this when git2's first callback arrives.
        self.shared.bar.set_length(total);
        self.shared.bar.set_style(style_for(true));
    }

    fn set_message(&self, msg: &str) {
        self.shared.bar.set_message(msg.to_owned());
    }

    fn finish(&self, _msg: Option<&str>) {
        if self.finished.claim() {
            self.shared.finish();
        }
    }
}

impl Drop for IndicatifTask {
    fn drop(&mut self) {
        self.finish(None);
    }
}

/// The style for a determinate bar or an indeterminate spinner.
///
/// The templates are constants above; `expect` is unreachable for them, which
/// `templates_are_valid` pins.
fn style_for(determinate: bool) -> ProgressStyle {
    if determinate {
        ProgressStyle::with_template(BAR_TEMPLATE)
            .expect("BAR_TEMPLATE is a valid indicatif template")
            .progress_chars("=>-")
    } else {
        ProgressStyle::with_template(SPINNER_TEMPLATE)
            .expect("SPINNER_TEMPLATE is a valid indicatif template")
    }
}

/// A draw target that writes frames to stderr whether or not stderr is a
/// terminal — the fallback described in [`IndicatifSink::for_stderr`].
///
/// The cursor movements are the standard ANSI sequences, the same ones
/// `console` would emit: a redirected stderr that is later `tail -f`'d or
/// replayed then renders as intended, and the alternative — appending a fresh
/// line per redraw — turns a long fetch into thousands of lines.
#[derive(Debug)]
struct PlainStderr {
    width: u16,
}

impl PlainStderr {
    fn new() -> Self {
        // Nothing to measure when the far end is a pipe. `COLUMNS` is what a
        // caller who cares can set; 80 is the conventional answer otherwise.
        let width = std::env::var("COLUMNS")
            .ok()
            .and_then(|c| c.parse().ok())
            .unwrap_or(80);
        Self { width }
    }

    fn write(&self, s: &str) -> io::Result<()> {
        io::stderr().lock().write_all(s.as_bytes())
    }

    fn move_cursor(&self, n: usize, direction: char) -> io::Result<()> {
        if n == 0 {
            return Ok(());
        }
        self.write(&format!("\x1b[{n}{direction}"))
    }
}

impl TermLike for PlainStderr {
    fn width(&self) -> u16 {
        self.width
    }

    fn move_cursor_up(&self, n: usize) -> io::Result<()> {
        self.move_cursor(n, 'A')
    }

    fn move_cursor_down(&self, n: usize) -> io::Result<()> {
        self.move_cursor(n, 'B')
    }

    fn move_cursor_right(&self, n: usize) -> io::Result<()> {
        self.move_cursor(n, 'C')
    }

    fn move_cursor_left(&self, n: usize) -> io::Result<()> {
        self.move_cursor(n, 'D')
    }

    fn write_line(&self, s: &str) -> io::Result<()> {
        self.write(s)?;
        self.write("\n")
    }

    fn write_str(&self, s: &str) -> io::Result<()> {
        self.write(s)
    }

    fn clear_line(&self) -> io::Result<()> {
        self.write("\r\x1b[2K")
    }

    fn flush(&self) -> io::Result<()> {
        io::stderr().lock().flush()
    }
}

#[cfg(test)]
mod tests {
    use std::thread::sleep;

    use clap::Parser as _;

    use super::*;
    use crate::cli::Cli;

    /// The command `argv` selects, as `machine_readable` sees it.
    fn is_machine_readable(argv: &[&str]) -> bool {
        let cli = Cli::try_parse_from(argv).expect("argv parses");
        machine_readable(&cli.command.expect("argv names a command"))
    }

    #[test]
    fn templates_are_valid() {
        style_for(true);
        style_for(false);
    }

    // -- machine_readable -------------------------------------------------

    #[test]
    fn json_flag_selects_machine_output() {
        for argv in [
            vec!["next", "list", "--json"],
            vec!["next", "next", "--json"],
            vec!["next", "show", "abc", "--json"],
            vec!["next", "tree", "--json"],
            vec!["next", "forecast", "--json"],
            vec!["next", "add", "A title", "--json"],
            vec!["next", "edit", "abc", "--json"],
            vec!["next", "done", "abc", "--json"],
            vec!["next", "cancel", "abc", "--json"],
            vec!["next", "start", "abc", "--json"],
            vec!["next", "stop", "abc", "--json"],
            vec!["next", "move", "abc", "--parent", "def", "--json"],
            vec!["next", "maintenance", "rebuild-cache", "--json"],
            vec!["next", "maintenance", "archive", "--json"],
            vec!["next", "tag", "show", "@work", "--json"],
            vec!["next", "tag", "data", "list", "@work", "--json"],
        ] {
            assert!(is_machine_readable(&argv), "{argv:?}");
        }
    }

    #[test]
    fn format_json_selects_machine_output() {
        for argv in [
            vec!["next", "list", "--format", "json"],
            vec!["next", "next", "--format", "json"],
            vec!["next", "tree", "--format", "json"],
            vec!["next", "forecast", "--format", "json"],
        ] {
            assert!(is_machine_readable(&argv), "{argv:?}");
        }
    }

    #[test]
    fn format_table_is_human_output() {
        for argv in [
            vec!["next", "list", "--format", "table"],
            vec!["next", "tree", "--format", "table"],
            vec!["next", "next", "--format", "table"],
            vec!["next", "forecast", "--format", "table"],
        ] {
            assert!(!is_machine_readable(&argv), "{argv:?}");
        }
        assert!(is_machine_readable(&["next", "list", "--format", "json"]));
    }

    #[test]
    fn human_output_is_not_machine_readable() {
        for argv in [
            vec!["next", "list"],
            vec!["next", "next"],
            vec!["next", "show", "abc"],
            vec!["next", "tree"],
            vec!["next", "forecast"],
            vec!["next", "add", "A title"],
            vec!["next", "done", "abc"],
            vec!["next", "maintenance", "rebuild-cache"],
            vec!["next", "maintenance", "archive"],
            vec!["next", "tag"],
            vec!["next", "tag", "show", "@work"],
            vec!["next", "tag", "rename", "@a", "@b"],
            vec!["next", "tag", "data", "list", "@work"],
            vec!["next", "tag", "data", "get", "@work", "k"],
            vec!["next", "sync"],
            vec!["next", "delete", "abc"],
            vec!["next", "open", "abc"],
            vec!["next", "data", "get", "abc", "k"],
            vec!["next", "user", "list"],
            vec!["next", "plugin", "list"],
            vec!["next", "init"],
            vec!["next", "tutorial"],
        ] {
            assert!(!is_machine_readable(&argv), "{argv:?}");
        }
    }

    // -- the gate ---------------------------------------------------------

    /// An interactive human run: everything says yes.
    fn interactive() -> ProgressGate {
        ProgressGate {
            stderr_is_terminal: true,
            quiet: false,
            no_progress: false,
            machine_readable: false,
            term_is_dumb: false,
            forced: false,
        }
    }

    #[test]
    fn interactive_human_run_renders() {
        assert!(interactive().should_render());
    }

    #[test]
    fn each_automatic_condition_suppresses_rendering() {
        for (name, gate) in [
            (
                "no terminal",
                ProgressGate {
                    stderr_is_terminal: false,
                    ..interactive()
                },
            ),
            (
                "machine-readable command",
                ProgressGate {
                    machine_readable: true,
                    ..interactive()
                },
            ),
            (
                "TERM=dumb",
                ProgressGate {
                    term_is_dumb: true,
                    ..interactive()
                },
            ),
        ] {
            assert!(!gate.should_render(), "{name}");
        }
    }

    #[test]
    fn explicit_flags_suppress_rendering() {
        assert!(!ProgressGate {
            quiet: true,
            ..interactive()
        }
        .should_render());
        assert!(!ProgressGate {
            no_progress: true,
            ..interactive()
        }
        .should_render());
    }

    #[test]
    fn forcing_overrides_the_automatic_conditions() {
        let gate = ProgressGate {
            stderr_is_terminal: false,
            machine_readable: true,
            term_is_dumb: true,
            forced: true,
            ..interactive()
        };
        assert!(gate.should_render(), "the escape hatch is the whole point");
    }

    #[test]
    fn forcing_does_not_override_the_explicit_flags() {
        assert!(!ProgressGate {
            quiet: true,
            forced: true,
            ..interactive()
        }
        .should_render());
        assert!(!ProgressGate {
            no_progress: true,
            forced: true,
            ..interactive()
        }
        .should_render());
    }

    // -- the sink ---------------------------------------------------------

    /// A sink that renders nowhere, so the tests observe the *decisions*
    /// (reveal, finish) rather than terminal bytes.
    fn test_sink(delay: Duration) -> IndicatifSink {
        IndicatifSink::with_target(ProgressDrawTarget::hidden(), delay)
    }

    #[test]
    fn a_task_finished_inside_the_window_never_paints() {
        let sink = test_sink(Duration::from_millis(100));
        {
            let task = sink.begin("Indexing tasks", Some(2));
            task.inc(2);
            task.finish(None);
        }
        // Well past the window: if the timer thread were going to paint, it
        // would have by now.
        sleep(Duration::from_millis(300));
        assert_eq!(sink.revealed(), 0, "a fast operation stays invisible");
        assert_eq!(sink.finished(), 1);
    }

    #[test]
    fn a_task_outliving_the_window_is_painted() {
        let sink = test_sink(Duration::from_millis(20));
        let task = sink.begin("Pulling", None);
        sleep(Duration::from_millis(250));
        assert_eq!(
            sink.revealed(),
            1,
            "a stalled task must appear without ever calling inc"
        );
        drop(task);
        assert_eq!(sink.finished(), 1);
    }

    #[test]
    fn a_forced_sink_paints_immediately() {
        let sink = test_sink(Duration::ZERO);
        let task = sink.begin("Indexing tasks", Some(1));
        assert_eq!(sink.revealed(), 1, "no delay means painted by `begin`");
        drop(task);
    }

    #[test]
    fn finish_then_drop_finishes_once() {
        let sink = test_sink(Duration::ZERO);
        {
            let task = sink.begin("Updating cache", Some(1));
            task.finish(None);
            task.finish(Some("ignored"));
        }
        assert_eq!(sink.finished(), 1, "drop must not double-report");
    }

    #[test]
    fn dropping_without_finishing_finishes_once() {
        let sink = test_sink(Duration::ZERO);
        drop(sink.begin("Archiving tasks", Some(1)));
        assert_eq!(sink.finished(), 1, "the `?` path finishes too");
    }

    #[test]
    fn a_spinner_that_learns_its_total_keeps_reporting() {
        let sink = test_sink(Duration::ZERO);
        let task = sink.begin("Pulling", None);
        task.set_total(10);
        task.set_message("receiving objects");
        task.inc(10);
        task.finish(None);
        assert_eq!(sink.finished(), 1);
    }
}
