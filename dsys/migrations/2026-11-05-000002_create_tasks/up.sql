-- Same shape as kyogre's `tasks` table.
CREATE TABLE tasks (
  id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
  title TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'Available'
    CHECK (status IN ('Available', 'In-Progress', 'Complete')),
  task_description_path TEXT NOT NULL,
  progress_log_path TEXT NOT NULL,
  model TEXT NOT NULL DEFAULT '',
  taken_by_pid INTEGER NOT NULL DEFAULT 0,
  taken_by_start INTEGER NOT NULL DEFAULT 0
);
