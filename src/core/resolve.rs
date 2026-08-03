use crate::core::store::Store;
use anyhow::bail;
use uuid::Uuid;

/// Resolves a user-supplied identifier to a task UUID.
///
/// Resolution order:
/// 1. Full UUID string (e.g. `"550e8400-e29b-41d4-a716-446655440000"`).
/// 2. User slug (e.g. `"water-plants"`).
/// 3. UUID hex prefix — minimum 4 hex chars (e.g. `"550e8400"`).
///
/// Returns an error when no match is found or when a prefix matches more than
/// one task (disambiguation required).
pub fn resolve_task_id(store: &dyn Store, id_str: &str) -> anyhow::Result<Uuid> {
    if let Ok(uuid) = Uuid::parse_str(id_str) {
        return Ok(uuid);
    }

    if let Some(task) = store.get_task_by_slug(id_str)? {
        return Ok(task.id);
    }

    if id_str.len() >= 4 {
        let matches = store.find_tasks_by_prefix(id_str)?;
        match matches.len() {
            1 => return Ok(matches[0].id),
            0 => {}
            n => bail!("{n} tasks match prefix {id_str:?} — provide more characters"),
        }
    }

    bail!("no task found for {:?}", id_str)
}
