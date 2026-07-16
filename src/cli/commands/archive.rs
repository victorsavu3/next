use chrono::Local;

use crate::core::archiver;
use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let today = Local::now().date_naive();
    let outcome = archiver::run_archive_pass(&mut ctx.repo, today)?;

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "archived": outcome.archived,
                "segments": outcome.segments,
                "pruned": outcome.pruned,
            })
        );
        return Ok(());
    }
    if outcome.archived == 0 && outcome.pruned.is_empty() {
        println!("Nothing to archive.");
    }
    if outcome.archived > 0 {
        println!(
            "Archived {} task(s) into {} segment(s):",
            outcome.archived,
            outcome.segments.len()
        );
        for seg in &outcome.segments {
            println!("  {seg}");
        }
    }
    if !outcome.pruned.is_empty() {
        println!("Pruned {} segment(s) to the cold tier:", outcome.pruned.len());
        for seg in &outcome.pruned {
            println!("  {seg}");
        }
    }
    Ok(())
}
