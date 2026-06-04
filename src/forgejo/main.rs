//! `next-plugin-forgejo` — Forgejo integration plugin binary.

use std::path::Path;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use next::forgejo::{config, hook, issues::ForgejoApi, reconcile, tasks::LibTaskStore, PLUGIN_NAME};

#[derive(Parser)]
#[command(name = "next-plugin-forgejo", about = "Forgejo integration plugin for next")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Import issues from the mapped repositories and reconcile resolution.
    Sync {
        /// Print the actions that would be taken without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Handle a `next` export-hook event: close the linked issue on resolution.
    Hook,
    /// Register this plugin with `next`'s export hook.
    Register,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Sync { dry_run } => sync(dry_run),
        Command::Hook => hook::run(),
        Command::Register => register(),
    }
}

/// Registers this binary as `next`'s export-hook handler (idempotent: replaces
/// the command, keeps existing task subscriptions).
fn ensure_registered(repo_root: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("resolve current executable")?;
    let command = vec![exe.to_string_lossy().into_owned(), "hook".to_owned()];
    next::plugin::registry::register(repo_root, PLUGIN_NAME, command)?;
    Ok(())
}

/// `register` subcommand: register the export hook without importing anything.
fn register() -> Result<()> {
    let cfg = config::load()?;
    let store = LibTaskStore::open(cfg.next_repo.as_deref())?;
    ensure_registered(store.repo_root())?;
    println!("registered {PLUGIN_NAME} export hook for {}", store.repo_root().display());
    Ok(())
}

/// Import issues and reconcile resolution across all configured mappings.
fn sync(dry_run: bool) -> Result<()> {
    let cfg = config::load()?;
    if cfg.mappings.is_empty() {
        eprintln!("no [[map]] entries configured; nothing to sync");
        return Ok(());
    }
    let issues = ForgejoApi::new(&cfg.forgejo_url, &cfg.forgejo_token)?;
    let mut tasks = LibTaskStore::open(cfg.next_repo.as_deref())?;

    // Self-register so the per-task `watch` during import succeeds and local
    // resolution later fires the hook. Idempotent (keeps existing watches).
    ensure_registered(tasks.repo_root())?;

    // Best-effort pull so we reconcile against the canonical repo and avoid
    // duplicate imports; never fatal (e.g. no remote configured).
    if !dry_run {
        if let Err(e) = tasks.git_pull() {
            tracing::warn!("git pull before sync failed (continuing): {e:#}");
        }
    }

    let summary = reconcile::sync(&issues, &mut tasks, &cfg.mappings, dry_run)?;

    if !dry_run {
        if let Err(e) = tasks.git_push() {
            tracing::warn!("git push after sync failed (continuing): {e:#}");
        }
    }

    println!(
        "forgejo sync: {} task(s) created, {} task(s) closed, {} issue(s) closed{}",
        summary.created,
        summary.tasks_closed,
        summary.issues_closed,
        if dry_run { " (dry-run)" } else { "" },
    );
    Ok(())
}
