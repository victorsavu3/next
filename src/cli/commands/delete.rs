use crate::{resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task to delete: UUID, UUID prefix, or slug.
    pub id: String,

    /// Skip confirmation prompt.
    #[arg(long)]
    pub yes: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let id = resolve_task_id(&*ctx.store, &args.id)?;
    let task = ctx.store.get_task(id)?;

    if !args.yes {
        anyhow::bail!(
            "this will permanently delete {:?} — rerun with --yes to confirm",
            task.title
        );
    }

    // Compute path before deleting so we can stage the removal.
    let task_path = crate::storage::task_path(&ctx.repo_root, &task);
    ctx.store.delete_task(id)?;
    ctx.vcs
        .commit(&[task_path], &format!("next: delete {}", task.title))?;

    ctx.log
        .info("delete", &format!("[{}] {}", &task.id.to_string()[..8], task.title));
    Ok(())
}
