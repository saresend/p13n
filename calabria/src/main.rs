use std::collections::BTreeSet;
use std::rc::Rc;

use gloo_storage::{LocalStorage, Storage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use web_sys::{HtmlInputElement, HtmlTextAreaElement};
use yew::prelude::*;

mod api;
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
/// Save a note this long after typing stops.
const SAVE_DELAY_MS: u32 = 600;
const WORDS_PER_MINUTE: usize = 250;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Serialize, Deserialize)]
enum Kind {
    Folder { open: bool, children: Vec<Item> },
    Note { body: String },
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Item {
    id: u64,
    name: String,
    kind: Kind,
}

impl Item {
    fn folder(id: u64, name: &str, children: Vec<Item>) -> Self {
        Self { id, name: name.into(), kind: Kind::Folder { open: true, children } }
    }
    fn note(id: u64, name: &str, body: &str) -> Self {
        Self { id, name: name.into(), kind: Kind::Note { body: body.into() } }
    }
    fn is_note(&self) -> bool {
        matches!(self.kind, Kind::Note { .. })
    }
    /// Name on disk: notes are `<name>.md`, folders are directories.
    fn file_name(&self) -> String {
        if self.is_note() { format!("{}.md", self.name) } else { self.name.clone() }
    }
}

/// The pre-dsys localStorage format.
#[derive(Deserialize)]
struct LegacyState {
    items: Vec<Item>,
}

/// Per-browser settings and view state.
#[derive(Serialize, Deserialize, Default, PartialEq)]
struct Prefs {
    backend: Option<String>,
    /// Paths of collapsed folders.
    #[serde(default)]
    closed: Vec<String>,
    selected: Option<String>,
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
    items: Vec<Item>,
    selected: Option<u64>,
    next_id: u64,
    renaming: Option<u64>,
    /// Server path of the item being renamed, as it was before the rename.
    rename_from: Option<String>,
    /// Notes whose body has changed since it was last queued for saving.
    dirty: BTreeSet<u64>,
    /// Bumped on every edit; drives the save debounce.
    edits: u64,
    /// The note being edited and its length when editing began.
    session: Option<(u64, usize)>,
    sync: SyncStatus,
    outbox: Outbox,
}

fn find(items: &[Item], id: u64) -> Option<&Item> {
    items.iter().find_map(|it| {
        if it.id == id {
            return Some(it);
        }
        match &it.kind {
            Kind::Folder { children, .. } => find(children, id),
            _ => None,
        }
    })
}

fn find_mut(items: &mut [Item], id: u64) -> Option<&mut Item> {
    for it in items.iter_mut() {
        if it.id == id {
            return Some(it);
        }
        if let Kind::Folder { children, .. } = &mut it.kind {
            if let Some(found) = find_mut(children, id) {
                return Some(found);
            }
        }
    }
    None
}

/// Ids from the root down to (and including) `id`.
fn path_to(items: &[Item], id: u64) -> Vec<u64> {
    for it in items {
        if it.id == id {
            return vec![it.id];
        }
        if let Kind::Folder { children, .. } = &it.kind {
            let mut p = path_to(children, id);
            if !p.is_empty() {
                p.insert(0, it.id);
                return p;
            }
        }
    }
    Vec::new()
}

/// The item's path on the server, e.g. `Notebook/Folder/Note.md`.
fn server_path(items: &[Item], id: u64) -> Option<String> {
    let ids = path_to(items, id);
    if ids.is_empty() {
        return None;
    }
    let names: Vec<String> = ids.iter().filter_map(|i| find(items, *i)).map(Item::file_name).collect();
    Some(names.join("/"))
}

fn parent_of(items: &[Item], id: u64) -> Option<u64> {
    let p = path_to(items, id);
    p.len().checked_sub(2).map(|i| p[i])
}

fn children_of(items: &[Item], parent: Option<u64>) -> &[Item] {
    match parent.and_then(|p| find(items, p)) {
        Some(Item { kind: Kind::Folder { children, .. }, .. }) => children,
        _ => items,
    }
}

fn remove(items: &mut Vec<Item>, id: u64) -> bool {
    if let Some(i) = items.iter().position(|it| it.id == id) {
        items.remove(i);
        return true;
    }
    items.iter_mut().any(|it| match &mut it.kind {
        Kind::Folder { children, .. } => remove(children, id),
        _ => false,
    })
}

fn first_note(items: &[Item]) -> Option<u64> {
    items.iter().find_map(|it| match &it.kind {
        Kind::Note { .. } => Some(it.id),
        Kind::Folder { children, .. } => first_note(children),
    })
}

fn ids_in(item: &Item, out: &mut Vec<u64>) {
    out.push(item.id);
    if let Kind::Folder { children, .. } = &item.kind {
        for c in children {
            ids_in(c, out);
        }
    }
}

fn count_notes(items: &[Item]) -> usize {
    items
        .iter()
        .map(|it| match &it.kind {
            Kind::Note { .. } => 1,
            Kind::Folder { children, .. } => count_notes(children),
        })
        .sum()
}

/// Push `item` into the folder `parent` (or the root when `None`).
fn insert_into(items: &mut Vec<Item>, parent: Option<u64>, item: Item) {
    let target = parent.and_then(|p| find_mut(items, p));
    match target {
        Some(Item { kind: Kind::Folder { open, children }, .. }) => {
            *open = true;
            children.push(item);
        }
        _ => items.push(item),
    }
}

/// Make a user-typed name safe to use as a file or directory name.
fn clean_name(raw: &str) -> String {
    let s: String = raw.chars().map(|c| if matches!(c, '/' | '\\' | '\0') { '-' } else { c }).collect();
    let s = s.trim().trim_start_matches('.').trim();
    if s.is_empty() { "Untitled".into() } else { s.into() }
}

/// `base`, or `base 2`, `base 3`, ... so no sibling has the same file name.
fn unique_name(siblings: &[Item], base: &str, note: bool, except: Option<u64>) -> String {
    let file = |n: &str| if note { format!("{n}.md") } else { n.to_string() };
    let taken = |n: &str| {
        let f = file(n);
        siblings.iter().any(|s| Some(s.id) != except && s.file_name().eq_ignore_ascii_case(&f))
    };
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|c| !taken(c)).expect("some suffix is free")
}

/// Clean and de-duplicate names imported from the old local format.
fn normalize_names(items: &mut [Item]) {
    for i in 0..items.len() {
        let base = clean_name(&items[i].name);
        let name = unique_name(&items[..i], &base, items[i].is_note(), None);
        items[i].name = name;
        if let Kind::Folder { children, .. } = &mut items[i].kind {
            normalize_names(children);
        }
    }
}

fn max_id(items: &[Item]) -> u64 {
    items
        .iter()
        .map(|it| match &it.kind {
            Kind::Folder { children, .. } => it.id.max(max_id(children)),
            _ => it.id,
        })
        .max()
        .unwrap_or(0)
}

/// Build the sidebar tree from the server listing: directories become
/// folders, `*.md` files become notes, everything else is ignored.
fn items_from_tree(entries: &[api::FsEntry], closed: &BTreeSet<&str>, next_id: &mut u64) -> Vec<Item> {
    let mut out = Vec::new();
    for e in entries {
        if e.kind == "dir" {
            let children = items_from_tree(e.children.as_deref().unwrap_or(&[]), closed, next_id);
            let mut f = Item::folder(*next_id, &e.name, children);
            *next_id += 1;
            if let Kind::Folder { open, .. } = &mut f.kind {
                *open = !closed.contains(e.path.as_str());
            }
            out.push(f);
        } else if let (Some(stem), Some(body)) = (e.name.strip_suffix(".md"), &e.content) {
            out.push(Item::note(*next_id, stem, body));
            *next_id += 1;
        }
    }
    out
}

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

fn normalize_url(raw: &str) -> String {
    let s = raw.trim().trim_end_matches('/');
    if s.contains("://") { s.to_string() } else { format!("http://{s}") }
}

impl State {
    fn new() -> Self {
        let prefs = Prefs::load();
        Self {
            backend: prefs.backend.unwrap_or_else(|| DEFAULT_BACKEND.into()),
            generation: 0,
            load: Load::Loading,
            items: Vec::new(),
            selected: None,
            next_id: 1,
            renaming: None,
            rename_from: None,
            dirty: BTreeSet::new(),
            edits: 0,
            session: None,
            sync: SyncStatus::default(),
            outbox: Outbox::default(),
        }
    }

    fn selected_note(&self) -> Option<&Item> {
        self.selected.and_then(|id| find(&self.items, id)).filter(|it| it.is_note())
    }

    /// The folder new items should go into: the selected note's parent folder.
    fn current_folder(&self) -> Option<u64> {
        self.selected.and_then(|id| parent_of(&self.items, id))
    }

    fn alloc(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id - 1
    }

    fn path(&self, id: u64) -> String {
        server_path(&self.items, id).unwrap_or_default()
    }

    fn body(&self, id: u64) -> Option<&str> {
        match find(&self.items, id) {
            Some(Item { kind: Kind::Note { body }, .. }) => Some(body),
            _ => None,
        }
    }

    fn record(&self, action_name: &'static str, payload: Value) {
        self.outbox.push(Op::Record { action_name, payload, timestamp: now_ms() });
    }

    /// Queue saves for every note edited since the last flush, at their
    /// current paths. Called before anything that changes paths.
    fn flush(&mut self) {
        for id in std::mem::take(&mut self.dirty) {
            if let Some(body) = self.body(id) {
                self.outbox.push(Op::Write { path: self.path(id), body: body.to_string(), create_only: false });
            }
        }
    }

    /// Record the end of an editing session on a note.
    fn end_session(&mut self) {
        if let Some((id, chars_before)) = self.session.take() {
            if let Some(body) = self.body(id) {
                self.record(
                    "note.edited",
                    json!({
                        "path": self.path(id),
                        "chars_before": chars_before,
                        "chars_after": body.chars().count(),
                        "words": word_count(body),
                    }),
                );
            }
        }
    }

    /// Queue creation of every folder and note (used when importing).
    fn seed(&self, items: &[Item]) {
        for it in items {
            let path = self.path(it.id);
            match &it.kind {
                Kind::Folder { children, .. } => {
                    self.outbox.push(Op::CreateDir { path });
                    self.seed(children);
                }
                Kind::Note { body } => self.outbox.push(Op::Write { path, body: body.clone(), create_only: true }),
            }
        }
    }
}

enum Action {
    /// Switch to (or reconnect to) a server.
    Connect(String),
    Loaded { generation: u64, items: Vec<Item>, next_id: u64, selected: Option<u64>, imported: bool },
    LoadFailed { generation: u64, error: String },
    Sync(SyncStatus),
    Select(u64),
    Toggle(u64),
    StartRename(u64),
    Rename(u64, String),
    EndRename,
    SetBody(String),
    /// Queue saves for edited notes (fired by the debounce timer).
    Flush,
    /// The page is closing.
    Unload,
    NewNote(Option<u64>),
    NewFolder(Option<u64>),
    Delete(u64),
    /// Move text out of the current note into a new sibling note.
    Breakout { remaining: String, extracted: String },
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
            Action::Loaded { generation, items, next_id, selected, imported } => {
                if generation != s.generation {
                    return self;
                }
                s.items = items;
                s.next_id = next_id;
                s.selected = selected;
                s.load = Load::Ready;
                if imported {
                    s.seed(&s.items);
                    s.record("notes.imported", json!({ "notes": count_notes(&s.items) }));
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
            Action::Toggle(id) => {
                if let Some(Item { kind: Kind::Folder { open, .. }, .. }) = find_mut(&mut s.items, id) {
                    *open = !*open;
                }
            }
            Action::StartRename(id) => {
                s.flush();
                s.rename_from = server_path(&s.items, id);
                s.renaming = Some(id);
            }
            Action::Rename(id, name) => {
                if let Some(it) = find_mut(&mut s.items, id) {
                    it.name = name;
                }
            }
            Action::EndRename => {
                let Some(id) = s.renaming.take() else { return self };
                let from = s.rename_from.take();
                let Some(item) = find(&s.items, id) else { return s.into() };
                let note = item.is_note();
                let siblings = children_of(&s.items, parent_of(&s.items, id));
                let name = unique_name(siblings, &clean_name(&item.name), note, Some(id));
                if let Some(it) = find_mut(&mut s.items, id) {
                    it.name = name;
                }
                let to = s.path(id);
                if let Some(from) = from.filter(|f| *f != to) {
                    s.outbox.push(Op::Move { from: from.clone(), to: to.clone() });
                    let action_name = if note { "note.renamed" } else { "folder.renamed" };
                    s.record(action_name, json!({ "from": from, "to": to }));
                }
                s.flush();
            }
            Action::SetBody(text) => {
                let Some(id) = s.selected_note().map(|n| n.id) else { return self };
                if s.session.map(|(sid, _)| sid) != Some(id) {
                    s.end_session();
                    s.session = Some((id, s.body(id).map_or(0, |b| b.chars().count())));
                }
                if let Some(Item { kind: Kind::Note { body }, .. }) = find_mut(&mut s.items, id) {
                    *body = text;
                }
                s.dirty.insert(id);
                s.edits += 1;
            }
            Action::Flush => {
                // Paths are in flux while a name is being typed; EndRename flushes.
                if s.dirty.is_empty() || s.renaming.is_some() {
                    return self;
                }
                s.flush();
            }
            Action::Unload => {
                s.flush();
                s.end_session();
            }
            Action::NewNote(parent) => {
                s.flush();
                s.end_session();
                let name = unique_name(children_of(&s.items, parent), "Untitled", true, None);
                let id = s.alloc();
                insert_into(&mut s.items, parent, Item::note(id, &name, ""));
                let path = s.path(id);
                s.outbox.push(Op::Write { path: path.clone(), body: String::new(), create_only: true });
                s.record("note.created", json!({ "path": path }));
                s.selected = Some(id);
                s.renaming = Some(id);
                s.rename_from = Some(path);
            }
            Action::NewFolder(parent) => {
                s.flush();
                let name = unique_name(children_of(&s.items, parent), "New Folder", false, None);
                let id = s.alloc();
                insert_into(&mut s.items, parent, Item::folder(id, &name, vec![]));
                let path = s.path(id);
                s.outbox.push(Op::CreateDir { path: path.clone() });
                s.record("folder.created", json!({ "path": path }));
                s.renaming = Some(id);
                s.rename_from = Some(path);
            }
            Action::Delete(id) => {
                let Some(item) = find(&s.items, id).cloned() else { return self };
                let mut gone = Vec::new();
                ids_in(&item, &mut gone);
                s.dirty.retain(|d| !gone.contains(d));
                if s.session.is_some_and(|(sid, _)| gone.contains(&sid)) {
                    s.session = None;
                }
                s.flush();
                let path = s.path(id);
                if item.is_note() {
                    s.outbox.push(Op::DeleteFile { path: path.clone() });
                    s.record("note.deleted", json!({ "path": path }));
                } else {
                    s.outbox.push(Op::DeleteDir { path: path.clone() });
                    s.record("folder.deleted", json!({ "path": path, "notes": count_notes(&[item]) }));
                }
                remove(&mut s.items, id);
                if s.selected.is_some_and(|sel| find(&s.items, sel).is_none()) {
                    s.selected = first_note(&s.items);
                }
            }
            Action::Breakout { remaining, extracted } => {
                let Some(from_id) = s.selected_note().map(|n| n.id) else { return self };
                let parent = s.current_folder();
                if let Some(Item { kind: Kind::Note { body }, .. }) = find_mut(&mut s.items, from_id) {
                    *body = remaining;
                }
                s.dirty.insert(from_id);
                s.flush();
                s.end_session();
                let words: String = extracted.split_whitespace().take(4).collect::<Vec<_>>().join(" ");
                let name = unique_name(children_of(&s.items, parent), &clean_name(&words), true, None);
                let id = s.alloc();
                let body = extracted.trim().to_string();
                insert_into(&mut s.items, parent, Item::note(id, &name, &body));
                let (from, to) = (s.path(from_id), s.path(id));
                s.outbox.push(Op::Write { path: to.clone(), body: body.clone(), create_only: true });
                s.record("note.breakout", json!({ "from": from, "to": to, "chars": body.chars().count() }));
                s.selected = Some(id);
            }
        }
        s.into()
    }
}

// ---------------------------------------------------------------------------
// Sidebar tree
// ---------------------------------------------------------------------------

#[derive(Properties, PartialEq)]
struct TreeProps {
    items: Vec<Item>,
    depth: usize,
    active_path: Rc<Vec<u64>>,
    renaming: Option<u64>,
    dispatch: Callback<Action>,
}

#[function_component(Tree)]
fn tree(props: &TreeProps) -> Html {
    html! {
        <ul class={classes!("tree", format!("depth-{}", props.depth.min(2)))}>
            { for props.items.iter().map(|it| html! {
                <TreeRow key={it.id} item={it.clone()} depth={props.depth}
                    active_path={props.active_path.clone()}
                    renaming={props.renaming} dispatch={props.dispatch.clone()} />
            }) }
        </ul>
    }
}

#[derive(Properties, PartialEq)]
struct RowProps {
    item: Item,
    depth: usize,
    active_path: Rc<Vec<u64>>,
    renaming: Option<u64>,
    dispatch: Callback<Action>,
}

#[function_component(TreeRow)]
fn tree_row(props: &RowProps) -> Html {
    let RowProps { item, depth, active_path, renaming, dispatch } = props;
    let id = item.id;
    let active = active_path.contains(&id);

    let icon = match &item.kind {
        Kind::Note { .. } => html! {},
        Kind::Folder { .. } if *depth == 0 => html! {
            <svg class="glyph" viewBox="0 0 12 12"><path d="M1 6l3.5-3v6zM11 6L7.5 3v6z" fill="currentColor"/></svg>
        },
        Kind::Folder { open: true, .. } => html! {
            <svg class="glyph" viewBox="0 0 12 12"><path d="M6 2.5L10 9.5H2z" fill="none" stroke="currentColor" stroke-width="1.2"/></svg>
        },
        Kind::Folder { open: false, .. } => html! {
            <svg class="glyph" viewBox="0 0 12 12"><path d="M2 3.5h8L6 10z" fill="currentColor"/></svg>
        },
    };

    let label = if *renaming == Some(id) {
        let on_input = dispatch.reform(move |e: InputEvent| {
            Action::Rename(id, e.target_unchecked_into::<HtmlInputElement>().value())
        });
        let on_key = {
            let dispatch = dispatch.clone();
            Callback::from(move |e: KeyboardEvent| {
                if e.key() == "Enter" || e.key() == "Escape" {
                    dispatch.emit(Action::EndRename);
                }
            })
        };
        html! {
            <input class="rename" value={item.name.clone()} autofocus=true
                oninput={on_input} onkeydown={on_key}
                onblur={dispatch.reform(|_| Action::EndRename)} />
        }
    } else {
        html! { <span class="name">{ &item.name }</span> }
    };

    let onclick = match &item.kind {
        Kind::Note { .. } => dispatch.reform(move |_: MouseEvent| Action::Select(id)),
        Kind::Folder { .. } => dispatch.reform(move |_: MouseEvent| Action::Toggle(id)),
    };
    let ondblclick = dispatch.reform(move |_: MouseEvent| Action::StartRename(id));

    // Hover actions.
    let stop = |a: Action| {
        let dispatch = dispatch.clone();
        let a = Rc::new(std::cell::Cell::new(Some(a)));
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            if let Some(a) = a.take() {
                dispatch.emit(a);
            }
        })
    };
    let actions = match &item.kind {
        Kind::Folder { .. } => html! {
            <span class="actions">
                <button title="New note" onclick={stop(Action::NewNote(Some(id)))}>{ "+" }</button>
                <button title="New folder" onclick={stop(Action::NewFolder(Some(id)))}>{ "▸" }</button>
                <button title="Delete folder" onclick={stop(Action::Delete(id))}>{ "×" }</button>
            </span>
        },
        Kind::Note { .. } => html! {
            <span class="actions">
                <button title="Delete note" onclick={stop(Action::Delete(id))}>{ "×" }</button>
            </span>
        },
    };

    let children = match &item.kind {
        Kind::Folder { open: true, children } if !children.is_empty() => html! {
            <Tree items={children.clone()} depth={depth + 1} active_path={active_path.clone()}
                renaming={*renaming} dispatch={dispatch.clone()} />
        },
        _ => html! {},
    };

    let kind = if item.is_note() { "note" } else { "folder" };
    html! {
        <li>
            <div class={classes!("row", kind, active.then_some("active"))} {onclick} {ondblclick}>
                { icon }{ label }{ actions }
            </div>
            { children }
        </li>
    }
}

// ---------------------------------------------------------------------------
// Editor
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Format {
    Heading,
    Quote,
    List,
    Link,
}

/// Apply a markdown-style format to `text` over the byte range `start..end`.
fn apply_format(text: &str, start: usize, end: usize, fmt: Format) -> String {
    match fmt {
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
}

#[function_component(Editor)]
fn editor(props: &EditorProps) -> Html {
    let textarea = use_node_ref();
    let toolbar = use_state(|| None::<(i32, i32)>);

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
                { button("Heading", fmt(Format::Heading)) }
                { button("Quote", fmt(Format::Quote)) }
                { button("List", fmt(Format::List)) }
                { button("Link", fmt(Format::Link)) }
                { button("Breakout Paragraph", breakout) }
            </div>
        }
    });

    html! {
        <>
            <textarea
                ref={textarea}
                key={props.note_id}
                class="body"
                placeholder="Begin writing…"
                value={props.body.clone()}
                oninput={on_input}
                onmouseup={on_mouseup}
                onkeyup={on_keyup}
                onblur={{ let t = toolbar.clone(); Callback::from(move |_| t.set(None)) }}
            />
            { for popover }
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
}

#[function_component(Settings)]
fn settings(props: &SettingsProps) -> Html {
    let draft = use_state(|| props.backend.clone());
    let probe = use_state(|| Probe::Idle);

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
    let use_local = {
        let draft = draft.clone();
        let probe = probe.clone();
        Callback::from(move |_: MouseEvent| {
            draft.set(DEFAULT_BACKEND.to_string());
            probe.set(Probe::Idle);
        })
    };

    let current = match &props.load {
        Load::Ready => format!(
            "Connected · {} note{}",
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
                <div class="buttons">
                    <button class="quiet" onclick={use_local}>{ "Use local" }</button>
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
// App
// ---------------------------------------------------------------------------

/// Turn a server listing into app state, importing old local notes into an
/// empty server once.
fn prepare(generation: u64, root: api::FsEntry) -> Action {
    let prefs = Prefs::load();
    let closed: BTreeSet<&str> = prefs.closed.iter().map(String::as_str).collect();
    let mut next_id = 1;
    let mut items = items_from_tree(root.children.as_deref().unwrap_or(&[]), &closed, &mut next_id);
    let mut imported = false;

    let already_migrated = LocalStorage::get::<bool>(MIGRATED_KEY).unwrap_or(false);
    if items.is_empty() && !already_migrated {
        if let Ok(mut legacy) = LocalStorage::get::<LegacyState>(LEGACY_KEY) {
            if !legacy.items.is_empty() {
                normalize_names(&mut legacy.items);
                next_id = max_id(&legacy.items) + 1;
                items = legacy.items;
                imported = true;
            }
        }
    }
    if !items.is_empty() || imported {
        // Old notes stay in localStorage as a backup; just don't import twice.
        let _ = LocalStorage::set(MIGRATED_KEY, true);
    }

    let selected = prefs
        .selected
        .as_deref()
        .and_then(|want| {
            let mut ids = Vec::new();
            items.iter().for_each(|it| ids_in(it, &mut ids));
            ids.into_iter().find(|id| server_path(&items, *id).as_deref() == Some(want))
        })
        .filter(|id| find(&items, *id).is_some_and(Item::is_note))
        .or_else(|| first_note(&items));
    Action::Loaded { generation, items, next_id, selected, imported }
}

fn closed_folders(items: &[Item], prefix: &str, out: &mut Vec<String>) {
    for it in items {
        if let Kind::Folder { open, children } = &it.kind {
            let path = if prefix.is_empty() { it.name.clone() } else { format!("{prefix}/{}", it.name) };
            if !open {
                out.push(path.clone());
            }
            closed_folders(children, &path, out);
        }
    }
}

#[function_component(App)]
fn app() -> Html {
    let state = use_reducer(State::new);
    let settings_open = use_state(|| false);
    let latest = use_mut_ref(|| None::<Rc<State>>);
    *latest.borrow_mut() = Some(Rc::new((*state).clone()));

    let dispatch = {
        let state = state.clone();
        Callback::from(move |a: Action| state.dispatch(a))
    };

    // Load the tree whenever we (re)connect.
    {
        let dispatch = dispatch.clone();
        let backend = state.backend.clone();
        use_effect_with(state.generation, move |generation| {
            let generation = *generation;
            wasm_bindgen_futures::spawn_local(async move {
                dispatch.emit(match api::tree(&backend).await {
                    Ok(root) => prepare(generation, root),
                    Err(e) => Action::LoadFailed { generation, error: e.message() },
                });
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

    // Remember the server, collapsed folders and the open note.
    {
        let prefs = (state.load == Load::Ready).then(|| {
            let mut closed = Vec::new();
            closed_folders(&state.items, "", &mut closed);
            Prefs {
                backend: Some(state.backend.clone()),
                closed,
                selected: state.selected.and_then(|id| server_path(&state.items, id)),
            }
        });
        let backend = state.backend.clone();
        use_effect_with((prefs, backend), |(prefs, backend)| {
            let prefs = match prefs {
                Some(p) => Prefs { backend: p.backend.clone(), closed: p.closed.clone(), selected: p.selected.clone() },
                None => Prefs { backend: Some(backend.clone()), ..Prefs::load() },
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

    let active_path = Rc::new(state.selected.map(|id| path_to(&state.items, id)).unwrap_or_default());
    let note = state.selected_note().cloned();

    let words = note
        .as_ref()
        .map(|n| match &n.kind {
            Kind::Note { body } => word_count(body),
            _ => 0,
        })
        .unwrap_or(0);
    let minutes = words.div_ceil(WORDS_PER_MINUTE).max(1);

    let open_settings = {
        let settings_open = settings_open.clone();
        Callback::from(move |_: MouseEvent| settings_open.set(true))
    };

    let content = match (&state.load, note) {
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
        (Load::Ready, Some(Item { id, kind: Kind::Note { body }, .. })) => html! {
            <Editor key={id} note_id={id} {body} dispatch={dispatch.clone()} />
        },
        (Load::Ready, _) => html! {
            <div class="empty">
                <p>{ "Nothing selected." }</p>
                <button class="link" onclick={dispatch.reform(|_| Action::NewNote(None))}>
                    { "Start a new note" }
                </button>
            </div>
        },
    };

    html! {
        <div class="window">
            <nav class="sidebar">
                <Tree items={state.items.clone()} depth={0} {active_path}
                    renaming={state.renaming} dispatch={dispatch.clone()} />
                if state.load == Load::Ready {
                    <button class="new-notebook" onclick={dispatch.reform(|_| Action::NewFolder(None))}>
                        { "+ New notebook" }
                    </button>
                }
                <svg class="mark" viewBox="0 0 14 26" aria-label="Calabria">
                    <path d="M1 1h12v14L7 25 1 15z" fill="#2b2b2b"/>
                    <path d="M7 4v13" stroke="#fff" stroke-width="1.4"/>
                </svg>
            </nav>
            <main class="page">
                { content }
            </main>
            <Settings
                open={*settings_open}
                on_open={{ let s = settings_open.clone(); Callback::from(move |v| s.set(v)) }}
                backend={state.backend.clone()}
                load={state.load.clone()}
                sync={state.sync.clone()}
                unsaved={state.dirty.len()}
                notes={count_notes(&state.items)}
                dispatch={dispatch.clone()}
            />
            if words > 0 {
                <footer class="count">{ format!("{words} words ({minutes} min)") }</footer>
            }
        </div>
    }
}

fn main() {
    yew::Renderer::<App>::new().render();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Item> {
        vec![Item::folder(1, "Book", vec![Item::note(2, "Ch 1", "x"), Item::folder(3, "Drafts", vec![])])]
    }

    #[test]
    fn server_paths() {
        let items = sample();
        assert_eq!(server_path(&items, 1).as_deref(), Some("Book"));
        assert_eq!(server_path(&items, 2).as_deref(), Some("Book/Ch 1.md"));
        assert_eq!(server_path(&items, 3).as_deref(), Some("Book/Drafts"));
        assert_eq!(server_path(&items, 9), None);
    }

    #[test]
    fn names_are_cleaned_and_unique() {
        assert_eq!(clean_name("  a/b\\c "), "a-b-c");
        assert_eq!(clean_name(" ..hidden"), "hidden");
        assert_eq!(clean_name("   "), "Untitled");
        let items = sample();
        let book = children_of(&items, Some(1));
        assert_eq!(unique_name(book, "Ch 1", true, None), "Ch 1 2");
        assert_eq!(unique_name(book, "ch 1", true, Some(2)), "ch 1");
        // A folder and a note may share a name: Drafts/ vs Drafts.md.
        assert_eq!(unique_name(book, "Drafts", true, None), "Drafts");
        assert_eq!(unique_name(book, "Drafts", false, None), "Drafts 2");
    }

    #[test]
    fn tree_conversion_keeps_md_files_only() {
        let entry = |name: &str, path: &str, kind: &str, content: Option<&str>, children: Option<Vec<api::FsEntry>>| {
            api::FsEntry {
                name: name.into(),
                path: path.into(),
                kind: kind.into(),
                content: content.map(Into::into),
                children,
            }
        };
        let tree = vec![entry(
            "Book",
            "Book",
            "dir",
            None,
            Some(vec![
                entry("a.md", "Book/a.md", "file", Some("hello"), None),
                entry("image.png", "Book/image.png", "file", None, None),
            ]),
        )];
        let closed = BTreeSet::from(["Book"]);
        let mut next = 1;
        let items = items_from_tree(&tree, &closed, &mut next);
        assert_eq!(next, 3);
        let Kind::Folder { open, children } = &items[0].kind else { panic!() };
        assert!(!open);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "a");
    }
}
