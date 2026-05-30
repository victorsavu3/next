use axum::{
    body::Body,
    extract::State,
    http::{HeaderValue, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::server::AppState;

const RESOURCE_METADATA_URL: &str =
    "https://next-mcp.victorsavu.eu/.well-known/oauth-protected-resource";

/// Middleware that requires a valid `Authorization: Bearer <token>` header.
/// Returns 401 with a `WWW-Authenticate` header pointing to the protected-resource
/// metadata document so OAuth clients can discover the authorization server.
pub async fn require_mcp_bearer(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if check_bearer(req.headers(), &state.bearer_token).is_err() {
        return unauthorized_response();
    }
    next.run(req).await
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

fn unauthorized_response() -> Response {
    let www_auth = format!(r#"Bearer resource_metadata="{RESOURCE_METADATA_URL}""#);
    let mut resp = StatusCode::UNAUTHORIZED.into_response();
    resp.headers_mut().insert(
        axum::http::header::WWW_AUTHENTICATE,
        HeaderValue::from_str(&www_auth).unwrap(),
    );
    resp
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

/// Constant-time byte-slice comparison that does not leak the length of `b`
/// (the expected secret) via response-time differences.
///
/// Always processes every byte of `b`, using 0 for out-of-bounds positions of `a`,
/// so timing is O(b.len()) regardless of how long `a` is.  The final length check
/// leaks only the length of `a`, which the caller already knows they sent.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mismatch = b
        .iter()
        .enumerate()
        .fold(0u8, |acc, (i, &expected)| {
            acc | (a.get(i).copied().unwrap_or(0) ^ expected)
        });
    mismatch == 0 && a.len() == b.len()
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

    #[test]
    fn constant_time_eq_truncated() {
        assert!(!constant_time_eq(b"secre", b"secret"));
    }

    #[test]
    fn constant_time_eq_extended() {
        assert!(!constant_time_eq(b"secretX", b"secret"));
    }

    #[test]
    fn constant_time_eq_empty_submitted() {
        assert!(!constant_time_eq(b"", b"secret"));
    }
}
