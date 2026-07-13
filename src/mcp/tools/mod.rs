pub mod data;
pub mod state;
pub mod tags;
pub mod tasks;
pub mod view;

use std::fmt::Write as _;

use serde_json::{json, Value};

use crate::core::domain::tag::TagMeta;
use crate::TaskRepository;

use self::tags::{CatalogEntry, TagCatalog};
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
            name: "get_diff",
            description: "Return the current working-tree diff (git status + git diff HEAD). Useful for inspecting conflicts or uncommitted local changes before deciding how to resolve them.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "force_sync",
            description: "Fetch from the remote and hard-reset the working tree to FETCH_HEAD, discarding all local changes and merge conflicts. Use this to recover when sync is stuck due to conflicts. Does not push — the remote is taken as the source of truth.",
            input_schema: json!({ "type": "object", "properties": {} }),
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
            description: "List upcoming occurrences within a configurable horizon: concrete tasks due in the window plus projected (not-yet-spawned) future instances of active schedule-type recurrence series. Each entry carries a `projected` flag (true for forecast-only occurrences).",
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

// ── Server instructions (initialize) ───────────────────────────────────────────

/// The static tagging guide — conventions that never change. Always included so
/// the model understands the tag system even on an empty repository.
const TAGGING_GUIDE: &str = "\
# next task manager

Every task is labelled with tags. A tag's leading character classifies it:
- `@context` — a working environment (e.g. `@work`, `@home`). Tasks carrying an \
`@` tag are hidden unless that context is active or no context is active; tasks \
with no `@` tag are always shown.
- `#resource` — a physical or situational resource (e.g. `#printer`). Tasks are \
hidden while the resource is marked unavailable.
- bare `freeform` — a plain label (e.g. `python`, `errand`), no filtering effect.

All three kinds nest with `/` (e.g. `@work/frontend`, `#office/printer`); a \
filter on a parent segment matches every descendant. A tag may carry metadata: a \
default `priority` (high/medium/low) added to the urgency of tasks bearing it, \
and `no_time_urgency` to stop age and deadlines from raising that urgency.

When creating tasks, reuse an existing tag from the lists below rather than \
inventing a near-duplicate, and apply the active context unless the user says \
otherwise.";

/// Builds the `instructions` string returned in the MCP `initialize` result.
///
/// Combines the static [`TAGGING_GUIDE`] with a live, connect-time snapshot of
/// the active context/resource state and the known-tag catalog, so a client can
/// inject the whole thing into the model's system prompt. Storage reads that
/// fail are skipped rather than propagated — the guide is always returned.
pub fn server_instructions(ctx: &TaskRepository) -> String {
    let mut out = String::from(TAGGING_GUIDE);

    if let Ok(state) = ctx.store.get_state() {
        out.push_str("\n\n## Current state\n");
        if state.active_contexts.is_empty() {
            out.push_str("- Active context: none (tasks from all contexts are shown)\n");
        } else {
            let _ = writeln!(out, "- Active context: {}", state.active_contexts.join(", "));
        }
        if !state.excluded_contexts.is_empty() {
            let _ = writeln!(out, "- Excluded contexts: {}", state.excluded_contexts.join(", "));
        }
        let mut unavailable: Vec<&String> = state
            .resources
            .iter()
            .filter(|(_, available)| !**available)
            .map(|(name, _)| name)
            .collect();
        unavailable.sort();
        if !unavailable.is_empty() {
            // Normalise to a single `#` prefix regardless of how the key was stored.
            let names: Vec<String> = unavailable
                .iter()
                .map(|n| format!("#{}", crate::core::domain::tag::bare_name(n)))
                .collect();
            let _ = writeln!(out, "- Unavailable resources: {}", names.join(", "));
        }
        if !state.active_users.is_empty() {
            let _ = writeln!(out, "- Active user filter: {}", state.active_users.join(", "));
        }
    }

    if let Ok(catalog) = tags::tag_catalog(ctx) {
        render_catalog_section(&mut out, &catalog);
    }

    out
}

/// Renders the known-tag catalog as markdown lists, omitting empty groups.
fn render_catalog_section(out: &mut String, catalog: &TagCatalog) {
    let groups: [(&str, &[CatalogEntry]); 3] = [
        ("Known contexts", &catalog.contexts),
        ("Known resources", &catalog.resources),
        ("Known freeform tags", &catalog.freeform),
    ];
    for (heading, entries) in groups {
        if entries.is_empty() {
            continue;
        }
        let _ = write!(out, "\n\n## {heading}\n");
        for entry in entries {
            let _ = write!(out, "- `{}`", entry.tag);
            let annotation = annotate_meta(&entry.meta);
            if !annotation.is_empty() {
                let _ = write!(out, " — {annotation}");
            }
            out.push('\n');
        }
    }
}

/// Formats a tag's metadata into a single-line annotation, or "" if it carries
/// no description, priority, or no-time-urgency flag.
fn annotate_meta(meta: &TagMeta) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(desc) = &meta.description {
        parts.push(desc.clone());
    }
    if let Some(priority) = &meta.priority {
        parts.push(format!("[default priority: {priority}]"));
    }
    if meta.no_time_urgency {
        parts.push("[no time urgency]".to_string());
    }
    parts.join(" ")
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
    ctx: &mut TaskRepository,
    scheduler: &super::sync_manager::SyncScheduler,
) -> CallToolResult {
    let result = match call_tool(tool_name, params, ctx, scheduler) {
        Ok(v) => CallToolResult::success(v),
        Err(e) => {
            let client_msg = sanitize_error(&e, tool_name);
            CallToolResult::error(client_msg)
        }
    };

    // Notify subscribed plugins of any task changes recorded by the tool. The
    // repo lock was released when each tool's transaction returned (and autosync
    // already ran inside call_tool), so spawning here is safe. Fire-and-forget;
    // handlers only record after a successful mutation, so a failed call leaves
    // the buffer empty.
    let events = ctx.take_task_events();
    if !events.is_empty() {
        let repo_root = ctx.repo_root.clone();
        crate::core::plugin::notify(&repo_root, &events, ctx.plugin_origin());
        for ev in &events {
            if ev.verb == "delete" {
                let _ = crate::core::plugin::registry::prune_task(&repo_root, ev.task_id);
            }
        }
    }

    result
}

/// Returns the full error chain as a string for the MCP client.
///
/// The full chain (`{err:#}`) is both logged server-side and returned to the
/// client so that callers can diagnose failures without needing server log access.
fn sanitize_error(err: &anyhow::Error, tool_name: &str) -> String {
    let full = format!("{err:#}");
    tracing::error!(cmd = %format!("mcp/{tool_name}"), "{full}");
    full
}

fn call_tool(
    tool_name: &str,
    params: &Value,
    ctx: &mut TaskRepository,
    scheduler: &super::sync_manager::SyncScheduler,
) -> anyhow::Result<Value> {
    let autosync = params.get("autosync").and_then(|v| v.as_bool()).unwrap_or(true);

    match tool_name {
        // ── Read-only tools (no sync) ────────────────────────────────────────
        "list_tasks"   => tasks::list_tasks(params, ctx),
        "get_task"     => tasks::get_task(params, ctx),
        "get_state"    => state::get_state(params, ctx),
        "get_forecast" => view::get_forecast(params, ctx),
        "get_diff"     => state::get_diff(params, ctx),

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

        "force_sync" => {
            let _permit = scheduler
                .try_acquire()
                .ok_or_else(|| anyhow::anyhow!("sync already in progress — try again shortly"))?;
            let result = state::force_sync(params, ctx)?;
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
fn run_autosync(autosync: bool, ctx: &mut TaskRepository, scheduler: &super::sync_manager::SyncScheduler) {
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
    use crate::core::error::TaskError;
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

    /// Errors include the full chain so callers can diagnose failures.
    #[test]
    fn sanitize_error_returns_full_chain() {
        let io_err = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permission denied",
        );
        let app_err = TaskError::Io(io_err);
        let anyhow_err = anyhow::anyhow!(app_err)
            .context("reading tasks/foo.toml".to_string());

        let msg = sanitize_error(&anyhow_err, "add_task");
        assert!(msg.contains("permission denied"), "expected IO description, got: {msg}");
    }

    /// User-facing errors include their full detail.
    #[test]
    fn sanitize_error_preserves_user_facing_errors() {
        let not_found = anyhow::anyhow!(TaskError::TaskNotFound("abc123".into()));
        let msg = sanitize_error(&not_found, "get_task");
        assert!(msg.contains("abc123"), "task-not-found should be verbatim: {msg}");
        assert!(msg.contains("task not found"), "expected 'task not found': {msg}");

        let ambiguous = anyhow::anyhow!(TaskError::AmbiguousId("ab".into(), 3));
        let msg = sanitize_error(&ambiguous, "get_task");
        assert!(msg.contains("ambiguous"), "expected ambiguous id message: {msg}");

        let slug = anyhow::anyhow!(TaskError::SlugConflict("my-task".into()));
        let msg = sanitize_error(&slug, "add_task");
        assert!(msg.contains("my-task"), "slug conflict should mention slug: {msg}");
    }

    /// dispatch() surfaces a descriptive error result (is_error=true) when a
    /// write fails.
    #[test]
    fn dispatch_internal_error_returns_error_result() {
        let (_dir, mut ctx) = make_ctx();
        let scheduler = crate::mcp::sync_manager::SyncScheduler::new_for_test();

        // Replace the tasks directory with a file so writes fail.
        let tasks_dir = ctx.repo_root.join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        std::fs::remove_dir(&tasks_dir).unwrap();
        std::fs::write(&tasks_dir, b"not a directory").unwrap();

        let params = serde_json::json!({ "title": "Should fail", "autosync": false });
        let result = dispatch("add_task", &params, &mut ctx, &scheduler);

        assert_eq!(result.is_error, Some(true), "expected an error result");
        let text = &result.content[0].text;
        assert!(!text.is_empty(), "error message must not be empty");
    }

    // ── server_instructions ─────────────────────────────────────────────────

    /// On an empty repo the instructions still carry the static tagging guide
    /// and report that no context is active.
    #[test]
    fn instructions_include_conventions_on_empty_repo() {
        let (_dir, ctx) = make_ctx();
        let text = server_instructions(&ctx);
        assert!(!text.is_empty());
        // Conventions are always present.
        assert!(text.contains("@context"), "missing context convention: {text}");
        assert!(text.contains("#resource"), "missing resource convention: {text}");
        assert!(text.contains("freeform"), "missing freeform convention: {text}");
        // No tags exist yet, so no catalog headings are emitted.
        assert!(!text.contains("## Known contexts"), "unexpected catalog: {text}");
        assert!(text.contains("Active context: none"), "missing active-context line: {text}");
    }

    /// Known tags and their metadata appear in the snapshot, grouped by kind.
    #[test]
    fn instructions_include_known_tags_with_metadata() {
        let (_dir, mut ctx) = make_ctx();
        // A described, high-priority context and a described resource.
        tags::manage_tag(
            &json!({ "action": "describe", "tag": "@work", "description": "Office tasks" }),
            &mut ctx,
        )
        .unwrap();
        tags::manage_tag(
            &json!({ "action": "set_priority", "tag": "@work", "priority": "high" }),
            &mut ctx,
        )
        .unwrap();
        tags::manage_tag(
            &json!({ "action": "describe", "tag": "#printer", "description": "2nd-floor laser" }),
            &mut ctx,
        )
        .unwrap();

        let text = server_instructions(&ctx);
        assert!(text.contains("## Known contexts"), "missing contexts heading: {text}");
        assert!(text.contains("`@work`"), "missing @work entry: {text}");
        assert!(text.contains("Office tasks"), "missing @work description: {text}");
        assert!(
            text.contains("[default priority: high]"),
            "missing priority annotation: {text}"
        );
        assert!(text.contains("## Known resources"), "missing resources heading: {text}");
        assert!(text.contains("`#printer`"), "missing #printer entry: {text}");
    }

    /// Active context and unavailable resources are reflected in the snapshot.
    #[test]
    fn instructions_reflect_active_state() {
        let (_dir, mut ctx) = make_ctx();
        state::set_context(&json!({ "contexts": ["@work"] }), &mut ctx).unwrap();
        state::set_resource(&json!({ "resource": "#printer", "available": false }), &mut ctx)
            .unwrap();

        let text = server_instructions(&ctx);
        assert!(text.contains("Active context: @work"), "missing active context: {text}");
        assert!(
            text.contains("Unavailable resources: #printer"),
            "missing unavailable resource: {text}"
        );
    }
}
