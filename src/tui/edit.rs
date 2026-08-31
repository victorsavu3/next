//! The edit-modal form state for the TUI.
//!
//! [`EditForm`] is a self-contained snapshot of an editable task: every field is
//! seeded from the task when the modal opens, edited locally, and turned back
//! into a [`EditTaskParams`] on save (via [`EditForm::to_edit_params`]). Applying
//! those params, plus the out-of-band `data` edits, lives in
//! [`super::app::App`] so this module stays free of store/VCS concerns and is
//! cheap to unit-test.
//!
//! Clear semantics: each optional text field remembers whether the task
//! originally had a value. If it did and the field is now empty, the matching
//! `clear_*` flag is set; if it was empty and stays empty, nothing happens
//! (no-op). Title is required (non-empty). Tags are edited as an explicit
//! add/remove set diff against the original tag list.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;
use tui_textarea::TextArea;

use crate::core::domain::date_parse::parse_date;
use crate::core::domain::task::{Priority, Recurrence, Task};
use crate::core::recurrence::{parse_recurrence, snap_leeway_to_str};
use crate::core::service::EditTaskParams;

/// Which recurrence flavour the sub-form is editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecurMode {
    /// No recurrence (clears any existing rule on save).
    None,
    /// Schedule-based (RRULE + anchor).
    Schedule,
    /// Completion-based (interval days).
    Completion,
}

impl RecurMode {
    fn label(self) -> &'static str {
        match self {
            RecurMode::None => "none",
            RecurMode::Schedule => "schedule",
            RecurMode::Completion => "completion",
        }
    }

    /// Cycle to the next flavour (None → Schedule → Completion → None).
    fn next(self) -> Self {
        match self {
            RecurMode::None => RecurMode::Schedule,
            RecurMode::Schedule => RecurMode::Completion,
            RecurMode::Completion => RecurMode::None,
        }
    }
}

/// Sub-mode for the tag editor while [`Field::Tags`] is focused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagMode {
    /// Browsing / removing existing tags. The cursor selects one chip.
    List,
    /// Typing a new tag with autocompletion from the repo's known tags.
    Add,
}

/// The logical fields of the form, in navigation order. The `Tab`/`Shift-Tab`
/// keys move between adjacent variants; the modal renders one row per field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Due,
    Start,
    Priority,
    Tags,
    Assignee,
    Url,
    ScoreAdjustment,
    LongTerm,
    RecurMode,
    RecurRule,
    RecurCompletion,
    RecurSnap,
    RecurSnapLeeway,
    Description,
    Notes,
    DataKey,
    DataValue,
}

impl Field {
    /// Field navigation order.
    const ORDER: [Field; 18] = [
        Field::Title,
        Field::Due,
        Field::Start,
        Field::Priority,
        Field::Tags,
        Field::Assignee,
        Field::Url,
        Field::ScoreAdjustment,
        Field::LongTerm,
        Field::RecurMode,
        Field::RecurRule,
        Field::RecurCompletion,
        Field::RecurSnap,
        Field::RecurSnapLeeway,
        Field::Description,
        Field::Notes,
        Field::DataKey,
        Field::DataValue,
    ];

    fn index(self) -> usize {
        Self::ORDER.iter().position(|f| *f == self).unwrap()
    }

    fn next(self) -> Self {
        let i = self.index();
        Self::ORDER[(i + 1) % Self::ORDER.len()]
    }

    fn prev(self) -> Self {
        let i = self.index();
        Self::ORDER[(i + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }
}

/// Editable form state for a single task. Built with [`EditForm::from_task`].
pub struct EditForm {
    /// Id of the task being edited; used by the apply step.
    pub task_id: uuid::Uuid,
    /// The currently focused field.
    pub focus: Field,

    pub title: Input,
    pub due: Input,
    pub start: Input,
    pub priority: Priority,
    /// Sub-mode for the tag field (browsing existing vs. typing new).
    pub tag_mode: TagMode,
    /// Selected chip index in [`TagMode::List`].
    pub tag_cursor: usize,
    /// Free-entry tag input; used while in [`TagMode::Add`].
    pub tag_input: Input,
    /// Filtered autocomplete suggestions shown while in [`TagMode::Add`].
    pub tag_suggestions: Vec<String>,
    /// Selected suggestion index in [`TagMode::Add`].
    pub suggestion_cursor: usize,
    /// The working tag list (mutated by add/remove during editing).
    pub tags: Vec<String>,
    pub assignee: Input,
    pub url: Input,
    pub score_adjustment: Input,
    pub long_term: bool,

    pub recur_mode: RecurMode,
    pub recur_rule: Input,
    pub recur_completion: Input,
    pub recur_snap: Input,
    pub recur_snap_leeway: Input,

    pub description: TextArea<'static>,
    pub notes: TextArea<'static>,

    /// Working copy of the task's data map (sorted for stable display).
    pub data: BTreeMap<String, serde_json::Value>,
    pub data_key: Input,
    pub data_value: Input,

    /// All tags known to exist in the repository; used for autocomplete.
    known_tags: Vec<String>,

    // ── Originals, for clear-semantics & recurrence anchor preservation ──────
    orig_due: Option<NaiveDate>,
    orig_start: Option<NaiveDate>,
    orig_assignee: Option<String>,
    orig_url: Option<String>,
    orig_description: Option<String>,
    orig_notes: Option<String>,
    orig_tags: Vec<String>,
    orig_recurrence: Option<Recurrence>,
    orig_data: BTreeMap<String, serde_json::Value>,
}

impl EditForm {
    /// Seeds a form from `task`. Every field is populated from the task's
    /// current value (dates rendered as ISO so the round-trip is loss-free).
    ///
    /// `known_tags` is the full list of tags that exist in the repository,
    /// used to power the tag autocomplete in [`TagMode::Add`].
    pub fn from_task(task: &Task, known_tags: Vec<String>) -> Self {
        let date_str = |d: Option<NaiveDate>| d.map(|d| d.to_string()).unwrap_or_default();

        // Blank means unset for both snap fields, so a form the user never
        // touches writes back exactly what it was seeded from.
        let leeway_str = task
            .recurrence
            .as_ref()
            .and_then(Recurrence::snap_leeway)
            .map(snap_leeway_to_str)
            .unwrap_or_default();
        let (recur_mode, recur_rule, recur_completion, recur_snap) = match &task.recurrence {
            Some(Recurrence::Schedule { rrule, snap, .. }) => (
                RecurMode::Schedule,
                rrule.clone(),
                String::new(),
                snap_to_str(snap),
            ),
            Some(Recurrence::Completion {
                interval_days,
                snap,
                ..
            }) => (
                RecurMode::Completion,
                String::new(),
                interval_days.to_string(),
                snap_to_str(snap),
            ),
            None => (RecurMode::None, String::new(), String::new(), String::new()),
        };

        let mut description = TextArea::from(task.description.clone().unwrap_or_default().lines());
        description.set_cursor_line_style(ratatui::style::Style::default());
        let mut notes = TextArea::from(task.notes.clone().unwrap_or_default().lines());
        notes.set_cursor_line_style(ratatui::style::Style::default());

        let data: BTreeMap<String, serde_json::Value> = task
            .data
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let score_adjustment = if task.score_adjustment == 0.0 {
            String::new()
        } else {
            task.score_adjustment.to_string()
        };

        Self {
            task_id: task.id,
            focus: Field::Title,
            title: Input::new(task.title.clone()),
            due: Input::new(date_str(task.due)),
            start: Input::new(date_str(task.start)),
            priority: task.priority.clone(),
            tag_mode: TagMode::List,
            tag_cursor: 0,
            tag_input: Input::default(),
            tag_suggestions: Vec::new(),
            suggestion_cursor: 0,
            tags: task.tags.clone(),
            assignee: Input::new(task.assignee.clone().unwrap_or_default()),
            url: Input::new(task.url.clone().unwrap_or_default()),
            score_adjustment: Input::new(score_adjustment),
            long_term: task.long_term,
            recur_mode,
            recur_rule: Input::new(recur_rule),
            recur_completion: Input::new(recur_completion),
            recur_snap: Input::new(recur_snap),
            recur_snap_leeway: Input::new(leeway_str),
            description,
            notes,
            data: data.clone(),
            data_key: Input::default(),
            data_value: Input::default(),
            known_tags,
            orig_due: task.due,
            orig_start: task.start,
            orig_assignee: task.assignee.clone(),
            orig_url: task.url.clone(),
            orig_description: task.description.clone(),
            orig_notes: task.notes.clone(),
            orig_tags: task.tags.clone(),
            orig_recurrence: task.recurrence.clone(),
            orig_data: data,
        }
    }

    // ── Navigation ───────────────────────────────────────────────────────────

    /// Move focus to the next field.
    pub fn focus_next(&mut self) {
        self.leave_tag_add_mode();
        self.focus = self.focus.next();
    }

    /// Move focus to the previous field.
    pub fn focus_prev(&mut self) {
        self.leave_tag_add_mode();
        self.focus = self.focus.prev();
    }

    /// Whether the tag editor is in Add mode with suggestions available,
    /// meaning Up/Down should navigate suggestions rather than move fields.
    pub fn tags_wants_vertical_nav(&self) -> bool {
        self.focus == Field::Tags
            && self.tag_mode == TagMode::Add
            && !self.tag_suggestions.is_empty()
    }

    fn leave_tag_add_mode(&mut self) {
        if self.focus == Field::Tags && self.tag_mode == TagMode::Add {
            self.tag_input = Input::default();
            self.tag_suggestions.clear();
            self.suggestion_cursor = 0;
            self.tag_mode = TagMode::List;
        }
    }

    /// Whether the focused field is a multi-line textarea (so the modal knows
    /// `Tab` must still move fields rather than insert a tab).
    pub fn focus_is_multiline(&self) -> bool {
        matches!(self.focus, Field::Description | Field::Notes)
    }

    /// The current recurrence mode as a display label.
    pub fn recur_mode_label(&self) -> &'static str {
        self.recur_mode.label()
    }

    // ── Per-field key handling ────────────────────────────────────────────────

    /// Feeds a key event to the currently focused field. Returns `true` when the
    /// key was consumed as field editing. Field navigation (Tab) and modal-level
    /// keys (Esc/Ctrl-S) are handled by the caller before this is reached.
    pub fn handle_field_key(&mut self, key: KeyEvent) {
        let ev = ratatui::crossterm::event::Event::Key(key);
        match self.focus {
            Field::Title => {
                self.title.handle_event(&ev);
            }
            Field::Due => {
                self.due.handle_event(&ev);
            }
            Field::Start => {
                self.start.handle_event(&ev);
            }
            Field::Priority => self.handle_priority_key(key),
            Field::Tags => self.handle_tags_key(key, &ev),
            Field::Assignee => {
                self.assignee.handle_event(&ev);
            }
            Field::Url => {
                self.url.handle_event(&ev);
            }
            Field::ScoreAdjustment => {
                self.score_adjustment.handle_event(&ev);
            }
            Field::LongTerm => self.handle_long_term_key(key),
            Field::RecurMode => self.handle_recur_mode_key(key),
            Field::RecurRule => {
                self.recur_rule.handle_event(&ev);
            }
            Field::RecurCompletion => {
                self.recur_completion.handle_event(&ev);
            }
            Field::RecurSnap => {
                self.recur_snap.handle_event(&ev);
            }
            Field::RecurSnapLeeway => {
                self.recur_snap_leeway.handle_event(&ev);
            }
            Field::Description => {
                self.description.input(key);
            }
            Field::Notes => {
                self.notes.input(key);
            }
            Field::DataKey => {
                self.data_key.handle_event(&ev);
            }
            Field::DataValue => self.handle_data_value_key(key, &ev),
        }
    }

    fn handle_priority_key(&mut self, key: KeyEvent) {
        // Space / Left / Right cycle through the three priorities.
        match key.code {
            KeyCode::Char(' ') | KeyCode::Right => {
                self.priority = match self.priority {
                    Priority::Low => Priority::Medium,
                    Priority::Medium => Priority::High,
                    Priority::High => Priority::Low,
                };
            }
            KeyCode::Left => {
                self.priority = match self.priority {
                    Priority::Low => Priority::High,
                    Priority::Medium => Priority::Low,
                    Priority::High => Priority::Medium,
                };
            }
            _ => {}
        }
    }

    fn handle_long_term_key(&mut self, key: KeyEvent) {
        if matches!(
            key.code,
            KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
        ) {
            self.long_term = !self.long_term;
        }
    }

    fn handle_recur_mode_key(&mut self, key: KeyEvent) {
        if matches!(
            key.code,
            KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
        ) {
            self.recur_mode = self.recur_mode.next();
        }
    }

    fn handle_tags_key(&mut self, key: KeyEvent, ev: &ratatui::crossterm::event::Event) {
        match self.tag_mode {
            TagMode::List => self.handle_tags_list_key(key),
            TagMode::Add => self.handle_tags_add_key(key, ev),
        }
    }

    fn handle_tags_list_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Left => {
                self.tag_cursor = self.tag_cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                if !self.tags.is_empty() {
                    self.tag_cursor = (self.tag_cursor + 1).min(self.tags.len() - 1);
                }
            }
            KeyCode::Delete | KeyCode::Backspace => {
                if !self.tags.is_empty() {
                    self.tags.remove(self.tag_cursor);
                    if self.tag_cursor >= self.tags.len() {
                        self.tag_cursor = self.tags.len().saturating_sub(1);
                    }
                }
            }
            // Any printable character seeds the Add mode input.
            KeyCode::Char(c) => {
                self.tag_mode = TagMode::Add;
                self.tag_input = Input::new(c.to_string());
                self.rebuild_suggestions();
            }
            KeyCode::Enter => {
                self.tag_mode = TagMode::Add;
                self.tag_input = Input::default();
                self.rebuild_suggestions();
            }
            _ => {}
        }
    }

    fn handle_tags_add_key(&mut self, key: KeyEvent, ev: &ratatui::crossterm::event::Event) {
        match key.code {
            KeyCode::Up => {
                self.suggestion_cursor = self.suggestion_cursor.saturating_sub(1);
            }
            KeyCode::Down => {
                if !self.tag_suggestions.is_empty() {
                    self.suggestion_cursor =
                        (self.suggestion_cursor + 1).min(self.tag_suggestions.len() - 1);
                }
            }
            KeyCode::Enter => {
                let tag = if !self.tag_suggestions.is_empty() {
                    self.tag_suggestions[self.suggestion_cursor].clone()
                } else {
                    self.tag_input.value().trim().to_owned()
                };
                if !tag.is_empty() && !self.tags.contains(&tag) {
                    self.tags.push(tag);
                    self.tag_cursor = self.tags.len() - 1;
                }
                self.tag_input = Input::default();
                self.tag_suggestions.clear();
                self.suggestion_cursor = 0;
                self.tag_mode = TagMode::List;
            }
            _ => {
                self.tag_input.handle_event(ev);
                // Rebuild suggestions and reset the selection after every keystroke.
                self.rebuild_suggestions();
                self.suggestion_cursor = 0;
            }
        }
    }

    fn rebuild_suggestions(&mut self) {
        let input = self.tag_input.value().to_lowercase();
        self.tag_suggestions = self
            .known_tags
            .iter()
            .filter(|t| !self.tags.contains(t))
            .filter(|t| input.is_empty() || t.to_lowercase().contains(input.as_str()))
            .take(8)
            .cloned()
            .collect();
    }

    fn handle_data_value_key(&mut self, key: KeyEvent, ev: &ratatui::crossterm::event::Event) {
        match key.code {
            // Enter commits the key/value pair into the working data map.
            KeyCode::Enter => {
                let k = self.data_key.value().trim().to_owned();
                if !k.is_empty() {
                    let raw = self.data_value.value();
                    let value = crate::core::parse_value(raw)
                        .unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()));
                    self.data.insert(k, value);
                    self.data_key = Input::default();
                    self.data_value = Input::default();
                }
            }
            // Ctrl-D deletes the entry named by the key field, if present.
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let k = self.data_key.value().trim().to_owned();
                if !k.is_empty() {
                    self.data.remove(&k);
                }
            }
            _ => {
                self.data_value.handle_event(ev);
            }
        }
    }

    // ── Date previews (rendered next to the date fields) ──────────────────────

    /// Parsed preview of the `due` text against `today`, or an error string. An
    /// empty input previews as `None`.
    pub fn due_preview(&self, today: NaiveDate) -> Result<Option<NaiveDate>, String> {
        preview_date(self.due.value(), today)
    }

    /// Parsed preview of the `start` text against `today`.
    pub fn start_preview(&self, today: NaiveDate) -> Result<Option<NaiveDate>, String> {
        preview_date(self.start.value(), today)
    }

    // ── Save: build EditTaskParams ────────────────────────────────────────────

    /// Derives [`EditTaskParams`] from the current form state, resolving dates
    /// (`today` is the base for natural-language parsing), diffing tags, and
    /// building the recurrence rule with anchor preservation.
    ///
    /// Validation of url/tags and the recurrence rule itself is performed here so
    /// the caller surfaces a single error; the data-map changes are returned
    /// separately via [`EditForm::data_changes`] because [`EditTaskParams`] has
    /// no `data` field.
    pub fn to_edit_params(&self, today: NaiveDate) -> anyhow::Result<EditTaskParams> {
        // Title is required.
        let title = self.title.value().trim();
        if title.is_empty() {
            anyhow::bail!("title must not be empty");
        }

        let (due, clear_due) = resolve_optional_date(self.due.value(), self.orig_due, today)?;
        let (start, clear_start) =
            resolve_optional_date(self.start.value(), self.orig_start, today)?;

        let (assignee, clear_assignee) =
            resolve_optional_text(self.assignee.value(), &self.orig_assignee);

        let (url, clear_url) = resolve_optional_text(self.url.value(), &self.orig_url);
        if let Some(ref u) = url {
            crate::core::service::validate_url(u)?;
        }

        // Description / notes from the textareas.
        let desc_text = textarea_text(&self.description);
        let (description, clear_description) =
            resolve_optional_text(&desc_text, &self.orig_description);

        // `notes` has no clear flag in EditTaskParams; the CLI always sets it
        // when provided. Mirror that: pass the (possibly empty) text through so
        // emptying it stores an empty string, matching `--notes ""`.
        let notes_text = textarea_text(&self.notes);
        let notes = if notes_text == self.orig_notes.clone().unwrap_or_default() {
            None
        } else {
            Some(notes_text)
        };

        // Tag diff against the original list.
        let add_tags: Vec<String> = self
            .tags
            .iter()
            .filter(|t| !self.orig_tags.contains(t))
            .cloned()
            .collect();
        let remove_tags: Vec<String> = self
            .orig_tags
            .iter()
            .filter(|t| !self.tags.contains(t))
            .cloned()
            .collect();
        for t in &add_tags {
            crate::core::domain::tag::validate_tag(t).map_err(|e| anyhow::anyhow!(e))?;
        }

        // Score adjustment (numeric text). Only set when it differs from the
        // original; an empty field means "leave unchanged".
        let score_adjustment = {
            let raw = self.score_adjustment.value().trim();
            if raw.is_empty() {
                None
            } else {
                let v: f64 = raw
                    .parse()
                    .map_err(|_| anyhow::anyhow!("score adjustment must be a number"))?;
                Some(v)
            }
        };

        // The form always knows the current boolean, so send it unconditionally;
        // `apply_edits` treats `Some(x)` as "set to x" (an idempotent no-op when
        // unchanged).
        let long_term = Some(self.long_term);

        // Recurrence build with anchor preservation.
        let (recurrence, clear_recurrence) = self.build_recurrence(today)?;

        Ok(EditTaskParams {
            title: Some(title.to_owned()),
            due,
            clear_due,
            start,
            clear_start,
            priority: Some(self.priority.to_string()),
            slug: None,
            assignee,
            clear_assignee,
            add_tags,
            remove_tags,
            parent: None,
            clear_parent: false,
            blocked_by: Vec::new(),
            clear_blocked_by: false,
            description,
            clear_description,
            url,
            clear_url,
            notes,
            recurrence,
            clear_recurrence,
            long_term,
            score_adjustment,
        })
    }

    /// Builds the recurrence update, preserving the existing schedule anchor when
    /// the rule is still schedule-based. Returns `(recurrence, clear_recurrence)`.
    fn build_recurrence(&self, today: NaiveDate) -> anyhow::Result<(Option<Recurrence>, bool)> {
        let snap = blank_is_unset(self.recur_snap.value());
        // A blank field is "no leeway", which is also what clearing one means,
        // so the form has no separate clear flag to pass.
        let leeway = blank_is_unset(self.recur_snap_leeway.value());
        match self.recur_mode {
            RecurMode::None => Ok((None, true)),
            RecurMode::Schedule => {
                let rule = self.recur_rule.value().trim();
                if rule.is_empty() {
                    anyhow::bail!("recurrence schedule requires an RRULE");
                }
                // Preserve the anchor if the task already had a schedule rule;
                // otherwise anchor on start/due/today (mirrors the CLI).
                let anchor = match &self.orig_recurrence {
                    Some(Recurrence::Schedule { anchor, .. }) => *anchor,
                    _ => self.resolved_anchor_date(today).unwrap_or(today),
                };
                let rec =
                    parse_recurrence(Some(rule.to_owned()), None, snap, leeway, false, anchor)?;
                Ok((rec, false))
            }
            RecurMode::Completion => {
                let raw = self.recur_completion.value().trim();
                let interval: u32 = raw.parse().map_err(|_| {
                    anyhow::anyhow!("completion interval must be a positive integer")
                })?;
                let rec = parse_recurrence(None, Some(interval), snap, leeway, false, today)?;
                Ok((rec, false))
            }
        }
    }

    /// The start/due date currently in the form (parsed), used as a fallback
    /// anchor for a newly-added schedule rule.
    fn resolved_anchor_date(&self, today: NaiveDate) -> Option<NaiveDate> {
        if let Ok(Some(d)) = preview_date(self.start.value(), today) {
            return Some(d);
        }
        if let Ok(Some(d)) = preview_date(self.due.value(), today) {
            return Some(d);
        }
        None
    }

    /// The set of data-map mutations to apply after `apply_edits`, as
    /// `(key, Some(value))` for set/insert and `(key, None)` for delete. Diffed
    /// against the original map so unchanged entries are skipped.
    pub fn data_changes(&self) -> Vec<(String, Option<serde_json::Value>)> {
        let mut changes = Vec::new();
        // Sets / updates.
        for (k, v) in &self.data {
            if self.orig_data.get(k) != Some(v) {
                changes.push((k.clone(), Some(v.clone())));
            }
        }
        // Deletions.
        for k in self.orig_data.keys() {
            if !self.data.contains_key(k) {
                changes.push((k.clone(), None));
            }
        }
        changes
    }
}

/// A trimmed form field, or `None` when the user left it empty — the form's
/// spelling of "unset" for the two optional recurrence strings.
fn blank_is_unset(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Renders a [`Recurrence`] `Snap` back into the CLI snap string the
/// [`parse_recurrence`] parser accepts (round-trips the form value).
fn snap_to_str(snap: &Option<crate::core::domain::task::Snap>) -> String {
    use crate::core::domain::task::Snap;
    match snap {
        None => String::new(),
        Some(Snap::NextWorkday) => "next-workday".to_owned(),
        Some(Snap::NextWeekday { weekday }) => ["mon", "tue", "wed", "thu", "fri", "sat", "sun"]
            .get(*weekday as usize)
            .copied()
            .unwrap_or("mon")
            .to_owned(),
        Some(Snap::DayOfMonth { day }) => format!("dom:{day}"),
    }
}

/// Joins a textarea's lines into a single `\n`-delimited string.
fn textarea_text(ta: &TextArea) -> String {
    ta.lines().join("\n")
}

/// Parses a date preview: empty → `Ok(None)`, otherwise the parsed date or the
/// parser's error message.
fn preview_date(raw: &str, today: NaiveDate) -> Result<Option<NaiveDate>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    parse_date(raw, today).map(Some).map_err(|e| e.to_string())
}

/// Resolves an optional date field into `(value, clear_flag)`:
/// - empty + originally set → `(None, true)` (clear)
/// - empty + originally unset → `(None, false)` (no-op)
/// - non-empty → `(Some(parsed), false)`
fn resolve_optional_date(
    raw: &str,
    orig: Option<NaiveDate>,
    today: NaiveDate,
) -> anyhow::Result<(Option<NaiveDate>, bool)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok((None, orig.is_some()));
    }
    let d = parse_date(raw, today)?;
    Ok((Some(d), false))
}

/// Resolves an optional text field into `(value, clear_flag)` with the same
/// clear-semantics as [`resolve_optional_date`]. A non-empty value that equals
/// the original is still sent (harmless idempotent set), keeping the logic
/// simple; only the emptied-a-set-field case toggles the clear flag.
fn resolve_optional_text(raw: &str, orig: &Option<String>) -> (Option<String>, bool) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        (None, orig.is_some())
    } else {
        (Some(trimmed.to_owned()), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::domain::task::{Recurrence, Snap, SnapLeeway, Task};

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn today() -> NaiveDate {
        d(2026, 6, 6)
    }

    #[test]
    fn from_task_seeds_fields() {
        let mut task = Task::new("Original");
        task.due = Some(d(2026, 7, 1));
        task.priority = Priority::High;
        task.tags = vec!["#rust".into()];
        task.assignee = Some("alice".into());
        task.url = Some("https://example.com".into());
        task.description = Some("line1\nline2".into());
        let form = EditForm::from_task(&task, vec![]);
        assert_eq!(form.title.value(), "Original");
        assert_eq!(form.due.value(), "2026-07-01");
        assert_eq!(form.priority, Priority::High);
        assert_eq!(form.tags, vec!["#rust".to_owned()]);
        assert_eq!(form.assignee.value(), "alice");
        assert_eq!(form.url.value(), "https://example.com");
        assert_eq!(textarea_text(&form.description), "line1\nline2");
    }

    #[test]
    fn unchanged_form_produces_noop_params() {
        let mut task = Task::new("Title");
        task.due = Some(d(2026, 7, 1));
        let form = EditForm::from_task(&task, vec![]);
        let p = form.to_edit_params(today()).unwrap();
        // No clear flags for unchanged set fields.
        assert!(!p.clear_due);
        assert!(!p.clear_assignee);
        assert!(!p.clear_url);
        assert!(!p.clear_description);
        // Due passes through as the same date (idempotent set).
        assert_eq!(p.due, Some(d(2026, 7, 1)));
        assert!(p.add_tags.is_empty());
        assert!(p.remove_tags.is_empty());
    }

    #[test]
    fn emptying_a_set_field_sets_clear_flag() {
        let mut task = Task::new("Title");
        task.due = Some(d(2026, 7, 1));
        task.assignee = Some("alice".into());
        task.url = Some("https://example.com".into());
        task.description = Some("desc".into());
        let mut form = EditForm::from_task(&task, vec![]);
        form.due = Input::default();
        form.assignee = Input::default();
        form.url = Input::default();
        form.description = TextArea::default();
        let p = form.to_edit_params(today()).unwrap();
        assert!(p.clear_due);
        assert!(p.clear_assignee);
        assert!(p.clear_url);
        assert!(p.clear_description);
        assert_eq!(p.due, None);
        assert_eq!(p.assignee, None);
    }

    #[test]
    fn emptying_an_unset_field_is_noop() {
        let task = Task::new("Title");
        let form = EditForm::from_task(&task, vec![]);
        let p = form.to_edit_params(today()).unwrap();
        assert!(!p.clear_due);
        assert!(!p.clear_assignee);
        assert!(!p.clear_url);
    }

    #[test]
    fn empty_title_is_error() {
        let task = Task::new("Title");
        let mut form = EditForm::from_task(&task, vec![]);
        form.title = Input::new("   ".to_owned());
        assert!(form.to_edit_params(today()).is_err());
    }

    #[test]
    fn tag_diff_add_and_remove() {
        let mut task = Task::new("Title");
        task.tags = vec!["@work".into(), "#laptop".into()];
        let mut form = EditForm::from_task(&task, vec![]);
        // Remove #laptop, add urgent.
        form.tags = vec!["@work".into(), "urgent".into()];
        let p = form.to_edit_params(today()).unwrap();
        assert_eq!(p.add_tags, vec!["urgent".to_owned()]);
        assert_eq!(p.remove_tags, vec!["#laptop".to_owned()]);
    }

    #[test]
    fn invalid_added_tag_is_error() {
        let task = Task::new("Title");
        let mut form = EditForm::from_task(&task, vec![]);
        form.tags = vec!["bad..tag".into()];
        assert!(form.to_edit_params(today()).is_err());
    }

    #[test]
    fn invalid_url_is_error() {
        let task = Task::new("Title");
        let mut form = EditForm::from_task(&task, vec![]);
        form.url = Input::new("ftp://nope".to_owned());
        assert!(form.to_edit_params(today()).is_err());
    }

    #[test]
    fn natural_language_due_resolves() {
        let task = Task::new("Title");
        let mut form = EditForm::from_task(&task, vec![]);
        form.due = Input::new("2026-08-15".to_owned());
        let p = form.to_edit_params(today()).unwrap();
        assert_eq!(p.due, Some(d(2026, 8, 15)));
    }

    #[test]
    fn priority_cycles() {
        let task = Task::new("Title");
        let mut form = EditForm::from_task(&task, vec![]);
        form.focus = Field::Priority;
        let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        let start = form.priority.clone();
        form.handle_field_key(space);
        assert_ne!(form.priority, start);
    }

    // ── recurrence ──────────────────────────────────────────────────────────

    #[test]
    fn recurrence_none_clears() {
        let mut task = Task::new("Title");
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 7,
            snap: None,
            snap_leeway: None,
        });
        let mut form = EditForm::from_task(&task, vec![]);
        form.recur_mode = RecurMode::None;
        let p = form.to_edit_params(today()).unwrap();
        assert!(p.clear_recurrence);
        assert!(p.recurrence.is_none());
    }

    #[test]
    fn recurrence_schedule_preserves_existing_anchor() {
        let anchor = d(2026, 1, 1);
        let mut task = Task::new("Title");
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=WEEKLY;BYDAY=MO".into(),
            anchor,
            snap: None,
            snap_leeway: None,
        });
        let mut form = EditForm::from_task(&task, vec![]);
        // Change the rule but keep schedule mode → anchor must be preserved.
        form.recur_rule = Input::new("FREQ=WEEKLY;BYDAY=TU".to_owned());
        let p = form.to_edit_params(today()).unwrap();
        match p.recurrence {
            Some(Recurrence::Schedule {
                rrule, anchor: a, ..
            }) => {
                assert_eq!(rrule, "FREQ=WEEKLY;BYDAY=TU");
                assert_eq!(a, anchor, "existing anchor must be preserved");
            }
            other => panic!("expected schedule, got {other:?}"),
        }
        assert!(!p.clear_recurrence);
    }

    #[test]
    fn recurrence_new_schedule_anchors_on_due() {
        let task = Task::new("Title");
        let mut form = EditForm::from_task(&task, vec![]);
        form.recur_mode = RecurMode::Schedule;
        form.recur_rule = Input::new("FREQ=MONTHLY;BYMONTHDAY=1".to_owned());
        form.due = Input::new("2026-09-10".to_owned());
        let p = form.to_edit_params(today()).unwrap();
        match p.recurrence {
            Some(Recurrence::Schedule { anchor, .. }) => {
                assert_eq!(
                    anchor,
                    d(2026, 9, 10),
                    "new rule anchors on the form due date"
                );
            }
            other => panic!("expected schedule, got {other:?}"),
        }
    }

    #[test]
    fn recurrence_completion_builds_interval_and_snap() {
        let task = Task::new("Title");
        let mut form = EditForm::from_task(&task, vec![]);
        form.recur_mode = RecurMode::Completion;
        form.recur_completion = Input::new("14".to_owned());
        form.recur_snap = Input::new("friday".to_owned());
        let p = form.to_edit_params(today()).unwrap();
        match p.recurrence {
            Some(Recurrence::Completion {
                interval_days,
                snap,
                ..
            }) => {
                assert_eq!(interval_days, 14);
                assert_eq!(snap, Some(Snap::NextWeekday { weekday: 4 }));
            }
            other => panic!("expected completion, got {other:?}"),
        }
    }

    // ── snap leeway ─────────────────────────────────────────────────────────

    #[test]
    fn the_leeway_field_round_trips_through_the_form() {
        let mut task = Task::new("Pay the rent");
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 30,
            snap: Some(Snap::DayOfMonth { day: 1 }),
            snap_leeway: Some(SnapLeeway {
                back: 5,
                forward: Some(0),
            }),
        });
        let form = EditForm::from_task(&task, vec![]);
        assert_eq!(form.recur_snap_leeway.value(), "5,0");

        // Saving an untouched form must write back exactly what it was seeded
        // from — the field is a spec string, not a display rendering.
        let p = form.to_edit_params(today()).unwrap();
        assert_eq!(
            p.recurrence.as_ref().and_then(Recurrence::snap_leeway),
            Some(&SnapLeeway {
                back: 5,
                forward: Some(0)
            })
        );
    }

    #[test]
    fn a_task_without_a_leeway_seeds_a_blank_field() {
        let mut task = Task::new("Pay the rent");
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 30,
            snap: Some(Snap::DayOfMonth { day: 1 }),
            snap_leeway: None,
        });
        let form = EditForm::from_task(&task, vec![]);
        assert_eq!(form.recur_snap_leeway.value(), "");

        let p = form.to_edit_params(today()).unwrap();
        assert_eq!(
            p.recurrence.as_ref().and_then(Recurrence::snap_leeway),
            None
        );
    }

    /// The form inherits the shared validation rather than repeating it, so
    /// the message is the one the CLI and MCP report.
    #[test]
    fn the_form_reports_the_shared_validation_message() {
        let task = Task::new("Pay the rent");
        let mut form = EditForm::from_task(&task, vec![]);
        form.recur_mode = RecurMode::Completion;
        form.recur_completion = Input::new("30".to_owned());
        form.recur_snap_leeway = Input::new("3".to_owned());
        let err = form.to_edit_params(today()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "--recur-snap-leeway requires a snap; set --recur-snap first \
             (e.g. dom:1, monday, next-workday)"
        );
    }

    #[test]
    fn the_leeway_field_follows_the_snap_in_the_tab_order() {
        assert_eq!(Field::RecurSnap.next(), Field::RecurSnapLeeway);
        assert_eq!(Field::RecurSnapLeeway.prev(), Field::RecurSnap);
        assert_eq!(Field::RecurSnapLeeway.next(), Field::Description);
        // Every variant must be reachable, or Tab strands the user on it.
        assert_eq!(Field::ORDER.len(), 18);
    }

    #[test]
    fn recurrence_invalid_rrule_is_error() {
        let task = Task::new("Title");
        let mut form = EditForm::from_task(&task, vec![]);
        form.recur_mode = RecurMode::Schedule;
        form.recur_rule = Input::new("NONSENSE".to_owned());
        assert!(form.to_edit_params(today()).is_err());
    }

    // ── data ─────────────────────────────────────────────────────────────────

    #[test]
    fn data_changes_detects_add_update_delete() {
        let mut task = Task::new("Title");
        task.data.insert("keep".into(), serde_json::json!(1));
        task.data.insert("drop".into(), serde_json::json!("x"));
        let mut form = EditForm::from_task(&task, vec![]);
        // Add a new key, change "keep", delete "drop".
        form.data.insert("new".into(), serde_json::json!(true));
        form.data.insert("keep".into(), serde_json::json!(2));
        form.data.remove("drop");
        let mut changes = form.data_changes();
        changes.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0], ("drop".to_owned(), None));
        assert_eq!(changes[1], ("keep".to_owned(), Some(serde_json::json!(2))));
        assert_eq!(
            changes[2],
            ("new".to_owned(), Some(serde_json::json!(true)))
        );
    }

    #[test]
    fn data_changes_empty_when_unchanged() {
        let mut task = Task::new("Title");
        task.data.insert("a".into(), serde_json::json!(1));
        let form = EditForm::from_task(&task, vec![]);
        assert!(form.data_changes().is_empty());
    }

    // ── tag editor ───────────────────────────────────────────────────────────

    fn press(form: &mut EditForm, code: KeyCode) {
        form.focus = Field::Tags;
        form.handle_field_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn tag_list_left_right_moves_cursor() {
        let mut task = Task::new("T");
        task.tags = vec!["@a".into(), "@b".into(), "@c".into()];
        let mut form = EditForm::from_task(&task, vec![]);
        assert_eq!(form.tag_cursor, 0);
        press(&mut form, KeyCode::Right);
        assert_eq!(form.tag_cursor, 1);
        press(&mut form, KeyCode::Right);
        assert_eq!(form.tag_cursor, 2);
        // Cannot go past the last tag.
        press(&mut form, KeyCode::Right);
        assert_eq!(form.tag_cursor, 2);
        press(&mut form, KeyCode::Left);
        assert_eq!(form.tag_cursor, 1);
    }

    #[test]
    fn tag_list_delete_removes_selected() {
        let mut task = Task::new("T");
        task.tags = vec!["@a".into(), "@b".into(), "@c".into()];
        let mut form = EditForm::from_task(&task, vec![]);
        // Move cursor to the middle tag and delete it.
        press(&mut form, KeyCode::Right);
        press(&mut form, KeyCode::Delete);
        assert_eq!(form.tags, vec!["@a".to_owned(), "@c".to_owned()]);
        // Cursor clamped to last valid index (index 1 → still valid at "@c").
        assert_eq!(form.tag_cursor, 1);
    }

    #[test]
    fn tag_list_delete_clamps_cursor_when_last_removed() {
        let mut task = Task::new("T");
        task.tags = vec!["@a".into(), "@b".into()];
        let mut form = EditForm::from_task(&task, vec![]);
        press(&mut form, KeyCode::Right); // cursor = 1 (@b)
        press(&mut form, KeyCode::Delete);
        assert_eq!(form.tags, vec!["@a".to_owned()]);
        assert_eq!(form.tag_cursor, 0, "cursor clamped to last remaining tag");
    }

    #[test]
    fn tag_enter_switches_to_add_mode() {
        let task = Task::new("T");
        let mut form = EditForm::from_task(&task, vec!["@work".into(), "@home".into()]);
        press(&mut form, KeyCode::Enter);
        assert_eq!(form.tag_mode, TagMode::Add);
    }

    #[test]
    fn tag_add_suggestions_filtered_by_input() {
        let task = Task::new("T");
        let known = vec!["@work".into(), "@home".into(), "@hobby".into()];
        let mut form = EditForm::from_task(&task, known);
        press(&mut form, KeyCode::Enter); // enter Add mode
                                          // Type 'h' — should match @home and @hobby.
        press(&mut form, KeyCode::Char('h'));
        assert_eq!(
            form.tag_suggestions,
            vec!["@home".to_owned(), "@hobby".to_owned()]
        );
    }

    #[test]
    fn tag_add_suggestions_exclude_existing_tags() {
        let mut task = Task::new("T");
        task.tags = vec!["@work".into()];
        let known = vec!["@work".into(), "@home".into()];
        let mut form = EditForm::from_task(&task, known);
        press(&mut form, KeyCode::Enter);
        // No filter text → all non-owned known tags shown.
        assert_eq!(form.tag_suggestions, vec!["@home".to_owned()]);
    }

    #[test]
    fn tag_add_enter_picks_suggestion() {
        let task = Task::new("T");
        let known = vec!["@work".into(), "@home".into()];
        let mut form = EditForm::from_task(&task, known);
        press(&mut form, KeyCode::Enter); // enter Add mode
                                          // Down once to select @home (index 1).
        press(&mut form, KeyCode::Down);
        press(&mut form, KeyCode::Enter); // commit
        assert_eq!(form.tags, vec!["@home".to_owned()]);
        assert_eq!(form.tag_mode, TagMode::List);
    }

    #[test]
    fn tag_add_enter_with_no_suggestion_uses_typed_text() {
        let task = Task::new("T");
        let mut form = EditForm::from_task(&task, vec![]);
        press(&mut form, KeyCode::Enter); // enter Add mode
                                          // Type a new tag not in known_tags.
        press(&mut form, KeyCode::Char('@'));
        press(&mut form, KeyCode::Char('a'));
        press(&mut form, KeyCode::Char('i'));
        press(&mut form, KeyCode::Enter); // commit typed text
        assert_eq!(form.tags, vec!["@ai".to_owned()]);
        assert_eq!(form.tag_mode, TagMode::List);
    }

    #[test]
    fn tag_focus_next_resets_add_mode() {
        let task = Task::new("T");
        let mut form = EditForm::from_task(&task, vec!["@x".into()]);
        press(&mut form, KeyCode::Enter); // enter Add mode
        assert_eq!(form.tag_mode, TagMode::Add);
        form.focus_next(); // leaving Tags field
        assert_eq!(form.tag_mode, TagMode::List);
        assert!(form.tag_suggestions.is_empty());
    }
}
