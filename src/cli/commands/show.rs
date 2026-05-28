use chrono::Local;
use crate::domain::scoring;

use crate::{resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task to show: UUID, UUID prefix, or slug.
    pub id: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let today = Local::now().date_naive();
    let id = resolve_task_id(&*ctx.store, &args.id)?;
    let task = ctx.store.get_task(id)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
        return Ok(());
    }

    let all_tasks = ctx.store.list_tasks()?;
    let parent = task
        .parent_id
        .and_then(|pid| all_tasks.iter().find(|t| t.id == pid));

    let score = scoring::score(&task, parent, today, &ctx.config.scoring);

    let short_id = task.id.to_string().replace('-', "");
    println!("ID:       {}", &short_id[..8]);
    println!("Title:    {}", task.title);
    println!("Status:   {:?}", task.status);
    println!("Priority: {:?}", task.priority);
    println!("Score:    {:.2}", score);

    if let Some(due) = task.due {
        println!("Due:      {due}");
    }
    if let Some(start) = task.start {
        println!("Start:    {start}");
    }
    if !task.tags.is_empty() {
        println!("Tags:     {}", task.tags.join("  "));
    }
    if let Some(p) = parent {
        println!("Parent:   [{}] {}", &p.id.to_string()[..8], p.title);
    }

    let children: Vec<_> = all_tasks
        .iter()
        .filter(|t| t.parent_id == Some(task.id))
        .collect();
    if !children.is_empty() {
        println!("Children:");
        for child in &children {
            println!("  [{}] {}", &child.id.to_string()[..8], child.title);
        }
    }
    if !task.blocked_by.is_empty() {
        let blocker_strs: Vec<String> = task
            .blocked_by
            .iter()
            .map(|bid| {
                all_tasks
                    .iter()
                    .find(|t| t.id == *bid)
                    .map(|t| format!("[{}] {}", &t.id.to_string()[..8], t.title))
                    .unwrap_or_else(|| bid.to_string())
            })
            .collect();
        println!("Blocked:  {}", blocker_strs.join(", "));
    }
    if let Some(ref assignee) = task.assignee {
        println!("Assignee: {assignee}");
    }
    if let Some(ref slug) = task.slug {
        println!("Slug:     {slug}");
    }
    if let Some(ref rec) = task.recurrence {
        match rec {
            crate::domain::task::Recurrence::Schedule { rule } => {
                println!("Recur:    schedule ({rule})");
            }
            crate::domain::task::Recurrence::Completion { interval_days } => {
                println!("Recur:    {interval_days}d after completion");
            }
        }
    }
    println!(
        "Created:  {}",
        task.created_at.format("%Y-%m-%d %H:%M UTC")
    );
    println!(
        "Updated:  {}",
        task.updated_at.format("%Y-%m-%d %H:%M UTC")
    );
    if !task.data.is_empty() {
        let mut keys: Vec<&String> = task.data.keys().collect();
        keys.sort();
        for key in keys {
            println!("Data[{key}]: {}", task.data[key]);
        }
    }
    if let Some(ref url) = task.url {
        println!("URL:      {url}");
    }
    if let Some(ref desc) = task.description {
        println!("\nDescription:\n{desc}");
    }
    if let Some(ref notes) = task.notes {
        println!("\nNotes:\n{notes}");
    }

    Ok(())
}
