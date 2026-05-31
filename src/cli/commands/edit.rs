use chrono::Local;
use super::add::validate_url;
use crate::cli::recurrence_parse::parse_recurrence;
use crate::domain::{
    date_parse::parse_date,
    recurrence::parse_snap,
    tag,
    task::Recurrence,
};

use crate::{resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task to edit: UUID, UUID prefix, or slug.
    pub id: String,

    /// New title.
    #[arg(long)]
    pub title: Option<String>,

    /// Due date (ISO 8601 or natural language).
    #[arg(long)]
    pub due: Option<String>,

    /// Start date.
    #[arg(long)]
    pub start: Option<String>,

    /// Priority (low, medium, high).
    #[arg(long)]
    pub priority: Option<String>,

    /// User-provided slug.
    #[arg(long)]
    pub slug: Option<String>,

    /// Assign to a user (or change assignee).
    #[arg(long)]
    pub assignee: Option<String>,

    /// Remove the assignee (make the task unassigned).
    #[arg(long)]
    pub clear_assignee: bool,

    /// Tags to attach (repeatable).
    #[arg(long = "tag", action = clap::ArgAction::Append)]
    pub tags: Vec<String>,

    /// Tags to remove (repeatable).
    #[arg(long = "remove-tag", action = clap::ArgAction::Append)]
    pub remove_tags: Vec<String>,

    /// Parent task: UUID, UUID prefix, or slug.
    #[arg(long)]
    pub parent: Option<String>,

    /// Explicit blocker task (repeatable).
    #[arg(long = "blocked-by", action = clap::ArgAction::Append)]
    pub blocked_by: Vec<String>,

    /// Longer description.
    #[arg(long)]
    pub description: Option<String>,

    /// Remove the description.
    #[arg(long)]
    pub clear_description: bool,

    /// URL associated with this task.
    #[arg(long)]
    pub url: Option<String>,

    /// Remove the URL.
    #[arg(long)]
    pub clear_url: bool,

    /// Free-text notes.
    #[arg(long)]
    pub notes: Option<String>,

    /// Schedule-based recurrence rule.
    #[arg(long)]
    pub recur_schedule: Option<String>,

    /// Completion-based recurrence interval in days.
    #[arg(long)]
    pub recur_completion: Option<u32>,

    /// Calendar snap applied to the computed next occurrence date.
    /// Values: next-workday, monday … sunday, dom:N (day-of-month).
    #[arg(long)]
    pub recur_snap: Option<String>,

    /// Remove the recurrence rule from this task.
    #[arg(long)]
    pub clear_recurrence: bool,

    /// Suppress age-based scoring.
    #[arg(long)]
    pub long_term: bool,

    /// Manual urgency score adjustment.
    #[arg(long)]
    pub adjust: Option<f64>,

    /// Remove the due date.
    #[arg(long)]
    pub clear_due: bool,

    /// Remove the start date.
    #[arg(long)]
    pub clear_start: bool,

    /// Remove the parent link.
    #[arg(long)]
    pub clear_parent: bool,

    /// Remove all explicit blockers.
    #[arg(long)]
    pub clear_blocked_by: bool,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    /// Trailing +tag / -tag tokens to add or remove tags.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub tag_tokens: Vec<String>,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let today = Local::now().date_naive();
    let id = resolve_task_id(&*ctx.store, &args.id)?;
    let mut task = ctx.store.get_task(id)?;

    if let Some(title) = args.title {
        task.title = title;
    }

    if args.clear_due {
        task.due = None;
    } else if let Some(expr) = args.due {
        task.due = Some(parse_date(&expr, today)?);
    }

    if args.clear_start {
        task.start = None;
    } else if let Some(expr) = args.start {
        task.start = Some(parse_date(&expr, today)?);
    }

    if let Some(p) = args.priority {
        task.priority = p.parse()?;
    }

    if let Some(slug) = args.slug {
        task.slug = Some(slug);
    }

    if args.clear_assignee {
        task.assignee = None;
    } else if let Some(assignee) = args.assignee {
        task.assignee = Some(assignee);
    }

    // Validate and add tags from --tag flags (deduplicate).
    for t in &args.tags {
        tag::validate_tag(t).map_err(|e| anyhow::anyhow!(e))?;
        if !task.tags.contains(t) {
            task.tags.push(t.clone());
        }
    }
    // Remove tags from --remove-tag flags.
    for t in &args.remove_tags {
        task.tags.retain(|existing| existing != t);
    }
    // Process trailing +tag / -tag tokens.
    for token in &args.tag_tokens {
        if let Some(t) = token.strip_prefix('+') {
            tag::validate_tag(t).map_err(|e| anyhow::anyhow!(e))?;
            if !task.tags.contains(&t.to_owned()) {
                task.tags.push(t.to_owned());
            }
        } else if let Some(t) = token.strip_prefix('-') {
            task.tags.retain(|existing| existing != t);
        } else {
            anyhow::bail!("unrecognised trailing argument {token:?} — use +tag to add or -tag to remove");
        }
    }

    if args.clear_parent {
        task.parent_id = None;
    } else if let Some(ref parent_ref) = args.parent {
        task.parent_id = Some(resolve_task_id(&*ctx.store, parent_ref)?);
    }

    if args.clear_blocked_by {
        task.blocked_by.clear();
    } else {
        for blocker_ref in &args.blocked_by {
            let bid = resolve_task_id(&*ctx.store, blocker_ref)?;
            if !task.blocked_by.contains(&bid) {
                task.blocked_by.push(bid);
            }
        }
    }

    if args.clear_description {
        task.description = None;
    } else if let Some(d) = args.description {
        task.description = Some(d);
    }

    if args.clear_url {
        task.url = None;
    } else if let Some(ref u) = args.url {
        validate_url(u)?;
        task.url = args.url;
    }

    if let Some(notes) = args.notes {
        task.notes = Some(notes);
    }

    if args.clear_recurrence {
        task.recurrence = None;
        task.recurrence_id = None;
    } else if args.recur_schedule.is_some() || args.recur_completion.is_some() {
        // Keep existing anchor if the task already has a Schedule rule; otherwise
        // derive anchor from start/due or fall back to today.
        let anchor = match &task.recurrence {
            Some(Recurrence::Schedule { anchor, .. }) => *anchor,
            _ => task.start.or(task.due).unwrap_or(today),
        };
        if let Some(recurrence) = parse_recurrence(
            args.recur_schedule,
            args.recur_completion,
            args.recur_snap.as_deref(),
            anchor,
        )? {
            task.recurrence = Some(recurrence);
            task.recurrence_id.get_or_insert(task.id);
        }
    } else if let Some(snap_str) = args.recur_snap {
        // Standalone --recur-snap: update the snap on an existing recurrence rule.
        let snap = Some(parse_snap(&snap_str)?);
        match &mut task.recurrence {
            Some(Recurrence::Schedule { snap: s, .. }) => *s = snap,
            Some(Recurrence::Completion { snap: s, .. }) => *s = snap,
            None => anyhow::bail!("--recur-snap requires an existing recurrence rule; use --recur-schedule or --recur-completion first"),
        }
    }

    if args.long_term {
        task.long_term = true;
    }

    if let Some(adj) = args.adjust {
        task.score_adjustment = adj;
    }

    task.touch();
    ctx.store.save_task(&task)?;

    let task_path = crate::storage::task_path(&ctx.repo_root, &task);
    ctx.vcs
        .commit(&[task_path], &format!("next: edit {}", task.title))?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        ctx.log
            .info("edit", &format!("[{}] {}", &task.id.to_string()[..8], task.title));
    }
    Ok(())
}

