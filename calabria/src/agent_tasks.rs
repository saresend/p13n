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

/// The block every Monitor task must carry: keep the note's progress page
/// current, the user's own preferences for it, and how to share the page
/// with other Monitor tasks. All one quote.
pub fn progress_instructions(progress: &str, prefs: &str) -> String {
    let mut out = format!(
        "> You are responsible for keeping `{progress}` up to date. The user will review progress using this \
html file, so as you work make sure to add visuals and *concise* descriptions of the status of this work."
    );
    let prefs = prefs.trim();
    if !prefs.is_empty() {
        out.push_str("\n>\n> The user's preferences for the progress page:\n>\n");
        let quoted: Vec<String> =
            prefs.lines().map(|l| if l.trim().is_empty() { "> >".to_string() } else { format!("> > {l}") }).collect();
        out.push_str(&quoted.join("\n"));
    }
    out.push_str(&format!(
        "\n>\n> Other Monitor tasks may share this file, one section per work item. Read it first \
(`dsys-cli files get \"{progress}\"`), change only your own section, and write it back with \
`dsys-cli files put \"{progress}\" --if-match <etag>` so you don't overwrite someone else's update. \
If the write is rejected, read the file again and retry."
    ));
    out
}

/// The description of the "check for action items" task for a note.
/// `task_id` is this task's own id, once dsys has assigned it.
pub fn check_description(note_path: &str, prefs: &str, task_id: Option<i64>) -> String {
    let progress = progress_path(note_path);
    let monitor_block = progress_instructions(&progress, prefs)
        .lines()
        .map(|l| format!("   {l}"))
        .collect::<Vec<_>>()
        .join("\n");
    let me = task_id.map_or_else(|| "<this task's id>".to_string(), |id| id.to_string());
    format!(
        r#"The user just saved `{note_path}`. Your job is to keep the agent tasks for this note in step with what it now says. Be conservative: only create or delete tasks when you are confident it is what the user wants. When in doubt, do nothing. Use `dsys-cli` for everything below. This task's id is {me}.

1. **Read the note.** Run `dsys-cli files get "{note_path}"`. This is the current, saved version. Work only from what it says.

2. **Find the existing tasks for this note.** Every task for this note has a title starting with `[{note_path}]`. Run `dsys-cli tasks list` and, for each matching task, read its description with `dsys-cli tasks description <id>`. Each work item has a pair of tasks:
   - `[{note_path}] Do: <work item>`
   - `[{note_path}] Monitor: <work item>`

3. **Remove tasks that clearly no longer apply.** Only if the note plainly no longer asks for a work item (the user deleted it, checked it off, or replaced it), delete both of its tasks with `dsys-cli tasks delete <id>`. Leave no trace for the user: don't add log entries, and remove that work item's section from `{progress}`. Read it with `dsys-cli files get`, write it back with `dsys-cli files put --if-match <etag>`, and re-read and retry if the write is rejected. If the page has no sections left, delete it with `dsys-cli files rm "{progress}"`.

4. **Create tasks for new work, only when you are confident.** Create tasks only for work the note clearly asks for, with enough detail to act on, and that no existing task covers. Do nothing for notes, ideas, questions, or anything ambiguous. For each such work item, create two tasks:

   - **Do task:** `dsys-cli tasks create -t "[{note_path}] Do: <short name>" -d "<description>"`. The description should state what to do, what "done" means, and any details from the note the worker needs. It should be self-contained, because the worker won't see this task.

   - **Monitor task:** `dsys-cli tasks create -t "[{note_path}] Monitor: <short name>" -d "<description>"`. Its description must include the Do task's id, and say: *Watch task `<id>` (`dsys-cli tasks get <id>` and `dsys-cli tasks log <id>`) until it is Complete, and report its progress to the user as described below. When the work is done and the report says so, mark this task Complete.* It must also include this block:

{monitor_block}

5. **Finish.** Mark this task Complete: `dsys-cli tasks update {me} --status Complete`.
"#
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
    fn description_includes_preferences_in_the_monitor_block() {
        let d = check_description("Trip.md", "Lead with a chart.\n\n- Keep it short", Some(7));
        assert!(d.contains("   > The user's preferences for the progress page:\n   >\n   > > Lead with a chart.\n   > >\n   > > - Keep it short"));
        assert!(!check_description("Trip.md", "  \n", None).contains("preferences for"));
    }

    #[test]
    fn description_names_the_note_its_report_and_itself() {
        let d = check_description("Archive/Trip.md", "", Some(42));
        assert!(d.contains("The user just saved `Archive/Trip.md`"));
        assert!(d.contains("`dsys-cli files get \"Archive/Trip.md\"`"));
        assert!(d.contains("title starting with `[Archive/Trip.md]`"));
        assert!(d.contains("`Archive/Trip_progress.html`"));
        assert!(d.contains("This task's id is 42."));
        assert!(d.contains("`dsys-cli tasks update 42 --status Complete`"));
        assert!(check_description("Trip.md", "", None).contains("<this task's id>"));
    }
}
