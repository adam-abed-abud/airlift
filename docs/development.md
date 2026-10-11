# Development

## Build and test

```bash
cargo build                 # debug: frontend read from web/ at request time
cargo build --release       # frontend embedded; ~3 MB binary, LTO, stripped
cargo test                  # unit tests + tests/api.rs through the router
cargo clippy --all-targets
cargo fmt
```

Run from the repository with `cargo run -- serve ./some/dir`. In debug
builds you can edit `web/*.{html,css,js}` and reload the browser; no
rebuild needed.

Type-check the frontend (optional, needs a TypeScript install):

```bash
npx tsc --noEmit --allowJs --checkJs --target es2020 --lib dom,es2020 web/app.js
```

## Manual smoke test

```bash
cargo run -- serve /tmp/share --allow-upload --upload-dir /tmp/inbox
# then on another device, or locally:
curl -c cj -H "Origin: http://127.0.0.1:8080" -H "Content-Type: application/json" \
     -d '{"code":"123456"}' http://127.0.0.1:8080/api/auth
curl -b cj http://127.0.0.1:8080/api/files
curl -b cj -H "Range: bytes=0-99" -o part http://127.0.0.1:8080/api/files/<id>/download
curl -b cj -H "Origin: http://127.0.0.1:8080" -F file=@photo.heic http://127.0.0.1:8080/api/upload
```

A headless Chrome session at phone width (via the DevTools protocol) was
used to verify login, listing, preview, upload progress and logout. Real
iPad testing is still on the checklist in [ios.md](ios.md).

## Logging

`-v` enables debug output including one line per request from
`tower_http`; `-vv` enables trace. Without `-v`, `RUST_LOG` is honoured,
e.g. `RUST_LOG=lanshare=debug`. Access codes are printed, never logged.

## Roadmap

Implemented (v0.1): everything under "Essential MVP" in
`lanshare_project_notes.md`, plus `--once` and `--timeout`.

Planned, roughly in order:

1. Multi-select with streamed ZIP (store-only for already compressed media).
2. HTTPS with a workable trust story (see security.md), which also unlocks
   the Share-to-Photos path on iOS.
3. Resumable/chunked uploads with SHA-256 verification.
4. Push mode: CLI announces a new file to an open page.
5. Clipboard/text snippets.
6. Connected-device view and optional per-device approval.
7. Release builds for macOS, Windows and Raspberry Pi.

## Conventions

- Comments explain *why*; the code says what.
- Nothing the browser sends is used as a path. Add new file routes through
  `Share::resolve` only.
- Keep the frontend dependency-free and buildless.
- Every security-relevant behaviour has a test in `tests/api.rs`.
