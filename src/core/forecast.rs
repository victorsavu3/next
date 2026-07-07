//! Forecast projection shared by the CLI (`next forecast`) and the TUI.
//!
//! [`build_entries`] turns a filtered, scored task set into a chronologically
//! ordered list of [`ForecastEntry`]s: every concrete task due within the
//! horizon, plus the projected (not-yet-spawned) future occurrences of active
//! schedule-type recurrence series. It lives in `core` so both the CLI command
//! and the TUI view can call it without depending on each other.

use std::collections::HashMap;

use chrono::NaiveDate;
use serde::Serialize;

use crate::core::domain::filter::{self, FilterSet};
use crate::core::domain::state::GlobalState;
use crate::core::domain::tag::TagMeta;
use crate::core::domain::task::Task;
use crate::core::recurrence;
use crate::core::scoring::{self, ScoringConfig};

/// A single occurrence in the forecast: either a concrete existing task or a
/// projected (not-yet-spawned) future instance of a schedule-type series.
#[derive(Debug, Clone, Serialize)]
pub struct ForecastEntry {
    /// The forecast date (the task's `due`, or the projected occurrence date).
    pub date: NaiveDate,
    /// Short 8-char id of the originating task (the open instance for projected ones).
    pub id: String,
    pub title: String,
    /// Urgency score of the originating task.
    pub score: f64,
    /// `true` for projected future occurrences that do not yet exist as tasks.
    pub projected: bool,
}

/// Build the chronologically-ordered forecast entries for the given horizon.
///
/// `all_tasks` is the complete (unfiltered) task list; `filter_set` selects the
/// subset to forecast; `state`, `scoring`, and `tag_metas` drive filtering and
/// scoring exactly as the list view does. The result contains concrete existing
/// tasks due on or before `today + horizon` plus projected future occurrences of
/// active schedule-type recurrence series, sorted by `(date, projected)` so the
/// current instance precedes its projections on a shared date.
pub fn build_entries(
    all_tasks: &[Task],
    state: &GlobalState,
    scoring: &ScoringConfig,
    tag_metas: &HashMap<String, TagMeta>,
    filter_set: &FilterSet,
    today: NaiveDate,
    horizon: u32,
) -> Vec<ForecastEntry> {
    let filtered = filter::apply(all_tasks.to_vec(), filter_set, state, today);
    let scored = scoring::score_and_sort(filtered, all_tasks, today, scoring, tag_metas);

    let cutoff = today + chrono::Duration::days(horizon as i64);

    let mut entries: Vec<ForecastEntry> = Vec::new();
    for st in &scored {
        let short = st.task.id.to_string().replace('-', "")[..8].to_string();

        // Concrete existing task whose stored due falls within the horizon.
        if st.task.due.is_some_and(|d| d <= cutoff) {
            entries.push(ForecastEntry {
                date: st.task.due.unwrap(),
                id: short.clone(),
                title: st.task.title.clone(),
                score: st.score,
                projected: false,
            });
        }

        // Project the recurrence series forward. Only schedule-type series have
        // deterministic future dates; completion-type series depend on unknown
        // future completion dates and cannot be projected, so they are skipped.
        for date in recurrence::project_series(&st.task, today, cutoff) {
            entries.push(ForecastEntry {
                date,
                id: short.clone(),
                title: st.task.title.clone(),
                score: st.score,
                projected: true,
            });
        }
    }

    // Order chronologically; concrete tasks sort before projected ones on the
    // same date so the current instance is shown ahead of its projections.
    entries.sort_by(|a, b| a.date.cmp(&b.date).then(a.projected.cmp(&b.projected)));

    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::domain::task::{Recurrence, Task};

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// An empty filter set (no implicit filtering disabled, no tokens).
    fn empty_filter() -> FilterSet {
        crate::core::FilterArgs::parse(Vec::new())
            .to_filter_set()
            .unwrap()
    }

    #[test]
    fn concrete_due_task_within_horizon_is_emitted() {
        let today = date(2026, 6, 6);
        let mut due_soon = Task::new("due soon");
        due_soon.due = Some(date(2026, 6, 10));
        let mut due_far = Task::new("due far");
        due_far.due = Some(date(2026, 12, 1));

        let all = vec![due_soon, due_far];
        let entries = build_entries(
            &all,
            &GlobalState::default(),
            &ScoringConfig::default(),
            &HashMap::new(),
            &empty_filter(),
            today,
            30,
        );
        let titles: Vec<&str> = entries.iter().map(|e| e.title.as_str()).collect();
        assert!(titles.contains(&"due soon"));
        assert!(!titles.contains(&"due far"), "beyond horizon must be excluded");
        assert!(entries.iter().all(|e| !e.projected));
    }

    #[test]
    fn schedule_recurrence_emits_projected_entries() {
        let today = date(2026, 6, 6);
        let mut task = Task::new("weekly standup");
        task.due = Some(date(2026, 6, 8));
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=WEEKLY".to_owned(),
            anchor: date(2026, 6, 8),
            snap: None,
        });

        let all = vec![task];
        let entries = build_entries(
            &all,
            &GlobalState::default(),
            &ScoringConfig::default(),
            &HashMap::new(),
            &empty_filter(),
            today,
            30,
        );
        // The concrete due date plus at least one projected occurrence.
        assert!(entries.iter().any(|e| !e.projected));
        assert!(
            entries.iter().any(|e| e.projected),
            "schedule series should project future occurrences"
        );
        // Sorted chronologically and concrete-before-projected on a shared date.
        for w in entries.windows(2) {
            assert!(
                (w[0].date, w[0].projected) <= (w[1].date, w[1].projected),
                "entries not sorted: {:?}",
                entries
            );
        }
    }

    #[test]
    fn future_start_task_appears_when_include_future_is_set() {
        let today = date(2026, 6, 6);
        let tomorrow = date(2026, 6, 7);
        let mut task = Task::new("starts tomorrow");
        task.start = Some(tomorrow);
        task.due = Some(tomorrow);

        let mut filter = empty_filter();
        filter.include_future = true;

        let entries = build_entries(
            &[task],
            &GlobalState::default(),
            &ScoringConfig::default(),
            &HashMap::new(),
            &filter,
            today,
            30,
        );
        let titles: Vec<&str> = entries.iter().map(|e| e.title.as_str()).collect();
        assert!(
            titles.contains(&"starts tomorrow"),
            "future-start task must appear in forecast when include_future is true"
        );
    }

    #[test]
    fn future_start_task_hidden_without_include_future() {
        let today = date(2026, 6, 6);
        let tomorrow = date(2026, 6, 7);
        let mut task = Task::new("starts tomorrow");
        task.start = Some(tomorrow);
        task.due = Some(tomorrow);

        let entries = build_entries(
            &[task],
            &GlobalState::default(),
            &ScoringConfig::default(),
            &HashMap::new(),
            &empty_filter(),
            today,
            30,
        );
        assert!(
            entries.is_empty(),
            "future-start task must be hidden without include_future"
        );
    }

    #[test]
    fn filter_token_restricts_entries() {
        let today = date(2026, 6, 6);
        let mut tagged = Task::new("tagged");
        tagged.due = Some(date(2026, 6, 10));
        tagged.tags = vec!["#rust".to_owned()];
        let mut plain = Task::new("plain");
        plain.due = Some(date(2026, 6, 10));

        let all = vec![tagged, plain];
        let filter_set = crate::core::FilterArgs::parse(vec!["+#rust".to_owned()])
            .to_filter_set()
            .unwrap();
        let entries = build_entries(
            &all,
            &GlobalState::default(),
            &ScoringConfig::default(),
            &HashMap::new(),
            &filter_set,
            today,
            30,
        );
        let titles: Vec<&str> = entries.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, vec!["tagged"], "filter token must restrict entries");
    }
}
