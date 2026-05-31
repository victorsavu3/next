use std::collections::{HashMap, HashSet};

use crate::domain::task::{Status, Task};
use uuid::Uuid;

use crate::AppContext;

/// Top-level `next tree` subcommand.
#[derive(clap::Args, Debug)]
pub struct Args {
    /// Include done and cancelled tasks.
    #[arg(long)]
    pub all: bool,

    /// Output as JSON (flat list with parent_id fields).
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &AppContext) -> anyhow::Result<()> {
    let all_tasks = ctx.store().list_tasks()?;

    if args.json {
        let tasks: Vec<&Task> = if args.all {
            all_tasks.iter().collect()
        } else {
            all_tasks
                .iter()
                .filter(|t| t.is_active())
                .collect()
        };
        println!("{}", serde_json::to_string_pretty(&tasks)?);
        return Ok(());
    }

    let visible_ids: HashSet<Uuid> = if args.all {
        all_tasks.iter().map(|t| t.id).collect()
    } else {
        all_tasks
            .iter()
            .filter(|t| t.is_active())
            .map(|t| t.id)
            .collect()
    };

    // Children map: parent_id -> sorted list of child tasks (visible only).
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
        println!("No tasks.");
        return Ok(());
    }

    for root in roots {
        print_node(root, &children, 0);
    }

    Ok(())
}

fn print_node(task: &Task, children: &HashMap<Uuid, Vec<&Task>>, depth: usize) {
    let indent = "  ".repeat(depth);
    let short = &task.id.to_string().replace('-', "")[..8];
    let status = match task.status {
        Status::Open => "○",
        Status::Started => "▶",
        Status::Done => "✓",
        Status::Cancelled => "✗",
    };
    println!("{indent}{status} [{short}] {}", task.title);

    if let Some(kids) = children.get(&task.id) {
        let mut sorted: Vec<&Task> = kids.to_vec();
        sorted.sort_by_key(|t| &t.title);
        for child in sorted {
            print_node(child, children, depth + 1);
        }
    }
}
