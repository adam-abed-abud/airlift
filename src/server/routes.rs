//! HTTP handlers. Thin by design: they parse the request, call into the
//! auth/files/transfer modules and shape the response.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::error::ApiError;
use super::middleware::{ClientIp, session_token};
use super::state::{AppState, Scheme};
use crate::auth::{COOKIE_NAME, LoginError, session::MAX_LIFETIME};

#[derive(Serialize)]
pub struct Status {
    version: &'static str,
    authenticated: bool,
}

/// Public: lets the page decide whether to show the login form. Nothing here
/// is sensitive, which is the whole point of keeping it unauthenticated.
pub async fn status(State(state): State<AppState>, headers: HeaderMap) -> Json<Status> {
    let authenticated = session_token(&headers)
        .map(|t| state.auth.sessions.validate(&t))
        .unwrap_or(false);
    Json(Status {
        version: env!("CARGO_PKG_VERSION"),
        authenticated,
    })
}

#[derive(Deserialize)]
pub struct AuthRequest {
    code: String,
}

pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(body): Json<AuthRequest>,
) -> Result<Response, ApiError> {
    match state.auth.login(ip, &body.code) {
        Ok(token) => {
            info!(%ip, "login ok");
            let mut response = (StatusCode::NO_CONTENT, ()).into_response();
            response
                .headers_mut()
                .insert(header::SET_COOKIE, session_cookie(&token, state.scheme));
            Ok(response)
        }
        Err(LoginError::WrongPin) => {
            warn!(%ip, "wrong access code");
            Err(ApiError::new(StatusCode::UNAUTHORIZED, "wrong code"))
        }
        Err(LoginError::Throttled(wait)) => {
            warn!(%ip, "login throttled");
            let secs = wait.as_secs().max(1);
            Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                format!("too many attempts, try again in {secs}s"),
            )
            .with_retry_after(secs))
        }
    }
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = session_token(&headers) {
        state.auth.sessions.revoke(&token);
    }
    let mut response = (StatusCode::NO_CONTENT, ()).into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, clear_cookie(state.scheme));
    response
}

/// `HttpOnly` keeps the token away from page scripts, `SameSite=Strict`
/// stops cross-site requests from carrying it, and `Secure` is added as soon
/// as the server speaks HTTPS. `Max-Age` matches the server-side cap so the
/// browser forgets the cookie around the time it stops working anyway.
fn session_cookie(token: &str, scheme: Scheme) -> HeaderValue {
    let secure = if scheme == Scheme::Https { "; Secure" } else { "" };
    let value = format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
        MAX_LIFETIME.as_secs()
    );
    HeaderValue::from_str(&value).expect("cookie is ASCII")
}

fn clear_cookie(scheme: Scheme) -> HeaderValue {
    let secure = if scheme == Scheme::Https { "; Secure" } else { "" };
    let value = format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}");
    HeaderValue::from_str(&value).expect("cookie is ASCII")
}
