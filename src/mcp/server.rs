use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, Form, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
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
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
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
        .route("/oauth/authorize", get(oauth_authorize))
        .route("/oauth/token", post(oauth_token))
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

async fn oauth_metadata(headers: HeaderMap) -> impl IntoResponse {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    let base = format!("{proto}://{host}");

    let mut resp = Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
    }))
    .into_response();
    resp.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    resp
}

#[derive(Deserialize)]
struct AuthorizeParams {
    response_type: String,
    redirect_uri: String,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
}

async fn oauth_authorize(
    State(app): State<AppState>,
    Query(params): Query<AuthorizeParams>,
) -> Response {
    if params.response_type != "code" {
        return (StatusCode::BAD_REQUEST, "unsupported_response_type").into_response();
    }

    let code = uuid::Uuid::new_v4().to_string().replace('-', "");

    app.oauth_codes.lock().await.insert(
        code.clone(),
        PendingAuth {
            code_challenge: params.code_challenge,
            code_challenge_method: params.code_challenge_method,
        },
    );

    let mut url = format!("{}?code={}", params.redirect_uri, code);
    if let Some(s) = &params.state {
        url.push_str("&state=");
        url.push_str(s);
    }

    Redirect::to(&url).into_response()
}

#[derive(Deserialize)]
struct TokenForm {
    grant_type: String,
    code: Option<String>,
    code_verifier: Option<String>,
}

async fn oauth_token(
    State(app): State<AppState>,
    Form(form): Form<TokenForm>,
) -> Response {
    if form.grant_type != "authorization_code" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "unsupported_grant_type"})),
        )
            .into_response();
    }

    let code = match form.code {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_request", "error_description": "missing code"})),
            )
                .into_response();
        }
    };

    let pending = app.oauth_codes.lock().await.remove(&code);
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

    // Verify PKCE when the authorize request included a challenge.
    if let Some(challenge) = pending.code_challenge {
        let verifier = match form.code_verifier {
            Some(v) => v,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "invalid_request", "error_description": "code_verifier required"})),
                )
                    .into_response();
            }
        };
        let method = pending.code_challenge_method.as_deref().unwrap_or("S256");
        let valid = match method {
            "S256" => verify_pkce_s256(&verifier, &challenge),
            "plain" => verifier == challenge,
            _ => false,
        };
        if !valid {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant"})),
            )
                .into_response();
        }
    }

    let mut resp = Json(json!({
        "access_token": app.bearer_token,
        "token_type": "bearer",
    }))
    .into_response();
    resp.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    resp
}

fn verify_pkce_s256(verifier: &str, challenge: &str) -> bool {
    use base64::Engine as _;
    use sha2::Digest as _;
    let hash = sha2::Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash) == challenge
}
