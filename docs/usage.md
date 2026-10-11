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
| `-v`, `-vv` | | Debug / trace logging. `RUST_LOG` is honoured when no `-v` is given. |

## What the terminal shows

```text
LANShare
Sharing: /home/adam/Videos
Uploads: enabled -> /home/adam/Videos
URL: http://192.168.1.105:8080
Access code: 482-913
[QR code of the URL]
Press Ctrl+C to stop
```

The QR code contains only the URL, never the code. The code is new every
run and is not configurable by design.

If too many wrong codes are entered (from anywhere), a new code is
generated and printed:

```text
!! Too many wrong codes. New access code: 913-204
```

Every login and download is logged with the client's IP.

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
