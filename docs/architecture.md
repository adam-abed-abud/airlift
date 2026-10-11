# Architecture

A single Rust binary: Axum HTTP server, embedded static frontend, no
database, no background services.

## Layout

```text
src/
  main.rs         argument parsing, banner, hands over to server::run
  lib.rs          module tree (library so tests can build the router)
  cli/            clap definitions, size parsing
  auth/           pin.rs      code generation + constant-time check
                  throttle.rs per-client lockouts, global rotation
                  session.rs  in-memory token table with expiry
                  mod.rs      ties them together: login() -> token
  files/          mod.rs      Share: roots, opaque IDs, resolve()
                  walk.rs     bounded directory walk, no symlinks
                  mime.rs     content types incl. Apple formats
  transfer/       download.rs streaming responses, Range, Content-Disposition
                  range.rs    Range header parsing
                  upload.rs   multipart -> temp file -> rename, sanitising
                  thumbnail.rs image -> small JPEG; HEIC and video frames via
                              external tools; LRU cache
  server/         mod.rs      router assembly, run loop, shutdown
                  routes.rs   handlers
                  middleware.rs require_session, same_origin_only, ClientIp
                  assets.rs   embedded frontend
                  state.rs    AppState, Scheme
                  error.rs    JSON error type
  utils/          net.rs (LAN IP), qr.rs, log.rs, term.rs (colour),
                  activity.rs (the stdout feed)
web/
  index.html, style.css, app.js, favicon.svg
tests/
  api.rs          end-to-end tests through the router
docs/
```

## Request flow

```text
GET /                      -> embedded index.html (CSP, no-cache)
GET /app.js, /style.css    -> embedded assets (fallback route)

GET  /api/status           public   {authenticated, share_name, upload_enabled}
POST /api/auth             public   {code} -> Set-Cookie session   (throttled)
POST /api/logout           public   revoke cookie's session

GET  /api/files                     session   list entries
GET  /api/files/{id}/download       session   attachment stream, Range
GET  /api/files/{id}/preview        session   inline stream, Range, media only
GET  /api/files/{id}/thumb          session   JPEG rendition (?size=small|large), cacheable
POST /api/upload                    session   multipart, only if --allow-upload
```

Middleware order, outermost first: `TraceLayer` → `nosniff` header →
(`/api` only) `Cache-Control: no-store` unless the handler set one →
`same_origin_only` (all non-GET) → (`/api/files*`, `/api/upload`)
`require_session` → handler. The thumbnail handler is the only one that
sets its own `Cache-Control`.

`require_session` is a `route_layer`, so unknown `/api` paths return 404,
not 401: the API cannot be used to discover which routes exist.

## Key decisions

**Opaque file IDs.** The browser never sends paths. Listing assigns each
`(root, relative path)` a random 64-bit hex ID that stays stable across
rescans, so an open page keeps working after a refresh. `resolve()`
re-canonicalises and re-checks containment at download time.

**Streaming with `ReaderStream`.** Downloads open the file with Tokio, seek
to the range start and wrap `file.take(len)` in a 256 KiB `ReaderStream`.
`take` guarantees the body never exceeds the announced `Content-Length`
even if the file grows while being read. HEAD is handled by Axum/hyper
automatically.

**Range support** is single-range only; multipart ranges fall back to a full
200 response, which the spec allows and which no browser media player
needs. `If-Range` is honoured for ETags (size + mtime), not dates.

**Uploads via Axum's multipart extractor** stream each part to a
`NamedTempFile` in the destination directory. Same directory means same
filesystem, so the final `persist_noclobber` is an atomic rename and
cannot leave a half-written file under the real name. The body limit is
applied per route so it only affects uploads.

**Thumbnails: pure Rust where possible, a subprocess where not.** The
`image` crate handles JPEG/PNG/GIF/WebP/BMP/TIFF, including EXIF
orientation, inside the process. HEIC has no Rust decoder and libheif
would drag in a C++ toolchain plus libde265 at build time, and video
frames would need a codec stack, so instead an external program found on
`PATH` (ImageMagick and friends for HEIC, ffmpeg or `gst-launch-1.0` for
video) converts the source to JPEG in a private temp directory; the source is symlinked there under a fixed name so no tool
ever parses a user-controlled path (ImageMagick reads `a:b.heic` as a
coder prefix). Every rendition then goes through the same Rust
resize/orient/encode step, so output is uniform whichever decoder ran.
Results are cached in memory, bounded by bytes with LRU eviction, and
failures are cached too so a corrupt file is not retried on every refresh.
Decoding runs under a semaphore sized to the CPU count (max 4) so a grid
of hundreds of photos cannot starve downloads.

**Sessions in memory, never on disk.** "Stop the program, everything is
revoked" is the simplest guarantee to reason about and matches the
temporary nature of the tool.

**LAN IP via UDP connect.** Connecting a UDP socket to an unroutable
address makes the OS pick the outbound interface without sending anything.
It is a few lines, dependency-free and cross-platform. `--host` overrides
it when it guesses wrong (VPNs, multiple NICs).

**Frontend without a build step.** `web/app.js` is plain ES2020 with JSDoc
types and `// @ts-check`, so it can be type-checked with `tsc` but ships
as-is. `rust-embed` compiles `web/` into release binaries and reads from
disk in debug builds, so UI edits need only a browser reload.

**Why no `/{*path}` asset route.** A catch-all route also matches
`POST /api/unknown` and answers 405; the fallback handler does not, so
nested API 404s behave.

## Shutdown

Three things end the run: Ctrl+C, the `--timeout` deadline, or a
`--once` completion, which cancels a `CancellationToken` held in
`AppState`. All three trigger Axum's graceful shutdown (stop accepting,
let in-flight requests finish) bounded by a 3-second timeout, after which
the process exits regardless. Since sessions are gone with the process, an
interrupted download could not be resumed anyway.

**Detecting a completed download.** The download body is wrapped in
`CompletionStream`, which counts bytes out and fires a callback from
`Drop` once the promised length has been produced and the range reached
the end of the file. It has to be `Drop` rather than the stream's end:
hyper knows the Content-Length and stops polling the body as soon as that
many bytes are written, so the final `None` is usually never observed. A
body dropped with bytes remaining (client gave up) never fires.

## Extension points

- `Scheme::Https` is plumbed through state, cookie flags and origin
  checks; adding TLS means a listener change in `server::run` and a
  certificate strategy (see security.md).
- ZIP-of-selection, resumable uploads and push notifications belong in
  `transfer/` and `routes.rs` without touching auth.
