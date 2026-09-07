use crate::core::domain::task::{Recurrence, Snap, SnapLeeway, Task};
use crate::core::recurrence::spawn_next;
use crate::core::scoring;
use chrono::{Local, NaiveDate};

use crate::{core::resolve::resolve_task_id, AppContext};

/// The recurrence block `show` prints: the rule, what it snaps to, and where
/// the next instance would land.
///
/// Returned rather than printed because the wording *is* the feature. `show`
/// used to destructure `snap` and `anchor` away, so a user could configure a
/// snap and never see it again — and a leeway is unverifiable under exactly
/// the same defect. Building the lines as values is what lets a test assert
/// them.
pub fn recurrence_lines(task: &Task, today: NaiveDate) -> Vec<String> {
    let Some(rec) = task.recurrence.as_ref() else {
        return Vec::new();
    };

    let mut lines = vec![match rec {
        // The anchor decides which dates the rule can produce at all, so a
        // schedule is not reproducible without it.
        Recurrence::Schedule { rrule, anchor, .. } => {
            format!("Recur:    schedule ({rrule}), anchor {anchor}")
        }
        Recurrence::Completion { interval_days, .. } => {
            format!("Recur:    {interval_days}d after completion")
        }
    }];

    lines.push(match rec.snap() {
        None => "Snap:     none".to_owned(),
        Some(snap) => format!(
            "Snap:     {}, {}",
            describe_snap(snap),
            describe_leeway(rec)
        ),
    });

    // `spawn_next` is what `done` actually runs, so this is the date the task
    // will get rather than a second calculation that could disagree with it.
    let next = spawn_next(task, today)
        .ok()
        .flatten()
        .and_then(|t| t.start.or(t.due));
    lines.push(match next {
        // Both arms assume completion today: for a schedule that only chooses
        // where the search starts, but saying so keeps the line honest about
        // resting on an assumption at all.
        Some(date) => format!("Next:     {date}  (if completed today)"),
        None => "Next:     none  (the rule is exhausted)".to_owned(),
    });

    lines
}

fn describe_snap(snap: &Snap) -> String {
    match snap {
        Snap::DayOfMonth { day } => format!("day {day} of month"),
        Snap::NextWorkday => "next workday".to_owned(),
        Snap::NextWeekday { weekday } => [
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ]
        .get(*weekday as usize)
        .copied()
        .unwrap_or("Monday")
        .to_owned(),
    }
}

/// How far the snap may move a date, spelled out — including when the answer
/// is "as far as it likes, forwards".
///
/// The parenthetical on the default is the whole point of the line. An absent
/// leeway is a policy, not an absence of one, and it is the policy that
/// lengthens a cycle every time the task is completed late.
fn describe_leeway(rec: &Recurrence) -> String {
    match rec.snap_leeway() {
        None => "no leeway (always moves later)".to_owned(),
        Some(SnapLeeway { back, forward }) => {
            let forward = match forward {
                Some(f) => format!("{f}d forward"),
                // Only reachable from a hand-written file: the spec parser
                // always sets both directions.
                None => "unbounded forward".to_owned(),
            };
            format!("leeway {back}d back / {forward}")
        }
    }
}

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task to show: UUID, UUID prefix, or slug.
    pub id: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    /// Return only these fields, e.g. `--fields id,title,due`. Requires
    /// `--json`. Applies to the task and its children alike.
    #[arg(long, value_delimiter = ',')]
    pub fields: Vec<String>,
}

pub fn run(args: Args, ctx: &AppContext) -> anyhow::Result<()> {
    crate::cli::commands::reject_fields_without_json(&args.fields, args.json)?;
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
    for line in recurrence_lines(&task, today) {
        println!("{line}");
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
