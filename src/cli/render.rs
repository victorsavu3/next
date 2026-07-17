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

/// Truncates `s` to at most `max` characters (never byte indices, so
/// multibyte text is safe), appending `…` when truncation occurs. The
/// ellipsis counts towards `max`, so the result never exceeds `max` chars.
pub fn truncate(s: &str, max: usize) -> std::borrow::Cow<'_, str> {
    if s.chars().count() <= max {
        return std::borrow::Cow::Borrowed(s);
    }
    let keep = max.saturating_sub(1);
    let end = s
        .char_indices()
        .nth(keep)
        .map(|(byte_pos, _)| byte_pos)
        .unwrap_or(0);
    std::borrow::Cow::Owned(format!("{}…", &s[..end]))
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn short_ascii_unchanged() {
        assert_eq!(truncate("hello", 45), "hello");
    }

    #[test]
    fn exact_length_unchanged() {
        let s = "a".repeat(45);
        assert_eq!(truncate(&s, 45), s);
    }

    #[test]
    fn long_ascii_truncated_with_ellipsis() {
        let s = "a".repeat(60);
        let out = truncate(&s, 45);
        assert_eq!(out.chars().count(), 45);
        assert_eq!(out, format!("{}…", "a".repeat(44)));
    }

    #[test]
    fn multibyte_boundary_does_not_panic() {
        // 'é' is 2 bytes; byte index 45 falls mid-character, which the old
        // byte-slicing implementation panicked on.
        let s = "é".repeat(60);
        let out = truncate(&s, 45);
        assert_eq!(out, format!("{}…", "é".repeat(44)));
    }

    #[test]
    fn emoji_truncated_on_char_boundary() {
        let s = "🎉".repeat(50);
        let out = truncate(&s, 45);
        assert_eq!(out.chars().count(), 45);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn multibyte_within_limit_unchanged() {
        let s = "café ☕";
        assert_eq!(truncate(s, 45), s);
    }
}
