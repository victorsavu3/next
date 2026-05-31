pub mod add;
pub mod cancel;
pub mod context;
pub mod data;
pub mod delete;
pub mod done;
pub mod edit;
pub mod export;
pub mod forecast;
pub mod import;
pub mod init;
pub mod list;
pub mod move_cmd;
pub mod next_cmd;
pub mod open;
pub mod resource;
pub mod show;
pub mod start;
pub mod stop;
pub mod sync;
pub mod tag;
pub mod tree;
pub mod tutorial;
pub mod user;

use std::path::PathBuf;
use crate::{domain::task::Task, resolve::resolve_task_id, AppContext};

/// Resolves `id_str`, applies `mutate` to the task, saves it, and returns
/// `(task, task_path)`.  The caller is responsible for the git commit so it
/// can include additional paths (e.g. a spawned recurrence instance).
pub fn change_status(
    ctx: &mut AppContext,
    id_str: &str,
    mutate: impl FnOnce(&mut Task),
) -> anyhow::Result<(Task, PathBuf)> {
    let id = resolve_task_id(&*ctx.store, id_str)?;
    let mut task = ctx.store.get_task(id)?;
    mutate(&mut task);
    ctx.store.save_task(&task)?;
    let path = crate::storage::task_path(&ctx.repo_root, &task);
    Ok((task, path))
}
