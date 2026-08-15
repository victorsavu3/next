use crate::core::scoring;
use chrono::Local;

use crate::{core::resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task to show: UUID, UUID prefix, or slug.
    pub id: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    /// Return only these fields, e.g. `--fields id,title,due`. JSON output
    /// only. Applies to the task and its children alike.
    #[arg(long, value_delimiter = ',')]
    pub fields: Vec<String>,
}

pub fn run(args: Args, ctx: &AppContext) -> anyhow::Result<()> {
    let today = Local::now().date_naive();
    let id = resolve_task_id(ctx.repo.store(), &args.id)?;
    let task = ctx.repo.store().get_task(id)?;

    let tag_metas = ctx.repo.store().list_tag_metas()?;
    let parent = match task.parent_id {
        Some(pid) => ctx.repo.store().get_tasks(&[pid])?.pop(),
        None => None,
    };
    let mut dated: Vec<_> = vec![task.clone()];
    dated.extend(parent.clone());
    let task_dates = ctx.repo.task_git_dates_for(&dated);

    let bd = scoring::score_with_breakdown(
        &task,
        parent.as_ref(),
        &task_dates,
        today,
        &ctx.repo.scoring,
        &tag_metas,
    );

    if args.json {
        let children = ctx
            .repo
            .store()
            .query_tasks(&crate::core::TaskQuery {
                parent_id: Some(task.id),
                ..crate::core::TaskQuery::unpaginated()
            })?
            .items;
        let projection = crate::core::projection::Projection::parse(&args.fields)?;
        let mut json = serde_json::json!({
            "task": task,
            "score": bd.total,
            "score_breakdown": bd,
            "children": children,
        });
        // Children are tasks too, and a caller asking for `id,title` wants that
        // shape throughout rather than one trimmed task beside a set of full
        // ones. `score` and `score_breakdown` go unless named, as in `list`.
        projection.apply_to_detail(&mut json);
        println!("{}", serde_json::to_string_pretty(&json)?);
        return Ok(());
    }

    let short_id = task.id.to_string().replace('-', "");
    println!("ID:       {}", &short_id[..8]);
    println!("Title:    {}", task.title);
    println!("Status:   {}", task.status);
    println!("Priority: {}", task.priority);

    // Score line + compact breakdown of non-zero factors.
    let mut parts: Vec<String> = bd
        .nonzero_factors()
        .into_iter()
        .map(|(label, value)| format!("{label} {value:.2}"))
        .collect();
    if bd.no_time_urgency {
        parts.push("no-time-urgency".to_string());
    }
    if parts.is_empty() {
        println!("Score:    {:.2}", bd.total);
    } else {
        println!("Score:    {:.2}  ({})", bd.total, parts.join("  "));
    }

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

    let children = ctx
        .repo
        .store()
        .query_tasks(&crate::core::TaskQuery {
            parent_id: Some(task.id),
            ..crate::core::TaskQuery::unpaginated()
        })?
        .items;
    if !children.is_empty() {
        println!("Children:");
        for child in &children {
            println!("  [{}] {}", &child.id.to_string()[..8], child.title);
        }
    }
    if !task.blocked_by.is_empty() {
        let blockers = ctx.repo.store().get_tasks(&task.blocked_by)?;
        let blocker_strs: Vec<String> = task
            .blocked_by
            .iter()
            .map(|bid| {
                blockers
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
            crate::core::domain::task::Recurrence::Schedule { rrule, .. } => {
                println!("Recur:    schedule ({rrule})");
            }
            crate::core::domain::task::Recurrence::Completion { interval_days, .. } => {
                println!("Recur:    {interval_days}d after completion");
            }
        }
    }
    if let Some(dates) = task_dates.get(&task.id) {
        println!(
            "Created:  {}",
            dates.created_at.format("%Y-%m-%d %H:%M UTC")
        );
        println!(
            "Updated:  {}",
            dates.updated_at.format("%Y-%m-%d %H:%M UTC")
        );
    }
    if let Some(completed) = task.completed_at {
        println!("Completed: {}", completed.format("%Y-%m-%d"));
    }
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
