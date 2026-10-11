//! HTTP handlers. Thin by design: they parse the request, call into the
//! auth/files/transfer modules and shape the response.

use axum::Json;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::error::ApiError;
use super::middleware::{ClientIp, session_token};
use super::state::{AppState, Scheme};
use crate::auth::{COOKIE_NAME, LoginError, session::MAX_LIFETIME};
use crate::files::FileEntry;
use crate::transfer::download::{Disposition, stream_file};
use crate::transfer::upload::{Saved, UploadError};

#[derive(Serialize)]
pub struct Status {
    version: &'static str,
    authenticated: bool,
    /// Shown as the page title before login; a folder name is not a secret.
    share_name: String,
    upload_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_upload_bytes: Option<u64>,
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
        share_name: state.share.display_name(),
        upload_enabled: state.upload.is_some(),
        max_upload_bytes: state.upload.as_ref().map(|u| u.max_bytes),
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
    let secure = if scheme == Scheme::Https {
        "; Secure"
    } else {
        ""
    };
    let value = format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
        MAX_LIFETIME.as_secs()
    );
    HeaderValue::from_str(&value).expect("cookie is ASCII")
}

fn clear_cookie(scheme: Scheme) -> HeaderValue {
    let secure = if scheme == Scheme::Https {
        "; Secure"
    } else {
        ""
    };
    let value = format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}");
    HeaderValue::from_str(&value).expect("cookie is ASCII")
}

#[derive(Serialize)]
pub struct FileList {
    files: Vec<FileEntry>,
    truncated: bool,
}

pub async fn list_files(State(state): State<AppState>) -> Json<FileList> {
    // Walking the tree is blocking I/O; keep it off the async workers so a
    // slow disk cannot stall the download streams of other clients.
    let share = state.share.clone();
    let files = tokio::task::spawn_blocking(move || share.list())
        .await
        .unwrap_or_default();
    let truncated = files.len() >= crate::files::MAX_ENTRIES;
    Json(FileList { files, truncated })
}

pub async fn download(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let file = state
        .share
        .resolve(&id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such file"))?;
    info!(name = %file.name, "download");
    let done = state.clone();
    let name = file.name.clone();
    let on_complete = Box::new(move || {
        info!(name = %name, "download complete");
        done.transfer_completed("download");
    });
    stream_file(&file, &headers, Disposition::Attachment, Some(on_complete)).await
}

pub async fn preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let file = state
        .share
        .resolve(&id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such file"))?;
    if !crate::files::is_previewable(&file.mime) {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "no preview for this type",
        ));
    }
    // Previews never count as a transfer: watching is not receiving.
    stream_file(&file, &headers, Disposition::Inline, None).await
}

#[derive(Serialize)]
pub struct UploadOutcome {
    saved: Vec<Saved>,
    failed: Vec<UploadFailure>,
}

#[derive(Serialize)]
pub struct UploadFailure {
    name: String,
    error: String,
}

/// Accept one or more files in a multipart body. Each file is committed
/// independently so one bad file does not lose the others in the batch.
pub async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<UploadOutcome>, ApiError> {
    let config = state
        .upload
        .as_ref()
        .ok_or_else(|| ApiError::new(StatusCode::FORBIDDEN, "uploads are disabled"))?;

    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    config.check_space(declared).map_err(upload_error)?;

    let mut outcome = UploadOutcome {
        saved: Vec::new(),
        failed: Vec::new(),
    };
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(e) => {
                // The body limit layer surfaces here as a multipart error once
                // the stream is cut off; report what we did manage to save.
                warn!(error = %e.body_text(), "multipart read failed");
                if outcome.saved.is_empty() {
                    return Err(ApiError::new(e.status(), e.body_text()));
                }
                outcome.failed.push(UploadFailure {
                    name: "(remaining files)".into(),
                    error: e.body_text(),
                });
                break;
            }
        };
        if field.file_name().is_none() {
            continue; // Not a file part; nothing else is expected, skip it.
        }
        let name = field.file_name().unwrap_or("upload").to_string();
        match config.save_field(field).await {
            Ok(saved) => outcome.saved.push(saved),
            Err(e) => {
                warn!(name = %name, error = %e, "upload failed");
                outcome.failed.push(UploadFailure {
                    name,
                    error: e.to_string(),
                });
                // Disk-level failures will not get better for the next file.
                if matches!(e, UploadError::DiskFull | UploadError::TooLarge) {
                    break;
                }
            }
        }
    }
    if !outcome.saved.is_empty() {
        state.transfer_completed("upload");
    }
    Ok(Json(outcome))
}

fn upload_error(e: UploadError) -> ApiError {
    let status = match e {
        UploadError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        UploadError::DiskFull => StatusCode::INSUFFICIENT_STORAGE,
        UploadError::Read(_) => StatusCode::BAD_REQUEST,
        UploadError::Write(_) | UploadError::Collision => StatusCode::INTERNAL_SERVER_ERROR,
    };
    ApiError::new(status, e.to_string())
}
