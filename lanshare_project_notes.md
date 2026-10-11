# LANShare: Local File Transfer Tool

## Project idea

Build a lightweight, terminal-first, self-hosted file transfer application in **Rust**. Run it on Ubuntu (and eventually other platforms), open its web interface on an iPad/iPhone or another device on the same local network, authenticate with a temporary code, and transfer files without cloud storage or an iOS app.

**Primary goal:** Make sending a video from Ubuntu to an iPad as simple as running `lanshare send movie.mp4`, scanning a QR code, entering a PIN, and tapping **Download**.

## User experience

1. Launch on Ubuntu:
   ```bash
   lanshare send movie.mp4
   # or share a directory:
   lanshare serve ~/Videos
   ```
2. Terminal prints the local URL, a newly generated access code, and a QR code:
   ```text
   LANShare
   URL: http://192.168.1.105:8080
   Access code: 482-913
   [QR code]
   Press Ctrl+C to stop
   ```
3. Scan QR code or open URL in Safari on iPad.
4. Enter the access code to start a temporary session.
5. View available files and tap an explicit **Download** button, or choose files/photos/videos on iPad to upload to Ubuntu (when uploads are enabled).
6. End sharing by stopping the program; optionally exit automatically after the transfer completes.

The code should be freshly generated at startup, not permanently configured or embedded in the QR code by default.

## Scope and roadmap

### v0.1: Essential MVP

- Single Rust executable with a minimal responsive web interface.
- `send <file>` and `serve <directory>` CLI commands.
- Fresh temporary 6-digit access code on startup, with rate-limited authentication attempts.
- Terminal URL and QR code.
- Authenticated, expiring browser session; logout and server shutdown revoke access.
- Browse explicitly shared files, with size/type metadata.
- Explicit downloads of **any file type**: videos, photos, PDFs, archives, documents, etc.
- Stream files rather than loading them into memory; support HTTP `Range` for large media files and resumable downloads where clients support it.
- Upload files from an iPhone/iPad browser into a restricted destination, behind an explicit `--allow-upload` option.
- Basic photo and video previews, separate from downloads.
- Read-only by default, clear progress/error states and cancel controls where practical.
- Bound all file access to specified share roots; defend against traversal and symlink escapes.
- Appropriate upload limits, safe destination filenames, collision handling, and temporary-file cleanup.

### Later versions

- Multi-file selection with streamed ZIP download (store already-compressed media without recompressing).
- Resumable/chunked uploads and integrity checks (e.g. SHA-256).
- One-transfer mode (`--once`), shutdown timeouts (`--timeout 30m`), connected-device view.
- Push-style transfers: CLI announces an incoming file on an already-open web page; recipient accepts and downloads.
- Clipboard/text sharing.
- Optional device approval, discovery, and richer UI.
- Cross-platform builds for Linux, macOS, Windows, and Raspberry Pi.

## iPad/iPhone specifics

- **Downloads:** Safari can play media inline instead of downloading it. Provide separate Preview and Download actions. The download endpoint should send `Content-Disposition: attachment; filename="..."` (with correct encoding for non-ASCII names) and an appropriate content type, avoiding JavaScript blob buffering for large files. The result should appear in Safari Downloads / the iOS Files app.
- **Uploads:** Use standard browser file inputs so iOS can offer Photo Library, Files, or camera where available.
- **Photos:** Saving into the iOS Photos library is distinct from downloading to Files. When supported, offer an iOS share-sheet action; keep Files download as a fallback. Browser APIs cannot silently write to Photos.
- **Original formats:** Preserve uploaded or downloaded HEIC, MOV, MP4, Live Photo components, filenames, and file bytes. Avoid automatic conversion. Live Photos may require transferring multiple related files.
- **Large videos:** Use streaming, byte-range requests, visible progress, and cancellation. Test multi-GB transfers on a real iPad.
- **Web Share API:** File sharing may require a secure browser context (HTTPS), and support is feature-dependent. Detect availability rather than assuming it works.
- **Push limitations:** A browser cannot offer fully native AirDrop-style background file saving. Keep the page open and require user interaction for downloads.

## Architecture

### Recommended stack

| Area | Technology |
|---|---|
| Language | Rust |
| HTTP framework | Axum |
| Async I/O | Tokio |
| HTTP middleware/static assets | tower-http |
| CLI | clap |
| QR codes | qrcode or similar |
| Random PIN/session secrets | OS-backed CSPRNG |
| Logging | tracing |
| TLS | rustls |
| Frontend | HTML + CSS + small amount of TypeScript |

Keep frontend assets embedded in the executable if feasible. Avoid a database, cloud server, user accounts, or a heavyweight SPA for the first version.

### Suggested modules

```text
src/
  main.rs
  cli/
  server/       # routes, state, middleware
  auth/         # PIN checks, throttling, session management
  transfer/     # streaming downloads and uploads
  files/        # safe listing, paths, metadata
  utils/        # LAN IP, QR code, logging
web/
  index.html
  style.css
  app.ts
```

### Candidate HTTP routes

| Endpoint | Method | Purpose |
|---|---|---|
| `/` | GET | Login or file UI |
| `/api/auth` | POST | Exchange valid PIN for an authenticated session |
| `/api/logout` | POST | Revoke session |
| `/api/files` | GET | List shared files |
| `/api/files/:id/download` | GET | Stream an attachment |
| `/api/files/:id/preview` | GET | Browser-displayable media |
| `/api/upload` | POST | Authenticated upload to allowed directory |
| `/api/status` | GET | Minimal status (avoid disclosing sensitive details) |

All file listing, preview, upload, and download routes should require authentication. Use opaque server-generated file IDs or validated paths; do not trust arbitrary paths supplied by the browser.

## Security decisions

- **PIN is authorization, not encryption.** Six digits are convenient but have limited entropy; strictly rate-limit attempts, expire PINs/sessions, and consider lockouts or new PIN generation after repeated failure.
- **HTTP on a trusted LAN is still unencrypted.** Other parties able to intercept traffic may capture the PIN, session, or files. Clearly label an HTTP convenience mode as suitable only for trusted networks.
- **HTTPS mode** should be considered for secure transfer and browser features. Certificates for local IPs require a real trust strategy; a random self-signed certificate with a browser warning is not a seamless or verified solution.
- Use unpredictable session tokens in `HttpOnly`, `SameSite` cookies, with `Secure` when running over HTTPS; protect mutating endpoints against CSRF and validate request origins.
- Limit exposed directories; block directory traversal, symlink escapes, and unauthorized upload locations. Avoid overwriting files unexpectedly.
- Prefer explicit CLI opt-in for uploads; enforce upload limits and available disk space.
- Bind to the intended network interface and display what is exposed. Avoid automatic public internet exposure or port forwarding.
- Stop the server with `Ctrl+C`, and ensure outstanding sessions become invalid.

## CLI ideas

```bash
lanshare send movie.mkv
lanshare serve ~/Videos
lanshare serve ~/Videos ~/Pictures
lanshare serve ~/Videos --read-only
lanshare serve ~/Downloads --allow-upload
lanshare serve ~/Videos --port 9000
lanshare serve ~/Videos --timeout 30m
lanshare send movie.mp4 --once
```

Flags shown are proposed UX, not implemented functionality.

## Why Rust?

- Distributable as a standalone executable with no runtime setup.
- Efficient, bounded-memory streaming for very large files.
- Solid async networking ecosystem (Tokio/Axum).
- Portability to other desktop OSes and small computers.
- Memory safety, while still requiring explicit care for web authentication and filesystem security.

## Similar tools / inspiration

- [LocalSend](https://localsend.org): Native-app local network sharing.
- [PairDrop](https://pairdrop.net): Browser-based AirDrop-like experience.
- [Snapdrop](https://github.com/SnapDrop/snapdrop): Browser-based local transfer.

**Differentiation:** A small, temporary, password-protected **CLI-to-browser** workflow. No receiver-side app required, and explicitly optimized for Safari/iPadOS.

## Suggested build sequence

1. Rust CLI starts Axum and prints its local network URL.
2. Generate PIN and QR code; implement rate-limited login and sessions.
3. Expose one specified file with a real attachment download button; verify download to iPad Files.
4. Add shared-directory listing and robust large-file streaming/range support.
5. Add opt-in uploads from iPad Photos/Files and protect filesystem boundaries.
6. Add responsive previews, progress, error handling, and session expiration.
7. Test on iPadOS Safari with MP4, MOV, HEIC, PDF, ZIP, filenames with spaces/Unicode, and multi-GB transfers.
8. Decide on the HTTPS trust model before positioning it for untrusted Wi-Fi.
9. Extend with multi-file ZIP, push mode, resumable uploads, and clipboard sharing.

## Open design decisions

- Should the default command share one file (`send`) or host an upload/drop area (`serve`)? **Recommendation:** Start with `send`, add `serve`.
- Should uploads default to enabled? **Recommendation:** No, require `--allow-upload`.
- Should first release be HTTP-only? **Recommendation:** Document as trusted-network-only; design interfaces that allow verified HTTPS later.
- Should a PIN unlock all currently shared files or require approval per transfer? **Recommendation:** One short-lived session for MVP; optional per-transfer approval later.
- How are files saved into Photos? **Recommendation:** Browser Share API where available, otherwise Files-based workflow, no native iOS dependency.

**Guiding principle:** `lanshare send movie.mp4` → scan QR → enter code → tap Download → file is available offline on the iPad.
