use std::path::{Path, PathBuf};

use anyhow::Context as _;
use git2::build::RepoBuilder;

use super::config::McpConfig;

/// Ensures the tasks git repository is present at `config.repo_path`.
///
/// * If the directory already contains a `.git` folder: opens it and returns the path.
/// * Otherwise: clones from `config.git_url` (required if repo absent).
///
/// After a fresh clone, initialises the `tasks/` directory and `.gitignore`
/// (idempotent — same as `next init`).
pub fn clone_or_open(config: &McpConfig) -> anyhow::Result<PathBuf> {
    let repo_path = &config.repo_path;

    if repo_path.join(".git").exists() {
        return Ok(repo_path.clone());
    }

    let git_url = config.git_url.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "repository not found at {} and NEXT_GIT_URL is not set",
            repo_path.display()
        )
    })?;

    eprintln!("Cloning {} → {}", git_url, repo_path.display());

    let mut callbacks = git2::RemoteCallbacks::new();
    let (url_for_auth, user_from_url, token_from_url) = extract_credentials(git_url);

    // Resolve credentials: explicit env vars take precedence over URL-embedded ones.
    let git_user: Option<String> = config.git_user.clone().or(user_from_url);
    let git_token: Option<String> = config.git_token.clone().or(token_from_url);

    let mut tried = false;
    callbacks.credentials(move |_url, _username, allowed| {
        if tried {
            return Err(git2::Error::from_str("authentication failed"));
        }
        tried = true;
        if allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
            if let (Some(u), Some(t)) = (git_user.as_deref(), git_token.as_deref()) {
                return git2::Cred::userpass_plaintext(u, t);
            }
        }
        Err(git2::Error::from_str("no credentials available"))
    });

    let mut fetch_opts = git2::FetchOptions::new();
    fetch_opts.remote_callbacks(callbacks);

    let repo = RepoBuilder::new()
        .fetch_options(fetch_opts)
        .clone(&url_for_auth, repo_path)
        .with_context(|| format!("git clone {git_url} failed"))?;

    ensure_git_identity(&repo)?;
    init_repo_structure(repo_path)?;
    Ok(repo_path.clone())
}

/// Splits `https://user:token@host/repo.git` into the bare URL and credentials.
fn extract_credentials(url: &str) -> (String, Option<String>, Option<String>) {
    // Only handle http(s) URLs
    let rest = if let Some(r) = url.strip_prefix("https://") {
        r
    } else if let Some(r) = url.strip_prefix("http://") {
        r
    } else {
        return (url.to_owned(), None, None);
    };

    if let Some(at_pos) = rest.find('@') {
        let credentials = &rest[..at_pos];
        let host_and_path = &rest[at_pos + 1..];
        let prefix = if url.starts_with("https") { "https" } else { "http" };
        let clean_url = format!("{prefix}://{host_and_path}");

        if let Some(colon) = credentials.find(':') {
            let user = credentials[..colon].to_owned();
            let token = credentials[colon + 1..].to_owned();
            return (clean_url, Some(user), Some(token));
        }
        return (clean_url, Some(credentials.to_owned()), None);
    }

    (url.to_owned(), None, None)
}

/// Ensures `user.name` and `user.email` are set in the repo's local config,
/// falling back to sensible defaults when neither global nor system config provides them.
/// This is required for `next-mcp` to commit inside a container with no git identity.
fn ensure_git_identity(repo: &git2::Repository) -> anyhow::Result<()> {
    let mut config = repo.config()
        .with_context(|| "failed to open git config")?;
    let local = config.open_level(git2::ConfigLevel::Local)
        .with_context(|| "failed to open local git config")?;

    // Check if user.name is already set at any level.
    let has_name = config.get_string("user.name").is_ok();
    let has_email = config.get_string("user.email").is_ok();

    if !has_name {
        config.set_str("user.name", "next-mcp")?;
    }
    if !has_email {
        config.set_str("user.email", "next-mcp@localhost")?;
    }

    drop(local);
    Ok(())
}

/// Creates `tasks/` and `.gitignore` inside a freshly-cloned repo.
fn init_repo_structure(repo_path: &Path) -> anyhow::Result<()> {
    let tasks_dir = repo_path.join("tasks");
    if !tasks_dir.exists() {
        std::fs::create_dir_all(&tasks_dir)
            .with_context(|| format!("create {}", tasks_dir.display()))?;
    }

    let gitignore = repo_path.join(".gitignore");
    let entry = ".next.db\n";
    if gitignore.exists() {
        let content = std::fs::read_to_string(&gitignore)?;
        if !content.lines().any(|l| l.trim() == ".next.db") {
            let mut appended = content;
            if !appended.ends_with('\n') {
                appended.push('\n');
            }
            appended.push_str(entry);
            std::fs::write(&gitignore, appended)?;
        }
    } else {
        std::fs::write(&gitignore, entry)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_user_token_from_url() {
        let (url, user, token) = extract_credentials("https://alice:secret@git.example.com/repo.git");
        assert_eq!(url, "https://git.example.com/repo.git");
        assert_eq!(user.as_deref(), Some("alice"));
        assert_eq!(token.as_deref(), Some("secret"));
    }

    #[test]
    fn extract_no_credentials() {
        let (url, user, token) = extract_credentials("https://git.example.com/repo.git");
        assert_eq!(url, "https://git.example.com/repo.git");
        assert!(user.is_none());
        assert!(token.is_none());
    }

    #[test]
    fn init_repo_structure_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        init_repo_structure(dir.path()).unwrap();
        init_repo_structure(dir.path()).unwrap(); // second call must not fail
        assert!(dir.path().join("tasks").is_dir());
        let gi = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(gi.matches(".next.db").count(), 1);
    }
}
