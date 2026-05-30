use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Open,
    /// Explicitly started / in-progress.
    Started,
    Done,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    #[default]
    Medium,
    High,
}

/// Calendar snap applied after computing the raw next date.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Snap {
    /// Advance to the next occurrence of this ISO weekday (0=Mon … 6=Sun).
    /// If the raw date already falls on this weekday, keep it.
    NextWeekday { weekday: u8 },
    /// Advance to the next Mon–Fri. Keep the date if it already qualifies.
    NextWorkday,
    /// Use day `day` of the current month if it is still in the future;
    /// otherwise use day `day` of the next month.
    DayOfMonth { day: u8 },
}

/// Recurrence rule attached to a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Recurrence {
    /// Fixed calendar schedule. `rrule` is an RFC 5545 RRULE string (without prefix).
    /// `anchor` pins the series to a specific start date for INTERVAL calculations.
    Schedule {
        /// Accept old field name "rule" from existing TOML files.
        #[serde(alias = "rule")]
        rrule: String,
        /// Immutable series anchor; must equal the first instance's `start` or `due` date.
        anchor: chrono::NaiveDate,
        #[serde(skip_serializing_if = "Option::is_none")]
        snap: Option<Snap>,
    },

    /// Next instance is `interval_days` after completion, then optionally snapped.
    Completion {
        interval_days: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        snap: Option<Snap>,
    },
}

/// A single task — the central domain object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub title: String,
    pub status: Status,
    pub priority: Priority,

    /// Optional deadline. When set, the due-date factor dominates the urgency score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<NaiveDate>,

    /// Task is hidden from the default list until this date.
    /// Also disables age-based scoring while the date is in the future.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<NaiveDate>,

    /// When `true`, age does not contribute to the urgency score.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub long_term: bool,

    /// Optional user-provided identifier, e.g. `"water-plants"` or `"work-infra"`.
    /// Must be unique across all tasks. Used to reference the task as a parent or
    /// blocker without knowing its UUID, and as the target of `parent:<slug>` filters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,

    /// UUID of the parent task. When set the task is a child of that task.
    /// The parent is hidden from the default list until all direct children are resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,

    /// The user responsible for this task.
    ///
    /// `None` means the task is unassigned and visible to all users. When a user
    /// filter is active, only unassigned tasks and tasks matching the active users
    /// are shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,

    /// Tags using the unified prefix convention: `@context`, `#resource`, or bare freeform.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// UUIDs of tasks that must be done or cancelled before this task becomes visible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<Uuid>,

    /// Manually applied bonus or penalty added directly to the computed score.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub score_adjustment: f64,

    /// Arbitrary key-value pairs for tool integrations or AI-provided metadata.
    /// Values are any JSON-compatible type (string, number, bool, array, object).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub data: HashMap<String, JsonValue>,

    /// Multi-line free-form description providing context or detail beyond the title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// A URL associated with the task (e.g. a ticket, doc, or reference link).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// Free-form notes in Markdown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,

    /// Forgejo issue URL set by the importer; used for deduplication and write-back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forgejo_issue: Option<String>,

    /// iCalendar UID set by the importer; used for deduplication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webcal_uid: Option<String>,

    /// Recurrence rule; present only on recurring tasks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<Recurrence>,

    /// Shared UUID across all instances of the same recurrence series.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<Uuid>,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

impl Task {
    /// Creates a new open task in the inbox with sensible defaults.
    pub fn new(title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            status: Status::Open,
            priority: Priority::Medium,
            due: None,
            start: None,
            long_term: false,
            slug: None,
            parent_id: None,
            assignee: None,
            tags: Vec::new(),
            blocked_by: Vec::new(),
            score_adjustment: 0.0,
            data: HashMap::new(),
            description: None,
            url: None,
            notes: None,
            forgejo_issue: None,
            webcal_uid: None,
            recurrence: None,
            recurrence_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Returns `true` when age must not contribute to the urgency score:
    /// either because the task is marked long-term, or because its start date
    /// is still in the future (the task is not yet active).
    pub fn age_scoring_disabled(&self, today: NaiveDate) -> bool {
        if self.long_term {
            return true;
        }
        self.start.is_some_and(|s| s > today)
    }

    /// Returns `true` when the task should be hidden from the default list
    /// because its start date is in the future.
    pub fn is_hidden(&self, today: NaiveDate) -> bool {
        self.start.is_some_and(|s| s > today)
    }

    /// Returns `true` when the task is open (not done or cancelled).
    pub fn is_open(&self) -> bool {
        self.status == Status::Open
    }

    /// Returns `true` when the task is actionable: open or started.
    pub fn is_active(&self) -> bool {
        matches!(self.status, Status::Open | Status::Started)
    }

    /// Marks the task as started (in-progress), logs the event in `data["time_log"]`,
    /// and sets `updated_at` to now.
    pub fn mark_started(&mut self) {
        self.status = Status::Started;
        self.updated_at = Utc::now();
        self.append_time_event("start");
    }

    /// Marks the task as stopped (back to open), logs the event in `data["time_log"]`,
    /// and sets `updated_at` to now.
    pub fn mark_stopped(&mut self) {
        self.status = Status::Open;
        self.updated_at = Utc::now();
        self.append_time_event("stop");
    }

    fn append_time_event(&mut self, event: &str) {
        use serde_json::json;
        let entry = json!({"event": event, "at": self.updated_at.to_rfc3339()});
        let log = self.data.entry("time_log".to_owned()).or_insert_with(|| serde_json::Value::Array(vec![]));
        if let serde_json::Value::Array(arr) = log {
            arr.push(entry);
        }
    }

    /// Marks the task as done and sets `updated_at` to now.
    pub fn mark_done(&mut self) {
        self.status = Status::Done;
        self.updated_at = Utc::now();
    }

    /// Marks the task as cancelled and sets `updated_at` to now.
    pub fn mark_cancelled(&mut self) {
        self.status = Status::Cancelled;
        self.updated_at = Utc::now();
    }

    /// Touches `updated_at` without changing any other field.
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 5, 16).unwrap()
    }

    #[test]
    fn new_task_has_correct_defaults() {
        let t = Task::new("Buy milk");
        assert_eq!(t.title, "Buy milk");
        assert_eq!(t.status, Status::Open);
        assert_eq!(t.priority, Priority::Medium);
        assert!(!t.long_term);
        assert!(t.tags.is_empty());
        assert!(t.blocked_by.is_empty());
        assert_eq!(t.score_adjustment, 0.0);
    }

    #[test]
    fn age_scoring_disabled_when_long_term() {
        let mut t = Task::new("Read a book");
        t.long_term = true;
        assert!(t.age_scoring_disabled(today()));
    }

    #[test]
    fn age_scoring_disabled_when_start_is_future() {
        let mut t = Task::new("Plan holiday");
        t.start = Some(today() + chrono::Duration::days(7));
        assert!(t.age_scoring_disabled(today()));
    }

    #[test]
    fn age_scoring_enabled_when_start_is_today_or_past() {
        let mut t = Task::new("Review budget");
        t.start = Some(today());
        assert!(!t.age_scoring_disabled(today()));

        t.start = Some(today() - chrono::Duration::days(1));
        assert!(!t.age_scoring_disabled(today()));
    }

    #[test]
    fn age_scoring_enabled_with_no_start_and_not_long_term() {
        let t = Task::new("Normal task");
        assert!(!t.age_scoring_disabled(today()));
    }

    #[test]
    fn is_hidden_when_start_is_future() {
        let mut t = Task::new("Future task");
        t.start = Some(today() + chrono::Duration::days(1));
        assert!(t.is_hidden(today()));
    }

    #[test]
    fn not_hidden_when_no_start() {
        assert!(!Task::new("No start").is_hidden(today()));
    }

    #[test]
    fn mark_done_changes_status() {
        let mut t = Task::new("Do laundry");
        t.mark_done();
        assert_eq!(t.status, Status::Done);
    }
}
