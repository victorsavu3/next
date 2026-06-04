//! Export-hook handler: when a watched task is resolved locally, close its
//! Forgejo issue.
//!
//! `next` spawns `next-plugin-forgejo hook` after a watched task changes,
//! delivering the event on stdin and in `NEXT_PLUGIN_EVENT`, with `NEXT_REPO`
//! pointing at the repository. This handler only reads the task (via the
//! library) and mutates Forgejo, so it never loops.

use std::io::Read as _;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;
use uuid::Uuid;

use super::{
    config,
    issues::{ForgejoApi, IssueSource},
    tasks::{forgejo_link, LibTaskStore, TaskStore},
};

#[derive(Deserialize)]
struct Event {
    event: String,
    task_id: String,
}

/// Processes one export-hook event.
pub fn run() -> Result<()> {
    let event = read_event()?;

    // Resolution only (close-only): ignore other verbs.
    if event.event != "done" && event.event != "cancel" {
        return Ok(());
    }
    let id = Uuid::parse_str(&event.task_id).context("event task_id is not a UUID")?;

    let repo = std::env::var("NEXT_REPO").ok().map(PathBuf::from);
    let store = LibTaskStore::open(repo.as_deref())?;
    let Some(task) = store.show(id)? else {
        return Ok(()); // task vanished
    };
    let Some((repo_full, number)) = forgejo_link(&task) else {
        return Ok(()); // not a Forgejo-linked task
    };
    let (owner, repo_name) = repo_full
        .split_once('/')
        .with_context(|| format!("malformed __forgejo-repo {repo_full:?}"))?;

    let cfg = config::load()?;
    let issues = ForgejoApi::new(&cfg.forgejo_url, &cfg.forgejo_token)?;
    // Closing an already-closed issue is idempotent, so no need to fetch state.
    issues.set_closed(owner, repo_name, number, true)?;
    tracing::info!("forgejo: closed {repo_full}#{number} (task {id} {})", event.event);
    Ok(())
}

/// Reads the event JSON from `NEXT_PLUGIN_EVENT`, falling back to stdin.
fn read_event() -> Result<Event> {
    let raw = match std::env::var("NEXT_PLUGIN_EVENT") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).context("read event from stdin")?;
            s
        }
    };
    serde_json::from_str(&raw).context("parse plugin event")
}
