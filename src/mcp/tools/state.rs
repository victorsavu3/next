use serde_json::{json, Value};

use crate::TaskRepository;

fn strings_param(params: &Value, key: &str) -> Vec<String> {
    params
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

// ── sync ──────────────────────────────────────────────────────────────────────

pub fn sync(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let push_only = params.get("push_only").and_then(|v| v.as_bool()).unwrap_or(false);
    let pull_only = params.get("pull_only").and_then(|v| v.as_bool()).unwrap_or(false);

    if !push_only {
        match ctx.vcs.pull()? {
            crate::core::store::PullResult::Clean => {
                tracing::info!(cmd = "mcp/sync", "pull: clean");
            }
            crate::core::store::PullResult::Conflicts(paths) => {
                let names: Vec<_> = paths.iter().map(|p| p.display().to_string()).collect();
                let msg = format!("merge conflicts: {}", names.join(", "));
                tracing::error!(cmd = "mcp/sync", "{msg}");
                anyhow::bail!("{msg}");
            }
        }
    }
    if !pull_only {
        ctx.vcs.push()?;
        tracing::info!(cmd = "mcp/sync", "push: ok");
    }

    Ok(json!({ "status": "ok" }))
}

// ── get_state ────────────────────────────────────────────────────────────────

pub fn get_state(_params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let state = ctx.store.get_state()?;
    Ok(serde_json::to_value(&state)?)
}

// ── set_context ───────────────────────────────────────────────────────────────

pub fn set_context(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let contexts = strings_param(params, "contexts");
    for c in &contexts {
        if !c.starts_with('@') {
            anyhow::bail!("context tags must start with '@', got: {c}");
        }
    }
    // Optional: also replace excluded_contexts if provided.
    let excluded = if params.get("excluded_contexts").is_some() {
        let excluded = strings_param(params, "excluded_contexts");
        for c in &excluded {
            if !c.starts_with('@') {
                anyhow::bail!("excluded context tags must start with '@', got: {c}");
            }
        }
        Some(excluded)
    } else {
        None
    };

    let state = ctx.state_transaction(|store| {
        let mut state = store.get_state()?;
        state.active_contexts = contexts;
        if let Some(excluded) = excluded {
            state.excluded_contexts = excluded;
        }
        store.save_state(&state)?;
        Ok(state)
    })?;
    tracing::info!(cmd = "mcp/context", "active={:?} excluded={:?}", state.active_contexts, state.excluded_contexts);
    Ok(serde_json::to_value(&state)?)
}

// ── set_resource ──────────────────────────────────────────────────────────────

pub fn set_resource(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let resource = params
        .get("resource")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: resource"))?;
    let available = params
        .get("available")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: available"))?;

    if !resource.starts_with('#') {
        anyhow::bail!("resource names must start with '#', got: {resource}");
    }

    let bare = crate::core::domain::tag::bare_name(resource).to_owned();
    let state = ctx.state_transaction(|store| {
        let mut state = store.get_state()?;
        // Store the bare name (without `#`), matching the CLI and the key
        // convention that `GlobalState::is_resource_available` looks up. Storing
        // the prefixed form here meant MCP-set resources never actually filtered.
        state.resources.insert(bare, available);
        store.save_state(&state)?;
        Ok(state)
    })?;
    tracing::info!(cmd = "mcp/resource", "set {resource}={available}");
    Ok(serde_json::to_value(&state)?)
}

// ── set_user_filter ───────────────────────────────────────────────────────────

pub fn set_user_filter(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let users = strings_param(params, "users");
    let state = ctx.state_transaction(|store| {
        let mut state = store.get_state()?;
        state.active_users = users;
        store.save_state(&state)?;
        Ok(state)
    })?;
    tracing::info!(cmd = "mcp/user", "set {:?}", state.active_users);
    Ok(serde_json::to_value(&state)?)
}

// ── get_diff ──────────────────────────────────────────────────────────────────

pub fn get_diff(_params: &Value, ctx: &TaskRepository) -> anyhow::Result<Value> {
    let diff = ctx.vcs.diff()?;
    Ok(json!({ "diff": diff }))
}

// ── force_sync ────────────────────────────────────────────────────────────────

/// Fetches from the default remote and hard-resets to `FETCH_HEAD`, discarding
/// local changes and resolving any conflicts. Updates the store cache to match
/// the new HEAD. Does NOT push — the remote is the source of truth here.
pub fn force_sync(_params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let new_head = ctx.vcs.force_pull()?;
    ctx.store.after_pull(&new_head)?;
    tracing::info!(cmd = "mcp/force_sync", head = %new_head, "reset to remote");
    Ok(json!({ "status": "ok", "head": new_head }))
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
    fn set_and_get_context() {
        let (_dir, mut ctx) = make_ctx();
        set_context(&json!({ "contexts": ["@work"] }), &mut ctx).unwrap();
        let state = get_state(&json!({}), &mut ctx).unwrap();
        assert_eq!(state["active_contexts"][0], "@work");
    }

    #[test]
    fn set_context_requires_at_prefix() {
        let (_dir, mut ctx) = make_ctx();
        let err = set_context(&json!({ "contexts": ["work"] }), &mut ctx).unwrap_err();
        assert!(err.to_string().contains("'@'"));
    }

    #[test]
    fn set_resource() {
        let (_dir, mut ctx) = make_ctx();
        super::set_resource(&json!({ "resource": "#printer", "available": false }), &mut ctx).unwrap();
        let state = get_state(&json!({}), &mut ctx).unwrap();
        // Key is stored bare (without `#`) so it matches is_resource_available's lookup.
        assert_eq!(state["resources"]["printer"], false);
        assert!(state["resources"].get("#printer").is_none(), "key must not carry a '#' prefix");
    }

    /// Regression: a resource marked unavailable via MCP must actually be
    /// reported unavailable by `GlobalState::is_resource_available`, which keys
    /// on the bare name.
    #[test]
    fn set_resource_actually_filters() {
        let (_dir, mut ctx) = make_ctx();
        super::set_resource(&json!({ "resource": "#printer", "available": false }), &mut ctx).unwrap();
        let state = ctx.store.get_state().unwrap();
        assert!(!state.is_resource_available("#printer"));
    }

    #[test]
    fn set_resource_requires_hash_prefix() {
        let (_dir, mut ctx) = make_ctx();
        let err = super::set_resource(&json!({ "resource": "printer", "available": true }), &mut ctx).unwrap_err();
        assert!(err.to_string().contains("'#'"));
    }

    #[test]
    fn set_user_filter_and_clear() {
        let (_dir, mut ctx) = make_ctx();
        set_user_filter(&json!({ "users": ["alice"] }), &mut ctx).unwrap();
        let state = get_state(&json!({}), &mut ctx).unwrap();
        assert_eq!(state["active_users"][0], "alice");

        set_user_filter(&json!({ "users": [] }), &mut ctx).unwrap();
        let state = get_state(&json!({}), &mut ctx).unwrap();
        assert_eq!(state["active_users"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn sync_no_remote_returns_error() {
        let (_dir, mut ctx) = make_ctx();
        // Repo has no remote — sync should error.
        let err = sync(&json!({}), &mut ctx).unwrap_err();
        assert!(!err.to_string().is_empty());
    }
}
