use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use walkdir::WalkDir;

use super::{MAX_DEPTH, Root, RootKind};

pub struct Found {
    pub rel: PathBuf,
    pub name: String,
    pub size: u64,
    pub modified: Option<u64>,
}

/// Enumerate regular files under a root, in a stable sorted order.
///
/// Symlinks are never followed *and* never listed: following them is how a
/// share of `~/Videos` quietly becomes a share of wherever a link points.
pub fn walk_root(root: &Root, include_hidden: bool, limit: usize) -> Vec<Found> {
    match root.kind {
        RootKind::File => describe(&root.path, PathBuf::new()).into_iter().collect(),
        RootKind::Dir => WalkDir::new(&root.path)
            .min_depth(1)
            .max_depth(MAX_DEPTH)
            .follow_links(false)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| include_hidden || !is_hidden(e.file_name()))
            .filter_map(Result::ok)
            // `file_type` comes from the directory entry, so a symlink shows
            // up as a symlink here rather than as its target.
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| {
                let rel = e.path().strip_prefix(&root.path).ok()?.to_path_buf();
                describe(e.path(), rel)
            })
            .take(limit)
            .collect(),
    }
}

fn describe(path: &Path, rel: PathBuf) -> Option<Found> {
    let meta = path.symlink_metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    Some(Found {
        name: path.file_name()?.to_string_lossy().into_owned(),
        rel,
        size: meta.len(),
        modified,
    })
}

fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}
