use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, Form, Json as ExtractJson, Query, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Json, Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
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

struct PendingAuth {
    code_challenge: String,
}

#[derive(Clone)]
pub struct AppState {
    pub ctx: Arc<Mutex<AppContext>>,
    pub bearer_token: String,
    pub webhook_token: Option<String>,
    pub scheduler: SyncScheduler,
    oauth_codes: Arc<Mutex<HashMap<String, PendingAuth>>>,
}

impl AppState {
    pub fn new(
        ctx: Arc<Mutex<AppContext>>,
        bearer_token: String,
        webhook_token: Option<String>,
        scheduler: SyncScheduler,
    ) -> Self {
        Self {
            ctx,
            bearer_token,
            webhook_token,
            scheduler,
            oauth_codes: Arc::new(Mutex::new(HashMap::new())),
        }
    }
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

    // OAuth endpoints are public (no Bearer auth) — they ARE the auth flow.
    let oauth_routes = Router::new()
        .route("/.well-known/oauth-authorization-server", get(oauth_metadata))
        .route("/register", post(register_handler))
        .route("/authorize", get(authorize_handler))
        .route("/token", post(token_handler))
        .with_state(state.clone());

    Router::new()
        .merge(mcp_route)
        .merge(webhook_route)
        .merge(oauth_routes)
        // Limit request bodies to 64 KB — more than enough for any valid MCP request.
        .layer(DefaultBodyLimit::max(65_536))
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
    // Fail fast if a sync is already running — prevent queuing.
    let permit = match state.scheduler.try_acquire() {
        Some(p) => p,
        None => {
            return Json(json!({ "status": "error", "message": "sync already in progress" }))
                .into_response();
        }
    };

    let ctx = state.ctx.clone();
    let scheduler = state.scheduler.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut ctx = ctx.blocking_lock();
        let sync_result = crate::mcp::sync_manager::do_sync(&mut ctx);
        scheduler.cancel(); // clear any pending deferred timer
        drop(permit);
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

// ── OAuth 2.0 handlers ────────────────────────────────────────────────────────

async fn oauth_metadata() -> impl IntoResponse {
    Json(json!({
        "issuer": "https://next-mcp.victorsavu.eu",
        "authorization_endpoint": "https://next-mcp.victorsavu.eu/authorize",
        "token_endpoint": "https://next-mcp.victorsavu.eu/token",
        "registration_endpoint": "https://next-mcp.victorsavu.eu/register",
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
    }))
}

// Dynamic client registration (RFC 7591) — accepts any client, issues a UUID client_id.
// We don't validate client_id on subsequent requests so no state needs to be kept.
async fn register_handler(ExtractJson(body): ExtractJson<Value>) -> impl IntoResponse {
    let redirect_uris = body.get("redirect_uris").cloned().unwrap_or(json!([]));
    (
        StatusCode::CREATED,
        Json(json!({
            "client_id": uuid::Uuid::new_v4().to_string(),
            "client_id_issued_at": chrono::Utc::now().timestamp(),
            "redirect_uris": redirect_uris,
            "grant_types": ["authorization_code"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        })),
    )
}

#[derive(Deserialize)]
struct AuthorizeParams {
    redirect_uri: String,
    code_challenge: String,
    state: Option<String>,
}

async fn authorize_handler(
    State(app): State<AppState>,
    Query(params): Query<AuthorizeParams>,
) -> Response {
    let code = uuid::Uuid::new_v4().to_string().replace('-', "");

    app.oauth_codes.lock().await.insert(
        code.clone(),
        PendingAuth { code_challenge: params.code_challenge },
    );

    let mut url = format!("{}?code={}", params.redirect_uri, code);
    if let Some(s) = &params.state {
        url.push_str("&state=");
        url.push_str(s);
    }

    Redirect::to(&url).into_response()
}

#[derive(Deserialize)]
struct TokenRequest {
    code: String,
    code_verifier: String,
}

async fn token_handler(
    State(app): State<AppState>,
    Form(req): Form<TokenRequest>,
) -> Response {
    let pending = app.oauth_codes.lock().await.remove(&req.code);
    let pending = match pending {
        Some(p) => p,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant"})),
            )
                .into_response();
        }
    };

    if !verify_pkce_s256(&req.code_verifier, &pending.code_challenge) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        )
            .into_response();
    }

    Json(json!({
        "access_token": app.bearer_token,
        "token_type": "Bearer",
        "expires_in": 3600,
    }))
    .into_response()
}

fn verify_pkce_s256(verifier: &str, challenge: &str) -> bool {
    use base64::Engine as _;
    use sha2::Digest as _;
    let hash = sha2::Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash) == challenge
}
