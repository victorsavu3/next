use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::Mutex;

use next::log::Logger;
use next::mcp::{
    config::McpConfig,
    git_init::clone_or_open,
    server::{AppState, build_router},
    sync_manager::{spawn_deferred_sync, spawn_periodic_sync},
};
use next::{AppContext, Config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = McpConfig::from_env()?;

    // Ensure the git repo is present (clone if needed).
    let repo_path = clone_or_open(&config)?;

    // Open the task store.
    let (store, vcs) = next::storage::open(repo_path.clone())
        .map_err(|e| anyhow::anyhow!("failed to open task store: {e}"))?;
    let log = Logger::new(&repo_path);

    let ctx = AppContext {
        config: Config::default(),
        store: Box::new(store),
        vcs: Box::new(vcs),
        repo_root: repo_path,
        log,
    };
    let ctx = Arc::new(Mutex::new(ctx));

    // Background deferred-sync task.
    let scheduler = spawn_deferred_sync(ctx.clone());

    // Background periodic-sync task (if configured).
    if let Some(interval) = config.sync_interval {
        spawn_periodic_sync(ctx.clone(), interval);
    }

    let state = AppState {
        ctx,
        bearer_token: config.bearer_token,
        webhook_token: config.webhook_token,
        scheduler,
    };

    let router = build_router(state);

    let listener = TcpListener::bind(config.bind_addr).await?;
    eprintln!("next-mcp listening on {}", config.bind_addr);

    axum::serve(listener, router).await?;
    Ok(())
}
