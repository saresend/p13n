//! Push-to-talk dictation using the browser's speech recognition
//! (`SpeechRecognition` / `webkitSpeechRecognition`: Chrome, Edge, Safari).
//!
//! This is the only place that talks to the speech engine, so a different
//! one (e.g. Whisper behind dsys) can replace it without touching the editor.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use js_sys::{Function, Reflect};
use web_sys::wasm_bindgen::closure::Closure;
use web_sys::wasm_bindgen::{JsCast, JsValue};

fn constructor() -> Option<Function> {
    let window = web_sys::window()?;
    ["SpeechRecognition", "webkitSpeechRecognition"]
        .iter()
        .find_map(|name| Reflect::get(&window, &JsValue::from_str(name)).ok()?.dyn_into::<Function>().ok())
}

/// The hosted demo build can't use the microphone (its frame blocks it), so
/// it "hears" sample sentences instead, to show how dictation feels.
pub fn sample_voice() -> bool {
    option_env!("CALABRIA_DSYS_URL") == Some("demo")
}

const SAMPLES: [&str; 3] = [
    "remember to call the plumber about the kitchen sink",
    "add a task to compare flights for the second week of May",
    "ask the agent to draft a short summary of this note",
];

/// Whether this browser can dictate at all.
pub fn supported() -> bool {
    sample_voice() || constructor().is_some()
}

/// What the recognizer reports.
pub enum Event {
    /// Words heard so far that may still change.
    Interim(String),
    /// Words that are settled; insert them.
    Final(String),
    /// Something went wrong (e.g. "not-allowed" when the mic is blocked).
    Error(String),
    /// The session is over.
    End,
}

/// A running dictation session. Dropping it aborts the session.
pub struct Session(Engine);

enum Engine {
    Browser { rec: JsValue, _handlers: Vec<Closure<dyn FnMut(JsValue)>> },
    Sample(Rc<SampleState>),
}

/// The demo's stand-in: reveals a sample sentence word by word.
struct SampleState {
    words: Vec<&'static str>,
    heard: Cell<usize>,
    done: Cell<bool>,
    on_event: Rc<dyn Fn(Event)>,
    ticker: RefCell<Option<gloo_timers::callback::Interval>>,
}

static SAMPLE_TURN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl SampleState {
    fn finish(self: &Rc<Self>, keep: bool) {
        if self.done.replace(true) {
            return;
        }
        self.ticker.borrow_mut().take();
        let this = self.clone();
        // Report asynchronously, like the real engine does.
        gloo_timers::callback::Timeout::new(80, move || {
            let n = this.heard.get();
            if keep && n > 0 {
                (this.on_event)(Event::Final(this.words[..n].join(" ")));
            }
            (this.on_event)(Event::End);
        })
        .forget();
    }
}

fn get(v: &JsValue, key: &str) -> JsValue {
    Reflect::get(v, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

fn call(v: &JsValue, method: &str) {
    if let Ok(f) = get(v, method).dyn_into::<Function>() {
        let _ = f.call0(v);
    }
}

impl Session {
    /// Start listening. `on_event` gets interim and final text as it comes.
    pub fn start(on_event: impl Fn(Event) + 'static) -> Option<Session> {
        if sample_voice() {
            let turn = SAMPLE_TURN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let state = Rc::new(SampleState {
                words: SAMPLES[turn % SAMPLES.len()].split(' ').collect(),
                heard: Cell::new(0),
                done: Cell::new(false),
                on_event: Rc::new(on_event),
                ticker: RefCell::new(None),
            });
            let weak = Rc::downgrade(&state);
            let tick = gloo_timers::callback::Interval::new(260, move || {
                let Some(s) = weak.upgrade() else { return };
                let n = (s.heard.get() + 1).min(s.words.len());
                s.heard.set(n);
                (s.on_event)(Event::Interim(s.words[..n].join(" ")));
            });
            *state.ticker.borrow_mut() = Some(tick);
            return Some(Session(Engine::Sample(state)));
        }
        let ctor = constructor()?;
        let rec = Reflect::construct(&ctor, &js_sys::Array::new()).ok()?;
        let set = |k: &str, v: JsValue| {
            let _ = Reflect::set(&rec, &JsValue::from_str(k), &v);
        };
        set("continuous", JsValue::TRUE);
        set("interimResults", JsValue::TRUE);
        if let Some(lang) = web_sys::window().and_then(|w| w.navigator().language()) {
            set("lang", JsValue::from_str(&lang));
        }

        let on_event = Rc::new(on_event);
        let mut handlers = Vec::new();
        let mut on = |name: &str, f: Box<dyn FnMut(JsValue)>| {
            let c = Closure::<dyn FnMut(JsValue)>::wrap(f);
            set(name, c.as_ref().clone());
            handlers.push(c);
        };

        let e = on_event.clone();
        on(
            "onresult",
            Box::new(move |ev: JsValue| {
                let results = get(&ev, "results");
                let from = get(&ev, "resultIndex").as_f64().unwrap_or(0.0) as u32;
                let len = get(&results, "length").as_f64().unwrap_or(0.0) as u32;
                let (mut finished, mut pending) = (String::new(), String::new());
                for i in from..len {
                    let result = Reflect::get_u32(&results, i).unwrap_or(JsValue::UNDEFINED);
                    let best = Reflect::get_u32(&result, 0).unwrap_or(JsValue::UNDEFINED);
                    let text = get(&best, "transcript").as_string().unwrap_or_default();
                    if get(&result, "isFinal").as_bool().unwrap_or(false) {
                        finished.push_str(&text);
                    } else {
                        pending.push_str(&text);
                    }
                }
                if !finished.trim().is_empty() {
                    e(Event::Final(finished));
                }
                e(Event::Interim(pending));
            }),
        );
        let e = on_event.clone();
        on(
            "onerror",
            Box::new(move |ev: JsValue| {
                let code = get(&ev, "error").as_string().unwrap_or_else(|| "unknown".into());
                // "no-speech" and "aborted" are normal for a short or cancelled hold.
                if code != "no-speech" && code != "aborted" {
                    e(Event::Error(code));
                }
            }),
        );
        let e = on_event;
        on("onend", Box::new(move |_| e(Event::End)));

        call(&rec, "start");
        Some(Session(Engine::Browser { rec, _handlers: handlers }))
    }

    /// Stop listening; words already heard still arrive as `Final`.
    pub fn stop(&self) {
        match &self.0 {
            Engine::Browser { rec, .. } => call(rec, "stop"),
            Engine::Sample(s) => s.finish(true),
        }
    }

    /// Stop and throw away anything not yet final.
    pub fn abort(&self) {
        match &self.0 {
            Engine::Browser { rec, .. } => call(rec, "abort"),
            Engine::Sample(s) => s.finish(false),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        match &self.0 {
            Engine::Browser { rec, .. } => {
                // Detach handlers before the closures are freed.
                for name in ["onresult", "onerror", "onend"] {
                    let _ = Reflect::set(rec, &JsValue::from_str(name), &JsValue::NULL);
                }
                call(rec, "abort");
            }
            Engine::Sample(s) => {
                s.done.set(true);
                s.ticker.borrow_mut().take();
            }
        }
    }
}

/// A plain-language message for a recognition error code.
pub fn explain(code: &str) -> String {
    match code {
        "not-allowed" | "service-not-allowed" => "Microphone access is blocked for this page.".into(),
        "audio-capture" => "No microphone found.".into(),
        "network" => "Dictation needs a network connection in this browser.".into(),
        other => format!("Dictation stopped ({other})."),
    }
}

/// Insert dictated `words` into `text` at byte offset `at`, adding a space
/// where needed and capitalizing at the start of a sentence. Returns the new
/// text and the byte offset just after the insertion.
pub fn insert(text: &str, at: usize, words: &str) -> (String, usize) {
    let words = words.trim();
    if words.is_empty() {
        return (text.to_string(), at);
    }
    let before = &text[..at];
    let after = &text[at..];
    let prev = before.chars().last();
    let sentence_start = before.trim_end_matches([' ', '\t']).chars().last().is_none_or(|c| matches!(c, '.' | '!' | '?' | '\n'));
    let mut piece = String::new();
    if prev.is_some_and(|c| !c.is_whitespace()) {
        piece.push(' ');
    }
    let mut chars = words.chars();
    if let Some(first) = chars.next() {
        if sentence_start {
            piece.extend(first.to_uppercase());
        } else {
            piece.push(first);
        }
        piece.push_str(chars.as_str());
    }
    if after.chars().next().is_some_and(|c| !c.is_whitespace() && !matches!(c, '.' | ',' | '!' | '?' | ';' | ':')) {
        piece.push(' ');
    }
    let end = at + piece.len();
    (format!("{before}{piece}{after}"), end)
}

#[cfg(test)]
mod tests {
    use super::insert;

    #[test]
    fn inserts_with_spacing_and_capitals() {
        assert_eq!(insert("", 0, "hello there"), ("Hello there".into(), 11));
        assert_eq!(insert("Hi.", 3, "next one"), ("Hi. Next one".into(), 12));
        assert_eq!(insert("buy milk", 8, "and eggs"), ("buy milk and eggs".into(), 17));
        assert_eq!(insert("buy  eggs", 4, "milk"), ("buy milk eggs".into(), 8));
        assert_eq!(insert("ab", 1, " x "), ("a x b".into(), 4));
        assert_eq!(insert("end", 3, "   "), ("end".into(), 3));
        assert_eq!(insert("line\n", 5, "new line"), ("line\nNew line".into(), 13));
        assert_eq!(insert("so", 2, "done"), ("so done".into(), 7));
        assert_eq!(insert("x.", 1, "y"), ("x y.".into(), 3));
    }

}
