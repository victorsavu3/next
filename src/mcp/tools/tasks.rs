use chrono::Local;
use serde_json::{json, Value};

use crate::cli::commands::add::validate_url;
use crate::domain::{
    date_parse::parse_date,
    filter,
    recurrence::{parse_snap, spawn_next},
    scoring,
    tag,
    task::{Priority, Recurrence, Task},
};
use crate::resolve::resolve_task_id;
use crate::storage;
use crate::AppContext;

/// Validates a user-supplied slug.
///
/// Allowed characters: ASCII letters (`a-z`, `A-Z`), digits (`0-9`), hyphen (`-`),
/// and underscore (`_`).  This allowlist prevents path traversal (no `/`, `..`, or
/// null bytes) while keeping slugs clean identifiers.  Additional characters may be
/// permitted in future versions.
fn validate_slug(slug: &str) -> anyhow::Result<()> {
    if slug.is_empty() {
        anyhow::bail!("slug must not be empty");
    }
    if let Some(bad) = slug.chars().find(|c| !matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_')) {
        anyhow::bail!(
            "slug contains invalid character {bad:?} — only letters, digits, '-' and '_' are allowed"
        );
    }
    Ok(())
}

fn parse_priority(s: &str) -> anyhow::Result<Priority> {
    match s.to_lowercase().as_str() {
        "low" => Ok(Priority::Low),
        "medium" | "med" => Ok(Priority::Medium),
        "high" => Ok(Priority::High),
        _ => anyhow::bail!("unknown priority {s:?} — expected low, medium, or high"),
    }
}

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
    let filter_set = filter_args.to_filter_set()?;

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

    // Include direct children for context.
    let all_tasks = ctx.store.list_tasks()?;
    let children: Vec<&Task> = all_tasks.iter().filter(|t| t.parent_id == Some(id)).collect();

    Ok(json!({
        "task": task,
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

    let mut task = Task::new(title);

    if let Some(expr) = str_param(params, "due") {
        task.due = Some(parse_date(expr, today)?);
    }
    if let Some(expr) = str_param(params, "start") {
        task.start = Some(parse_date(expr, today)?);
    }
    if let Some(p) = str_param(params, "priority") {
        task.priority = parse_priority(p)?;
    }
    if let Some(s) = str_param(params, "slug") {
        validate_slug(s)?;
        task.slug = Some(s.to_owned());
    }
    task.assignee = str_param(params, "assignee").map(str::to_owned);
    task.description = str_param(params, "description").map(str::to_owned);
    task.notes = str_param(params, "notes").map(str::to_owned);
    task.long_term = bool_param(params, "long_term");

    if let Some(u) = str_param(params, "url") {
        validate_url(u)?;
        task.url = Some(u.to_owned());
    }
    if let Some(adj) = params.get("score_adjustment").and_then(|v| v.as_f64()) {
        task.score_adjustment = adj;
    }

    for t in strings_param(params, "tags") {
        tag::validate_tag(&t).map_err(|e| anyhow::anyhow!(e))?;
        task.tags.push(t);
    }
    if let Some(parent_ref) = str_param(params, "parent") {
        task.parent_id = Some(resolve_task_id(&*ctx.store, parent_ref)?);
    }
    for blocker_ref in strings_param(params, "blocked_by") {
        task.blocked_by.push(resolve_task_id(&*ctx.store, &blocker_ref)?);
    }

    if let Some(rule) = str_param(params, "recur_schedule") {
        let anchor = task.start.or(task.due).unwrap_or(today);
        let snap = str_param(params, "recur_snap").map(parse_snap).transpose()?;
        task.recurrence = Some(Recurrence::Schedule { rrule: rule.to_owned(), anchor, snap });
        task.recurrence_id = Some(task.id);
    } else if let Some(days) = params.get("recur_completion").and_then(|v| v.as_u64()) {
        let snap = str_param(params, "recur_snap").map(parse_snap).transpose()?;
        task.recurrence = Some(Recurrence::Completion { interval_days: days as u32, snap });
        task.recurrence_id = Some(task.id);
    }

    let task_path = storage::task_path(&ctx.repo_root, &task);
    ctx.store.save_task(&task)?;
    ctx.vcs.commit(&[task_path], &format!("next: add {}", task.title))?;
    ctx.log.info("mcp/add", &format!("[{}] {}", &task.id.to_string()[..8], task.title));

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
    let mut task = ctx.store.get_task(id)?;

    // ── State transition ────────────────────────────────────────────────────
    let action = str_param(params, "action");
    match action {
        Some("start")  => task.mark_started(),
        Some("stop")   => task.mark_stopped(),
        Some("cancel") => task.mark_cancelled(),
        Some("done") | Some("move") | None => {} // handled below
        Some(other) => anyhow::bail!("unknown action {other:?}: expected start, stop, done, cancel, move"),
    }

    // ── done with recurrence ────────────────────────────────────────────────
    if action == Some("done") {
        task.mark_done();
        let task_path = storage::task_path(&ctx.repo_root, &task);
        let mut paths = vec![task_path];
        if let Some(next_task) = spawn_next(&task, today)? {
            let next_path = storage::task_path(&ctx.repo_root, &next_task);
            ctx.store.save_task(&next_task)?;
            paths.push(next_path);
        }
        ctx.store.save_task(&task)?;
        ctx.vcs.commit(&paths, &format!("next: done {}", task.title))?;
        ctx.log.info("mcp/done", &format!("[{}] {}", &task.id.to_string()[..8], task.title));
        return Ok(serde_json::to_value(&task)?);
    }

    // ── Field edits ─────────────────────────────────────────────────────────
    if let Some(title) = str_param(params, "title") {
        task.title = title.to_owned();
    }
    if bool_param(params, "clear_due") {
        task.due = None;
    } else if let Some(expr) = str_param(params, "due") {
        task.due = Some(parse_date(expr, today)?);
    }
    if bool_param(params, "clear_start") {
        task.start = None;
    } else if let Some(expr) = str_param(params, "start") {
        task.start = Some(parse_date(expr, today)?);
    }
    if let Some(p) = str_param(params, "priority") {
        task.priority = parse_priority(p)?;
    }
    if let Some(slug) = str_param(params, "slug") {
        validate_slug(slug)?;
        task.slug = Some(slug.to_owned());
    }
    if bool_param(params, "clear_assignee") {
        task.assignee = None;
    } else if let Some(a) = str_param(params, "assignee") {
        task.assignee = Some(a.to_owned());
    }
    if bool_param(params, "clear_description") {
        task.description = None;
    } else if let Some(d) = str_param(params, "description") {
        task.description = Some(d.to_owned());
    }
    if bool_param(params, "clear_url") {
        task.url = None;
    } else if let Some(u) = str_param(params, "url") {
        validate_url(u)?;
        task.url = Some(u.to_owned());
    }
    if let Some(notes) = str_param(params, "notes") {
        task.notes = Some(notes.to_owned());
    }
    if let Some(adj) = params.get("score_adjustment").and_then(|v| v.as_f64()) {
        task.score_adjustment = adj;
    }
    if params.get("long_term").and_then(|v| v.as_bool()).unwrap_or(false) {
        task.long_term = true;
    }

    // ── Tags ─────────────────────────────────────────────────────────────────
    for t in strings_param(params, "add_tags") {
        tag::validate_tag(&t).map_err(|e| anyhow::anyhow!(e))?;
        if !task.tags.contains(&t) { task.tags.push(t); }
    }
    for t in strings_param(params, "remove_tags") {
        task.tags.retain(|existing| existing != &t);
    }

    // ── Parent / move ────────────────────────────────────────────────────────
    if bool_param(params, "clear_parent") {
        task.parent_id = None;
    } else if let Some(parent_ref) = str_param(params, "parent") {
        task.parent_id = Some(resolve_task_id(&*ctx.store, parent_ref)?);
    }

    // ── Blocked-by ───────────────────────────────────────────────────────────
    if bool_param(params, "clear_blocked_by") {
        task.blocked_by.clear();
    } else {
        for b in strings_param(params, "blocked_by") {
            let bid = resolve_task_id(&*ctx.store, &b)?;
            if !task.blocked_by.contains(&bid) { task.blocked_by.push(bid); }
        }
    }

    // ── Recurrence ───────────────────────────────────────────────────────────
    if bool_param(params, "clear_recurrence") {
        task.recurrence = None;
        task.recurrence_id = None;
    } else if let Some(rule) = str_param(params, "recur_schedule") {
        let anchor = match &task.recurrence {
            Some(Recurrence::Schedule { anchor, .. }) => *anchor,
            _ => task.start.or(task.due).unwrap_or(today),
        };
        let snap = str_param(params, "recur_snap").map(parse_snap).transpose()?;
        task.recurrence = Some(Recurrence::Schedule { rrule: rule.to_owned(), anchor, snap });
        task.recurrence_id.get_or_insert(task.id);
    } else if let Some(days) = params.get("recur_completion").and_then(|v| v.as_u64()) {
        let snap = str_param(params, "recur_snap").map(parse_snap).transpose()?;
        task.recurrence = Some(Recurrence::Completion { interval_days: days as u32, snap });
        task.recurrence_id.get_or_insert(task.id);
    }

    task.touch();
    let task_path = storage::task_path(&ctx.repo_root, &task);
    ctx.store.save_task(&task)?;

    let verb = action.unwrap_or("edit");
    ctx.vcs.commit(&[task_path], &format!("next: {verb} {}", task.title))?;
    ctx.log.info(&format!("mcp/{verb}"), &format!("[{}] {}", &task.id.to_string()[..8], task.title));

    Ok(serde_json::to_value(&task)?)
}

// ── delete_task ───────────────────────────────────────────────────────────────

pub fn delete_task(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
    let id_str = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: id"))?;

    let id = resolve_task_id(&*ctx.store, id_str)?;
    let task = ctx.store.get_task(id)?;
    let task_path = storage::task_path(&ctx.repo_root, &task);

    ctx.store.delete_task(id)?;
    ctx.vcs.commit(&[task_path], &format!("next: delete {}", task.title))?;
    ctx.log.info("mcp/delete", &format!("[{}] {}", &task.id.to_string()[..8], task.title));

    Ok(json!({ "deleted": task.id.to_string(), "title": task.title }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, AppContext, log::Logger};
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
        let log = Logger::new(dir.path());
        let ctx = AppContext {
            config: Config::default(),
            store: Box::new(store),
            vcs: Box::new(vcs),
            repo_root: dir.path().to_path_buf(),
            log,
        };
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
}
