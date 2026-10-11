//! HTTP handlers. Thin by design: they parse the request, call into the
//! auth/files/transfer modules and shape the response.

use axum::Json;
use axum::extract::{Multipart, Path, Query, State};
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
use crate::transfer::thumbnail::{Size, ThumbError};
use crate::transfer::upload::{Saved, UploadError};
use crate::utils::{activity, term};

#[derive(Serialize)]
pub struct Status {
    version: &'static str,
    authenticated: bool,
    /// Shown as the page title before login; a folder name is not a secret.
    share_name: String,
    upload_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_upload_bytes: Option<u64>,
    /// Name of the sharing computer, shown on the login screen so the
    /// person knows which machine's terminal to look at for the code.
    host_name: String,
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
        host_name: host_name(),
    })
}

#[derive(Deserialize)]
pub struct AuthRequest {
    code: String,
    /// What the browser thinks it is running on ("iPad", "Mac"). Purely
    /// cosmetic, for the activity feed; the page computes it because the
    /// User-Agent alone cannot tell an iPad from a Mac any more.
    #[serde(default)]
    device: Option<String>,
}

pub fn host_name() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

/// Reduce a client-supplied device name to something safe to print: short,
/// printable, no control characters that could mess with the terminal.
fn device_label(raw: Option<&str>, ip: std::net::IpAddr) -> String {
    let name: String = raw
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(32)
        .collect::<String>()
        .trim()
        .to_string();
    if name.is_empty() {
        format!("Device ({ip})")
    } else {
        format!("{name} ({ip})")
    }
}

pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(body): Json<AuthRequest>,
) -> Result<Response, ApiError> {
    let device = device_label(body.device.as_deref(), ip);
    match state.auth.login(ip, &body.code, device.clone()) {
        Ok(token) => {
            info!(%ip, "login ok");
            activity::info(format!("{} connected", term::bold(&device)));
            let mut response = (StatusCode::NO_CONTENT, ()).into_response();
            response
                .headers_mut()
                .insert(header::SET_COOKIE, session_cookie(&token, state.scheme));
            Ok(response)
        }
        Err(LoginError::WrongPin) => {
            info!(%ip, "wrong access code");
            activity::problem(format!("Wrong code from {device}"));
            Err(ApiError::new(StatusCode::UNAUTHORIZED, "wrong code"))
        }
        Err(LoginError::Throttled(wait)) => {
            info!(%ip, "login throttled");
            let secs = wait.as_secs().max(1);
            activity::problem(format!("{device} locked out for {secs} s"));
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
        if let Some(device) = state.auth.sessions.device(&token) {
            activity::info(format!("{device} logged out"));
        }
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
    let mut files = tokio::task::spawn_blocking(move || share.list())
        .await
        .unwrap_or_default();
    if let Some(thumbs) = &state.thumbs {
        for f in &mut files {
            f.thumbnail = thumbs.supports(&f.mime);
        }
    }
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
    let device = session_token(&headers)
        .and_then(|t| state.auth.sessions.device(&t))
        .unwrap_or_else(|| "a device".into());
    let size = tokio::fs::metadata(&file.path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    // Safari fetches big files in several ranges; announce only the first.
    let first_request = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|r| r.starts_with("bytes=0-"))
        .unwrap_or(true);
    if first_request {
        activity::info(format!(
            "{device} started downloading {} ({})",
            term::bold(&file.name),
            term::human_size(size)
        ));
    }
    let started = std::time::Instant::now();
    let done = state.clone();
    let name = file.name.clone();
    let on_complete = Box::new(move || {
        info!(name = %name, "download complete");
        activity::done(format!(
            "{} sent to {device} · {} in {}",
            term::bold(&name),
            term::human_size(size),
            term::human_duration(started.elapsed())
        ));
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

#[derive(Deserialize)]
pub struct ThumbQuery {
    #[serde(default)]
    size: Option<String>,
}

/// Small JPEG rendition of an image. Unlike the other file routes this one
/// is cacheable: the URL carries the file's size and mtime from the
/// listing, so a changed file gets a new URL and a day of `max-age` is safe.
pub async fn thumbnail(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ThumbQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let thumbs = state
        .thumbs
        .as_ref()
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "thumbnails disabled"))?;
    let file = state
        .share
        .resolve(&id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such file"))?;
    let size = match query.size.as_deref() {
        Some("large") => Size::Large,
        _ => Size::Small,
    };
    let thumb = thumbs
        .get(&file.path, &file.mime, size)
        .await
        .map_err(|e| {
            let status = match e {
                ThumbError::Unsupported | ThumbError::TooLarge => {
                    StatusCode::UNSUPPORTED_MEDIA_TYPE
                }
                ThumbError::Io(_) => StatusCode::NOT_FOUND,
                ThumbError::Decode(_) | ThumbError::Tool(_) => StatusCode::UNPROCESSABLE_ENTITY,
            };
            ApiError::new(status, e.to_string())
        })?;

    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == thumb.etag))
    {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg")),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, max-age=86400"),
            ),
            (
                header::ETAG,
                HeaderValue::from_str(&thumb.etag).expect("etag is ASCII"),
            ),
        ],
        thumb.jpeg,
    )
        .into_response())
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
        let device = session_token(&headers)
            .and_then(|t| state.auth.sessions.device(&t))
            .unwrap_or_else(|| "a device".into());
        let total: u64 = outcome.saved.iter().map(|s| s.size).sum();
        let mut names: Vec<&str> = outcome
            .saved
            .iter()
            .map(|s| s.name.as_str())
            .take(3)
            .collect();
        if outcome.saved.len() > 3 {
            names.push("…");
        }
        activity::done(format!(
            "Received {} from {device} · {} · {}",
            term::bold(&match outcome.saved.len() {
                1 => "1 file".to_string(),
                n => format!("{n} files"),
            }),
            term::human_size(total),
            names.join(", ")
        ));
        state.transfer_completed("upload");
    }
    for failure in &outcome.failed {
        activity::problem(format!(
            "Upload of {} failed: {}",
            failure.name, failure.error
        ));
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
