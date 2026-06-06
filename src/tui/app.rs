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

/// Which top-level view is active. [`View::List`] is the scored, flat list
/// (the original UI); [`View::Tree`] is the parent/child hierarchy; and
/// [`View::Forecast`] is the chronological, sectioned forecast.
///
/// Switch with `1`/`2`/`3` (or `Tab` to cycle) from [`Mode::Normal`]. The list
/// and tree views share the detail pane and the per-task actions/edit keys; the
/// forecast view is a full-width read-only list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// The scored, sorted flat task list.
    #[default]
    List,
    /// The parent/child task hierarchy.
    Tree,
    /// The chronological forecast (concrete + projected occurrences).
    Forecast,
}

impl View {
    /// The short label shown in the title bar.
    pub fn label(self) -> &'static str {
        match self {
            View::List => "list",
            View::Tree => "tree",
            View::Forecast => "forecast",
        }
    }

    /// The next view in the `List → Tree → Forecast → List` cycle (for `Tab`).
    fn next(self) -> Self {
        match self {
            View::List => View::Tree,
            View::Tree => View::Forecast,
            View::Forecast => View::List,
        }
    }
}

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
    /// Confirming deletion of the selected task in a yes/no popup.
    ConfirmDelete,
    /// Picking a new parent for the selected task in a searchable list.
    MovePicker,
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

    /// Complete the selected task (recurrence-aware), then reload.
    Done,
    /// Cancel the selected task, then reload.
    Cancel,
    /// Toggle the selected task between started and stopped, then reload.
    ToggleStart,
    /// Open the selected task's URL in the system browser.
    OpenUrl,
    /// Open the delete-confirmation popup for the selected task.
    OpenDelete,
    /// While in [`Mode::ConfirmDelete`]: confirm and delete the task.
    ConfirmDelete,
    /// While in [`Mode::ConfirmDelete`]: dismiss the popup without deleting.
    CancelDelete,
    /// Open the move (parent-picker) popup for the selected task.
    OpenMove,
    /// While in [`Mode::MovePicker`]: feed a raw key event to the search buffer.
    MoveInput(KeyEvent),
    /// While in [`Mode::MovePicker`]: move the highlight to the next candidate.
    MoveNext,
    /// While in [`Mode::MovePicker`]: move the highlight to the previous candidate.
    MovePrev,
    /// While in [`Mode::MovePicker`]: reparent to the highlighted candidate.
    MoveConfirm,
    /// While in [`Mode::MovePicker`]: dismiss the popup without moving.
    MoveCancel,

    /// Switch to a specific top-level view (List / Tree / Forecast).
    SwitchView(View),
    /// Cycle to the next view (`Tab`).
    CycleView,

    /// In [`View::Tree`]: move the highlight to the next visible node.
    TreeNext,
    /// In [`View::Tree`]: move the highlight to the previous visible node.
    TreePrev,
    /// In [`View::Tree`]: expand the highlighted node.
    TreeExpand,
    /// In [`View::Tree`]: collapse the highlighted node.
    TreeCollapse,
    /// In [`View::Tree`]: toggle expand/collapse of the highlighted node.
    TreeToggle,
    /// In [`View::Tree`]: toggle the tree-local include-done/cancelled flag.
    TreeToggleAll,

    /// In [`View::Forecast`]: widen the forecast horizon.
    ForecastWiden,
    /// In [`View::Forecast`]: narrow the forecast horizon.
    ForecastNarrow,
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

/// One selectable entry in the move (parent-picker) list: either a concrete
/// task or the synthetic "top-level" option (`task_id == None`).
pub struct MoveCandidate {
    /// The target parent id, or `None` for "no parent / top-level".
    pub task_id: Option<uuid::Uuid>,
    /// The label shown in the list.
    pub label: String,
}

/// Live state for the move (parent-picker) popup. Holds the full candidate set
/// (already pruned of the moved task and its descendants to prevent cycles), a
/// live search buffer, and the highlighted row within the *filtered* view.
pub struct MovePicker {
    /// The task being reparented.
    task_id: uuid::Uuid,
    /// All valid candidates (top-level option first, then non-descendant tasks).
    candidates: Vec<MoveCandidate>,
    /// The live search buffer.
    query: Input,
    /// Highlighted index within the filtered candidate list.
    selected: usize,
}

impl MovePicker {
    /// The candidates matching the current query (case-insensitive substring).
    /// The top-level option (empty-ish label match) always matches an empty
    /// query and matches when its label contains the query.
    pub fn filtered(&self) -> Vec<&MoveCandidate> {
        let q = self.query.value().to_lowercase();
        self.candidates
            .iter()
            .filter(|c| q.is_empty() || c.label.to_lowercase().contains(&q))
            .collect()
    }

    /// The live search buffer, for rendering the input line.
    pub fn query(&self) -> &Input {
        &self.query
    }

    /// The highlighted index within the filtered list.
    pub fn selected(&self) -> usize {
        self.selected
    }
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

    /// The active top-level view (list / tree / forecast).
    view: View,
    /// Tree-view state (selection + expansion + include-all toggle).
    tree_view: super::tree::TreeView,
    /// Forecast-view state (the horizon).
    forecast_view: super::forecast::ForecastView,

    /// The current interaction mode.
    mode: Mode,
    /// The live edit-modal form, present only while in [`Mode::Edit`].
    edit_form: Option<super::edit::EditForm>,
    /// The live move (parent-picker) state, present only in [`Mode::MovePicker`].
    move_picker: Option<MovePicker>,
    /// A transient status message shown in the footer.
    status: Option<String>,
    /// Set when the user asks to quit; the event loop checks this.
    should_quit: bool,
}

impl App {
    /// Builds a fresh app over the given repository. Call [`App::reload`] before
    /// the first draw to populate the task list.
    pub fn new(config: Config, repo: TaskRepository, source: ConfigSource, today: NaiveDate) -> Self {
        let forecast_view = super::forecast::ForecastView::new(config.forecast_horizon_days);
        Self {
            config,
            repo,
            source,
            today,
            view: View::default(),
            tree_view: super::tree::TreeView::default(),
            forecast_view,
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
            move_picker: None,
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

    /// The active top-level view, for the title bar and draw dispatch.
    pub fn view(&self) -> View {
        self.view
    }

    /// Mutable access to the tree-view state (for the stateful tree render).
    pub fn tree_view_mut(&mut self) -> &mut super::tree::TreeView {
        &mut self.tree_view
    }

    /// The tree-local include-done/cancelled toggle.
    pub fn tree_include_all(&self) -> bool {
        self.tree_view.include_all()
    }

    /// The forecast horizon in days.
    pub fn forecast_horizon(&self) -> u32 {
        self.forecast_view.horizon()
    }

    /// Builds the displayable tree items from the cached task list, honouring
    /// the tree-local include-all toggle. Recomputed each frame (cheap).
    pub fn tree_items(&self) -> Vec<tui_tree_widget::TreeItem<'static, uuid::Uuid>> {
        super::tree::build_items(&self.all_tasks, self.tree_view.include_all())
    }

    /// Computes the forecast entries for the current horizon, honouring the
    /// active filter tokens/flags. Returns an empty vec on a filter error (the
    /// same tokens already drive the list, so an error is surfaced there).
    pub fn forecast_entries(&self) -> Vec<crate::core::forecast::ForecastEntry> {
        let mut fa = FilterArgs::parse(self.filter_tokens.clone());
        fa.future = self.filter_future;
        fa.all = self.filter_all;
        fa.all_users = self.filter_all_users;
        let Ok(filter_set) = fa.to_filter_set() else {
            return Vec::new();
        };
        let store = self.repo.store();
        let (Ok(state), Ok(tag_metas)) = (store.get_state(), store.list_tag_metas()) else {
            return Vec::new();
        };
        crate::core::forecast::build_entries(
            &self.all_tasks,
            &state,
            &self.repo.scoring,
            &tag_metas,
            &filter_set,
            self.today,
            self.forecast_view.horizon(),
        )
    }

    /// The live edit form, for the drawing layer to render the modal. Present
    /// only while in [`Mode::Edit`].
    pub fn edit_form(&self) -> Option<&super::edit::EditForm> {
        self.edit_form.as_ref()
    }

    /// The live move (parent-picker) state, for the drawing layer to render the
    /// popup. Present only while in [`Mode::MovePicker`].
    pub fn move_picker(&self) -> Option<&MovePicker> {
        self.move_picker.as_ref()
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

    /// The task the per-task actions / detail pane operate on.
    ///
    /// In [`View::List`] and [`View::Forecast`] this is the list row under the
    /// selection; in [`View::Tree`] it is the highlighted tree node resolved
    /// against the cached task list — so the existing edit/done/etc. keys act on
    /// the tree's current node. Returns `None` when nothing is selected.
    pub fn selected_task(&self) -> Option<&Task> {
        match self.view {
            View::Tree => self
                .tree_view
                .selected_id()
                .and_then(|id| self.all_tasks.iter().find(|t| t.id == id)),
            View::List | View::Forecast => self.tasks.get(self.selected).map(|s| &s.task),
        }
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
            Mode::Normal => self.normal_key(key),
            Mode::Filter => Self::filter_key(key),
            Mode::Edit => self.edit_key(key),
            Mode::ConfirmDelete => Self::confirm_delete_key(key),
            Mode::MovePicker => Self::move_picker_key(key),
        }
    }

    /// Keys shared by every view in [`Mode::Normal`]: quit, reload, filter, view
    /// switching, and the per-task actions (which resolve the selected task in a
    /// view-aware way). Returns `None` if the key is not one of these.
    fn normal_common_key(key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Quit),
            KeyCode::Tab => Some(Action::CycleView),
            KeyCode::Char('1') => Some(Action::SwitchView(View::List)),
            KeyCode::Char('2') => Some(Action::SwitchView(View::Tree)),
            KeyCode::Char('3') => Some(Action::SwitchView(View::Forecast)),
            KeyCode::Char('r') => Some(Action::Reload),
            KeyCode::Char('/') => Some(Action::OpenFilter),
            _ => None,
        }
    }

    /// The per-task action keys (edit/done/cancel/start/open/move/delete), shared
    /// by the list and tree views; both resolve `selected_task()` view-aware.
    fn task_action_key(key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('e') => Some(Action::OpenEdit),
            KeyCode::Char('d') => Some(Action::Done),
            KeyCode::Char('c') => Some(Action::Cancel),
            KeyCode::Char('s') => Some(Action::ToggleStart),
            KeyCode::Char('o') => Some(Action::OpenUrl),
            KeyCode::Char('m') => Some(Action::OpenMove),
            KeyCode::Char('x') | KeyCode::Delete => Some(Action::OpenDelete),
            _ => None,
        }
    }

    /// Dispatches a Normal-mode key to the active view's handler.
    fn normal_key(&self, key: KeyEvent) -> Option<Action> {
        match self.view {
            View::List => Self::list_key(key),
            View::Tree => Self::tree_key(key),
            View::Forecast => Self::forecast_key(key),
        }
    }

    fn list_key(key: KeyEvent) -> Option<Action> {
        if let Some(a) = Self::normal_common_key(key) {
            return Some(a);
        }
        match key.code {
            // Ctrl-d / Ctrl-u scroll the detail pane; the plain keys below are
            // bound to actions, so the modifier guards must come first.
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::DetailPageDown)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::DetailPageUp)
            }
            KeyCode::Char('j') | KeyCode::Down => Some(Action::SelectNext),
            KeyCode::Char('k') | KeyCode::Up => Some(Action::SelectPrev),
            KeyCode::Char('g') | KeyCode::Home => Some(Action::SelectFirst),
            KeyCode::Char('G') | KeyCode::End => Some(Action::SelectLast),
            KeyCode::Char('A') => Some(Action::ToggleAll),
            KeyCode::Char('F') => Some(Action::ToggleFuture),
            KeyCode::Char('U') => Some(Action::ToggleAllUsers),
            KeyCode::PageDown => Some(Action::DetailPageDown),
            KeyCode::PageUp => Some(Action::DetailPageUp),
            _ => Self::task_action_key(key),
        }
    }

    /// Tree-view keys. Navigation drives the tree widget; `←/→` collapse/expand,
    /// `Space` toggles, `.` toggles include-done/cancelled, and the shared
    /// per-task action keys operate on the highlighted node. Ctrl-d/u still
    /// scroll the detail pane.
    fn tree_key(key: KeyEvent) -> Option<Action> {
        if let Some(a) = Self::normal_common_key(key) {
            return Some(a);
        }
        match key.code {
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::DetailPageDown)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::DetailPageUp)
            }
            KeyCode::Char('j') | KeyCode::Down => Some(Action::TreeNext),
            KeyCode::Char('k') | KeyCode::Up => Some(Action::TreePrev),
            KeyCode::Left => Some(Action::TreeCollapse),
            KeyCode::Right => Some(Action::TreeExpand),
            KeyCode::Char(' ') | KeyCode::Enter => Some(Action::TreeToggle),
            // `.` toggles include-done/cancelled (the list view's `A` is taken by
            // the global filter-all toggle, so the tree uses a distinct key).
            KeyCode::Char('.') => Some(Action::TreeToggleAll),
            KeyCode::PageDown => Some(Action::DetailPageDown),
            KeyCode::PageUp => Some(Action::DetailPageUp),
            _ => Self::task_action_key(key),
        }
    }

    /// Forecast-view keys: a read-only list, so only view switching, reload,
    /// filtering, and horizon adjustment (`+`/`-`) are bound.
    fn forecast_key(key: KeyEvent) -> Option<Action> {
        if let Some(a) = Self::normal_common_key(key) {
            return Some(a);
        }
        match key.code {
            KeyCode::Char('+') | KeyCode::Char('=') => Some(Action::ForecastWiden),
            KeyCode::Char('-') | KeyCode::Char('_') => Some(Action::ForecastNarrow),
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

    /// Key mapping for the delete-confirmation popup: `y`/Enter confirms,
    /// `n`/`Esc` (or `q`) cancels.
    fn confirm_delete_key(key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => Some(Action::ConfirmDelete),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                Some(Action::CancelDelete)
            }
            _ => None,
        }
    }

    /// Key mapping for the move (parent-picker) popup. `Esc` cancels, `Enter`
    /// confirms, ↑/↓ move the highlight, and everything else edits the search
    /// buffer.
    fn move_picker_key(key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Esc => Some(Action::MoveCancel),
            KeyCode::Enter => Some(Action::MoveConfirm),
            KeyCode::Down => Some(Action::MoveNext),
            KeyCode::Up => Some(Action::MovePrev),
            _ => Some(Action::MoveInput(key)),
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

            Action::Done => self.do_done(),
            Action::Cancel => self.do_cancel(),
            Action::ToggleStart => self.do_toggle_start(),
            Action::OpenUrl => self.do_open_url(),
            Action::OpenDelete => self.open_delete(),
            Action::ConfirmDelete => self.do_delete(),
            Action::CancelDelete => self.cancel_delete(),
            Action::OpenMove => self.open_move(),
            Action::MoveInput(key) => self.move_input(key),
            Action::MoveNext => self.move_select_next(),
            Action::MovePrev => self.move_select_prev(),
            Action::MoveConfirm => self.do_move(),
            Action::MoveCancel => self.cancel_move(),

            Action::SwitchView(view) => self.switch_view(view),
            Action::CycleView => self.switch_view(self.view.next()),

            Action::TreeNext => self.tree_view.key_down(),
            Action::TreePrev => self.tree_view.key_up(),
            Action::TreeExpand => self.tree_view.expand(),
            Action::TreeCollapse => self.tree_view.collapse(),
            Action::TreeToggle => self.tree_view.toggle(),
            Action::TreeToggleAll => {
                self.tree_view.toggle_all();
                // Re-anchor the selection in case the toggle hid the current node.
                let items = self.tree_items();
                self.tree_view.ensure_selection(&items);
                self.status = Some(if self.tree_view.include_all() {
                    "tree: showing all".to_owned()
                } else {
                    "tree: active only".to_owned()
                });
            }

            Action::ForecastWiden => {
                self.forecast_view.widen(30);
                self.status = Some(format!("horizon {} days", self.forecast_view.horizon()));
            }
            Action::ForecastNarrow => {
                self.forecast_view.narrow(30);
                self.status = Some(format!("horizon {} days", self.forecast_view.horizon()));
            }
        }
    }

    /// Switches the active view. On entering the tree view, ensures a node is
    /// selected (seeded to the current list selection's task when possible) so
    /// the detail pane and per-task actions have a target.
    fn switch_view(&mut self, view: View) {
        if self.view == view {
            return;
        }
        self.view = view;
        self.status = Some(format!("view: {}", view.label()));
        if view == View::Tree {
            let items = self.tree_items();
            // Seed the tree highlight from the list selection where possible.
            if self.tree_view.selected_id().is_none() {
                if let Some(id) = self.tasks.get(self.selected).map(|s| s.task.id) {
                    self.tree_view.state_mut().select(vec![id]);
                }
            }
            self.tree_view.ensure_selection(&items);
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

    // ── Keybound actions on the selected task ─────────────────────────────
    //
    // Each mutating action mirrors the corresponding CLI command but operates on
    // the already-selected task (no id resolution). On error (validation, store,
    // or git conflict) the status line carries the message and no state is lost;
    // on success a brief status is set, the list reloads, and the selection
    // follows the task by id where it still exists.

    /// Reloads after a mutation and re-selects the task with `keep_id` if it is
    /// still visible; otherwise clamps to the nearest remaining row. Reports
    /// `ok_msg` (or a reload error) into the status line.
    fn reload_keep(&mut self, keep_id: uuid::Uuid, ok_msg: String) {
        match self.reload() {
            Ok(()) => {
                if let Some(idx) = self.tasks.iter().position(|s| s.task.id == keep_id) {
                    self.selected = idx;
                }
                self.clamp_selection();
                self.status = Some(ok_msg);
            }
            Err(e) => self.status = Some(format!("{ok_msg}; reload failed: {e}")),
        }
    }

    /// Completes the selected task (recurrence-aware). The completed task leaves
    /// the default view, so the selection lands on the nearest remaining row. If
    /// the task carried a recurrence, the spawned next instance is noted.
    fn do_done(&mut self) {
        let Some(task) = self.selected_task() else {
            self.status = Some("nothing selected".to_owned());
            return;
        };
        let id = task.id;
        let had_recurrence = task.recurrence.is_some();

        let repo_root = self.repo.repo_root.clone();
        let result = crate::core::service::complete_task(
            id,
            self.today,
            &repo_root,
            &mut *self.repo.store,
            &*self.repo.vcs,
        );
        match result {
            Ok(_) => {
                self.repo.record_task_event("done", id);
                let msg = if had_recurrence {
                    "completed; spawned next occurrence".to_owned()
                } else {
                    "completed".to_owned()
                };
                // The completed task leaves the default view; keep the selection
                // near where it was (clamp handles the now-shorter list).
                match self.reload() {
                    Ok(()) => {
                        self.clamp_selection();
                        self.status = Some(msg);
                    }
                    Err(e) => self.status = Some(format!("{msg}; reload failed: {e}")),
                }
            }
            Err(e) => self.status = Some(format!("done error: {e}")),
        }
    }

    /// Cancels the selected task.
    fn do_cancel(&mut self) {
        let Some(id) = self.selected_task().map(|t| t.id) else {
            self.status = Some("nothing selected".to_owned());
            return;
        };
        let result = self.repo.transaction(|store, vcs, root| {
            let mut t = store.get_task(id)?;
            t.mark_cancelled();
            store.save_task(&t)?;
            let path = crate::core::storage::task_path(root, &t);
            vcs.commit(&[path], &format!("next: cancel {}", t.title))?;
            Ok(())
        });
        match result {
            Ok(()) => {
                self.repo.record_task_event("cancel", id);
                self.reload_keep(id, "cancelled".to_owned());
            }
            Err(e) => self.status = Some(format!("cancel error: {e}")),
        }
    }

    /// Toggles the selected task between started and stopped, based on its
    /// current status (a started task stops; any other active task starts).
    fn do_toggle_start(&mut self) {
        let Some(task) = self.selected_task() else {
            self.status = Some("nothing selected".to_owned());
            return;
        };
        let id = task.id;
        let starting = task.status != crate::core::domain::task::Status::Started;
        let (verb, commit, ok_msg) = if starting {
            ("start", "next: start", "started")
        } else {
            ("stop", "next: stop", "stopped")
        };
        let result = self.repo.transaction(|store, vcs, root| {
            let mut t = store.get_task(id)?;
            if starting {
                t.mark_started();
            } else {
                t.mark_stopped();
            }
            store.save_task(&t)?;
            let path = crate::core::storage::task_path(root, &t);
            vcs.commit(&[path], &format!("{commit} {}", t.title))?;
            Ok(())
        });
        match result {
            Ok(()) => {
                self.repo.record_task_event(verb, id);
                self.reload_keep(id, ok_msg.to_owned());
            }
            Err(e) => self.status = Some(format!("{verb} error: {e}")),
        }
    }

    /// Opens the selected task's URL via the system opener. Read-only; sets an
    /// error status when the task has no URL.
    fn do_open_url(&mut self) {
        let Some(task) = self.selected_task() else {
            self.status = Some("nothing selected".to_owned());
            return;
        };
        match task.url.clone() {
            Some(url) => match open_url(&url) {
                Ok(()) => self.status = Some(format!("opened {url}")),
                Err(e) => self.status = Some(format!("open error: {e}")),
            },
            None => self.status = Some("open error: task has no URL set".to_owned()),
        }
    }

    /// Opens the delete-confirmation popup for the selected task. No-op (with a
    /// status hint) when the list is empty.
    fn open_delete(&mut self) {
        if self.selected_task().is_none() {
            self.status = Some("nothing selected".to_owned());
            return;
        }
        self.mode = Mode::ConfirmDelete;
        self.status = None;
    }

    /// Dismisses the delete-confirmation popup without deleting.
    fn cancel_delete(&mut self) {
        self.mode = Mode::Normal;
        self.status = Some("delete cancelled".to_owned());
    }

    /// Deletes the selected task (after confirmation), then reloads. The deleted
    /// task is gone, so the selection clamps to the nearest remaining row.
    fn do_delete(&mut self) {
        self.mode = Mode::Normal;
        let Some(task) = self.selected_task().cloned() else {
            self.status = Some("nothing selected".to_owned());
            return;
        };
        let id = task.id;
        let result = self.repo.transaction(|store, vcs, root| {
            let path = crate::core::storage::task_path(root, &task);
            store.delete_task(id)?;
            vcs.commit(&[path], &format!("next: delete {}", task.title))?;
            Ok(())
        });
        match result {
            Ok(()) => {
                self.repo.record_task_event("delete", id);
                match self.reload() {
                    Ok(()) => {
                        self.clamp_selection();
                        self.status = Some("deleted".to_owned());
                    }
                    Err(e) => self.status = Some(format!("deleted; reload failed: {e}")),
                }
            }
            Err(e) => self.status = Some(format!("delete error: {e}")),
        }
    }

    /// Opens the move (parent-picker) popup for the selected task, seeded with
    /// every candidate that is neither the task itself nor one of its
    /// descendants (which would create a cycle), plus a "top-level" option.
    fn open_move(&mut self) {
        let Some(task) = self.selected_task() else {
            self.status = Some("nothing selected".to_owned());
            return;
        };
        let task_id = task.id;
        let banned = self.subtree_ids(task_id);

        let mut candidates = vec![MoveCandidate {
            task_id: None,
            label: "(top-level / no parent)".to_owned(),
        }];
        for t in &self.all_tasks {
            if banned.contains(&t.id) {
                continue;
            }
            candidates.push(MoveCandidate {
                task_id: Some(t.id),
                label: format!("[{}] {}", short_id(t), t.title),
            });
        }

        self.move_picker = Some(MovePicker {
            task_id,
            candidates,
            query: Input::default(),
            selected: 0,
        });
        self.mode = Mode::MovePicker;
        self.status = None;
    }

    /// The set of ids in the subtree rooted at `root_id` (inclusive). Used by the
    /// move picker to forbid reparenting a task under itself or a descendant.
    fn subtree_ids(&self, root_id: uuid::Uuid) -> std::collections::HashSet<uuid::Uuid> {
        let mut banned = std::collections::HashSet::new();
        banned.insert(root_id);
        // Iterate to a fixed point: a task joins the set once its parent is in it.
        loop {
            let mut added = false;
            for t in &self.all_tasks {
                if let Some(pid) = t.parent_id {
                    if banned.contains(&pid) && banned.insert(t.id) {
                        added = true;
                    }
                }
            }
            if !added {
                break;
            }
        }
        banned
    }

    /// Feeds a key event to the move picker's search buffer, then clamps the
    /// highlight to the new filtered length.
    fn move_input(&mut self, key: KeyEvent) {
        if let Some(picker) = self.move_picker.as_mut() {
            picker.query.handle_event(&Event::Key(key));
            let len = picker.filtered().len();
            if picker.selected >= len {
                picker.selected = len.saturating_sub(1);
            }
        }
    }

    /// Moves the move-picker highlight down by one (clamped at the last match).
    fn move_select_next(&mut self) {
        if let Some(picker) = self.move_picker.as_mut() {
            let len = picker.filtered().len();
            if len > 0 && picker.selected + 1 < len {
                picker.selected += 1;
            }
        }
    }

    /// Moves the move-picker highlight up by one (clamped at the first match).
    fn move_select_prev(&mut self) {
        if let Some(picker) = self.move_picker.as_mut() {
            picker.selected = picker.selected.saturating_sub(1);
        }
    }

    /// Dismisses the move picker without reparenting.
    fn cancel_move(&mut self) {
        self.move_picker = None;
        self.mode = Mode::Normal;
        self.status = Some("move cancelled".to_owned());
    }

    /// Reparents the picked task to the highlighted candidate (or top-level),
    /// then reloads and re-selects the moved task. Closes the popup on success;
    /// on a store/git error the popup closes and the error lands in the status.
    fn do_move(&mut self) {
        let Some(picker) = self.move_picker.as_ref() else {
            return;
        };
        let task_id = picker.task_id;
        let filtered = picker.filtered();
        let Some(candidate) = filtered.get(picker.selected) else {
            self.status = Some("move error: no candidate selected".to_owned());
            return;
        };
        let new_parent = candidate.task_id;

        self.move_picker = None;
        self.mode = Mode::Normal;

        let result = self.repo.transaction(|store, vcs, root| {
            let mut t = store.get_task(task_id)?;
            t.parent_id = new_parent;
            t.touch();
            store.save_task(&t)?;
            let path = crate::core::storage::task_path(root, &t);
            vcs.commit(&[path], &format!("next: move {}", t.title))?;
            Ok(())
        });
        match result {
            Ok(()) => {
                self.repo.record_task_event("move", task_id);
                self.reload_keep(task_id, "moved".to_owned());
            }
            Err(e) => self.status = Some(format!("move error: {e}")),
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

/// Spawns the system URL opener (`open` on macOS, `xdg-open` elsewhere) for
/// `url`, mirroring the CLI `open` command. Returns an error if the opener
/// cannot be launched or exits non-zero.
fn open_url(url: &str) -> anyhow::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let status = std::process::Command::new(opener)
        .arg(url)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to run {opener}: {e}"))?;
    if !status.success() {
        anyhow::bail!("{opener} exited with status {status}");
    }
    Ok(())
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

    // ── keybound actions (T7) ───────────────────────────────────────────────

    /// Creates `title` via the shared service over an existing repo, committing
    /// it so it is mutable. Returns its id.
    fn commit_task(app: &mut App, title: &str) -> uuid::Uuid {
        let repo_root = app.repo.repo_root.clone();
        let task = crate::core::service::create_task(
            title.to_owned(),
            crate::core::service::CreateTaskParams::default(),
            app.today,
            &repo_root,
            &mut *app.repo.store,
            &*app.repo.vcs,
        )
        .unwrap();
        task.id
    }

    #[test]
    fn done_removes_task_from_default_view() {
        let (mut app, id) = app_with_committed_task("finish me");
        assert_eq!(app.tasks().len(), 1);
        app.update(Action::Done);
        // Done tasks leave the default (implicitly-filtered) view.
        assert!(app.tasks().iter().all(|s| s.task.id != id));
        assert_eq!(app.status().unwrap(), "completed");
        // The store records it as done.
        assert_eq!(
            app.repo.store.get_task(id).unwrap().status,
            crate::core::domain::task::Status::Done
        );
    }

    #[test]
    fn done_with_recurrence_spawns_next_and_reports_it() {
        let mut repo = test_repo();
        let today = NaiveDate::from_ymd_opt(TODAY.0, TODAY.1, TODAY.2).unwrap();
        let params = crate::core::service::CreateTaskParams {
            recurrence: Some(crate::core::domain::task::Recurrence::Completion {
                interval_days: 7,
                snap: None,
            }),
            ..Default::default()
        };
        let task = crate::core::service::create_task(
            "recurring".to_owned(),
            params,
            today,
            &repo.repo_root.clone(),
            &mut *repo.store,
            &*repo.vcs,
        )
        .unwrap();
        let mut app = App::new(Config::default(), repo, ConfigSource::Default, today);
        app.reload().unwrap();

        app.update(Action::Done);
        assert_eq!(app.status().unwrap(), "completed; spawned next occurrence");
        // Original is done; a fresh open instance now exists in the store.
        let all = app.repo.store.list_tasks().unwrap();
        assert!(all.iter().any(|t| t.id == task.id
            && t.status == crate::core::domain::task::Status::Done));
        assert!(all.iter().any(|t| t.id != task.id
            && t.status == crate::core::domain::task::Status::Open
            && t.title == "recurring"));
    }

    #[test]
    fn cancel_marks_task_cancelled() {
        let (mut app, id) = app_with_committed_task("scrap me");
        app.update(Action::Cancel);
        assert_eq!(app.status().unwrap(), "cancelled");
        assert_eq!(
            app.repo.store.get_task(id).unwrap().status,
            crate::core::domain::task::Status::Cancelled
        );
    }

    #[test]
    fn toggle_start_starts_then_stops() {
        use crate::core::domain::task::Status;
        let (mut app, id) = app_with_committed_task("work on me");
        // First toggle starts it.
        app.update(Action::ToggleStart);
        assert_eq!(app.status().unwrap(), "started");
        assert_eq!(app.repo.store.get_task(id).unwrap().status, Status::Started);
        // Selection follows the still-visible task.
        assert_eq!(app.selected_task().unwrap().id, id);
        // Second toggle stops it (back to open).
        app.update(Action::ToggleStart);
        assert_eq!(app.status().unwrap(), "stopped");
        assert_eq!(app.repo.store.get_task(id).unwrap().status, Status::Open);
    }

    #[test]
    fn delete_requires_confirm_then_removes_task() {
        let (mut app, id) = app_with_committed_task("delete me");
        // Opening the popup does not delete.
        app.update(Action::OpenDelete);
        assert_eq!(app.mode(), Mode::ConfirmDelete);
        assert!(app.repo.store.get_task(id).is_ok());

        // Cancelling leaves it intact.
        app.update(Action::CancelDelete);
        assert_eq!(app.mode(), Mode::Normal);
        assert!(app.repo.store.get_task(id).is_ok());

        // Confirming removes it.
        app.update(Action::OpenDelete);
        app.update(Action::ConfirmDelete);
        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.status().unwrap(), "deleted");
        assert!(app.repo.store.get_task(id).is_err());
        assert!(app.tasks().is_empty());
    }

    #[test]
    fn move_reparents_task() {
        let (mut app, child_id) = app_with_committed_task("child");
        let parent_id = commit_task(&mut app, "parent");
        app.reload().unwrap();
        // Select the child.
        let idx = app.tasks().iter().position(|s| s.task.id == child_id).unwrap();
        app.selected = idx;

        app.update(Action::OpenMove);
        assert_eq!(app.mode(), Mode::MovePicker);
        // Highlight the "parent" candidate.
        let pos = app
            .move_picker()
            .unwrap()
            .filtered()
            .iter()
            .position(|c| c.task_id == Some(parent_id))
            .unwrap();
        app.move_picker.as_mut().unwrap().selected = pos;
        app.update(Action::MoveConfirm);

        assert_eq!(app.status().unwrap(), "moved");
        assert_eq!(app.repo.store.get_task(child_id).unwrap().parent_id, Some(parent_id));
    }

    #[test]
    fn move_to_top_level_clears_parent() {
        let (mut app, parent_id) = app_with_committed_task("parent");
        let child_id = commit_task(&mut app, "child");
        // Reparent the child under the parent directly in the store.
        app.repo
            .transaction(|store, vcs, root| {
                let mut t = store.get_task(child_id)?;
                t.parent_id = Some(parent_id);
                store.save_task(&t)?;
                let path = crate::core::storage::task_path(root, &t);
                vcs.commit(&[path], "setup")?;
                Ok(())
            })
            .unwrap();
        app.reload().unwrap();

        let idx = app.tasks().iter().position(|s| s.task.id == child_id).unwrap();
        app.selected = idx;
        app.update(Action::OpenMove);
        // The top-level option is first.
        app.move_picker.as_mut().unwrap().selected = 0;
        assert_eq!(app.move_picker().unwrap().filtered()[0].task_id, None);
        app.update(Action::MoveConfirm);
        assert_eq!(app.repo.store.get_task(child_id).unwrap().parent_id, None);
    }

    #[test]
    fn move_picker_excludes_self_and_descendants() {
        // root → mid → leaf; moving `root` may not target root, mid, or leaf.
        let (mut app, root_id) = app_with_committed_task("root");
        let mid_id = commit_task(&mut app, "mid");
        let leaf_id = commit_task(&mut app, "leaf");
        app.repo
            .transaction(|store, vcs, root| {
                let mut mid = store.get_task(mid_id)?;
                mid.parent_id = Some(root_id);
                store.save_task(&mid)?;
                let mut leaf = store.get_task(leaf_id)?;
                leaf.parent_id = Some(mid_id);
                store.save_task(&leaf)?;
                vcs.commit(
                    &[
                        crate::core::storage::task_path(root, &mid),
                        crate::core::storage::task_path(root, &leaf),
                    ],
                    "setup tree",
                )?;
                Ok(())
            })
            .unwrap();
        app.update(Action::ToggleAll); // reveal parents with open children
        app.reload().unwrap();

        let idx = app.tasks().iter().position(|s| s.task.id == root_id).unwrap();
        app.selected = idx;
        app.update(Action::OpenMove);
        let candidates = app.move_picker().unwrap();
        // None of root/mid/leaf may appear as a candidate target.
        for banned in [root_id, mid_id, leaf_id] {
            assert!(
                candidates
                    .filtered()
                    .iter()
                    .all(|c| c.task_id != Some(banned)),
                "descendant or self leaked into candidates"
            );
        }
        // The top-level option is still available.
        assert!(candidates.filtered().iter().any(|c| c.task_id.is_none()));
    }

    #[test]
    fn move_rejects_descendant_target_no_cycle() {
        // Even if a descendant id were somehow chosen, the picker never offers
        // it; confirm the resulting parent is a non-descendant and no cycle
        // forms. Here we move `mid` (under root) to top-level and back is safe.
        let (mut app, root_id) = app_with_committed_task("root");
        let mid_id = commit_task(&mut app, "mid");
        app.repo
            .transaction(|store, vcs, root| {
                let mut mid = store.get_task(mid_id)?;
                mid.parent_id = Some(root_id);
                store.save_task(&mid)?;
                let path = crate::core::storage::task_path(root, &mid);
                vcs.commit(&[path], "setup")?;
                Ok(())
            })
            .unwrap();
        app.update(Action::ToggleAll); // reveal parents with open children
        app.reload().unwrap();

        // Moving root: mid must not be a candidate (would create root→mid→root).
        let idx = app.tasks().iter().position(|s| s.task.id == root_id).unwrap();
        app.selected = idx;
        app.update(Action::OpenMove);
        assert!(
            app.move_picker()
                .unwrap()
                .filtered()
                .iter()
                .all(|c| c.task_id != Some(mid_id))
        );
    }

    #[test]
    fn open_url_without_url_sets_error_status() {
        let (mut app, _id) = app_with_committed_task("no url");
        app.update(Action::OpenUrl);
        assert!(app.status().unwrap().contains("no URL"));
    }

    #[test]
    fn move_picker_search_filters_candidates() {
        let (mut app, child_id) = app_with_committed_task("child");
        commit_task(&mut app, "alpha parent");
        commit_task(&mut app, "beta parent");
        app.reload().unwrap();
        let idx = app.tasks().iter().position(|s| s.task.id == child_id).unwrap();
        app.selected = idx;
        app.update(Action::OpenMove);

        // Type "alpha" into the search buffer; only the alpha candidate matches.
        for ch in "alpha".chars() {
            app.update(Action::MoveInput(KeyEvent::new(
                KeyCode::Char(ch),
                KeyModifiers::NONE,
            )));
        }
        let filtered = app.move_picker().unwrap().filtered();
        assert_eq!(filtered.len(), 1);
        assert!(filtered[0].label.contains("alpha parent"));
    }

    // ── view switching + tree/forecast (T8) ─────────────────────────────────

    #[test]
    fn switch_view_changes_active_view() {
        let mut app = app_with_repo_tasks(vec![Task::new("a")]);
        assert_eq!(app.view(), View::List);
        app.update(Action::SwitchView(View::Tree));
        assert_eq!(app.view(), View::Tree);
        app.update(Action::SwitchView(View::Forecast));
        assert_eq!(app.view(), View::Forecast);
    }

    #[test]
    fn cycle_view_rotates_list_tree_forecast() {
        let mut app = app_with_repo_tasks(vec![Task::new("a")]);
        app.update(Action::CycleView);
        assert_eq!(app.view(), View::Tree);
        app.update(Action::CycleView);
        assert_eq!(app.view(), View::Forecast);
        app.update(Action::CycleView);
        assert_eq!(app.view(), View::List);
    }

    #[test]
    fn tree_view_resolves_selected_task() {
        let (mut app, id) = app_with_committed_task("tree task");
        // Entering the tree view seeds the highlight from the list selection.
        app.update(Action::SwitchView(View::Tree));
        assert_eq!(app.view(), View::Tree);
        // The selected task resolves via the highlighted tree node.
        assert_eq!(app.selected_task().map(|t| t.id), Some(id));
        // Tree-mode actions operate on it: start the highlighted node.
        app.update(Action::ToggleStart);
        assert_eq!(
            app.repo.store.get_task(id).unwrap().status,
            crate::core::domain::task::Status::Started
        );
    }

    #[test]
    fn tree_toggle_all_includes_done_tasks() {
        // A done task is hidden from the tree until include-all is toggled.
        let (mut app, id) = app_with_committed_task("done one");
        app.repo
            .transaction(|store, vcs, root| {
                let mut t = store.get_task(id)?;
                t.mark_done();
                store.save_task(&t)?;
                let path = crate::core::storage::task_path(root, &t);
                vcs.commit(&[path], "done")?;
                Ok(())
            })
            .unwrap();
        app.reload().unwrap();
        app.update(Action::SwitchView(View::Tree));

        assert!(!app.tree_include_all());
        assert!(
            app.tree_items().is_empty(),
            "done task hidden without include-all"
        );
        app.update(Action::TreeToggleAll);
        assert!(app.tree_include_all());
        assert_eq!(app.tree_items().len(), 1, "done task visible with include-all");
    }

    #[test]
    fn forecast_entries_respect_filter_tokens() {
        let mut tagged = Task::new("tagged");
        tagged.due = Some(NaiveDate::from_ymd_opt(TODAY.0, TODAY.1, TODAY.2).unwrap());
        tagged.tags = vec!["#rust".to_owned()];
        let mut plain = Task::new("plain");
        plain.due = Some(NaiveDate::from_ymd_opt(TODAY.0, TODAY.1, TODAY.2).unwrap());
        let mut app = app_with_repo_tasks(vec![tagged, plain]);

        // No filter: both due-today tasks forecast.
        let titles: Vec<String> = app.forecast_entries().into_iter().map(|e| e.title).collect();
        assert!(titles.contains(&"tagged".to_owned()));
        assert!(titles.contains(&"plain".to_owned()));

        // Require #rust: only the tagged task survives in the forecast.
        app.filter_tokens = vec!["+#rust".to_owned()];
        app.reload().unwrap();
        let titles: Vec<String> = app.forecast_entries().into_iter().map(|e| e.title).collect();
        assert_eq!(titles, vec!["tagged".to_owned()]);
    }

    #[test]
    fn forecast_horizon_adjust_actions() {
        let mut app = app_with_repo_tasks(Vec::new());
        app.update(Action::SwitchView(View::Forecast));
        let base = app.forecast_horizon();
        app.update(Action::ForecastWiden);
        assert_eq!(app.forecast_horizon(), base + 30);
        app.update(Action::ForecastNarrow);
        assert_eq!(app.forecast_horizon(), base);
    }
}
