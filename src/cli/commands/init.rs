use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

use anyhow::Context as _;

#[derive(clap::Args, Debug)]
pub struct Args {}

/// Set up a new `next` task repository in `dir` (normally the current
/// working directory).
///
/// Steps:
///  1. `git init` if `.git` is absent.
///  2. Create `tasks/` if absent.
///  3. Write `config/scoring.toml` (the repo's committed scoring weights) if absent.
///  4. Append `.next.db`, its WAL sidecars, `.next.lock`, `state.toml` to `.gitignore`.
///  5. Create an initial git commit when the repository has no commits yet.
pub fn run(_args: Args, dir: &Path) -> anyhow::Result<()> {
    // Step 1 — git repository.
    let git_dir = dir.join(".git");
    if git_dir.exists() {
        println!("Using existing git repository.");
    } else {
        git(dir, &["init", "-q"]).context("failed to run `git init` — is git installed?")?;
        println!("Initialized git repository.");
    }

    // Step 2 — tasks/ directory.
    let tasks_dir = dir.join("tasks");
    if tasks_dir.exists() {
        println!("tasks/ already exists.");
    } else {
        fs::create_dir_all(&tasks_dir).context("failed to create tasks/")?;
        println!("Created tasks/");
    }

    // Step 3 — config/scoring.toml (committed scoring weights).
    ensure_scoring_config(dir)?;

    // Step 4 — .gitignore entries for generated files.
    // `.next.db-wal` / `.next.db-shm` are the SQLite WAL sidecar files.
    let gitignore_path = dir.join(".gitignore");
    for entry in &[
        ".next.db",
        ".next.db-wal",
        ".next.db-shm",
        ".next.lock",
        "state.toml",
    ] {
        ensure_gitignored(&gitignore_path, entry)?;
    }

    // Step 5 — initial commit when the repo is empty.
    let has_commits = has_any_commits(dir);
    if !has_commits {
        // Stage everything we just created.
        git(dir, &["add", ".gitignore", "config/scoring.toml"]).ok();

        match git(dir, &["commit", "-m", "next: init task repository"]) {
            Ok(_) => println!("Created initial commit."),
            Err(_) => {
                eprintln!(
                    "Note: could not create the initial commit.\n\
                     Set your git user info and commit manually:\n\
                     \n  git config user.name  'Your Name'\n\
                     \n  git config user.email 'you@example.com'\n\
                     \n  git add . && git commit -m 'init'"
                );
            }
        }
    }

    println!("\nReady. To get started:");
    println!("  cd {}", dir.display());
    println!("  next add \"My first task\"");
    println!("  next list");

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Environment variables that scope a `git` invocation to a repository.
/// Stripped so `next init` always targets `dir`, even when invoked from a
/// process that inherited them (e.g. a git hook exporting `GIT_DIR`).
const GIT_SCOPE_VARS: [&str; 5] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
];

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<()> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for var in GIT_SCOPE_VARS {
        cmd.env_remove(var);
    }
    let status = cmd
        .status()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;

    anyhow::ensure!(
        status.success(),
        "git {} exited with status {status}",
        args.join(" ")
    );
    Ok(())
}

/// Writes `<dir>/config/scoring.toml` with the default scoring weights when it
/// is absent. The file is committed to the repo (and synced), so every consumer
/// (cli/mcp/forgejo) scores tasks the same way. Omitted fields fall back to the
/// built-in defaults, so users may trim it to just the weights they change.
fn ensure_scoring_config(dir: &Path) -> anyhow::Result<()> {
    use crate::core::scoring::ScoringConfig;

    let path = crate::core::storage::scoring_path(dir);
    if path.exists() {
        println!("config/scoring.toml already exists.");
        return Ok(());
    }

    let config_dir = path.parent().expect("scoring_path always has a parent");
    fs::create_dir_all(config_dir).context("failed to create config/")?;

    let body = toml::to_string_pretty(&ScoringConfig::default())
        .context("failed to serialize default scoring config")?;
    let content = format!(
        "# Urgency scoring weights for this repository.\n\
         #\n\
         # Committed to the repo and synced, so the CLI, MCP server, and plugins\n\
         # all score tasks the same way. Edit to tune; omitted fields fall back to\n\
         # the built-in defaults.\n\
         \n{body}"
    );
    fs::write(&path, content).context("failed to write config/scoring.toml")?;
    println!("Created config/scoring.toml");
    Ok(())
}

fn ensure_gitignored(gitignore_path: &Path, entry: &str) -> anyhow::Result<()> {
    let already_present = if gitignore_path.exists() {
        fs::read_to_string(gitignore_path)
            .context("failed to read .gitignore")?
            .lines()
            .any(|l| l.trim() == entry)
    } else {
        false
    };

    if already_present {
        println!("{entry} already in .gitignore.");
    } else {
        let mut content = if gitignore_path.exists() {
            fs::read_to_string(gitignore_path).context("failed to read .gitignore")?
        } else {
            String::new()
        };
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(entry);
        content.push('\n');
        fs::write(gitignore_path, &content).context("failed to write .gitignore")?;
        println!("Added {entry} to .gitignore.");
    }
    Ok(())
}

fn has_any_commits(dir: &Path) -> bool {
    let mut cmd = Command::new("git");
    cmd.args(["rev-parse", "--verify", "HEAD"])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for var in GIT_SCOPE_VARS {
        cmd.env_remove(var);
    }
    cmd.status().map(|s| s.success()).unwrap_or(false)
}
