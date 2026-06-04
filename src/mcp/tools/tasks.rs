use chrono::Local;
use serde_json::{json, Value};

use crate::domain::{
    date_parse::parse_date,
    filter,
    recurrence::parse_snap,
    scoring,
    service::{apply_edits, complete_task, create_task, CreateTaskParams, EditTaskParams},
    task::{Recurrence, Task},
};
use crate::resolve::resolve_task_id;
use crate::storage;
use crate::AppContext;

fn str_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(|v| v.as_str())
}

fn bool_param(params: &Value, key: &str) -> bool {
    params.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn strings_param(params: &Value, key: &str) -> Vec<String> {
    params
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

// ── list_tasks ───────────────────────────────────────────────────────────────

pub fn list_tasks(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
    let today = Local::now().date_naive();
    let tokens = strings_param(params, "filter_tokens");
    let include_all = bool_param(params, "include_all");
    let limit: Option<usize> = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    let mut filter_args = crate::cli::filter::FilterArgs::parse(tokens);
    filter_args.all = include_all;
    let mut filter_set = filter_args.to_filter_set()?;

    // `context` param overrides the active context from state for this call.
    if params.get("context").is_some() {
        filter_set.context_override = Some(strings_param(params, "context"));
    }

    let state = ctx.store.get_state()?;
    let all_tasks = ctx.store.list_tasks()?;
    let tag_metas = ctx.store.list_tag_metas()?;

    let filtered = filter::apply(all_tasks.clone(), &filter_set, &state, today);
    let mut scored = scoring::score_and_sort(filtered, &all_tasks, today, &ctx.config.scoring, &tag_metas);

    if let Some(n) = limit {
        scored.truncate(n);
    }

    Ok(serde_json::to_value(&scored)?)
}

// ── get_task ─────────────────────────────────────────────────────────────────

pub fn get_task(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
    let id_str = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: id"))?;

    let id = resolve_task_id(&*ctx.store, id_str)?;
    let task = ctx.store.get_task(id)?;

    let today = Local::now().date_naive();
    let all_tasks = ctx.store.list_tasks()?;
    let tag_metas = ctx.store.list_tag_metas()?;
    let parent = task.parent_id.and_then(|pid| all_tasks.iter().find(|t| t.id == pid));
    let breakdown = scoring::score_with_breakdown(&task, parent, today, &ctx.config.scoring, &tag_metas);

    let children: Vec<&Task> = all_tasks.iter().filter(|t| t.parent_id == Some(id)).collect();

    Ok(json!({
        "task": task,
        "score": breakdown.total,
        "score_breakdown": breakdown,
        "children": children,
    }))
}

// ── add_task ─────────────────────────────────────────────────────────────────

pub fn add_task(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
    let today = Local::now().date_naive();

    let title = params
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: title"))?
        .to_owned();

    let due = str_param(params, "due")
        .map(|expr| parse_date(expr, today))
        .transpose()?;
    let start = str_param(params, "start")
        .map(|expr| parse_date(expr, today))
        .transpose()?;

    let recurrence = if let Some(rule) = str_param(params, "recur_schedule") {
        let anchor = start.or(due).unwrap_or(today);
        let snap = str_param(params, "recur_snap").map(parse_snap).transpose()?;
        Some(Recurrence::Schedule { rrule: rule.to_owned(), anchor, snap })
    } else if let Some(days) = params.get("recur_completion").and_then(|v| v.as_u64()) {
        let snap = str_param(params, "recur_snap").map(parse_snap).transpose()?;
        Some(Recurrence::Completion { interval_days: days as u32, snap })
    } else {
        None
    };

    let service_params = CreateTaskParams {
        due,
        start,
        priority: str_param(params, "priority").map(str::to_owned),
        slug: str_param(params, "slug").map(str::to_owned),
        assignee: str_param(params, "assignee").map(str::to_owned),
        description: str_param(params, "description").map(str::to_owned),
        notes: str_param(params, "notes").map(str::to_owned),
        long_term: bool_param(params, "long_term"),
        url: str_param(params, "url").map(str::to_owned),
        score_adjustment: params.get("score_adjustment").and_then(|v| v.as_f64()),
        tags: strings_param(params, "tags"),
        parent: str_param(params, "parent").map(str::to_owned),
        blocked_by: strings_param(params, "blocked_by"),
        recurrence,
    };

    let task = create_task(
        title,
        service_params,
        today,
        &ctx.repo_root.clone(),
        &mut *ctx.store,
        &*ctx.vcs,
    )?;

    tracing::info!(cmd = "mcp/add", "[{}] {}", &task.id.to_string()[..8], task.title);

    Ok(serde_json::to_value(&task)?)
}

// ── update_task ───────────────────────────────────────────────────────────────

pub fn update_task(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
    let today = Local::now().date_naive();
    let id_str = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: id"))?;

    let id = resolve_task_id(&*ctx.store, id_str)?;

    // ── State transitions that are NOT field edits ──────────────────────────
    let action = str_param(params, "action");
    match action {
        Some("start") => {
            let task = ctx.transaction(|store, vcs, root| {
                let mut task = store.get_task(id)?;
                task.mark_started();
                let task_path = storage::task_path(root, &task);
                store.save_task(&task)?;
                vcs.commit(&[task_path], &format!("next: start {}", task.title))?;
                Ok(task)
            })?;
            tracing::info!(cmd = "mcp/start", "[{}] {}", &task.id.to_string()[..8], task.title);
            return Ok(serde_json::to_value(&task)?);
        }
        Some("stop") => {
            let task = ctx.transaction(|store, vcs, root| {
                let mut task = store.get_task(id)?;
                task.mark_stopped();
                let task_path = storage::task_path(root, &task);
                store.save_task(&task)?;
                vcs.commit(&[task_path], &format!("next: stop {}", task.title))?;
                Ok(task)
            })?;
            tracing::info!(cmd = "mcp/stop", "[{}] {}", &task.id.to_string()[..8], task.title);
            return Ok(serde_json::to_value(&task)?);
        }
        Some("cancel") => {
            let task = ctx.transaction(|store, vcs, root| {
                let mut task = store.get_task(id)?;
                task.mark_cancelled();
                let task_path = storage::task_path(root, &task);
                store.save_task(&task)?;
                vcs.commit(&[task_path], &format!("next: cancel {}", task.title))?;
                Ok(task)
            })?;
            tracing::info!(cmd = "mcp/cancel", "[{}] {}", &task.id.to_string()[..8], task.title);
            return Ok(serde_json::to_value(&task)?);
        }
        Some("done") => {
            let completion_date = str_param(params, "completed_at")
                .map(|expr| parse_date(expr, today))
                .transpose()?
                .unwrap_or(today);
            let task = complete_task(
                id,
                completion_date,
                &ctx.repo_root.clone(),
                &mut *ctx.store,
                &*ctx.vcs,
            )?;
            tracing::info!(cmd = "mcp/done", "[{}] {}", &task.id.to_string()[..8], task.title);
            return Ok(serde_json::to_value(&task)?);
        }
        Some("move") | None => {} // fall through to field-edit path
        Some(other) => anyhow::bail!("unknown action {other:?}: expected start, stop, done, cancel, move"),
    }

    // ── Field edits (including optional move/parent update) ─────────────────

    // Build recurrence update from params.
    let recurrence: Option<Recurrence> = if bool_param(params, "clear_recurrence") {
        None // handled via clear_recurrence flag
    } else if let Some(rule) = str_param(params, "recur_schedule") {
        let existing = ctx.store.get_task(id)?;
        let anchor = match &existing.recurrence {
            Some(Recurrence::Schedule { anchor, .. }) => *anchor,
            _ => existing.start.or(existing.due).unwrap_or(today),
        };
        let snap = str_param(params, "recur_snap").map(parse_snap).transpose()?;
        Some(Recurrence::Schedule { rrule: rule.to_owned(), anchor, snap })
    } else if let Some(days) = params.get("recur_completion").and_then(|v| v.as_u64()) {
        let snap = str_param(params, "recur_snap").map(parse_snap).transpose()?;
        Some(Recurrence::Completion { interval_days: days as u32, snap })
    } else {
        None
    };

    let due = if bool_param(params, "clear_due") {
        None
    } else {
        str_param(params, "due")
            .map(|expr| parse_date(expr, today))
            .transpose()?
    };
    let start = if bool_param(params, "clear_start") {
        None
    } else {
        str_param(params, "start")
            .map(|expr| parse_date(expr, today))
            .transpose()?
    };

    let edits = EditTaskParams {
        title: str_param(params, "title").map(str::to_owned),
        due,
        clear_due: bool_param(params, "clear_due"),
        start,
        clear_start: bool_param(params, "clear_start"),
        priority: str_param(params, "priority").map(str::to_owned),
        slug: str_param(params, "slug").map(str::to_owned),
        assignee: str_param(params, "assignee").map(str::to_owned),
        clear_assignee: bool_param(params, "clear_assignee"),
        add_tags: strings_param(params, "add_tags"),
        remove_tags: strings_param(params, "remove_tags"),
        parent: str_param(params, "parent").map(str::to_owned),
        clear_parent: bool_param(params, "clear_parent"),
        blocked_by: strings_param(params, "blocked_by"),
        clear_blocked_by: bool_param(params, "clear_blocked_by"),
        description: str_param(params, "description").map(str::to_owned),
        clear_description: bool_param(params, "clear_description"),
        url: str_param(params, "url").map(str::to_owned),
        clear_url: bool_param(params, "clear_url"),
        notes: str_param(params, "notes").map(str::to_owned),
        recurrence,
        clear_recurrence: bool_param(params, "clear_recurrence"),
        long_term: if params.get("long_term").and_then(|v| v.as_bool()).unwrap_or(false) {
            Some(true)
        } else {
            None
        },
        score_adjustment: params.get("score_adjustment").and_then(|v| v.as_f64()),
    };

    let task = apply_edits(
        id,
        edits,
        today,
        &ctx.repo_root.clone(),
        &mut *ctx.store,
        &*ctx.vcs,
    )?;

    let verb = action.unwrap_or("edit");
    tracing::info!(cmd = %format!("mcp/{verb}"), "[{}] {}", &task.id.to_string()[..8], task.title);

    Ok(serde_json::to_value(&task)?)
}

// ── delete_task ───────────────────────────────────────────────────────────────

pub fn delete_task(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
    let id_str = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: id"))?;

    let id = resolve_task_id(&*ctx.store, id_str)?;
    let task = ctx.transaction(|store, vcs, root| {
        let task = store.get_task(id)?;
        let task_path = storage::task_path(root, &task);
        store.delete_task(id)?;
        vcs.commit(&[task_path], &format!("next: delete {}", task.title))?;
        Ok(task)
    })?;
    tracing::info!(cmd = "mcp/delete", "[{}] {}", &task.id.to_string()[..8], task.title);

    Ok(json!({ "deleted": task.id.to_string(), "title": task.title }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, AppContext};
    use tempfile::TempDir;

    fn make_ctx() -> (TempDir, AppContext) {
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
        let (store, vcs) = crate::storage::open(dir.path().to_path_buf()).unwrap();
        let ctx = AppContext::with_parts(Config::default(), Box::new(store), Box::new(vcs), dir.path().to_path_buf());
        (dir, ctx)
    }

    #[test]
    fn add_and_list_task() {
        let (_dir, mut ctx) = make_ctx();
        let params = serde_json::json!({ "title": "Buy milk" });
        let result = add_task(&params, &mut ctx).unwrap();
        assert_eq!(result["title"], "Buy milk");

        let list = list_tasks(&serde_json::json!({}), &mut ctx).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
    }

    #[test]
    fn get_task_returns_task_and_children() {
        let (_dir, mut ctx) = make_ctx();
        let parent = add_task(&json!({ "title": "Parent", "slug": "parent" }), &mut ctx).unwrap();
        let parent_id = parent["id"].as_str().unwrap().to_owned();
        add_task(&json!({ "title": "Child", "parent": "parent" }), &mut ctx).unwrap();

        let result = get_task(&json!({ "id": parent_id }), &mut ctx).unwrap();
        assert_eq!(result["task"]["title"], "Parent");
        assert_eq!(result["children"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn update_task_start_stop() {
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(&json!({ "title": "Work" }), &mut ctx).unwrap();
        let id = task["id"].as_str().unwrap().to_owned();

        let started = update_task(&json!({ "id": id, "action": "start" }), &mut ctx).unwrap();
        assert_eq!(started["status"], "started");

        let stopped = update_task(&json!({ "id": id, "action": "stop" }), &mut ctx).unwrap();
        assert_eq!(stopped["status"], "open");
    }

    #[test]
    fn update_task_done_with_recurrence_spawns_next() {
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(
            &json!({ "title": "Daily standup", "recur_completion": 1 }),
            &mut ctx,
        )
        .unwrap();
        let id = task["id"].as_str().unwrap().to_owned();

        update_task(&json!({ "id": id, "action": "done" }), &mut ctx).unwrap();

        // Original is done; a new instance should have been spawned.
        let all = list_tasks(&json!({ "include_all": true }), &mut ctx).unwrap();
        let tasks = all.as_array().unwrap();
        assert_eq!(tasks.len(), 2, "expected original + spawned next instance");
    }

    #[test]
    fn delete_task_removes_it() {
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(&json!({ "title": "To delete" }), &mut ctx).unwrap();
        let id = task["id"].as_str().unwrap();

        delete_task(&json!({ "id": id }), &mut ctx).unwrap();

        let list = list_tasks(&json!({ "include_all": true }), &mut ctx).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 0);
    }

    #[test]
    fn update_task_unknown_action_errors() {
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(&json!({ "title": "X" }), &mut ctx).unwrap();
        let id = task["id"].as_str().unwrap();
        let err = update_task(&json!({ "id": id, "action": "fly" }), &mut ctx).unwrap_err();
        assert!(err.to_string().contains("unknown action"));
    }

    #[test]
    fn update_task_cancel() {
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(&json!({ "title": "Cancel me" }), &mut ctx).unwrap();
        let id = task["id"].as_str().unwrap().to_owned();

        let cancelled = update_task(&json!({ "id": id, "action": "cancel" }), &mut ctx).unwrap();
        assert_eq!(cancelled["status"], "cancelled");
    }
}
