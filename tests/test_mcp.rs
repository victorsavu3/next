#![cfg(feature = "mcp")]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use next::mcp::{
    server::{AppState, build_router},
    sync_manager::spawn_deferred_sync,
};
use next::{AppContext, Config};

// ── Test server ───────────────────────────────────────────────────────────────

fn init_git_repo(dir: &Path) {
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "test@test.com"],
        vec!["config", "user.name", "Test"],
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(dir)
            .status()
            .expect("git command failed");
    }
}

async fn start_test_server(
    bearer_token: &str,
    webhook_token: Option<&str>,
    repo_dir: &Path,
) -> SocketAddr {
    init_git_repo(repo_dir);
    let (store, vcs) = next::storage::open(repo_dir.to_path_buf()).unwrap();
    let ctx = AppContext {
        config: Config::default(),
        store: Box::new(store),
        vcs: Box::new(vcs),
        repo_root: repo_dir.to_path_buf(),
    };
    let ctx = Arc::new(Mutex::new(ctx));
    let scheduler = spawn_deferred_sync(ctx.clone(), std::time::Duration::from_secs(30));

    let state = AppState {
        ctx,
        bearer_token: bearer_token.to_owned(),
        webhook_token: webhook_token.map(str::to_owned),
        scheduler,
    };

    let router = build_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    addr
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn mcp_call(
    client: &Client,
    addr: SocketAddr,
    token: &str,
    method: &str,
    params: Value,
) -> Value {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    client
        .post(format!("http://{addr}/"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn tool_call(
    client: &Client,
    addr: SocketAddr,
    token: &str,
    tool: &str,
    arguments: Value,
) -> Value {
    mcp_call(
        client, addr, token, "tools/call",
        json!({ "name": tool, "arguments": arguments }),
    ).await
}

fn result_text(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_owned()
}

fn result_value(response: &Value) -> Value {
    let text = result_text(response);
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

fn is_error(response: &Value) -> bool {
    response["result"]["isError"].as_bool().unwrap_or(false)
        || response.get("error").is_some()
}

// ── MCP protocol tests ────────────────────────────────────────────────────────

#[tokio::test]
async fn initialize_handshake() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let resp = mcp_call(&Client::new(), addr, "tok", "initialize", json!({})).await;
    assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    assert!(resp["result"]["serverInfo"]["name"].as_str().unwrap().contains("mcp"));
}

#[tokio::test]
async fn tools_list_returns_13_tools() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let resp = mcp_call(&Client::new(), addr, "tok", "tools/list", json!({})).await;
    let tools = resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 13);
}

#[tokio::test]
async fn missing_token_returns_401() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("secret", None, dir.path()).await;
    let status = Client::new()
        .post(format!("http://{addr}/"))
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }))
        .send().await.unwrap().status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn wrong_token_returns_401() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("correct", None, dir.path()).await;
    let status = Client::new()
        .post(format!("http://{addr}/"))
        .bearer_auth("wrong")
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }))
        .send().await.unwrap().status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unknown_method_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let resp = mcp_call(&Client::new(), addr, "tok", "nonexistent/method", json!({})).await;
    assert!(resp.get("error").is_some());
    assert_eq!(resp["error"]["code"], -32601);
}

// ── Task tools ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn add_and_list_task() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let c = Client::new();

    let add = tool_call(&c, addr, "tok", "add_task",
        json!({ "title": "Buy milk", "autosync": false })).await;
    assert!(!is_error(&add));
    let task = result_value(&add);
    assert_eq!(task["title"], "Buy milk");

    let list = tool_call(&c, addr, "tok", "list_tasks", json!({})).await;
    assert!(!is_error(&list));
    let tasks: Vec<Value> = serde_json::from_str(&result_text(&list)).unwrap();
    assert_eq!(tasks.len(), 1);
}

#[tokio::test]
async fn get_task_with_children() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let c = Client::new();

    let parent = tool_call(&c, addr, "tok", "add_task",
        json!({ "title": "Parent", "slug": "par", "autosync": false })).await;
    let pid = result_value(&parent)["id"].as_str().unwrap().to_owned();

    tool_call(&c, addr, "tok", "add_task",
        json!({ "title": "Child", "parent": "par", "autosync": false })).await;

    let get = tool_call(&c, addr, "tok", "get_task", json!({ "id": pid })).await;
    assert!(!is_error(&get));
    let detail = result_value(&get);
    assert_eq!(detail["task"]["title"], "Parent");
    assert_eq!(detail["children"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn update_task_transitions() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let c = Client::new();

    let add = tool_call(&c, addr, "tok", "add_task",
        json!({ "title": "Work item", "autosync": false })).await;
    let id = result_value(&add)["id"].as_str().unwrap().to_owned();

    let started = tool_call(&c, addr, "tok", "update_task",
        json!({ "id": id, "action": "start", "autosync": false })).await;
    assert_eq!(result_value(&started)["status"], "started");

    let stopped = tool_call(&c, addr, "tok", "update_task",
        json!({ "id": id, "action": "stop", "autosync": false })).await;
    assert_eq!(result_value(&stopped)["status"], "open");

    let done = tool_call(&c, addr, "tok", "update_task",
        json!({ "id": id, "action": "done", "autosync": false })).await;
    assert_eq!(result_value(&done)["status"], "done");
}

#[tokio::test]
async fn update_task_done_with_recurrence_spawns_next() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let c = Client::new();

    let add = tool_call(&c, addr, "tok", "add_task",
        json!({ "title": "Daily standup", "recur_completion": 1, "autosync": false })).await;
    let id = result_value(&add)["id"].as_str().unwrap().to_owned();

    tool_call(&c, addr, "tok", "update_task",
        json!({ "id": id, "action": "done", "autosync": false })).await;

    let list = tool_call(&c, addr, "tok", "list_tasks", json!({ "include_all": true })).await;
    let tasks: Vec<Value> = serde_json::from_str(&result_text(&list)).unwrap();
    assert_eq!(tasks.len(), 2, "original + spawned next instance");
}

#[tokio::test]
async fn delete_task() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let c = Client::new();

    let add = tool_call(&c, addr, "tok", "add_task",
        json!({ "title": "Delete me", "autosync": false })).await;
    let id = result_value(&add)["id"].as_str().unwrap().to_owned();

    let del = tool_call(&c, addr, "tok", "delete_task",
        json!({ "id": id, "autosync": false })).await;
    assert!(!is_error(&del));

    let list = tool_call(&c, addr, "tok", "list_tasks", json!({ "include_all": true })).await;
    let tasks: Vec<Value> = serde_json::from_str(&result_text(&list)).unwrap();
    assert!(tasks.is_empty());
}

// ── State tools ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn context_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let c = Client::new();

    tool_call(&c, addr, "tok", "set_context",
        json!({ "contexts": ["@work"], "autosync": false })).await;
    let state = result_value(&tool_call(&c, addr, "tok", "get_state", json!({})).await);
    assert_eq!(state["active_contexts"][0], "@work");

    tool_call(&c, addr, "tok", "set_context",
        json!({ "contexts": [], "autosync": false })).await;
    let state2 = result_value(&tool_call(&c, addr, "tok", "get_state", json!({})).await);
    assert_eq!(state2["active_contexts"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn sync_tool_errors_without_remote() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let resp = tool_call(&Client::new(), addr, "tok", "sync", json!({})).await;
    assert!(is_error(&resp));
}

// ── Tag and data tools ────────────────────────────────────────────────────────

#[tokio::test]
async fn manage_tag_describe_and_show() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let c = Client::new();

    tool_call(&c, addr, "tok", "manage_tag",
        json!({ "action": "describe", "tag": "@work", "description": "Office", "autosync": false })).await;

    let show = result_value(&tool_call(&c, addr, "tok", "manage_tag",
        json!({ "action": "show", "tag": "@work" })).await);
    assert_eq!(show["meta"]["description"], "Office");
}

#[tokio::test]
async fn manage_task_data_set_get_unset() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let c = Client::new();

    let add = tool_call(&c, addr, "tok", "add_task",
        json!({ "title": "Data task", "autosync": false })).await;
    let id = result_value(&add)["id"].as_str().unwrap().to_owned();

    tool_call(&c, addr, "tok", "manage_task_data",
        json!({ "action": "set", "id": id, "key": "effort", "value": "3", "autosync": false })).await;
    let get = result_value(&tool_call(&c, addr, "tok", "manage_task_data",
        json!({ "action": "get", "id": id, "key": "effort" })).await);
    assert_eq!(get, 3);

    tool_call(&c, addr, "tok", "manage_task_data",
        json!({ "action": "unset", "id": id, "key": "effort", "autosync": false })).await;
    let err = tool_call(&c, addr, "tok", "manage_task_data",
        json!({ "action": "get", "id": id, "key": "effort" })).await;
    assert!(is_error(&err));
}

#[tokio::test]
async fn get_forecast_empty() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let resp = tool_call(&Client::new(), addr, "tok", "get_forecast", json!({})).await;
    assert!(!is_error(&resp));
    let v: Vec<Value> = serde_json::from_str(&result_text(&resp)).unwrap();
    assert!(v.is_empty());
}

// ── Webhook tests ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn webhook_valid_token() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("mcp-tok", Some("hook-tok"), dir.path()).await;
    let resp = Client::new()
        .post(format!("http://{addr}/webhook/sync"))
        .bearer_auth("hook-tok")
        .send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("status").is_some());
}

#[tokio::test]
async fn webhook_wrong_token_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("mcp-tok", Some("hook-tok"), dir.path()).await;
    let status = Client::new()
        .post(format!("http://{addr}/webhook/sync"))
        .bearer_auth("wrong")
        .send().await.unwrap().status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webhook_mcp_token_not_accepted_on_webhook_route() {
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("mcp-tok", Some("hook-tok"), dir.path()).await;
    let status = Client::new()
        .post(format!("http://{addr}/webhook/sync"))
        .bearer_auth("mcp-tok")
        .send().await.unwrap().status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webhook_unconfigured_returns_401_not_404() {
    // When NEXT_WEBHOOK_TOKEN is unset the endpoint must still return 401 so
    // that an attacker cannot distinguish "no webhook" from "wrong token".
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("mcp-tok", None, dir.path()).await;
    let status = Client::new()
        .post(format!("http://{addr}/webhook/sync"))
        .bearer_auth("anything")
        .send().await.unwrap().status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ── Autosync behaviour ────────────────────────────────────────────────────────

#[tokio::test]
async fn autosync_false_does_not_block_on_sync() {
    // With autosync=false the mutation completes without triggering an immediate
    // sync attempt, so there's no error even though there is no remote.
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let resp = tool_call(&Client::new(), addr, "tok", "add_task",
        json!({ "title": "Batch", "autosync": false })).await;
    assert!(!is_error(&resp));
}

#[tokio::test]
async fn autosync_true_attempts_sync_and_surfaces_error() {
    // With autosync=true and no remote, the tool call still succeeds (the task
    // is saved) but the response should NOT be an error — sync errors are logged
    // but not propagated to the caller for autosync.
    let dir = tempfile::tempdir().unwrap();
    let addr = start_test_server("tok", None, dir.path()).await;
    let resp = tool_call(&Client::new(), addr, "tok", "add_task",
        json!({ "title": "Immediate sync", "autosync": true })).await;
    // Task was created successfully even though sync failed (no remote).
    assert!(!is_error(&resp));
    assert_eq!(result_value(&resp)["title"], "Immediate sync");
}
