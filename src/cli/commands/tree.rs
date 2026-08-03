use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Write};

use chrono::Local;

use crate::core::domain::filter::{self, FilterSet};
use crate::core::domain::tag;
use crate::core::domain::task::{Status, Task};
use uuid::Uuid;

use crate::AppContext;

/// Top-level `next tree` subcommand.
#[derive(clap::Args, Debug)]
pub struct Args {
    /// Include all tasks regardless of status or filters.
    #[arg(long)]
    pub all: bool,

    /// Show only done and cancelled tasks, respecting active context and filters.
    #[arg(long)]
    pub closed: bool,

    /// Output as JSON (flat list with parent_id fields).
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &AppContext) -> anyhow::Result<()> {
    run_with_writer(args, ctx, &mut io::stdout())
}

pub fn run_with_writer(args: Args, ctx: &AppContext, out: &mut dyn Write) -> anyhow::Result<()> {
    let all_tasks = ctx.repo.store().list_tasks()?;

    let closed_tasks: Option<Vec<Task>> = if args.closed {
        let state = ctx.repo.store().get_state()?;
        let today = Local::now().date_naive();
        let filter_set = FilterSet {
            closed_only: true,
            include_blocked_parents: true,
            ..Default::default()
        };
        Some(filter::apply(all_tasks.clone(), &filter_set, &state, today))
    } else {
        None
    };

    if args.json {
        let tasks: Vec<&Task> = if args.all {
            all_tasks.iter().collect()
        } else if let Some(ref closed) = closed_tasks {
            closed.iter().collect()
        } else {
            all_tasks.iter().filter(|t| t.is_active()).collect()
        };
        writeln!(out, "{}", serde_json::to_string_pretty(&tasks)?)?;
        return Ok(());
    }

    let visible_ids: HashSet<Uuid> = if args.all {
        all_tasks.iter().map(|t| t.id).collect()
    } else if let Some(closed) = closed_tasks {
        closed.into_iter().map(|t| t.id).collect()
    } else {
        all_tasks
            .iter()
            .filter(|t| t.is_active())
            .map(|t| t.id)
            .collect()
    };

    // Children map: parent_id -> child tasks (visible only).
    let mut children: HashMap<Uuid, Vec<&Task>> = HashMap::new();
    for task in all_tasks.iter().filter(|t| visible_ids.contains(&t.id)) {
        if let Some(pid) = task.parent_id {
            children.entry(pid).or_default().push(task);
        }
    }

    // Root tasks: visible tasks whose parent is absent or not visible.
    let mut roots: Vec<&Task> = all_tasks
        .iter()
        .filter(|t| {
            visible_ids.contains(&t.id)
                && t.parent_id
                    .map(|pid| !visible_ids.contains(&pid))
                    .unwrap_or(true)
        })
        .collect();
    roots.sort_by_key(|t| &t.title);

    if roots.is_empty() {
        writeln!(out, "No tasks.")?;
        return Ok(());
    }

    // -----------------------------------------------------------------------
    // Context grouping
    // -----------------------------------------------------------------------
    //
    // For each root task, find the "most specific" context tag(s) (deepest in
    // the hierarchy).  Children always follow their parent's context section.
    //
    // A task may have multiple context tags at the same depth — in that case it
    // is placed in *all* of those sections; but children are only printed under
    // the first (alphabetically earliest) primary section.

    // sections: context label -> list of (root_task, is_duplicate).
    // BTreeMap gives sorted iteration; "No context" is handled separately.
    let mut sections: BTreeMap<String, Vec<(&Task, bool)>> = BTreeMap::new();
    // Records which context owns the canonical (first) printing of each root.
    let mut primary_section: HashMap<Uuid, String> = HashMap::new();

    for root in &roots {
        let ctx_tags = tag::deepest_context_tags(&root.tags);

        if ctx_tags.is_empty() {
            // No context — deferred to trailing "No context" section.
            let entry = sections.entry("No context".to_string()).or_default();
            let is_dup = primary_section.contains_key(&root.id);
            if !is_dup {
                primary_section.insert(root.id, "No context".to_string());
            }
            entry.push((root, is_dup));
        } else {
            // Sort tags for deterministic ordering (alphabetical).
            let mut sorted_tags = ctx_tags;
            sorted_tags.sort();
            for (i, ctx_tag) in sorted_tags.iter().enumerate() {
                let entry = sections.entry(ctx_tag.clone()).or_default();
                let is_dup = i > 0 || primary_section.contains_key(&root.id);
                if !is_dup {
                    primary_section.insert(root.id, ctx_tag.clone());
                }
                entry.push((root, is_dup));
            }
        }
    }

    // Move "No context" to the end: extract before iterating named sections.
    let no_ctx = sections.remove("No context");

    // Print named context sections (alphabetical via BTreeMap).
    for (section_name, tasks) in &sections {
        writeln!(out, "── {section_name} ──")?;
        for (task, is_dup) in tasks {
            let also_note = if *is_dup {
                let primary = primary_section
                    .get(&task.id)
                    .map(|s| s.as_str())
                    .unwrap_or("");
                format!("  (also in {primary})")
            } else {
                String::new()
            };
            print_node_with_note(task, &children, 0, &also_note, !is_dup, out)?;
        }
        writeln!(out)?;
    }

    // Print "No context" last.
    if let Some(tasks) = no_ctx {
        writeln!(out, "── No context ──")?;
        for (task, _is_dup) in tasks {
            print_node(task, &children, 0, out)?;
        }
        writeln!(out)?;
    }

    Ok(())
}

fn print_node(
    task: &Task,
    children: &HashMap<Uuid, Vec<&Task>>,
    depth: usize,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    print_node_with_note(task, children, depth, "", true, out)
}

/// Print a task node, optionally with a trailing note and optional child recursion.
///
/// * `note`           — appended after the title (e.g. `"  (also in @work)"`)
/// * `print_children` — when `false`, skip recursive child printing (used for
///   duplicate entries in secondary context sections).
fn print_node_with_note(
    task: &Task,
    children: &HashMap<Uuid, Vec<&Task>>,
    depth: usize,
    note: &str,
    print_children: bool,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let indent = "  ".repeat(depth);
    let short = &task.id.to_string().replace('-', "")[..8];
    let status = match task.status {
        Status::Open => "○",
        Status::Started => "▶",
        Status::Done => "✓",
        Status::Cancelled => "✗",
    };
    writeln!(out, "{indent}{status} [{short}] {}{note}", task.title)?;

    if print_children {
        if let Some(kids) = children.get(&task.id) {
            let mut sorted: Vec<&Task> = kids.to_vec();
            sorted.sort_by_key(|t| &t.title);
            for child in sorted {
                print_node(child, children, depth + 1, out)?;
            }
        }
    }

    Ok(())
}
