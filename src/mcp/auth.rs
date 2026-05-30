use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};

use super::server::AppState;

/// Middleware that requires a valid `Authorization: Bearer <token>` header.
pub async fn require_mcp_bearer(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    check_bearer(req.headers(), &state.bearer_token)?;
    Ok(next.run(req).await)
}

/// Middleware that requires the webhook-specific bearer token.
pub async fn require_webhook_bearer(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = state.webhook_token.as_deref().ok_or(StatusCode::NOT_FOUND)?;
    check_bearer(req.headers(), token)?;
    Ok(next.run(req).await)
}

fn check_bearer(headers: &axum::http::HeaderMap, expected: &str) -> Result<(), StatusCode> {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let provided = auth.strip_prefix("Bearer ").ok_or(StatusCode::UNAUTHORIZED)?;

    // Constant-time comparison to resist timing attacks.
    if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

/// Simple constant-time byte-slice comparison (no external crate needed).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches() {
        assert!(constant_time_eq(b"secret", b"secret"));
    }

    #[test]
    fn constant_time_eq_differs() {
        assert!(!constant_time_eq(b"secret", b"wrong!"));
    }

    #[test]
    fn constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"short", b"longer"));
    }
}
