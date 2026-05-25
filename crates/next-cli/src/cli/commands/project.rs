use next::domain::task::{Priority, Status, Task};

use crate::{resolve::resolve_task_id, AppContext};

/// Tag used to identify project tasks.
const PROJECT_TAG: &str = "project";

/// Top-level `next project` subcommand.
/// A project is any task tagged `project`. Subtasks attach via `--parent`.
#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: ProjectSubcommand,
}

#[derive(clap::Subcommand, Debug)]
pub enum ProjectSubcommand {
    /// List all open project tasks in a tree view.
    List(ListArgs),
    /// Create a new project task (shorthand for `next add --tag project`).
    Add(AddArgs),
    /// Show a project and all its descendants.
    Show(ShowArgs),
}

#[derive(clap::Args, Debug)]
pub struct ListArgs {
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args, Debug)]
pub struct AddArgs {
    /// Project title.
    pub title: String,

    /// User-provided slug for stable referencing.
    #[arg(long)]
    pub slug: Option<String>,

    /// Project priority (low, medium, high).
    #[arg(long)]
    pub priority: Option<String>,

    /// Free-text notes (serves as project description).
    #[arg(long)]
    pub notes: Option<String>,

    /// Parent task: UUID, UUID prefix, or slug.
    #[arg(long)]
    pub parent: Option<String>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args, Debug)]
pub struct ShowArgs {
    /// Project task: UUID, UUID prefix, or slug.
    pub id: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        ProjectSubcommand::List(a) => list(ctx, a.json),
        ProjectSubcommand::Add(a) => add(ctx, a),
        ProjectSubcommand::Show(a) => show(ctx, &a.id, a.json),
    }
}

fn list(ctx: &mut AppContext, json: bool) -> anyhow::Result<()> {
    let all_tasks = ctx.store.list_tasks()?;
    let projects: Vec<&Task> = all_tasks
        .iter()
        .filter(|t| t.tags.iter().any(|tag| tag == PROJECT_TAG) && t.status == Status::Open)
        .collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&projects)?);
        return Ok(());
    }

    if projects.is_empty() {
        println!("No open projects.");
        return Ok(());
    }

    for project in &projects {
        let short = &project.id.to_string().replace('-', "")[..8];
        let open_children = all_tasks
            .iter()
            .filter(|t| t.parent_id == Some(project.id) && t.status == Status::Open)
            .count();
        println!("[{short}] {}  ({open_children} open)", project.title);
        for child in all_tasks
            .iter()
            .filter(|t| t.parent_id == Some(project.id) && t.status == Status::Open)
        {
            let cshort = &child.id.to_string().replace('-', "")[..8];
            println!("  [{cshort}] {}", child.title);
        }
    }
    Ok(())
}

fn add(ctx: &mut AppContext, args: AddArgs) -> anyhow::Result<()> {
    let mut task = Task::new(args.title);
    task.tags.push(PROJECT_TAG.to_string());
    task.slug = args.slug;
    task.notes = args.notes;

    if let Some(p) = args.priority {
        task.priority = parse_priority(&p)?;
    }
    if let Some(ref parent_ref) = args.parent {
        task.parent_id = Some(resolve_task_id(&*ctx.store, parent_ref)?);
    }

    let short_id = task.id.to_string().replace('-', "")[..8].to_owned();
    let task_path = next_storage::task_path(&ctx.repo_root, &task);
    ctx.store.save_task(&task)?;
    ctx.vcs
        .commit(&[task_path], &format!("next: add project {}", task.title))?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        ctx.log
            .info("project", &format!("add [{short_id}] {}", task.title));
    }
    Ok(())
}

fn show(ctx: &mut AppContext, id_str: &str, json: bool) -> anyhow::Result<()> {
    let id = resolve_task_id(&*ctx.store, id_str)?;
    let all_tasks = ctx.store.list_tasks()?;

    let root = all_tasks
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| anyhow::anyhow!("task not found: {id_str}"))?;

    if json {
        let tree = collect_descendants(root.id, &all_tasks);
        println!("{}", serde_json::to_string_pretty(&tree)?);
        return Ok(());
    }

    print_tree(root, &all_tasks, 0);
    Ok(())
}

fn collect_descendants(root_id: uuid::Uuid, all: &[Task]) -> Vec<&Task> {
    let mut result = Vec::new();
    let mut stack = vec![root_id];
    while let Some(id) = stack.pop() {
        if let Some(t) = all.iter().find(|t| t.id == id) {
            result.push(t);
            for child in all.iter().filter(|t| t.parent_id == Some(id)) {
                stack.push(child.id);
            }
        }
    }
    result
}

fn print_tree(task: &Task, all: &[Task], depth: usize) {
    let indent = "  ".repeat(depth);
    let short = &task.id.to_string().replace('-', "")[..8];
    let status = match task.status {
        Status::Open => "○",
        Status::Done => "✓",
        Status::Cancelled => "✗",
    };
    println!("{indent}{status} [{short}] {}", task.title);
    for child in all.iter().filter(|t| t.parent_id == Some(task.id)) {
        print_tree(child, all, depth + 1);
    }
}

fn parse_priority(s: &str) -> anyhow::Result<Priority> {
    match s.to_lowercase().as_str() {
        "low" => Ok(Priority::Low),
        "medium" | "med" => Ok(Priority::Medium),
        "high" => Ok(Priority::High),
        _ => anyhow::bail!("unknown priority {s:?} — expected low, medium, or high"),
    }
}
