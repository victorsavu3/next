//! Import + resolution reconcile between Forgejo issues and `next` tasks.
//!
//! `sync` runs the close-only reconcile for each configured mapping. The core
//! decision is the pure [`decide`] function, exhaustively unit-tested.

use std::collections::HashMap;

use anyhow::Result;

use crate::domain::task::Status;

use super::{
    config::Mapping,
    issues::{IssueSource, IssueState},
    tasks::{forgejo_link, TaskStore},
};

/// What to do for a single (issue, linked-task) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Open issue with no task → import it.
    CreateTask,
    /// Closed issue whose task is still active → resolve the task locally.
    MarkTaskDone,
    /// Resolved task whose issue is still open → close the issue.
    CloseIssue,
    /// Already consistent (or a closed issue we never imported).
    Nothing,
}

fn is_active(status: &Status) -> bool {
    matches!(status, Status::Open | Status::Started)
}

/// The close-only reconcile decision for one issue and its linked task status.
pub fn decide(issue: IssueState, task: Option<Status>) -> Action {
    match (issue, task) {
        (IssueState::Open, None) => Action::CreateTask,
        (IssueState::Closed, None) => Action::Nothing, // don't import already-closed issues
        (IssueState::Closed, Some(s)) if is_active(&s) => Action::MarkTaskDone,
        (IssueState::Open, Some(s)) if !is_active(&s) => Action::CloseIssue,
        _ => Action::Nothing,
    }
}

/// Counts of the actions taken (or that would be taken, in dry-run).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub created: usize,
    pub tasks_closed: usize,
    pub issues_closed: usize,
}

/// Reconciles every mapping. With `dry_run`, prints the planned actions and
/// mutates nothing.
pub fn sync(
    issues: &dyn IssueSource,
    tasks: &mut dyn TaskStore,
    mappings: &[Mapping],
    dry_run: bool,
) -> Result<Summary> {
    let mut summary = Summary::default();
    for mapping in mappings {
        let (owner, repo) = mapping.owner_repo()?;
        let repo_full = mapping.repo.clone();

        let issue_list = issues.list_issues(owner, repo)?;
        let linked = tasks.list_linked(&repo_full)?;
        let by_issue: HashMap<i64, &crate::domain::task::Task> = linked
            .iter()
            .filter_map(|t| forgejo_link(t).map(|(_, n)| (n, t)))
            .collect();

        for issue in &issue_list {
            let task = by_issue.get(&issue.number).copied();
            match decide(issue.state, task.map(|t| t.status.clone())) {
                Action::CreateTask => {
                    if dry_run {
                        println!("[dry-run] create task for {repo_full}#{} {:?}", issue.number, issue.title);
                    } else {
                        tasks.create_from_issue(issue, &mapping.context, &repo_full)?;
                    }
                    summary.created += 1;
                }
                Action::MarkTaskDone => {
                    let id = task.expect("task present for MarkTaskDone").id;
                    if dry_run {
                        println!("[dry-run] mark task {id} done ({repo_full}#{} closed)", issue.number);
                    } else {
                        tasks.mark_done(id)?;
                    }
                    summary.tasks_closed += 1;
                }
                Action::CloseIssue => {
                    if dry_run {
                        println!("[dry-run] close issue {repo_full}#{} (task resolved)", issue.number);
                    } else {
                        issues.set_closed(owner, repo, issue.number, true)?;
                    }
                    summary.issues_closed += 1;
                }
                Action::Nothing => {}
            }
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::domain::task::Task;
    use crate::forgejo::issues::ForgejoIssue;
    use crate::forgejo::keys;

    // ── decide matrix ──────────────────────────────────────────────────────────

    #[test]
    fn decide_matrix() {
        use Action::*;
        use IssueState::*;
        assert_eq!(decide(Open, None), CreateTask);
        assert_eq!(decide(Closed, None), Nothing);
        assert_eq!(decide(Closed, Some(Status::Open)), MarkTaskDone);
        assert_eq!(decide(Closed, Some(Status::Started)), MarkTaskDone);
        assert_eq!(decide(Closed, Some(Status::Done)), Nothing);
        assert_eq!(decide(Closed, Some(Status::Cancelled)), Nothing);
        assert_eq!(decide(Open, Some(Status::Done)), CloseIssue);
        assert_eq!(decide(Open, Some(Status::Cancelled)), CloseIssue);
        assert_eq!(decide(Open, Some(Status::Open)), Nothing);
        assert_eq!(decide(Open, Some(Status::Started)), Nothing);
    }

    // ── sync orchestration with fakes ───────────────────────────────────────────

    struct FakeIssues {
        issues: Vec<ForgejoIssue>,
        closed: RefCell<Vec<i64>>,
    }
    impl IssueSource for FakeIssues {
        fn list_issues(&self, _o: &str, _r: &str) -> Result<Vec<ForgejoIssue>> {
            Ok(self.issues.clone())
        }
        fn set_closed(&self, _o: &str, _r: &str, number: i64, closed: bool) -> Result<()> {
            assert!(closed);
            self.closed.borrow_mut().push(number);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeTasks {
        linked: Vec<Task>,
        created: RefCell<Vec<i64>>,
        done: RefCell<Vec<Uuid>>,
    }
    impl TaskStore for FakeTasks {
        fn list_linked(&self, _repo: &str) -> Result<Vec<Task>> {
            Ok(self.linked.clone())
        }
        fn create_from_issue(&mut self, issue: &ForgejoIssue, _ctx: &str, _repo: &str) -> Result<Uuid> {
            self.created.borrow_mut().push(issue.number);
            Ok(Uuid::new_v4())
        }
        fn mark_done(&mut self, id: Uuid) -> Result<()> {
            self.done.borrow_mut().push(id);
            Ok(())
        }
        fn show(&self, _id: Uuid) -> Result<Option<Task>> {
            Ok(None)
        }
    }

    fn issue(number: i64, state: IssueState) -> ForgejoIssue {
        ForgejoIssue {
            number,
            title: format!("issue {number}"),
            body: String::new(),
            state,
            html_url: format!("https://f/issues/{number}"),
            labels: vec![],
        }
    }

    fn linked_task(number: i64, status: Status) -> Task {
        let mut t = Task::new(format!("task {number}"));
        t.status = status;
        t.data.insert(keys::REPO.into(), json!("victor/x"));
        t.data.insert(keys::ISSUE.into(), json!(number));
        t
    }

    fn mapping() -> Vec<Mapping> {
        vec![Mapping { repo: "victor/x".into(), context: "@ai/x".into() }]
    }

    #[test]
    fn sync_imports_open_issue_without_task() {
        let issues = FakeIssues { issues: vec![issue(1, IssueState::Open)], closed: RefCell::default() };
        let mut tasks = FakeTasks::default();
        let s = sync(&issues, &mut tasks, &mapping(), false).unwrap();
        assert_eq!(s.created, 1);
        assert_eq!(*tasks.created.borrow(), vec![1]);
    }

    #[test]
    fn sync_closes_task_when_issue_closed() {
        let issues = FakeIssues { issues: vec![issue(7, IssueState::Closed)], closed: RefCell::default() };
        let mut tasks = FakeTasks { linked: vec![linked_task(7, Status::Open)], ..Default::default() };
        let s = sync(&issues, &mut tasks, &mapping(), false).unwrap();
        assert_eq!(s.tasks_closed, 1);
        assert_eq!(tasks.done.borrow().len(), 1);
    }

    #[test]
    fn sync_closes_issue_when_task_done() {
        let issues = FakeIssues { issues: vec![issue(9, IssueState::Open)], closed: RefCell::default() };
        let mut tasks = FakeTasks { linked: vec![linked_task(9, Status::Done)], ..Default::default() };
        let s = sync(&issues, &mut tasks, &mapping(), false).unwrap();
        assert_eq!(s.issues_closed, 1);
        assert_eq!(*issues.closed.borrow(), vec![9]);
    }

    #[test]
    fn sync_skips_closed_issue_without_task_and_is_consistent() {
        let issues = FakeIssues {
            issues: vec![issue(1, IssueState::Closed), issue(2, IssueState::Open)],
            closed: RefCell::default(),
        };
        // issue 2 already imported and still open → nothing.
        let mut tasks = FakeTasks { linked: vec![linked_task(2, Status::Open)], ..Default::default() };
        let s = sync(&issues, &mut tasks, &mapping(), false).unwrap();
        assert_eq!(s, Summary::default(), "no actions for closed-unimported or consistent pairs");
    }

    #[test]
    fn dry_run_mutates_nothing() {
        let issues = FakeIssues {
            issues: vec![issue(1, IssueState::Open), issue(2, IssueState::Open)],
            closed: RefCell::default(),
        };
        let mut tasks = FakeTasks { linked: vec![linked_task(2, Status::Done)], ..Default::default() };
        let s = sync(&issues, &mut tasks, &mapping(), true).unwrap();
        assert_eq!(s.created, 1);
        assert_eq!(s.issues_closed, 1);
        assert!(tasks.created.borrow().is_empty(), "dry-run creates nothing");
        assert!(issues.closed.borrow().is_empty(), "dry-run closes nothing");
    }
}
