//! Tasks the app hands to agents through dsys.
//!
//! When a note is saved, a "check for action items" task asks an agent to
//! read the change and file follow-up tasks (with `dsys-cli`) for any work it
//! finds. Each follow-up task must keep the note's progress report current.

use std::collections::HashMap;

use gloo_storage::{LocalStorage, Storage};

use crate::progress::progress_path;

pub const CHECK_TITLE: &str = "Check new user text for action items";

/// Which check task was last filed for each note path, so a note doesn't
/// queue a new one while the previous one is still waiting to be picked up.
const PENDING_KEY: &str = "calabria.check-tasks.v1";

/// Where the user's progress preferences live on dsys (hidden from the notes list).
pub const PROGRESS_PREFS_PATH: &str = ".calabria/progress_preferences.md";

/// The block every follow-up task must carry: keep the progress report
/// current, plus the user's own preferences for it, all as one quote.
pub fn progress_instructions(progress: &str, prefs: &str) -> String {
    let mut out = format!(
        "> You are responsible for keeping `{progress}` up to date. The user will review progress using this \
html file, so as you work make sure to add visuals and *concise* descriptions of the status of this work."
    );
    let prefs = prefs.trim();
    if !prefs.is_empty() {
        out.push_str("\n>\n> The user's preferences for the progress page:\n>\n");
        let quoted: Vec<String> =
            prefs.lines().map(|l| if l.trim().is_empty() { ">".to_string() } else { format!("> {l}") }).collect();
        out.push_str(&quoted.join("\n"));
    }
    out
}

/// The description of the "check for action items" task for a note.
pub fn check_description(note_path: &str, prefs: &str) -> String {
    let instructions = progress_instructions(&progress_path(note_path), prefs);
    format!(
        "The user just made some changes to file `{note_path}`. You are to process that and determine \
whether there is actionable work items agents need to do as part of the user's changes.

If there are, please scope and submit new tasks using the `dsys-cli` describing the work items. \
For each work item, please include the following info in the task:

{instructions}
"
    )
}

pub fn last_check_task(note_path: &str) -> Option<i64> {
    LocalStorage::get::<HashMap<String, i64>>(PENDING_KEY).ok()?.get(note_path).copied()
}

pub fn remember_check_task(note_path: &str, id: i64) {
    let mut map: HashMap<String, i64> = LocalStorage::get(PENDING_KEY).unwrap_or_default();
    map.insert(note_path.to_string(), id);
    let _ = LocalStorage::set(PENDING_KEY, map);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_includes_preferences_in_the_quote() {
        let d = check_description("Trip.md", "Lead with a chart.\n\n- Keep it short");
        assert!(d.contains("> The user's preferences for the progress page:\n>\n> Lead with a chart.\n>\n> - Keep it short"));
        assert!(!check_description("Trip.md", "  \n").contains("preferences"));
    }

    #[test]
    fn description_names_the_note_and_its_report() {
        let d = check_description("Archive/Trip.md", "");
        assert!(d.contains("changes to file `Archive/Trip.md`"));
        assert!(d.contains("keeping `Archive/Trip_progress.html` up to date"));
        assert!(d.contains("`dsys-cli`"));
    }
}
