//! Receiving files from the browser.
//!
//! Each multipart field is streamed into a temporary file inside the
//! destination directory and renamed into place only once it is complete.
//! Same directory means same filesystem, so the rename is atomic, and a
//! dropped connection just drops the temp file (its guard deletes it).

use std::path::{Path, PathBuf};

use axum::extract::multipart::Field;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

/// Never fill the disk completely: leave this much headroom so the host
/// stays usable even after a large upload.
const MIN_FREE_AFTER_UPLOAD: u64 = 256 * 1024 * 1024;
/// How many `name (n).ext` variants to try before giving up on a collision.
const MAX_COLLISION_SUFFIX: u32 = 1000;
/// Linux, macOS and Windows all cap a single path component around here.
const MAX_NAME_BYTES: usize = 255;

#[derive(Debug, Clone)]
pub struct UploadConfig {
    /// Canonical destination directory.
    pub dir: PathBuf,
    /// Cap on a single upload request (all files in it together).
    pub max_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct Saved {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("not enough free disk space")]
    DiskFull,
    #[error("upload too large")]
    TooLarge,
    #[error("could not read upload: {0}")]
    Read(String),
    #[error("could not write file: {0}")]
    Write(String),
    #[error("too many files with that name already exist")]
    Collision,
}

impl UploadConfig {
    pub fn new(dir: &Path, max_bytes: u64) -> anyhow::Result<Self> {
        let dir = dir
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("cannot access upload directory {}: {e}", dir.display()))?;
        if !dir.is_dir() {
            anyhow::bail!("upload destination {} is not a directory", dir.display());
        }
        // Fail at start-up rather than on the first upload from the iPad.
        let probe = tempfile::Builder::new().prefix(".lanshare-probe-").tempfile_in(&dir)
            .map_err(|e| anyhow::anyhow!("upload directory {} is not writable: {e}", dir.display()))?;
        drop(probe);
        Ok(Self { dir, max_bytes })
    }

    /// Reject early if the declared request size cannot fit on disk. The
    /// write path also handles ENOSPC, but refusing up front saves the
    /// client from streaming gigabytes only to fail at the end.
    pub fn check_space(&self, declared_len: Option<u64>) -> Result<(), UploadError> {
        if let Some(len) = declared_len {
            if len > self.max_bytes {
                return Err(UploadError::TooLarge);
            }
            if let Ok(free) = fs4::available_space(&self.dir) {
                if free.saturating_sub(MIN_FREE_AFTER_UPLOAD) < len {
                    return Err(UploadError::DiskFull);
                }
            }
        }
        Ok(())
    }

    /// Stream one multipart field to disk.
    pub async fn save_field(&self, mut field: Field<'_>) -> Result<Saved, UploadError> {
        let original = field.file_name().unwrap_or("").to_string();
        let name = sanitize_filename(&original);

        let mut builder = tempfile::Builder::new();
        builder.prefix(".lanshare-").suffix(".part");
        // Temp files default to 0600, which would make every received photo
        // private to this user. Ask for ordinary file permissions instead;
        // the umask still applies, so this behaves like any saved file.
        #[cfg(unix)]
        builder.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o644));
        let temp = builder
            .tempfile_in(&self.dir)
            .map_err(|e| UploadError::Write(e.to_string()))?;
        // tokio needs its own handle; the NamedTempFile keeps ownership of
        // the path so the file is removed if we bail out before persisting.
        let std_handle = temp
            .as_file()
            .try_clone()
            .map_err(|e| UploadError::Write(e.to_string()))?;
        let mut out = tokio::fs::File::from_std(std_handle);

        let mut written: u64 = 0;
        loop {
            let chunk = match field.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(e) => return Err(UploadError::Read(e.body_text())),
            };
            written += chunk.len() as u64;
            if let Err(e) = out.write_all(&chunk).await {
                return Err(match e.raw_os_error() {
                    Some(libc_enospc) if libc_enospc == 28 => UploadError::DiskFull,
                    _ => UploadError::Write(e.to_string()),
                });
            }
        }
        out.flush().await.map_err(|e| UploadError::Write(e.to_string()))?;
        // Make sure the bytes are on disk before we claim success.
        out.sync_all().await.map_err(|e| UploadError::Write(e.to_string()))?;
        drop(out);

        let (final_name, _) = persist_without_overwrite(temp, &self.dir, &name)?;
        info!(name = %final_name, size = written, "upload saved");
        Ok(Saved { name: final_name, size: written })
    }
}

/// Move the finished temp file to its final name, never replacing an
/// existing file. Collisions get ` (1)`, ` (2)`, ... before the extension,
/// which is the convention people already know from their browser.
fn persist_without_overwrite(
    mut temp: tempfile::NamedTempFile,
    dir: &Path,
    name: &str,
) -> Result<(String, PathBuf), UploadError> {
    let (stem, ext) = split_extension(name);
    for n in 0..=MAX_COLLISION_SUFFIX {
        let candidate = if n == 0 {
            name.to_string()
        } else {
            format!("{stem} ({n}){ext}")
        };
        let target = dir.join(&candidate);
        match temp.persist_noclobber(&target) {
            Ok(_) => return Ok((candidate, target)),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                temp = e.file;
            }
            Err(e) => {
                warn!(error = %e.error, "persist failed");
                return Err(UploadError::Write(e.error.to_string()));
            }
        }
    }
    Err(UploadError::Collision)
}

/// `movie.tar.gz` -> (`movie.tar`, `.gz`): only the last extension moves,
/// which is what Finder/Explorer do too.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// Reduce a client-supplied name to a single safe path component.
///
/// The browser normally sends just a base name, but nothing stops a hostile
/// client from sending `../../.bashrc`. Keep Unicode (an iPhone photo called
/// `Strand 🏖️.HEIC` should arrive as such), drop path separators and
/// control characters, and never produce a hidden or empty name.
pub fn sanitize_filename(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let mut cleaned: String = base
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == '\0' { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_string();

    // `.` and `..` are directory references, anything else starting with a
    // dot would be invisible in the listing and in Finder.
    while cleaned.starts_with('.') {
        cleaned.remove(0);
    }
    if cleaned.is_empty() {
        cleaned = "upload".to_string();
    }

    if cleaned.len() > MAX_NAME_BYTES {
        let (stem, ext) = split_extension(&cleaned);
        let ext = if ext.len() > 32 { "" } else { ext };
        let budget = MAX_NAME_BYTES - ext.len();
        let mut cut = budget;
        while !stem.is_char_boundary(cut) {
            cut -= 1;
        }
        cleaned = format!("{}{}", &stem[..cut], ext);
    }
    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_paths_and_hidden_prefixes() {
        assert_eq!(sanitize_filename("movie.mp4"), "movie.mp4");
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("C:\\Users\\me\\x.txt"), "x.txt");
        assert_eq!(sanitize_filename(".bashrc"), "bashrc");
        assert_eq!(sanitize_filename("..."), "upload");
        assert_eq!(sanitize_filename(""), "upload");
        assert_eq!(sanitize_filename("  a\u{7}b.txt "), "ab.txt");
        assert_eq!(sanitize_filename("Strand 🏖️.HEIC"), "Strand 🏖️.HEIC");
    }

    #[test]
    fn sanitize_caps_length_but_keeps_extension() {
        let long = format!("{}.jpg", "x".repeat(300));
        let out = sanitize_filename(&long);
        assert_eq!(out.len(), MAX_NAME_BYTES);
        assert!(out.ends_with(".jpg"));
    }

    #[test]
    fn split_extension_cases() {
        assert_eq!(split_extension("a.tar.gz"), ("a.tar", ".gz"));
        assert_eq!(split_extension("noext"), ("noext", ""));
        assert_eq!(split_extension(".hidden"), (".hidden", ""));
    }

    #[test]
    fn persist_adds_suffix_on_collision() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"first").unwrap();
        let temp = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        let (name, path) = persist_without_overwrite(temp, dir.path(), "a.txt").unwrap();
        assert_eq!(name, "a (1).txt");
        assert!(path.exists());
        assert_eq!(std::fs::read(dir.path().join("a.txt")).unwrap(), b"first");
    }
}
