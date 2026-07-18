use crate::{core::resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task to cancel: UUID, UUID prefix, or slug.
    pub id: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let id = resolve_task_id(&*ctx.repo.store, &args.id)?;
    let task = ctx.repo.transaction(|store, vcs, root| {
        let mut task = store.get_task(id)?;
        task.mark_cancelled();
        store.save_task(&task)?;
        let path = crate::core::storage::task_path(root, &task);
        vcs.commit(&[path], &format!("next: cancel {}", task.title))?;
        Ok(task)
    })?;
    ctx.repo.record_task_event("cancel", task.id);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        println!("cancelled [{}] {}", &task.id.to_string()[..8], task.title);
        tracing::info!(cmd = "cancel", "[{}] {}", &task.id.to_string()[..8], task.title);
    }
    Ok(())
}
