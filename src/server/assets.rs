//! Serving the embedded frontend.
//!
//! `rust-embed` compiles `web/` into the binary for release builds, which is
//! what makes LANShare a single file to copy around. In debug builds it
//! reads from disk on every request so UI edits show up with a reload.

use axum::extract::Path;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "web/"]
struct Web;

/// Browser-side hardening for the page itself. The app only ever talks to
/// its own origin, so a strict CSP costs nothing and blocks injected
/// scripts should a file name ever slip into the DOM unescaped.
const CSP: &str = "default-src 'self'; img-src 'self' blob:; media-src 'self' blob:; \
                   frame-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'";

pub async fn index() -> Response {
    serve("index.html")
}

pub async fn asset(Path(name): Path<String>) -> Response {
    serve(&name)
}

fn serve(name: &str) -> Response {
    let Some(file) = Web::get(name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = mime_guess::from_path(name)
        .first_raw()
        .unwrap_or("application/octet-stream");
    let mut response = (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(mime)),
            // Always revalidate: the binary may be replaced between runs and
            // a stale app.js talking to a newer API is a confusing failure.
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            (header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY")),
            (header::REFERRER_POLICY, HeaderValue::from_static("no-referrer")),
            (header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP)),
        ],
        file.data,
    )
        .into_response();
    let etag = format!("\"{}\"", hex(&file.metadata.sha256_hash()));
    if let Ok(v) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, v);
    }
    response
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
