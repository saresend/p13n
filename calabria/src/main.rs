use std::rc::Rc;

use gloo_storage::{LocalStorage, Storage};
use serde::{Deserialize, Serialize};
use web_sys::{HtmlInputElement, HtmlTextAreaElement};
use yew::prelude::*;

const STORAGE_KEY: &str = "calabria.tree.v1";
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
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct State {
    items: Vec<Item>,
    selected: Option<u64>,
    next_id: u64,
    #[serde(skip)]
    renaming: Option<u64>,
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

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

impl State {
    fn sample() -> Self {
        let body = "Software needs to be more forgiving, not in the sense of end user software, \
but rather, development software. Compilers, interpreters, and so on.\n\n\
Select some text to format it, or break a passage out into its own note.";
        let items = vec![Item::folder(
            1,
            "Software Musings",
            vec![
                Item::folder(
                    2,
                    "Development Tools",
                    vec![
                        Item::note(3, "Commercial Tools", body),
                        Item::note(4, "Free and Open Source", ""),
                        Item::note(5, "Hack Your Own", ""),
                    ],
                ),
                Item { id: 6, name: "Agile Methodology".into(), kind: Kind::Folder { open: false, children: vec![] } },
            ],
        )];
        Self { items, selected: Some(3), next_id: 7, renaming: None }
    }

    fn load() -> Self {
        LocalStorage::get(STORAGE_KEY).unwrap_or_else(|_| Self::sample())
    }

    fn selected_note(&self) -> Option<&Item> {
        self.selected.and_then(|id| find(&self.items, id)).filter(|it| it.is_note())
    }

    /// The folder new items should go into: the selected note's parent folder.
    fn current_folder(&self) -> Option<u64> {
        let path = self.selected.map(|id| path_to(&self.items, id)).unwrap_or_default();
        path.len().checked_sub(2).map(|i| path[i])
    }

    fn alloc(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id - 1
    }
}

enum Action {
    Select(u64),
    Toggle(u64),
    StartRename(u64),
    Rename(u64, String),
    EndRename,
    SetBody(String),
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
            Action::Select(id) => s.selected = Some(id),
            Action::Toggle(id) => {
                if let Some(Item { kind: Kind::Folder { open, .. }, .. }) = find_mut(&mut s.items, id) {
                    *open = !*open;
                }
            }
            Action::StartRename(id) => s.renaming = Some(id),
            Action::Rename(id, name) => {
                if let Some(it) = find_mut(&mut s.items, id) {
                    it.name = name;
                }
            }
            Action::EndRename => {
                if let Some(it) = s.renaming.and_then(|id| find_mut(&mut s.items, id)) {
                    if it.name.trim().is_empty() {
                        it.name = "Untitled".into();
                    }
                }
                s.renaming = None;
            }
            Action::SetBody(text) => {
                if let Some(Item { kind: Kind::Note { body }, .. }) =
                    s.selected.and_then(|id| find_mut(&mut s.items, id))
                {
                    *body = text;
                }
            }
            Action::NewNote(parent) => {
                let id = s.alloc();
                insert_into(&mut s.items, parent, Item::note(id, "Untitled", ""));
                s.selected = Some(id);
                s.renaming = Some(id);
            }
            Action::NewFolder(parent) => {
                let id = s.alloc();
                insert_into(&mut s.items, parent, Item::folder(id, "New Folder", vec![]));
                s.renaming = Some(id);
            }
            Action::Delete(id) => {
                remove(&mut s.items, id);
                if s.selected.is_some_and(|sel| find(&s.items, sel).is_none()) {
                    s.selected = first_note(&s.items);
                }
            }
            Action::Breakout { remaining, extracted } => {
                let parent = s.current_folder();
                if let Some(Item { kind: Kind::Note { body }, .. }) =
                    s.selected.and_then(|id| find_mut(&mut s.items, id))
                {
                    *body = remaining;
                }
                let name: String = extracted.split_whitespace().take(4).collect::<Vec<_>>().join(" ");
                let id = s.alloc();
                insert_into(&mut s.items, parent, Item::note(id, &name, extracted.trim()));
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
// App
// ---------------------------------------------------------------------------

#[function_component(App)]
fn app() -> Html {
    let state = use_reducer(State::load);

    {
        use_effect_with(state.clone(), |s| {
            let _ = LocalStorage::set(STORAGE_KEY, &**s);
        });
    }

    let dispatch = {
        let state = state.clone();
        Callback::from(move |a: Action| state.dispatch(a))
    };

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

    let content = match note {
        Some(Item { id, kind: Kind::Note { body }, .. }) => html! {
            <Editor key={id} note_id={id} {body} dispatch={dispatch.clone()} />
        },
        _ => html! {
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
                <button class="new-notebook" onclick={dispatch.reform(|_| Action::NewFolder(None))}>
                    { "+ New notebook" }
                </button>
                <svg class="mark" viewBox="0 0 14 26" aria-label="Calabria">
                    <path d="M1 1h12v14L7 25 1 15z" fill="#2b2b2b"/>
                    <path d="M7 4v13" stroke="#fff" stroke-width="1.4"/>
                </svg>
            </nav>
            <main class="page">
                { content }
            </main>
            if words > 0 {
                <footer class="count">{ format!("{words} words ({minutes} min)") }</footer>
            }
        </div>
    }
}

fn main() {
    yew::Renderer::<App>::new().render();
}
