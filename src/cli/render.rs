use crate::core::scoring::ScoredTask;

/// Prints a human-readable table of scored tasks to stdout.
/// Prints the pagination footer for a windowed result, e.g.
/// `page 2 of 14 · 13402 matching · --page 3 for more`.
/// No output when the page already holds the entire result.
pub fn render_page_footer<T>(page: &crate::core::Page<T>) {
    if !page.is_paginated() {
        return;
    }
    let pages = page.page_count();
    let mut footer = format!("page {} of {} · {} matching", page.page, pages, page.total);
    if (page.page as u64) < pages {
        footer.push_str(&format!(" · --page {} for more", page.page + 1));
    }
    println!("{footer}");
}

pub fn render_task_list(tasks: &[ScoredTask]) {
    if tasks.is_empty() {
        println!("No tasks.");
        return;
    }

    for st in tasks {
        let task = &st.task;

        let short_id = {
            let hex = task.id.to_string().replace('-', "");
            hex[..8].to_owned()
        };

        let title = truncate(&task.title, 45);

        let due = task
            .due
            .map(|d| format!("due:{d}"))
            .unwrap_or_default();

        let tags = if task.tags.is_empty() {
            String::new()
        } else {
            task.tags.join(" ")
        };

        let assignee = task.assignee.as_deref().unwrap_or("").to_owned();
        let score = format!("{:.1}", st.score);

        println!(
            "{short_id}  {title:<45}  {score:>5}  {due:<14}  {assignee:<12}  {tags}",
        );
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}
