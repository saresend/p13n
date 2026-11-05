//! Progress reports for notes.
//!
//! Each note `X.md` can have a read-only report `X_progress.html` beside it,
//! written by the agent working on the note's tasks. Until that agent exists,
//! a simulated one here turns the note's checklist and list items into tasks,
//! "works" through them one after another, and writes the report the same way
//! the real agent would, so the progress view only ever reads a file.

use std::collections::HashMap;

use gloo_storage::{LocalStorage, Storage};

const FIRST_SEEN_KEY: &str = "calabria.sim-agent.v1";
const MAX_TASKS: usize = 8;

/// Where a note's progress report lives: `Dir/Name.md` -> `Dir/Name_progress.html`.
pub fn progress_path(note_path: &str) -> String {
    let stem = note_path.strip_suffix(".md").unwrap_or(note_path);
    format!("{stem}_progress.html")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub text: String,
    /// Already checked off in the note (`- [x]`).
    pub checked: bool,
}

/// Tasks implied by a note: checklist items, else list items, else section
/// headings (not the title line).
pub fn tasks_in(body: &str) -> Vec<Task> {
    let mut checklist = Vec::new();
    let mut items = Vec::new();
    let mut headings = Vec::new();
    let mut in_fence = false;
    let mut first_content_line = true;
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || t.is_empty() {
            continue;
        }
        let is_title = std::mem::replace(&mut first_content_line, false);
        let clean = |s: &str| s.replace(['*', '`'], "").trim().to_string();
        if let Some(rest) = t.strip_prefix("- [ ] ").or_else(|| t.strip_prefix("* [ ] ")) {
            checklist.push(Task { text: clean(rest), checked: false });
        } else if let Some(rest) =
            t.strip_prefix("- [x] ").or_else(|| t.strip_prefix("- [X] ")).or_else(|| t.strip_prefix("* [x] "))
        {
            checklist.push(Task { text: clean(rest), checked: true });
        } else if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("+ ")) {
            items.push(Task { text: clean(rest), checked: false });
        } else if t.starts_with('#') && !is_title {
            headings.push(Task { text: clean(t.trim_start_matches('#')), checked: false });
        } else {
            let digits = t.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 && t[digits..].starts_with(". ") {
                items.push(Task { text: clean(&t[digits + 2..]), checked: false });
            }
        }
    }
    let mut tasks = [checklist, items, headings].into_iter().find(|v| !v.is_empty()).unwrap_or_default();
    tasks.retain(|t| !t.text.is_empty());
    tasks.truncate(MAX_TASKS);
    tasks
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stage {
    Queued,
    Working { step: usize },
    Done,
}

const STEPS: [&str; 4] = ["Picked up the task", "Read the note for context", "Did the work", "Checked the result"];

/// Stable 32-bit hash, so a task always gets the same duration.
fn hash(s: &str) -> u32 {
    s.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// How long the simulated agent spends on a task: 20–50 seconds.
fn duration_ms(text: &str) -> i64 {
    20_000 + i64::from(hash(text) % 30_000)
}

pub struct Scheduled {
    pub task: Task,
    pub stage: Stage,
    pub start: i64,
    pub finish: i64,
}

/// Lay tasks out one after another: each starts when it was first seen or
/// when the previous one finishes, whichever is later.
pub fn schedule(tasks: &[Task], first_seen: &HashMap<String, i64>, now: i64) -> Vec<Scheduled> {
    let mut free_at = i64::MIN;
    tasks
        .iter()
        .map(|t| {
            let seen = first_seen.get(&t.text).copied().unwrap_or(now);
            if t.checked {
                return Scheduled { task: t.clone(), stage: Stage::Done, start: seen, finish: seen };
            }
            let start = seen.max(free_at);
            let finish = start + duration_ms(&t.text);
            free_at = finish;
            let stage = if now < start {
                Stage::Queued
            } else if now < finish {
                let step = ((now - start) * STEPS.len() as i64 / (finish - start)) as usize;
                Stage::Working { step: step.min(STEPS.len() - 1) }
            } else {
                Stage::Done
            };
            Scheduled { task: t.clone(), stage, start, finish }
        })
        .collect()
}

/// When each task was first seen, kept in this browser so the simulated
/// agent's progress survives reloads.
pub fn remember_tasks(tasks: &[Task], now: i64) -> HashMap<String, i64> {
    let mut seen: HashMap<String, i64> = LocalStorage::get(FIRST_SEEN_KEY).unwrap_or_default();
    let before = seen.len();
    for t in tasks {
        seen.entry(t.text.clone()).or_insert(now);
    }
    if seen.len() != before {
        let _ = LocalStorage::set(FIRST_SEEN_KEY, &seen);
    }
    seen
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn clock(ms: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen_float(ms));
    format!("{:02}:{:02}", d.get_hours(), d.get_minutes())
}

fn wasm_bindgen_float(ms: i64) -> web_sys::wasm_bindgen::JsValue {
    web_sys::wasm_bindgen::JsValue::from_f64(ms as f64)
}

/// The simulated agent's report for a note, or `None` if it has no tasks.
/// Contains no relative times, so it only changes when a task moves on.
pub fn simulated_report(title: &str, body: &str, now: i64) -> Option<String> {
    let tasks = tasks_in(body);
    if tasks.is_empty() {
        return None;
    }
    let seen = remember_tasks(&tasks, now);
    let plan = schedule(&tasks, &seen, now);
    Some(render(title, &plan, clock))
}

/// Build the report HTML. `clock` formats a timestamp (injected for tests).
pub fn render(title: &str, plan: &[Scheduled], clock: impl Fn(i64) -> String) -> String {
    let done = plan.iter().filter(|s| s.stage == Stage::Done).count();
    let current = plan.iter().find(|s| matches!(s.stage, Stage::Working { .. }));
    let summary = match (current, done == plan.len()) {
        (_, true) => "All tasks are done.".to_string(),
        (Some(c), _) => format!("Working on <strong>{}</strong>.", esc(&c.task.text)),
        (None, _) => "Waiting to start.".to_string(),
    };

    let mut rows = String::new();
    for s in plan {
        let (class, label) = match s.stage {
            Stage::Queued => ("queued", "Queued".to_string()),
            Stage::Working { .. } => ("working", "Working".to_string()),
            Stage::Done => ("done", "Done".to_string()),
        };
        let mut log = String::new();
        match s.stage {
            Stage::Queued => {}
            Stage::Working { step } => {
                for (i, text) in STEPS.iter().enumerate().take(step + 1) {
                    let at = s.start + (s.finish - s.start) * i as i64 / STEPS.len() as i64;
                    let now_class = if i == step { " class=\"now\"" } else { "" };
                    log.push_str(&format!("<li{now_class}><time>{}</time>{text}</li>", clock(at)));
                }
            }
            Stage::Done if s.task.checked => {
                log.push_str("<li><time></time>Already checked off in the note</li>");
            }
            Stage::Done => {
                log.push_str(&format!("<li><time>{}</time>Finished</li>", clock(s.finish)));
            }
        }
        let log = if log.is_empty() { String::new() } else { format!("<ol class=\"log\">{log}</ol>") };
        rows.push_str(&format!(
            "<li class=\"task {class}\"><div class=\"head\"><span class=\"state\">{label}</span>\
             <span class=\"text\">{}</span></div>{log}</li>",
            esc(&s.task.text)
        ));
    }

    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Progress: {title}</title>
<style>
  :root {{ color-scheme: dark; --bg: #141414; --ink: #d9d6d0; --muted: #8a867f; --faint: #4a4844;
          --line: #262524; --work: #e0b860; --done: #8fbf96; }}
  body {{ margin: 0; padding: 56px 48px 80px; background: var(--bg); color: var(--ink);
         font: 15px/1.6 "Inter", system-ui, sans-serif; }}
  main {{ max-width: 680px; margin: 0 auto; }}
  .eyebrow {{ font-size: 11px; letter-spacing: .08em; text-transform: uppercase; color: var(--muted); }}
  h1 {{ margin: 6px 0 4px; font: 600 28px/1.25 "Source Serif 4", Georgia, serif; }}
  .summary {{ margin: 0 0 6px; color: var(--muted); }}
  .summary strong {{ color: var(--ink); font-weight: 500; }}
  .meter {{ height: 2px; background: var(--line); margin: 18px 0 30px; }}
  .meter span {{ display: block; height: 100%; background: var(--done); width: {pct}%; }}
  ul.tasks {{ list-style: none; margin: 0; padding: 0; display: grid; gap: 18px; }}
  .head {{ display: flex; gap: 14px; align-items: baseline; }}
  .state {{ flex: none; width: 64px; font-size: 11px; letter-spacing: .06em; text-transform: uppercase; }}
  .queued .state, .queued .text {{ color: var(--faint); }}
  .working .state {{ color: var(--work); }}
  .done .state {{ color: var(--done); }}
  .done .text {{ color: var(--muted); }}
  .log {{ list-style: none; margin: 6px 0 0 78px; padding: 0; font-size: 13px; color: var(--muted); }}
  .log li {{ display: flex; gap: 10px; }}
  .log li.now {{ color: var(--ink); }}
  .log time {{ flex: none; width: 38px; color: var(--faint); font-variant-numeric: tabular-nums; }}
  footer {{ margin-top: 40px; font-size: 12px; color: var(--faint); }}
  @media (max-width: 560px) {{ body {{ padding: 32px 18px 60px; }} .log {{ margin-left: 0; }} }}
</style></head>
<body><main>
  <div class="eyebrow">Progress · {done} of {total} done</div>
  <h1>{title}</h1>
  <p class="summary">{summary}</p>
  <div class="meter"><span></span></div>
  <ul class="tasks">{rows}</ul>
  <footer>Simulated agent. This report is written to the note's _progress.html file, where a real agent will write it later.</footer>
</main></body></html>
"#,
        title = esc(title),
        pct = done * 100 / plan.len().max(1),
        total = plan.len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_paths() {
        assert_eq!(progress_path("Groceries.md"), "Groceries_progress.html");
        assert_eq!(progress_path("Archive/Plan.md"), "Archive/Plan_progress.html");
    }

    #[test]
    fn finds_tasks() {
        let body = "# Trip\n\n- [ ] Book **flights**\n- [x] Renew passport\n- not a task when checklists exist\n";
        let t = tasks_in(body);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], Task { text: "Book flights".into(), checked: false });
        assert!(t[1].checked);

        assert_eq!(tasks_in("# T\n\n1. one\n2. two\n").len(), 2);
        let h = tasks_in("# Title\n\n## Research\ntext\n## Draft\n");
        assert_eq!(h.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(), ["Research", "Draft"]);
        assert!(tasks_in("Just prose.\n```\n- inside code\n```").is_empty());
    }

    #[test]
    fn tasks_run_one_after_another() {
        let tasks = vec![
            Task { text: "a".into(), checked: false },
            Task { text: "b".into(), checked: false },
            Task { text: "c".into(), checked: true },
        ];
        let seen: HashMap<String, i64> = [("a".into(), 0), ("b".into(), 0), ("c".into(), 0)].into();
        let at = |now| schedule(&tasks, &seen, now).iter().map(|s| s.stage).collect::<Vec<_>>();
        assert!(matches!(at(1)[..], [Stage::Working { step: 0 }, Stage::Queued, Stage::Done]));
        let a_done = duration_ms("a");
        assert!(matches!(at(a_done + 1)[..], [Stage::Done, Stage::Working { .. }, Stage::Done]));
        assert!(at(a_done + duration_ms("b") + 1).iter().all(|s| *s == Stage::Done));
    }

    #[test]
    fn report_escapes_and_summarizes() {
        let tasks = vec![Task { text: "<b>x</b>".into(), checked: false }];
        let seen: HashMap<String, i64> = [("<b>x</b>".into(), 0)].into();
        let html = render("A & B", &schedule(&tasks, &seen, 1), |_| "09:00".into());
        assert!(html.contains("A &amp; B") && html.contains("&lt;b&gt;x&lt;/b&gt;"));
        assert!(html.contains("0 of 1 done") && html.contains("Working on"));
    }
}
