# Usage

## Commands

### `lanshare send <FILE>`

Shares exactly one file. The page lists that file with a Download button
(and Preview if it is an image, video, audio file, PDF or text).

### `lanshare serve <DIR>...`

Shares one or more directories. Every regular file underneath is listed
with its relative path, up to 16 levels deep and 10 000 entries. The list
is rescanned on every refresh, so files added after start-up appear.

What is *not* listed:

- symbolic links (never followed, never shown);
- dot-files and dot-directories, unless `--include-hidden` is given;
- anything that is not a regular file (sockets, devices, ...).

If two directories have the same name, the second is shown as `Name (2)`.

## Options

All options work with both commands unless noted.

| Option | Default | Meaning |
|---|---|---|
| `-p`, `--port <PORT>` | `8080` | TCP port to listen on. |
| `--bind <ADDR>` | `0.0.0.0` | Interface to bind. Use a specific address on machines with several networks to limit exposure. |
| `--host <HOST>` | auto | Host to print in the URL/QR. Auto-detection picks the address of the default route; override it if the wrong interface is chosen. |
| `--allow-upload` | off | Enable uploads from connected devices. Without it the share is strictly read-only and the upload endpoint does not exist. |
| `--upload-dir <DIR>` | first shared dir (`serve`), current dir (`send`) | Where uploads are written. Requires `--allow-upload`. |
| `--max-upload <SIZE>` | `10G` | Cap on one upload request (all files in it together). Accepts `500M`, `1.5G`, `2T`, plain bytes. |
| `--include-hidden` | off | `serve` only: list dot-files too. |
| `--once` | off | Exit after the first completed transfer: a download that reached the end of the file, or an upload request that saved at least one file. Previews do not count. |
| `--timeout <DURATION>` | none | Exit after this long, e.g. `30m`, `2h`, `1h30m`. |
| `--no-thumbnails` | off | Skip image thumbnails. Useful on very large photo trees or slow machines. |
| `-v`, `-vv` | | Debug / trace logging. `RUST_LOG` is honoured when no `-v` is given. |

## What the terminal shows

```text
  LANShare  0.1.0  sharing Videos (12 files, 3.4 GB)
  /home/adam/Videos

  URL    http://192.168.1.105:8080
  Code   482-913

  [QR code of the URL]
  On this computer: http://127.0.0.1:8080
  Uploads enabled → /home/adam/Videos
  Thumbnails: HEIC via /usr/bin/magick
  Press Ctrl+C to stop

14:03:21  ▸ iPad (192.168.1.23) connected
14:03:40  ▸ iPad (192.168.1.23) started downloading movie.mp4 (1.2 GB)
14:04:19  ✓ movie.mp4 sent to iPad (192.168.1.23) · 1.2 GB in 39 s
14:05:02  ✓ Received 3 files from iPad (192.168.1.23) · 8.1 MB · IMG_0412.HEIC, IMG_0413.HEIC, IMG_0414.HEIC
14:05:10  ✗ Wrong code from Android phone (192.168.1.40)
```

The QR code contains only the URL, never the code. The code is new every
run and is not configurable by design. Colour is used when stdout is a
terminal and is disabled by `NO_COLOR`.

Below the banner is the activity feed: connections, downloads (announced
when they start and when the last byte went out), uploads, wrong codes
and lockouts. The device name comes from the browser ("iPad", "Mac",
"Android phone"); the address is the client's IP.

If too many wrong codes are entered (from anywhere), a new code is
generated and printed in the feed:

```text
14:06:11  ✗ Too many wrong codes. New access code: 913-204
```

Diagnostic logs go to stderr and are silent unless `-v` is given.

## On the device

1. Scan the QR code or type the URL into the browser.
2. Enter the access code (dashes and spaces are ignored).
3. **Download** saves the file. **Preview** opens images, video, audio,
   PDFs and text inline without downloading.
4. If uploads are enabled, **Choose files…** opens the system picker. On
   iOS that offers Photo Library, Files and the camera. Several files can
   be selected at once; progress and a Cancel button are shown.
5. **Log out** ends the session on that device. Stopping the program
   (Ctrl+C) ends every session at once.

Sessions expire after 2 hours of inactivity and after 12 hours regardless.

## Thumbnails

Image files get a thumbnail in the list and a grid view, which is chosen
automatically when at least half of the shared files are images. The
choice is remembered per device, as is the sort order.

The page groups files by folder (collapsible), sorts by name, newest or
largest, filters by photos/videos/other and searches by name. The
preview overlay steps through the files currently shown with a swipe,
the on-screen arrows or the keyboard arrows. On a desktop browser, files
dragged onto the page are uploaded when uploads are enabled, and the
upload panel keeps a strip of what was received in this session.

- JPEG, PNG, GIF, WebP, BMP and TIFF are decoded by LANShare itself.
  EXIF orientation is applied, so phone photos are not sideways.
- HEIC/HEIF (iPhone photos) need an external decoder. At start-up LANShare
  looks on `PATH` for, in order: `magick` (ImageMagick 7 built with
  libheif), `heif-thumbnail` or `heif-convert` (package `libheif-examples`
  on Debian/Ubuntu), `vipsthumbnail`, `sips` (built into macOS), and
  finally `convert` (ImageMagick 6). The terminal reports which one is
  used, or that none was found. Set `LANSHARE_HEIC_TOOL=/path/to/program`
  to force a specific one.
- Without a decoder, HEIC files show a type badge instead and can still be
  downloaded and, on Safari, previewed.
- Videos get a frame thumbnail (one second in, or the first frame for
  shorter clips) when `ffmpeg` or GStreamer's `gst-launch-1.0` is on
  `PATH`. `LANSHARE_VIDEO_TOOL=/path/to/program` forces one. Phone
  recordings are rotated correctly by both tools.
- Thumbnails are generated on demand, at most a few at a time, and cached
  in memory (64 MB) for the lifetime of the run. The browser caches them
  for a day; a changed file gets a new URL automatically.
- Files above 256 MB are not thumbnailed.

The preview overlay also uses this machinery as a fallback: when the
browser cannot display the original (HEIC in Chrome or Firefox, TIFF), a
converted 1600 px rendition is shown with a note. The Download button
always delivers the untouched original.

## Uploads in detail

- Files keep their original name and bytes. Only path separators and
  control characters are removed; a leading dot is stripped so uploads are
  never hidden files. Unicode (including emoji) is preserved.
- An existing file is never overwritten: `photo.heic` becomes
  `photo (1).heic`, then `photo (2).heic`, and so on.
- Each file is written to a hidden `.lanshare-*.part` temp file in the
  destination and renamed into place when complete. A cancelled or failed
  upload leaves nothing behind.
- Before accepting, free disk space is checked against the request size,
  keeping 256 MB of headroom. Running out of space mid-upload fails that
  file cleanly.

## Stopping

Ctrl+C stops accepting connections, waits up to three seconds for
in-flight transfers, then exits. A download interrupted this way cannot be
resumed afterwards because all sessions are gone with the process.

`--once` and `--timeout` stop the server the same way without you being
there. `--once` is the natural companion of `send`: start it, walk over to
the iPad, tap Download, and the terminal is free again when the file has
fully arrived. A download that the client abandons part-way does not
count, so a flaky connection cannot end the share early.
