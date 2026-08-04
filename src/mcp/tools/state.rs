use serde_json::{json, Value};

use crate::TaskRepository;

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

// ── sync ──────────────────────────────────────────────────────────────────────

pub fn sync(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let push_only = params
        .get("push_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let pull_only = params
        .get("pull_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

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

// ── set_tag_state ────────────────────────────────────────────────────────────

/// Replaces `set_context` and `set_resource`: every tag takes the same three
/// states, so one tool covers what used to be two with different shapes.
pub fn set_tag_state(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    use crate::core::domain::state::TagState;

    let state_name = params
        .get("state")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: state"))?;
    let wanted = match state_name {
        "included" => Some(TagState::Included),
        "excluded" => Some(TagState::Excluded),
        "default" => Some(TagState::Default),
        // Distinct from "default": this removes the entry, so the tag inherits
        // from its parent again instead of being pinned to no state.
        "clear" => None,
        other => {
            anyhow::bail!("unknown state {other:?} — expected included, excluded, default or clear")
        }
    };

    let tags = strings_param(params, "tags");
    if tags.is_empty() {
        anyhow::bail!("missing required parameter: tags");
    }
    for t in &tags {
        crate::core::domain::tag::validate_tag(t).map_err(|e| anyhow::anyhow!("{e}"))?;
    }

    let state = ctx.state_transaction(|store| {
        let mut global = store.get_state()?;
        for t in &tags {
            global.set_state(t, wanted);
        }
        store.save_state(&global)?;
        Ok(global)
    })?;
    tracing::info!(cmd = "mcp/tag_state", "{state_name} {}", tags.join(" "));
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
        crate::core::test_git::init_test_repo(dir.path());
        let (store, vcs) = crate::core::storage::open(dir.path().to_path_buf()).unwrap();
        let ctx =
            TaskRepository::with_parts(Box::new(store), Box::new(vcs), dir.path().to_path_buf());
        (dir, ctx)
    }

    #[test]
    fn set_and_get_tag_state() {
        let (_dir, mut ctx) = make_ctx();
        set_tag_state(&json!({ "tags": ["@work"], "state": "included" }), &mut ctx).unwrap();
        let state = get_state(&json!({}), &mut ctx).unwrap();
        assert_eq!(state["tags"]["@work"], "included");
    }

    #[test]
    fn every_tag_kind_takes_every_state() {
        // The point of unification: a resource is not exclude-only and a
        // freeform tag is not a second-class citizen.
        let (_dir, mut ctx) = make_ctx();
        for (tag, want) in [
            ("#printer", "included"),
            ("errand", "excluded"),
            ("@home/kitchen", "default"),
        ] {
            set_tag_state(&json!({ "tags": [tag], "state": want }), &mut ctx).unwrap();
            let state = get_state(&json!({}), &mut ctx).unwrap();
            assert_eq!(state["tags"][tag], want, "{tag}");
        }
    }

    #[test]
    fn clear_removes_the_entry_rather_than_pinning_it() {
        let (_dir, mut ctx) = make_ctx();
        set_tag_state(
            &json!({ "tags": ["#printer"], "state": "excluded" }),
            &mut ctx,
        )
        .unwrap();
        set_tag_state(&json!({ "tags": ["#printer"], "state": "clear" }), &mut ctx).unwrap();
        let state = ctx.store.get_state().unwrap();
        assert!(state.tags.is_empty(), "clear drops the key entirely");
    }

    #[test]
    fn excluded_tag_actually_filters() {
        let (_dir, mut ctx) = make_ctx();
        set_tag_state(
            &json!({ "tags": ["#printer"], "state": "excluded" }),
            &mut ctx,
        )
        .unwrap();
        let state = ctx.store.get_state().unwrap();
        assert!(!state.admits(&["#printer".to_owned()]));
        assert!(
            !state.admits(&["#printer/color".to_owned()]),
            "descendants too"
        );
    }

    #[test]
    fn unknown_state_and_missing_tags_are_rejected() {
        let (_dir, mut ctx) = make_ctx();
        let err = set_tag_state(&json!({ "tags": ["@work"], "state": "sideways" }), &mut ctx)
            .unwrap_err();
        assert!(err.to_string().contains("unknown state"), "{err}");

        let err = set_tag_state(&json!({ "state": "included" }), &mut ctx).unwrap_err();
        assert!(err.to_string().contains("tags"), "{err}");

        let err =
            set_tag_state(&json!({ "tags": ["1bad"], "state": "included" }), &mut ctx).unwrap_err();
        assert!(
            err.to_string().contains("must start with a letter"),
            "{err}"
        );
    }

    #[test]
    fn set_user_filter_and_clear() {
        let (_dir, mut ctx) = make_ctx();
        set_user_filter(&json!({ "users": ["alice"] }), &mut ctx).unwrap();
        let state = get_state(&json!({}), &mut ctx).unwrap();
        assert_eq!(state["active_users"][0], "alice");

        // An empty filter is omitted rather than serialised as `[]`: the state
        // file stays free of entries that mean "nothing set".
        set_user_filter(&json!({ "users": [] }), &mut ctx).unwrap();
        let state = get_state(&json!({}), &mut ctx).unwrap();
        assert!(state.get("active_users").is_none());
    }

    #[test]
    fn sync_no_remote_returns_error() {
        let (_dir, mut ctx) = make_ctx();
        // Repo has no remote — sync should error.
        let err = sync(&json!({}), &mut ctx).unwrap_err();
        assert!(!err.to_string().is_empty());
    }
}
