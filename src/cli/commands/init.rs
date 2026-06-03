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
///  3. Append `.next.db`, its WAL sidecars, `.next.lock`, `state.toml` to `.gitignore`.
///  4. Create an initial git commit when the repository has no commits yet.
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

    // Step 3 — .gitignore entries for generated files.
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

    // Step 4 — initial commit when the repo is empty.
    let has_commits = has_any_commits(dir);
    if !has_commits {
        // Stage everything we just created.
        git(dir, &["add", ".gitignore"]).ok();

        match git(dir, &["commit", "-m", "next: init task repository"]) {
            Ok(_) => println!("Created initial commit."),
            Err(_) => {
                eprintln!(
                    "Note: could not create the initial commit.\n\
                     Set your git user info and commit manually:\n\
                     \n  git config user.name  'Your Name'\n\
                     \n  git config user.email 'you@example.com'\n\
                     \n  git add .gitignore && git commit -m 'init'"
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

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;

    anyhow::ensure!(
        status.success(),
        "git {} exited with status {status}",
        args.join(" ")
    );
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
    Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
