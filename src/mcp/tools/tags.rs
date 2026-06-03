use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::domain::tag::{self, TagKind, TagMeta};
use crate::domain::task::Priority;
use crate::storage;
use crate::AppContext;


/// Unified tag metadata tool.
///
/// `action` values:
///   read:  "list", "show"
///   write: "describe", "clear_description", "set_url", "clear_url",
///          "set_priority", "clear_priority",
///          "set_no_time_urgency", "clear_no_time_urgency"
pub fn manage_tag(params: &Value, ctx: &mut AppContext) -> anyhow::Result<(Value, bool)> {
    let action = params
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: action"))?;

    match action {
        "list" => Ok((list(ctx)?, false)),
        "show" => Ok((show(params, ctx)?, false)),
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
pub fn tag_catalog(ctx: &AppContext) -> anyhow::Result<TagCatalog> {
    let metas = ctx.store.list_tag_metas()?;
    let tasks = ctx.store.list_tasks()?;

    let mut all_tags: BTreeSet<String> = BTreeSet::new();
    for task in &tasks {
        for t in &task.tags { all_tags.insert(t.clone()); }
    }
    for t in metas.keys() { all_tags.insert(t.clone()); }

    let mut catalog = TagCatalog::default();
    for t in &all_tags {
        let meta = metas.get(t.as_str()).cloned().unwrap_or_default();
        let entry = CatalogEntry { tag: t.clone(), meta };
        match tag::classify(t) {
            TagKind::Context  => catalog.contexts.push(entry),
            TagKind::Resource => catalog.resources.push(entry),
            TagKind::Freeform => catalog.freeform.push(entry),
        }
    }
    Ok(catalog)
}

fn list(ctx: &AppContext) -> anyhow::Result<Value> {
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

fn show(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
    let t = require_tag(params)?;
    tag::validate_tag(t).map_err(|e| anyhow::anyhow!("{e}"))?;
    let meta = ctx.store.get_tag_meta(t)?.unwrap_or_default();
    Ok(json!({ "tag": t, "meta": meta }))
}

fn describe(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
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

fn clear_description(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
    let t = require_tag(params)?;
    ctx.transaction(|store, vcs, root| {
        store.delete_tag_description(t)?;
        let tag_path = storage::tag_meta_path(root, t);
        vcs.commit(&[tag_path], &format!("next: tag clear-description {t}"))?;
        Ok(())
    })?;
    Ok(json!({ "tag": t, "cleared": "description" }))
}

fn set_url(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
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

fn clear_url(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
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

fn set_priority(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
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

fn clear_priority(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
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

fn set_no_time_urgency(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
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

fn clear_no_time_urgency(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
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

    #[test]
    fn describe_and_show_tag() {
        let (_dir, mut ctx) = make_ctx();
        manage_tag(&json!({ "action": "describe", "tag": "@work", "description": "Office tasks" }), &mut ctx).unwrap();
        let (result, _) = manage_tag(&json!({ "action": "show", "tag": "@work" }), &mut ctx).unwrap();
        assert_eq!(result["meta"]["description"], "Office tasks");
    }

    #[test]
    fn set_and_clear_priority() {
        let (_dir, mut ctx) = make_ctx();
        manage_tag(&json!({ "action": "set_priority", "tag": "@work", "priority": "high" }), &mut ctx).unwrap();
        manage_tag(&json!({ "action": "clear_priority", "tag": "@work" }), &mut ctx).unwrap();
        let (result, _) = manage_tag(&json!({ "action": "show", "tag": "@work" }), &mut ctx).unwrap();
        assert!(result["meta"]["priority"].is_null());
    }

    #[test]
    fn list_tags_empty() {
        let (_dir, mut ctx) = make_ctx();
        let (result, _) = manage_tag(&json!({ "action": "list" }), &mut ctx).unwrap();
        assert_eq!(result["contexts"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn unknown_action_errors() {
        let (_dir, mut ctx) = make_ctx();
        let err = manage_tag(&json!({ "action": "fly" }), &mut ctx).unwrap_err();
        assert!(err.to_string().contains("unknown action"));
    }
}
