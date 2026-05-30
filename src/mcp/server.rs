use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Json, Response},
    routing::post,
    Router,
};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::AppContext;

use super::auth::{require_mcp_bearer, require_webhook_bearer};
use super::protocol::{
    CallToolResult, InitializeResult, JsonRpcRequest, JsonRpcResponse, ToolsListResult,
};
use super::sync_manager::SyncScheduler;
use super::tools;

// ── Shared state ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub ctx: Arc<Mutex<AppContext>>,
    pub bearer_token: String,
    pub webhook_token: Option<String>,
    pub scheduler: SyncScheduler,
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn build_router(state: AppState) -> Router {
    let mcp_route = Router::new()
        .route("/", post(mcp_handler))
        .layer(middleware::from_fn_with_state(state.clone(), require_mcp_bearer))
        .with_state(state.clone());

    let webhook_route = Router::new()
        .route("/webhook/sync", post(webhook_handler))
        .layer(middleware::from_fn_with_state(state.clone(), require_webhook_bearer))
        .with_state(state.clone());

    Router::new().merge(mcp_route).merge(webhook_route)
}

// ── MCP handler ───────────────────────────────────────────────────────────────

async fn mcp_handler(
    State(state): State<AppState>,
    body: String,
) -> Response {
    let req: JsonRpcRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(_) => {
            return Json(JsonRpcResponse::parse_error()).into_response();
        }
    };

    if req.jsonrpc != "2.0" {
        return Json(JsonRpcResponse::err(
            req.id,
            -32600,
            "invalid jsonrpc version — expected \"2.0\"",
        ))
        .into_response();
    }

    let response = match req.method.as_str() {
        "initialize" => handle_initialize(req.id),
        "tools/list"  => handle_tools_list(req.id),
        "tools/call"  => handle_tools_call(req.id, req.params, &state).await,
        other => JsonRpcResponse::method_not_found(req.id, other),
    };

    Json(response).into_response()
}

fn handle_initialize(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::ok(id, json!(InitializeResult::new()))
}

fn handle_tools_list(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        json!(ToolsListResult { tools: tools::all_tools() }),
    )
}

async fn handle_tools_call(
    id: Option<Value>,
    params: Option<Value>,
    state: &AppState,
) -> JsonRpcResponse {
    let params = match params {
        Some(p) => p,
        None => return JsonRpcResponse::invalid_params(id, "params is required for tools/call"),
    };

    let tool_name = match params.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_owned(),
        None => return JsonRpcResponse::invalid_params(id, "params.name is required"),
    };

    let tool_params = params.get("arguments").cloned().unwrap_or(json!({}));

    let ctx = state.ctx.clone();
    let scheduler = state.scheduler.clone();
    let result: CallToolResult = tokio::task::spawn_blocking(move || {
        let mut ctx = ctx.blocking_lock();
        tools::dispatch(&tool_name, &tool_params, &mut ctx, &scheduler)
    })
    .await
    .unwrap_or_else(|e| CallToolResult::error(format!("internal panic: {e}")));

    JsonRpcResponse::ok(id, json!(result))
}

// ── Webhook handler ───────────────────────────────────────────────────────────

async fn webhook_handler(State(state): State<AppState>) -> Response {
    let ctx = state.ctx.clone();
    let scheduler = state.scheduler.clone();

    let result = tokio::task::spawn_blocking(move || {
        let mut ctx = ctx.blocking_lock();
        let sync_result = crate::mcp::sync_manager::do_sync(&mut ctx);
        scheduler.cancel(); // clear any pending deferred timer
        sync_result
    })
    .await;

    match result {
        Ok(Ok(())) => Json(json!({ "status": "synced" })).into_response(),
        Ok(Err(e)) => (
            StatusCode::OK,
            Json(json!({ "status": "error", "message": e.to_string() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "status": "error", "message": format!("internal error: {e}") })),
        )
            .into_response(),
    }
}
