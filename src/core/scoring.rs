use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

use serde::{Deserialize, Serialize};

/// Git-derived timestamps for a task file — creation and last modification.
///
/// Both timestamps come from git history: `created_at` is the author time of
/// the first commit that added the task file; `updated_at` is the author time
/// of the most recent commit touching it.
///
/// Keyed by full task UUID in the maps passed to [`score_and_sort`].
#[derive(Debug, Clone)]
pub struct TaskDates {
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

use crate::core::domain::{
    tag::TagMeta,
    task::{Priority, Status, Task},
};

// ── Default scoring constants ─────────────────────────────────────────────────

pub const DEFAULT_DUE_OVERDUE_BASE: f64 = 12.0;
pub const DEFAULT_DUE_OVERDUE_PER_DAY: f64 = 0.3;
pub const DEFAULT_DUE_WEEK_BASE: f64 = 6.0;
pub const DEFAULT_DUE_WEEK_PER_DAY: f64 = 0.8;
pub const DEFAULT_DUE_MONTH_BASE: f64 = 3.0;
pub const DEFAULT_DUE_MONTH_PER_DAY: f64 = 0.1;
pub const DEFAULT_PRIORITY_LOW: f64 = 0.0;
pub const DEFAULT_PRIORITY_MEDIUM: f64 = 1.0;
pub const DEFAULT_PRIORITY_HIGH: f64 = 2.0;
pub const DEFAULT_PROJECT_LOW: f64 = -0.5;
pub const DEFAULT_PROJECT_MEDIUM: f64 = 0.0;
pub const DEFAULT_PROJECT_HIGH: f64 = 0.5;
pub const DEFAULT_AGE_PER_DAY: f64 = 0.01;
pub const DEFAULT_AGE_MAX: f64 = 2.0;
pub const DEFAULT_TAG_LOW: f64 = -1.0;
pub const DEFAULT_TAG_MEDIUM: f64 = 0.0;
pub const DEFAULT_TAG_HIGH: f64 = 1.0;
pub const DEFAULT_STARTED_BONUS: f64 = 4.0;

fn default_due_overdue_base() -> f64 { DEFAULT_DUE_OVERDUE_BASE }
fn default_due_overdue_per_day() -> f64 { DEFAULT_DUE_OVERDUE_PER_DAY }
fn default_due_week_base() -> f64 { DEFAULT_DUE_WEEK_BASE }
fn default_due_week_per_day() -> f64 { DEFAULT_DUE_WEEK_PER_DAY }
fn default_due_month_base() -> f64 { DEFAULT_DUE_MONTH_BASE }
fn default_due_month_per_day() -> f64 { DEFAULT_DUE_MONTH_PER_DAY }
fn default_priority_low() -> f64 { DEFAULT_PRIORITY_LOW }
fn default_priority_medium() -> f64 { DEFAULT_PRIORITY_MEDIUM }
fn default_priority_high() -> f64 { DEFAULT_PRIORITY_HIGH }
fn default_project_low() -> f64 { DEFAULT_PROJECT_LOW }
fn default_project_medium() -> f64 { DEFAULT_PROJECT_MEDIUM }
fn default_project_high() -> f64 { DEFAULT_PROJECT_HIGH }
fn default_age_per_day() -> f64 { DEFAULT_AGE_PER_DAY }
fn default_age_max() -> f64 { DEFAULT_AGE_MAX }
fn default_tag_low() -> f64 { DEFAULT_TAG_LOW }
fn default_tag_medium() -> f64 { DEFAULT_TAG_MEDIUM }
fn default_tag_high() -> f64 { DEFAULT_TAG_HIGH }
fn default_started_bonus() -> f64 { DEFAULT_STARTED_BONUS }

/// Weights used in the urgency scoring formula. All fields are optional in the
/// config file; omitted fields keep their default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoringConfig {
    #[serde(default = "default_due_overdue_base")]
    pub due_overdue_base: f64,
    #[serde(default = "default_due_overdue_per_day")]
    pub due_overdue_per_day: f64,
    #[serde(default = "default_due_week_base")]
    pub due_week_base: f64,
    #[serde(default = "default_due_week_per_day")]
    pub due_week_per_day: f64,
    #[serde(default = "default_due_month_base")]
    pub due_month_base: f64,
    #[serde(default = "default_due_month_per_day")]
    pub due_month_per_day: f64,
    #[serde(default = "default_priority_low")]
    pub priority_low: f64,
    #[serde(default = "default_priority_medium")]
    pub priority_medium: f64,
    #[serde(default = "default_priority_high")]
    pub priority_high: f64,
    #[serde(default = "default_project_low")]
    pub project_low: f64,
    #[serde(default = "default_project_medium")]
    pub project_medium: f64,
    #[serde(default = "default_project_high")]
    pub project_high: f64,
    #[serde(default = "default_age_per_day")]
    pub age_per_day: f64,
    #[serde(default = "default_age_max")]
    pub age_max: f64,
    #[serde(default = "default_tag_low")]
    pub tag_low: f64,
    #[serde(default = "default_tag_medium")]
    pub tag_medium: f64,
    #[serde(default = "default_tag_high")]
    pub tag_high: f64,
    #[serde(default = "default_started_bonus")]
    pub started_bonus: f64,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        Self {
            due_overdue_base: default_due_overdue_base(),
            due_overdue_per_day: default_due_overdue_per_day(),
            due_week_base: default_due_week_base(),
            due_week_per_day: default_due_week_per_day(),
            due_month_base: default_due_month_base(),
            due_month_per_day: default_due_month_per_day(),
            priority_low: default_priority_low(),
            priority_medium: default_priority_medium(),
            priority_high: default_priority_high(),
            project_low: default_project_low(),
            project_medium: default_project_medium(),
            project_high: default_project_high(),
            age_per_day: default_age_per_day(),
            age_max: default_age_max(),
            tag_low: default_tag_low(),
            tag_medium: default_tag_medium(),
            tag_high: default_tag_high(),
            started_bonus: default_started_bonus(),
        }
    }
}

/// A task paired with its computed urgency score.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScoredTask {
    pub task: Task,
    pub score: f64,
}

/// The individual factor contributions that sum to a task's urgency score.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScoreBreakdown {
    pub due: f64,
    pub priority: f64,
    pub project: f64,
    pub age: f64,
    pub tags: f64,
    pub parent_tags: f64,
    pub started: f64,
    pub adjustment: f64,
    pub total: f64,
    /// True when a `no_time_urgency` tag suppressed the due and age factors.
    pub no_time_urgency: bool,
}

impl ScoreBreakdown {
    /// The breakdown of a task that scores nothing: every factor 0.0 and no
    /// `no_time_urgency` marker, so display surfaces render a bare `0.00`.
    fn zero() -> Self {
        ScoreBreakdown {
            due: 0.0,
            priority: 0.0,
            project: 0.0,
            age: 0.0,
            tags: 0.0,
            parent_tags: 0.0,
            started: 0.0,
            adjustment: 0.0,
            total: 0.0,
            no_time_urgency: false,
        }
    }

    /// The non-zero factors as `(label, value)` pairs, in display order, for a
    /// compact breakdown such as `due 12.00  priority 1.00`. The trailing
    /// `no-time-urgency` marker (value `0.0`) is appended when it applied.
    ///
    /// Shared by `next show` and the TUI detail pane so both stay in sync.
    pub fn nonzero_factors(&self) -> Vec<(&'static str, f64)> {
        let mut parts = Vec::new();
        if self.due != 0.0 {
            parts.push(("due", self.due));
        }
        if self.priority != 0.0 {
            parts.push(("priority", self.priority));
        }
        if self.project != 0.0 {
            parts.push(("project", self.project));
        }
        if self.age != 0.0 {
            parts.push(("age", self.age));
        }
        if self.tags != 0.0 {
            parts.push(("tags", self.tags));
        }
        if self.parent_tags != 0.0 {
            parts.push(("parent-tags", self.parent_tags));
        }
        if self.started != 0.0 {
            parts.push(("started", self.started));
        }
        if self.adjustment != 0.0 {
            parts.push(("adj", self.adjustment));
        }
        parts
    }
}

/// Due-date contribution to the urgency score.
///
/// Returns 0.0 when no due date is set. Otherwise the score rises steeply as
/// the deadline approaches and continues to increase after it has passed.
pub fn due_factor(due: Option<NaiveDate>, today: NaiveDate, w: &ScoringConfig) -> f64 {
    let Some(d) = due else { return 0.0 };
    let days = (d - today).num_days(); // negative means overdue
    if days <= 0 {
        // Due today or overdue: base + per-day penalty for each day past due.
        w.due_overdue_base + (-days) as f64 * w.due_overdue_per_day
    } else if days <= 7 {
        w.due_week_base + (7 - days) as f64 * w.due_week_per_day
    } else if days <= 30 {
        w.due_month_base + (30 - days) as f64 * w.due_month_per_day
    } else {
        // Far future: slow decay toward zero.
        (2.0_f64 - days as f64 * 0.01).max(0.0)
    }
}

/// Task-priority contribution to the urgency score.
pub fn priority_factor(priority: &Priority, w: &ScoringConfig) -> f64 {
    match priority {
        Priority::Low => w.priority_low,
        Priority::Medium => w.priority_medium,
        Priority::High => w.priority_high,
    }
}

/// Parent-task-priority contribution (the "project factor").
/// Returns 0.0 when the task has no parent.
pub fn project_factor(parent_priority: Option<&Priority>, w: &ScoringConfig) -> f64 {
    match parent_priority {
        None => 0.0,
        Some(Priority::Low) => w.project_low,
        Some(Priority::Medium) => w.project_medium,
        Some(Priority::High) => w.project_high,
    }
}

/// Age contribution to the urgency score.
///
/// Grows linearly with age (days since creation) up to `age_max`.
/// Returns 0.0 when age scoring is disabled for the task (long-term or future start date)
/// or when no git creation date is available.
pub fn age_factor(task: &Task, created_at: Option<DateTime<Utc>>, today: NaiveDate, w: &ScoringConfig) -> f64 {
    if task.age_scoring_disabled(today) {
        return 0.0;
    }
    let Some(created) = created_at else {
        return 0.0;
    };
    let age_days = (today - created.date_naive()).num_days().max(0) as f64;
    (age_days * w.age_per_day).min(w.age_max)
}

/// Tag-priority contribution: sum of the priority weights for each tag on the task
/// that has an explicit priority set in its `TagMeta`. Tags with no metadata or no
/// priority set contribute 0.0.
pub fn tag_factor(tags: &[String], tag_metas: &HashMap<String, TagMeta>, w: &ScoringConfig) -> f64 {
    tags.iter()
        .filter_map(|t| tag_metas.get(t))
        .filter_map(|meta| meta.priority.as_ref())
        .map(|p| match p {
            Priority::Low => w.tag_low,
            Priority::Medium => w.tag_medium,
            Priority::High => w.tag_high,
        })
        .sum()
}

/// Started-state contribution: a flat bonus when the task is in the `Started` state.
pub fn started_factor(status: &Status, w: &ScoringConfig) -> f64 {
    if *status == Status::Started { w.started_bonus } else { 0.0 }
}

/// True for the two terminal states, `Done` and `Cancelled`.
///
/// A score is advice about what to work on next, so it does not apply to a
/// task that is finished: both scoring entry points return 0 for these.
fn is_closed(status: &Status) -> bool {
    matches!(status, Status::Done | Status::Cancelled)
}

/// Returns `true` if any of `tags` has `no_time_urgency = true` in its metadata.
pub fn tag_no_time_urgency(tags: &[String], tag_metas: &HashMap<String, TagMeta>) -> bool {
    tags.iter()
        .filter_map(|t| tag_metas.get(t))
        .any(|meta| meta.no_time_urgency)
}

/// Computes the total urgency score for `task`.
///
/// `parent` is the parent task (if any); its priority and tags both contribute.
/// `task_dates` provides git-derived creation/update times; `None` disables age scoring.
/// `tag_metas` is the full tag-metadata map from the store.
///
/// A closed (done or cancelled) task always scores 0: urgency ranks what to do
/// next, and a finished task has nothing left to do. The gate lives here rather
/// than in the callers so every surface — `list --closed`/`--all`, `show`, the
/// TUI, MCP — agrees without each one having to remember.
pub fn score(
    task: &Task,
    parent: Option<&Task>,
    task_dates: &HashMap<Uuid, TaskDates>,
    today: NaiveDate,
    w: &ScoringConfig,
    tag_metas: &HashMap<String, TagMeta>,
) -> f64 {
    if is_closed(&task.status) {
        return 0.0;
    }

    let no_time = tag_no_time_urgency(&task.tags, tag_metas);
    let own_tags = tag_factor(&task.tags, tag_metas, w);
    let parent_tags = parent.map_or(0.0, |p| tag_factor(&p.tags, tag_metas, w));
    let created_at = task_dates.get(&task.id).map(|d| d.created_at);

    (if no_time { 0.0 } else { due_factor(task.due, today, w) })
        + priority_factor(&task.priority, w)
        + project_factor(parent.map(|p| &p.priority), w)
        + (if no_time { 0.0 } else { age_factor(task, created_at, today, w) })
        + own_tags
        + parent_tags
        + started_factor(&task.status, w)
        + task.score_adjustment
}

/// Like [`score`] but also returns the individual factor contributions.
///
/// Closed tasks get the all-zero breakdown, so a detail view shows a plain
/// `0.00` instead of a factor list that no longer means anything.
pub fn score_with_breakdown(
    task: &Task,
    parent: Option<&Task>,
    task_dates: &HashMap<Uuid, TaskDates>,
    today: NaiveDate,
    w: &ScoringConfig,
    tag_metas: &HashMap<String, TagMeta>,
) -> ScoreBreakdown {
    if is_closed(&task.status) {
        return ScoreBreakdown::zero();
    }

    let no_time = tag_no_time_urgency(&task.tags, tag_metas);
    let created_at = task_dates.get(&task.id).map(|d| d.created_at);
    let due       = if no_time { 0.0 } else { due_factor(task.due, today, w) };
    let priority  = priority_factor(&task.priority, w);
    let project   = project_factor(parent.map(|p| &p.priority), w);
    let age       = if no_time { 0.0 } else { age_factor(task, created_at, today, w) };
    let tags      = tag_factor(&task.tags, tag_metas, w);
    let parent_tags = parent.map_or(0.0, |p| tag_factor(&p.tags, tag_metas, w));
    let started   = started_factor(&task.status, w);
    let adjustment = task.score_adjustment;
    let total = due + priority + project + age + tags + parent_tags + started + adjustment;
    ScoreBreakdown { due, priority, project, age, tags, parent_tags, started, adjustment, total, no_time_urgency: no_time }
}

/// Scores each task in `tasks`, sorts by score descending (most urgent first),
/// and returns the paired results.
///
/// `all_tasks` is used to look up parent tasks for the project-priority factor.
/// It should be the full, unfiltered task list.
/// `task_dates` provides git-derived creation/update timestamps; pass an empty map
/// when git history is unavailable (tests, new uncommitted tasks lose age scoring).
///
/// The sort is stable and has no tiebreak, so tasks that score the same keep the
/// order they arrived in. Since every closed task scores 0, a `--closed` listing
/// comes out in the store's query order (unresolved first, then most recent
/// completion, ties by id) — the same order `--archived` uses — and a mixed
/// `--all` listing puts every open task above every closed one.
pub fn score_and_sort(
    tasks: Vec<Task>,
    all_tasks: &[Task],
    today: NaiveDate,
    w: &ScoringConfig,
    tag_metas: &HashMap<String, TagMeta>,
    task_dates: &HashMap<Uuid, TaskDates>,
) -> Vec<ScoredTask> {
    let by_id: HashMap<Uuid, &Task> = all_tasks.iter().map(|t| (t.id, t)).collect();

    let mut scored: Vec<ScoredTask> = tasks
        .into_iter()
        .map(|task| {
            let parent = task.parent_id.and_then(|id| by_id.get(&id).copied());
            let s = score(&task, parent, task_dates, today, w, tag_metas);
            ScoredTask { task, score: s }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 5, 17).unwrap()
    }

    fn weights() -> ScoringConfig {
        ScoringConfig::default()
    }

    // --- due_factor ---

    #[test]
    fn due_factor_no_due_date() {
        assert_eq!(due_factor(None, today(), &weights()), 0.0);
    }

    #[test]
    fn due_factor_due_today() {
        // due today = overdue by 0 days → base
        assert_eq!(due_factor(Some(today()), today(), &weights()), 12.0);
    }

    #[test]
    fn due_factor_overdue_one_day() {
        let yesterday = today() - chrono::Duration::days(1);
        assert_eq!(
            due_factor(Some(yesterday), today(), &weights()),
            12.0 + 1.0 * 0.3
        );
    }

    #[test]
    fn due_factor_overdue_ten_days() {
        let d = today() - chrono::Duration::days(10);
        assert_eq!(due_factor(Some(d), today(), &weights()), 12.0 + 10.0 * 0.3);
    }

    #[test]
    fn due_factor_due_in_one_day() {
        let tomorrow = today() + chrono::Duration::days(1);
        // days=1: 6.0 + (7-1)*0.8 = 6.0 + 4.8 = 10.8
        assert_eq!(due_factor(Some(tomorrow), today(), &weights()), 6.0 + 6.0 * 0.8);
    }

    #[test]
    fn due_factor_due_in_seven_days() {
        let d = today() + chrono::Duration::days(7);
        // days=7: 6.0 + (7-7)*0.8 = 6.0
        assert_eq!(due_factor(Some(d), today(), &weights()), 6.0);
    }

    #[test]
    fn due_factor_due_in_eight_days() {
        let d = today() + chrono::Duration::days(8);
        // days=8: 3.0 + (30-8)*0.1 = 3.0 + 2.2 = 5.2
        assert!((due_factor(Some(d), today(), &weights()) - 5.2).abs() < 1e-9);
    }

    #[test]
    fn due_factor_due_in_thirty_days() {
        let d = today() + chrono::Duration::days(30);
        // days=30: 3.0 + (30-30)*0.1 = 3.0
        assert_eq!(due_factor(Some(d), today(), &weights()), 3.0);
    }

    #[test]
    fn due_factor_due_in_31_days() {
        let d = today() + chrono::Duration::days(31);
        // 2.0 - 31*0.01 = 1.69
        assert!((due_factor(Some(d), today(), &weights()) - 1.69).abs() < 1e-9);
    }

    #[test]
    fn due_factor_far_future_clamps_to_zero() {
        let d = today() + chrono::Duration::days(300);
        assert_eq!(due_factor(Some(d), today(), &weights()), 0.0);
    }

    // --- priority_factor ---

    #[test]
    fn priority_factors() {
        let w = weights();
        assert_eq!(priority_factor(&Priority::Low, &w), 0.0);
        assert_eq!(priority_factor(&Priority::Medium, &w), 1.0);
        assert_eq!(priority_factor(&Priority::High, &w), 2.0);
    }

    // --- project_factor ---

    #[test]
    fn project_factor_no_parent() {
        assert_eq!(project_factor(None, &weights()), 0.0);
    }

    #[test]
    fn project_factors() {
        let w = weights();
        assert_eq!(project_factor(Some(&Priority::Low), &w), -0.5);
        assert_eq!(project_factor(Some(&Priority::Medium), &w), 0.0);
        assert_eq!(project_factor(Some(&Priority::High), &w), 0.5);
    }

    // --- age_factor ---

    #[test]
    fn age_factor_zero_age() {
        let task = Task::new("new task");
        // No creation date available → graceful degradation to 0.
        let score = age_factor(&task, None, today(), &weights());
        assert_eq!(score, 0.0);
    }

    #[test]
    fn age_factor_long_term_returns_zero() {
        let mut task = Task::new("long-term");
        task.long_term = true;
        assert_eq!(age_factor(&task, None, today(), &weights()), 0.0);
    }

    #[test]
    fn age_factor_future_start_returns_zero() {
        let mut task = Task::new("future");
        task.start = Some(today() + chrono::Duration::days(7));
        assert_eq!(age_factor(&task, None, today(), &weights()), 0.0);
    }

    #[test]
    fn age_factor_caps_at_max() {
        let task = Task::new("old task");
        // Pass a creation date far in the past to force the cap.
        // age_max=2.0, age_per_day=0.01 → 200 days to reach cap.
        let old_ts = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let score = age_factor(&task, Some(old_ts), today(), &weights());
        assert_eq!(score, 2.0); // capped at age_max
    }

    // --- tag_factor ---

    fn meta_with_priority(p: Priority) -> TagMeta {
        TagMeta { priority: Some(p), ..Default::default() }
    }

    #[test]
    fn tag_factor_no_tags() {
        assert_eq!(tag_factor(&[], &HashMap::new(), &weights()), 0.0);
    }

    #[test]
    fn tag_factor_tag_without_meta() {
        let task_tags = vec!["@work".to_string()];
        assert_eq!(tag_factor(&task_tags, &HashMap::new(), &weights()), 0.0);
    }

    #[test]
    fn tag_factor_high_priority_tag() {
        let mut metas = HashMap::new();
        metas.insert("@work".to_string(), meta_with_priority(Priority::High));
        let tags = vec!["@work".to_string()];
        assert_eq!(tag_factor(&tags, &metas, &weights()), 1.0); // tag_high default
    }

    #[test]
    fn tag_factor_low_priority_tag() {
        let mut metas = HashMap::new();
        metas.insert("someday".to_string(), meta_with_priority(Priority::Low));
        let tags = vec!["someday".to_string()];
        assert_eq!(tag_factor(&tags, &metas, &weights()), -1.0); // tag_low default
    }

    #[test]
    fn tag_factor_stacks_multiple_tags() {
        let mut metas = HashMap::new();
        metas.insert("@work".to_string(), meta_with_priority(Priority::High));
        metas.insert("#laptop".to_string(), meta_with_priority(Priority::High));
        let tags = vec!["@work".to_string(), "#laptop".to_string()];
        assert_eq!(tag_factor(&tags, &metas, &weights()), 2.0); // 1.0 + 1.0
    }

    // --- started_factor ---

    #[test]
    fn started_factor_open_is_zero() {
        assert_eq!(started_factor(&Status::Open, &weights()), 0.0);
    }

    #[test]
    fn started_factor_started_is_bonus() {
        assert_eq!(started_factor(&Status::Started, &weights()), 4.0); // default
    }

    #[test]
    fn started_factor_done_is_zero() {
        assert_eq!(started_factor(&Status::Done, &weights()), 0.0);
    }

    // --- score ---

    #[test]
    fn score_no_due_medium_priority_no_parent() {
        let task = Task::new("simple task");
        let s = score(&task, None, &HashMap::new(), today(), &weights(), &HashMap::new());
        // priority_medium=1.0, no due, no parent, age=0 (no git dates)
        assert!((s - 1.0).abs() < 0.01);
    }

    #[test]
    fn score_overdue_task_ranks_higher_than_no_due() {
        let mut overdue = Task::new("overdue");
        overdue.due = Some(today() - chrono::Duration::days(3));
        let normal = Task::new("no due");

        let w = weights();
        let metas = HashMap::new();
        let dates: HashMap<Uuid, TaskDates> = HashMap::new();
        assert!(
            score(&overdue, None, &dates, today(), &w, &metas)
                > score(&normal, None, &dates, today(), &w, &metas)
        );
    }

    #[test]
    fn score_adjustment_applied() {
        let mut task = Task::new("boosted");
        task.score_adjustment = 5.0;
        let s = score(&task, None, &HashMap::new(), today(), &weights(), &HashMap::new());
        assert!((s - 6.0).abs() < 0.01); // 1.0 (medium) + 5.0 (adj) + 0 (age)
    }

    #[test]
    fn score_started_bonus_applied() {
        let mut task = Task::new("in progress");
        task.mark_started();
        let s = score(&task, None, &HashMap::new(), today(), &weights(), &HashMap::new());
        // 1.0 (medium) + 4.0 (started) + 0 (age, no git dates)
        assert!((s - 5.0).abs() < 0.01);
    }

    #[test]
    fn score_high_priority_tag_boosts_score() {
        let mut task = Task::new("tagged");
        task.tags = vec!["@work".to_string()];
        let mut metas = HashMap::new();
        metas.insert("@work".to_string(), meta_with_priority(Priority::High));
        let s = score(&task, None, &HashMap::new(), today(), &weights(), &metas);
        // 1.0 (medium) + 1.0 (tag_high) = 2.0
        assert!((s - 2.0).abs() < 0.01);
    }

    #[test]
    fn score_parent_tag_priority_contributes() {
        let mut parent = Task::new("parent project");
        parent.tags = vec!["@work".to_string()];
        let child = Task::new("child task");

        let mut metas = HashMap::new();
        metas.insert("@work".to_string(), meta_with_priority(Priority::High));

        let dates: HashMap<Uuid, TaskDates> = HashMap::new();
        let child_score = score(&child, Some(&parent), &dates, today(), &weights(), &metas);
        let orphan_score = score(&child, None, &dates, today(), &weights(), &metas);
        // child gets +1.0 from parent's @work tag
        assert!((child_score - orphan_score - 1.0).abs() < 0.01);
    }

    #[test]
    fn score_and_sort_orders_descending() {
        let mut urgent = Task::new("urgent");
        urgent.due = Some(today() - chrono::Duration::days(1)); // overdue

        let normal = Task::new("normal");

        let all = vec![normal.clone(), urgent.clone()];
        let scored = score_and_sort(vec![normal, urgent], &all, today(), &weights(), &HashMap::new(), &HashMap::new());

        assert_eq!(scored[0].task.title, "urgent");
        assert!(scored[0].score > scored[1].score);
    }

    #[test]
    fn score_and_sort_uses_parent_priority() {
        let mut parent = Task::new("high-priority project");
        parent.priority = Priority::High;

        let mut child = Task::new("child task");
        child.parent_id = Some(parent.id);

        let orphan = Task::new("no parent");

        let all = vec![parent.clone(), child.clone(), orphan.clone()];
        let scored = score_and_sort(vec![child, orphan], &all, today(), &weights(), &HashMap::new(), &HashMap::new());

        // child gets +0.5 from high-priority parent; orphan gets +0
        let child_score = scored.iter().find(|s| s.task.title == "child task").unwrap().score;
        let orphan_score = scored.iter().find(|s| s.task.title == "no parent").unwrap().score;
        assert!(child_score > orphan_score);
    }

    // --- tag_no_time_urgency ---

    #[test]
    fn no_time_urgency_suppresses_due_and_age() {
        let mut task = Task::new("wishlist item");
        task.tags = vec!["wishlist".to_string()];
        task.due = Some(today() - chrono::Duration::days(5)); // overdue
        let old_ts = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        // Provide a git creation date far in the past so age would normally be capped.
        let mut dates: HashMap<Uuid, TaskDates> = HashMap::new();
        dates.insert(task.id, TaskDates { created_at: old_ts, updated_at: old_ts });

        let mut metas = HashMap::new();
        metas.insert("wishlist".to_string(), TagMeta { no_time_urgency: true, ..Default::default() });

        let s = score(&task, None, &dates, today(), &weights(), &metas);
        // Only priority_factor(medium)=1.0 contributes; due and age are zeroed.
        assert!((s - 1.0).abs() < 0.01, "expected ~1.0, got {s}");
    }

    #[test]
    fn no_time_urgency_false_still_scores_normally() {
        let mut task = Task::new("normal");
        task.tags = vec!["freeform".to_string()];
        task.due = Some(today()); // due today
        let mut metas = HashMap::new();
        metas.insert("freeform".to_string(), TagMeta::default()); // no_time_urgency = false
        let s = score(&task, None, &HashMap::new(), today(), &weights(), &metas);
        // due_factor(due today) = 12.0, priority = 1.0 → > 12.0
        assert!(s > 12.0);
    }

    #[test]
    fn score_and_sort_uses_parent_tag_priority() {
        let mut parent = Task::new("high-relevance project");
        parent.tags = vec!["@work".to_string()];

        let mut child = Task::new("child task");
        child.parent_id = Some(parent.id);
        let orphan = Task::new("no parent");

        let mut metas = HashMap::new();
        metas.insert("@work".to_string(), meta_with_priority(Priority::High));

        let all = vec![parent.clone(), child.clone(), orphan.clone()];
        let scored = score_and_sort(vec![child, orphan], &all, today(), &weights(), &metas, &HashMap::new());

        let child_score = scored.iter().find(|s| s.task.title == "child task").unwrap().score;
        let orphan_score = scored.iter().find(|s| s.task.title == "no parent").unwrap().score;
        // child gets +1.0 from parent's @work tag
        assert!((child_score - orphan_score - 1.0).abs() < 0.01);
    }

    // --- closed tasks score 0 ---

    /// A task loaded with every factor that would otherwise contribute: high
    /// priority, a long-overdue due date, a high-priority tag, a positive
    /// adjustment, and (via the caller) a far-past creation date.
    fn loaded_task(title: &str) -> (Task, HashMap<String, TagMeta>, HashMap<Uuid, TaskDates>) {
        let mut task = Task::new(title);
        task.priority = Priority::High;
        task.due = Some(today() - chrono::Duration::days(30));
        task.tags = vec!["@work".to_string()];
        task.score_adjustment = 5.0;

        let mut metas = HashMap::new();
        metas.insert("@work".to_string(), meta_with_priority(Priority::High));

        let old_ts = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut dates = HashMap::new();
        dates.insert(task.id, TaskDates { created_at: old_ts, updated_at: old_ts });

        (task, metas, dates)
    }

    #[test]
    fn score_open_task_with_every_factor_is_nonzero() {
        // Baseline for the two tests below: this fixture really does score.
        let (task, metas, dates) = loaded_task("loaded");
        let s = score(&task, None, &dates, today(), &weights(), &metas);
        assert!(s > 20.0, "expected a large score for the open fixture, got {s}");
    }

    #[test]
    fn score_done_task_is_zero() {
        let (mut task, metas, dates) = loaded_task("finished");
        task.mark_done(today());
        assert_eq!(score(&task, None, &dates, today(), &weights(), &metas), 0.0);
    }

    #[test]
    fn score_cancelled_task_is_zero() {
        let (mut task, metas, dates) = loaded_task("abandoned");
        task.mark_cancelled();
        assert_eq!(score(&task, None, &dates, today(), &weights(), &metas), 0.0);
    }

    #[test]
    fn score_closed_task_ignores_started_bonus_and_parent() {
        // A task that was started and then closed keeps neither the started
        // bonus nor anything inherited from a high-priority parent.
        let mut parent = Task::new("project");
        parent.priority = Priority::High;
        parent.tags = vec!["@work".to_string()];
        let mut metas = HashMap::new();
        metas.insert("@work".to_string(), meta_with_priority(Priority::High));

        let mut task = Task::new("was started");
        task.mark_started();
        task.mark_done(today());

        let s = score(&task, Some(&parent), &HashMap::new(), today(), &weights(), &metas);
        assert_eq!(s, 0.0);
    }

    #[test]
    fn score_open_and_started_tasks_are_unaffected_by_the_gate() {
        let open = Task::new("open");
        let mut started = Task::new("started");
        started.mark_started();
        let w = weights();
        let metas = HashMap::new();
        let dates: HashMap<Uuid, TaskDates> = HashMap::new();

        assert!((score(&open, None, &dates, today(), &w, &metas) - 1.0).abs() < 0.01);
        assert!((score(&started, None, &dates, today(), &w, &metas) - 5.0).abs() < 0.01);
    }

    #[test]
    fn score_with_breakdown_closed_task_is_all_zero() {
        for status in [Status::Done, Status::Cancelled] {
            let (mut task, metas, dates) = loaded_task("closed");
            task.status = status;
            let bd = score_with_breakdown(&task, None, &dates, today(), &weights(), &metas);
            assert_eq!(bd.due, 0.0);
            assert_eq!(bd.priority, 0.0);
            assert_eq!(bd.project, 0.0);
            assert_eq!(bd.age, 0.0);
            assert_eq!(bd.tags, 0.0);
            assert_eq!(bd.parent_tags, 0.0);
            assert_eq!(bd.started, 0.0);
            assert_eq!(bd.adjustment, 0.0);
            assert_eq!(bd.total, 0.0);
            // No marker either: `next show` must render a bare `0.00`.
            assert!(!bd.no_time_urgency);
            assert!(bd.nonzero_factors().is_empty());
        }
    }

    #[test]
    fn score_and_sort_keeps_input_order_among_closed_tasks() {
        // Every closed task scores 0 and the sort is stable with no tiebreak,
        // so a `--closed` listing must come out in the order the store handed
        // it over (unresolved first, then most recent completion, ties by id).
        let mut first = Task::new("closed first");
        first.priority = Priority::Low;
        first.mark_done(today());
        let mut second = Task::new("closed second");
        second.priority = Priority::High; // would outrank `first` if scored
        second.mark_done(today() - chrono::Duration::days(10));
        let mut third = Task::new("closed third");
        third.score_adjustment = 100.0; // would dominate if scored
        third.mark_cancelled();

        let input = vec![first.clone(), second.clone(), third.clone()];
        let all = input.clone();
        let scored = score_and_sort(input, &all, today(), &weights(), &HashMap::new(), &HashMap::new());

        assert_eq!(titles(&scored), ["closed first", "closed second", "closed third"]);
        assert!(scored.iter().all(|s| s.score == 0.0));
    }

    #[test]
    fn score_and_sort_puts_scoring_open_tasks_above_closed_ones() {
        // The `--all` reading: live work first, the closed tail in arrival
        // order. Only open tasks that score above 0 are guaranteed to sort
        // above the closed ones — a low-priority open task with no other
        // factor also scores 0 and merely keeps its input position.
        let mut done_high = Task::new("done high");
        done_high.priority = Priority::High;
        done_high.mark_done(today());
        let mut cancelled = Task::new("cancelled");
        cancelled.score_adjustment = 50.0;
        cancelled.mark_cancelled();
        let medium_open = Task::new("open medium");
        let mut started_open = Task::new("open started");
        started_open.mark_started();

        let input = vec![done_high, medium_open, cancelled, started_open];
        let all = input.clone();
        let scored = score_and_sort(input, &all, today(), &weights(), &HashMap::new(), &HashMap::new());

        assert_eq!(
            titles(&scored),
            ["open started", "open medium", "done high", "cancelled"]
        );
    }

    fn titles(scored: &[ScoredTask]) -> Vec<&str> {
        scored.iter().map(|s| s.task.title.as_str()).collect()
    }

    #[test]
    fn nonzero_factors_lists_only_nonzero_in_order() {
        let bd = ScoreBreakdown {
            due: 12.0,
            priority: 1.0,
            project: 0.0,
            age: 0.5,
            tags: 0.0,
            parent_tags: 0.0,
            started: 0.0,
            adjustment: -2.0,
            total: 11.5,
            no_time_urgency: false,
        };
        let factors = bd.nonzero_factors();
        assert_eq!(
            factors,
            vec![("due", 12.0), ("priority", 1.0), ("age", 0.5), ("adj", -2.0)]
        );
    }

    #[test]
    fn nonzero_factors_empty_when_all_zero() {
        let bd = ScoreBreakdown {
            due: 0.0,
            priority: 0.0,
            project: 0.0,
            age: 0.0,
            tags: 0.0,
            parent_tags: 0.0,
            started: 0.0,
            adjustment: 0.0,
            total: 0.0,
            no_time_urgency: true,
        };
        assert!(bd.nonzero_factors().is_empty());
    }
}
