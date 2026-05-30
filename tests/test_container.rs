//! Container integration tests.
//!
//! These tests build and run the real `next-mcp` container image via Podman/Docker
//! and verify end-to-end behaviour including git clone, MCP protocol, auth, and webhook.
//!
//! **Opt-in**: skipped unless the environment variable `CONTAINER_TESTS=1` is set.
//! Run with:
//!   CONTAINER_TESTS=1 cargo test --features mcp --test test_container
//!
//! The image must be pre-built before running:
//!   podman build -f Containerfile -t localhost/next-mcp:latest .

#![cfg(feature = "mcp")]

use std::collections::HashMap;

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

/// Creates a local bare git repo with a `tasks/` directory and an initial commit.
/// Returns the path to the bare repo.
fn create_bare_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();

    // Init bare repo.
    std::process::Command::new("git")
        .args(["init", "--bare", "-q"])
        .current_dir(dir.path())
        .status()
        .unwrap();

    // Create a working clone to make the initial commit.
    let work_dir = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["clone", dir.path().to_str().unwrap(), "."])
        .current_dir(work_dir.path())
        .status()
        .unwrap();
    for args in [
        vec!["config", "user.email", "test@test.com"],
        vec!["config", "user.name", "Test"],
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(work_dir.path())
            .status()
            .unwrap();
    }
    std::fs::create_dir(work_dir.path().join("tasks")).unwrap();
    std::fs::write(work_dir.path().join(".gitignore"), ".next.db\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(work_dir.path())
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(work_dir.path())
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["push"])
        .current_dir(work_dir.path())
        .status()
        .unwrap();

    dir
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn container_initialize_and_tools_list() {
    if !container_tests_enabled() { return; }

    let bare_repo = create_bare_repo();
    let bare_path = bare_repo.path().to_str().unwrap().to_owned();

    let container = GenericImage::new("localhost/next-mcp", "latest")
        .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
        .with_exposed_port(ContainerPort::Tcp(3000))
        .with_env_var("NEXT_BEARER_TOKEN", "test-token")
        .with_env_var("NEXT_GIT_URL", format!("file://{bare_path}"))
        .with_env_var("NEXT_REPO_PATH", "/data/tasks")
        .with_env_var("NEXT_SYNC_INTERVAL", "0")
        .with_mount(Mount::volume_mount("next-tasks-test", "/data/tasks"))
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

    let bare_repo = create_bare_repo();
    let bare_path = bare_repo.path().to_str().unwrap().to_owned();

    let container = GenericImage::new("localhost/next-mcp", "latest")
        .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
        .with_exposed_port(ContainerPort::Tcp(3000))
        .with_env_var("NEXT_BEARER_TOKEN", "correct-token")
        .with_env_var("NEXT_GIT_URL", format!("file://{bare_path}"))
        .with_env_var("NEXT_SYNC_INTERVAL", "0")
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

    let bare_repo = create_bare_repo();
    let bare_path = bare_repo.path().to_str().unwrap().to_owned();
    let volume_name = format!("next-test-{}", uuid::Uuid::new_v4().as_simple());

    let container = GenericImage::new("localhost/next-mcp", "latest")
        .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
        .with_exposed_port(ContainerPort::Tcp(3000))
        .with_env_var("NEXT_BEARER_TOKEN", "tok")
        .with_env_var("NEXT_GIT_URL", format!("file://{bare_path}"))
        .with_env_var("NEXT_SYNC_INTERVAL", "0")
        .with_mount(Mount::volume_mount(&volume_name, "/data/tasks"))
        .start()
        .await
        .unwrap();

    let port = container.get_host_port_ipv4(3000).await.unwrap();
    let client = Client::new();

    // Add a task (autosync=false to avoid push errors against bare repo over file://).
    let add = tool_call(&client, port, "tok", "add_task",
        json!({ "title": "Container task", "autosync": false })).await;
    assert!(!is_error(&add), "add_task failed: {add}");
    assert_eq!(result_value(&add)["title"], "Container task");

    let list = tool_call(&client, port, "tok", "list_tasks", json!({})).await;
    let tasks: Vec<Value> = serde_json::from_str(&result_text(&list)).unwrap();
    assert_eq!(tasks.len(), 1);
}

#[tokio::test]
async fn container_git_init_idempotent() {
    if !container_tests_enabled() { return; }

    let bare_repo = create_bare_repo();
    let bare_path = bare_repo.path().to_str().unwrap().to_owned();
    let volume_name = format!("next-test-{}", uuid::Uuid::new_v4().as_simple());

    // First start: clones the repo.
    {
        let container = GenericImage::new("localhost/next-mcp", "latest")
            .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
            .with_exposed_port(ContainerPort::Tcp(3000))
            .with_env_var("NEXT_BEARER_TOKEN", "tok")
            .with_env_var("NEXT_GIT_URL", format!("file://{bare_path}"))
            .with_env_var("NEXT_SYNC_INTERVAL", "0")
            .with_mount(Mount::volume_mount(&volume_name, "/data/tasks"))
            .start()
            .await
            .unwrap();

        let port = container.get_host_port_ipv4(3000).await.unwrap();
        let resp = mcp_call(&Client::new(), port, "tok", "initialize", json!({})).await;
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    }

    // Second start: repo already exists on volume — must not fail.
    {
        let container = GenericImage::new("localhost/next-mcp", "latest")
            .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
            .with_exposed_port(ContainerPort::Tcp(3000))
            .with_env_var("NEXT_BEARER_TOKEN", "tok")
            // No NEXT_GIT_URL — repo is already on the volume.
            .with_env_var("NEXT_SYNC_INTERVAL", "0")
            .with_mount(Mount::volume_mount(&volume_name, "/data/tasks"))
            .start()
            .await
            .unwrap();

        let port = container.get_host_port_ipv4(3000).await.unwrap();
        let resp = mcp_call(&Client::new(), port, "tok", "initialize", json!({})).await;
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    }
}

#[tokio::test]
async fn container_missing_repo_and_no_git_url_fails() {
    if !container_tests_enabled() { return; }

    // Container with no repo and no NEXT_GIT_URL must exit non-zero.
    let result = GenericImage::new("localhost/next-mcp", "latest")
        .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
        .with_exposed_port(ContainerPort::Tcp(3000))
        .with_env_var("NEXT_BEARER_TOKEN", "tok")
        // Intentionally no NEXT_GIT_URL and no pre-existing volume.
        .start()
        .await;

    // testcontainers will fail to reach the WaitFor condition because the
    // container exits with an error before printing the listen message.
    assert!(result.is_err(), "expected container to fail without a git URL");
}

#[tokio::test]
async fn container_webhook_sync() {
    if !container_tests_enabled() { return; }

    let bare_repo = create_bare_repo();
    let bare_path = bare_repo.path().to_str().unwrap().to_owned();

    let container = GenericImage::new("localhost/next-mcp", "latest")
        .with_wait_for(WaitFor::message_on_stderr("next-mcp listening"))
        .with_exposed_port(ContainerPort::Tcp(3000))
        .with_env_var("NEXT_BEARER_TOKEN", "mcp-tok")
        .with_env_var("NEXT_WEBHOOK_TOKEN", "hook-tok")
        .with_env_var("NEXT_GIT_URL", format!("file://{bare_path}"))
        .with_env_var("NEXT_SYNC_INTERVAL", "0")
        .start()
        .await
        .unwrap();

    let port = container.get_host_port_ipv4(3000).await.unwrap();
    let client = Client::new();

    // Valid webhook token.
    let resp = client
        .post(format!("http://127.0.0.1:{port}/webhook/sync"))
        .bearer_auth("hook-tok")
        .send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // MCP bearer token must not work on webhook route.
    let status = client
        .post(format!("http://127.0.0.1:{port}/webhook/sync"))
        .bearer_auth("mcp-tok")
        .send().await.unwrap().status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
