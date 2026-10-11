# iPad and iPhone notes

Airlift's web page is designed around how Safari on iOS/iPadOS behaves.
This page explains what to expect and why some things work the way they do.

## Downloading

**Download** is a plain link to an endpoint that answers with
`Content-Disposition: attachment`. Safari therefore saves the file rather
than playing it, shows it in the Downloads list (the arrow icon in the
toolbar), and stores it in the Files app under *Downloads* (iCloud Drive or
On My iPad, depending on Safari settings).

Because the download is a direct stream, not a JavaScript blob, there is no
size limit beyond free space on the device, and Safari shows its own
progress. Multi-gigabyte videos are fine.

Non-ASCII filenames are sent in both the legacy and RFC 8187 forms, so
`Füße 🎬.mov` arrives with that exact name.

## Preview

**Preview** uses the browser's own `<video>`, `<img>`, `<audio>` and PDF
viewer with an `inline` disposition. The server supports HTTP range
requests, which Safari requires to seek inside a video and to start playing
before the file is fully transferred.

Previewing does not save anything. Use Download for that.

Safari decodes HEIC natively, so previews of iPhone photos show the
original. Thumbnails in the list and grid come from the server; for HEIC
that requires a decoder on the sharing computer (see
[usage.md](usage.md#thumbnails)).

## Getting photos and videos into the Photos app

A browser cannot write into the Photos library silently. There are two
routes:

1. **Share…** button: appears only when the Web Share API with file support
   is available. It fetches the file into memory and opens the iOS share
   sheet, where *Save Image* / *Save Video* adds it to Photos. Limited to
   files under 200 MB for that reason. Safari only exposes this API in a
   secure context, so over plain HTTP the button is usually absent; see
   the HTTPS note in [security.md](security.md).
2. **Download, then save from Files**: open the file in the Files app, use
   the share icon, choose *Save Image* / *Save Video*. Works for any size.

## Uploading from the device

The upload control is a standard `<input type="file" multiple>`. iOS turns
that into a sheet offering **Photo Library**, **Take Photo or Video** and
**Choose Files**.

- Photos and videos are sent as the original files (HEIC, MOV, MP4, ...).
  No conversion happens on either side. Note that iOS itself may offer to
  convert HEIC to JPEG when "Most Compatible" transfer settings are on
  (Settings → Camera → Formats / Settings → Photos → Transfer to Mac or PC).
- Live Photos consist of a still image plus a short video. The browser
  picker normally provides only the still; the paired `.mov` is not
  accessible through a web page.
- Keep the page in the foreground during large uploads. iOS suspends
  background tabs and the upload will be cancelled.

## Add to Home Screen

The page sets `apple-mobile-web-app-capable`, so *Add to Home Screen*
gives a full-screen launcher. The access code still has to be entered on
each new run of the server.

## Testing checklist

Verified in this repository with headless Chrome at phone width. To be
checked on real hardware before relying on it:

- MP4, MOV, HEIC, PDF and ZIP downloads land in Files with intact names.
- Video preview seeks correctly (range requests).
- Upload of several photos from Photo Library, including HEIC.
- A file larger than 2 GB in both directions.
- Filenames with spaces, umlauts and emoji.
