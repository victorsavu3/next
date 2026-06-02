pub mod data;
pub mod state;
pub mod tags;
pub mod tasks;
pub mod view;

use serde_json::{json, Value};

use crate::error::AppError;
use crate::AppContext;

use super::protocol::{CallToolResult, Tool};
use super::sync_manager::do_sync;

// ── Tool registry ─────────────────────────────────────────────────────────────

pub fn all_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_tasks",
            description: "List tasks scored by urgency. Supports filter tokens (+tag, -tag, parent:slug, context:@name, user:name).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filter_tokens": { "type": "array", "items": { "type": "string" }, "description": "Filter tokens e.g. [\"+@work\", \"-done\", \"parent:infra\"]" },
                    "context": { "type": "array", "items": { "type": "string" }, "description": "Override active context for this call (e.g. [\"@work\"]). Pass [] to disable context filtering. Overrides state." },
                    "limit": { "type": "integer", "description": "Maximum number of tasks to return" },
                    "include_all": { "type": "boolean", "description": "Disable implicit filters (blocked, future start, done/cancelled)" }
                }
            }),
        },
        Tool {
            name: "get_task",
            description: "Get full details of a single task including its direct children.",
            input_schema: json!({
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id": { "type": "string", "description": "UUID, UUID prefix, or slug" }
                }
            }),
        },
        Tool {
            name: "add_task",
            description: "Create a new task.",
            input_schema: json!({
                "type": "object",
                "required": ["title"],
                "properties": {
                    "title": { "type": "string" },
                    "due": { "type": "string", "description": "Due date: ISO 8601 or natural language e.g. 'next Monday'" },
                    "start": { "type": "string", "description": "Hidden until date: ISO 8601 or natural language" },
                    "priority": { "type": "string", "enum": ["low", "medium", "high"] },
                    "slug": { "type": "string", "description": "Stable human-readable identifier" },
                    "assignee": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Use @ for context, # for resource" },
                    "parent": { "type": "string", "description": "Parent task: UUID, UUID prefix, or slug" },
                    "blocked_by": { "type": "array", "items": { "type": "string" } },
                    "description": { "type": "string" },
                    "url": { "type": "string", "description": "http/https URL associated with this task" },
                    "notes": { "type": "string" },
                    "recur_schedule": { "type": "string", "description": "RFC 5545 RRULE string e.g. 'FREQ=WEEKLY;BYDAY=MO'" },
                    "recur_completion": { "type": "integer", "description": "Completion-based recurrence: repeat N days after done" },
                    "recur_snap": { "type": "string", "description": "Snap next date: next-workday, monday…sunday, dom:N" },
                    "long_term": { "type": "boolean", "description": "Suppress age-based scoring" },
                    "score_adjustment": { "type": "number" },
                    "autosync": { "type": "boolean", "default": true, "description": "Sync immediately after mutation (set false for bulk edits)" }
                }
            }),
        },
        Tool {
            name: "update_task",
            description: "Edit task fields or transition its state (start/stop/done/cancel/move).",
            input_schema: json!({
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id": { "type": "string", "description": "UUID, UUID prefix, or slug" },
                    "action": { "type": "string", "enum": ["start", "stop", "done", "cancel", "move"], "description": "State transition or move. Omit to only edit fields." },
                    "completed_at": { "type": "string", "description": "Completion date for recurrence scheduling (action=done only). ISO 8601 or natural language. Defaults to today." },
                    "title": { "type": "string" },
                    "due": { "type": "string" },
                    "clear_due": { "type": "boolean" },
                    "start": { "type": "string" },
                    "clear_start": { "type": "boolean" },
                    "priority": { "type": "string", "enum": ["low", "medium", "high"] },
                    "slug": { "type": "string" },
                    "assignee": { "type": "string" },
                    "clear_assignee": { "type": "boolean" },
                    "add_tags": { "type": "array", "items": { "type": "string" } },
                    "remove_tags": { "type": "array", "items": { "type": "string" } },
                    "parent": { "type": "string", "description": "New parent (use with action=move, or as plain field edit)" },
                    "clear_parent": { "type": "boolean" },
                    "blocked_by": { "type": "array", "items": { "type": "string" }, "description": "Append blockers" },
                    "clear_blocked_by": { "type": "boolean" },
                    "description": { "type": "string" },
                    "clear_description": { "type": "boolean" },
                    "url": { "type": "string" },
                    "clear_url": { "type": "boolean" },
                    "notes": { "type": "string" },
                    "recur_schedule": { "type": "string" },
                    "recur_completion": { "type": "integer" },
                    "recur_snap": { "type": "string" },
                    "clear_recurrence": { "type": "boolean" },
                    "long_term": { "type": "boolean" },
                    "score_adjustment": { "type": "number" },
                    "autosync": { "type": "boolean", "default": true }
                }
            }),
        },
        Tool {
            name: "delete_task",
            description: "Permanently remove a task.",
            input_schema: json!({
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id": { "type": "string", "description": "UUID, UUID prefix, or slug" },
                    "autosync": { "type": "boolean", "default": true }
                }
            }),
        },
        Tool {
            name: "sync",
            description: "Pull from and push to the remote git repository. Cancels any pending deferred sync timer.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "push_only": { "type": "boolean" },
                    "pull_only": { "type": "boolean" }
                }
            }),
        },
        Tool {
            name: "get_state",
            description: "Get the current machine-local state: active contexts, active users, and resource availability.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "set_context",
            description: "Set the active and/or excluded context filters. Pass an empty array to clear.",
            input_schema: json!({
                "type": "object",
                "required": ["contexts"],
                "properties": {
                    "contexts": { "type": "array", "items": { "type": "string" }, "description": "@-prefixed active context tags. Pass [] to clear." },
                    "excluded_contexts": { "type": "array", "items": { "type": "string" }, "description": "@-prefixed context tags to always hide. Omit to leave unchanged." },
                    "autosync": { "type": "boolean", "default": true }
                }
            }),
        },
        Tool {
            name: "set_resource",
            description: "Toggle availability of a resource tag.",
            input_schema: json!({
                "type": "object",
                "required": ["resource", "available"],
                "properties": {
                    "resource": { "type": "string", "description": "#-prefixed resource name" },
                    "available": { "type": "boolean" },
                    "autosync": { "type": "boolean", "default": true }
                }
            }),
        },
        Tool {
            name: "set_user_filter",
            description: "Set the active user filter. Pass an empty array to show all users.",
            input_schema: json!({
                "type": "object",
                "required": ["users"],
                "properties": {
                    "users": { "type": "array", "items": { "type": "string" } },
                    "autosync": { "type": "boolean", "default": true }
                }
            }),
        },
        Tool {
            name: "manage_tag",
            description: "Manage tag metadata. Actions: list, show, describe, clear_description, set_url, clear_url, set_priority, clear_priority, set_no_time_urgency, clear_no_time_urgency.",
            input_schema: json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string", "enum": ["list", "show", "describe", "clear_description", "set_url", "clear_url", "set_priority", "clear_priority", "set_no_time_urgency", "clear_no_time_urgency"] },
                    "tag": { "type": "string" },
                    "description": { "type": "string" },
                    "url": { "type": "string" },
                    "priority": { "type": "string", "enum": ["low", "medium", "high"] },
                    "autosync": { "type": "boolean", "default": true }
                }
            }),
        },
        Tool {
            name: "manage_task_data",
            description: "Manage arbitrary key-value data on a task. Actions: get, list, set, unset.",
            input_schema: json!({
                "type": "object",
                "required": ["action", "id"],
                "properties": {
                    "action": { "type": "string", "enum": ["get", "list", "set", "unset"] },
                    "id": { "type": "string", "description": "Task UUID, UUID prefix, or slug" },
                    "key": { "type": "string" },
                    "value": { "type": "string", "description": "JSON value or bare string" },
                    "autosync": { "type": "boolean", "default": true }
                }
            }),
        },
        Tool {
            name: "get_forecast",
            description: "List upcoming tasks by due date within a configurable horizon.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "horizon_days": { "type": "integer", "description": "Days to look ahead (default: 90)" },
                    "filter_tokens": { "type": "array", "items": { "type": "string" } },
                    "context": { "type": "array", "items": { "type": "string" }, "description": "Override active context for this call. Overrides state." }
                }
            }),
        },
    ]
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

/// Dispatches a tool call.  Returns `CallToolResult` (never an `Err` — errors
/// are encoded as `is_error: true` content so the MCP caller sees them).
///
/// User-facing errors (task not found, invalid input, etc.) are returned
/// verbatim.  Internal / system errors are logged at ERROR level with full
/// detail and replaced with a short sanitized message before being sent to the
/// client, preventing information disclosure of file paths and git internals.
pub fn dispatch(
    tool_name: &str,
    params: &Value,
    ctx: &mut AppContext,
    scheduler: &super::sync_manager::SyncScheduler,
) -> CallToolResult {
    match call_tool(tool_name, params, ctx, scheduler) {
        Ok(v) => CallToolResult::success(v),
        Err(e) => {
            let client_msg = sanitize_error(&e, tool_name);
            CallToolResult::error(client_msg)
        }
    }
}

/// Classifies an error as user-facing or internal.
///
/// User-facing errors describe a problem with the request (wrong ID, invalid
/// argument, etc.) and are safe to return verbatim.  Internal errors contain
/// system details (file paths, git internals) that must not be disclosed; they
/// are logged with full context and replaced by a short generic message.
fn sanitize_error(err: &anyhow::Error, tool_name: &str) -> String {
    // Attempt to downcast to the structured AppError type.
    if let Some(app_err) = err.downcast_ref::<AppError>() {
        match app_err {
            // User-facing: safe to return verbatim.
            AppError::TaskNotFound(_)
            | AppError::InvalidDate(_, _)
            | AppError::AmbiguousId(_, _)
            | AppError::SlugConflict(_)
            | AppError::GitConflict(_) => return app_err.to_string(),

            // Internal: log and sanitize.
            AppError::Io(_) | AppError::Other(_) => {
                tracing::error!(cmd = %format!("mcp/{tool_name}"), "{err:#}");
                return "storage error".to_owned();
            }
        }
    }

    // Unknown / anyhow-only error chains — treat as internal.
    tracing::error!(cmd = %format!("mcp/{tool_name}"), "{err:#}");
    "internal error".to_owned()
}

fn call_tool(
    tool_name: &str,
    params: &Value,
    ctx: &mut AppContext,
    scheduler: &super::sync_manager::SyncScheduler,
) -> anyhow::Result<Value> {
    let autosync = params.get("autosync").and_then(|v| v.as_bool()).unwrap_or(true);

    match tool_name {
        // ── Read-only tools (no sync) ────────────────────────────────────────
        "list_tasks"   => tasks::list_tasks(params, ctx),
        "get_task"     => tasks::get_task(params, ctx),
        "get_state"    => state::get_state(params, ctx),
        "get_forecast" => view::get_forecast(params, ctx),

        // ── Mutation tools ────────────────────────────────────────────────────
        "add_task" => {
            let result = tasks::add_task(params, ctx)?;
            run_autosync(autosync, ctx, scheduler);
            Ok(result)
        }
        "update_task" => {
            let result = tasks::update_task(params, ctx)?;
            run_autosync(autosync, ctx, scheduler);
            Ok(result)
        }
        "delete_task" => {
            let result = tasks::delete_task(params, ctx)?;
            run_autosync(autosync, ctx, scheduler);
            Ok(result)
        }

        // ── sync tool: acquire semaphore (fail-fast if busy), run, cancel timer ─
        "sync" => {
            let _permit = scheduler
                .try_acquire()
                .ok_or_else(|| anyhow::anyhow!("sync already in progress — try again shortly"))?;
            let result = state::sync(params, ctx)?;
            scheduler.cancel();
            Ok(result)
        }

        // ── State tools ───────────────────────────────────────────────────────
        "set_context" => {
            let result = state::set_context(params, ctx)?;
            run_autosync(autosync, ctx, scheduler);
            Ok(result)
        }
        "set_resource" => {
            let result = state::set_resource(params, ctx)?;
            run_autosync(autosync, ctx, scheduler);
            Ok(result)
        }
        "set_user_filter" => {
            let result = state::set_user_filter(params, ctx)?;
            run_autosync(autosync, ctx, scheduler);
            Ok(result)
        }

        // ── Tag tool (write actions sync, read actions don't) ─────────────────
        "manage_tag" => {
            let (result, is_mutation) = tags::manage_tag(params, ctx)?;
            if is_mutation {
                run_autosync(autosync, ctx, scheduler);
            }
            Ok(result)
        }

        // ── Data tool (write actions sync, read actions don't) ────────────────
        "manage_task_data" => {
            let (result, is_mutation) = data::manage_task_data(params, ctx)?;
            if is_mutation {
                run_autosync(autosync, ctx, scheduler);
            }
            Ok(result)
        }

        other => anyhow::bail!("unknown tool: {other}"),
    }
}

/// Runs sync inline (autosync=true) or schedules a deferred sync (autosync=false).
/// Sync errors are logged but don't fail the tool call.
fn run_autosync(autosync: bool, ctx: &mut AppContext, scheduler: &super::sync_manager::SyncScheduler) {
    if autosync {
        if let Err(e) = do_sync(ctx) {
            tracing::error!(cmd = "mcp/autosync", "{e}");
        }
    } else {
        scheduler.schedule_deferred();
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

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
        let ctx = AppContext {
            config: Config::default(),
            store: Box::new(store),
            vcs: Box::new(vcs),
            repo_root: dir.path().to_path_buf(),
        };
        (dir, ctx)
    }

    /// Internal errors (AppError::Io / AppError::Other with file paths) must be
    /// sanitized — the client message must not contain filesystem paths.
    #[test]
    fn sanitize_error_strips_filesystem_path() {
        let io_err = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permission denied",
        );
        let app_err = AppError::Io(io_err);
        let anyhow_err = anyhow::anyhow!(app_err)
            .context("reading /home/victor/tasks/foo.toml".to_string());

        let msg = sanitize_error(&anyhow_err, "add_task");
        assert_eq!(msg, "storage error", "expected generic storage error, got: {msg}");
        assert!(
            !msg.contains("/home/"),
            "client message must not contain a filesystem path: {msg}",
        );
    }

    /// AppError::Other that embeds an absolute path must not reach the client.
    #[test]
    fn sanitize_error_other_strips_path() {
        let app_err = AppError::Other(
            "path not inside repository: /home/victor/.config/task-manager/config.toml".into(),
        );
        let anyhow_err = anyhow::anyhow!(app_err);

        let msg = sanitize_error(&anyhow_err, "update_task");
        assert_eq!(msg, "storage error");
        assert!(!msg.contains("/home/"), "path must be stripped: {msg}");
    }

    /// User-facing errors (TaskNotFound, InvalidDate, etc.) must still be
    /// returned verbatim so the client can act on them.
    #[test]
    fn sanitize_error_preserves_user_facing_errors() {
        let not_found = anyhow::anyhow!(AppError::TaskNotFound("abc123".into()));
        let msg = sanitize_error(&not_found, "get_task");
        assert!(msg.contains("abc123"), "task-not-found should be verbatim: {msg}");
        assert!(msg.contains("task not found"), "expected 'task not found': {msg}");

        let ambiguous = anyhow::anyhow!(AppError::AmbiguousId("ab".into(), 3));
        let msg = sanitize_error(&ambiguous, "get_task");
        assert!(msg.contains("ambiguous"), "expected ambiguous id message: {msg}");

        let slug = anyhow::anyhow!(AppError::SlugConflict("my-task".into()));
        let msg = sanitize_error(&slug, "add_task");
        assert!(msg.contains("my-task"), "slug conflict should mention slug: {msg}");
    }

    /// dispatch() must not expose filesystem paths through the CallToolResult.
    #[test]
    fn dispatch_internal_error_no_path_disclosure() {
        let (_dir, mut ctx) = make_ctx();
        let scheduler = crate::mcp::sync_manager::SyncScheduler::new_for_test();

        // Corrupt the backing store path to force an IO error on the next write.
        // We achieve this by replacing the tasks directory with a file, then
        // attempting to add a task (which needs to write into that directory).
        let tasks_dir = ctx.repo_root.join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        // Remove the dir and put a file in its place so writes fail.
        std::fs::remove_dir(&tasks_dir).unwrap();
        std::fs::write(&tasks_dir, b"not a directory").unwrap();

        let params = serde_json::json!({ "title": "Should fail", "autosync": false });
        let result = dispatch("add_task", &params, &mut ctx, &scheduler);

        assert_eq!(result.is_error, Some(true), "expected an error result");
        let text = &result.content[0].text;
        assert!(
            !text.contains("/home/"),
            "response must not contain home path, got: {text}",
        );
        assert!(
            !text.contains(ctx.repo_root.to_str().unwrap()),
            "response must not contain repo root path, got: {text}",
        );
    }
}
