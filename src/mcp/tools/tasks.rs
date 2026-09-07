use chrono::Local;
use serde_json::{json, Value};

use crate::core::domain::{
    date_parse::parse_date,
    filter,
    task::{Recurrence, Task},
};
use crate::core::recurrence::{
    edit_anchor, parse_snap, resolve_snap_leeway, validate_recurrence, validate_rrule, SnapEdit,
    MCP_SNAP_PARAMS,
};
use crate::core::resolve::resolve_task_id;
use crate::core::scoring;
use crate::core::service::{
    apply_edits, complete_task, create_task, CreateTaskParams, EditTaskParams,
};
use crate::core::storage;
use crate::TaskRepository;

fn str_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(|v| v.as_str())
}

fn bool_param(params: &Value, key: &str) -> bool {
    params.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// The `recur_completion` interval, as the `u32` the domain stores.
///
/// JSON has no integer width, so `as_u64` happily accepts a value no `u32` can
/// hold and `as u32` then wraps it without a word — `5000000000` became
/// `705032704`, a rule nobody asked for. A negative or fractional value is
/// rejected here too rather than read as "no recurrence at all", which is what
/// a bare `as_u64()` silently did.
fn completion_interval(params: &Value) -> anyhow::Result<Option<u32>> {
    let value = match params.get("recur_completion") {
        None => return Ok(None),
        Some(v) if v.is_null() => return Ok(None),
        Some(v) => v,
    };
    let days = value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "recur_completion must be a whole number of days between 1 and {}, got {value}",
                u32::MAX
            )
        })?;
    Ok(Some(days))
}

fn strings_param(params: &Value, key: &str) -> Vec<String> {
    params
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Parameter keys that represent a field edit (as opposed to a state
/// transition). Used by `update_task` to decide whether the field-edit path
/// should run in addition to any `action`, so edits are never silently dropped.
const EDIT_PARAM_KEYS: &[&str] = &[
    "title",
    "due",
    "clear_due",
    "start",
    "clear_start",
    "priority",
    "slug",
    "assignee",
    "clear_assignee",
    "add_tags",
    "remove_tags",
    "parent",
    "clear_parent",
    "blocked_by",
    "clear_blocked_by",
    "description",
    "clear_description",
    "url",
    "clear_url",
    "notes",
    "recur_schedule",
    "recur_completion",
    "recur_snap",
    "clear_recur_snap",
    "recur_snap_leeway",
    "clear_recur_snap_leeway",
    "clear_recurrence",
    "long_term",
    "score_adjustment",
];

fn has_edit_params(params: &Value) -> bool {
    EDIT_PARAM_KEYS
        .iter()
        .any(|k| params.get(*k).is_some_and(|v| !v.is_null()))
}

// ── list_tasks ───────────────────────────────────────────────────────────────

/// The `filter` expression for this call, empty when absent.
///
/// One string, identical to what the CLI takes — that sameness is the point of
/// the parameter, so an agent and a person can copy a query between them.
pub(crate) fn filter_string(params: &Value) -> String {
    params
        .get("filter")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned()
}

/// Refuses the parameters S4 removed, naming their replacement.
///
/// An agent holding the old schema would otherwise get a generic "unknown
/// parameter" — or worse, silence, since an ignored `filter_tokens` reads as
/// "no filter" and returns the entire list looking perfectly successful. One
/// error turns a confusing failure into a self-service fix.
pub(crate) fn reject_removed_filter_params(params: &Value) -> anyhow::Result<()> {
    if params.get("filter_tokens").is_some() {
        anyhow::bail!(
            "filter_tokens was replaced by filter; pass the whole expression as one string, \
             e.g. filter: \"+@work due<+7d\""
        );
    }
    if params.get("context").is_some() {
        anyhow::bail!(
            "the context parameter was removed; put the tag in filter instead, \
             e.g. filter: \"+@work\""
        );
    }
    Ok(())
}

pub fn list_tasks(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let today = Local::now().date_naive();
    let include_all = bool_param(params, "include_all");
    let limit: Option<u32> = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    let page: u32 = params.get("page").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    // `limit` is the legacy name for the same cap; `page_size` wins when both given.
    let page_size: u32 = params
        .get("page_size")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
        .or(limit)
        .unwrap_or(crate::core::store::DEFAULT_PAGE_SIZE);

    reject_removed_filter_params(params)?;
    let projection = crate::core::projection::Projection::parse(&strings_param(params, "fields"))?;
    let mut filter_args = crate::core::FilterArgs::parse_query(&filter_string(params))?;
    filter_args.all = include_all;
    let filter_set = filter_args.to_filter_set()?;

    // Archived view: the same grammar as the active tier, plus pagination; no
    // scoring and no implicit gate, most recently completed first.
    if bool_param(params, "archived") {
        let result = ctx.store.query_tasks(&crate::core::TaskQuery {
            archived: true,
            filter: Some(filter_set.to_store_filter(today)?),
            page,
            page_size,
            ..Default::default()
        })?;
        let mut json = serde_json::to_value(&result)?;
        projection.apply_to_page(&mut json);
        return Ok(json);
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

    let result = crate::core::store::paginate(scored, page, page_size);
    let mut json = serde_json::to_value(&result)?;
    projection.apply_to_page(&mut json);
    Ok(json)
}

// ── get_task ─────────────────────────────────────────────────────────────────

pub fn get_task(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let id_str = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: id"))?;

    let id = resolve_task_id(&*ctx.store, id_str)?;
    let task = ctx.store.get_task(id)?;

    let today = Local::now().date_naive();
    let tag_metas = ctx.store.list_tag_metas()?;
    let parent = match task.parent_id {
        Some(pid) => ctx.store.get_tasks(&[pid])?.pop(),
        None => None,
    };
    let children = ctx
        .store
        .query_tasks(&crate::core::TaskQuery {
            parent_id: Some(id),
            ..crate::core::TaskQuery::unpaginated()
        })?
        .items;

    let mut dated: Vec<Task> = vec![task.clone()];
    dated.extend(parent.clone());
    let task_dates = ctx.task_git_dates_for(&dated);
    let breakdown = scoring::score_with_breakdown(
        &task,
        parent.as_ref(),
        &task_dates,
        today,
        &ctx.scoring,
        &tag_metas,
    );

    let projection = crate::core::projection::Projection::parse(&strings_param(params, "fields"))?;
    let mut out = json!({
        "task": task,
        "score": breakdown.total,
        "score_breakdown": breakdown,
        "children": children,
    });
    // Children get the same shape as the task — a caller asking for `id,title`
    // wants that throughout, not one trimmed task beside a set of full ones —
    // and `score`/`score_breakdown` go unless named, as they do in a listing.
    projection.apply_to_detail(&mut out);
    Ok(out)
}

// ── add_task ─────────────────────────────────────────────────────────────────

pub fn add_task(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
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

    let snap = str_param(params, "recur_snap")
        .map(parse_snap)
        .transpose()?;
    let snap_leeway = resolve_snap_leeway(str_param(params, "recur_snap_leeway"), false)?;
    let recurrence = if let Some(rule) = str_param(params, "recur_schedule") {
        validate_rrule(rule)
            .map_err(|e| anyhow::anyhow!("invalid recurrence rule {rule:?}: {e}"))?;
        let anchor = start.or(due).unwrap_or(today);
        Some(Recurrence::Schedule {
            rrule: rule.to_owned(),
            anchor,
            snap,
            snap_leeway,
        })
    } else if let Some(days) = completion_interval(params)? {
        Some(Recurrence::Completion {
            interval_days: days,
            snap,
            snap_leeway,
        })
    } else {
        // No rule to hang a leeway on. `recur_snap` alone has always been
        // dropped here; the leeway says so instead of vanishing.
        anyhow::ensure!(
            snap_leeway.is_none(),
            "--recur-snap-leeway requires a snap; set --recur-snap first \
             (e.g. dom:1, monday, next-workday)"
        );
        None
    };
    // Built directly rather than through `parse_recurrence` — MCP takes the two
    // rule kinds as separate parameters and prefers the schedule — so the shared
    // semantic checks have to be invoked explicitly.
    if let Some(rule) = &recurrence {
        validate_recurrence(rule)?;
    }

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
    ctx.record_task_event("add", task.id);

    tracing::info!(
        cmd = "mcp/add",
        "[{}] {}",
        &task.id.to_string()[..8],
        task.title
    );

    Ok(serde_json::to_value(&task)?)
}

// ── update_task ───────────────────────────────────────────────────────────────

/// Builds the recurrence rule an `update_task` call leaves behind, or `None`
/// when the call does not replace one.
///
/// Loading the stored task is what makes the edit additive: a schedule change
/// keeps the series anchor, and any rule change keeps the snap unless the call
/// sets a new one (`recur_snap`) or drops it (`clear_recur_snap`). Without the
/// carry-forward, `recur_completion: 31` on its own silently wiped the snap.
fn recurrence_edit(
    params: &Value,
    id: uuid::Uuid,
    ctx: &mut TaskRepository,
    today: chrono::NaiveDate,
) -> anyhow::Result<Option<Recurrence>> {
    let snap_edit = SnapEdit::parse(
        str_param(params, "recur_snap"),
        bool_param(params, "clear_recur_snap"),
        str_param(params, "recur_snap_leeway"),
        bool_param(params, "clear_recur_snap_leeway"),
        MCP_SNAP_PARAMS,
    )?;

    let schedule = str_param(params, "recur_schedule");
    let completion = completion_interval(params)?;
    let rule_edit = schedule.is_some() || completion.is_some();
    // `clear_recurrence` drops the rule wholesale through its own flag, so
    // nothing needs building here.
    if bool_param(params, "clear_recurrence") || !(rule_edit || !snap_edit.is_empty()) {
        return Ok(None);
    }

    let existing = ctx.store.get_task(id)?;
    // The rule these parameters describe, snap still empty: `SnapEdit::apply`
    // decides what snap it ends up carrying. Built here rather than through
    // `parse_recurrence` because MCP takes the two rule kinds as separate
    // parameters and prefers the schedule.
    let replacement = if let Some(rule) = schedule {
        validate_rrule(rule)
            .map_err(|e| anyhow::anyhow!("invalid recurrence rule {rule:?}: {e}"))?;
        Some(Recurrence::Schedule {
            rrule: rule.to_owned(),
            anchor: edit_anchor(
                existing.recurrence.as_ref(),
                existing.start.or(existing.due).unwrap_or(today),
            ),
            snap: None,
            snap_leeway: None,
        })
    } else {
        completion.map(|days| Recurrence::Completion {
            interval_days: days,
            snap: None,
            snap_leeway: None,
        })
    };

    Ok(Some(snap_edit.apply(existing.recurrence, replacement)?))
}

pub fn update_task(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let today = Local::now().date_naive();
    let id_str = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: id"))?;

    let id = resolve_task_id(&*ctx.store, id_str)?;

    // Validate the action up front; the transition itself runs *after* the
    // field edits below so that combining the two (e.g. {action:"done",
    // notes:"…"}) no longer silently discards the edits.
    let action = str_param(params, "action");
    match action {
        None | Some("start") | Some("stop") | Some("cancel") | Some("done") | Some("move") => {}
        Some(other) => {
            anyhow::bail!("unknown action {other:?}: expected start, stop, done, cancel, move")
        }
    }

    // ── Field edits (applied first) ─────────────────────────────────────────
    // Runs whenever edit fields are present, and always for a plain edit or a
    // `move` (which carries its change in the `parent` field).
    let mut edited: Option<Task> = None;
    if has_edit_params(params) || matches!(action, None | Some("move")) {
        // Build recurrence update from params.
        let recurrence: Option<Recurrence> = recurrence_edit(params, id, ctx, today)?;

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
            // Map the JSON bool directly so `long_term: false` clears it
            // instead of being ignored.
            long_term: params.get("long_term").and_then(|v| v.as_bool()),
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
        let verb: &'static str = if matches!(action, Some("move")) {
            "move"
        } else {
            "edit"
        };
        ctx.record_task_event(verb, task.id);
        tracing::info!(cmd = %format!("mcp/{verb}"), "[{}] {}", &task.id.to_string()[..8], task.title);
        edited = Some(task);
    }

    // ── State transition (applied after edits) ──────────────────────────────
    let task = match action {
        Some("start") => {
            let task = ctx.transaction(|store, vcs, root| {
                let mut task = store.get_task(id)?;
                task.mark_started();
                let task_path = storage::task_path(root, &task);
                store.save_task(&task)?;
                vcs.commit(&[task_path], &format!("next: start {}", task.title))?;
                Ok(task)
            })?;
            ctx.record_task_event("start", task.id);
            tracing::info!(
                cmd = "mcp/start",
                "[{}] {}",
                &task.id.to_string()[..8],
                task.title
            );
            task
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
            ctx.record_task_event("stop", task.id);
            tracing::info!(
                cmd = "mcp/stop",
                "[{}] {}",
                &task.id.to_string()[..8],
                task.title
            );
            task
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
            ctx.record_task_event("cancel", task.id);
            tracing::info!(
                cmd = "mcp/cancel",
                "[{}] {}",
                &task.id.to_string()[..8],
                task.title
            );
            task
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
            ctx.record_task_event("done", task.id);
            tracing::info!(
                cmd = "mcp/done",
                "[{}] {}",
                &task.id.to_string()[..8],
                task.title
            );
            task
        }
        // "move" and a plain edit already produced the task above.
        _ => edited.ok_or_else(|| {
            anyhow::anyhow!("update_task: nothing to update — provide fields or an action")
        })?,
    };

    Ok(serde_json::to_value(&task)?)
}

// ── delete_task ───────────────────────────────────────────────────────────────

pub fn delete_task(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
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
    ctx.record_task_event("delete", task.id);
    tracing::info!(
        cmd = "mcp/delete",
        "[{}] {}",
        &task.id.to_string()[..8],
        task.title
    );

    Ok(json!({ "deleted": task.id.to_string(), "title": task.title }))
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
    fn add_and_list_task() {
        let (_dir, mut ctx) = make_ctx();
        let params = serde_json::json!({ "title": "Buy milk" });
        let result = add_task(&params, &mut ctx).unwrap();
        assert_eq!(result["title"], "Buy milk");

        let list = list_tasks(&serde_json::json!({}), &mut ctx).unwrap();
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
        assert_eq!(list["total"], 1);
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
        let tasks = all["items"].as_array().unwrap();
        assert_eq!(tasks.len(), 2, "expected original + spawned next instance");
    }

    #[test]
    fn delete_task_removes_it() {
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(&json!({ "title": "To delete" }), &mut ctx).unwrap();
        let id = task["id"].as_str().unwrap();

        delete_task(&json!({ "id": id }), &mut ctx).unwrap();

        let list = list_tasks(&json!({ "include_all": true }), &mut ctx).unwrap();
        assert_eq!(list["items"].as_array().unwrap().len(), 0);
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

    #[test]
    fn update_task_done_applies_field_edits() {
        // {action:"done", notes:"…"} must set the notes AND complete the task,
        // not silently drop the notes.
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(&json!({ "title": "Ship it" }), &mut ctx).unwrap();
        let id = task["id"].as_str().unwrap().to_owned();

        let done = update_task(
            &json!({ "id": id, "action": "done", "notes": "wrapped up" }),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(done["status"], "done");
        assert_eq!(
            done["notes"], "wrapped up",
            "notes edit must not be dropped by the done action"
        );
    }

    #[test]
    fn update_task_start_applies_field_edits() {
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(&json!({ "title": "Task" }), &mut ctx).unwrap();
        let id = task["id"].as_str().unwrap().to_owned();

        let started = update_task(
            &json!({ "id": id, "action": "start", "priority": "high", "add_tags": ["@work"] }),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(started["status"], "started");
        assert_eq!(started["priority"], "high");
        assert!(started["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "@work"));
    }

    #[test]
    fn update_task_stop_applies_field_edits() {
        // {action:"stop", <edits>} must apply the edits AND perform the stop.
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(&json!({ "title": "Task" }), &mut ctx).unwrap();
        let id = task["id"].as_str().unwrap().to_owned();
        update_task(&json!({ "id": id, "action": "start" }), &mut ctx).unwrap();

        let stopped = update_task(
            &json!({ "id": id, "action": "stop", "description": "paused for now" }),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(stopped["status"], "open");
        assert_eq!(
            stopped["description"], "paused for now",
            "description edit must not be dropped by the stop action"
        );
    }

    #[test]
    fn update_task_cancel_applies_field_edits() {
        // {action:"cancel", <edits>} must apply the edits AND perform the cancel.
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(&json!({ "title": "Task" }), &mut ctx).unwrap();
        let id = task["id"].as_str().unwrap().to_owned();

        let cancelled = update_task(
            &json!({ "id": id, "action": "cancel", "notes": "no longer needed" }),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(cancelled["status"], "cancelled");
        assert_eq!(
            cancelled["notes"], "no longer needed",
            "notes edit must not be dropped by the cancel action"
        );
    }

    #[test]
    fn update_task_long_term_false_clears() {
        // Both true and false must take effect; previously only true was mapped.
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(&json!({ "title": "LT" }), &mut ctx).unwrap();
        let id = task["id"].as_str().unwrap().to_owned();

        let on = update_task(&json!({ "id": id, "long_term": true }), &mut ctx).unwrap();
        assert_eq!(on["long_term"], true);

        // `long_term` is omitted from JSON when false (skip_serializing_if), so
        // clearing a previously-true flag shows up as the key going absent.
        let off = update_task(&json!({ "id": id, "long_term": false }), &mut ctx).unwrap();
        assert!(
            off["long_term"].is_null(),
            "long_term:false must clear it, not be ignored: {off}"
        );
    }

    #[test]
    fn update_task_standalone_recur_snap_updates_existing() {
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(
            &json!({ "title": "Recurring", "recur_completion": 7 }),
            &mut ctx,
        )
        .unwrap();
        let id = task["id"].as_str().unwrap().to_owned();
        assert!(task["recurrence"]["snap"].is_null());

        let updated = update_task(&json!({ "id": id, "recur_snap": "monday" }), &mut ctx).unwrap();
        assert!(
            !updated["recurrence"]["snap"].is_null(),
            "standalone recur_snap must set the snap"
        );
        assert_eq!(updated["recurrence"]["type"], "completion");
        assert_eq!(
            updated["recurrence"]["interval_days"], 7,
            "interval must be preserved"
        );
    }

    #[test]
    fn update_task_recur_snap_without_recurrence_errors() {
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(&json!({ "title": "No recurrence" }), &mut ctx).unwrap();
        let id = task["id"].as_str().unwrap();
        let err = update_task(&json!({ "id": id, "recur_snap": "monday" }), &mut ctx).unwrap_err();
        assert!(
            err.to_string()
                .contains("recur_snap requires an existing recurrence"),
            "unexpected error: {err}"
        );
    }

    /// A recurring task with a `dom:1` snap, returning its id.
    fn snapped_task(ctx: &mut TaskRepository) -> String {
        let task = add_task(
            &json!({ "title": "Pay the rent", "recur_completion": 7, "recur_snap": "dom:1" }),
            ctx,
        )
        .unwrap();
        assert_eq!(task["recurrence"]["snap"]["day"], 1);
        task["id"].as_str().unwrap().to_owned()
    }

    #[test]
    fn update_task_recur_completion_keeps_existing_snap() {
        // Changing only the interval must not wipe the snap (UX-2).
        let (_dir, mut ctx) = make_ctx();
        let id = snapped_task(&mut ctx);

        let updated = update_task(&json!({ "id": id, "recur_completion": 31 }), &mut ctx).unwrap();
        assert_eq!(updated["recurrence"]["interval_days"], 31);
        assert_eq!(
            updated["recurrence"]["snap"]["day"], 1,
            "the snap must survive a bare recur_completion: {updated}"
        );
    }

    #[test]
    fn update_task_recur_schedule_keeps_existing_snap() {
        let (_dir, mut ctx) = make_ctx();
        let id = snapped_task(&mut ctx);

        let updated = update_task(
            &json!({ "id": id, "recur_schedule": "FREQ=MONTHLY;BYMONTHDAY=28" }),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(updated["recurrence"]["rrule"], "FREQ=MONTHLY;BYMONTHDAY=28");
        assert_eq!(updated["recurrence"]["snap"]["day"], 1);
    }

    #[test]
    fn update_task_recur_snap_overrides_the_carried_one() {
        let (_dir, mut ctx) = make_ctx();
        let id = snapped_task(&mut ctx);

        let updated = update_task(
            &json!({ "id": id, "recur_completion": 14, "recur_snap": "monday" }),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(updated["recurrence"]["interval_days"], 14);
        assert_eq!(updated["recurrence"]["snap"]["type"], "next_weekday");
    }

    #[test]
    fn update_task_clear_recur_snap_keeps_the_rule() {
        let (_dir, mut ctx) = make_ctx();
        let id = snapped_task(&mut ctx);

        let updated =
            update_task(&json!({ "id": id, "clear_recur_snap": true }), &mut ctx).unwrap();
        assert_eq!(
            updated["recurrence"]["interval_days"], 7,
            "the rule itself must be untouched"
        );
        assert!(
            updated["recurrence"]["snap"].is_null(),
            "clear_recur_snap must drop the snap: {updated}"
        );
    }

    #[test]
    fn update_task_clear_recur_snap_alongside_a_rule_change() {
        let (_dir, mut ctx) = make_ctx();
        let id = snapped_task(&mut ctx);

        let updated = update_task(
            &json!({ "id": id, "recur_completion": 31, "clear_recur_snap": true }),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(updated["recurrence"]["interval_days"], 31);
        assert!(updated["recurrence"]["snap"].is_null());
    }

    #[test]
    fn update_task_recur_snap_and_clear_recur_snap_conflict() {
        let (_dir, mut ctx) = make_ctx();
        let id = snapped_task(&mut ctx);

        let err = update_task(
            &json!({ "id": id, "recur_snap": "monday", "clear_recur_snap": true }),
            &mut ctx,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("recur_snap and clear_recur_snap are mutually exclusive"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn update_task_clear_recur_snap_without_recurrence_errors() {
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(&json!({ "title": "No recurrence" }), &mut ctx).unwrap();
        let id = task["id"].as_str().unwrap();
        let err =
            update_task(&json!({ "id": id, "clear_recur_snap": true }), &mut ctx).unwrap_err();
        assert!(
            err.to_string()
                .contains("clear_recur_snap requires an existing recurrence rule"),
            "unexpected error: {err}"
        );
    }

    // ── snap leeway ─────────────────────────────────────────────────────────

    /// The worked example: 30 days after completion, snapped to the 1st, three
    /// days of tolerance either way.
    fn leeway_task(ctx: &mut TaskRepository) -> String {
        let task = add_task(
            &json!({
                "title": "Pay the rent",
                "recur_completion": 30,
                "recur_snap": "dom:1",
                "recur_snap_leeway": "3",
            }),
            ctx,
        )
        .unwrap();
        assert_eq!(task["recurrence"]["snap_leeway"]["back"], 3);
        assert_eq!(task["recurrence"]["snap_leeway"]["forward"], 3);
        task["id"].as_str().unwrap().to_owned()
    }

    #[test]
    fn add_task_stores_an_asymmetric_leeway() {
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(
            &json!({
                "title": "Pay the rent",
                "recur_completion": 30,
                "recur_snap": "dom:1",
                "recur_snap_leeway": "5,0",
            }),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(task["recurrence"]["snap_leeway"]["back"], 5);
        assert_eq!(task["recurrence"]["snap_leeway"]["forward"], 0);
    }

    /// T13, MCP half: `add_task` builds its rule by hand, so this is what
    /// proves it still reaches the shared checks.
    #[test]
    fn add_task_rejects_a_leeway_with_no_snap_to_qualify() {
        let (_dir, mut ctx) = make_ctx();
        let err = add_task(
            &json!({
                "title": "Pay the rent",
                "recur_completion": 30,
                "recur_snap_leeway": "3",
            }),
            &mut ctx,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "--recur-snap-leeway requires a snap; set --recur-snap first \
             (e.g. dom:1, monday, next-workday)"
        );
    }

    #[test]
    fn add_task_rejects_a_backward_leeway_the_interval_cannot_absorb() {
        let (_dir, mut ctx) = make_ctx();
        let err = add_task(
            &json!({
                "title": "Pay the rent",
                "recur_completion": 3,
                "recur_snap": "dom:1",
                "recur_snap_leeway": "5,0",
            }),
            &mut ctx,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "backward leeway (5d) must be less than the completion interval (3d), \
             or the series would not advance"
        );
    }

    #[test]
    fn add_task_rejects_a_zero_day_interval() {
        // The CLI has always refused this; MCP built the rule directly and let
        // it through, producing a series that never advances.
        let (_dir, mut ctx) = make_ctx();
        let err = add_task(
            &json!({ "title": "Stuck", "recur_completion": 0 }),
            &mut ctx,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "completion interval must be >= 1 day, got 0"
        );
    }

    #[test]
    fn add_task_rejects_an_interval_no_u32_can_hold() {
        // `as u32` used to wrap this into 705032704 without a word.
        let (_dir, mut ctx) = make_ctx();
        let err = add_task(
            &json!({ "title": "Wrapped", "recur_completion": 5000000000u64 }),
            &mut ctx,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "recur_completion must be a whole number of days between 1 and 4294967295, \
             got 5000000000"
        );
    }

    #[test]
    fn update_task_recur_completion_keeps_the_leeway_too() {
        let (_dir, mut ctx) = make_ctx();
        let id = leeway_task(&mut ctx);

        let updated = update_task(&json!({ "id": id, "recur_completion": 31 }), &mut ctx).unwrap();
        assert_eq!(updated["recurrence"]["interval_days"], 31);
        assert_eq!(updated["recurrence"]["snap"]["day"], 1);
        assert_eq!(
            updated["recurrence"]["snap_leeway"]["back"], 3,
            "the leeway must survive a bare recur_completion: {updated}"
        );
    }

    #[test]
    fn update_task_standalone_recur_snap_leeway_replaces_it() {
        let (_dir, mut ctx) = make_ctx();
        let id = leeway_task(&mut ctx);

        let updated =
            update_task(&json!({ "id": id, "recur_snap_leeway": "5,0" }), &mut ctx).unwrap();
        assert_eq!(updated["recurrence"]["interval_days"], 30);
        assert_eq!(updated["recurrence"]["snap"]["day"], 1);
        assert_eq!(updated["recurrence"]["snap_leeway"]["back"], 5);
        assert_eq!(updated["recurrence"]["snap_leeway"]["forward"], 0);
    }

    #[test]
    fn update_task_clear_recur_snap_leeway_keeps_the_snap() {
        let (_dir, mut ctx) = make_ctx();
        let id = leeway_task(&mut ctx);

        let updated = update_task(
            &json!({ "id": id, "clear_recur_snap_leeway": true }),
            &mut ctx,
        )
        .unwrap();
        assert_eq!(updated["recurrence"]["snap"]["day"], 1);
        assert!(
            updated["recurrence"]["snap_leeway"].is_null(),
            "clear_recur_snap_leeway must drop only the leeway: {updated}"
        );
    }

    #[test]
    fn update_task_clear_recur_snap_takes_the_leeway_with_it() {
        let (_dir, mut ctx) = make_ctx();
        let id = leeway_task(&mut ctx);

        let updated =
            update_task(&json!({ "id": id, "clear_recur_snap": true }), &mut ctx).unwrap();
        assert!(updated["recurrence"]["snap"].is_null());
        assert!(
            updated["recurrence"]["snap_leeway"].is_null(),
            "a leeway with no snap is not a rule anyone can act on: {updated}"
        );
    }

    /// T13, MCP half: the carry-forward path. `back = 3` was stored days ago
    /// and never passes through a parser on this call.
    #[test]
    fn update_task_rejects_an_interval_the_carried_leeway_would_stall() {
        let (_dir, mut ctx) = make_ctx();
        let id = leeway_task(&mut ctx);

        let err = update_task(&json!({ "id": id, "recur_completion": 3 }), &mut ctx).unwrap_err();
        assert_eq!(
            err.to_string(),
            "backward leeway (3d) must be less than the completion interval (3d), \
             or the series would not advance"
        );
    }

    #[test]
    fn update_task_recur_snap_leeway_and_clear_conflict() {
        let (_dir, mut ctx) = make_ctx();
        let id = leeway_task(&mut ctx);

        let err = update_task(
            &json!({ "id": id, "recur_snap_leeway": "3", "clear_recur_snap_leeway": true }),
            &mut ctx,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "--recur-snap-leeway and --clear-recur-snap-leeway are mutually exclusive"
        );
    }

    #[test]
    fn update_task_recur_snap_leeway_without_recurrence_errors() {
        let (_dir, mut ctx) = make_ctx();
        let task = add_task(&json!({ "title": "No recurrence" }), &mut ctx).unwrap();
        let id = task["id"].as_str().unwrap();
        let err =
            update_task(&json!({ "id": id, "recur_snap_leeway": "3" }), &mut ctx).unwrap_err();
        assert!(
            err.to_string()
                .contains("recur_snap_leeway requires an existing recurrence rule"),
            "unexpected error: {err}"
        );
    }

    /// A leeway edit that is not registered in `EDIT_PARAM_KEYS` is dropped on
    /// the floor whenever the call carries no other edit field.
    #[test]
    fn the_leeway_params_count_as_edits() {
        assert!(has_edit_params(&json!({ "recur_snap_leeway": "3" })));
        assert!(has_edit_params(&json!({ "clear_recur_snap_leeway": true })));
    }
}
