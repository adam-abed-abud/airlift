//! Integration tests that exercise the HTTP API end to end through the
//! router, without binding a port. Each test builds its own share in a
//! temporary directory.

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use http_body_util::BodyExt;
use lanshare::auth::{Auth, pin::Pin};
use lanshare::files::Share;
use lanshare::server;
use lanshare::transfer::upload::UploadConfig;
use tempfile::TempDir;
use tower::ServiceExt;

const PIN: &str = "482913";
const HOST: &str = "192.168.1.5:8080";
const ORIGIN: &str = "http://192.168.1.5:8080";

struct Fixture {
    app: Router,
    #[allow(dead_code)]
    share_dir: TempDir,
    upload_dir: Option<TempDir>,
    shutdown: tokio_util::sync::CancellationToken,
}

fn fixture(uploads: bool) -> Fixture {
    fixture_with(uploads, false)
}

fn fixture_with(uploads: bool, once: bool) -> Fixture {
    let share_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        share_dir.path().join("hello.txt"),
        b"hello world 0123456789",
    )
    .unwrap();
    std::fs::write(share_dir.path().join("archive.zip"), b"PK\x03\x04").unwrap();
    std::fs::create_dir(share_dir.path().join("sub")).unwrap();
    std::fs::write(share_dir.path().join("sub/clip.mp4"), vec![7u8; 4096]).unwrap();

    let upload_dir = uploads.then(|| tempfile::tempdir().unwrap());
    let upload = upload_dir
        .as_ref()
        .map(|d| UploadConfig::new(d.path(), 1024 * 1024).unwrap());

    let share = Share::directories(&[share_dir.path().to_path_buf()], false).unwrap();
    let auth = Auth::new(Pin::from_digits(PIN).unwrap());
    let state = server::state(auth, share, upload, once);
    let shutdown = state.shutdown.clone();
    let app = server::router(state);
    Fixture {
        app,
        share_dir,
        upload_dir,
        shutdown,
    }
}

async fn send(app: &Router, req: Request<Body>) -> Response {
    app.clone().oneshot(req).await.unwrap()
}

async fn body_bytes(res: Response) -> Bytes {
    res.into_body().collect().await.unwrap().to_bytes()
}

async fn body_json(res: Response) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(res).await).unwrap()
}

fn get(path: &str, cookie: Option<&str>) -> Request<Body> {
    let mut req = Request::get(path).header(header::HOST, HOST);
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    req.body(Body::empty()).unwrap()
}

fn login_request(code: &str, with_origin: bool) -> Request<Body> {
    let mut req = Request::post("/api/auth")
        .header(header::HOST, HOST)
        .header(header::CONTENT_TYPE, "application/json");
    if with_origin {
        req = req.header(header::ORIGIN, ORIGIN);
    }
    req.body(Body::from(format!(r#"{{"code":"{code}"}}"#)))
        .unwrap()
}

/// Log in and return the `Cookie` header value to send back.
async fn login(app: &Router) -> String {
    let res = send(app, login_request(PIN, true)).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let set_cookie = res.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_string();
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    assert!(set_cookie.contains("SameSite=Strict"), "{set_cookie}");
    set_cookie.split(';').next().unwrap().to_string()
}

async fn file_id(app: &Router, cookie: &str, name: &str) -> String {
    let list = body_json(send(app, get("/api/files", Some(cookie))).await).await;
    list["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == name)
        .map(|f| f["id"].as_str().unwrap().to_string())
        .unwrap_or_else(|| panic!("{name} not listed in {list}"))
}

#[tokio::test]
async fn status_is_public_and_files_are_not() {
    let f = fixture(false);
    let res = send(&f.app, get("/api/status", None)).await;
    assert_eq!(res.status(), StatusCode::OK);
    let status = body_json(res).await;
    assert_eq!(status["authenticated"], false);
    assert_eq!(status["upload_enabled"], false);

    for path in [
        "/api/files",
        "/api/files/abc/download",
        "/api/files/abc/preview",
    ] {
        let res = send(&f.app, get(path, None)).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    let res = send(&f.app, get("/api/files", Some("lanshare_session=forged"))).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_rejects_wrong_code_and_missing_origin() {
    let f = fixture(false);
    let res = send(&f.app, login_request("000000", true)).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Right code but no Origin: this is what a cross-site form post would
    // look like, and it must not mint a session.
    let res = send(&f.app, login_request(PIN, false)).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn login_is_throttled() {
    let f = fixture(false);
    let mut last = StatusCode::OK;
    for _ in 0..6 {
        last = send(&f.app, login_request("111111", true)).await.status();
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    // Even the right code is refused while locked out.
    let res = send(&f.app, login_request(PIN, true)).await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(res.headers().contains_key(header::RETRY_AFTER));
}

#[tokio::test]
async fn listing_download_and_range() {
    let f = fixture(false);
    let cookie = login(&f.app).await;

    let list = body_json(send(&f.app, get("/api/files", Some(&cookie))).await).await;
    let names: Vec<_> = list["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["archive.zip", "hello.txt", "sub/clip.mp4"]);

    let id = file_id(&f.app, &cookie, "hello.txt").await;
    let res = send(
        &f.app,
        get(&format!("/api/files/{id}/download"), Some(&cookie)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::ACCEPT_RANGES], "bytes");
    assert_eq!(res.headers()[header::CONTENT_LENGTH], "22");
    assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
    assert!(
        res.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment;")
    );
    assert_eq!(&body_bytes(res).await[..], b"hello world 0123456789");

    let mut req = get(&format!("/api/files/{id}/download"), Some(&cookie));
    req.headers_mut()
        .insert(header::RANGE, "bytes=6-10".parse().unwrap());
    let res = send(&f.app, req).await;
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 6-10/22");
    assert_eq!(&body_bytes(res).await[..], b"world");

    let mut req = get(&format!("/api/files/{id}/download"), Some(&cookie));
    req.headers_mut()
        .insert(header::RANGE, "bytes=500-".parse().unwrap());
    let res = send(&f.app, req).await;
    assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE);

    let res = send(
        &f.app,
        get("/api/files/doesnotexist/download", Some(&cookie)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn preview_only_for_renderable_types() {
    let f = fixture(false);
    let cookie = login(&f.app).await;

    let id = file_id(&f.app, &cookie, "clip.mp4").await;
    let res = send(
        &f.app,
        get(&format!("/api/files/{id}/preview"), Some(&cookie)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "video/mp4");
    assert!(
        res.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("inline;")
    );

    let id = file_id(&f.app, &cookie, "archive.zip").await;
    let res = send(
        &f.app,
        get(&format!("/api/files/{id}/preview"), Some(&cookie)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn logout_revokes_session() {
    let f = fixture(false);
    let cookie = login(&f.app).await;
    assert_eq!(
        send(&f.app, get("/api/files", Some(&cookie)))
            .await
            .status(),
        StatusCode::OK
    );

    let req = Request::post("/api/logout")
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .unwrap();
    let res = send(&f.app, req).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert!(
        res.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );

    assert_eq!(
        send(&f.app, get("/api/files", Some(&cookie)))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

fn multipart(parts: &[(&str, &[u8])]) -> (String, Vec<u8>) {
    let boundary = "----lanshare-test-boundary";
    let mut body = Vec::new();
    for (filename, data) in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn upload_request(cookie: &str, parts: &[(&str, &[u8])]) -> Request<Body> {
    let (content_type, body) = multipart(parts);
    Request::post("/api/upload")
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn upload_route_absent_when_disabled() {
    let f = fixture(false);
    let cookie = login(&f.app).await;
    let res = send(&f.app, upload_request(&cookie, &[("a.txt", b"a")])).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn upload_saves_sanitises_and_never_overwrites() {
    let f = fixture(true);
    let dir = f.upload_dir.as_ref().unwrap().path();
    let cookie = login(&f.app).await;

    // Unauthenticated upload must fail before any bytes are written.
    let mut req = upload_request(&cookie, &[("a.txt", b"a")]);
    req.headers_mut().remove(header::COOKIE);
    assert_eq!(send(&f.app, req).await.status(), StatusCode::UNAUTHORIZED);

    let res = send(
        &f.app,
        upload_request(
            &cookie,
            &[("photo.heic", b"heic-bytes"), ("../../evil.sh", b"#!")],
        ),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let out = body_json(res).await;
    let saved: Vec<_> = out["saved"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(saved, vec!["photo.heic", "evil.sh"]);
    assert_eq!(
        std::fs::read(dir.join("photo.heic")).unwrap(),
        b"heic-bytes"
    );
    assert!(dir.join("evil.sh").exists());

    // Same name again: original untouched, new file gets a suffix.
    let res = send(
        &f.app,
        upload_request(&cookie, &[("photo.heic", b"second")]),
    )
    .await;
    let out = body_json(res).await;
    assert_eq!(out["saved"][0]["name"], "photo (1).heic");
    assert_eq!(
        std::fs::read(dir.join("photo.heic")).unwrap(),
        b"heic-bytes"
    );

    // No temp files left behind.
    let leftovers: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
        .collect();
    assert!(leftovers.is_empty());
}

#[tokio::test]
async fn upload_over_limit_is_rejected() {
    let f = fixture(true);
    let cookie = login(&f.app).await;
    let big = vec![0u8; 2 * 1024 * 1024];
    let res = send(&f.app, upload_request(&cookie, &[("big.bin", &big)])).await;
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        std::fs::read_dir(f.upload_dir.as_ref().unwrap().path())
            .unwrap()
            .next()
            .is_none()
    );
}

#[tokio::test]
async fn page_is_served_with_hardening_headers() {
    let f = fixture(false);
    let res = send(&f.app, get("/", None)).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert!(res.headers().contains_key(header::CONTENT_SECURITY_POLICY));
    assert_eq!(res.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    let html = String::from_utf8(body_bytes(res).await.to_vec()).unwrap();
    assert!(html.contains("Access code"));

    assert_eq!(
        send(&f.app, get("/app.js", None)).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        send(&f.app, get("/nope.js", None)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&f.app, get("/api/nope", None)).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn once_stops_only_after_a_complete_download() {
    let f = fixture_with(true, true);
    let cookie = login(&f.app).await;
    let id = file_id(&f.app, &cookie, "clip.mp4").await;

    // Watching a preview is not a transfer.
    let res = send(
        &f.app,
        get(&format!("/api/files/{id}/preview"), Some(&cookie)),
    )
    .await;
    body_bytes(res).await;
    assert!(!f.shutdown.is_cancelled());

    // A partial range that does not reach the end is not complete either.
    let mut req = get(&format!("/api/files/{id}/download"), Some(&cookie));
    req.headers_mut()
        .insert(header::RANGE, "bytes=0-99".parse().unwrap());
    body_bytes(send(&f.app, req).await).await;
    assert!(!f.shutdown.is_cancelled());

    // A download whose body is dropped before the end never completes.
    let res = send(
        &f.app,
        get(&format!("/api/files/{id}/download"), Some(&cookie)),
    )
    .await;
    drop(res);
    assert!(!f.shutdown.is_cancelled());

    // Reading the last range to EOF completes the transfer.
    let mut req = get(&format!("/api/files/{id}/download"), Some(&cookie));
    req.headers_mut()
        .insert(header::RANGE, "bytes=100-".parse().unwrap());
    body_bytes(send(&f.app, req).await).await;
    assert!(f.shutdown.is_cancelled());
}

#[tokio::test]
async fn once_stops_after_a_saved_upload() {
    let f = fixture_with(true, true);
    let cookie = login(&f.app).await;
    body_bytes(send(&f.app, upload_request(&cookie, &[("a.txt", b"a")])).await).await;
    assert!(f.shutdown.is_cancelled());
}

#[tokio::test]
async fn without_once_nothing_stops() {
    let f = fixture(false);
    let cookie = login(&f.app).await;
    let id = file_id(&f.app, &cookie, "hello.txt").await;
    body_bytes(
        send(
            &f.app,
            get(&format!("/api/files/{id}/download"), Some(&cookie)),
        )
        .await,
    )
    .await;
    assert!(!f.shutdown.is_cancelled());
}
