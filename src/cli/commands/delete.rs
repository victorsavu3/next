use crate::{core::resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task to delete: UUID, UUID prefix, or slug.
    pub id: String,

    /// Skip confirmation prompt.
    #[arg(long)]
    pub yes: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let id = resolve_task_id(&*ctx.repo.store, &args.id)?;
    let task = ctx.repo.store.get_task(id)?;

    if !args.yes {
        eprint!("Delete {:?}? [y/N] ", task.title);
        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .map_err(|e| anyhow::anyhow!("failed to read input: {e}"))?;
        if !matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            anyhow::bail!("aborted");
        }
    }

    ctx.repo.transaction(|store, vcs, root| {
        // Compute path before deleting so we can stage the removal.
        let task_path = crate::core::storage::task_path(root, &task);
        store.delete_task(id)?;
        vcs.commit(&[task_path], &format!("next: delete {}", task.title))?;
        Ok(())
    })?;
    ctx.repo.record_task_event("delete", id);

    println!("deleted [{}] {}", &task.id.to_string()[..8], task.title);
    tracing::info!(cmd = "delete", "[{}] {}", &task.id.to_string()[..8], task.title);
    Ok(())
}
