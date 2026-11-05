//! On-disk files for tasks: `<tasks_dir>/<title>/{description.md,progress_log.md}`,
//! laid out the same way kyogre does it.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::errors::ApiResult;

pub(crate) const DESCRIPTION_FILE: &str = "description.md";
pub(crate) const PROGRESS_LOG_FILE: &str = "progress_log.md";

#[derive(Clone, Debug)]
pub(crate) struct TaskStorage {
    root: PathBuf,
}

pub(crate) struct TaskFiles {
    pub description: PathBuf,
    pub progress_log: PathBuf,
}

/// Turn a title into a directory name: trimmed, with `/`, `\` and NUL replaced.
pub(crate) fn sanitize_title(title: &str) -> String {
    let cleaned: String = title
        .trim()
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | '\0') { '-' } else { c })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "task".to_string()
    } else {
        cleaned
    }
}

impl TaskStorage {
    pub fn new(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root: root.canonicalize()? })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Create a fresh folder for a task (title, title-2, title-3, ...) and
    /// seed its description and progress log.
    pub fn create(&self, title: &str, description: &str) -> ApiResult<TaskFiles> {
        let base = sanitize_title(title);
        let mut dir = self.root.join(&base);
        let mut n = 1;
        // create_dir (not create_dir_all) fails if it exists, so two concurrent
        // creates with the same title can't share a folder.
        loop {
            match fs::create_dir(&dir) {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    n += 1;
                    dir = self.root.join(format!("{base}-{n}"));
                }
                Err(e) => return Err(e.into()),
            }
        }

        let files = TaskFiles {
            description: dir.join(DESCRIPTION_FILE),
            progress_log: dir.join(PROGRESS_LOG_FILE),
        };
        fs::write(&files.description, format!("# {title}\n\n{description}\n"))?;
        fs::write(
            &files.progress_log,
            format!("# Progress Log: {title}\n\nAppend dated log entries below as you work on this task.\n"),
        )?;
        Ok(files)
    }

    pub fn read(path: &str) -> ApiResult<String> {
        Ok(fs::read_to_string(path)?)
    }

    pub fn write(path: &str, contents: &str) -> ApiResult<()> {
        Ok(fs::write(path, contents)?)
    }

    pub fn append_log(path: &str, entry: &str, timestamp_ms: i64) -> ApiResult<()> {
        let mut f = OpenOptions::new().append(true).create(true).open(path)?;
        let secs = timestamp_ms / 1000;
        write!(f, "\n## {}\n\n{}\n", format_utc(secs), entry.trim_end())?;
        Ok(())
    }

    /// Remove a task's folder, but only if it lives under our root.
    pub fn remove_task_dir(&self, description_path: &str) -> ApiResult<()> {
        let Some(dir) = Path::new(description_path).parent() else {
            return Ok(());
        };
        if dir.starts_with(&self.root) && dir != self.root && dir.exists() {
            fs::remove_dir_all(dir)?;
        }
        Ok(())
    }
}

/// `YYYY-MM-DD HH:MM:SS UTC` without pulling in a date crate.
fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_titles() {
        assert_eq!(sanitize_title("  a/b\\c "), "a-b-c");
        assert_eq!(sanitize_title(".."), "task");
        assert_eq!(sanitize_title("   "), "task");
    }

    #[test]
    fn formats_utc() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_utc(1_793_891_625), "2026-11-05 15:13:45 UTC");
    }
}
