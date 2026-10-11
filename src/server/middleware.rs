//! Request-level guards: client address, session cookie and CSRF checks.

use std::net::{IpAddr, SocketAddr};

use axum::body::Body;
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::error::ApiError;
use super::state::AppState;
use crate::auth::COOKIE_NAME;

/// Peer IP of the request, used for throttling. Falls back to loopback when
/// the router is driven without a real socket (tests), rather than failing.
pub struct ClientIp(pub IpAddr);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip())
            .unwrap_or(IpAddr::from([127, 0, 0, 1]));
        Ok(ClientIp(ip))
    }
}

/// Extract the session token from the `Cookie` header, if any.
pub fn session_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .map(str::trim)
        .find_map(|kv| kv.strip_prefix(COOKIE_NAME)?.strip_prefix('='))
        .map(str::to_string)
}

/// Reject requests without a live session. Applied to every file/upload
/// route; the login and status routes are mounted outside it.
pub async fn require_session(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let valid = session_token(request.headers())
        .map(|t| state.auth.sessions.validate(&t))
        .unwrap_or(false);
    if !valid {
        return ApiError::new(StatusCode::UNAUTHORIZED, "login required").into_response();
    }
    next.run(request).await
}

/// CSRF defence for state-changing requests.
///
/// The session cookie is `SameSite=Strict`, which already stops browsers from
/// attaching it to cross-site requests. This is the second layer: every POST
/// must carry an `Origin` header that matches the host the request arrived
/// on. Browsers always send `Origin` on cross-origin and on fetch/XHR POSTs,
/// and a page served by us has, by construction, our origin.
pub async fn same_origin_only(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        let headers = request.headers();
        let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
        let origin = headers.get(header::ORIGIN).and_then(|h| h.to_str().ok());
        let expected = host.map(|h| format!("{}://{h}", state.scheme.as_str()));
        let ok = matches!((origin, expected.as_deref()), (Some(o), Some(e)) if o.eq_ignore_ascii_case(e));
        if !ok {
            return ApiError::new(StatusCode::FORBIDDEN, "cross-origin request rejected")
                .into_response();
        }
    }
    next.run(request).await
}
