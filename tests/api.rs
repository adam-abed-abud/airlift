//! Integration tests that exercise the HTTP API end to end through the
//! router, without binding a port. Each test builds its own share in a
//! temporary directory.

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use http_body_util::BodyExt;
use airlift::auth::{Auth, pin::Pin};
use airlift::files::Share;
use airlift::server;
use airlift::transfer::thumbnail::{HeicTool, Thumbnailer, VideoTool};
use airlift::transfer::upload::UploadConfig;
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
    fixture_full(uploads, once, None)
}

fn fixture_full(uploads: bool, once: bool, heic_tool: Option<HeicTool>) -> Fixture {
    fixture_tools(uploads, once, heic_tool, None)
}

fn fixture_tools(
    uploads: bool,
    once: bool,
    heic_tool: Option<HeicTool>,
    video_tool: Option<VideoTool>,
) -> Fixture {
    let share_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        share_dir.path().join("hello.txt"),
        b"hello world 0123456789",
    )
    .unwrap();
    std::fs::write(share_dir.path().join("archive.zip"), b"PK\x03\x04").unwrap();
    std::fs::create_dir(share_dir.path().join("sub")).unwrap();
    std::fs::write(share_dir.path().join("sub/clip.mp4"), vec![7u8; 4096]).unwrap();
    std::fs::write(share_dir.path().join("photo.png"), test_png(800, 400)).unwrap();
    std::fs::write(share_dir.path().join("IMG_0001.HEIC"), b"not really heic").unwrap();

    let upload_dir = uploads.then(|| tempfile::tempdir().unwrap());
    let upload = upload_dir
        .as_ref()
        .map(|d| UploadConfig::new(d.path(), 1024 * 1024).unwrap());

    let share = Share::directories(&[share_dir.path().to_path_buf()], false).unwrap();
    let auth = Auth::new(Pin::from_digits(PIN).unwrap());
    let thumbs = Some(Thumbnailer::new(heic_tool, video_tool));
    let state = server::state(auth, share, upload, thumbs, once);
    let shutdown = state.shutdown.clone();
    let app = server::router(state);
    Fixture {
        app,
        share_dir,
        upload_dir,
        shutdown,
    }
}

/// A PNG with a horizontal gradient, generated in-process so tests carry
/// no binary fixtures.
fn test_png(width: u32, height: u32) -> Vec<u8> {
    use image::{DynamicImage, ImageBuffer, Rgb};
    let img = ImageBuffer::from_fn(width, height, |x, _| Rgb([(x % 256) as u8, 64, 128]));
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn jpeg_dimensions(data: &[u8]) -> (u32, u32) {
    image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .unwrap()
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
    let res = send(&f.app, get("/api/files", Some("airlift_session=forged"))).await;
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
    assert_eq!(
        names,
        vec![
            "IMG_0001.HEIC",
            "archive.zip",
            "hello.txt",
            "photo.png",
            "sub/clip.mp4"
        ]
    );

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
    let boundary = "----airlift-test-boundary";
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

#[tokio::test]
async fn thumbnails_for_images_with_caching_headers() {
    let f = fixture(false);
    let cookie = login(&f.app).await;

    // Listing says which entries can have one; HEIC cannot without a tool.
    let list = body_json(send(&f.app, get("/api/files", Some(&cookie))).await).await;
    let flag = |name: &str| {
        list["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == name)
            .unwrap()["thumbnail"]
            .as_bool()
            .unwrap()
    };
    assert!(flag("photo.png"));
    assert!(!flag("IMG_0001.HEIC"));
    assert!(!flag("clip.mp4"));
    assert!(!flag("archive.zip"));

    let id = file_id(&f.app, &cookie, "photo.png").await;
    assert_eq!(
        send(&f.app, get(&format!("/api/files/{id}/thumb"), None))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let res = send(
        &f.app,
        get(&format!("/api/files/{id}/thumb"), Some(&cookie)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(
        res.headers()[header::CACHE_CONTROL],
        "private, max-age=86400"
    );
    let etag = res.headers()[header::ETAG].to_str().unwrap().to_string();
    let jpeg = body_bytes(res).await;
    assert_eq!(jpeg_dimensions(&jpeg), (320, 160));

    let mut req = get(&format!("/api/files/{id}/thumb"), Some(&cookie));
    req.headers_mut()
        .insert(header::IF_NONE_MATCH, etag.parse().unwrap());
    assert_eq!(send(&f.app, req).await.status(), StatusCode::NOT_MODIFIED);

    let res = send(
        &f.app,
        get(&format!("/api/files/{id}/thumb?size=large"), Some(&cookie)),
    )
    .await;
    assert_eq!(jpeg_dimensions(&body_bytes(res).await), (800, 400));

    let zip = file_id(&f.app, &cookie, "archive.zip").await;
    let res = send(
        &f.app,
        get(&format!("/api/files/{zip}/thumb"), Some(&cookie)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
}

/// HEIC goes through an external program. Stand one in with a shell script
/// that writes a known JPEG wherever it is told to, which exercises the
/// temp-dir, argument and post-processing path without needing libheif.
#[cfg(unix)]
#[tokio::test]
async fn heic_thumbnail_via_external_tool() {
    use std::os::unix::fs::PermissionsExt;
    let tools = tempfile::tempdir().unwrap();
    let prepared = tools.path().join("prepared.jpg");
    {
        use image::{DynamicImage, ImageBuffer, Rgb};
        let img =
            ImageBuffer::from_fn(1000, 500, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 0]));
        DynamicImage::ImageRgb8(img).save(&prepared).unwrap();
    }
    // Mimics `heif-thumbnail -s SIZE INPUT OUTPUT`: copies the prepared JPEG
    // to OUTPUT and records INPUT so we can check it was a symlink to the
    // real file and not the original path.
    let script = tools.path().join("heif-thumbnail");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$3\" > {log}\ncp {prepared} \"$4\"\n",
            log = tools.path().join("input.log").display(),
            prepared = prepared.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let f = fixture_full(false, false, HeicTool::from_program(&script));
    let cookie = login(&f.app).await;
    let id = file_id(&f.app, &cookie, "IMG_0001.HEIC").await;
    let list = body_json(send(&f.app, get("/api/files", Some(&cookie))).await).await;
    assert!(
        list["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "IMG_0001.HEIC" && e["thumbnail"] == true)
    );

    let res = send(
        &f.app,
        get(&format!("/api/files/{id}/thumb"), Some(&cookie)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(jpeg_dimensions(&body_bytes(res).await), (320, 160));

    let seen = std::fs::read_to_string(tools.path().join("input.log")).unwrap();
    let seen = std::path::Path::new(seen.trim());
    assert_eq!(seen.file_name().unwrap(), "input.HEIC");
    assert_ne!(seen, f.share_dir.path().join("IMG_0001.HEIC"));
}

#[tokio::test]
async fn thumbnails_can_be_disabled() {
    let share_dir = tempfile::tempdir().unwrap();
    std::fs::write(share_dir.path().join("photo.png"), test_png(10, 10)).unwrap();
    let share = Share::directories(&[share_dir.path().to_path_buf()], false).unwrap();
    let auth = Auth::new(Pin::from_digits(PIN).unwrap());
    let app = server::router(server::state(auth, share, None, None, false));
    let cookie = login(&app).await;
    let list = body_json(send(&app, get("/api/files", Some(&cookie))).await).await;
    assert_eq!(list["files"][0]["thumbnail"], false);
    let id = list["files"][0]["id"].as_str().unwrap();
    assert_eq!(
        send(&app, get(&format!("/api/files/{id}/thumb"), Some(&cookie)))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

/// Video frames also come from an external program. A fake `ffmpeg` that
/// ignores its arguments except the output path stands in for the real one.
#[cfg(unix)]
#[tokio::test]
async fn video_thumbnail_via_external_tool() {
    use std::os::unix::fs::PermissionsExt;
    let tools = tempfile::tempdir().unwrap();
    let prepared = tools.path().join("frame.jpg");
    {
        use image::{DynamicImage, ImageBuffer, Rgb};
        let img =
            ImageBuffer::from_fn(640, 360, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 90]));
        DynamicImage::ImageRgb8(img).save(&prepared).unwrap();
    }
    // ffmpeg's output path is its last argument.
    let script = tools.path().join("ffmpeg");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nfor last; do :; done\ncp {} \"$last\"\n",
            prepared.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let f = fixture_tools(false, false, None, VideoTool::from_program(&script));
    let cookie = login(&f.app).await;
    let list = body_json(send(&f.app, get("/api/files", Some(&cookie))).await).await;
    assert!(
        list["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "clip.mp4" && e["thumbnail"] == true)
    );

    let id = file_id(&f.app, &cookie, "clip.mp4").await;
    let res = send(
        &f.app,
        get(&format!("/api/files/{id}/thumb"), Some(&cookie)),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(jpeg_dimensions(&body_bytes(res).await), (320, 180));
}
