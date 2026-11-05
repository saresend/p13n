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

/// The description of the "check for action items" task for a note.
pub fn check_description(note_path: &str) -> String {
    let progress = progress_path(note_path);
    format!(
        "The user just made some changes to file `{note_path}`. You are to process that and determine \
whether there is actionable work items agents need to do as part of the user's changes.

If there are, please scope and submit new tasks using the `dsys-cli` describing the work items. \
For each work item, please include the following info in the task:

> You are responsible for keeping `{progress}` up to date. The user will review progress using this \
html file, so as you work make sure to add visuals and *concise* descriptions of the status of this work.
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
    fn description_names_the_note_and_its_report() {
        let d = check_description("Archive/Trip.md");
        assert!(d.contains("changes to file `Archive/Trip.md`"));
        assert!(d.contains("keeping `Archive/Trip_progress.html` up to date"));
        assert!(d.contains("`dsys-cli`"));
    }
}
