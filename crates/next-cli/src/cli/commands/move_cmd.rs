use crate::{resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
#[command(name = "move")]
pub struct Args {
    /// Task to move: UUID, UUID prefix, or slug.
    pub id: String,

    /// New parent task: UUID, UUID prefix, or slug. Pass "none" to remove.
    #[arg(long)]
    pub parent: Option<String>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let id = resolve_task_id(&*ctx.store, &args.id)?;
    let mut task = ctx.store.get_task(id)?;

    match args.parent.as_deref() {
        Some("none") => task.parent_id = None,
        Some(parent_ref) => {
            task.parent_id = Some(resolve_task_id(&*ctx.store, parent_ref)?);
        }
        None => {}
    }

    task.touch();
    ctx.store.save_task(&task)?;

    let task_path = next_storage::task_path(&ctx.repo_root, &task);
    ctx.vcs
        .commit(&[task_path], &format!("next: move {}", task.title))?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        ctx.log
            .info("move", &format!("[{}] {}", &task.id.to_string()[..8], task.title));
    }
    Ok(())
}
