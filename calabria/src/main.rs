use std::collections::BTreeSet;
use std::rc::Rc;

use gloo_storage::{LocalStorage, Storage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use web_sys::wasm_bindgen::JsCast;
use web_sys::{HtmlInputElement, HtmlTextAreaElement};
use yew::prelude::*;

mod agent_tasks;
mod api;
mod demo;
mod dictation;
mod markdown;
mod progress;
mod sync;

use sync::{Op, Outbox, SyncStatus};

/// Where notes lived before dsys; imported once into an empty server.
const LEGACY_KEY: &str = "calabria.tree.v1";
const MIGRATED_KEY: &str = "calabria.migrated-to-dsys";
const PREFS_KEY: &str = "calabria.prefs.v1";
/// Override at build time with `CALABRIA_DSYS_URL=... trunk build`.
const DEFAULT_BACKEND: &str = match option_env!("CALABRIA_DSYS_URL") {
    Some(url) => url,
    None => "http://127.0.0.1:8081",
};
/// Archived notes live in this directory on the server.
const ARCHIVE_DIR: &str = "Archive";
/// Save a note this long after typing stops.
const SAVE_DELAY_MS: u32 = 600;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// A note is a `<name>.md` file. New notes go at the top level; archived ones
/// in `Archive/`. Notes found in other directories are listed too, and stay
/// where they are.
#[derive(Clone, PartialEq, Debug)]
struct Note {
    id: u64,
    /// Directory on the server, `""` for the top level.
    dir: String,
    name: String,
    body: String,
    /// Unix epoch ms of the last change; the list is newest first.
    modified: i64,
}

impl Note {
    fn path(&self) -> String {
        if self.dir.is_empty() { format!("{}.md", self.name) } else { format!("{}/{}.md", self.dir, self.name) }
    }
    fn archived(&self) -> bool {
        self.dir == ARCHIVE_DIR || self.dir.starts_with(&format!("{ARCHIVE_DIR}/"))
    }
}

/// The pre-dsys localStorage format (a tree of folders and notes).
#[derive(Deserialize)]
struct LegacyState {
    items: Vec<LegacyItem>,
}

#[derive(Deserialize)]
struct LegacyItem {
    name: String,
    kind: LegacyKind,
}

#[derive(Deserialize)]
enum LegacyKind {
    Folder { children: Vec<LegacyItem> },
    Note { body: String },
}

fn flatten_legacy(items: Vec<LegacyItem>, out: &mut Vec<(String, String)>) {
    for it in items {
        match it.kind {
            LegacyKind::Folder { children } => flatten_legacy(children, out),
            LegacyKind::Note { body } => out.push((it.name, body)),
        }
    }
}

/// Per-browser settings and view state.
#[derive(Serialize, Deserialize, Default, PartialEq)]
struct Prefs {
    backend: Option<String>,
    /// Path of the open note.
    selected: Option<String>,
    /// Run the stand-in progress agent (until a real one exists).
    #[serde(default)]
    simulate_agent: Option<bool>,
    /// File a "check for action items" task when a note is saved.
    #[serde(default)]
    send_to_agents: Option<bool>,
}

impl Prefs {
    fn load() -> Self {
        LocalStorage::get(PREFS_KEY).unwrap_or_default()
    }
}

#[derive(Clone, PartialEq)]
enum Load {
    Loading,
    Ready,
    Failed(String),
}

#[derive(Clone, PartialEq)]
struct State {
    /// Base URL of the dsys server (the system of record).
    backend: String,
    /// Bumped on every (re)connect; stale load results are ignored.
    generation: u64,
    load: Load,
    notes: Vec<Note>,
    selected: Option<u64>,
    next_id: u64,
    /// Notes whose body has changed since it was last queued for saving.
    dirty: BTreeSet<u64>,
    /// Bumped on every edit; drives the save debounce.
    edits: u64,
    /// The note being edited and its length when editing began.
    session: Option<(u64, usize)>,
    sync: SyncStatus,
    outbox: Outbox,
    /// File a "check for action items" task for agents on each save.
    send_to_agents: bool,
    /// The user's progress preferences (markdown), added to agent tasks.
    progress_prefs: String,
    /// Progress preferences changed since they were last queued for saving.
    prefs_dirty: bool,
}

/// Make a user-typed name safe to use as a file name.
fn clean_name(raw: &str) -> String {
    let s: String = raw.chars().map(|c| if matches!(c, '/' | '\\' | '\0') { '-' } else { c }).collect();
    let s = s.trim().trim_start_matches('.').trim();
    let s = s.strip_suffix(".md").unwrap_or(s).trim();
    if s.is_empty() { "Untitled".into() } else { s.into() }
}

/// Longest title taken from a first line, in characters.
const MAX_TITLE_CHARS: usize = 60;

/// A note's title: its first non-blank line, without markdown markers,
/// shortened at a word boundary, and safe to use as a file name.
///
/// This is the one place titles come from. Later this is meant to be set by a
/// lightweight LLM; it is applied in `State::flush`, when a note is saved.
fn title_from(body: &str) -> String {
    let line = body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let mut t = line.trim_start_matches('#').trim_start();
    for prefix in ["> ", ">", "- [ ] ", "- [x] ", "- [X] ", "- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(prefix) {
            t = rest.trim_start();
            break;
        }
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && matches!(t.as_bytes().get(digits), Some(b'.' | b')')) {
        t = t[digits + 1..].trim_start();
    }
    if t.starts_with("```") || t.starts_with("~~~") {
        t = "";
    }
    // [text](url) -> text, then drop emphasis and code markers.
    let mut plain = String::new();
    let mut rest = t;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        match (after.find("]("), after.find(')')) {
            (Some(mid), Some(close)) if mid < close => {
                plain.push_str(&rest[..open]);
                plain.push_str(&after[..mid]);
                rest = &after[close + 1..];
            }
            _ => {
                plain.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    plain.push_str(rest);
    let plain: String = plain.chars().filter(|c| !matches!(c, '*' | '`')).collect();
    let plain = plain.trim_matches('_').trim();

    let short = if plain.chars().count() > MAX_TITLE_CHARS {
        let cut: String = plain.chars().take(MAX_TITLE_CHARS).collect();
        match cut.rfind(' ') {
            Some(i) if i > MAX_TITLE_CHARS / 2 => cut[..i].trim_end().to_string(),
            _ => cut,
        }
    } else {
        plain.to_string()
    };
    clean_name(&short)
}

/// `base`, or `base 2`, `base 3`, ... so no other note in `dir` has the name.
fn unique_name(notes: &[Note], dir: &str, base: &str, except: Option<u64>) -> String {
    let taken = |n: &str| notes.iter().any(|o| Some(o.id) != except && o.dir == dir && o.name.eq_ignore_ascii_case(n));
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|c| !taken(c)).expect("some suffix is free")
}

/// Notes in list order: most recently changed first.
fn ordered(notes: &[Note], archived: bool) -> Vec<Note> {
    let mut v: Vec<Note> = notes.iter().filter(|n| n.archived() == archived).cloned().collect();
    v.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    v
}

/// Every `*.md` file in the server listing becomes a note.
fn notes_from_tree(entries: &[api::FsEntry], dir: &str, next_id: &mut u64, out: &mut Vec<Note>) {
    for e in entries {
        if e.name.starts_with('.') {
            continue; // App data such as .calabria/, not notes.
        }
        if e.kind == "dir" {
            let sub = if dir.is_empty() { e.name.clone() } else { format!("{dir}/{}", e.name) };
            notes_from_tree(e.children.as_deref().unwrap_or(&[]), &sub, next_id, out);
        } else if let (Some(stem), Some(body)) = (e.name.strip_suffix(".md"), &e.content) {
            out.push(Note { id: *next_id, dir: dir.into(), name: stem.into(), body: body.clone(), modified: e.modified_ms });
            *next_id += 1;
        }
    }
}

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

fn normalize_url(raw: &str) -> String {
    let s = raw.trim().trim_end_matches('/');
    if demo::is_demo(s) {
        return demo::DEMO_BACKEND.into();
    }
    if s.contains("://") { s.to_string() } else { format!("http://{s}") }
}

impl State {
    fn new() -> Self {
        let prefs = Prefs::load();
        Self {
            backend: prefs.backend.unwrap_or_else(|| DEFAULT_BACKEND.into()),
            generation: 0,
            load: Load::Loading,
            notes: Vec::new(),
            selected: None,
            next_id: 1,
            dirty: BTreeSet::new(),
            edits: 0,
            session: None,
            sync: SyncStatus::default(),
            outbox: Outbox::default(),
            send_to_agents: prefs.send_to_agents.unwrap_or(true),
            progress_prefs: String::new(),
            prefs_dirty: false,
        }
    }

    fn find(&self, id: u64) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }

    fn find_mut(&mut self, id: u64) -> Option<&mut Note> {
        self.notes.iter_mut().find(|n| n.id == id)
    }

    fn selected_note(&self) -> Option<&Note> {
        self.selected.and_then(|id| self.find(id))
    }

    fn alloc(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id - 1
    }

    fn path(&self, id: u64) -> String {
        self.find(id).map(Note::path).unwrap_or_default()
    }

    fn record(&self, action_name: &'static str, payload: Value) {
        self.outbox.push(Op::Record { action_name, payload, timestamp: now_ms() });
    }

    /// Queue saves for every note edited since the last flush, at their
    /// current paths. Called before anything that changes paths.
    ///
    /// The title follows the first line: when it has changed, the file is
    /// renamed first, then written under its new name.
    fn flush(&mut self) {
        if std::mem::take(&mut self.prefs_dirty) {
            self.outbox.push(Op::Write {
                path: agent_tasks::PROGRESS_PREFS_PATH.into(),
                body: self.progress_prefs.clone(),
                create_only: false,
            });
            self.record("settings.progress_preferences_updated", json!({ "chars": self.progress_prefs.chars().count() }));
        }
        for id in std::mem::take(&mut self.dirty) {
            let Some(note) = self.find(id).cloned() else { continue };
            let title = unique_name(&self.notes, &note.dir, &title_from(&note.body), Some(id));
            let mut path = note.path();
            if title != note.name {
                if let Some(n) = self.find_mut(id) {
                    n.name = title;
                }
                let to = self.path(id);
                self.move_note_files(&path, &to);
                self.record("note.renamed", json!({ "from": path, "to": to }));
                path = to;
            }
            let check = self.send_to_agents && !note.archived() && !note.body.trim().is_empty();
            self.outbox.push(Op::Write { path: path.clone(), body: note.body, create_only: false });
            if check {
                self.outbox.push(Op::RequestCheck { path, prefs: self.progress_prefs.clone() });
            }
        }
    }

    /// Record the end of an editing session on a note.
    fn end_session(&mut self) {
        if let Some((id, chars_before)) = self.session.take() {
            if let Some(n) = self.find(id) {
                self.record(
                    "note.edited",
                    json!({
                        "path": n.path(),
                        "chars_before": chars_before,
                        "chars_after": n.body.chars().count(),
                        "words": word_count(&n.body),
                    }),
                );
            }
        }
    }

    /// After the open note goes away, open the newest active note.
    fn reselect(&mut self) {
        if self.selected.is_some_and(|id| self.find(id).is_none_or(Note::archived)) {
            self.selected = ordered(&self.notes, false).first().map(|n| n.id);
        }
    }

    /// Move a note to `dir`, keeping its name unless it clashes there.
    fn relocate(&mut self, id: u64, dir: &str) -> Option<(String, String)> {
        self.flush();
        let note = self.find(id)?.clone();
        let name = unique_name(&self.notes, dir, &note.name, Some(id));
        let from = note.path();
        let n = self.find_mut(id)?;
        n.dir = dir.into();
        n.name = name;
        let to = n.path();
        self.move_note_files(&from, &to);
        Some((from, to))
    }

    /// Move a note and, if it has one, its progress report.
    fn move_note_files(&self, from: &str, to: &str) {
        self.outbox.push(Op::Move { from: from.into(), to: to.into(), if_exists: false });
        self.outbox.push(Op::Move {
            from: progress::progress_path(from),
            to: progress::progress_path(to),
            if_exists: true,
        });
    }
}

enum Action {
    /// Switch to (or reconnect to) a server.
    Connect(String),
    Loaded { generation: u64, notes: Vec<Note>, next_id: u64, selected: Option<u64>, imported: bool, progress_prefs: String },
    /// Edit the progress preferences (saved like a note, after a pause).
    SetProgressPrefs(String),
    LoadFailed { generation: u64, error: String },
    Sync(SyncStatus),
    Select(u64),
    SetBody(String),
    /// Queue saves for edited notes (fired by the debounce timer).
    Flush,
    /// The page is closing.
    Unload,
    NewNote,
    Archive(u64),
    Restore(u64),
    /// Permanently delete (offered for archived notes).
    Delete(u64),
    /// Move text out of the current note into a new note.
    Breakout { remaining: String, extracted: String },
    /// Write a note's progress report (from the simulated agent).
    WriteProgress { id: u64, html: String },
    SetSendToAgents(bool),
}

impl Reducible for State {
    type Action = Action;

    fn reduce(self: Rc<Self>, action: Action) -> Rc<Self> {
        let mut s = (*self).clone();
        match action {
            Action::Connect(backend) => {
                s.outbox.reset();
                s = State { backend, generation: s.generation + 1, outbox: s.outbox, ..State::new() };
            }
            Action::Loaded { generation, notes, next_id, selected, imported, progress_prefs } => {
                if generation != s.generation {
                    return self;
                }
                s.progress_prefs = progress_prefs;
                s.notes = notes;
                s.next_id = next_id;
                s.selected = selected;
                s.load = Load::Ready;
                if imported {
                    for n in &s.notes {
                        s.outbox.push(Op::Write { path: n.path(), body: n.body.clone(), create_only: true });
                    }
                    s.record("notes.imported", json!({ "notes": s.notes.len() }));
                }
            }
            Action::LoadFailed { generation, error } => {
                if generation == s.generation {
                    s.load = Load::Failed(error);
                }
            }
            Action::Sync(status) => s.sync = status,
            Action::Select(id) => {
                if s.selected == Some(id) {
                    return self;
                }
                s.flush();
                s.end_session();
                s.selected = Some(id);
                s.record("note.opened", json!({ "path": s.path(id) }));
            }
            Action::SetBody(text) => {
                let Some(id) = s.selected_note().map(|n| n.id) else { return self };
                if s.session.map(|(sid, _)| sid) != Some(id) {
                    s.end_session();
                    s.session = Some((id, s.find(id).map_or(0, |n| n.body.chars().count())));
                }
                if let Some(n) = s.find_mut(id) {
                    n.body = text;
                    n.modified = now_ms();
                }
                s.dirty.insert(id);
                s.edits += 1;
            }
            Action::SetProgressPrefs(text) => {
                s.progress_prefs = text;
                s.prefs_dirty = true;
                s.edits += 1;
            }
            Action::Flush => {
                if s.dirty.is_empty() && !s.prefs_dirty {
                    return self;
                }
                s.flush();
            }
            Action::Unload => {
                s.flush();
                s.end_session();
            }
            Action::NewNote => {
                s.flush();
                s.end_session();
                let name = unique_name(&s.notes, "", "Untitled", None);
                let id = s.alloc();
                let note = Note { id, dir: String::new(), name, body: String::new(), modified: now_ms() };
                let path = note.path();
                s.notes.push(note);
                s.outbox.push(Op::Write { path: path.clone(), body: String::new(), create_only: true });
                s.record("note.created", json!({ "path": path }));
                s.selected = Some(id);
            }
            Action::Archive(id) => {
                if s.session.is_some_and(|(sid, _)| sid == id) {
                    s.end_session();
                }
                let Some((from, to)) = s.relocate(id, ARCHIVE_DIR) else { return self };
                s.record("note.archived", json!({ "from": from, "to": to }));
                s.reselect();
            }
            Action::Restore(id) => {
                let Some((from, to)) = s.relocate(id, "") else { return self };
                s.record("note.restored", json!({ "from": from, "to": to }));
                if let Some(n) = s.find_mut(id) {
                    n.modified = now_ms();
                }
            }
            Action::Delete(id) => {
                let Some(note) = s.find(id).cloned() else { return self };
                s.dirty.remove(&id);
                if s.session.is_some_and(|(sid, _)| sid == id) {
                    s.session = None;
                }
                s.flush();
                let path = note.path();
                s.outbox.push(Op::DeleteFile { path: path.clone(), if_exists: false });
                s.outbox.push(Op::DeleteFile { path: progress::progress_path(&path), if_exists: true });
                s.record("note.deleted", json!({ "path": path }));
                s.notes.retain(|n| n.id != id);
                s.reselect();
            }
            Action::SetSendToAgents(on) => {
                s.send_to_agents = on;
                let _ = LocalStorage::set(PREFS_KEY, Prefs { send_to_agents: Some(on), ..Prefs::load() });
            }
            Action::WriteProgress { id, html } => {
                let Some(note) = s.find(id) else { return self };
                let path = progress::progress_path(&note.path());
                s.outbox.push(Op::Write { path, body: html, create_only: false });
                return s.into();
            }
            Action::Breakout { remaining, extracted } => {
                let Some(from_id) = s.selected_note().map(|n| n.id) else { return self };
                if let Some(n) = s.find_mut(from_id) {
                    n.body = remaining;
                    n.modified = now_ms();
                }
                s.dirty.insert(from_id);
                s.flush();
                s.end_session();
                let source = s.find(from_id).cloned().expect("selected note exists");
                // New notes from an archived note still start out active.
                let dir = if source.archived() { String::new() } else { source.dir.clone() };
                let name = unique_name(&s.notes, &dir, &title_from(&extracted), None);
                let id = s.alloc();
                let body = extracted.trim().to_string();
                let note = Note { id, dir, name, body: body.clone(), modified: now_ms() };
                let to = note.path();
                s.notes.push(note);
                s.outbox.push(Op::Write { path: to.clone(), body: body.clone(), create_only: true });
                s.record("note.breakout", json!({ "from": source.path(), "to": to, "chars": body.chars().count() }));
                s.selected = Some(id);
            }
        }
        s.into()
    }
}

// ---------------------------------------------------------------------------
// Notes list
// ---------------------------------------------------------------------------

#[derive(Properties, PartialEq)]
struct NoteRowProps {
    note: Note,
    active: bool,
    dispatch: Callback<Action>,
}

#[function_component(NoteRow)]
fn note_row(props: &NoteRowProps) -> Html {
    let NoteRowProps { note, active, dispatch } = props;
    let id = note.id;

    // Hover actions; stop the click from also opening the note.
    let act = |label: &'static str, title: &'static str, a: fn(u64) -> Action| {
        let dispatch = dispatch.clone();
        html! {
            <button {title} onclick={Callback::from(move |e: MouseEvent| {
                e.stop_propagation();
                dispatch.emit(a(id));
            })}>{ label }</button>
        }
    };
    let actions = if note.archived() {
        html! {
            <span class="actions">
                { act("Restore", "Move back to notes", Action::Restore) }
                { act("Delete", "Delete permanently", Action::Delete) }
            </span>
        }
    } else {
        html! { <span class="actions">{ act("Archive", "Move to the archive", Action::Archive) }</span> }
    };

    html! {
        <li>
            <div class={classes!("row", active.then_some("active"))}
                onclick={dispatch.reform(move |_: MouseEvent| Action::Select(id))}>
                <span class="name">{ &note.name }</span>
                { actions }
            </div>
        </li>
    }
}

#[derive(Properties, PartialEq)]
struct NoteListProps {
    notes: Vec<Note>,
    selected: Option<u64>,
    dispatch: Callback<Action>,
}

#[function_component(NoteList)]
fn note_list(props: &NoteListProps) -> Html {
    html! {
        <ul class="notes">
            { for props.notes.iter().map(|n| html! {
                <NoteRow key={n.id} note={n.clone()} active={props.selected == Some(n.id)}
                    dispatch={props.dispatch.clone()} />
            }) }
        </ul>
    }
}

// ---------------------------------------------------------------------------
// Editor
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Format {
    Bold,
    Italic,
    Code,
    Heading,
    Quote,
    List,
    Link,
}

/// Wrap `start..end` in `delim`, or unwrap it if it is already wrapped
/// (either inside or just outside the selection).
fn toggle_wrap(text: &str, start: usize, end: usize, delim: &str) -> String {
    let sel = &text[start..end];
    let d = delim.len();
    if sel.len() >= 2 * d && sel.starts_with(delim) && sel.ends_with(delim) {
        return format!("{}{}{}", &text[..start], &sel[d..sel.len() - d], &text[end..]);
    }
    if text[..start].ends_with(delim) && text[end..].starts_with(delim) {
        return format!("{}{sel}{}", &text[..start - d], &text[end + d..]);
    }
    // Keep surrounding spaces outside the markers: "**word** " not "**word **".
    let inner = sel.trim();
    let lead = &sel[..sel.len() - sel.trim_start().len()];
    let trail = &sel[sel.trim_end().len()..];
    format!("{}{lead}{delim}{inner}{delim}{trail}{}", &text[..start], &text[end..])
}

/// Apply a markdown-style format to `text` over the byte range `start..end`.
fn apply_format(text: &str, start: usize, end: usize, fmt: Format) -> String {
    match fmt {
        Format::Bold => toggle_wrap(text, start, end, "**"),
        Format::Italic => toggle_wrap(text, start, end, "*"),
        Format::Code => toggle_wrap(text, start, end, "`"),
        Format::Link => format!("{}[{}](https://){}", &text[..start], &text[start..end], &text[end..]),
        Format::Heading | Format::Quote | Format::List => {
            let prefix = match fmt {
                Format::Heading => "## ",
                Format::Quote => "> ",
                _ => "- ",
            };
            let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
            let block = &text[line_start..end];
            let toggled_off = block.lines().all(|l| l.is_empty() || l.starts_with(prefix));
            let formatted: Vec<String> = block
                .split('\n')
                .map(|l| match () {
                    _ if l.is_empty() => String::new(),
                    _ if toggled_off => l.trim_start_matches(prefix).to_string(),
                    _ => format!("{prefix}{l}"),
                })
                .collect();
            format!("{}{}{}", &text[..line_start], formatted.join("\n"), &text[end..])
        }
    }
}

/// Convert a UTF-16 offset (what the DOM reports) into a byte offset.
fn utf16_to_byte(s: &str, offset: u32) -> usize {
    let mut units = 0u32;
    for (i, c) in s.char_indices() {
        if units >= offset {
            return i;
        }
        units += c.len_utf16() as u32;
    }
    s.len()
}

#[derive(Properties, PartialEq)]
struct EditorProps {
    note_id: u64,
    body: String,
    dispatch: Callback<Action>,
    /// Bumped by the app to put the cursor in the editor.
    #[prop_or_default]
    focus: u64,
}

#[function_component(Editor)]
fn editor(props: &EditorProps) -> Html {
    let textarea = use_node_ref();
    let toolbar = use_state(|| None::<(i32, i32)>);

    // Put the cursor at the end of the note when asked to.
    {
        let textarea = textarea.clone();
        use_effect_with(props.focus, move |focus| {
            if *focus > 0 {
                if let Some(el) = textarea.cast::<HtmlTextAreaElement>() {
                    let _ = el.focus();
                    let end = el.value().encode_utf16().count() as u32;
                    let _ = el.set_selection_range(end, end);
                }
            }
        });
    }

    // Push-to-talk dictation: hold Fn (where the browser reports it) or Right
    // Alt/Option. The indicator shows only while the key is held; words are
    // inserted at the cursor as the speech engine settles them.
    let listening = use_state(|| None::<(String, Option<String>)>);
    {
        let textarea = textarea.clone();
        let dispatch = props.dispatch.clone();
        let listening = listening.clone();
        use_effect_with((), move |_| {
            let session = Rc::new(std::cell::RefCell::new(None::<dictation::Session>));
            let held = Rc::new(std::cell::Cell::new(None::<String>));
            let window = web_sys::window().expect("window");
            let is_hold = |k: &web_sys::KeyboardEvent| k.key() == "Fn" || k.code() == "Fn" || k.code() == "AltRight";

            let down = {
                let (session, held, listening) = (session.clone(), held.clone(), listening.clone());
                gloo_events::EventListener::new(&window, "keydown", move |e| {
                    let Some(k) = e.dyn_ref::<web_sys::KeyboardEvent>() else { return };
                    if let Some(code) = held.take() {
                        if k.code() == code {
                            held.set(Some(code)); // key repeat
                            return;
                        }
                        // Another key while holding Right Alt: it's a shortcut or
                        // an accented letter, not dictation. Cancel quietly.
                        if let Some(s) = session.borrow().as_ref() {
                            s.abort();
                        }
                        listening.set(None);
                        return;
                    }
                    if !is_hold(k) || k.repeat() || !dictation::supported() {
                        return;
                    }
                    e.prevent_default();
                    held.set(Some(k.code()));
                    listening.set(Some((String::new(), None)));
                    let on_event = {
                        let (textarea, dispatch, listening, held, session) =
                            (textarea.clone(), dispatch.clone(), listening.clone(), held.clone(), session.clone());
                        move |ev: dictation::Event| match ev {
                            dictation::Event::Interim(t) => {
                                if held.take().map(|c| held.set(Some(c))).is_some() {
                                    listening.set(Some((t, None)));
                                }
                            }
                            dictation::Event::Error(code) => {
                                if held.take().map(|c| held.set(Some(c))).is_some() {
                                    listening.set(Some((String::new(), Some(dictation::explain(&code)))));
                                }
                            }
                            dictation::Event::Final(words) => {
                                let Some(el) = textarea.cast::<HtmlTextAreaElement>() else { return };
                                let value = el.value();
                                let at = el.selection_end().ok().flatten().map_or(value.len(), |u| utf16_to_byte(&value, u));
                                let (next, end) = dictation::insert(&value, at, &words);
                                el.set_value(&next);
                                let caret = next[..end].encode_utf16().count() as u32;
                                let _ = el.set_selection_range(caret, caret);
                                dispatch.emit(Action::SetBody(next));
                            }
                            dictation::Event::End => {
                                // Free the session after this callback returns.
                                let session = session.clone();
                                gloo_timers::callback::Timeout::new(0, move || drop(session.borrow_mut().take())).forget();
                            }
                        }
                    };
                    // A previous session may still be finishing; replace it.
                    *session.borrow_mut() = dictation::Session::start(on_event);
                })
            };
            let up = {
                let (session, held, listening) = (session.clone(), held.clone(), listening.clone());
                gloo_events::EventListener::new(&window, "keyup", move |e| {
                    let Some(k) = e.dyn_ref::<web_sys::KeyboardEvent>() else { return };
                    match held.take() {
                        Some(code) if code == k.code() => {
                            if let Some(s) = session.borrow().as_ref() {
                                s.stop();
                            }
                            listening.set(None);
                        }
                        other => held.set(other),
                    }
                })
            };
            let blur = {
                let (session, held, listening) = (session.clone(), held.clone(), listening.clone());
                gloo_events::EventListener::new(&window, "blur", move |_| {
                    if held.take().is_some() {
                        if let Some(s) = session.borrow().as_ref() {
                            s.stop();
                        }
                        listening.set(None);
                    }
                })
            };
            move || {
                drop((down, up, blur));
                drop(session.borrow_mut().take());
            }
        });
    }

    // Grow the textarea to fit its content so the page scrolls, not the field.
    {
        let textarea = textarea.clone();
        use_effect_with(props.body.clone(), move |_| {
            if let Some(el) = textarea.cast::<HtmlTextAreaElement>() {
                let style = el.style();
                let _ = style.set_property("height", "auto");
                let _ = style.set_property("height", &format!("{}px", el.scroll_height()));
            }
        });
    }

    let selection = {
        let textarea = textarea.clone();
        move || -> Option<(String, usize, usize)> {
            let el = textarea.cast::<HtmlTextAreaElement>()?;
            let value = el.value();
            let s = el.selection_start().ok()??;
            let e = el.selection_end().ok()??;
            (e > s).then(|| (value.clone(), utf16_to_byte(&value, s), utf16_to_byte(&value, e)))
        }
    };

    let on_input = props.dispatch.reform(|e: InputEvent| {
        Action::SetBody(e.target_unchecked_into::<HtmlTextAreaElement>().value())
    });

    let on_mouseup = {
        let toolbar = toolbar.clone();
        let selection = selection.clone();
        Callback::from(move |e: MouseEvent| {
            toolbar.set(selection().map(|_| (e.client_x(), e.client_y())));
        })
    };
    let on_keyup = {
        let toolbar = toolbar.clone();
        let selection = selection.clone();
        Callback::from(move |_: KeyboardEvent| {
            if selection().is_none() {
                toolbar.set(None);
            }
        })
    };

    let button = |label: &'static str, f: Rc<dyn Fn(String, usize, usize) -> Action>| {
        let dispatch = props.dispatch.clone();
        let toolbar = toolbar.clone();
        let selection = selection.clone();
        html! {
            <button
                // Keep the textarea selection alive while clicking.
                onmousedown={Callback::from(|e: MouseEvent| e.prevent_default())}
                onclick={Callback::from(move |_| {
                    if let Some((text, s, e)) = selection() {
                        dispatch.emit(f(text, s, e));
                    }
                    toolbar.set(None);
                })}
            >{ label }</button>
        }
    };
    let fmt = |f: Format| -> Rc<dyn Fn(String, usize, usize) -> Action> {
        Rc::new(move |t, s, e| Action::SetBody(apply_format(&t, s, e, f)))
    };
    let breakout: Rc<dyn Fn(String, usize, usize) -> Action> = Rc::new(|t, s, e| Action::Breakout {
        remaining: format!("{}{}", t[..s].trim_end(), {
            let rest = t[e..].trim_start();
            if rest.is_empty() || s == 0 { rest.to_string() } else { format!("\n\n{rest}") }
        }),
        extracted: t[s..e].to_string(),
    });

    let popover = toolbar.map(|(x, y)| {
        let style = format!("left:{x}px;top:{}px", y + 18);
        html! {
            <div class="format-bar" {style}>
                { button("Bold", fmt(Format::Bold)) }
                { button("Italic", fmt(Format::Italic)) }
                { button("Code", fmt(Format::Code)) }
                <span class="divider" />
                { button("Heading", fmt(Format::Heading)) }
                { button("Quote", fmt(Format::Quote)) }
                { button("List", fmt(Format::List)) }
                { button("Link", fmt(Format::Link)) }
                <span class="divider" />
                { button("Breakout Paragraph", breakout) }
            </div>
        }
    });

    // Ctrl/Cmd+click on a link opens it.
    let on_click = {
        let textarea = textarea.clone();
        Callback::from(move |e: MouseEvent| {
            if !(e.ctrl_key() || e.meta_key()) {
                return;
            }
            let Some(el) = textarea.cast::<HtmlTextAreaElement>() else { return };
            let value = el.value();
            let Some(at) = el.selection_start().ok().flatten() else { return };
            if let Some(url) = markdown::link_at(&value, utf16_to_byte(&value, at)) {
                e.prevent_default();
                if let Some(w) = web_sys::window() {
                    let _ = w.open_with_url_and_target(&url, "_blank");
                }
            }
        })
    };

    html! {
        <>
            <div class="editor">
                <div class="body highlight" aria-hidden="true">{ markdown::render(&props.body) }</div>
                <textarea
                    ref={textarea}
                    class="body source"
                    placeholder="Begin writing…"
                    spellcheck="true"
                    value={props.body.clone()}
                    oninput={on_input}
                    onclick={on_click}
                    onmouseup={on_mouseup}
                    onkeyup={on_keyup}
                    onblur={{ let t = toolbar.clone(); Callback::from(move |_| t.set(None)) }}
                />
            </div>
            // Always present, so showing the toolbar never changes the shape
            // of this list (which made Yew rebuild the textarea and drop the selection).
            <div class="format-layer">{ for popover }</div>
            <div class="dictation-layer">{
                for listening.as_ref().map(|(_, error)| match error {
                    Some(e) => html! {
                        <div class="dictation failed" role="status" aria-live="polite">
                            <span class="message">{ e.clone() }</span>
                        </div>
                    },
                    // While the key is held: just a moving waveform.
                    None => html! {
                        <div class="dictation" role="status" aria-label="Listening">
                            <span class="wave" aria-hidden="true">
                                { for (0..WAVE_BARS).map(|i| html! {
                                    <span class="bar" style={format!("animation-delay:-{}ms", i * 120)} />
                                }) }
                            </span>
                        </div>
                    },
                })
            }</div>
        </>
    }
}

// ---------------------------------------------------------------------------
// Settings: the system-of-record server
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
enum Probe {
    Idle,
    Checking,
    Ok,
    Failed(String),
}

#[derive(Properties, PartialEq)]
struct SettingsProps {
    open: bool,
    on_open: Callback<bool>,
    backend: String,
    load: Load,
    sync: SyncStatus,
    unsaved: usize,
    notes: usize,
    dispatch: Callback<Action>,
    simulate: bool,
    on_simulate: Callback<bool>,
    send_to_agents: bool,
}

/// The usual address of a dsys running on this machine.
const DEFAULT_LOCAL: &str = "http://127.0.0.1:8081";

/// The most useful part of an action's payload, for one line.
fn action_detail(payload: &Value) -> String {
    match (payload["from"].as_str(), payload["to"].as_str(), payload["path"].as_str()) {
        (Some(from), Some(to), _) => format!("{from} → {to}"),
        (_, _, Some(path)) => path.to_string(),
        _ => payload["notes"].as_u64().map(|n| format!("{n} notes")).unwrap_or_default(),
    }
}

fn ago(timestamp_ms: i64) -> String {
    let secs = ((now_ms() - timestamp_ms) / 1000).max(0);
    match secs {
        0..=9 => "just now".into(),
        10..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

#[function_component(Settings)]
fn settings(props: &SettingsProps) -> Html {
    let draft = use_state(|| props.backend.clone());
    let probe = use_state(|| Probe::Idle);
    let activity = use_state(|| None::<Result<Vec<api::ActionRecord>, String>>);

    // Recent actions from the current server, refreshed as changes are sent.
    {
        let activity = activity.clone();
        let backend = props.backend.clone();
        let ready = props.load == Load::Ready;
        use_effect_with((props.open, ready, props.sync.pending, backend), move |(open, ready, _, backend)| {
            if *open && *ready {
                let backend = backend.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    activity.set(Some(api::recent_actions(&backend, 6).await.map_err(|e| e.message())));
                });
            }
        });
    }

    // Start from the current server each time the panel opens.
    {
        let draft = draft.clone();
        let probe = probe.clone();
        let backend = props.backend.clone();
        use_effect_with(props.open, move |open| {
            if *open {
                draft.set(backend);
                probe.set(Probe::Idle);
            }
        });
    }

    let (dot, summary) = match (&props.load, &props.sync) {
        (Load::Failed(_), _) => ("err", "Can't reach the notes server".to_string()),
        (_, SyncStatus { error: Some(e), .. }) => ("err", e.clone()),
        (Load::Loading, _) => ("busy", "Connecting…".to_string()),
        _ if props.unsaved > 0 || props.sync.pending > 0 => ("busy", "Saving…".to_string()),
        _ => ("ok", "All changes saved".to_string()),
    };

    let toggle = {
        let on_open = props.on_open.clone();
        let open = props.open;
        Callback::from(move |_: MouseEvent| on_open.emit(!open))
    };
    let close = props.on_open.reform(|_: MouseEvent| false);

    let connect = {
        let dispatch = props.dispatch.clone();
        let on_open = props.on_open.clone();
        let draft = draft.clone();
        Rc::new(move || {
            dispatch.emit(Action::Connect(normalize_url(&draft)));
            on_open.emit(false);
        })
    };
    let test = {
        let draft = draft.clone();
        let probe = probe.clone();
        Callback::from(move |_: MouseEvent| {
            let url = normalize_url(&draft);
            let probe = probe.clone();
            probe.set(Probe::Checking);
            wasm_bindgen_futures::spawn_local(async move {
                probe.set(match api::health(&url).await {
                    Ok(()) => Probe::Ok,
                    Err(e) => Probe::Failed(e.message()),
                });
            });
        })
    };
    let on_input = {
        let draft = draft.clone();
        let probe = probe.clone();
        Callback::from(move |e: InputEvent| {
            draft.set(e.target_unchecked_into::<HtmlInputElement>().value());
            probe.set(Probe::Idle);
        })
    };
    let on_key = {
        let connect = connect.clone();
        let on_open = props.on_open.clone();
        Callback::from(move |e: KeyboardEvent| match e.key().as_str() {
            "Enter" => connect(),
            "Escape" => on_open.emit(false),
            _ => {}
        })
    };
    let preset = |url: &'static str| {
        let draft = draft.clone();
        let probe = probe.clone();
        Callback::from(move |_: MouseEvent| {
            draft.set(url.to_string());
            probe.set(Probe::Idle);
        })
    };

    let demo_mode = demo::is_demo(&props.backend);
    let current = match &props.load {
        Load::Ready => format!(
            "{} · {} note{}",
            if demo_mode { "Demo server in this browser" } else { "Connected" },
            props.notes,
            if props.notes == 1 { "" } else { "s" }
        ),
        Load::Loading => "Connecting…".into(),
        Load::Failed(e) => format!("Not connected · {e}"),
    };
    let changing = normalize_url(&draft) != props.backend;
    let unsaved = props.unsaved + props.sync.pending;

    let panel = props.open.then(|| html! {
        <>
            <div class="scrim" onclick={close.clone()} />
            <div class="settings-panel" role="dialog" aria-label="System of record">
                <h2>{ "System of record" }</h2>
                <p class="hint">{ "Notes are stored on this dsys server, and what you do here is recorded to it." }</p>
                <label for="backend-url">{ "Server" }</label>
                <input id="backend-url" type="url" spellcheck="false" autofocus=true
                    value={(*draft).clone()} oninput={on_input} onkeydown={on_key} />
                <p class={classes!("status", matches!(props.load, Load::Failed(_)).then_some("bad"))}>
                    <span class={classes!("dot", dot)} />{ current }
                </p>
                if let Some(err) = &props.sync.error {
                    <p class="status bad">{ err }</p>
                }
                {
                    match &*probe {
                        Probe::Idle => html! {},
                        Probe::Checking => html! { <p class="status">{ "Checking…" }</p> },
                        Probe::Ok => html! { <p class="status good">{ "Server is reachable." }</p> },
                        Probe::Failed(e) => html! { <p class="status bad">{ format!("Not reachable: {e}") }</p> },
                    }
                }
                if changing && unsaved > 0 {
                    <p class="status bad">
                        { format!("{unsaved} unsaved change{} will be discarded.", if unsaved == 1 { "" } else { "s" }) }
                    </p>
                }
                <label class="check">
                    <input id="send-to-agents" type="checkbox" checked={props.send_to_agents}
                        onchange={props.dispatch.reform(|e: Event| Action::SetSendToAgents(
                            e.target_unchecked_into::<HtmlInputElement>().checked()))} />
                    <span>{ "Send note changes to agents" }</span>
                </label>
                <p class="hint small">{ "On save, files a task asking an agent to check the note for action items." }</p>
                <label class="check">
                    <input id="simulate-agent" type="checkbox" checked={props.simulate}
                        onchange={props.on_simulate.reform(|e: Event| e.target_unchecked_into::<HtmlInputElement>().checked())} />
                    <span>{ "Simulate the progress agent" }</span>
                </label>
                <p class="hint small">{ "Writes each note's _progress.html from its checklist until a real agent does." }</p>
                if props.load == Load::Ready {
                    <div class="activity">
                        <label>{ "Recent activity" }</label>
                        {
                            match &*activity {
                                None => html! { <p class="status">{ "Loading…" }</p> },
                                Some(Err(e)) => html! { <p class="status bad">{ format!("Couldn't load activity: {e}") }</p> },
                                Some(Ok(rows)) if rows.is_empty() => html! { <p class="status">{ "Nothing recorded yet." }</p> },
                                Some(Ok(rows)) => html! {
                                    <ol>
                                        { for rows.iter().map(|a| html! {
                                            <li key={a.id}>
                                                <span class="what">{ &a.action_name }</span>
                                                <span class="detail">{ action_detail(&a.payload) }</span>
                                                <span class="when">{ ago(a.timestamp) }</span>
                                            </li>
                                        }) }
                                    </ol>
                                },
                            }
                        }
                    </div>
                }
                <div class="buttons">
                    <button class="quiet" onclick={preset(DEFAULT_LOCAL)}>{ "Use local" }</button>
                    <button class="quiet" onclick={preset(demo::DEMO_BACKEND)}>{ "Demo" }</button>
                    <span class="spacer" />
                    <button class="quiet" onclick={test}>{ "Test" }</button>
                    <button class="primary" onclick={{ let c = connect.clone(); Callback::from(move |_| c()) }}>
                        { if changing { "Connect" } else { "Reconnect" } }
                    </button>
                </div>
            </div>
        </>
    });

    html! {
        <div class="settings">
            <button class={classes!("settings-toggle", props.open.then_some("open"))}
                title={summary} aria-label="Server settings" onclick={toggle}>
                <span class={classes!("dot", dot)} />
                <svg viewBox="0 0 16 16" aria-hidden="true">
                    <circle cx="8" cy="8" r="2.2" fill="none" stroke="currentColor" stroke-width="1.2"/>
                    <path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4"
                        stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
                </svg>
            </button>
            { for panel }
        </div>
    }
}

// ---------------------------------------------------------------------------
// Settings page
// ---------------------------------------------------------------------------

#[derive(Properties, PartialEq)]
struct SettingsPageProps {
    prefs: String,
    saving: bool,
    dispatch: Callback<Action>,
    on_back: Callback<MouseEvent>,
}

#[function_component(SettingsPage)]
fn settings_page(props: &SettingsPageProps) -> Html {
    let textarea = use_node_ref();
    // Grow with the text, like the note editor.
    {
        let textarea = textarea.clone();
        use_effect_with(props.prefs.clone(), move |_| {
            if let Some(el) = textarea.cast::<HtmlTextAreaElement>() {
                let style = el.style();
                let _ = style.set_property("height", "auto");
                let _ = style.set_property("height", &format!("{}px", el.scroll_height().max(140)));
            }
        });
    }
    let on_input = props.dispatch.reform(|e: InputEvent| {
        Action::SetProgressPrefs(e.target_unchecked_into::<HtmlTextAreaElement>().value())
    });
    let preview = agent_tasks::progress_instructions("<note>_progress.html", &props.prefs);

    html! {
        <div class="settings-page">
            <button class="back" onclick={props.on_back.clone()}>{ "← Back to writing" }</button>
            <h1>{ "Settings" }</h1>
            <section>
                <div class="section-head">
                    <h2>{ "Progress preferences" }</h2>
                    <span class="saved">{ if props.saving { "Saving…" } else { "Saved" } }</span>
                </div>
                <p class="hint">{ "How you'd like agents to report progress. This is added to every task Calabria files when a note is saved, right after the instruction to keep the note's progress page up to date. Markdown is fine." }</p>
                <textarea ref={textarea} id="progress-prefs" class="prefs" spellcheck="true"
                    placeholder={"For example:\n- Start with one line saying what's done and what's next.\n- Show a simple progress bar.\n- Keep the whole page to one screen."}
                    value={props.prefs.clone()} oninput={on_input} />
                <details class="preview">
                    <summary>{ "How it appears in agent tasks" }</summary>
                    <pre>{ preview }</pre>
                </details>
            </section>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Progress view: the note's read-only report
// ---------------------------------------------------------------------------

/// Bars in the dictation waveform.
const WAVE_BARS: usize = 9;

/// How often the progress view re-reads the report.
const PROGRESS_POLL_MS: u32 = 2_000;
/// How often the simulated agent updates the open note's report.
const SIMULATE_EVERY_MS: u32 = 3_000;

/// Dark defaults put in front of the report, so a bare agent-written page
/// still matches the view; the report's own styles win.
const REPORT_BASE: &str = "<link rel=\"stylesheet\" href=\"https://fonts.googleapis.com/css2?family=Inter:wght@400;500\
&family=Source+Serif+4:opsz,wght@8..60,400;8..60,600&display=swap\"><style>:root{color-scheme:dark}html,body{background:#141414;color:#d9d6d0;\
font:15px/1.6 Inter,system-ui,sans-serif}a{color:#9fb4d9}</style>";

#[derive(Clone, PartialEq)]
enum Report {
    Loading,
    Missing,
    Failed(String),
    Ready(String),
}

#[derive(Properties, PartialEq)]
struct ProgressViewProps {
    backend: String,
    /// Path of the report file, or `None` with no note open.
    path: Option<String>,
    title: String,
}

#[function_component(ProgressView)]
fn progress_view(props: &ProgressViewProps) -> Html {
    let report = use_state(|| Report::Loading);

    // Read the report now and every couple of seconds while it's shown.
    {
        let report = report.clone();
        use_effect_with((props.backend.clone(), props.path.clone()), move |(backend, path)| {
            report.set(Report::Loading);
            let poll = path.clone().map(|path| {
                let backend = backend.clone();
                let fetch = move || {
                    let (backend, path, report) = (backend.clone(), path.clone(), report.clone());
                    wasm_bindgen_futures::spawn_local(async move {
                        let next = match api::read_file(&backend, &path).await {
                            Ok(Some(html)) => Report::Ready(html),
                            Ok(None) => Report::Missing,
                            Err(e) => Report::Failed(e.message()),
                        };
                        // Only re-render (and reload the frame) when it changed.
                        if *report != next {
                            report.set(next);
                        }
                    });
                };
                fetch();
                gloo_timers::callback::Interval::new(PROGRESS_POLL_MS, fetch)
            });
            move || drop(poll)
        });
    }

    let body = match (&props.path, &*report) {
        (None, _) => html! { <div class="progress-empty"><p>{ "No note open." }</p></div> },
        (_, Report::Loading) => html! { <div class="progress-empty"><p>{ "Loading progress…" }</p></div> },
        (_, Report::Failed(e)) => html! {
            <div class="progress-empty"><p>{ "Couldn't load the progress report." }</p><p class="detail">{ e }</p></div>
        },
        (Some(_), Report::Missing) => html! {
            <div class="progress-empty"><p>{ "No Progress Yet" }</p></div>
        },
        (_, Report::Ready(html)) => html! {
            // No scripts, no same-origin access: the report is display-only.
            <iframe class="report" sandbox="" title={format!("Progress: {}", props.title)}
                srcdoc={format!("{REPORT_BASE}{html}")} />
        },
    };
    html! { <div class="progress-view">{ body }</div> }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

/// Writing the note, or reading its progress report.
#[derive(Clone, Copy, PartialEq)]
enum View {
    Write,
    Progress,
    Settings,
}

/// Turn a server listing into app state, importing old local notes into an
/// empty server once.
fn prepare(generation: u64, root: api::FsEntry) -> Action {
    let prefs = Prefs::load();
    let mut next_id = 1;
    let mut notes = Vec::new();
    notes_from_tree(root.children.as_deref().unwrap_or(&[]), "", &mut next_id, &mut notes);
    let mut imported = false;

    let already_migrated = LocalStorage::get::<bool>(MIGRATED_KEY).unwrap_or(false);
    if notes.is_empty() && !already_migrated {
        if let Ok(legacy) = LocalStorage::get::<LegacyState>(LEGACY_KEY) {
            let mut flat = Vec::new();
            flatten_legacy(legacy.items, &mut flat);
            let now = now_ms();
            for (i, (name, body)) in flat.into_iter().enumerate() {
                let name = unique_name(&notes, "", &clean_name(&name), None);
                // Keep the old order: earlier notes count as more recent.
                notes.push(Note { id: next_id, dir: String::new(), name, body, modified: now - i as i64 });
                next_id += 1;
            }
            imported = !notes.is_empty();
        }
    }
    if !notes.is_empty() || imported {
        // Old notes stay in localStorage as a backup; just don't import twice.
        let _ = LocalStorage::set(MIGRATED_KEY, true);
    }

    let selected = prefs
        .selected
        .as_deref()
        .and_then(|want| notes.iter().find(|n| n.path() == want))
        .map(|n| n.id)
        .or_else(|| ordered(&notes, false).first().map(|n| n.id));
    Action::Loaded { generation, notes, next_id, selected, imported, progress_prefs: String::new() }
}

#[function_component(App)]
fn app() -> Html {
    let state = use_reducer(State::new);
    let settings_open = use_state(|| false);
    // The notes list is hidden until asked for, to leave the screen to writing.
    let nav_open = use_state(|| false);
    let archive_open = use_state(|| false);
    let focus_editor = use_state(|| 0u64);
    let view = use_state(|| View::Write);
    let simulate = use_state(|| Prefs::load().simulate_agent.unwrap_or(true));
    let latest = use_mut_ref(|| None::<Rc<State>>);
    *latest.borrow_mut() = Some(Rc::new((*state).clone()));

    let dispatch = {
        let state = state.clone();
        Callback::from(move |a: Action| state.dispatch(a))
    };

    // Esc: close the notes list if it's open; otherwise switch between
    // writing and progress. (The settings panel handles its own Esc.)
    {
        let nav_open = nav_open.clone();
        let view = view.clone();
        let focus_editor = focus_editor.clone();
        let dispatch = dispatch.clone();
        let latest = latest.clone();
        let deps = (*nav_open, *settings_open, *view, *focus_editor);
        use_effect_with(deps, move |&(open, settings, current, focus)| {
            let window = web_sys::window().expect("window");
            let listener = gloo_events::EventListener::new(&window, "keydown", move |e| {
                let Some(k) = e.dyn_ref::<web_sys::KeyboardEvent>() else { return };
                if k.key() != "Escape" || settings || k.repeat() {
                    return;
                }
                e.prevent_default();
                if open {
                    nav_open.set(false);
                } else if current == View::Settings {
                    view.set(View::Write);
                    focus_editor.set(focus + 1);
                } else if current == View::Write {
                    dispatch.emit(Action::Flush);
                    let path = latest.borrow().as_ref().and_then(|s| s.selected_note().map(Note::path));
                    if let Some(path) = path {
                        let st = latest.borrow().clone();
                        if let Some(st) = st {
                            st.record("progress.viewed", json!({ "path": path }));
                        }
                    }
                    view.set(View::Progress);
                } else {
                    view.set(View::Write);
                    focus_editor.set(focus + 1);
                }
            });
            move || drop(listener)
        });
    }

    // The page behind the window follows the view's theme.
    use_effect_with(*view, |v| {
        if let Some(root) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.document_element()) {
            let _ = root.class_list().toggle_with_force("dark", *v == View::Progress);
        }
    });

    // Stand-in agent: keep the open note's progress report up to date.
    {
        let latest = latest.clone();
        let dispatch = dispatch.clone();
        let written = use_mut_ref(std::collections::HashMap::<u64, String>::new);
        // Re-run (and write at once) when the open note or connection changes.
        let deps = (*simulate, state.selected, state.load == Load::Ready, state.generation);
        use_effect_with(deps, move |&(on, _, _, _)| {
            let timer = on.then(|| {
                let tick = move || {
                    let Some(s) = latest.borrow().clone() else { return };
                    if s.load != Load::Ready {
                        return;
                    }
                    let Some(note) = s.selected_note() else { return };
                    let Some(html) = progress::simulated_report(&note.name, &note.body, now_ms()) else { return };
                    if written.borrow().get(&note.id) != Some(&html) {
                        written.borrow_mut().insert(note.id, html.clone());
                        dispatch.emit(Action::WriteProgress { id: note.id, html });
                    }
                };
                tick();
                gloo_timers::callback::Interval::new(SIMULATE_EVERY_MS, tick)
            });
            move || drop(timer)
        });
    }

    // Load the notes whenever we (re)connect.
    {
        let dispatch = dispatch.clone();
        let backend = state.backend.clone();
        use_effect_with(state.generation, move |generation| {
            let generation = *generation;
            wasm_bindgen_futures::spawn_local(async move {
                let loaded = match api::tree(&backend).await {
                    Ok(root) => prepare(generation, root),
                    Err(e) => Action::LoadFailed { generation, error: e.message() },
                };
                let loaded = match loaded {
                    Action::Loaded { generation, notes, next_id, selected, imported, .. } => {
                        // Missing (or unreadable) preferences just mean none yet.
                        let progress_prefs = api::read_file(&backend, agent_tasks::PROGRESS_PREFS_PATH)
                            .await
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        Action::Loaded { generation, notes, next_id, selected, imported, progress_prefs }
                    }
                    other => other,
                };
                dispatch.emit(loaded);
            });
        });
    }

    // Send queued operations.
    {
        let outbox = state.outbox.clone();
        let backend = state.backend.clone();
        let report = dispatch.reform(Action::Sync);
        use_effect(move || outbox.run(backend, report));
    }

    // Save shortly after typing stops.
    {
        let dispatch = dispatch.clone();
        use_effect_with(state.edits, move |_| {
            let timer = gloo_timers::callback::Timeout::new(SAVE_DELAY_MS, move || dispatch.emit(Action::Flush));
            move || drop(timer)
        });
    }

    // Remember the server and the open note.
    {
        let selected = (state.load == Load::Ready).then(|| state.selected_note().map(Note::path));
        let backend = state.backend.clone();
        use_effect_with((selected, backend), |(selected, backend)| {
            let prefs = Prefs {
                backend: Some(backend.clone()),
                selected: match selected {
                    Some(s) => s.clone(),
                    None => Prefs::load().selected,
                },
                simulate_agent: Prefs::load().simulate_agent,
                send_to_agents: Prefs::load().send_to_agents,
            };
            let _ = LocalStorage::set(PREFS_KEY, prefs);
        });
    }

    // Warn before closing with unsaved changes, and push them out.
    {
        let latest = latest.clone();
        let dispatch = dispatch.clone();
        use_effect_with((), move |_| {
            let window = web_sys::window().expect("window");
            let listener = gloo_events::EventListener::new(&window, "beforeunload", move |e| {
                let busy = latest
                    .borrow()
                    .as_ref()
                    .is_some_and(|s| !s.dirty.is_empty() || s.outbox.len() > 0 || s.session.is_some());
                if busy {
                    dispatch.emit(Action::Unload);
                    e.prevent_default();
                }
            });
            move || drop(listener)
        });
    }

    let open_settings = {
        let settings_open = settings_open.clone();
        Callback::from(move |_: MouseEvent| settings_open.set(true))
    };

    // A new note opens straight into writing; its first line becomes its title.
    let start_note = {
        let dispatch = dispatch.clone();
        let nav_open = nav_open.clone();
        let focus_editor = focus_editor.clone();
        Callback::from(move |_: MouseEvent| {
            dispatch.emit(Action::NewNote);
            nav_open.set(false);
            focus_editor.set(*focus_editor + 1);
        })
    };

    let progress_content = (*view == View::Progress && state.load == Load::Ready).then(|| {
        let note = state.selected_note();
        html! {
            <ProgressView backend={state.backend.clone()}
                path={note.map(|n| progress::progress_path(&n.path()))}
                title={note.map(|n| n.name.clone()).unwrap_or_default()} />
        }
    });

    let content = match (&state.load, state.selected_note().cloned()) {
        (Load::Loading, _) => html! {
            <div class="empty"><p>{ format!("Connecting to {}…", state.backend) }</p></div>
        },
        (Load::Failed(error), _) => html! {
            <div class="empty">
                <p>{ format!("Can't reach the notes server at {}.", state.backend) }</p>
                <p class="detail">{ error }</p>
                <button class="link" onclick={dispatch.reform({
                    let b = state.backend.clone();
                    move |_| Action::Connect(b.clone())
                })}>{ "Try again" }</button>
                <span class="sep">{ " · " }</span>
                <button class="link" onclick={open_settings.clone()}>{ "Change server" }</button>
            </div>
        },
        (Load::Ready, Some(Note { id, body, .. })) => html! {
            <Editor key={id} note_id={id} {body} dispatch={dispatch.clone()} focus={*focus_editor} />
        },
        (Load::Ready, None) => html! {
            <div class="empty">
                <button class="link" onclick={start_note.clone()}>
                    { "Start a new note" }
                </button>
            </div>
        },
    };

    let toggle_nav = {
        let nav_open = nav_open.clone();
        Callback::from(move |_: MouseEvent| nav_open.set(!*nav_open))
    };
    let close_nav = {
        let nav_open = nav_open.clone();
        Callback::from(move |_: MouseEvent| nav_open.set(false))
    };
    // Picking a note closes the list and goes back to writing.
    let list_dispatch = {
        let dispatch = dispatch.clone();
        let view = view.clone();
        let nav_open = nav_open.clone();
        let focus_editor = focus_editor.clone();
        Callback::from(move |a: Action| {
            let picked = matches!(a, Action::Select(_));
            dispatch.emit(a);
            if picked {
                nav_open.set(false);
                focus_editor.set(*focus_editor + 1);
                if *view == View::Settings {
                    view.set(View::Write);
                }
            }
        })
    };
    let toggle_archive = {
        let archive_open = archive_open.clone();
        Callback::from(move |_: MouseEvent| archive_open.set(!*archive_open))
    };

    let active = ordered(&state.notes, false);
    let archived = ordered(&state.notes, true);

    html! {
        <div class={classes!("window", nav_open.then_some("nav-open"), (*view == View::Progress).then_some("viewing-progress"))}>
            <button class={classes!("nav-toggle", nav_open.then_some("open"))}
                title={if *nav_open { "Hide notes" } else { "Show notes" }}
                aria-label="Notes" aria-expanded={nav_open.to_string()} onclick={toggle_nav}>
                <svg viewBox="0 0 16 16" aria-hidden="true">
                    <path d="M2.5 4.5h11M2.5 8h11M2.5 11.5h11" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
                </svg>
            </button>
            if *nav_open {
                <div class="nav-scrim" onclick={close_nav} />
            }
            <nav class="sidebar" aria-hidden={(!*nav_open).to_string()}>
                if state.load == Load::Ready {
                    <button class="new-note" onclick={start_note}>{ "+ New note" }</button>
                    <NoteList notes={active} selected={state.selected} dispatch={list_dispatch.clone()} />
                    if !archived.is_empty() {
                        <button class={classes!("archive-toggle", archive_open.then_some("open"))}
                            aria-expanded={archive_open.to_string()} onclick={toggle_archive}>
                            { format!("Archive · {}", archived.len()) }
                        </button>
                        if *archive_open {
                            <div class="archived">
                                <NoteList notes={archived} selected={state.selected} dispatch={list_dispatch} />
                            </div>
                        }
                    }
                    <button class={classes!("settings-link", (*view == View::Settings).then_some("on"))}
                        onclick={{
                            let view = view.clone();
                            let nav_open = nav_open.clone();
                            Callback::from(move |_: MouseEvent| {
                                nav_open.set(false);
                                view.set(View::Settings);
                            })
                        }}>{ "Settings" }</button>
                }
            </nav>
            <main class="page">
                if *view == View::Settings && state.load == Load::Ready {
                    <SettingsPage prefs={state.progress_prefs.clone()} saving={state.prefs_dirty || state.sync.pending > 0}
                        dispatch={dispatch.clone()}
                        on_back={{
                            let view = view.clone();
                            let focus_editor = focus_editor.clone();
                            Callback::from(move |_: MouseEvent| {
                                view.set(View::Write);
                                focus_editor.set(*focus_editor + 1);
                            })
                        }} />
                } else if let Some(p) = progress_content { { p } } else { { content } }
            </main>
            <div class="corner">
            <button class={classes!("view-toggle", (*view == View::Progress).then_some("on"))}
                title={if *view == View::Progress { "Back to writing (Esc)" } else { "Show progress (Esc)" }}
                aria-label="Progress" aria-pressed={(*view == View::Progress).to_string()}
                onclick={{
                    let view = view.clone();
                    let focus_editor = focus_editor.clone();
                    let dispatch = dispatch.clone();
                    Callback::from(move |_: MouseEvent| {
                        if *view == View::Write {
                            dispatch.emit(Action::Flush);
                            view.set(View::Progress);
                        } else {
                            view.set(View::Write);
                            focus_editor.set(*focus_editor + 1);
                        }
                    })
                }}>
                <svg viewBox="0 0 16 16" aria-hidden="true">
                    <circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-width="1.2"/>
                    <path d="M8 2.4a5.6 5.6 0 0 1 0 11.2z" fill="currentColor"/>
                </svg>
            </button>
            <Settings
                open={*settings_open}
                on_open={{ let s = settings_open.clone(); Callback::from(move |v| s.set(v)) }}
                backend={state.backend.clone()}
                load={state.load.clone()}
                sync={state.sync.clone()}
                unsaved={state.dirty.len()}
                notes={state.notes.len()}
                dispatch={dispatch.clone()}
                simulate={*simulate}
                send_to_agents={state.send_to_agents}
                on_simulate={{
                    let simulate = simulate.clone();
                    Callback::from(move |on: bool| {
                        simulate.set(on);
                        let _ = LocalStorage::set(PREFS_KEY, Prefs { simulate_agent: Some(on), ..Prefs::load() });
                    })
                }}
            />
            </div>
        </div>
    }
}

fn main() {
    // Mount into #app when the page provides one (e.g. a page that keeps its
    // own <style> in <body>); otherwise take over <body>.
    let root = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id("app"));
    match root {
        Some(el) => yew::Renderer::<App>::with_root(el).render(),
        None => yew::Renderer::<App>::new().render(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: u64, dir: &str, name: &str, modified: i64) -> Note {
        Note { id, dir: dir.into(), name: name.into(), body: String::new(), modified }
    }

    #[test]
    fn inline_formats_toggle() {
        let t = "a word here";
        let (s, e) = (2, 6);
        let bold = apply_format(t, s, e, Format::Bold);
        assert_eq!(bold, "a **word** here");
        // Selecting the wrapped text, or just the inner word, both unwrap.
        assert_eq!(apply_format(&bold, 2, 10, Format::Bold), t);
        assert_eq!(apply_format(&bold, 4, 8, Format::Bold), t);
        assert_eq!(apply_format(t, 1, 7, Format::Italic), "a *word* here");
        assert_eq!(apply_format(t, s, e, Format::Code), "a `word` here");
    }

    #[test]
    fn titles_come_from_the_first_line() {
        assert_eq!(title_from(""), "Untitled");
        assert_eq!(title_from("\n\n  # Groceries  \nmilk"), "Groceries");
        assert_eq!(title_from("## **Big** idea with `code`"), "Big idea with code");
        assert_eq!(title_from("- [ ] call [Sam](https://x.y) back"), "call Sam back");
        assert_eq!(title_from("> quoted thought"), "quoted thought");
        assert_eq!(title_from("12. step twelve"), "step twelve");
        assert_eq!(title_from("a/b: c"), "a-b: c");
        assert_eq!(title_from("```rust\nfn main() {}"), "Untitled");
        let long = "This first line keeps going well past the point where a title should stop being a title";
        let t = title_from(long);
        assert!(t.chars().count() <= MAX_TITLE_CHARS && long.starts_with(&t) && !t.ends_with(' '), "{t}");
    }

    #[test]
    fn paths_and_archive() {
        assert_eq!(note(1, "", "Idea", 0).path(), "Idea.md");
        assert_eq!(note(1, "Old/Folder", "Idea", 0).path(), "Old/Folder/Idea.md");
        assert!(note(1, "Archive", "Idea", 0).archived());
        assert!(note(1, "Archive/2025", "Idea", 0).archived());
        assert!(!note(1, "Archives", "Idea", 0).archived());
    }

    #[test]
    fn names_are_cleaned_and_unique() {
        assert_eq!(clean_name("  a/b\\c "), "a-b-c");
        assert_eq!(clean_name(" ..hidden"), "hidden");
        assert_eq!(clean_name("note.md"), "note");
        assert_eq!(clean_name("   "), "Untitled");
        let notes = vec![note(1, "", "Idea", 0), note(2, "Archive", "Plan", 0)];
        assert_eq!(unique_name(&notes, "", "idea", None), "idea 2");
        assert_eq!(unique_name(&notes, "", "Idea", Some(1)), "Idea");
        assert_eq!(unique_name(&notes, "", "Plan", None), "Plan");
        assert_eq!(unique_name(&notes, "Archive", "Plan", None), "Plan 2");
    }

    #[test]
    fn list_is_newest_first_and_split_by_archive() {
        let notes = vec![note(1, "", "Old", 1), note(2, "x", "New", 3), note(3, "Archive", "Gone", 2)];
        let active: Vec<u64> = ordered(&notes, false).iter().map(|n| n.id).collect();
        assert_eq!(active, vec![2, 1]);
        assert_eq!(ordered(&notes, true)[0].id, 3);
    }

    #[test]
    fn tree_conversion_finds_notes_everywhere() {
        let entry = |name: &str, kind: &str, content: Option<&str>, children: Option<Vec<api::FsEntry>>| api::FsEntry {
            name: name.into(),
            kind: kind.into(),
            modified_ms: 5,
            content: content.map(Into::into),
            children,
        };
        let tree = vec![
            entry(
                "Book",
                "dir",
                None,
                Some(vec![entry("a.md", "file", Some("hello"), None), entry("image.png", "file", None, None)]),
            ),
            entry("b.md", "file", Some(""), None),
        ];
        let (mut next, mut out) = (1, Vec::new());
        notes_from_tree(&tree, "", &mut next, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].path(), "Book/a.md");
        assert_eq!(out[1].path(), "b.md");
        assert_eq!(out[0].modified, 5);
    }
}
