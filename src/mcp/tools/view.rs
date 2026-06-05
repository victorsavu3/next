use chrono::Local;
use serde_json::Value;

use crate::core::FilterArgs;
use crate::core::{domain::filter, scoring};
use crate::TaskRepository;

// ── get_forecast ──────────────────────────────────────────────────────────────

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
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();

    let filter_args = FilterArgs::parse(tokens);
    let mut filter_set = filter_args.to_filter_set()?;

    // `context` param overrides the active context from state for this call.
    if params.get("context").is_some() {
        let ctx_tags: Vec<String> = params
            .get("context")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        filter_set.context_override = Some(ctx_tags);
    }

    let state = ctx.store.get_state()?;
    let all_tasks = ctx.store.list_tasks()?;
    let tag_metas = ctx.store.list_tag_metas()?;

    let filtered = filter::apply(all_tasks.clone(), &filter_set, &state, today);
    let scored = scoring::score_and_sort(filtered, &all_tasks, today, &ctx.scoring, &tag_metas);

    let cutoff = today + chrono::Duration::days(horizon as i64);
    let due_tasks: Vec<_> = scored
        .into_iter()
        .filter(|st| st.task.due.is_some_and(|d| d <= cutoff))
        .collect();

    Ok(serde_json::to_value(&due_tasks)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskRepository;
    use tempfile::TempDir;

    fn make_ctx() -> (TempDir, TaskRepository) {
        let dir = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "test@test.com"],
            vec!["config", "user.name", "Test"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(dir.path())
                .status()
                .unwrap();
        }
        let (store, vcs) = crate::core::storage::open(dir.path().to_path_buf()).unwrap();
        let ctx = TaskRepository::with_parts(Box::new(store), Box::new(vcs), dir.path().to_path_buf());
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
        assert_eq!(result.as_array().unwrap().len(), 1);
    }
}
