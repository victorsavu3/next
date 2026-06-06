//! Central application state and the Elm-style update loop for the TUI.
//!
//! [`App`] holds everything the UI needs to render and mutate. Input flows as
//! key event → [`Action`] (via [`App::handle_key`]) → state change (via
//! [`App::update`]). Keeping the key mapping and the state transitions in
//! separate steps lets later tasks add modes and actions without reshaping the
//! event loop.

use std::collections::HashMap;

use chrono::NaiveDate;

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::core::domain::filter;
use crate::core::domain::tag::TagMeta;
use crate::core::domain::task::Task;
use crate::core::scoring::{self, ScoreBreakdown, ScoredTask};
use crate::core::{FilterArgs, TaskRepository};
use crate::Config;

use super::config::ConfigSource;

/// How many lines a single PageUp/PageDown (or Ctrl-u/Ctrl-d) moves the detail
/// pane. A fixed step keeps the action self-contained; the draw layer clamps it
/// against the actual content height.
const DETAIL_SCROLL_STEP: u16 = 10;

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
    /// Editing the selected task in the edit modal.
    Edit,
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
    /// Scroll the detail pane down by one page.
    DetailPageDown,
    /// Scroll the detail pane up by one page.
    DetailPageUp,
    /// Open the edit modal for the selected task.
    OpenEdit,
    /// While in [`Mode::Edit`]: feed a key event to the focused form field.
    EditInput(KeyEvent),
    /// While in [`Mode::Edit`]: move focus to the next field.
    EditFocusNext,
    /// While in [`Mode::Edit`]: move focus to the previous field.
    EditFocusPrev,
    /// While in [`Mode::Edit`]: validate + save the form, then reload.
    EditSave,
    /// While in [`Mode::Edit`]: discard the form and return to Normal.
    EditCancel,
}

/// Everything the detail pane needs for one task, resolved from the cached
/// full task list so the draw path makes no store calls.
///
/// `parent`, `children`, and `blockers` are pre-resolved `(short_id, title)`
/// pairs (blockers fall back to the raw id string when unresolved). `breakdown`
/// is recomputed from the cached tasks + tag metadata each frame (cheap).
pub struct DetailData<'a> {
    pub task: &'a Task,
    pub parent: Option<(String, String)>,
    pub children: Vec<(String, String)>,
    pub blockers: Vec<String>,
    pub breakdown: ScoreBreakdown,
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

    /// The full, unfiltered task list from the last [`App::reload`]. Cached so
    /// the detail pane can resolve parent/children/blockers without store calls.
    all_tasks: Vec<Task>,
    /// Tag metadata from the last reload, cached for the detail breakdown.
    tag_metas: HashMap<String, TagMeta>,

    /// Vertical scroll offset (in lines) of the detail pane. Reset to 0 whenever
    /// the selection changes; clamped against the content by the draw layer.
    detail_scroll: u16,

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
    /// The live edit-modal form, present only while in [`Mode::Edit`].
    edit_form: Option<super::edit::EditForm>,
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
            all_tasks: Vec::new(),
            tag_metas: HashMap::new(),
            detail_scroll: 0,
            filter_tokens: Vec::new(),
            filter_future: false,
            filter_all: false,
            filter_all_users: false,
            filter_input: Input::default(),
            mode: Mode::Normal,
            edit_form: None,
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

    /// The live edit form, for the drawing layer to render the modal. Present
    /// only while in [`Mode::Edit`].
    pub fn edit_form(&self) -> Option<&super::edit::EditForm> {
        self.edit_form.as_ref()
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

    /// The current detail-pane scroll offset, in lines.
    pub fn detail_scroll(&self) -> u16 {
        self.detail_scroll
    }

    /// The full task list cached at the last reload (for parent/tag pickers in
    /// later tasks, and for the detail pane's reference resolution).
    pub fn all_tasks(&self) -> &[Task] {
        &self.all_tasks
    }

    /// The tag metadata cached at the last reload (for tag multiselect / scoring).
    pub fn tag_metas(&self) -> &HashMap<String, TagMeta> {
        &self.tag_metas
    }

    /// Bundles everything the detail pane renders for the selected task: the
    /// task itself, its resolved parent/children/blockers, and its score
    /// breakdown. Resolved from the cached vecs, so it makes no store calls.
    /// Returns `None` when the list is empty.
    pub fn selected_detail(&self) -> Option<DetailData<'_>> {
        let task = self.selected_task()?;

        let parent = task.parent_id.and_then(|pid| {
            self.all_tasks
                .iter()
                .find(|t| t.id == pid)
                .map(|p| (short_id(p), p.title.clone()))
        });

        let children: Vec<(String, String)> = self
            .all_tasks
            .iter()
            .filter(|t| t.parent_id == Some(task.id))
            .map(|c| (short_id(c), c.title.clone()))
            .collect();

        let blockers: Vec<String> = task
            .blocked_by
            .iter()
            .map(|bid| {
                self.all_tasks
                    .iter()
                    .find(|t| t.id == *bid)
                    .map(|t| format!("[{}] {}", short_id(t), t.title))
                    .unwrap_or_else(|| bid.to_string())
            })
            .collect();

        let parent_task = task
            .parent_id
            .and_then(|pid| self.all_tasks.iter().find(|t| t.id == pid));
        let breakdown = scoring::score_with_breakdown(
            task,
            parent_task,
            self.today,
            &self.repo.scoring,
            &self.tag_metas,
        );

        Some(DetailData {
            task,
            parent,
            children,
            blockers,
            breakdown,
        })
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
        // Keep the full task list + tag metadata for the detail pane.
        self.all_tasks = all_tasks;
        self.tag_metas = tag_metas;
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
            Mode::Edit => self.edit_key(key),
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
            KeyCode::Char('e') => Some(Action::OpenEdit),
            KeyCode::Char('/') => Some(Action::OpenFilter),
            KeyCode::Char('A') => Some(Action::ToggleAll),
            KeyCode::Char('F') => Some(Action::ToggleFuture),
            KeyCode::Char('U') => Some(Action::ToggleAllUsers),
            KeyCode::PageDown => Some(Action::DetailPageDown),
            KeyCode::PageUp => Some(Action::DetailPageUp),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::DetailPageDown)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::DetailPageUp)
            }
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

    /// Key mapping for the edit modal. Modal-level keys (save/cancel/field
    /// navigation) win; everything else is routed to the focused field.
    ///
    /// `Tab` always moves to the next field, even inside multi-line textareas,
    /// so field navigation is never swallowed by the editor. Within a textarea
    /// the arrow keys edit text (they are passed through as field input).
    fn edit_key(&self, key: KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => Some(Action::EditCancel),
            KeyCode::Char('s') if ctrl => Some(Action::EditSave),
            KeyCode::Tab | KeyCode::BackTab => {
                if key.code == KeyCode::BackTab
                    || key.modifiers.contains(KeyModifiers::SHIFT)
                {
                    Some(Action::EditFocusPrev)
                } else {
                    Some(Action::EditFocusNext)
                }
            }
            // ↑/↓ move between fields, EXCEPT inside a multi-line textarea where
            // they navigate text. Tab is always available for field movement.
            KeyCode::Down if !self.edit_focus_is_multiline() => Some(Action::EditFocusNext),
            KeyCode::Up if !self.edit_focus_is_multiline() => Some(Action::EditFocusPrev),
            _ => Some(Action::EditInput(key)),
        }
    }

    /// Whether the edit modal's focused field is a multi-line textarea.
    fn edit_focus_is_multiline(&self) -> bool {
        self.edit_form
            .as_ref()
            .map(|f| f.focus_is_multiline())
            .unwrap_or(false)
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
            // A page is half the detail body height; the exact figure is
            // unknown here (it depends on the rendered area), so use a sensible
            // fixed step and let the draw layer clamp the offset to the content.
            Action::DetailPageDown => self.detail_scroll_down(DETAIL_SCROLL_STEP),
            Action::DetailPageUp => self.detail_scroll_up(DETAIL_SCROLL_STEP),
            Action::OpenEdit => self.open_edit(),
            Action::EditInput(key) => {
                if let Some(form) = self.edit_form.as_mut() {
                    form.handle_field_key(key);
                }
            }
            Action::EditFocusNext => {
                if let Some(form) = self.edit_form.as_mut() {
                    form.focus_next();
                }
            }
            Action::EditFocusPrev => {
                if let Some(form) = self.edit_form.as_mut() {
                    form.focus_prev();
                }
            }
            Action::EditSave => self.save_edit(),
            Action::EditCancel => self.cancel_edit(),
        }
    }

    /// Opens the edit modal for the selected task. No-op when the list is empty.
    fn open_edit(&mut self) {
        let Some(task) = self.selected_task() else {
            self.status = Some("nothing selected to edit".to_owned());
            return;
        };
        self.edit_form = Some(super::edit::EditForm::from_task(task));
        self.mode = Mode::Edit;
        self.status = None;
    }

    /// Discards the edit form and returns to the list.
    fn cancel_edit(&mut self) {
        self.edit_form = None;
        self.mode = Mode::Normal;
        self.status = Some("edit cancelled".to_owned());
    }

    /// Validates and applies the edit form. On any error (validation, apply,
    /// git conflict) the modal STAYS open with the error in the status line so
    /// the user can fix and retry. On success the modal closes, the list
    /// reloads, and the selection is kept on the edited task.
    fn save_edit(&mut self) {
        let Some(form) = self.edit_form.as_ref() else {
            return;
        };
        let task_id = form.task_id;

        // Build the params + data changes first; surface validation errors
        // without touching the store.
        let params = match form.to_edit_params(self.today) {
            Ok(p) => p,
            Err(e) => {
                self.status = Some(format!("edit error: {e}"));
                return;
            }
        };
        let data_changes = form.data_changes();

        if let Err(e) = self.apply_edit(task_id, params, data_changes) {
            // Keep the modal open so the user can retry (e.g. after a git
            // conflict resolves, or after fixing a field).
            self.status = Some(format!("edit error: {e}"));
            return;
        }

        // Success: close the modal, reload, and re-select the edited task.
        self.edit_form = None;
        self.mode = Mode::Normal;
        match self.reload() {
            Ok(()) => {
                if let Some(idx) = self.tasks.iter().position(|s| s.task.id == task_id) {
                    self.selected = idx;
                }
                self.status = Some("task saved".to_owned());
            }
            Err(e) => self.status = Some(format!("saved, but reload failed: {e}")),
        }
    }

    /// Applies the field edits via the shared service, then the out-of-band data
    /// changes (one commit each, mirroring the CLI `edit` and `data` commands).
    fn apply_edit(
        &mut self,
        task_id: uuid::Uuid,
        params: crate::core::service::EditTaskParams,
        data_changes: Vec<(String, Option<serde_json::Value>)>,
    ) -> anyhow::Result<()> {
        use crate::core::service::apply_edits;

        let repo_root = self.repo.repo_root.clone();
        let task = apply_edits(
            task_id,
            params,
            self.today,
            &repo_root,
            &mut *self.repo.store,
            &*self.repo.vcs,
        )?;
        self.repo.record_task_event("edit", task.id);

        // Apply data set/unset edits the way the CLI `data` command does: mutate
        // `task.data` and commit, one transaction per change.
        for (key, value) in data_changes {
            self.repo.transaction(|store, vcs, root| {
                let mut t = store.get_task(task_id)?;
                match value {
                    Some(v) => {
                        t.data.insert(key.clone(), v);
                    }
                    None => {
                        t.data.remove(&key);
                    }
                }
                t.touch();
                let path = crate::core::storage::task_path(root, &t);
                store.save_task(&t)?;
                vcs.commit(&[path], &format!("next: data edit {key} on {}", t.title))?;
                Ok(())
            })?;
            self.repo.record_task_event("data", task_id);
        }
        Ok(())
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
            self.detail_scroll = 0;
        }
    }

    fn select_prev(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let prev = self.selected;
        self.selected = self.selected.saturating_sub(1);
        if self.selected != prev {
            self.detail_scroll = 0;
        }
    }

    fn select_first(&mut self) {
        if self.selected != 0 {
            self.detail_scroll = 0;
        }
        self.selected = 0;
    }

    fn select_last(&mut self) {
        let last = self.tasks.len().saturating_sub(1);
        if self.selected != last {
            self.detail_scroll = 0;
        }
        self.selected = last;
    }

    // ── Detail scrolling ──────────────────────────────────────────────────

    /// Scrolls the detail pane down by `lines`. Over-scroll is clamped by the
    /// draw layer against the rendered content height, so we only guard the
    /// `u16` arithmetic here.
    fn detail_scroll_down(&mut self, lines: u16) {
        self.detail_scroll = self.detail_scroll.saturating_add(lines);
    }

    /// Scrolls the detail pane up by `lines`, saturating at the top.
    fn detail_scroll_up(&mut self, lines: u16) {
        self.detail_scroll = self.detail_scroll.saturating_sub(lines);
    }

    /// Clamps the detail scroll so the offset never exceeds `max`. The draw
    /// layer calls this once it knows the content height vs. the pane height.
    pub fn clamp_detail_scroll(&mut self, max: u16) {
        if self.detail_scroll > max {
            self.detail_scroll = max;
        }
    }
}

/// The first 8 hex digits of a task's UUID (dashes stripped), matching the
/// `[id]` form used by `next show`.
fn short_id(task: &Task) -> String {
    task.id.to_string().replace('-', "")[..8].to_owned()
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

    #[test]
    fn detail_scroll_clamps_to_content() {
        let mut app = app_with_n_tasks(1);
        app.update(Action::DetailPageDown);
        app.update(Action::DetailPageDown);
        assert!(app.detail_scroll() > 0);
        // A short pane (max offset 3) clamps the larger accumulated offset.
        app.clamp_detail_scroll(3);
        assert_eq!(app.detail_scroll(), 3);
        // Cannot go below zero.
        app.update(Action::DetailPageUp);
        app.update(Action::DetailPageUp);
        assert_eq!(app.detail_scroll(), 0);
    }

    #[test]
    fn detail_scroll_resets_on_selection_change() {
        let mut app = app_with_n_tasks(3);
        app.update(Action::DetailPageDown);
        assert!(app.detail_scroll() > 0);
        app.update(Action::SelectNext);
        assert_eq!(app.detail_scroll(), 0);

        // Scroll again, then jump to last / first.
        app.update(Action::DetailPageDown);
        assert!(app.detail_scroll() > 0);
        app.update(Action::SelectLast);
        assert_eq!(app.detail_scroll(), 0);

        app.update(Action::DetailPageDown);
        app.update(Action::SelectFirst);
        assert_eq!(app.detail_scroll(), 0);
    }

    #[test]
    fn detail_scroll_unchanged_when_selection_does_not_move() {
        let mut app = app_with_n_tasks(2);
        app.update(Action::DetailPageDown);
        let before = app.detail_scroll();
        // Already at first; SelectPrev is a no-op and must not reset scroll.
        app.update(Action::SelectPrev);
        assert_eq!(app.detail_scroll(), before);
    }

    // ── edit modal ────────────────────────────────────────────────────────────

    use tui_input::Input;

    /// Builds an `App` over a real repo that supports git commits (user.name /
    /// user.email set), then creates `title` via the shared service so it is
    /// committed and editable. Returns the app (reloaded) and the task id.
    fn app_with_committed_task(title: &str) -> (App, uuid::Uuid) {
        let mut repo = test_repo();
        let today = NaiveDate::from_ymd_opt(TODAY.0, TODAY.1, TODAY.2).unwrap();
        let task = crate::core::service::create_task(
            title.to_owned(),
            crate::core::service::CreateTaskParams::default(),
            today,
            &repo.repo_root.clone(),
            &mut *repo.store,
            &*repo.vcs,
        )
        .unwrap();
        let mut app = App::new(Config::default(), repo, ConfigSource::Default, today);
        app.reload().unwrap();
        (app, task.id)
    }

    #[test]
    fn open_edit_enters_mode_and_seeds_form() {
        let (mut app, _id) = app_with_committed_task("Edit me");
        app.update(Action::OpenEdit);
        assert_eq!(app.mode(), Mode::Edit);
        assert_eq!(app.edit_form().unwrap().title.value(), "Edit me");
    }

    #[test]
    fn cancel_edit_discards_and_returns_to_normal() {
        let (mut app, _id) = app_with_committed_task("Keep title");
        app.update(Action::OpenEdit);
        app.edit_form.as_mut().unwrap().title = Input::new("Changed".to_owned());
        app.update(Action::EditCancel);
        assert_eq!(app.mode(), Mode::Normal);
        assert!(app.edit_form().is_none());
        // The store is untouched.
        let stored = app.repo.store.get_task(_id).unwrap();
        assert_eq!(stored.title, "Keep title");
    }

    #[test]
    fn save_edit_round_trips_changes_to_store() {
        let (mut app, id) = app_with_committed_task("Before");
        app.update(Action::OpenEdit);
        {
            let form = app.edit_form.as_mut().unwrap();
            form.title = Input::new("After".to_owned());
            form.due = Input::new("2026-09-01".to_owned());
            form.priority = crate::core::domain::task::Priority::High;
            form.tags = vec!["#rust".to_owned()];
        }
        app.update(Action::EditSave);

        assert_eq!(app.mode(), Mode::Normal, "modal closes on success");
        let stored = app.repo.store.get_task(id).unwrap();
        assert_eq!(stored.title, "After");
        assert_eq!(stored.due, NaiveDate::from_ymd_opt(2026, 9, 1));
        assert_eq!(stored.priority, crate::core::domain::task::Priority::High);
        assert!(stored.tags.contains(&"#rust".to_owned()));
        // Selection follows the edited task.
        assert_eq!(app.selected_task().unwrap().id, id);
    }

    #[test]
    fn save_edit_applies_data_changes() {
        let (mut app, id) = app_with_committed_task("Data task");
        app.update(Action::OpenEdit);
        app.edit_form
            .as_mut()
            .unwrap()
            .data
            .insert("ticket".to_owned(), serde_json::json!("JIRA-7"));
        app.update(Action::EditSave);
        let stored = app.repo.store.get_task(id).unwrap();
        assert_eq!(stored.data.get("ticket").and_then(|v| v.as_str()), Some("JIRA-7"));
    }

    #[test]
    fn save_edit_invalid_field_keeps_modal_open() {
        let (mut app, id) = app_with_committed_task("Stays");
        app.update(Action::OpenEdit);
        // Empty title is invalid → modal stays open with an error status.
        app.edit_form.as_mut().unwrap().title = Input::new(String::new());
        app.update(Action::EditSave);
        assert_eq!(app.mode(), Mode::Edit, "modal stays open on validation error");
        assert!(app.status().unwrap().contains("edit error"));
        // Store unchanged.
        assert_eq!(app.repo.store.get_task(id).unwrap().title, "Stays");
    }

    #[test]
    fn tab_moves_field_focus() {
        let (mut app, _id) = app_with_committed_task("Tabs");
        app.update(Action::OpenEdit);
        let first = app.edit_form().unwrap().focus;
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let action = app.handle_key(tab).unwrap();
        app.update(action);
        assert_ne!(app.edit_form().unwrap().focus, first);
    }
}
