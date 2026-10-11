# LANShare

Send a file from your computer to an iPad, phone or any other device on the
same Wi-Fi, with nothing to install on the receiving side: run one command,
scan the QR code, type the access code, tap **Download**.

```text
$ lanshare send movie.mp4
LANShare
Sharing: /home/adam/Videos/movie.mp4
URL: http://192.168.1.105:8080
Access code: 482-913
[QR code]
Press Ctrl+C to stop
```

- Single static binary, no runtime, no accounts, no cloud.
- Fresh six-digit code on every start; brute-force throttled; sessions die
  when the program stops.
- Streams files of any size with HTTP range support (seekable video,
  resumable downloads). Original bytes and filenames are preserved.
- Optional uploads *to* your computer from the iPhone/iPad Photos app or
  Files, behind an explicit flag.
- Photo grid with thumbnails, swipeable preview, folder grouping, search.
  HEIC and video thumbnails use ImageMagick, libheif or ffmpeg/GStreamer
  when installed.
- Terminal shows a live feed: who connected, what was sent, what arrived.

## Install

With Cargo (needs a Rust toolchain, 1.85 or newer):

```bash
cargo install --path .        # or: cargo build --release && cp target/release/lanshare ~/.local/bin/
```

With Nix (flakes), which also brings ImageMagick and ffmpeg for HEIC and
video thumbnails:

```bash
nix run .# -- send movie.mp4           # run without installing
nix profile install .#                 # install into your profile
```

## Use

```bash
lanshare send movie.mp4                      # share one file
lanshare serve ~/Videos                      # share a folder (recursively)
lanshare serve ~/Videos ~/Pictures           # several folders
lanshare serve ~/Downloads --allow-upload    # also accept files from the device
lanshare send movie.mp4 --once               # exit after the download completes
lanshare serve ~/Videos --timeout 30m        # exit after 30 minutes
lanshare send report.pdf --port 9000         # different port
```

Open the URL on the other device (or scan the QR code), enter the code, then
**Download** saves to the Files app, **Preview** plays or shows it inline.

Run `lanshare --help` or see [docs/usage.md](docs/usage.md) for all options.

## Security in one paragraph

The code lets anyone on the network who knows it read everything you shared
(and write to one folder if uploads are on). Traffic is plain HTTP, so use it
on networks you trust. Only the paths you name are reachable; symlinks are
never followed. Details and the threat model: [docs/security.md](docs/security.md).

## Documentation

- [Usage and CLI reference](docs/usage.md)
- [iPad and iPhone notes](docs/ios.md)
- [Security model](docs/security.md)
- [Architecture](docs/architecture.md)
- [Development and roadmap](docs/development.md)
