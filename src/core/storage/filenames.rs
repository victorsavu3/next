use std::path::{Path, PathBuf};

use crate::core::domain::task::Task;

/// Converts a task title to a URL-safe slug component:
/// lowercase, runs of non-alphanumeric chars collapsed to a single `-`.
pub fn title_to_slug(title: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = true;
    for c in title.chars() {
        if c.is_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    slug.trim_end_matches('-').to_owned()
}

/// Returns the canonical filename (no directory prefix) for `task`.
///
/// - Tasks with an explicit slug use `<slug>.toml`.
/// - Tasks without a slug use `<title-slug>-<uuid8>.toml`, where `<uuid8>` is
///   the first 8 hex digits of the UUID (dashes stripped).
pub fn generate_filename(task: &Task) -> String {
    if let Some(ref slug) = task.slug {
        format!("{slug}.toml")
    } else {
        let title_part = title_to_slug(&task.title);
        let uuid_hex = task.id.to_string().replace('-', "");
        format!("{title_part}-{}.toml", &uuid_hex[..8])
    }
}

/// Returns the full path where `task` is (or will be) stored under `repo_root`.
pub fn task_path(repo_root: &Path, task: &Task) -> PathBuf {
    repo_root.join("tasks").join(generate_filename(task))
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use crate::core::domain::task::Task;

    use super::*;

    // ------------------------------------------------------------------
    // title_to_slug
    // ------------------------------------------------------------------

    #[test]
    fn slug_basic_words() {
        assert_eq!(title_to_slug("Buy milk"), "buy-milk");
    }

    #[test]
    fn slug_leading_spaces() {
        assert_eq!(title_to_slug("  leading spaces"), "leading-spaces");
    }

    #[test]
    fn slug_punctuation() {
        assert_eq!(title_to_slug("Hello, World!"), "hello-world");
    }

    #[test]
    fn slug_alphanumeric_only() {
        assert_eq!(title_to_slug("abc123"), "abc123");
    }

    #[test]
    fn slug_consecutive_separators_collapsed() {
        assert_eq!(title_to_slug("a--b"), "a-b");
    }

    #[test]
    fn slug_trailing_punctuation_stripped() {
        assert_eq!(title_to_slug("hello!"), "hello");
    }

    #[test]
    fn slug_empty_string() {
        assert_eq!(title_to_slug(""), "");
    }

    #[test]
    fn slug_only_punctuation() {
        assert_eq!(title_to_slug("!!!"), "");
    }

    // ------------------------------------------------------------------
    // generate_filename
    // ------------------------------------------------------------------

    #[test]
    fn filename_with_explicit_slug() {
        let mut task = Task::new("Some title");
        task.slug = Some("my-custom-slug".into());
        assert_eq!(generate_filename(&task), "my-custom-slug.toml");
    }

    #[test]
    fn filename_without_slug_uses_title_and_uuid() {
        let mut task = Task::new("Buy groceries");
        task.slug = None;
        let name = generate_filename(&task);
        // Must start with the title slug.
        assert!(
            name.starts_with("buy-groceries-"),
            "unexpected filename: {name}"
        );
        // Must end with .toml.
        assert!(name.ends_with(".toml"), "unexpected filename: {name}");
        // The UUID suffix is 8 hex chars between the last '-' and '.toml'.
        let stem = name.strip_suffix(".toml").unwrap();
        let suffix = stem.rsplit('-').next().unwrap();
        assert_eq!(
            suffix.len(),
            8,
            "uuid suffix should be 8 chars, got: {suffix}"
        );
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "suffix not hex: {suffix}"
        );
    }

    #[test]
    fn filename_without_slug_embeds_correct_uuid() {
        let task = Task::new("Fix bug");
        let name = generate_filename(&task);
        let expected_uuid8 = &task.id.to_string().replace('-', "")[..8];
        assert!(
            name.contains(expected_uuid8),
            "filename {name} should contain uuid8 {expected_uuid8}"
        );
    }

    #[test]
    fn filename_two_tasks_same_title_differ_by_uuid() {
        let t1 = Task::new("Duplicate title");
        let t2 = Task::new("Duplicate title");
        assert_ne!(t1.id, t2.id);
        let n1 = generate_filename(&t1);
        let n2 = generate_filename(&t2);
        assert_ne!(
            n1, n2,
            "two tasks with same title should get different filenames"
        );
    }

    #[test]
    fn filename_title_with_special_chars() {
        let task = Task::new("Fix: crash in parser!");
        let name = generate_filename(&task);
        assert!(
            name.starts_with("fix-crash-in-parser-"),
            "unexpected filename: {name}"
        );
    }

    // ------------------------------------------------------------------
    // task_path
    // ------------------------------------------------------------------

    #[test]
    fn task_path_under_repo_root() {
        let mut task = Task::new("My task");
        task.slug = Some("my-task".into());
        let root = std::path::Path::new("/repo");
        let path = task_path(root, &task);
        assert_eq!(path, std::path::PathBuf::from("/repo/tasks/my-task.toml"));
    }

    #[test]
    fn task_path_no_slug_includes_tasks_dir() {
        let task = Task::new("No slug task");
        let root = std::path::Path::new("/data/tasks-repo");
        let path = task_path(root, &task);
        let parent = path.parent().unwrap();
        assert_eq!(parent, std::path::Path::new("/data/tasks-repo/tasks"));
    }

    #[test]
    fn task_path_deterministic_for_same_task() {
        let task = Task::new("Stable");
        let root = std::path::Path::new("/r");
        assert_eq!(task_path(root, &task), task_path(root, &task));
    }

    #[test]
    fn task_path_different_roots() {
        let mut task = Task::new("T");
        task.slug = Some("t".into());
        let p1 = task_path(std::path::Path::new("/root1"), &task);
        let p2 = task_path(std::path::Path::new("/root2"), &task);
        assert_ne!(p1, p2);
    }

    // ------------------------------------------------------------------
    // Edge-case: known UUID value
    // ------------------------------------------------------------------

    #[test]
    fn filename_known_uuid_produces_expected_suffix() {
        use std::str::FromStr;
        let mut task = Task::new("Test");
        task.id = Uuid::from_str("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap();
        let name = generate_filename(&task);
        // UUID hex without dashes: "aaaaaaaabbbbccccddddeeeeeeeeeeee" → first 8 = "aaaaaaaa"
        assert_eq!(name, "test-aaaaaaaa.toml");
    }
}
