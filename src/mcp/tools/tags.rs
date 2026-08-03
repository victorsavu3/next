use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::core::domain::tag::{self, TagKind, TagMeta};
use crate::core::domain::task::Priority;
use crate::core::storage;
use crate::TaskRepository;

/// Unified tag metadata tool.
///
/// `action` values:
///   read:  "list", "show"
///   write: "rename", "describe", "clear_description", "set_url", "clear_url",
///          "set_priority", "clear_priority",
///          "set_no_time_urgency", "clear_no_time_urgency"
pub fn manage_tag(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<(Value, bool)> {
    let action = params
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: action"))?;

    match action {
        "list" => Ok((list(ctx)?, false)),
        "show" => Ok((show(params, ctx)?, false)),
        "rename" => Ok((rename(params, ctx)?, true)),
        "describe" => Ok((describe(params, ctx)?, true)),
        "clear_description" => Ok((clear_description(params, ctx)?, true)),
        "set_url" => Ok((set_url(params, ctx)?, true)),
        "clear_url" => Ok((clear_url(params, ctx)?, true)),
        "set_priority" => Ok((set_priority(params, ctx)?, true)),
        "clear_priority" => Ok((clear_priority(params, ctx)?, true)),
        "set_no_time_urgency" => Ok((set_no_time_urgency(params, ctx)?, true)),
        "clear_no_time_urgency" => Ok((clear_no_time_urgency(params, ctx)?, true)),
        other => anyhow::bail!("unknown action {other:?}"),
    }
}

fn require_tag(params: &Value) -> anyhow::Result<&str> {
    params
        .get("tag")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: tag"))
}

/// A single tag plus its metadata, as surfaced in the catalog.
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub tag: String,
    pub meta: TagMeta,
}

/// The full set of known tags, grouped by kind and sorted within each group.
///
/// "Known" means any tag that either carries metadata (`tags/<tag>.toml`) or is
/// in use on at least one task. Shared by the `manage_tag list` action and by
/// the server `instructions` snapshot so both report the same view.
#[derive(Debug, Clone, Default)]
pub struct TagCatalog {
    pub contexts: Vec<CatalogEntry>,
    pub resources: Vec<CatalogEntry>,
    pub freeform: Vec<CatalogEntry>,
}

/// Builds the tag catalog from tag metadata and the tags in use across tasks.
pub fn tag_catalog(ctx: &TaskRepository) -> anyhow::Result<TagCatalog> {
    let metas = ctx.store.list_tag_metas()?;
    let tasks = ctx.store.list_tasks()?;

    let mut all_tags: BTreeSet<String> = BTreeSet::new();
    for task in &tasks {
        for t in &task.tags {
            all_tags.insert(t.clone());
        }
    }
    for t in metas.keys() {
        all_tags.insert(t.clone());
    }

    let mut catalog = TagCatalog::default();
    for t in &all_tags {
        let meta = metas.get(t.as_str()).cloned().unwrap_or_default();
        let entry = CatalogEntry {
            tag: t.clone(),
            meta,
        };
        match tag::classify(t) {
            TagKind::Context => catalog.contexts.push(entry),
            TagKind::Resource => catalog.resources.push(entry),
            TagKind::Freeform => catalog.freeform.push(entry),
        }
    }
    Ok(catalog)
}

fn list(ctx: &TaskRepository) -> anyhow::Result<Value> {
    let catalog = tag_catalog(ctx)?;
    let to_json = |entries: &[CatalogEntry]| -> Vec<Value> {
        entries
            .iter()
            .map(|e| json!({ "tag": e.tag, "meta": e.meta }))
            .collect()
    };
    Ok(json!({
        "contexts": to_json(&catalog.contexts),
        "resources": to_json(&catalog.resources),
        "freeform": to_json(&catalog.freeform),
    }))
}

fn show(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let t = require_tag(params)?;
    tag::validate_tag(t).map_err(|e| anyhow::anyhow!("{e}"))?;
    let meta = ctx.store.get_tag_meta(t)?.unwrap_or_default();
    Ok(json!({ "tag": t, "meta": meta }))
}

fn rename(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let old = require_tag(params)?;
    let new = params
        .get("new_tag")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: new_tag"))?;
    let merge = params
        .get("merge")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let outcome = crate::core::tag_rename::rename_tag(ctx, old, new, merge)?;
    Ok(json!({
        "tag": old,
        "new_tag": new,
        "active_tasks": outcome.active_tasks,
        "archived_tasks": outcome.archived_tasks,
        "segments": outcome.segments,
        "restored_segments": outcome.restored_segments,
        "metas_moved": outcome
            .metas_moved
            .iter()
            .map(|(from, to)| json!({ "from": from, "to": to }))
            .collect::<Vec<_>>(),
        "metas_dropped": outcome.metas_dropped,
        "state_fields": outcome.state_fields,
    }))
}

fn describe(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let t = require_tag(params)?;
    let description = params
        .get("description")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: description"))?;
    tag::validate_tag(t).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.transaction(|store, vcs, root| {
        store.set_tag_description(t, description)?;
        let tag_path = storage::tag_meta_path(root, t);
        vcs.commit(&[tag_path], &format!("next: tag describe {t}"))?;
        Ok(())
    })?;
    Ok(json!({ "tag": t, "description": description }))
}

fn clear_description(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let t = require_tag(params)?;
    tag::validate_tag(t).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.transaction(|store, vcs, root| {
        store.delete_tag_description(t)?;
        let tag_path = storage::tag_meta_path(root, t);
        vcs.commit(&[tag_path], &format!("next: tag clear-description {t}"))?;
        Ok(())
    })?;
    Ok(json!({ "tag": t, "cleared": "description" }))
}

fn set_url(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let t = require_tag(params)?;
    let url = params
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: url"))?;
    tag::validate_tag(t).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.transaction(|store, vcs, root| {
        let mut meta = store.get_tag_meta(t)?.unwrap_or_default();
        meta.url = Some(url.to_owned());
        store.set_tag_meta(t, meta)?;
        let tag_path = storage::tag_meta_path(root, t);
        vcs.commit(&[tag_path], &format!("next: tag set-url {t}"))?;
        Ok(())
    })?;
    Ok(json!({ "tag": t, "url": url }))
}

fn clear_url(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let t = require_tag(params)?;
    tag::validate_tag(t).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.transaction(|store, vcs, root| {
        let mut meta = store
            .get_tag_meta(t)?
            .ok_or_else(|| anyhow::anyhow!("no metadata set for tag {t:?}"))?;
        meta.url = None;
        let tag_path = storage::tag_meta_path(root, t);
        if meta == TagMeta::default() {
            store.delete_tag_meta(t)?;
        } else {
            store.set_tag_meta(t, meta)?;
        }
        vcs.commit(&[tag_path], &format!("next: tag clear-url {t}"))?;
        Ok(())
    })?;
    Ok(json!({ "tag": t, "cleared": "url" }))
}

fn set_priority(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let t = require_tag(params)?;
    let priority_str = params
        .get("priority")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: priority"))?;
    tag::validate_tag(t).map_err(|e| anyhow::anyhow!("{e}"))?;
    let priority: Priority = priority_str.parse()?;
    ctx.transaction(|store, vcs, root| {
        let mut meta = store.get_tag_meta(t)?.unwrap_or_default();
        meta.priority = Some(priority);
        store.set_tag_meta(t, meta)?;
        let tag_path = storage::tag_meta_path(root, t);
        vcs.commit(&[tag_path], &format!("next: tag set-priority {t}"))?;
        Ok(())
    })?;
    Ok(json!({ "tag": t, "priority": priority_str }))
}

fn clear_priority(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let t = require_tag(params)?;
    tag::validate_tag(t).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.transaction(|store, vcs, root| {
        let mut meta = store
            .get_tag_meta(t)?
            .ok_or_else(|| anyhow::anyhow!("no metadata set for tag {t:?}"))?;
        meta.priority = None;
        let tag_path = storage::tag_meta_path(root, t);
        if meta == TagMeta::default() {
            store.delete_tag_meta(t)?;
        } else {
            store.set_tag_meta(t, meta)?;
        }
        vcs.commit(&[tag_path], &format!("next: tag clear-priority {t}"))?;
        Ok(())
    })?;
    Ok(json!({ "tag": t, "cleared": "priority" }))
}

fn set_no_time_urgency(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let t = require_tag(params)?;
    tag::validate_tag(t).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.transaction(|store, vcs, root| {
        let mut meta = store.get_tag_meta(t)?.unwrap_or_default();
        meta.no_time_urgency = true;
        store.set_tag_meta(t, meta)?;
        let tag_path = storage::tag_meta_path(root, t);
        vcs.commit(&[tag_path], &format!("next: tag set-no-time-urgency {t}"))?;
        Ok(())
    })?;
    Ok(json!({ "tag": t, "no_time_urgency": true }))
}

fn clear_no_time_urgency(params: &Value, ctx: &mut TaskRepository) -> anyhow::Result<Value> {
    let t = require_tag(params)?;
    tag::validate_tag(t).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.transaction(|store, vcs, root| {
        let mut meta = store
            .get_tag_meta(t)?
            .ok_or_else(|| anyhow::anyhow!("no metadata set for tag {t:?}"))?;
        meta.no_time_urgency = false;
        let tag_path = storage::tag_meta_path(root, t);
        if meta == TagMeta::default() {
            store.delete_tag_meta(t)?;
        } else {
            store.set_tag_meta(t, meta)?;
        }
        vcs.commit(&[tag_path], &format!("next: tag clear-no-time-urgency {t}"))?;
        Ok(())
    })?;
    Ok(json!({ "tag": t, "no_time_urgency": false }))
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
    fn describe_and_show_tag() {
        let (_dir, mut ctx) = make_ctx();
        manage_tag(
            &json!({ "action": "describe", "tag": "@work", "description": "Office tasks" }),
            &mut ctx,
        )
        .unwrap();
        let (result, _) =
            manage_tag(&json!({ "action": "show", "tag": "@work" }), &mut ctx).unwrap();
        assert_eq!(result["meta"]["description"], "Office tasks");
    }

    #[test]
    fn set_and_clear_priority() {
        let (_dir, mut ctx) = make_ctx();
        manage_tag(
            &json!({ "action": "set_priority", "tag": "@work", "priority": "high" }),
            &mut ctx,
        )
        .unwrap();
        manage_tag(
            &json!({ "action": "clear_priority", "tag": "@work" }),
            &mut ctx,
        )
        .unwrap();
        let (result, _) =
            manage_tag(&json!({ "action": "show", "tag": "@work" }), &mut ctx).unwrap();
        assert!(result["meta"]["priority"].is_null());
    }

    #[test]
    fn list_tags_empty() {
        let (_dir, mut ctx) = make_ctx();
        let (result, _) = manage_tag(&json!({ "action": "list" }), &mut ctx).unwrap();
        assert_eq!(result["contexts"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn clear_description_rejects_traversal_tag() {
        // Regression test: an unvalidated tag like "../../config/config" would
        // resolve tags/<tag>.toml outside the repo and delete a foreign file.
        let outer = tempfile::tempdir().unwrap();
        let repo_root = outer.path().join("repo");
        std::fs::create_dir(&repo_root).unwrap();
        crate::core::test_git::init_test_repo(&repo_root);
        let (store, vcs) = crate::core::storage::open(repo_root.clone()).unwrap();
        let mut ctx = TaskRepository::with_parts(Box::new(store), Box::new(vcs), repo_root);

        // Victim file outside the repo, exactly where the traversal tag points:
        // <repo>/tags/../../config/config.toml == <outer>/config/config.toml
        let victim_dir = outer.path().join("config");
        std::fs::create_dir(&victim_dir).unwrap();
        let victim = victim_dir.join("config.toml");
        std::fs::write(&victim, "secret = true\n").unwrap();

        let err = manage_tag(
            &json!({ "action": "clear_description", "tag": "../../config/config" }),
            &mut ctx,
        )
        .unwrap_err();
        assert!(err.to_string().contains("'..'"), "unexpected error: {err}");
        assert!(victim.exists(), "file outside the repo was deleted");
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "secret = true\n");
    }

    #[test]
    fn rename_moves_tasks_and_metadata() {
        use crate::core::domain::task::Task;

        let (_dir, mut ctx) = make_ctx();
        let mut task = Task::new("Tagged");
        task.tags = vec!["@work/frontend".into(), "python".into()];
        ctx.transaction(|store, vcs, root| {
            store.save_task(&task)?;
            vcs.commit(&[crate::core::storage::task_path(root, &task)], "next: add")?;
            Ok(())
        })
        .unwrap();
        manage_tag(
            &json!({ "action": "describe", "tag": "@work", "description": "Office" }),
            &mut ctx,
        )
        .unwrap();

        let (result, is_mutation) = manage_tag(
            &json!({ "action": "rename", "tag": "@work", "new_tag": "@office" }),
            &mut ctx,
        )
        .unwrap();
        assert!(is_mutation, "rename must trigger autosync");
        assert_eq!(result["active_tasks"], 1);
        assert_eq!(result["metas_moved"][0]["to"], "@office");
        assert_eq!(
            ctx.store.get_task(task.id).unwrap().tags,
            vec!["@office/frontend", "python"]
        );
    }

    #[test]
    fn rename_requires_new_tag() {
        let (_dir, mut ctx) = make_ctx();
        let err = manage_tag(&json!({ "action": "rename", "tag": "@work" }), &mut ctx).unwrap_err();
        assert!(
            err.to_string().contains("new_tag"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn unknown_action_errors() {
        let (_dir, mut ctx) = make_ctx();
        let err = manage_tag(&json!({ "action": "fly" }), &mut ctx).unwrap_err();
        assert!(err.to_string().contains("unknown action"));
    }
}
