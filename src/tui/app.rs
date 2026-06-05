//! Central application state and the Elm-style update loop for the TUI.
//!
//! [`App`] holds everything the UI needs to render and mutate. Input flows as
//! key event → [`Action`] (via [`App::handle_key`]) → state change (via
//! [`App::update`]). Keeping the key mapping and the state transitions in
//! separate steps lets later tasks add modes and actions without reshaping the
//! event loop.

use chrono::NaiveDate;

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::core::domain::filter;
use crate::core::domain::task::Task;
use crate::core::scoring::{self, ScoredTask};
use crate::core::{FilterArgs, TaskRepository};
use crate::Config;

use super::config::ConfigSource;

/// Which interaction mode the UI is in.
///
/// [`Mode::Normal`] browses the list; [`Mode::Filter`] edits the live filter
/// buffer. Later tasks (edit modal, confirm popup) add variants here, and the
/// event loop / drawing code branch on the active mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Browsing the task list; keys drive selection and reload.
    #[default]
    Normal,
    /// Editing the filter buffer in the filter bar.
    Filter,
}

/// A discrete state transition produced by [`App::handle_key`] and applied by
/// [`App::update`]. Extend this enum as new behaviour is added; the event loop
/// does not need to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Quit the application.
    Quit,
    /// Move the selection to the next task.
    SelectNext,
    /// Move the selection to the previous task.
    SelectPrev,
    /// Jump to the first task.
    SelectFirst,
    /// Jump to the last task.
    SelectLast,
    /// Re-run the load+score pipeline.
    Reload,
    /// Open the filter bar, pre-filled with the current tokens.
    OpenFilter,
    /// While in [`Mode::Filter`]: feed a raw input event to the buffer.
    FilterInput(KeyEvent),
    /// While in [`Mode::Filter`]: commit the buffer into the active filter and reload.
    CommitFilter,
    /// While in [`Mode::Filter`]: cancel editing and restore the previous tokens.
    CancelFilter,
    /// Toggle the `--all` filter flag and reload.
    ToggleAll,
    /// Toggle the `--future` filter flag and reload.
    ToggleFuture,
    /// Toggle the `--all-users` filter flag and reload.
    ToggleAllUsers,
}

/// The central application state.
pub struct App {
    config: Config,
    repo: TaskRepository,
    source: ConfigSource,
    today: NaiveDate,

    /// The currently visible, scored + sorted task list.
    tasks: Vec<ScoredTask>,
    /// Index of the selected task in `tasks`. Meaningless when `tasks` is empty.
    selected: usize,

    /// Active filter tokens (`+tag`, `-tag`, `parent:`, `context:`, `user:`),
    /// threaded into [`FilterArgs`] on every [`App::reload`].
    filter_tokens: Vec<String>,
    /// `--future`: include tasks scheduled in the future.
    filter_future: bool,
    /// `--all`: bypass implicit filtering (show done/blocked/etc.).
    filter_all: bool,
    /// `--all-users`: ignore the active user filter.
    filter_all_users: bool,

    /// The editable filter buffer, live only while in [`Mode::Filter`].
    filter_input: Input,

    /// The current interaction mode.
    mode: Mode,
    /// A transient status message shown in the footer.
    status: Option<String>,
    /// Set when the user asks to quit; the event loop checks this.
    should_quit: bool,
}

impl App {
    /// Builds a fresh app over the given repository. Call [`App::reload`] before
    /// the first draw to populate the task list.
    pub fn new(config: Config, repo: TaskRepository, source: ConfigSource, today: NaiveDate) -> Self {
        Self {
            config,
            repo,
            source,
            today,
            tasks: Vec::new(),
            selected: 0,
            filter_tokens: Vec::new(),
            filter_future: false,
            filter_all: false,
            filter_all_users: false,
            filter_input: Input::default(),
            mode: Mode::Normal,
            status: None,
            should_quit: false,
        }
    }

    // ── Accessors used by the drawing layer ───────────────────────────────

    /// The repository root, for the title bar.
    pub fn repo_root(&self) -> &std::path::Path {
        &self.repo.repo_root
    }

    /// Which config file the settings came from, for the title bar.
    pub fn source(&self) -> ConfigSource {
        self.source
    }

    /// "Today", used by the list for overdue / due-today styling.
    pub fn today(&self) -> NaiveDate {
        self.today
    }

    /// The current interaction mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The scored, sorted, currently-visible tasks.
    pub fn tasks(&self) -> &[ScoredTask] {
        &self.tasks
    }

    /// The selected index. Only meaningful when [`App::tasks`] is non-empty.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The transient footer status message, if any.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Whether the event loop should exit.
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// The active filter tokens, for rendering the filter summary.
    pub fn filter_tokens(&self) -> &[String] {
        &self.filter_tokens
    }

    /// Whether the `--all` toggle is active.
    pub fn filter_all(&self) -> bool {
        self.filter_all
    }

    /// Whether the `--future` toggle is active.
    pub fn filter_future(&self) -> bool {
        self.filter_future
    }

    /// Whether the `--all-users` toggle is active.
    pub fn filter_all_users(&self) -> bool {
        self.filter_all_users
    }

    /// The live filter buffer, for rendering the filter bar in [`Mode::Filter`].
    pub fn filter_input(&self) -> &Input {
        &self.filter_input
    }

    /// The task currently under the selection, or `None` when the list is empty.
    pub fn selected_task(&self) -> Option<&Task> {
        self.tasks.get(self.selected).map(|s| &s.task)
    }

    // ── Data loading ──────────────────────────────────────────────────────

    /// Re-runs the load + score pipeline (the same one `next list` uses) with
    /// the active filter tokens + flags, and clamps the selection to the new
    /// list length.
    ///
    /// A bad filter token surfaces as an `Err` carrying the parse message; the
    /// caller ([`App::update`]) shows it in the status line and leaves the
    /// previous list untouched.
    pub fn reload(&mut self) -> anyhow::Result<()> {
        // Build the filter first: an invalid token must NOT clear the list, so
        // we fail before touching `self.tasks`.
        let mut fa = FilterArgs::parse(self.filter_tokens.clone());
        fa.future = self.filter_future;
        fa.all = self.filter_all;
        fa.all_users = self.filter_all_users;
        let filter_set = fa.to_filter_set()?;

        let store = self.repo.store();
        let state = store.get_state()?;
        let all_tasks = store.list_tasks()?;
        let tag_metas = store.list_tag_metas()?;

        let filtered = filter::apply(all_tasks.clone(), &filter_set, &state, self.today);
        let scored = scoring::score_and_sort(
            filtered,
            &all_tasks,
            self.today,
            &self.repo.scoring,
            &tag_metas,
        );

        let limit = self.config.list_limit;
        self.tasks = scored;
        if let Some(n) = limit {
            self.tasks.truncate(n);
        }
        self.clamp_selection();
        Ok(())
    }

    // ── Event handling ────────────────────────────────────────────────────

    /// Maps a key press to an [`Action`], based on the current [`Mode`].
    /// Returns `None` when the key is not bound.
    pub fn handle_key(&self, key: KeyEvent) -> Option<Action> {
        match self.mode {
            Mode::Normal => Self::normal_key(key),
            Mode::Filter => Self::filter_key(key),
        }
    }

    fn normal_key(key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Quit),
            KeyCode::Char('j') | KeyCode::Down => Some(Action::SelectNext),
            KeyCode::Char('k') | KeyCode::Up => Some(Action::SelectPrev),
            KeyCode::Char('g') | KeyCode::Home => Some(Action::SelectFirst),
            KeyCode::Char('G') | KeyCode::End => Some(Action::SelectLast),
            KeyCode::Char('r') => Some(Action::Reload),
            KeyCode::Char('/') => Some(Action::OpenFilter),
            KeyCode::Char('A') => Some(Action::ToggleAll),
            KeyCode::Char('F') => Some(Action::ToggleFuture),
            KeyCode::Char('U') => Some(Action::ToggleAllUsers),
            _ => None,
        }
    }

    fn filter_key(key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Enter => Some(Action::CommitFilter),
            KeyCode::Esc => Some(Action::CancelFilter),
            // Everything else is editing input (chars, backspace, arrows, …).
            _ => Some(Action::FilterInput(key)),
        }
    }

    /// Applies an [`Action`] to the state.
    pub fn update(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::SelectNext => self.select_next(),
            Action::SelectPrev => self.select_prev(),
            Action::SelectFirst => self.select_first(),
            Action::SelectLast => self.select_last(),
            Action::Reload => self.reload_with_status("reloaded"),
            Action::OpenFilter => self.open_filter(),
            Action::FilterInput(key) => {
                self.filter_input.handle_event(&Event::Key(key));
            }
            Action::CommitFilter => self.commit_filter(),
            Action::CancelFilter => self.cancel_filter(),
            Action::ToggleAll => {
                self.filter_all = !self.filter_all;
                self.reload_with_status("reloaded");
            }
            Action::ToggleFuture => {
                self.filter_future = !self.filter_future;
                self.reload_with_status("reloaded");
            }
            Action::ToggleAllUsers => {
                self.filter_all_users = !self.filter_all_users;
                self.reload_with_status("reloaded");
            }
        }
    }

    /// Runs [`App::reload`], reporting either `ok_msg` or the error into the
    /// status line. On error the previous list is preserved (see [`App::reload`]).
    fn reload_with_status(&mut self, ok_msg: &str) {
        match self.reload() {
            Ok(()) => self.status = Some(ok_msg.to_owned()),
            Err(e) => self.status = Some(format!("filter error: {e}")),
        }
    }

    /// Enters [`Mode::Filter`], pre-filling the buffer with the current tokens.
    fn open_filter(&mut self) {
        self.filter_input = Input::new(self.filter_tokens.join(" "));
        self.mode = Mode::Filter;
        self.status = None;
    }

    /// Parses the buffer into `filter_tokens` (whitespace split) and reloads.
    /// On a parse error the tokens are still updated but the list is preserved,
    /// and the error is shown so the user can fix the buffer (`/` to re-edit).
    fn commit_filter(&mut self) {
        self.filter_tokens = self
            .filter_input
            .value()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        self.mode = Mode::Normal;
        self.reload_with_status("filter applied");
    }

    /// Leaves [`Mode::Filter`] without changing the active filter or list.
    fn cancel_filter(&mut self) {
        self.mode = Mode::Normal;
        self.status = None;
    }

    // ── Selection helpers (pure, clamp to bounds, no-op on empty list) ─────

    /// Clamps `selected` into the valid range for the current list.
    fn clamp_selection(&mut self) {
        if self.tasks.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.tasks.len() {
            self.selected = self.tasks.len() - 1;
        }
    }

    fn select_next(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        if self.selected + 1 < self.tasks.len() {
            self.selected += 1;
        }
    }

    fn select_prev(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        self.selected = self.selected.saturating_sub(1);
    }

    fn select_first(&mut self) {
        self.selected = 0;
    }

    fn select_last(&mut self) {
        if self.tasks.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.tasks.len() - 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scoring::ScoredTask;

    /// Builds an `App` with a synthetic task list and no real repository work,
    /// to exercise the pure selection logic. We avoid opening a repo by using
    /// `MaybeUninit`-free construction: the selection helpers never touch the
    /// repo, so we build via a small fixture that fills only the list.
    fn app_with_n_tasks(n: usize) -> App {
        let tasks: Vec<ScoredTask> = (0..n)
            .map(|i| ScoredTask {
                task: Task::new(format!("task {i}")),
                score: i as f64,
            })
            .collect();
        let mut app = App::new(
            Config::default(),
            test_repo(),
            ConfigSource::Default,
            chrono::NaiveDate::from_ymd_opt(2026, 6, 5).unwrap(),
        );
        app.tasks = tasks;
        app
    }

    fn test_repo() -> TaskRepository {
        let dir = tempfile::tempdir().unwrap();
        // Leak the tempdir so the path stays valid for the test's lifetime.
        let root = dir.keep();
        let repo = git2::Repository::init(&root).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Test").unwrap();
        cfg.set_str("user.email", "test@test.com").unwrap();
        let config = Config::default();
        crate::core::bootstrap::open_repository(root, &config).unwrap()
    }

    const TODAY: (i32, u32, u32) = (2026, 6, 5);

    /// Builds an `App` over a real (empty) repo containing the given tasks, then
    /// reloads so `tasks()` reflects them through the full filter pipeline.
    fn app_with_repo_tasks(tasks: Vec<Task>) -> App {
        let mut repo = test_repo();
        for task in &tasks {
            repo.store_mut().save_task(task).unwrap();
        }
        let today = NaiveDate::from_ymd_opt(TODAY.0, TODAY.1, TODAY.2).unwrap();
        let mut app = App::new(Config::default(), repo, ConfigSource::Default, today);
        app.reload().unwrap();
        app
    }

    /// Titles currently visible, for order-independent assertions.
    fn visible_titles(app: &App) -> Vec<String> {
        app.tasks().iter().map(|s| s.task.title.clone()).collect()
    }

    #[test]
    fn commit_filter_parses_buffer_into_tokens() {
        let mut app = app_with_repo_tasks(Vec::new());
        app.open_filter();
        app.filter_input = Input::new("  +#rust   -#chore  ".to_owned());
        app.commit_filter();
        assert_eq!(app.filter_tokens(), &["+#rust", "-#chore"]);
        assert_eq!(app.mode(), Mode::Normal);
    }

    #[test]
    fn reload_applies_required_tag_filter() {
        let mut tagged = Task::new("rusty");
        tagged.tags = vec!["#rust".to_owned()];
        let plain = Task::new("plain");
        let mut app = app_with_repo_tasks(vec![tagged, plain]);

        // No filter: both tasks visible.
        assert_eq!(visible_titles(&app).len(), 2);

        // Require the #rust tag: only the tagged task survives.
        app.filter_tokens = vec!["+#rust".to_owned()];
        app.reload().unwrap();
        assert_eq!(visible_titles(&app), vec!["rusty".to_owned()]);
    }

    #[test]
    fn toggle_all_changes_results() {
        let open = Task::new("open task");
        let mut done = Task::new("done task");
        done.mark_done();
        let mut app = app_with_repo_tasks(vec![open, done]);

        // Implicit filter hides the done task.
        assert_eq!(visible_titles(&app), vec!["open task".to_owned()]);

        // `--all` (toggled via the action) bypasses implicit filtering.
        app.update(Action::ToggleAll);
        assert!(app.filter_all());
        let titles = visible_titles(&app);
        assert!(titles.contains(&"open task".to_owned()));
        assert!(titles.contains(&"done task".to_owned()));

        // Toggling back restores the filtered view.
        app.update(Action::ToggleAll);
        assert!(!app.filter_all());
        assert_eq!(visible_titles(&app), vec!["open task".to_owned()]);
    }

    #[test]
    fn invalid_token_sets_status_and_preserves_list() {
        let mut app = app_with_repo_tasks(vec![Task::new("keepme")]);
        let before = visible_titles(&app);
        assert_eq!(before, vec!["keepme".to_owned()]);

        // A malformed tag fails validation in `to_filter_set`.
        app.open_filter();
        app.filter_input = Input::new("+#bad..tag".to_owned());
        app.commit_filter();

        // The list is unchanged and a status message explains the failure.
        assert_eq!(visible_titles(&app), before);
        assert!(app.status().unwrap().starts_with("filter error:"));
        assert_eq!(app.mode(), Mode::Normal);
    }

    #[test]
    fn cancel_filter_restores_tokens_without_reload() {
        let mut app = app_with_repo_tasks(vec![Task::new("a")]);
        app.filter_tokens = vec!["+#rust".to_owned()];
        app.open_filter();
        // Edit the buffer, then cancel.
        app.filter_input = Input::new("+#other".to_owned());
        app.cancel_filter();
        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.filter_tokens(), &["+#rust"]); // unchanged
    }

    #[test]
    fn open_filter_prefills_current_tokens() {
        let mut app = app_with_repo_tasks(Vec::new());
        app.filter_tokens = vec!["+#rust".to_owned(), "-#chore".to_owned()];
        app.open_filter();
        assert_eq!(app.mode(), Mode::Filter);
        assert_eq!(app.filter_input().value(), "+#rust -#chore");
    }

    #[test]
    fn select_next_clamps_at_end() {
        let mut app = app_with_n_tasks(3);
        app.select_last();
        assert_eq!(app.selected(), 2);
        app.select_next();
        assert_eq!(app.selected(), 2); // stays at last
    }

    #[test]
    fn select_prev_clamps_at_start() {
        let mut app = app_with_n_tasks(3);
        assert_eq!(app.selected(), 0);
        app.select_prev();
        assert_eq!(app.selected(), 0); // stays at first
    }

    #[test]
    fn next_then_prev_round_trips() {
        let mut app = app_with_n_tasks(3);
        app.select_next();
        app.select_next();
        assert_eq!(app.selected(), 2);
        app.select_prev();
        assert_eq!(app.selected(), 1);
    }

    #[test]
    fn first_and_last_jump() {
        let mut app = app_with_n_tasks(5);
        app.select_last();
        assert_eq!(app.selected(), 4);
        app.select_first();
        assert_eq!(app.selected(), 0);
    }

    #[test]
    fn selection_on_empty_list_is_noop() {
        let mut app = app_with_n_tasks(0);
        app.select_next();
        app.select_prev();
        app.select_first();
        app.select_last();
        assert_eq!(app.selected(), 0);
        assert!(app.selected_task().is_none());
    }

    #[test]
    fn clamp_selection_after_shrink() {
        let mut app = app_with_n_tasks(5);
        app.select_last();
        assert_eq!(app.selected(), 4);
        app.tasks.truncate(2);
        app.clamp_selection();
        assert_eq!(app.selected(), 1);
    }

    #[test]
    fn selected_task_tracks_index() {
        let mut app = app_with_n_tasks(3);
        app.select_next();
        assert_eq!(app.selected_task().unwrap().title, "task 1");
    }

    #[test]
    fn quit_action_sets_flag() {
        let mut app = app_with_n_tasks(1);
        assert!(!app.should_quit());
        app.update(Action::Quit);
        assert!(app.should_quit());
    }
}
