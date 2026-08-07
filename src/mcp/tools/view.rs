use chrono::{Local, NaiveDate};
use serde::Serialize;
use serde_json::Value;

use crate::core::FilterArgs;
use crate::core::{domain::filter, recurrence, scoring};
use crate::TaskRepository;

// ── get_forecast ──────────────────────────────────────────────────────────────

/// A single occurrence in the forecast: either a concrete existing task whose
/// stored `due` falls within the horizon, or a projected (not-yet-spawned)
/// future instance of an active schedule-type recurrence series.
#[derive(Debug, Clone, Serialize)]
struct ForecastEntry {
    /// The forecast date (the task's `due`, or the projected occurrence date).
    date: NaiveDate,
    /// Short 8-char id of the originating task.
    id: String,
    title: String,
    /// Urgency score of the originating task.
    score: f64,
    /// `true` for projected future occurrences that do not yet exist as tasks.
    projected: bool,
}

pub fn get_forecast(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let today = Local::now().date_naive();
    let horizon: u32 = params
        .get("horizon_days")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .unwrap_or(crate::core::config::DEFAULT_FORECAST_HORIZON_DAYS);

    let tokens: Vec<String> = params
        .get("filter_tokens")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    let filter_args = FilterArgs::parse(tokens)?;
    let mut filter_set = filter_args.to_filter_set()?;

    // `context` param overrides the active context from state for this call.
    if params.get("context").is_some() {
        let ctx_tags: Vec<String> = params
            .get("context")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        filter_set.required_override = Some(ctx_tags);
    }

    let state = ctx.store.get_state()?;
    let candidates = crate::core::listing::load_candidates(&*ctx.store, &filter_set)?;
    let tag_metas = ctx.store.list_tag_metas()?;

    // Dates before filtering: `created:` and `updated:` are query terms.
    let pool = crate::core::listing::extend_with_parents(&*ctx.store, candidates.clone())?;
    let task_dates = ctx.task_git_dates_for(&pool);

    let filtered = filter::apply(candidates, &filter_set, &state, today, &task_dates);
    let scored = scoring::score_and_sort(
        filtered,
        &pool,
        today,
        &ctx.scoring,
        &tag_metas,
        &task_dates,
    );

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

        // Project active schedule-type recurrence series forward. Completion-type,
        // done, and cancelled tasks are skipped inside `project_series`.
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

    Ok(serde_json::to_value(&entries)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskRepository;
    use tempfile::TempDir;

    fn make_ctx() -> (TempDir, TaskRepository) {
        let dir = tempfile::tempdir().unwrap();
        crate::core::test_git::init_test_repo(dir.path());
        let (store, vcs) = crate::core::storage::open(dir.path().to_path_buf()).unwrap();
        let ctx =
            TaskRepository::with_parts(Box::new(store), Box::new(vcs), dir.path().to_path_buf());
        (dir, ctx)
    }

    #[test]
    fn forecast_empty_repo() {
        let (_dir, mut ctx) = make_ctx();
        let result = get_forecast(&serde_json::json!({}), &mut ctx).unwrap();
        assert_eq!(result.as_array().unwrap().len(), 0);
    }

    #[test]
    fn forecast_includes_task_due_soon() {
        let (_dir, mut ctx) = make_ctx();
        use crate::core::domain::task::Task;
        use crate::core::storage;
        let mut task = Task::new("Fix bug".to_owned());
        task.due = Some(Local::now().date_naive() + chrono::Duration::days(3));
        let path = storage::task_path(&ctx.repo_root, &task);
        ctx.store.save_task(&task).unwrap();
        ctx.vcs.commit(&[path], "next: add Fix bug").unwrap();

        let result = get_forecast(&serde_json::json!({ "horizon_days": 7 }), &mut ctx).unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        // Concrete due tasks are not marked projected.
        assert_eq!(arr[0]["projected"], serde_json::json!(false));
    }

    #[test]
    fn forecast_projects_schedule_recurrence() {
        let (_dir, mut ctx) = make_ctx();
        use crate::core::domain::task::{Recurrence, Task};
        use crate::core::storage;
        let today = Local::now().date_naive();
        let mut task = Task::new("Weekly review".to_owned());
        // Anchor on today's weekday so projected occurrences fall weekly ahead.
        task.due = Some(today);
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=WEEKLY".into(),
            anchor: today,
            snap: None,
        });
        let path = storage::task_path(&ctx.repo_root, &task);
        ctx.store.save_task(&task).unwrap();
        ctx.vcs.commit(&[path], "next: add Weekly review").unwrap();

        // 30-day horizon → the concrete instance plus 4 projected weekly occurrences.
        let result = get_forecast(&serde_json::json!({ "horizon_days": 30 }), &mut ctx).unwrap();
        let arr = result.as_array().unwrap();
        let projected: Vec<_> = arr
            .iter()
            .filter(|e| e["projected"] == serde_json::json!(true))
            .collect();
        assert_eq!(projected.len(), 4);
        assert!(arr
            .iter()
            .any(|e| e["projected"] == serde_json::json!(false)));
    }

    #[test]
    fn forecast_done_task_not_shown_completion_task_projected() {
        let (_dir, mut ctx) = make_ctx();
        use crate::core::domain::task::{Recurrence, Status, Task};
        use crate::core::storage;
        let today = Local::now().date_naive();

        // Completion-type recurrence: concrete instance + projected future occurrences.
        let mut completion = Task::new("Water plants".to_owned());
        completion.due = Some(today + chrono::Duration::days(2));
        completion.recurrence = Some(Recurrence::Completion {
            interval_days: 7,
            snap: None,
        });
        let p1 = storage::task_path(&ctx.repo_root, &completion);
        ctx.store.save_task(&completion).unwrap();
        ctx.vcs.commit(&[p1], "next: add Water plants").unwrap();

        // Done schedule task: filtered out entirely (done is excluded by default).
        let mut done = Task::new("Old review".to_owned());
        done.due = Some(today + chrono::Duration::days(1));
        done.status = Status::Done;
        done.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=WEEKLY".into(),
            anchor: today,
            snap: None,
        });
        let p2 = storage::task_path(&ctx.repo_root, &done);
        ctx.store.save_task(&done).unwrap();
        ctx.vcs.commit(&[p2], "next: add Old review").unwrap();

        let result = get_forecast(&serde_json::json!({ "horizon_days": 30 }), &mut ctx).unwrap();
        let arr = result.as_array().unwrap();
        // Done task must not appear at all.
        assert!(arr
            .iter()
            .all(|e| e["title"] != serde_json::json!("Old review")));
        // Completion-type task: one concrete + projected occurrences (7-day interval, 30-day horizon).
        let concrete: Vec<_> = arr
            .iter()
            .filter(|e| e["projected"] == serde_json::json!(false))
            .collect();
        let projected: Vec<_> = arr
            .iter()
            .filter(|e| e["projected"] == serde_json::json!(true))
            .collect();
        assert_eq!(concrete.len(), 1, "one concrete instance expected");
        assert!(
            !projected.is_empty(),
            "projected occurrences expected for completion recurrence"
        );
    }
}
