//! Central application state and the Elm-style update loop for the TUI.
//!
//! [`App`] holds everything the UI needs to render and mutate. Input flows as
//! key event → [`Action`] (via [`App::handle_key`]) → state change (via
//! [`App::update`]). Keeping the key mapping and the state transitions in
//! separate steps lets later tasks add modes and actions without reshaping the
//! event loop.

use chrono::NaiveDate;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::core::domain::filter;
use crate::core::domain::task::Task;
use crate::core::scoring::{self, ScoredTask};
use crate::core::{FilterArgs, TaskRepository};
use crate::Config;

use super::config::ConfigSource;

/// Which interaction mode the UI is in.
///
/// Only [`Mode::Normal`] exists in T3. Later tasks (filter bar, edit modal,
/// confirm popup) add variants here, and the event loop / drawing code branch
/// on the active mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Browsing the task list; keys drive selection and reload.
    #[default]
    Normal,
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

    /// The task currently under the selection, or `None` when the list is empty.
    pub fn selected_task(&self) -> Option<&Task> {
        self.tasks.get(self.selected).map(|s| &s.task)
    }

    // ── Data loading ──────────────────────────────────────────────────────

    /// Re-runs the load + score pipeline (the same one `next list` uses with no
    /// filter tokens) and clamps the selection to the new list length.
    ///
    /// The filter pipeline lives here; the live filter bar (T4) should feed its
    /// tokens into the [`FilterArgs`] built below instead of the empty default.
    pub fn reload(&mut self) -> anyhow::Result<()> {
        let store = self.repo.store();
        let state = store.get_state()?;
        let all_tasks = store.list_tasks()?;
        let tag_metas = store.list_tag_metas()?;

        // T3: no filter tokens — the default implicit filtering, like `next list`.
        let filter_set = FilterArgs::parse(Vec::new()).to_filter_set()?;
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
            _ => None,
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
            Action::Reload => match self.reload() {
                Ok(()) => self.status = Some("reloaded".to_owned()),
                Err(e) => self.status = Some(format!("reload failed: {e}")),
            },
        }
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
