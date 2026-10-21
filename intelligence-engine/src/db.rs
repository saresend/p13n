//! Embedded SQLite: one row per action, plus each schema posted to `/init`.
//! Schema creation is idempotent. Evolve it with explicit migrations, never by dropping data.

use crate::action::{Action, Source};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS actions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id     TEXT NOT NULL,
    session_id  TEXT NOT NULL,
    action_name TEXT NOT NULL,
    payload     TEXT NOT NULL,
    source      TEXT NOT NULL CHECK (source IN ('user', 'agent', 'system')),
    received_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS actions_by_user ON actions (user_id, id);
CREATE TABLE IF NOT EXISTS schemas (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    schema     TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
";

/// Opens (creating if needed) the database. Each thread opens its own connection;
/// WAL lets worker reads run alongside API writes.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn insert_action(conn: &Connection, action: &Action) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO actions (user_id, session_id, action_name, payload, source, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            action.user_id,
            action.session_id,
            action.action_name,
            action.payload.to_string(),
            action.source.as_str(),
            now_ms()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Number of the user's actions with id greater than `after`.
pub fn count_after(conn: &Connection, user_id: &str, after: i64) -> rusqlite::Result<usize> {
    conn.query_row(
        "SELECT COUNT(*) FROM actions WHERE user_id = ?1 AND id > ?2",
        params![user_id, after],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n as usize)
}

/// The user's newest `limit` actions with id greater than `after`, oldest first, with their ids.
pub fn latest_after(
    conn: &Connection,
    user_id: &str,
    after: i64,
    limit: usize,
) -> rusqlite::Result<Vec<(i64, Action)>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, session_id, action_name, payload, source FROM actions
         WHERE user_id = ?1 AND id > ?2 ORDER BY id DESC LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![user_id, after, limit as i64], |row| {
        let payload: String = row.get(4)?;
        let source: String = row.get(5)?;
        Ok((
            row.get(0)?,
            Action {
                user_id: row.get(1)?,
                session_id: row.get(2)?,
                action_name: row.get(3)?,
                payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                // The CHECK constraint keeps this valid.
                source: Source::parse(&source).unwrap_or(Source::User),
            },
        ))
    })?;
    let mut actions = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    actions.reverse();
    Ok(actions)
}

pub fn insert_schema(conn: &Connection, schema: &Value) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO schemas (schema, created_at) VALUES (?1, ?2)",
        params![schema.to_string(), now_ms()],
    )?;
    Ok(())
}

pub fn latest_schema(conn: &Connection) -> rusqlite::Result<Option<Value>> {
    let text: Option<String> = conn
        .query_row(
            "SELECT schema FROM schemas ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(text.and_then(|text| serde_json::from_str(&text).ok()))
}
