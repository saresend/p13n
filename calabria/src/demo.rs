//! An in-browser stand-in for dsys, used when the server is set to `demo`.
//! It keeps the same file tree and action log in localStorage, with the same
//! error behavior (404 / 409 / 412), so the app can be tried without a server.

use std::collections::{BTreeMap, BTreeSet};

use gloo_storage::{LocalStorage, Storage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::{ActionRecord, ApiError, FsEntry, SOURCE_APPLICATION};

pub const DEMO_BACKEND: &str = "demo";
const KEY: &str = "calabria.demo-server.v4";

#[derive(Serialize, Deserialize, Default)]
struct Store {
    dirs: BTreeSet<String>,
    files: BTreeMap<String, String>,
    /// Last-modified time of each file, epoch ms.
    #[serde(default)]
    modified: BTreeMap<String, i64>,
    actions: Vec<ActionRecord>,
}

pub fn is_demo(base: &str) -> bool {
    base.trim().eq_ignore_ascii_case(DEMO_BACKEND)
}

fn load() -> Store {
    LocalStorage::get(KEY).unwrap_or_else(|_| sample())
}

fn save(s: &Store) -> Result<(), ApiError> {
    LocalStorage::set(KEY, s).map_err(|e| ApiError::Status(507, format!("browser storage is full or blocked: {e}")))
}

fn err(code: u16, msg: impl Into<String>) -> ApiError {
    ApiError::Status(code, msg.into())
}

fn parent(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(p, _)| p)
}

fn check_path(path: &str) -> Result<(), ApiError> {
    let ok = !path.is_empty() && path.split('/').all(|s| !s.is_empty() && s != "." && s != "..");
    if ok { Ok(()) } else { Err(err(400, format!("invalid path {path:?}"))) }
}

fn add_parents(s: &mut Store, path: &str) -> Result<(), ApiError> {
    let mut p = parent(path);
    while let Some(dir) = p {
        if s.files.contains_key(dir) {
            return Err(err(409, "a parent of the path is a file"));
        }
        s.dirs.insert(dir.to_string());
        p = parent(dir);
    }
    Ok(())
}

fn exists(s: &Store, path: &str) -> bool {
    s.dirs.contains(path) || s.files.contains_key(path)
}

pub fn tree() -> Result<FsEntry, ApiError> {
    let s = load();
    fn children(s: &Store, dir: &str) -> Vec<FsEntry> {
        let is_child = |p: &str| match parent(p) {
            Some(par) => par == dir,
            None => dir.is_empty(),
        };
        let name = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
        let mut dirs: Vec<FsEntry> = s
            .dirs
            .iter()
            .filter(|p| is_child(p))
            .map(|p| FsEntry {
                name: name(p),
                kind: "dir".into(),
                modified_ms: 0,
                content: None,
                children: Some(children(s, p)),
            })
            .collect();
        let mut files: Vec<FsEntry> = s
            .files
            .iter()
            .filter(|(p, _)| is_child(p))
            .map(|(p, c)| FsEntry {
                name: name(p),
                kind: "file".into(),
                modified_ms: s.modified.get(p).copied().unwrap_or(0),
                content: Some(c.clone()),
                children: None,
            })
            .collect();
        dirs.sort_by_key(|e| e.name.to_lowercase());
        files.sort_by_key(|e| e.name.to_lowercase());
        dirs.extend(files);
        dirs
    }
    Ok(FsEntry {
        name: String::new(),
        kind: "dir".into(),
        modified_ms: 0,
        content: None,
        children: Some(children(&s, "")),
    })
}

pub fn read_file(path: &str) -> Result<Option<String>, ApiError> {
    Ok(load().files.get(path).cloned())
}

pub fn write_file(path: &str, body: &str, create_only: bool) -> Result<(), ApiError> {
    check_path(path)?;
    let mut s = load();
    if s.dirs.contains(path) {
        return Err(err(409, format!("{path:?} is a directory")));
    }
    if create_only && s.files.contains_key(path) {
        return Err(err(412, format!("{path:?} already exists")));
    }
    add_parents(&mut s, path)?;
    s.files.insert(path.to_string(), body.to_string());
    s.modified.insert(path.to_string(), js_sys::Date::now() as i64);
    save(&s)
}

pub fn move_entry(from: &str, to: &str) -> Result<(), ApiError> {
    check_path(from)?;
    check_path(to)?;
    let mut s = load();
    if !exists(&s, from) {
        return Err(err(404, "source not found"));
    }
    if from == to {
        return Ok(());
    }
    if exists(&s, to) {
        return Err(err(409, format!("{to:?} already exists")));
    }
    if to.starts_with(&format!("{from}/")) {
        return Err(err(400, "cannot move a directory into itself"));
    }
    add_parents(&mut s, to)?;
    let re = |p: &str| -> Option<String> {
        if p == from {
            Some(to.to_string())
        } else {
            p.strip_prefix(&format!("{from}/")).map(|rest| format!("{to}/{rest}"))
        }
    };
    s.dirs = s.dirs.iter().map(|p| re(p).unwrap_or_else(|| p.clone())).collect();
    s.files = s.files.into_iter().map(|(p, c)| (re(&p).unwrap_or(p), c)).collect();
    s.modified = s.modified.into_iter().map(|(p, t)| (re(&p).unwrap_or(p), t)).collect();
    save(&s)
}

pub fn delete_file(path: &str) -> Result<(), ApiError> {
    let mut s = load();
    if s.dirs.contains(path) {
        return Err(err(409, format!("{path:?} is a directory")));
    }
    s.files.remove(path).ok_or_else(|| err(404, "file not found"))?;
    save(&s)
}

pub fn record_action(action_name: &str, payload: &Value, timestamp: i64) -> Result<(), ApiError> {
    let mut s = load();
    let id = s.actions.last().map_or(1, |a| a.id + 1);
    s.actions.push(ActionRecord {
        id,
        source_application: SOURCE_APPLICATION.into(),
        action_name: action_name.into(),
        payload: payload.clone(),
        timestamp,
    });
    // Keep the log from growing without bound in browser storage.
    let excess = s.actions.len().saturating_sub(500);
    s.actions.drain(..excess);
    save(&s)
}

pub fn recent_actions(limit: usize) -> Result<Vec<ActionRecord>, ApiError> {
    Ok(load().actions.into_iter().rev().take(limit).collect())
}

/// What a fresh demo server starts with.
fn sample() -> Store {
    let mut s = Store::default();
    let now = js_sys::Date::now() as i64;
    let mut put = |path: &str, body: &str, minutes_ago: i64| {
        let _ = add_parents(&mut s, path);
        s.files.insert(path.into(), body.into());
        s.modified.insert(path.into(), now - minutes_ago * 60_000);
    };
    put(
        "Forgiving software.md",
        "# Forgiving software\n\n\
Software needs to be **more forgiving**, not in the sense of *end user* software, but rather, \
_development_ software. Compilers, interpreters, and so on.\n\n\
## What that looks like\n\n\
- Errors that `explain` themselves\n\
- Tools that **guess well**\n\
1. Recover\n\
2. Continue\n\n\
> The compiler is a collaborator, not a gatekeeper.\n\n\
See [the essay](https://en.wikipedia.org/wiki/Robustness_principle) for the old version of this idea. \
Ctrl/Cmd+click a link to open it.\n\n\
```rust\n\
// A recoverable error instead of a crash.\n\
fn parse_port(s: &str) -> Result<u16, String> {\n\
    s.trim().parse::<u16>().map_err(|e| format!(\"bad port {s:?}: {e}\"))\n\
}\n\
```\n\n\
```python\n\
def total(prices, qty=1):\n\
    return sum(p * qty for p in prices)  # 0 when empty\n\
```\n\n\
Select some text to format it, or break a passage out into its own note.\n",
        1,
    );
    put(
        "Plan the Lisbon trip.md",
        "# Plan the Lisbon trip\n\nPress Esc to see the agent's progress on this list, and Esc again to come back.\n\n\
- [ ] Compare flights for the second week of May\n\
- [x] Renew passport\n\
- [ ] Shortlist three hotels near Alfama\n\
- [ ] Draft a rough day-by-day itinerary\n",
        0,
    );
    put("Free and open source.md", "# Free and open source\n\nTools I'd like to try:\n\n- [ ] A forgiving parser\n- [x] A REPL with undo\n", 30);
    put("Hack your own.md", "# Hack your own\n\n", 120);
    put("Archive/Agile methodology.md", "# Agile methodology\n\nOld notes from a workshop.\n", 60 * 24 * 7);
    s
}
