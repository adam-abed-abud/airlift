//! The share: which files are visible, under which opaque IDs, and how a
//! browser-supplied ID turns back into a path that is guaranteed to lie
//! inside one of the share roots.
//!
//! Nothing in here trusts the client. IDs are random handles minted by the
//! server; the only path operations on client input are table lookups.

mod mime;
mod walk;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, bail};
use serde::Serialize;

pub use mime::{is_previewable, mime_for};

/// Upper bound on listed entries. Enough for any realistic "share this
/// folder with my iPad" use, and a backstop against sharing `/` by mistake.
pub const MAX_ENTRIES: usize = 10_000;
pub const MAX_DEPTH: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootKind {
    /// `lanshare send FILE`: the root *is* the file.
    File,
    /// `lanshare serve DIR`: everything underneath is listed.
    Dir,
}

#[derive(Debug, Clone)]
pub struct Root {
    /// Short name shown to the user when several roots are shared.
    pub label: String,
    /// Canonical absolute path. Canonical so containment checks are exact.
    pub path: PathBuf,
    pub kind: RootKind,
}

/// One listed file, as sent to the browser.
#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub id: String,
    pub name: String,
    /// Path relative to its root, for display and grouping only.
    pub path: String,
    pub root: String,
    pub size: u64,
    /// Seconds since the Unix epoch, if the filesystem reports it.
    pub modified: Option<u64>,
    pub mime: String,
    pub previewable: bool,
}

/// A file ID resolved back to something the transfer layer can open.
#[derive(Debug, Clone)]
pub struct ResolvedFile {
    pub path: PathBuf,
    pub name: String,
    pub mime: String,
}

/// Maps between (root, relative path) and opaque IDs, in both directions.
/// IDs stay stable across rescans so a page left open keeps working.
#[derive(Default)]
struct IdRegistry {
    by_location: HashMap<(usize, PathBuf), String>,
    by_id: HashMap<String, (usize, PathBuf)>,
}

impl IdRegistry {
    fn id_for(&mut self, root: usize, rel: &Path) -> String {
        let key = (root, rel.to_path_buf());
        if let Some(id) = self.by_location.get(&key) {
            return id.clone();
        }
        let id = fresh_id();
        self.by_location.insert(key.clone(), id.clone());
        self.by_id.insert(id.clone(), key);
        id
    }
}

pub struct Share {
    roots: Vec<Root>,
    ids: Mutex<IdRegistry>,
    include_hidden: bool,
}

impl Share {
    /// Share exactly one file.
    pub fn single_file(file: &Path) -> anyhow::Result<Self> {
        let path = file
            .canonicalize()
            .with_context(|| format!("cannot access {}", file.display()))?;
        if !path.is_file() {
            bail!(
                "{} is not a regular file (use `serve` for directories)",
                file.display()
            );
        }
        let label = file_name(&path);
        Ok(Self::with_roots(
            vec![Root {
                label,
                path,
                kind: RootKind::File,
            }],
            true,
        ))
    }

    /// Share one or more directories.
    pub fn directories(dirs: &[PathBuf], include_hidden: bool) -> anyhow::Result<Self> {
        let mut roots = Vec::with_capacity(dirs.len());
        for dir in dirs {
            let path = dir
                .canonicalize()
                .with_context(|| format!("cannot access {}", dir.display()))?;
            if !path.is_dir() {
                bail!(
                    "{} is not a directory (use `send` for a single file)",
                    dir.display()
                );
            }
            roots.push(Root {
                label: file_name(&path),
                path,
                kind: RootKind::Dir,
            });
        }
        disambiguate_labels(&mut roots);
        Ok(Self::with_roots(roots, include_hidden))
    }

    fn with_roots(roots: Vec<Root>, include_hidden: bool) -> Self {
        Self {
            roots,
            ids: Mutex::new(IdRegistry::default()),
            include_hidden,
        }
    }

    pub fn roots(&self) -> &[Root] {
        &self.roots
    }

    /// Human-readable name for the page title / terminal banner.
    pub fn display_name(&self) -> String {
        match self.roots.as_slice() {
            [one] => one.label.clone(),
            many => format!("{} folders", many.len()),
        }
    }

    /// Walk the roots and produce the current listing. Rescanned on every
    /// call so files added after start-up show up on refresh; the walk is
    /// bounded by `MAX_ENTRIES`/`MAX_DEPTH` so it stays cheap.
    pub fn list(&self) -> Vec<FileEntry> {
        let mut ids = self.ids.lock().unwrap();
        let mut out = Vec::new();
        for (idx, root) in self.roots.iter().enumerate() {
            let remaining = MAX_ENTRIES.saturating_sub(out.len());
            if remaining == 0 {
                break;
            }
            for found in walk::walk_root(root, self.include_hidden, remaining) {
                let id = ids.id_for(idx, &found.rel);
                let mime = mime_for(&found.name);
                out.push(FileEntry {
                    id,
                    previewable: is_previewable(&mime),
                    mime,
                    name: found.name,
                    path: found.rel.to_string_lossy().into_owned(),
                    root: root.label.clone(),
                    size: found.size,
                    modified: found.modified,
                });
            }
        }
        out
    }

    /// Turn an ID back into a path, re-checking at access time that the
    /// target still lies inside its root and is a regular file. The listing
    /// never follows symlinks, but a file could be swapped for one between
    /// listing and download; canonicalising here closes that window.
    pub fn resolve(&self, id: &str) -> Option<ResolvedFile> {
        let (root_idx, rel) = self.ids.lock().unwrap().by_id.get(id).cloned()?;
        let root = self.roots.get(root_idx)?;
        let nominal = match root.kind {
            RootKind::File => root.path.clone(),
            RootKind::Dir => root.path.join(&rel),
        };
        let real = nominal.canonicalize().ok()?;
        if real != nominal || !real.starts_with(&root.path) || !real.is_file() {
            return None;
        }
        let name = file_name(&real);
        let mime = mime_for(&name);
        Some(ResolvedFile {
            path: real,
            name,
            mime,
        })
    }
}

/// Short random handle. Unpredictability is not a security property here
/// (every file route is behind the session check); randomness just keeps
/// the IDs free of any path information.
fn fresh_id() -> String {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("OS random source unavailable");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// `serve ~/a/Videos ~/b/Videos` would otherwise show two roots called
/// "Videos"; suffix duplicates so the listing stays unambiguous.
fn disambiguate_labels(roots: &mut [Root]) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for root in roots.iter_mut() {
        let n = seen.entry(root.label.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            root.label = format!("{} ({})", root.label, n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lanshare-test-{}", fresh_id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn single_file_share_lists_one_entry() {
        let dir = tmp();
        let file = dir.join("movie.mp4");
        fs::write(&file, b"data").unwrap();
        let share = Share::single_file(&file).unwrap();
        let list = share.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "movie.mp4");
        assert_eq!(list[0].mime, "video/mp4");
        let resolved = share.resolve(&list[0].id).unwrap();
        assert_eq!(resolved.path, file.canonicalize().unwrap());
        assert!(share.resolve("nonexistent").is_none());
    }

    #[test]
    fn directory_share_skips_symlinks_and_hidden_and_keeps_ids_stable() {
        let dir = tmp();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), b"a").unwrap();
        fs::write(dir.join("sub/b.pdf"), b"b").unwrap();
        fs::write(dir.join(".secret"), b"s").unwrap();
        let outside = tmp().join("outside.txt");
        fs::write(&outside, b"o").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link.txt")).unwrap();

        let share = Share::directories(std::slice::from_ref(&dir), false).unwrap();
        let first = share.list();
        let names: Vec<_> = first.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "sub/b.pdf"]);

        let second = share.list();
        assert_eq!(first[0].id, second[0].id);

        let hidden = Share::directories(&[dir], true).unwrap();
        assert!(hidden.list().iter().any(|e| e.name == ".secret"));
    }

    #[test]
    fn resolve_rejects_file_replaced_by_escaping_symlink() {
        let dir = tmp();
        fs::write(dir.join("a.txt"), b"a").unwrap();
        let share = Share::directories(std::slice::from_ref(&dir), false).unwrap();
        let id = share.list()[0].id.clone();
        assert!(share.resolve(&id).is_some());

        let outside = tmp().join("outside.txt");
        fs::write(&outside, b"o").unwrap();
        fs::remove_file(dir.join("a.txt")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("a.txt")).unwrap();
        assert!(share.resolve(&id).is_none());
    }
}
