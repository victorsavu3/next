use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task to cancel: UUID, UUID prefix, or slug.
    pub id: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let (task, path) = super::change_status(ctx, &args.id, |t| t.mark_cancelled())?;
    ctx.vcs.commit(&[path], &format!("next: cancel {}", task.title))?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        ctx.log.info("cancel", &format!("[{}] {}", &task.id.to_string()[..8], task.title));
    }
    Ok(())
}
