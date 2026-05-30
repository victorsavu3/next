//! Container integration tests.
//!
//! These tests build and run the real `next-mcp` container image via Podman/Docker
//! and verify end-to-end behaviour including git clone, MCP protocol, auth, and webhook.
//!
//! **Opt-in**: skipped unless the environment variable `CONTAINER_TESTS=1` is set.
//! Run with:
//!   CONTAINER_TESTS=1 \
//!   DOCKER_HOST=unix:///run/user/1000/podman/podman.sock \
//!   cargo test --features mcp --test test_container
//!
//! The image must be pre-built before running:
//!   podman build -f Containerfile -t localhost/next-mcp:latest .

#![cfg(feature = "mcp")]

use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use testcontainers::{
    core::{ContainerPort, Mount, WaitFor},
    runners::AsyncRunner,
    GenericImage, ImageExt,
};

fn container_tests_enabled() -> bool {
    std::env::var("CONTAINER_TESTS").as_deref() == Ok("1")
}

/// Creates a local bare git repo with `tasks/` and an initial commit.
/// The caller must keep the returned `TempDir` alive for the test duration.
fn create_bare_repo() -> tempfile::TempDir {
    let bare = tempfile::tempdir().unwrap();

    std::process::Command::new("git")
        .args(["init", "--bare", "-q"])
        .current_dir(bare.path())
        .status()
        .unwrap();

    // Make initial commit via a working clone.
    let work = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["clone", bare.path().to_str().unwrap(), "."])
        .current_dir(work.path())
        .status()
        .unwrap();
    for args in [
        vec!["config", "user.email", "test@test.com"],
        vec!["config", "user.name", "Test"],
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(work.path())
            .status()
            .unwrap();
    }
    std::fs::create_dir(work.path().join("tasks")).unwrap();
    std::fs::write(work.path().join(".gitignore"), ".next.db\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(work.path())
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(work.path())
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["push"])
        .current_dir(work.path())
        .status()
        .unwrap();

    // Relabel for SELinux container access (no-op if SELinux is absent or already labeled).
    let _ = std::process::Command::new("chcon")
        .args(["-t", "container_file_t", "-R", bare.path().to_str().unwrap()])
        .status();

    bare
}

/// Returns the number of commits in a bare git repo on the host.
fn bare_repo_commit_count(bare_path: &std::path::Path) -> usize {
    let out = std::process::Command::new("git")
        .args(["-C", bare_path.to_str().unwrap(), "log", "--oneline"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .count()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn mcp_call(client: &Client, port: u16, token: &str, method: &str, params: Value) -> Value {
    let url = format!("http://127.0.0.1:{port}/");
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    client
        .post(&url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn tool_call(client: &Client, port: u16, token: &str, tool: &str, args: Value) -> Value {
    mcp_call(client, port, token, "tools/call",
        json!({ "name": tool, "arguments": args })).await
}

fn result_text(r: &Value) -> String {
    r["result"]["content"][0]["text"].as_str().unwrap_or("").to_owned()
}
fn result_value(r: &Value) -> Value {
    serde_json::from_str(&result_text(r)).unwrap_or(Value::Null)
}
fn is_error(r: &Value) -> bool {
    r["result"]["isError"].as_bool().unwrap_or(false) || r.get("error").is_some()
}

// The bare repo is bind-mounted at this path inside every container.
const CONTAINER_BARE_REPO: &str = "/bare-repo";

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn container_initialize_and_tools_list() {
    if !container_tests_enabled() { return; }

    let bare = create_bare_repo();

    let container = GenericImage::new("localhost/next-mcp", "latest")
        .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
        .with_exposed_port(ContainerPort::Tcp(3000))
        .with_env_var("NEXT_BEARER_TOKEN", "test-token")
        .with_env_var("NEXT_GIT_URL", format!("file://{CONTAINER_BARE_REPO}"))
        .with_env_var("NEXT_REPO_PATH", "/data/tasks")
        .with_env_var("NEXT_SYNC_INTERVAL", "0")
        .with_mount(Mount::bind_mount(bare.path().to_str().unwrap(), CONTAINER_BARE_REPO))
        .start()
        .await
        .unwrap();

    let port = container.get_host_port_ipv4(3000).await.unwrap();
    let client = Client::new();

    // Initialize handshake.
    let resp = mcp_call(&client, port, "test-token", "initialize", json!({})).await;
    assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");

    // Tools list.
    let resp = mcp_call(&client, port, "test-token", "tools/list", json!({})).await;
    assert_eq!(resp["result"]["tools"].as_array().unwrap().len(), 13);
}

#[tokio::test]
async fn container_auth_enforced() {
    if !container_tests_enabled() { return; }

    let bare = create_bare_repo();

    let container = GenericImage::new("localhost/next-mcp", "latest")
        .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
        .with_exposed_port(ContainerPort::Tcp(3000))
        .with_env_var("NEXT_BEARER_TOKEN", "correct-token")
        .with_env_var("NEXT_GIT_URL", format!("file://{CONTAINER_BARE_REPO}"))
        .with_env_var("NEXT_SYNC_INTERVAL", "0")
        .with_mount(Mount::bind_mount(bare.path().to_str().unwrap(), CONTAINER_BARE_REPO))
        .start()
        .await
        .unwrap();

    let port = container.get_host_port_ipv4(3000).await.unwrap();
    let client = Client::new();

    // Missing token.
    let status = client
        .post(format!("http://127.0.0.1:{port}/"))
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }))
        .send().await.unwrap().status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Wrong token.
    let status = client
        .post(format!("http://127.0.0.1:{port}/"))
        .bearer_auth("wrong")
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }))
        .send().await.unwrap().status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Correct token.
    let resp = mcp_call(&client, port, "correct-token", "initialize", json!({})).await;
    assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
}

#[tokio::test]
async fn container_add_task_persists_to_volume() {
    if !container_tests_enabled() { return; }

    let bare = create_bare_repo();
    let volume_name = format!("next-test-{}", uuid::Uuid::new_v4().as_simple());

    let container = GenericImage::new("localhost/next-mcp", "latest")
        .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
        .with_exposed_port(ContainerPort::Tcp(3000))
        .with_env_var("NEXT_BEARER_TOKEN", "tok")
        .with_env_var("NEXT_GIT_URL", format!("file://{CONTAINER_BARE_REPO}"))
        .with_env_var("NEXT_SYNC_INTERVAL", "0")
        .with_mount(Mount::bind_mount(bare.path().to_str().unwrap(), CONTAINER_BARE_REPO))
        .with_mount(Mount::volume_mount(&volume_name, "/data/tasks"))
        .start()
        .await
        .unwrap();

    let port = container.get_host_port_ipv4(3000).await.unwrap();
    let client = Client::new();

    let add = tool_call(&client, port, "tok", "add_task",
        json!({ "title": "Container task", "autosync": false })).await;
    assert!(!is_error(&add), "add_task failed: {add}");
    assert_eq!(result_value(&add)["title"], "Container task");

    let list = tool_call(&client, port, "tok", "list_tasks", json!({})).await;
    let tasks: Vec<Value> = serde_json::from_str(&result_text(&list)).unwrap();
    assert_eq!(tasks.len(), 1);

    // Clean up named volume.
    drop(container);
    let _ = std::process::Command::new("podman")
        .args(["volume", "rm", "-f", &volume_name])
        .status();
}

#[tokio::test]
async fn container_git_init_idempotent() {
    if !container_tests_enabled() { return; }

    let bare = create_bare_repo();
    let volume_name = format!("next-test-{}", uuid::Uuid::new_v4().as_simple());

    // First start: clones the repo.
    {
        let container = GenericImage::new("localhost/next-mcp", "latest")
            .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
            .with_exposed_port(ContainerPort::Tcp(3000))
            .with_env_var("NEXT_BEARER_TOKEN", "tok")
            .with_env_var("NEXT_GIT_URL", format!("file://{CONTAINER_BARE_REPO}"))
            .with_env_var("NEXT_SYNC_INTERVAL", "0")
            .with_mount(Mount::bind_mount(bare.path().to_str().unwrap(), CONTAINER_BARE_REPO))
            .with_mount(Mount::volume_mount(&volume_name, "/data/tasks"))
            .start()
            .await
            .unwrap();

        let port = container.get_host_port_ipv4(3000).await.unwrap();
        let resp = mcp_call(&Client::new(), port, "tok", "initialize", json!({})).await;
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    }

    // Second start: repo already exists on volume; NEXT_GIT_URL absent → must open cleanly.
    {
        let container = GenericImage::new("localhost/next-mcp", "latest")
            .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
            .with_exposed_port(ContainerPort::Tcp(3000))
            .with_env_var("NEXT_BEARER_TOKEN", "tok")
            .with_env_var("NEXT_SYNC_INTERVAL", "0")
            .with_mount(Mount::volume_mount(&volume_name, "/data/tasks"))
            .start()
            .await
            .unwrap();

        let port = container.get_host_port_ipv4(3000).await.unwrap();
        let resp = mcp_call(&Client::new(), port, "tok", "initialize", json!({})).await;
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    }

    let _ = std::process::Command::new("podman")
        .args(["volume", "rm", "-f", &volume_name])
        .status();
}

#[tokio::test]
async fn container_missing_repo_and_no_git_url_fails() {
    if !container_tests_enabled() { return; }

    // Container with no repo and no NEXT_GIT_URL must exit before the listen message.
    let result = GenericImage::new("localhost/next-mcp", "latest")
        .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
        .with_exposed_port(ContainerPort::Tcp(3000))
        .with_env_var("NEXT_BEARER_TOKEN", "tok")
        .start()
        .await;

    assert!(result.is_err(), "expected container to fail without a git URL");
}

#[tokio::test]
async fn container_webhook_sync() {
    if !container_tests_enabled() { return; }

    let bare = create_bare_repo();

    let container = GenericImage::new("localhost/next-mcp", "latest")
        .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
        .with_exposed_port(ContainerPort::Tcp(3000))
        .with_env_var("NEXT_BEARER_TOKEN", "mcp-tok")
        .with_env_var("NEXT_WEBHOOK_TOKEN", "hook-tok")
        .with_env_var("NEXT_GIT_URL", format!("file://{CONTAINER_BARE_REPO}"))
        .with_env_var("NEXT_SYNC_INTERVAL", "0")
        .with_mount(Mount::bind_mount(bare.path().to_str().unwrap(), CONTAINER_BARE_REPO))
        .start()
        .await
        .unwrap();

    let port = container.get_host_port_ipv4(3000).await.unwrap();
    let client = Client::new();

    // Valid webhook token → 200 (sync may fail since file:// remote doesn't support push,
    // but the HTTP response is still 200 with a status field).
    let resp = client
        .post(format!("http://127.0.0.1:{port}/webhook/sync"))
        .bearer_auth("hook-tok")
        .send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("status").is_some());

    // MCP bearer token must not work on webhook route.
    let status = client
        .post(format!("http://127.0.0.1:{port}/webhook/sync"))
        .bearer_auth("mcp-tok")
        .send().await.unwrap().status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn container_sync_pushes_to_remote() {
    if !container_tests_enabled() { return; }

    let bare = create_bare_repo();
    let initial_commits = bare_repo_commit_count(bare.path());

    let container = GenericImage::new("localhost/next-mcp", "latest")
        .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
        .with_exposed_port(ContainerPort::Tcp(3000))
        .with_env_var("NEXT_BEARER_TOKEN", "tok")
        .with_env_var("NEXT_GIT_URL", format!("file://{CONTAINER_BARE_REPO}"))
        .with_env_var("NEXT_SYNC_INTERVAL", "0")
        .with_mount(Mount::bind_mount(bare.path().to_str().unwrap(), CONTAINER_BARE_REPO))
        .start()
        .await
        .unwrap();

    let port = container.get_host_port_ipv4(3000).await.unwrap();
    let client = Client::new();

    // Add a task without autosync so nothing is pushed yet.
    let add = tool_call(&client, port, "tok", "add_task",
        json!({ "title": "Sync test task", "autosync": false })).await;
    assert!(!is_error(&add), "add_task failed: {add}");

    // Bare repo should still have only the initial commits (no push yet).
    assert_eq!(
        bare_repo_commit_count(bare.path()),
        initial_commits,
        "bare repo should not have new commits before explicit sync"
    );

    // Call the sync tool.
    let sync_resp = tool_call(&client, port, "tok", "sync", json!({})).await;
    assert!(!is_error(&sync_resp), "sync failed: {sync_resp}");

    // Bare repo should now have the new commit.
    assert!(
        bare_repo_commit_count(bare.path()) > initial_commits,
        "bare repo should have new commits after sync"
    );
}

#[tokio::test]
async fn container_deferred_sync_fires_after_delay() {
    if !container_tests_enabled() { return; }

    let bare = create_bare_repo();
    let initial_commits = bare_repo_commit_count(bare.path());

    // Use a very short deferred delay so the test doesn't take 30 seconds.
    let container = GenericImage::new("localhost/next-mcp", "latest")
        .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
        .with_exposed_port(ContainerPort::Tcp(3000))
        .with_env_var("NEXT_BEARER_TOKEN", "tok")
        .with_env_var("NEXT_GIT_URL", format!("file://{CONTAINER_BARE_REPO}"))
        .with_env_var("NEXT_SYNC_INTERVAL", "0")
        .with_env_var("NEXT_DEFERRED_SYNC_DELAY_SECS", "3")
        .with_mount(Mount::bind_mount(bare.path().to_str().unwrap(), CONTAINER_BARE_REPO))
        .start()
        .await
        .unwrap();

    let port = container.get_host_port_ipv4(3000).await.unwrap();
    let client = Client::new();

    // Add tasks with autosync=false — this schedules the deferred 3-second timer.
    for i in 0..3 {
        let add = tool_call(&client, port, "tok", "add_task",
            json!({ "title": format!("Deferred task {i}"), "autosync": false })).await;
        assert!(!is_error(&add), "add_task {i} failed: {add}");
    }

    // Nothing pushed yet.
    assert_eq!(
        bare_repo_commit_count(bare.path()),
        initial_commits,
        "bare repo should not have new commits immediately after mutations"
    );

    // Wait long enough for the deferred timer to fire (3s delay + buffer).
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;

    // Deferred sync should have pushed the commits.
    assert!(
        bare_repo_commit_count(bare.path()) > initial_commits,
        "bare repo should have new commits after deferred sync delay"
    );
}
