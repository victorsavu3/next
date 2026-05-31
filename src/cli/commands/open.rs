use crate::{resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task whose URL to open: UUID, UUID prefix, or slug.
    pub id: String,
}

pub fn run(args: Args, ctx: &AppContext) -> anyhow::Result<()> {
    let id = resolve_task_id(ctx.store(), &args.id)?;
    let task = ctx.store().get_task(id)?;

    let url = task
        .url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("task [{}] has no URL set", &task.id.to_string()[..8]))?;

    open_url(url)?;
    ctx.log
        .info("open", &format!("[{}] {url}", &task.id.to_string()[..8]));
    Ok(())
}

fn open_url(url: &str) -> anyhow::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };

    let status = std::process::Command::new(opener)
        .arg(url)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to run {opener}: {e}"))?;

    if !status.success() {
        anyhow::bail!("{opener} exited with status {status}");
    }
    Ok(())
}
