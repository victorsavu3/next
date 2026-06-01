use serde_json::{json, Value};

use crate::cli::commands::data::parse_value;
use crate::domain::task::validate_key;
use crate::resolve::resolve_task_id;
use crate::storage;
use crate::AppContext;

/// Unified task data key-value tool.
///
/// `action` values:
///   read:  "get", "list"
///   write: "set", "unset"
///
/// Returns `(result_value, is_mutation)`.
pub fn manage_task_data(params: &Value, ctx: &mut AppContext) -> anyhow::Result<(Value, bool)> {
    let action = params
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: action"))?;

    match action {
        "get"   => Ok((get(params, ctx)?, false)),
        "list"  => Ok((list(params, ctx)?, false)),
        "set"   => Ok((set(params, ctx)?, true)),
        "unset" => Ok((unset(params, ctx)?, true)),
        other   => anyhow::bail!("unknown action {other:?}: expected get, list, set, unset"),
    }
}

fn require_id(params: &Value) -> anyhow::Result<&str> {
    params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: id"))
}

fn get(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
    let id_str = require_id(params)?;
    let key = params
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: key"))?;
    validate_key(key)?;

    let id = resolve_task_id(&*ctx.store, id_str)?;
    let task = ctx.store.get_task(id)?;

    let value = task
        .data
        .get(key)
        .ok_or_else(|| anyhow::anyhow!("task [{}] has no data key {key:?}", &task.id.to_string()[..8]))?;

    Ok(value.clone())
}

fn list(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
    let id_str = require_id(params)?;
    let id = resolve_task_id(&*ctx.store, id_str)?;
    let task = ctx.store.get_task(id)?;
    Ok(serde_json::to_value(&task.data)?)
}

fn set(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
    let id_str = require_id(params)?;
    let key = params
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: key"))?;
    validate_key(key)?;
    let raw = params
        .get("value")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: value"))?;

    let value = parse_value(raw)?;

    let id = resolve_task_id(&*ctx.store, id_str)?;
    let mut task = ctx.store.get_task(id)?;
    task.data.insert(key.to_owned(), value.clone());
    task.touch();

    let task_path = storage::task_path(&ctx.repo_root, &task);
    ctx.store.save_task(&task)?;
    ctx.vcs.commit(&[task_path], &format!("next: data set {} on {}", key, task.title))?;

    Ok(json!({ "id": task.id.to_string(), "key": key, "value": value }))
}

fn unset(params: &Value, ctx: &mut AppContext) -> anyhow::Result<Value> {
    let id_str = require_id(params)?;
    let key = params
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: key"))?;
    validate_key(key)?;

    let id = resolve_task_id(&*ctx.store, id_str)?;
    let mut task = ctx.store.get_task(id)?;

    if !task.data.contains_key(key) {
        anyhow::bail!("task [{}] has no data key {key:?}", &task.id.to_string()[..8]);
    }
    task.data.remove(key);
    task.touch();

    let task_path = storage::task_path(&ctx.repo_root, &task);
    ctx.store.save_task(&task)?;
    ctx.vcs.commit(&[task_path], &format!("next: data unset {} on {}", key, task.title))?;

    Ok(json!({ "id": task.id.to_string(), "unset": key }))
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

    fn add_task_raw(title: &str, ctx: &mut AppContext) -> String {
        use crate::domain::task::Task;
        use crate::storage;
        let task = Task::new(title.to_owned());
        let path = storage::task_path(&ctx.repo_root, &task);
        ctx.store.save_task(&task).unwrap();
        ctx.vcs.commit(&[path], &format!("next: add {title}")).unwrap();
        task.id.to_string()
    }

    #[test]
    fn set_get_unset_data() {
        let (_dir, mut ctx) = make_ctx();
        let id = add_task_raw("Test task", &mut ctx);

        let (_, is_mut) = manage_task_data(&json!({ "action": "set", "id": id, "key": "score", "value": "42" }), &mut ctx).unwrap();
        assert!(is_mut);

        let (val, _) = manage_task_data(&json!({ "action": "get", "id": id, "key": "score" }), &mut ctx).unwrap();
        assert_eq!(val, 42);

        let (all, _) = manage_task_data(&json!({ "action": "list", "id": id }), &mut ctx).unwrap();
        assert_eq!(all["score"], 42);

        manage_task_data(&json!({ "action": "unset", "id": id, "key": "score" }), &mut ctx).unwrap();
        let err = manage_task_data(&json!({ "action": "get", "id": id, "key": "score" }), &mut ctx).unwrap_err();
        assert!(err.to_string().contains("no data key"));
    }

    #[test]
    fn unset_missing_key_errors() {
        let (_dir, mut ctx) = make_ctx();
        let id = add_task_raw("Test", &mut ctx);
        let err = manage_task_data(&json!({ "action": "unset", "id": id, "key": "nope" }), &mut ctx).unwrap_err();
        assert!(err.to_string().contains("no data key"));
    }

    #[test]
    fn validate_key_rejects_oversized_key() {
        let (_dir, mut ctx) = make_ctx();
        let id = add_task_raw("Test", &mut ctx);
        let long_key = "a".repeat(257);
        let err = manage_task_data(
            &json!({ "action": "set", "id": id, "key": long_key, "value": "1" }),
            &mut ctx,
        )
        .unwrap_err();
        assert!(err.to_string().contains("too long"), "expected 'too long' in: {err}");
    }

    #[test]
    fn validate_key_rejects_space() {
        let (_dir, mut ctx) = make_ctx();
        let id = add_task_raw("Test", &mut ctx);
        let err = manage_task_data(
            &json!({ "action": "set", "id": id, "key": "bad key", "value": "1" }),
            &mut ctx,
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid character"), "expected 'invalid character' in: {err}");
    }

    #[test]
    fn validate_key_rejects_slash() {
        let (_dir, mut ctx) = make_ctx();
        let id = add_task_raw("Test", &mut ctx);
        let err = manage_task_data(
            &json!({ "action": "set", "id": id, "key": "path/traversal", "value": "1" }),
            &mut ctx,
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid character"), "expected 'invalid character' in: {err}");
    }

    #[test]
    fn validate_key_rejects_dot() {
        let (_dir, mut ctx) = make_ctx();
        let id = add_task_raw("Test", &mut ctx);
        let err = manage_task_data(
            &json!({ "action": "set", "id": id, "key": "some.key", "value": "1" }),
            &mut ctx,
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid character"), "expected 'invalid character' in: {err}");
    }

    #[test]
    fn validate_key_unit_rejects_empty() {
        use crate::domain::task::validate_key;
        assert!(validate_key("").is_err());
    }

    #[test]
    fn validate_key_unit_accepts_valid_keys() {
        use crate::domain::task::validate_key;
        assert!(validate_key("score").is_ok());
        assert!(validate_key("my-key_123").is_ok());
        assert!(validate_key(&"a".repeat(256)).is_ok());
    }
}
