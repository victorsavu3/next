use chrono::Local;
use crate::core::recurrence::parse_recurrence;
use crate::core::domain::{date_parse::parse_date, tag, task::Recurrence};
use crate::core::recurrence::parse_snap;
use crate::core::service::{apply_edits, validate_url, EditTaskParams};

use crate::{core::resolve::resolve_task_id, AppContext};

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
    let id = resolve_task_id(&*ctx.repo.store, &args.id)?;

    // Resolve dates.
    let due = if args.clear_due {
        None
    } else {
        args.due.map(|expr| parse_date(&expr, today)).transpose()?
    };
    let start = if args.clear_start {
        None
    } else {
        args.start.map(|expr| parse_date(&expr, today)).transpose()?
    };

    // Validate URL before building params (so we can report errors early).
    if let Some(ref u) = args.url {
        if !args.clear_url {
            validate_url(u)?;
        }
    }

    // Validate tags from --tag and --remove-tag. Removals are validated too:
    // an invalid tag can never be on a task, so a misspelt removal would
    // otherwise no-op silently.
    for t in args.tags.iter().chain(args.remove_tags.iter()) {
        tag::validate_tag(t).map_err(|e| anyhow::anyhow!(e))?;
    }

    // Process trailing +tag / -tag tokens. An unknown `--flag` (typically a
    // typo of a real flag) would land here because of allow_hyphen_values;
    // reject it up front instead of misreading it as a `-tag` removal.
    crate::core::reject_flag_like_tokens(&args.tag_tokens, "next edit --help")?;
    let mut add_tags = args.tags.clone();
    let mut remove_tags = args.remove_tags.clone();
    for token in &args.tag_tokens {
        if let Some(t) = token.strip_prefix('+') {
            tag::validate_tag(t).map_err(|e| anyhow::anyhow!(e))?;
            add_tags.push(t.to_owned());
        } else if let Some(t) = token.strip_prefix('-') {
            tag::validate_tag(t).map_err(|e| anyhow::anyhow!(e))?;
            remove_tags.push(t.to_owned());
        } else {
            anyhow::bail!("unrecognised trailing argument {token:?} — use +tag to add or -tag to remove");
        }
    }

    // Build the recurrence update if any recurrence flags are set.
    //
    // We need the current task's anchor to preserve it when editing an existing
    // Schedule rule — so load the task here just for that.
    let recurrence: Option<Recurrence> = if args.clear_recurrence {
        None // handled via clear_recurrence flag
    } else if args.recur_schedule.is_some() || args.recur_completion.is_some() {
        let existing = ctx.repo.store.get_task(id)?;
        let anchor = match &existing.recurrence {
            Some(Recurrence::Schedule { anchor, .. }) => *anchor,
            _ => existing.start.or(existing.due).unwrap_or(today),
        };
        parse_recurrence(
            args.recur_schedule,
            args.recur_completion,
            args.recur_snap.as_deref(),
            anchor,
        )?
    } else if let Some(ref snap_str) = args.recur_snap {
        // Standalone --recur-snap: update the snap on an existing recurrence rule.
        let snap = Some(parse_snap(snap_str)?);
        let existing = ctx.repo.store.get_task(id)?;
        match existing.recurrence {
            Some(Recurrence::Schedule { rrule, anchor, .. }) => {
                Some(Recurrence::Schedule { rrule, anchor, snap })
            }
            Some(Recurrence::Completion { interval_days, .. }) => {
                Some(Recurrence::Completion { interval_days, snap })
            }
            None => anyhow::bail!(
                "--recur-snap requires an existing recurrence rule; use --recur-schedule or --recur-completion first"
            ),
        }
    } else {
        None
    };

    let edits = EditTaskParams {
        title: args.title,
        due,
        clear_due: args.clear_due,
        start,
        clear_start: args.clear_start,
        priority: args.priority,
        slug: args.slug,
        assignee: args.assignee,
        clear_assignee: args.clear_assignee,
        add_tags,
        remove_tags,
        parent: args.parent,
        clear_parent: args.clear_parent,
        blocked_by: args.blocked_by,
        clear_blocked_by: args.clear_blocked_by,
        description: args.description,
        clear_description: args.clear_description,
        url: args.url,
        clear_url: args.clear_url,
        notes: args.notes,
        recurrence,
        clear_recurrence: args.clear_recurrence,
        long_term: if args.long_term { Some(true) } else { None },
        score_adjustment: args.adjust,
    };

    let task = apply_edits(
        id,
        edits,
        today,
        &ctx.repo.repo_root.clone(),
        &mut *ctx.repo.store,
        &*ctx.repo.vcs,
    )?;
    ctx.repo.record_task_event("edit", task.id);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        println!("edited [{}] {}", &task.id.to_string()[..8], task.title);
        tracing::info!(cmd = "edit", "[{}] {}", &task.id.to_string()[..8], task.title);
    }
    Ok(())
}
