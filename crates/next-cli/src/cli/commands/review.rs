use chrono::Local;
use next::domain::{filter, scoring, task::Stage};

use crate::cli::render;
use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {}

/// GTD weekly review: surface tasks in each stage that need attention.
pub fn run(_args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let today = Local::now().date_naive();
    let state = ctx.store.get_state()?;
    let all_tasks = ctx.store.list_tasks()?;

    for (label, stage) in &[
        ("── Inbox (needs processing) ──", Stage::Inbox),
        ("── Waiting (check in) ──", Stage::Waiting),
        ("── Someday/Maybe (revisit) ──", Stage::Someday),
    ] {
        let filter_set = next::domain::filter::FilterSet {
            stage: Some(stage.clone()),
            disable_implicit: true,
            ..Default::default()
        };
        let mut tasks = filter::apply(all_tasks.clone(), &filter_set, &state, today);
        tasks.retain(|t| t.status == next::domain::task::Status::Open);
        if tasks.is_empty() {
            continue;
        }
        println!("\n{label}");
        let scored = scoring::score_and_sort(tasks, &all_tasks, today, &ctx.config.scoring);
        render::render_task_list(&scored);
    }

    // Open projects with no open children (stalled).
    let stalled: Vec<_> = all_tasks
        .iter()
        .filter(|t| t.stage == Stage::Project && t.status == next::domain::task::Status::Open)
        .filter(|p| {
            !all_tasks
                .iter()
                .any(|c| c.parent_id == Some(p.id) && c.status == next::domain::task::Status::Open)
        })
        .collect();
    if !stalled.is_empty() {
        println!("\n── Stalled projects (no open subtasks) ──");
        for p in &stalled {
            let short = &p.id.to_string().replace('-', "")[..8];
            println!("  [{short}] {}", p.title);
        }
    }

    Ok(())
}
