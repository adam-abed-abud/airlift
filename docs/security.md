# Security model

LANShare is a convenience tool for moving files between your own devices
on a network you trust. This page states exactly what it protects against,
what it does not, and how.

## Threat model

**Protected against**

- Someone on the LAN who does not know the code reading the shared files.
- Guessing the code by brute force.
- A web page on another site using your logged-in browser to read or
  upload files (CSRF).
- The share reaching into files outside the directories you named
  (traversal, symlinks, crafted upload names).
- Uploads overwriting existing files or filling the disk.
- Access lingering after you stop the program.

**Not protected against**

- Anyone who can see your terminal (they can read the code).
- Anyone who can sniff traffic on the network: plain HTTP carries the
  code, the session cookie and the file contents in the clear.
- A hostile device that has already obtained a valid session.
- Exposure to the Internet. LANShare never sets up port forwarding or UPnP;
  if you forward the port yourself, the PIN is not a sufficient defence.

The page footer says "Unencrypted HTTP: use only on networks you trust"
for this reason.

## Access code

- Six random digits from the OS CSPRNG (`getrandom`), generated at every
  start. There is intentionally no way to configure a fixed code.
- Comparison is constant-time (`subtle`).
- The QR code encodes only the URL, so a photo of the screen's QR is not
  enough to get in.

### Throttling

Six digits is one million possibilities, so guessing must be slow:

| Rule | Value |
|---|---|
| Wrong guesses per client before lockout | 5 |
| First lockout | 30 s, doubling each time, capped at 15 min |
| Wrong guesses across all clients before the code is replaced | 30 |

Lockouts are checked *before* the code, so a locked client learns nothing
from its guesses. When the code is replaced, a warning and the new code are
printed in the terminal. At these rates a single attacker gets roughly ten
guesses in the first few minutes and the code rotates long before the
search space is dented.

## Sessions

- A successful login returns a 256-bit random token in a cookie:
  `HttpOnly` (not readable by scripts), `SameSite=Strict` (not sent on
  cross-site requests), `Path=/`, `Secure` when served over HTTPS.
- Sessions are held in memory only. Stopping the process revokes all of
  them; there is nothing on disk to leak or to clean up.
- Idle timeout 2 h, absolute lifetime 12 h. **Log out** revokes immediately.
- Every route that touches files requires a valid session. `/api/status`
  is public and reveals only the share's display name, version and whether
  uploads are enabled.

## CSRF

Two independent layers:

1. The `SameSite=Strict` cookie is never attached to requests initiated by
   another origin.
2. Every non-GET request must carry an `Origin` header equal to
   `scheme://Host` of the request, otherwise it is rejected with 403 before
   reaching any handler.

The page is served with `Content-Security-Policy: default-src 'self' ...`,
`X-Frame-Options: DENY`, `Referrer-Policy: no-referrer` and
`X-Content-Type-Options: nosniff`.

## Filesystem boundaries

- Share roots are canonicalised at start-up.
- Listing walks with `follow_links(false)`; symlinks are skipped entirely,
  as are non-regular files. Dot-files are skipped unless `--include-hidden`.
- The browser never sends a path. Files are addressed by random opaque IDs
  that map to `(root, relative path)` on the server.
- When an ID is resolved for download, the path is canonicalised again and
  must still equal the nominal path, lie under its root and be a regular
  file. This closes the window where a file is replaced by a symlink
  between listing and download.
- Upload names are reduced to a single path component; separators and
  control characters are removed and leading dots stripped. Files are
  written to a temp file in the destination and moved with
  `persist_noclobber`, so an existing file is never replaced.
- Uploads are off by default. When off, the route is not mounted at all.

## Resource limits

- Downloads stream in 256 KiB chunks; memory use is independent of file size.
- Upload requests are capped by `--max-upload` (default 10 GiB) via the
  body limit, and refused up front if free disk space minus 256 MB headroom
  is insufficient.
- Listings stop at 10 000 entries / depth 16. The throttle table holds at
  most 4096 client entries.

## HTTPS

Not implemented in this version. The obstacles are not technical but about
trust: a self-signed certificate for a LAN IP produces a scary warning on
the iPad on every run, which trains people to click through warnings and
provides no real verification. Options under consideration for a later
version:

- a per-install CA generated once, with a one-time profile install on the
  iPad, after which every run is trusted;
- mDNS hostnames with a persistent certificate;
- a pairing step that pins the certificate fingerprint via the QR code.

The code already carries a `Scheme` through the server state so cookie
flags and origin checks switch correctly once TLS lands. Until then, treat
LANShare as a trusted-network tool. Note that HTTPS would also unlock the
Web Share API in Safari (see [ios.md](ios.md)).
