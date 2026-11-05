//! A general file tree rooted at one directory (`DSYS_FILES_DIR`), addressed by
//! relative `/`-separated paths. Every path is checked so nothing can reach
//! outside the root: no `..`, no absolute paths, and no symlinks anywhere along
//! the way.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

use serde::Serialize;

use crate::errors::{ApiError, ApiResult};

/// Largest file whose contents `tree(include_content)` will inline.
pub(crate) const MAX_INLINE_CONTENT: u64 = 1024 * 1024;

/// Prefix of the temp files used for atomic writes; never listed.
const TMP_PREFIX: &str = ".dsys-tmp-";

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub(crate) struct FileRoot {
    root: PathBuf,
    /// Serializes mutations so conditional writes (If-Match / If-None-Match),
    /// moves and deletes can't interleave.
    lock: Arc<Mutex<()>>,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum EntryKind {
    Dir,
    File,
}

#[derive(Serialize, Debug)]
pub(crate) struct Entry {
    pub name: String,
    /// Relative to the root; `""` for the root itself.
    pub path: String,
    pub kind: EntryKind,
    /// Bytes; 0 for directories.
    pub size: u64,
    /// Unix epoch milliseconds.
    pub modified_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// UTF-8 contents, only when requested and the file is small enough.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<Entry>>,
}

/// Preconditions for a write, from `If-Match` / `If-None-Match: *`.
#[derive(Debug, Default)]
pub(crate) struct WriteCondition {
    pub if_match: Option<String>,
    pub if_none_match_any: bool,
}

/// Strong ETag over the file contents (FNV-1a 64), so it is stable across restarts.
pub(crate) fn etag_of(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("\"{h:016x}\"")
}

/// Split and check a relative path. `""` (or `"/"`) means the root.
pub(crate) fn parse_rel(rel: &str) -> ApiResult<Vec<&str>> {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    trimmed
        .split('/')
        .map(|seg| {
            let bad = seg.is_empty()
                || seg == "."
                || seg == ".."
                || seg.contains(['\\', '\0'])
                || seg.len() > 255;
            if bad {
                Err(ApiError::BadRequest(format!("invalid path segment {seg:?} in {rel:?}")))
            } else {
                Ok(seg)
            }
        })
        .collect()
}

fn not_found(e: std::io::Error, what: &'static str) -> ApiError {
    if e.kind() == ErrorKind::NotFound { ApiError::NotFound(what) } else { e.into() }
}

fn modified_ms(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl FileRoot {
    pub fn new(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root: root.canonicalize()?, lock: Arc::new(Mutex::new(())) })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Resolve a relative path, refusing symlinks on any existing component.
    fn resolve(&self, rel: &str) -> ApiResult<(PathBuf, String)> {
        let segs = parse_rel(rel)?;
        let mut p = self.root.clone();
        for seg in &segs {
            p.push(seg);
            match fs::symlink_metadata(&p) {
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(ApiError::BadRequest(format!("{rel:?} goes through a symlink")));
                }
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok((p, segs.join("/")))
    }

    fn resolve_non_root(&self, rel: &str) -> ApiResult<(PathBuf, String)> {
        let (p, norm) = self.resolve(rel)?;
        if norm.is_empty() {
            return Err(ApiError::BadRequest("path must not be the root".into()));
        }
        Ok((p, norm))
    }

    fn entry(rel: String, meta: &fs::Metadata) -> Entry {
        let name = rel.rsplit('/').next().unwrap_or("").to_string();
        Entry {
            name,
            path: rel,
            kind: if meta.is_dir() { EntryKind::Dir } else { EntryKind::File },
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified_ms: modified_ms(meta),
            etag: None,
            content: None,
            children: None,
        }
    }

    // -----------------------------------------------------------------------
    // Reads
    // -----------------------------------------------------------------------

    /// Recursive listing of a directory. Hidden entries (leading `.`) and
    /// symlinks are skipped unless `include_hidden`; symlinks are always skipped.
    pub fn tree(&self, rel: &str, include_content: bool, include_hidden: bool) -> ApiResult<Entry> {
        let (abs, norm) = self.resolve(rel)?;
        let meta = fs::metadata(&abs).map_err(|e| not_found(e, "directory"))?;
        if !meta.is_dir() {
            return Err(ApiError::BadRequest(format!("{norm:?} is not a directory")));
        }
        let mut root = Self::entry(norm, &meta);
        root.children = Some(self.list_children(&abs, &root.path, include_content, include_hidden)?);
        Ok(root)
    }

    fn list_children(
        &self,
        dir: &Path,
        rel: &str,
        include_content: bool,
        include_hidden: bool,
    ) -> ApiResult<Vec<Entry>> {
        let mut out = Vec::new();
        for de in fs::read_dir(dir)? {
            let de = de?;
            let Ok(name) = de.file_name().into_string() else { continue };
            if name.starts_with(TMP_PREFIX) || (!include_hidden && name.starts_with('.')) {
                continue;
            }
            let ft = de.file_type()?;
            if ft.is_symlink() {
                continue;
            }
            let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let abs = de.path();
            let meta = de.metadata()?;
            let mut e = Self::entry(child_rel, &meta);
            if ft.is_dir() {
                e.children = Some(self.list_children(&abs, &e.path, include_content, include_hidden)?);
            } else if include_content && meta.len() <= MAX_INLINE_CONTENT {
                let bytes = fs::read(&abs)?;
                e.etag = Some(etag_of(&bytes));
                e.content = String::from_utf8(bytes).ok();
            }
            out.push(e);
        }
        out.sort_by(|a, b| {
            (a.kind == EntryKind::File)
                .cmp(&(b.kind == EntryKind::File))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(out)
    }

    pub fn read_file(&self, rel: &str) -> ApiResult<(Vec<u8>, String)> {
        let (abs, norm) = self.resolve_non_root(rel)?;
        let meta = fs::metadata(&abs).map_err(|e| not_found(e, "file"))?;
        if meta.is_dir() {
            return Err(ApiError::Conflict(format!("{norm:?} is a directory")));
        }
        let bytes = fs::read(&abs).map_err(|e| not_found(e, "file"))?;
        let etag = etag_of(&bytes);
        Ok((bytes, etag))
    }

    // -----------------------------------------------------------------------
    // Writes
    // -----------------------------------------------------------------------

    /// Write a file atomically (temp file + rename), creating parent
    /// directories. Returns the entry and whether the file was newly created.
    pub fn write_file(&self, rel: &str, bytes: &[u8], cond: &WriteCondition) -> ApiResult<(Entry, bool)> {
        let (abs, norm) = self.resolve_non_root(rel)?;
        let _g = self.guard();

        let existing = match fs::metadata(&abs) {
            Ok(m) if m.is_dir() => return Err(ApiError::Conflict(format!("{norm:?} is a directory"))),
            Ok(_) => Some(fs::read(&abs)?),
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if cond.if_none_match_any && existing.is_some() {
            return Err(ApiError::PreconditionFailed(format!("{norm:?} already exists")));
        }
        if let Some(want) = &cond.if_match {
            let have = existing.as_deref().map(etag_of);
            if want != "*" && have.as_deref() != Some(want.as_str()) || want == "*" && have.is_none() {
                return Err(ApiError::PreconditionFailed(format!(
                    "{norm:?} has changed (etag {})",
                    have.unwrap_or_else(|| "none".into())
                )));
            }
        }

        let parent = abs.parent().expect("non-root path has a parent");
        self.mkdirs(parent)?;
        let tmp = parent.join(format!(
            "{TMP_PREFIX}{}-{}",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let res = (|| -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            fs::rename(&tmp, &abs)
        })();
        if let Err(e) = res {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }

        let meta = fs::metadata(&abs)?;
        let mut e = Self::entry(norm, &meta);
        e.etag = Some(etag_of(bytes));
        Ok((e, existing.is_none()))
    }

    /// `create_dir_all` that refuses to create through a non-directory.
    fn mkdirs(&self, dir: &Path) -> ApiResult<()> {
        match fs::create_dir_all(dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::AlreadyExists || e.kind() == ErrorKind::NotADirectory => {
                Err(ApiError::Conflict("a parent of the path is a file".into()))
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete_file(&self, rel: &str, if_match: Option<&str>) -> ApiResult<()> {
        let (abs, norm) = self.resolve_non_root(rel)?;
        let _g = self.guard();
        let meta = fs::metadata(&abs).map_err(|e| not_found(e, "file"))?;
        if meta.is_dir() {
            return Err(ApiError::Conflict(format!("{norm:?} is a directory")));
        }
        if let Some(want) = if_match.filter(|w| *w != "*") {
            let have = etag_of(&fs::read(&abs)?);
            if have != want {
                return Err(ApiError::PreconditionFailed(format!("{norm:?} has changed (etag {have})")));
            }
        }
        fs::remove_file(&abs).map_err(|e| not_found(e, "file"))
    }

    /// Create a directory (and parents). Returns whether it was newly created.
    pub fn create_dir(&self, rel: &str) -> ApiResult<(Entry, bool)> {
        let (abs, norm) = self.resolve_non_root(rel)?;
        let _g = self.guard();
        let created = match fs::metadata(&abs) {
            Ok(m) if m.is_dir() => false,
            Ok(_) => return Err(ApiError::Conflict(format!("{norm:?} is a file"))),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                self.mkdirs(&abs)?;
                true
            }
            Err(e) => return Err(e.into()),
        };
        let meta = fs::metadata(&abs)?;
        Ok((Self::entry(norm, &meta), created))
    }

    pub fn delete_dir(&self, rel: &str, recursive: bool) -> ApiResult<()> {
        let (abs, norm) = self.resolve_non_root(rel)?;
        let _g = self.guard();
        let meta = fs::metadata(&abs).map_err(|e| not_found(e, "directory"))?;
        if !meta.is_dir() {
            return Err(ApiError::Conflict(format!("{norm:?} is a file")));
        }
        if recursive {
            fs::remove_dir_all(&abs)?;
        } else if fs::read_dir(&abs)?.next().is_some() {
            return Err(ApiError::Conflict(format!(
                "{norm:?} is not empty; pass recursive=true to delete it and its contents"
            )));
        } else {
            fs::remove_dir(&abs)?;
        }
        Ok(())
    }

    /// Move or rename a file or directory. Parent directories of `to` are
    /// created; an existing `to` is a conflict.
    pub fn rename(&self, from: &str, to: &str) -> ApiResult<Entry> {
        let (from_abs, from_norm) = self.resolve_non_root(from)?;
        let (to_abs, to_norm) = self.resolve_non_root(to)?;
        let _g = self.guard();
        let meta = fs::symlink_metadata(&from_abs).map_err(|e| not_found(e, "source"))?;
        if from_norm != to_norm {
            if meta.is_dir() && to_norm.starts_with(&format!("{from_norm}/")) {
                return Err(ApiError::BadRequest("cannot move a directory into itself".into()));
            }
            if fs::symlink_metadata(&to_abs).is_ok() {
                return Err(ApiError::Conflict(format!("{to_norm:?} already exists")));
            }
            self.mkdirs(to_abs.parent().expect("non-root path has a parent"))?;
            fs::rename(&from_abs, &to_abs)?;
        }
        let meta = fs::metadata(&to_abs)?;
        Ok(Self::entry(to_norm, &meta))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(name: &str) -> FileRoot {
        let dir = std::env::temp_dir().join(format!("dsys-files-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        FileRoot::new(&dir).unwrap()
    }

    #[test]
    fn rejects_escaping_paths() {
        for bad in ["..", "a/../../b", "a//b", "./a", "a/.", "a\\b", "a\0b"] {
            assert!(parse_rel(bad).is_err(), "{bad:?} should be rejected");
        }
        assert_eq!(parse_rel("/a/b/").unwrap(), vec!["a", "b"]);
        assert!(parse_rel("").unwrap().is_empty());
    }

    #[test]
    fn refuses_symlinks() {
        let r = tmp_root("symlink");
        let outside = std::env::temp_dir().join(format!("dsys-outside-{}", std::process::id()));
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, r.root().join("link")).unwrap();
        assert!(matches!(r.write_file("link/x.md", b"hi", &Default::default()), Err(ApiError::BadRequest(_))));
        assert!(!outside.join("x.md").exists());
        let tree = r.tree("", false, false).unwrap();
        assert!(tree.children.unwrap().is_empty(), "symlinks are not listed");
    }

    #[test]
    fn write_read_move_delete() {
        let r = tmp_root("crud");
        let (e, created) = r.write_file("Notes/a.md", b"one", &Default::default()).unwrap();
        assert!(created);
        assert_eq!(e.path, "Notes/a.md");
        let etag = e.etag.unwrap();

        // Create-only fails once it exists; If-Match must match.
        let create_only = WriteCondition { if_none_match_any: true, ..Default::default() };
        assert!(matches!(r.write_file("Notes/a.md", b"x", &create_only), Err(ApiError::PreconditionFailed(_))));
        let stale = WriteCondition { if_match: Some("\"0\"".into()), ..Default::default() };
        assert!(matches!(r.write_file("Notes/a.md", b"x", &stale), Err(ApiError::PreconditionFailed(_))));
        let fresh = WriteCondition { if_match: Some(etag), ..Default::default() };
        let (_, created) = r.write_file("Notes/a.md", b"two", &fresh).unwrap();
        assert!(!created);
        assert_eq!(r.read_file("Notes/a.md").unwrap().0, b"two");

        r.rename("Notes", "Archive/Notes").unwrap();
        assert!(matches!(r.read_file("Notes/a.md"), Err(ApiError::NotFound(_))));
        assert_eq!(r.read_file("Archive/Notes/a.md").unwrap().0, b"two");
        assert!(matches!(r.rename("Archive", "Archive/Notes/x"), Err(ApiError::BadRequest(_))));

        let tree = r.tree("", true, false).unwrap();
        let archive = &tree.children.as_ref().unwrap()[0];
        let note = &archive.children.as_ref().unwrap()[0].children.as_ref().unwrap()[0];
        assert_eq!(note.content.as_deref(), Some("two"));

        assert!(matches!(r.delete_dir("Archive", false), Err(ApiError::Conflict(_))));
        r.delete_dir("Archive", true).unwrap();
        assert!(r.tree("", false, false).unwrap().children.unwrap().is_empty());
        assert!(matches!(r.delete_dir("", true), Err(ApiError::BadRequest(_))));
    }
}
