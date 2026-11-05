use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::{ApiError, ApiResult};

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

#[derive(Queryable, Selectable, Debug, Clone)]
#[diesel(table_name = crate::schema::action)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub(crate) struct Action {
    pub id: i32,
    pub source_application: String,
    pub action_name: String,
    pub payload: String,
    pub timestamp: i64,
}

#[derive(Insertable, Debug)]
#[diesel(table_name = crate::schema::action)]
pub(crate) struct NewAction {
    pub source_application: String,
    pub action_name: String,
    pub payload: String,
    pub timestamp: i64,
}

#[derive(AsChangeset, Default, Debug)]
#[diesel(table_name = crate::schema::action)]
pub(crate) struct ActionChangeset {
    pub source_application: Option<String>,
    pub action_name: Option<String>,
    pub payload: Option<String>,
    pub timestamp: Option<i64>,
}

/// What clients send and receive: `payload` is real JSON, not a string.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct ActionView {
    pub id: i32,
    pub source_application: String,
    pub action_name: String,
    pub payload: Value,
    /// Unix epoch milliseconds.
    pub timestamp: i64,
}

impl From<Action> for ActionView {
    fn from(a: Action) -> Self {
        // Payloads are validated on the way in; fall back to a string if a row
        // was written some other way.
        let payload = serde_json::from_str(&a.payload).unwrap_or(Value::String(a.payload));
        Self {
            id: a.id,
            source_application: a.source_application,
            action_name: a.action_name,
            payload,
            timestamp: a.timestamp,
        }
    }
}

#[derive(Deserialize, Debug)]
pub(crate) struct CreateAction {
    pub source_application: String,
    pub action_name: String,
    #[serde(default)]
    pub payload: Value,
    /// Defaults to now.
    pub timestamp: Option<i64>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct UpdateAction {
    pub source_application: Option<String>,
    pub action_name: Option<String>,
    pub payload: Option<Value>,
    pub timestamp: Option<i64>,
}

// ---------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------

pub(crate) const STATUSES: [&str; 3] = ["Available", "In-Progress", "Complete"];

pub(crate) fn validate_status(s: &str) -> ApiResult<()> {
    if STATUSES.contains(&s) {
        Ok(())
    } else {
        Err(ApiError::BadRequest(format!(
            "status must be one of {}",
            STATUSES.join(", ")
        )))
    }
}

#[derive(Queryable, Selectable, Serialize, Debug, Clone)]
#[diesel(table_name = crate::schema::tasks)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub(crate) struct Task {
    pub id: i32,
    pub title: String,
    pub status: String,
    pub task_description_path: String,
    pub progress_log_path: String,
    pub model: String,
    pub taken_by_pid: i32,
    pub taken_by_start: i32,
}

#[derive(Insertable, Debug)]
#[diesel(table_name = crate::schema::tasks)]
pub(crate) struct NewTask {
    pub title: String,
    pub status: String,
    pub task_description_path: String,
    pub progress_log_path: String,
    pub model: String,
}

#[derive(AsChangeset, Deserialize, Default, Debug)]
#[diesel(table_name = crate::schema::tasks)]
pub(crate) struct UpdateTask {
    pub title: Option<String>,
    pub status: Option<String>,
    pub model: Option<String>,
    pub taken_by_pid: Option<i32>,
    pub taken_by_start: Option<i32>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct CreateTask {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub model: String,
}
