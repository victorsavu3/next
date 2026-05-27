use std::collections::HashMap;

use chrono::NaiveDate;
use uuid::Uuid;

use crate::{
    config::ScoringConfig,
    domain::task::{Priority, Task},
};

/// A task paired with its computed urgency score.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScoredTask {
    pub task: Task,
    pub score: f64,
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
/// Returns 0.0 when age scoring is disabled for the task (long-term or future start date).
pub fn age_factor(task: &Task, today: NaiveDate, w: &ScoringConfig) -> f64 {
    if task.age_scoring_disabled(today) {
        return 0.0;
    }
    let age_days = (today - task.created_at.date_naive()).num_days().max(0) as f64;
    (age_days * w.age_per_day).min(w.age_max)
}

/// Computes the total urgency score for `task`.
///
/// `parent` is the parent task (if any), used for the project-priority factor.
pub fn score(task: &Task, parent: Option<&Task>, today: NaiveDate, w: &ScoringConfig) -> f64 {
    due_factor(task.due, today, w)
        + priority_factor(&task.priority, w)
        + project_factor(parent.map(|p| &p.priority), w)
        + age_factor(task, today, w)
        + task.score_adjustment
}

/// Scores each task in `tasks`, sorts by score descending (most urgent first),
/// and returns the paired results.
///
/// `all_tasks` is used to look up parent tasks for the project-priority factor.
/// It should be the full, unfiltered task list.
pub fn score_and_sort(
    tasks: Vec<Task>,
    all_tasks: &[Task],
    today: NaiveDate,
    w: &ScoringConfig,
) -> Vec<ScoredTask> {
    let by_id: HashMap<Uuid, &Task> = all_tasks.iter().map(|t| (t.id, t)).collect();

    let mut scored: Vec<ScoredTask> = tasks
        .into_iter()
        .map(|task| {
            let parent = task.parent_id.and_then(|id| by_id.get(&id).copied());
            let s = score(&task, parent, today, w);
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
        let task = Task::new("new task"); // created_at ≈ now
        let score = age_factor(&task, today(), &weights());
        // Created just now so age in days ≈ 0 → factor ≈ 0
        assert!(score < 0.01);
    }

    #[test]
    fn age_factor_long_term_returns_zero() {
        let mut task = Task::new("long-term");
        task.long_term = true;
        assert_eq!(age_factor(&task, today(), &weights()), 0.0);
    }

    #[test]
    fn age_factor_future_start_returns_zero() {
        let mut task = Task::new("future");
        task.start = Some(today() + chrono::Duration::days(7));
        assert_eq!(age_factor(&task, today(), &weights()), 0.0);
    }

    #[test]
    fn age_factor_caps_at_max() {
        let mut task = Task::new("old task");
        // Set created_at far in the past to force the cap.
        // age_max=2.0, age_per_day=0.01 → 200 days to reach cap.
        let old_ts = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        task.created_at = old_ts;
        let score = age_factor(&task, today(), &weights());
        assert_eq!(score, 2.0); // capped at age_max
    }

    // --- score ---

    #[test]
    fn score_no_due_medium_priority_no_parent() {
        let task = Task::new("simple task");
        let s = score(&task, None, today(), &weights());
        // priority_medium=1.0, no due, no parent, age≈0
        assert!((s - 1.0).abs() < 0.01);
    }

    #[test]
    fn score_overdue_task_ranks_higher_than_no_due() {
        let mut overdue = Task::new("overdue");
        overdue.due = Some(today() - chrono::Duration::days(3));
        let normal = Task::new("no due");

        let w = weights();
        assert!(score(&overdue, None, today(), &w) > score(&normal, None, today(), &w));
    }

    #[test]
    fn score_adjustment_applied() {
        let mut task = Task::new("boosted");
        task.score_adjustment = 5.0;
        let s = score(&task, None, today(), &weights());
        assert!((s - 6.0).abs() < 0.01); // 1.0 (medium) + 5.0 (adj) + 0 (age)
    }

    #[test]
    fn score_and_sort_orders_descending() {
        let mut urgent = Task::new("urgent");
        urgent.due = Some(today() - chrono::Duration::days(1)); // overdue

        let normal = Task::new("normal");
        // Default medium priority, no due

        let all = vec![normal.clone(), urgent.clone()];
        let scored = score_and_sort(vec![normal, urgent], &all, today(), &weights());

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
        let scored = score_and_sort(vec![child, orphan], &all, today(), &weights());

        // child gets +0.5 from high-priority parent; orphan gets +0
        let child_score = scored.iter().find(|s| s.task.title == "child task").unwrap().score;
        let orphan_score = scored.iter().find(|s| s.task.title == "no parent").unwrap().score;
        assert!(child_score > orphan_score);
    }
}
