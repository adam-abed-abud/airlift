//! Content-type guessing. `mime_guess` covers the common cases; the table
//! below fills in Apple formats it does not know, because HEIC photos and
//! QuickTime movies are exactly what an iPhone produces.

use std::path::Path;

const OVERRIDES: &[(&str, &str)] = &[
    ("heic", "image/heic"),
    ("heif", "image/heif"),
    ("heics", "image/heic-sequence"),
    ("avif", "image/avif"),
    ("mov", "video/quicktime"),
    ("m4v", "video/x-m4v"),
    ("mkv", "video/x-matroska"),
    ("webm", "video/webm"),
    ("m4a", "audio/mp4"),
    ("aac", "audio/aac"),
    ("flac", "audio/flac"),
    ("opus", "audio/ogg"),
    ("md", "text/markdown"),
    ("ts", "video/mp2t"),
];

pub fn mime_for(name: &str) -> String {
    let ext = Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if let Some((_, m)) = OVERRIDES.iter().find(|(e, _)| *e == ext) {
        return (*m).to_string();
    }
    mime_guess::from_ext(&ext)
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_string()
}

/// Whether Safari (and browsers generally) can show the type inline. Used
/// to decide if a Preview button makes sense; the download button is
/// offered for everything regardless.
pub fn is_previewable(mime: &str) -> bool {
    mime.starts_with("image/")
        || mime.starts_with("video/")
        || mime.starts_with("audio/")
        || mime == "application/pdf"
        || mime == "text/plain"
        || mime == "text/markdown"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_formats_are_known() {
        assert_eq!(mime_for("IMG_0001.HEIC"), "image/heic");
        assert_eq!(mime_for("clip.MOV"), "video/quicktime");
        assert_eq!(mime_for("movie.mp4"), "video/mp4");
        assert_eq!(mime_for("doc.pdf"), "application/pdf");
        assert_eq!(mime_for("noext"), "application/octet-stream");
    }

    #[test]
    fn previewable_types() {
        assert!(is_previewable("image/heic"));
        assert!(is_previewable("video/quicktime"));
        assert!(is_previewable("application/pdf"));
        assert!(!is_previewable("application/zip"));
    }
}
