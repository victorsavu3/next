//! Forgejo issue access.
//!
//! [`IssueSource`] is the (synchronous) interface the reconcile logic uses; the
//! real [`ForgejoApi`] implementation wraps the async `forgejo-api` crate,
//! owning a small Tokio runtime so the rest of the plugin stays synchronous and
//! testable with an in-memory fake.

use anyhow::{Context, Result};

/// Whether a Forgejo issue is open or closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueState {
    Open,
    Closed,
}

/// A Forgejo issue, decoupled from `forgejo-api`'s wire types.
#[derive(Debug, Clone)]
pub struct ForgejoIssue {
    pub number: i64,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    pub html_url: String,
    pub labels: Vec<String>,
}

/// Read/close access to Forgejo issues. Implemented by [`ForgejoApi`] in
/// production and by an in-memory fake in tests.
pub trait IssueSource {
    /// Lists all issues (open and closed, excluding pull requests) in `owner/repo`.
    fn list_issues(&self, owner: &str, repo: &str) -> Result<Vec<ForgejoIssue>>;
    /// Closes (`closed = true`) or reopens (`false`) the given issue.
    fn set_closed(&self, owner: &str, repo: &str, number: i64, closed: bool) -> Result<()>;
}

/// Real implementation backed by the `forgejo-api` crate.
pub struct ForgejoApi {
    runtime: tokio::runtime::Runtime,
    client: forgejo_api::Forgejo,
}

impl ForgejoApi {
    /// Builds a client from a base URL and API token.
    pub fn new(base_url: &str, token: &str) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("build tokio runtime")?;
        let url = base_url
            .parse::<url::Url>()
            .with_context(|| format!("invalid forgejo_url {base_url:?}"))?;
        let client = forgejo_api::Forgejo::new(forgejo_api::Auth::Token(token), url)
            .context("create Forgejo client")?;
        Ok(Self { runtime, client })
    }
}

impl IssueSource for ForgejoApi {
    fn list_issues(&self, owner: &str, repo: &str) -> Result<Vec<ForgejoIssue>> {
        use forgejo_api::structs::{
            IssueListIssuesQuery, IssueListIssuesQueryState, IssueListIssuesQueryType,
        };

        // NOTE: forgejo-api 0.10's typed `IssueListIssuesQuery` exposes no
        // page/limit, so this returns the server's first/default page only.
        // Pagination is a follow-up; adequate for typical repos.
        self.runtime.block_on(async {
            let query = IssueListIssuesQuery {
                state: Some(IssueListIssuesQueryState::All),
                r#type: Some(IssueListIssuesQueryType::Issues),
                ..Default::default()
            };
            let (_, issues) = self
                .client
                .issue_list_issues(owner, repo, query)
                .await
                .with_context(|| format!("list issues for {owner}/{repo}"))?;
            Ok(issues.into_iter().map(convert_issue).collect())
        })
    }

    fn set_closed(&self, owner: &str, repo: &str, number: i64, closed: bool) -> Result<()> {
        use forgejo_api::structs::EditIssueOption;

        let state = if closed { "closed" } else { "open" };
        self.runtime.block_on(async {
            self.client
                .issue_edit_issue(
                    owner,
                    repo,
                    number,
                    EditIssueOption {
                        state: Some(state.to_owned()),
                        title: None,
                        body: None,
                        assignee: None,
                        assignees: None,
                        due_date: None,
                        milestone: None,
                        r#ref: None,
                        unset_due_date: None,
                        updated_at: None,
                    },
                )
                .await
                .with_context(|| format!("set {owner}/{repo}#{number} state={state}"))?;
            Ok(())
        })
    }
}

/// Converts a `forgejo-api` issue into our decoupled [`ForgejoIssue`].
fn convert_issue(issue: forgejo_api::structs::Issue) -> ForgejoIssue {
    // `state` is an enum; its Debug rendering is "Open"/"Closed".
    let state = match issue
        .state
        .as_ref()
        .map(|s| format!("{s:?}").to_lowercase())
    {
        Some(ref s) if s == "closed" => IssueState::Closed,
        _ => IssueState::Open,
    };
    let labels = issue
        .labels
        .into_iter()
        .flatten()
        .filter_map(|l| l.name)
        .collect();
    ForgejoIssue {
        number: issue.number.unwrap_or(0),
        title: issue.title.unwrap_or_default(),
        body: issue.body.unwrap_or_default(),
        state,
        html_url: issue.html_url.map(|u| u.to_string()).unwrap_or_default(),
        labels,
    }
}
