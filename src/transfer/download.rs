//! Streaming file responses for both the download and preview endpoints.

use std::io::SeekFrom;
use std::time::UNIX_EPOCH;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use super::range::{RangeOutcome, parse as parse_range};
use crate::files::ResolvedFile;
use crate::server::error::ApiError;

/// Read chunk size. Large enough that a multi-GB video does not spend its
/// time in syscalls, small enough that memory per connection stays trivial.
const CHUNK_SIZE: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Force a save: this is what makes Safari put the file in Downloads /
    /// the Files app instead of playing it.
    Attachment,
    /// Let the browser render it (video player, image, PDF viewer).
    Inline,
}

/// Build a streaming response for `file`, honouring `Range`/`If-Range`.
pub async fn stream_file(
    file: &ResolvedFile,
    request_headers: &HeaderMap,
    disposition: Disposition,
) -> Result<Response, ApiError> {
    let mut handle = tokio::fs::File::open(&file.path).await.map_err(|e| {
        tracing::warn!(path = %file.path.display(), error = %e, "cannot open file");
        ApiError::new(StatusCode::NOT_FOUND, "file not available")
    })?;
    let meta = handle
        .metadata()
        .await
        .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, "file not available"))?;
    let size = meta.len();
    let etag = etag_for(size, &meta);

    // `If-Range` lets a client resume only if the file is unchanged. If the
    // validator does not match, the range is ignored and the full file sent.
    let range_header = request_headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .filter(|_| if_range_matches(request_headers, &etag));

    let mut headers = HeaderMap::new();
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&file.mime)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        content_disposition(disposition, &file.name),
    );
    if let Some(lm) = last_modified(&meta) {
        headers.insert(header::LAST_MODIFIED, lm);
    }

    let (status, start, len) = match parse_range(range_header, size) {
        RangeOutcome::Full => (StatusCode::OK, 0, size),
        RangeOutcome::Partial(r) => {
            headers.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {}-{}/{}", r.start, r.end, size)).unwrap(),
            );
            (StatusCode::PARTIAL_CONTENT, r.start, r.byte_count())
        }
        RangeOutcome::Unsatisfiable => {
            headers.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{size}")).unwrap(),
            );
            return Ok((StatusCode::RANGE_NOT_SATISFIABLE, headers).into_response());
        }
    };
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));

    if start > 0 {
        handle
            .seek(SeekFrom::Start(start))
            .await
            .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "seek failed"))?;
    }
    // `take` caps the stream at the requested length so a file growing while
    // it is read cannot push more bytes than Content-Length promised.
    let stream = ReaderStream::with_capacity(handle.take(len), CHUNK_SIZE);
    Ok((status, headers, Body::from_stream(stream)).into_response())
}

/// Weak-ish but adequate validator: size plus mtime. Good enough to detect
/// "the file was replaced" for resume purposes without hashing gigabytes.
fn etag_for(size: u64, meta: &std::fs::Metadata) -> String {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("\"{size:x}-{mtime:x}\"")
}

fn if_range_matches(headers: &HeaderMap, etag: &str) -> bool {
    match headers.get(header::IF_RANGE).and_then(|v| v.to_str().ok()) {
        None => true,
        // Only entity-tag validators are supported; a date is treated as a
        // mismatch, which is the safe direction (full response).
        Some(v) => v.trim() == etag,
    }
}

fn last_modified(meta: &std::fs::Metadata) -> Option<HeaderValue> {
    let t = meta.modified().ok()?;
    HeaderValue::from_str(&httpdate::fmt_http_date(t)).ok()
}

/// Characters that must be percent-encoded in an RFC 8187 `filename*`
/// value: everything except `attr-char`.
const ATTR_CHAR_ENCODE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'!')
    .remove(b'#')
    .remove(b'$')
    .remove(b'&')
    .remove(b'+')
    .remove(b'-')
    .remove(b'.')
    .remove(b'^')
    .remove(b'_')
    .remove(b'`')
    .remove(b'|')
    .remove(b'~');

/// Build `Content-Disposition` with both an ASCII fallback and the UTF-8
/// `filename*` form so `Füße 🎬.mov` survives the trip to iOS Files intact.
pub fn content_disposition(disposition: Disposition, name: &str) -> HeaderValue {
    let kind = match disposition {
        Disposition::Attachment => "attachment",
        Disposition::Inline => "inline",
    };
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded = utf8_percent_encode(name, ATTR_CHAR_ENCODE);
    let value = format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{encoded}");
    HeaderValue::from_str(&value).expect("percent-encoded value is ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_disposition_handles_unicode_and_quotes() {
        let v = content_disposition(Disposition::Attachment, "Füße \"x\".mov");
        assert_eq!(
            v.to_str().unwrap(),
            "attachment; filename=\"F__e _x_.mov\"; filename*=UTF-8''F%C3%BC%C3%9Fe%20%22x%22.mov"
        );
        let v = content_disposition(Disposition::Inline, "plain.pdf");
        assert_eq!(
            v.to_str().unwrap(),
            "inline; filename=\"plain.pdf\"; filename*=UTF-8''plain.pdf"
        );
    }
}
