//! Integration tests for the Forgejo plugin's library-backed task store.
//! Gated on the `forgejo` feature (run with `--features forgejo`).
#![cfg(feature = "forgejo")]

mod common;

use next::core::domain::task::Status;
use next::core::plugin::registry;
use next::forgejo::issues::{ForgejoIssue, IssueState};
use next::forgejo::keys;
use next::forgejo::tasks::{forgejo_link, LibTaskStore, TaskStore};

fn issue() -> ForgejoIssue {
    ForgejoIssue {
        number: 42,
        title: "Fix the bug".into(),
        body: "steps to reproduce".into(),
        state: IssueState::Open,
        html_url: "https://forgejo.example.com/victor/x/issues/42".into(),
        labels: vec!["bug".into(), "p1".into()],
    }
}

#[test]
fn create_from_issue_links_and_watches() {
    let dir = tempfile::tempdir().unwrap();
    common::setup_in(dir.path());
    // `sync` self-registers; mirror that so per-task watch succeeds.
    registry::register(dir.path(), "next-forgejo", vec!["x".into(), "hook".into()]).unwrap();
    let mut store = LibTaskStore::open(Some(dir.path())).unwrap();

    let id = store
        .create_from_issue(&issue(), "@ai/x", "victor/x")
        .unwrap();

    let linked = store.list_linked("victor/x").unwrap();
    assert_eq!(linked.len(), 1);
    let task = &linked[0];
    assert_eq!(task.title, "Fix the bug");
    assert_eq!(task.description.as_deref(), Some("steps to reproduce"));
    assert!(
        task.tags.contains(&"@ai/x".to_string()),
        "mapped context tag applied"
    );
    assert_eq!(
        task.url.as_deref(),
        Some("https://forgejo.example.com/victor/x/issues/42")
    );
    assert_eq!(forgejo_link(task), Some(("victor/x".to_owned(), 42)));
    assert_eq!(
        task.data.get(keys::LABELS).unwrap(),
        &serde_json::json!(["bug", "p1"]),
        "labels stored as a data attribute, not local tags"
    );
    assert!(
        !task.tags.iter().any(|t| t == "bug" || t == "p1"),
        "labels must not become local tags"
    );

    // The plugin subscribed to the new task so local resolution notifies it.
    let reg = registry::load(dir.path()).unwrap();
    assert!(
        reg.plugins
            .iter()
            .any(|p| p.name == "next-forgejo" && p.tasks.contains(&id)),
        "new task is watched"
    );
}

#[test]
fn mark_done_resolves_the_task() {
    let dir = tempfile::tempdir().unwrap();
    common::setup_in(dir.path());
    // `sync` self-registers; mirror that so per-task watch succeeds.
    registry::register(dir.path(), "next-forgejo", vec!["x".into(), "hook".into()]).unwrap();
    let mut store = LibTaskStore::open(Some(dir.path())).unwrap();
    let id = store
        .create_from_issue(&issue(), "@ai/x", "victor/x")
        .unwrap();

    store.mark_done(id).unwrap();

    let task = store.show(id).unwrap().expect("task exists");
    assert_eq!(task.status, Status::Done);
}

#[test]
fn list_linked_filters_by_repo() {
    let dir = tempfile::tempdir().unwrap();
    common::setup_in(dir.path());
    // `sync` self-registers; mirror that so per-task watch succeeds.
    registry::register(dir.path(), "next-forgejo", vec!["x".into(), "hook".into()]).unwrap();
    let mut store = LibTaskStore::open(Some(dir.path())).unwrap();
    store
        .create_from_issue(&issue(), "@ai/x", "victor/x")
        .unwrap();

    assert_eq!(store.list_linked("victor/x").unwrap().len(), 1);
    assert!(store.list_linked("other/repo").unwrap().is_empty());
}
