//! Thin client for the dsys HTTP API: the `/fs` file tree that stores notes,
//! and `/actions`, which records what the user does.

use gloo_net::http::{Request, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::demo::{self, is_demo};

pub const SOURCE_APPLICATION: &str = "calabria";

#[derive(Deserialize, Clone, Debug)]
pub struct FsEntry {
    pub name: String,
    pub kind: String,
    /// Unix epoch milliseconds.
    #[serde(default)]
    pub modified_ms: i64,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub children: Option<Vec<FsEntry>>,
}

/// A row from `/actions`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ActionRecord {
    pub id: i64,
    pub source_application: String,
    pub action_name: String,
    #[serde(default)]
    pub payload: Value,
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
pub enum ApiError {
    /// The request never got an HTTP response (server down, CORS, DNS, ...).
    Network(String),
    Status(u16, String),
}

impl ApiError {
    /// Worth trying again later, as opposed to a request the server rejected.
    pub fn is_retryable(&self) -> bool {
        match self {
            ApiError::Network(_) => true,
            ApiError::Status(code, _) => *code >= 500 || *code == 408 || *code == 429,
        }
    }

    pub fn message(&self) -> String {
        match self {
            ApiError::Network(detail) => format!("can't reach the server ({detail})"),
            ApiError::Status(code, msg) => format!("{msg} ({code})"),
        }
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn url(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/'))
}

fn enc(s: &str) -> String {
    js_sys::encode_uri_component(s).into()
}

async fn check(resp: Result<Response, gloo_net::Error>) -> ApiResult<Response> {
    let resp = resp.map_err(|e| ApiError::Network(e.to_string()))?;
    if resp.ok() {
        return Ok(resp);
    }
    let status = resp.status();
    let msg = match resp.json::<Value>().await {
        Ok(v) => v["error"].as_str().map(str::to_string),
        Err(_) => None,
    };
    Err(ApiError::Status(status, msg.unwrap_or_else(|| resp.status_text())))
}

async fn send(req: Result<Request, gloo_net::Error>) -> ApiResult<Response> {
    let req = req.map_err(|e| ApiError::Network(e.to_string()))?;
    check(req.send().await).await
}

pub async fn health(base: &str) -> ApiResult<()> {
    if is_demo(base) {
        return Ok(());
    }
    check(Request::get(&url(base, "/health")).send().await).await.map(|_| ())
}

/// The whole tree, with file contents inlined.
pub async fn tree(base: &str) -> ApiResult<FsEntry> {
    if is_demo(base) {
        return demo::tree();
    }
    let resp = check(Request::get(&url(base, "/fs/tree?include_content=true")).send().await).await?;
    resp.json().await.map_err(|e| ApiError::Network(e.to_string()))
}

/// A file's text, or `None` if it doesn't exist.
pub async fn read_file(base: &str, path: &str) -> ApiResult<Option<String>> {
    if is_demo(base) {
        return demo::read_file(path);
    }
    let resp = Request::get(&url(base, &format!("/fs/file?path={}", enc(path)))).send().await;
    match check(resp).await {
        Ok(r) => r.text().await.map(Some).map_err(|e| ApiError::Network(e.to_string())),
        Err(ApiError::Status(404, _)) => Ok(None),
        Err(e) => Err(e),
    }
}

pub async fn write_file(base: &str, path: &str, body: &str, create_only: bool) -> ApiResult<()> {
    if is_demo(base) {
        return demo::write_file(path, body, create_only);
    }
    let mut req = Request::put(&url(base, &format!("/fs/file?path={}", enc(path))))
        .header("Content-Type", "text/markdown; charset=utf-8");
    if create_only {
        req = req.header("If-None-Match", "*");
    }
    send(req.body(body.to_string())).await.map(|_| ())
}

pub async fn move_entry(base: &str, from: &str, to: &str) -> ApiResult<()> {
    if is_demo(base) {
        return demo::move_entry(from, to);
    }
    send(Request::post(&url(base, "/fs/move")).json(&json!({ "from": from, "to": to }))).await.map(|_| ())
}

pub async fn delete_file(base: &str, path: &str) -> ApiResult<()> {
    if is_demo(base) {
        return demo::delete_file(path);
    }
    check(Request::delete(&url(base, &format!("/fs/file?path={}", enc(path)))).send().await).await.map(|_| ())
}

pub async fn record_action(base: &str, action_name: &str, payload: &Value, timestamp: i64) -> ApiResult<()> {
    if is_demo(base) {
        return demo::record_action(action_name, payload, timestamp);
    }
    let body = json!({
        "source_application": SOURCE_APPLICATION,
        "action_name": action_name,
        "payload": payload,
        "timestamp": timestamp,
    });
    send(Request::post(&url(base, "/actions")).json(&body)).await.map(|_| ())
}

/// The most recent actions this app recorded, newest first.
pub async fn recent_actions(base: &str, limit: usize) -> ApiResult<Vec<ActionRecord>> {
    if is_demo(base) {
        return demo::recent_actions(limit);
    }
    let u = url(base, &format!("/actions?source_application={}&limit={limit}", enc(SOURCE_APPLICATION)));
    let resp = check(Request::get(&u).send().await).await?;
    resp.json().await.map_err(|e| ApiError::Network(e.to_string()))
}

/// Create a dsys task; returns its id.
pub async fn create_task(base: &str, title: &str, description: &str) -> ApiResult<i64> {
    if is_demo(base) {
        return demo::create_task(title, description);
    }
    let body = json!({ "title": title, "description": description });
    let resp = send(Request::post(&url(base, "/tasks")).json(&body)).await?;
    let task: Value = resp.json().await.map_err(|e| ApiError::Network(e.to_string()))?;
    task["id"].as_i64().ok_or_else(|| ApiError::Status(500, "the server didn't return a task id".into()))
}

/// A task's status (Available, In-Progress, Complete), or `None` if it's gone.
pub async fn task_status(base: &str, id: i64) -> ApiResult<Option<String>> {
    if is_demo(base) {
        return demo::task_status(id);
    }
    match check(Request::get(&url(base, &format!("/tasks/{id}"))).send().await).await {
        Ok(resp) => {
            let task: Value = resp.json().await.map_err(|e| ApiError::Network(e.to_string()))?;
            Ok(task["status"].as_str().map(str::to_string))
        }
        Err(ApiError::Status(404, _)) => Ok(None),
        Err(e) => Err(e),
    }
}
