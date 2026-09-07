use crate::core::domain::date_parse::parse_date;
use crate::core::domain::task::{Recurrence, Snap};
use crate::core::recurrence::parse_recurrence;
use crate::core::service::{create_task, CreateTaskParams};
use chrono::Local;

use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task title.
    pub title: String,

    /// Due date (ISO 8601 or natural language, e.g. "in two weeks").
    #[arg(long)]
    pub due: Option<String>,

    /// Start date — task is hidden until this date.
    #[arg(long)]
    pub start: Option<String>,

    /// Task priority (low, medium, high).
    #[arg(long)]
    pub priority: Option<String>,

    /// User-provided slug for stable referencing (e.g. "water-plants").
    #[arg(long)]
    pub slug: Option<String>,

    /// Assign to a specific user.
    #[arg(long)]
    pub assignee: Option<String>,

    /// Tags to attach (repeatable). Use @ for context, # for resource.
    #[arg(long = "tag", action = clap::ArgAction::Append)]
    pub tags: Vec<String>,

    /// Parent task: UUID, UUID prefix, or slug.
    #[arg(long)]
    pub parent: Option<String>,

    /// Explicit blocker task: UUID, UUID prefix, or slug (repeatable).
    #[arg(long = "blocked-by", action = clap::ArgAction::Append)]
    pub blocked_by: Vec<String>,

    /// Longer description (multi-line context or detail).
    #[arg(long)]
    pub description: Option<String>,

    /// URL associated with this task (ticket, doc, reference link).
    #[arg(long)]
    pub url: Option<String>,

    /// Free-text notes.
    #[arg(long)]
    pub notes: Option<String>,

    /// Schedule-based recurrence rule, e.g. "every Monday".
    #[arg(long)]
    pub recur_schedule: Option<String>,

    /// Completion-based recurrence interval in days.
    #[arg(long)]
    pub recur_completion: Option<u32>,

    /// Calendar snap applied to the computed next occurrence date.
    /// Values: next-workday, monday … sunday, dom:N (day-of-month).
    #[arg(long)]
    pub recur_snap: Option<String>,

    /// N (both directions), BACK,FORWARD, or BACK,* for unbounded forward. Days.
    #[arg(long)]
    pub recur_snap_leeway: Option<String>,

    /// Suppress age-based scoring (suitable for long-running tasks).
    #[arg(long)]
    pub long_term: bool,

    /// Manual urgency score adjustment (positive boosts, negative penalises).
    #[arg(long)]
    pub adjust: Option<f64>,

    /// Output result as JSON.
    #[arg(long)]
    pub json: bool,

    /// Suppress the no-leeway hint (set from the global `--quiet`, not a flag
    /// of its own).
    #[arg(skip)]
    pub quiet: bool,
}

/// The note printed when a snap is set with no leeway to bound it.
///
/// Its subject is a *default*, so there is no wrong value to complain about
/// and nothing to reject — only a consequence the user cannot see in what they
/// typed. Hence a hint on `add` rather than a different default (D4): every
/// stored task keeps the dates it already has, and the one person who can
/// still choose differently is told at the moment of choosing.
const NO_LEEWAY_HINT: &str = "\
hint: with no leeway, a snap only ever moves dates later — completing
      this task late will lengthen its cycle. --recur-snap-leeway 3
      keeps the cadence. See `next tutorial`.";

/// Whether this rule is one whose forward-only default silently stretches a
/// cycle: the snaps whose boundaries are far enough apart for it to matter.
///
/// `next-workday` is excluded deliberately — it can push a date by at most two
/// days, which is a rounding, not a skipped cycle.
fn wants_leeway_hint(recurrence: Option<&Recurrence>) -> bool {
    let Some(rule) = recurrence else {
        return false;
    };
    if rule.snap_leeway().is_some() {
        return false;
    }
    matches!(
        rule.snap(),
        Some(Snap::DayOfMonth { .. } | Snap::NextWeekday { .. })
    )
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let today = Local::now().date_naive();

    let due = args.due.map(|expr| parse_date(&expr, today)).transpose()?;
    let start = args
        .start
        .map(|expr| parse_date(&expr, today))
        .transpose()?;

    let anchor = start.or(due).unwrap_or(today);
    let recurrence = parse_recurrence(
        args.recur_schedule,
        args.recur_completion,
        args.recur_snap.as_deref(),
        args.recur_snap_leeway.as_deref(),
        // Clearing a leeway on a task being created has nothing to clear, so
        // `add` carries no `--clear-recur-snap-leeway` to pass on.
        false,
        anchor,
    )?;
    let hint = wants_leeway_hint(recurrence.as_ref());

    let params = CreateTaskParams {
        due,
        start,
        priority: args.priority,
        slug: args.slug,
        assignee: args.assignee,
        tags: args.tags,
        parent: args.parent,
        blocked_by: args.blocked_by,
        description: args.description,
        url: args.url,
        notes: args.notes,
        long_term: args.long_term,
        score_adjustment: args.adjust,
        recurrence,
    };

    let task = create_task(
        args.title,
        params,
        today,
        &ctx.repo.repo_root.clone(),
        &mut *ctx.repo.store,
        &*ctx.repo.vcs,
    )?;
    ctx.repo.record_task_event("add", task.id);

    let short_id = task.id.to_string().replace('-', "")[..8].to_owned();
    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        println!("added [{short_id}] {}", task.title);
        tracing::info!(cmd = "add", "added [{}] {}", short_id, task.title);
        // stderr, so a `next add … | …` pipeline keeps carrying only the
        // confirmation line. Silent under --json for the same reason and under
        // --quiet because that is what --quiet means.
        if hint && !args.quiet {
            eprintln!("{NO_LEEWAY_HINT}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::domain::task::SnapLeeway;

    fn completion(snap: Option<Snap>, snap_leeway: Option<SnapLeeway>) -> Recurrence {
        Recurrence::Completion {
            interval_days: 30,
            snap,
            snap_leeway,
        }
    }

    #[test]
    fn a_day_of_month_snap_without_leeway_is_hinted() {
        let rule = completion(Some(Snap::DayOfMonth { day: 1 }), None);
        assert!(wants_leeway_hint(Some(&rule)));
    }

    #[test]
    fn a_weekday_snap_without_leeway_is_hinted() {
        let rule = completion(Some(Snap::NextWeekday { weekday: 0 }), None);
        assert!(wants_leeway_hint(Some(&rule)));
    }

    #[test]
    fn a_snap_that_already_has_a_leeway_is_not_hinted() {
        let rule = completion(
            Some(Snap::DayOfMonth { day: 1 }),
            Some(SnapLeeway {
                back: 3,
                forward: Some(3),
            }),
        );
        assert!(!wants_leeway_hint(Some(&rule)));
    }

    #[test]
    fn next_workday_is_not_hinted() {
        // At most a two-day push: a rounding, not a skipped cycle.
        let rule = completion(Some(Snap::NextWorkday), None);
        assert!(!wants_leeway_hint(Some(&rule)));
    }

    #[test]
    fn a_rule_without_a_snap_is_not_hinted() {
        assert!(!wants_leeway_hint(Some(&completion(None, None))));
        assert!(!wants_leeway_hint(None));
    }

    #[test]
    fn the_hint_names_the_flag_and_a_reachable_help_topic() {
        // `next help recurrence` does not exist; the pointer has to lead
        // somewhere a reader can actually go.
        assert!(NO_LEEWAY_HINT.contains("--recur-snap-leeway 3"));
        assert!(NO_LEEWAY_HINT.contains("next tutorial"));
    }
}
