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
            description: "List tasks scored by urgency. Supports filter tokens (+tag, -tag, parent:slug, context:@name, user:name). Returns { items, page, page_size, total }; total > items.len() means the result is truncated — fetch the next page.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filter_tokens": { "type": "array", "items": { "type": "string" }, "description": "Filter tokens e.g. [\"+@work\", \"-done\", \"parent:infra\"]" },
                    "context": { "type": "array", "items": { "type": "string" }, "description": "Override the included tags for this call (e.g. [\"@work\"]). Pass [] to include nothing, which shows every tag that is not excluded. Exclusions still come from the stored state." },
                    "limit": { "type": "integer", "description": "Legacy alias for page_size" },
                    "page_size": { "type": "integer", "description": "Tasks per page (default 50)" },
                    "page": { "type": "integer", "description": "1-indexed page of results (default 1)" },
                    "include_all": { "type": "boolean", "description": "Disable implicit filters (blocked, future start, done/cancelled)" },
                    "archived": { "type": "boolean", "description": "List archived tasks instead (most recently completed first; tag filters and pagination apply, scoring does not)" }
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
                    "action": { "type": "string", "enum": ["start", "stop", "done", "cancel", "move"], "description": "State transition or move. Omit to only edit fields. Any field edits in the same call are applied first, then the transition — e.g. {action:\"done\", notes:\"…\"} sets the notes and completes the task." },
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
                    "recur_snap": { "type": "string", "description": "Snap next date: next-workday, monday…sunday, dom:N. May be sent alone to change the snap on an existing recurrence." },
                    "clear_recurrence": { "type": "boolean" },
                    "long_term": { "type": "boolean", "description": "Suppress age-based scoring. false clears it (both values take effect)." },
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
            description: "Get the current machine-local state: the per-tag state map (included/excluded/default) and the active users.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "set_tag_state",
            description: "Set the machine-local state of one or more tags. Every tag kind works the same way: `included` limits the list to tasks carrying it, `excluded` hides them, `default` pins the tag to no state so it ignores a parent tag's state, and `clear` removes the entry so it inherits again. Replaces the old set_context and set_resource tools.",
            input_schema: json!({
                "type": "object",
                "required": ["tags", "state"],
                "properties": {
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Tags to change, e.g. [\"@work\", \"#printer\", \"errand\"]" },
                    "state": { "type": "string", "enum": ["included", "excluded", "default", "clear"] },
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
            description: "Manage tags. Actions: list, show, rename, describe, clear_description, set_url, clear_url, set_priority, clear_priority, set_no_time_urgency, clear_no_time_urgency. `rename` moves a tag and everything nested under it across all tasks (active and archived), its metadata file, and machine-local state.",
            input_schema: json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string", "enum": ["list", "show", "rename", "describe", "clear_description", "set_url", "clear_url", "set_priority", "clear_priority", "set_no_time_urgency", "clear_no_time_urgency"] },
                    "tag": { "type": "string" },
                    "new_tag": { "type": "string", "description": "rename only: the new tag name. Must keep the same kind (@context, #resource, or freeform)." },
                    "merge": { "type": "boolean", "default": false, "description": "rename only: allow the destination tag to already exist, folding the old tag into it." },
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

Every task is labelled with tags. A tag's leading character is a naming \
convention that says what the tag is for — it does not change how filtering \
works:
- `@context` — a working environment (e.g. `@work`, `@home`).
- `#resource` — a physical or situational resource (e.g. `#printer`).
- bare `freeform` — a plain label (e.g. `python`, `errand`).

Every tag, whatever its kind, is in one of three states, set with \
`set_tag_state`:
- **included** — while anything is included, only tasks carrying an included \
tag are listed.
- **excluded** — tasks carrying it are hidden. Exclusion beats inclusion.
- **default** — no state. Setting this explicitly also stops the tag \
inheriting a parent tag's state; `clear` instead removes the entry so it \
inherits again.

All kinds nest with `/` (e.g. `@work/frontend`, `#office/printer`): a state on \
a parent applies to every descendant, and a filter on a parent segment matches \
them too. A tag may carry metadata: a default `priority` (high/medium/low) \
added to the urgency of tasks bearing it, and `no_time_urgency` to stop age and \
deadlines from raising that urgency.

When creating tasks, reuse an existing tag from the lists below rather than \
inventing a near-duplicate, and apply the included context unless the user says \
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
        use crate::core::domain::state::TagState;
        let included = state.tags_with(TagState::Included);
        if included.is_empty() {
            out.push_str("- Included tags: none (every tag that is not excluded is shown)\n");
        } else {
            let _ = writeln!(out, "- Included tags: {}", included.join(", "));
        }
        let excluded = state.tags_with(TagState::Excluded);
        if !excluded.is_empty() {
            let _ = writeln!(out, "- Excluded tags: {}", excluded.join(", "));
        }
        let defaulted = state.tags_with(TagState::Default);
        if !defaulted.is_empty() {
            let _ = writeln!(
                out,
                "- Tags pinned to no state (ignoring their parent): {}",
                defaulted.join(", ")
            );
        }
        if !state.active_users.is_empty() {
            let _ = writeln!(
                out,
                "- Active user filter: {}",
                state.active_users.join(", ")
            );
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
    let autosync = params
        .get("autosync")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    match tool_name {
        // ── Read-only tools (no sync) ────────────────────────────────────────
        "list_tasks" => tasks::list_tasks(params, ctx),
        "get_task" => tasks::get_task(params, ctx),
        "get_state" => state::get_state(params, ctx),
        "get_forecast" => view::get_forecast(params, ctx),
        "get_diff" => state::get_diff(params, ctx),

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
        "set_tag_state" => {
            let result = state::set_tag_state(params, ctx)?;
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
fn run_autosync(
    autosync: bool,
    ctx: &mut TaskRepository,
    scheduler: &super::sync_manager::SyncScheduler,
) {
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
        crate::core::test_git::init_test_repo(dir.path());
        let (store, vcs) = crate::core::storage::open(dir.path().to_path_buf()).unwrap();
        let ctx =
            TaskRepository::with_parts(Box::new(store), Box::new(vcs), dir.path().to_path_buf());
        (dir, ctx)
    }

    /// Errors include the full chain so callers can diagnose failures.
    #[test]
    fn sanitize_error_returns_full_chain() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied");
        let app_err = TaskError::Io(io_err);
        let anyhow_err = anyhow::anyhow!(app_err).context("reading tasks/foo.toml".to_string());

        let msg = sanitize_error(&anyhow_err, "add_task");
        assert!(
            msg.contains("permission denied"),
            "expected IO description, got: {msg}"
        );
    }

    /// User-facing errors include their full detail.
    #[test]
    fn sanitize_error_preserves_user_facing_errors() {
        let not_found = anyhow::anyhow!(TaskError::TaskNotFound("abc123".into()));
        let msg = sanitize_error(&not_found, "get_task");
        assert!(
            msg.contains("abc123"),
            "task-not-found should be verbatim: {msg}"
        );
        assert!(
            msg.contains("task not found"),
            "expected 'task not found': {msg}"
        );

        let ambiguous = anyhow::anyhow!(TaskError::AmbiguousId("ab".into(), 3));
        let msg = sanitize_error(&ambiguous, "get_task");
        assert!(
            msg.contains("ambiguous"),
            "expected ambiguous id message: {msg}"
        );

        let slug = anyhow::anyhow!(TaskError::SlugConflict("my-task".into()));
        let msg = sanitize_error(&slug, "add_task");
        assert!(
            msg.contains("my-task"),
            "slug conflict should mention slug: {msg}"
        );
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
        assert!(
            text.contains("@context"),
            "missing context convention: {text}"
        );
        assert!(
            text.contains("#resource"),
            "missing resource convention: {text}"
        );
        assert!(
            text.contains("freeform"),
            "missing freeform convention: {text}"
        );
        // No tags exist yet, so no catalog headings are emitted.
        assert!(
            !text.contains("## Known contexts"),
            "unexpected catalog: {text}"
        );
        assert!(
            text.contains("Included tags: none"),
            "missing tag-state line: {text}"
        );
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
        assert!(
            text.contains("## Known contexts"),
            "missing contexts heading: {text}"
        );
        assert!(text.contains("`@work`"), "missing @work entry: {text}");
        assert!(
            text.contains("Office tasks"),
            "missing @work description: {text}"
        );
        assert!(
            text.contains("[default priority: high]"),
            "missing priority annotation: {text}"
        );
        assert!(
            text.contains("## Known resources"),
            "missing resources heading: {text}"
        );
        assert!(
            text.contains("`#printer`"),
            "missing #printer entry: {text}"
        );
    }

    /// The tag state is reflected in the snapshot, whatever the tag's sigil.
    #[test]
    fn instructions_reflect_tag_state() {
        let (_dir, mut ctx) = make_ctx();
        state::set_tag_state(&json!({ "tags": ["@work"], "state": "included" }), &mut ctx).unwrap();
        state::set_tag_state(
            &json!({ "tags": ["#printer", "errand"], "state": "excluded" }),
            &mut ctx,
        )
        .unwrap();

        let text = server_instructions(&ctx);
        assert!(
            text.contains("Included tags: @work"),
            "missing included tag: {text}"
        );
        assert!(
            text.contains("Excluded tags: #printer, errand"),
            "a resource and a freeform tag are listed the same way: {text}"
        );
    }
}
