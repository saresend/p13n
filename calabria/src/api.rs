//! Thin client for the dsys HTTP API: the `/fs` file tree that stores notes,
//! and `/actions`, which records what the user does.

use gloo_net::http::{Request, Response};
use serde::Deserialize;
use serde_json::{Value, json};

pub const SOURCE_APPLICATION: &str = "calabria";

#[derive(Deserialize, Clone, Debug)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub children: Option<Vec<FsEntry>>,
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
    check(Request::get(&url(base, "/health")).send().await).await.map(|_| ())
}

/// The whole tree, with file contents inlined.
pub async fn tree(base: &str) -> ApiResult<FsEntry> {
    let resp = check(Request::get(&url(base, "/fs/tree?include_content=true")).send().await).await?;
    resp.json().await.map_err(|e| ApiError::Network(e.to_string()))
}

pub async fn write_file(base: &str, path: &str, body: &str, create_only: bool) -> ApiResult<()> {
    let mut req = Request::put(&url(base, &format!("/fs/file?path={}", enc(path))))
        .header("Content-Type", "text/markdown; charset=utf-8");
    if create_only {
        req = req.header("If-None-Match", "*");
    }
    send(req.body(body.to_string())).await.map(|_| ())
}

pub async fn create_dir(base: &str, path: &str) -> ApiResult<()> {
    check(Request::post(&url(base, &format!("/fs/dir?path={}", enc(path)))).send().await).await.map(|_| ())
}

pub async fn move_entry(base: &str, from: &str, to: &str) -> ApiResult<()> {
    send(Request::post(&url(base, "/fs/move")).json(&json!({ "from": from, "to": to }))).await.map(|_| ())
}

pub async fn delete_file(base: &str, path: &str) -> ApiResult<()> {
    check(Request::delete(&url(base, &format!("/fs/file?path={}", enc(path)))).send().await).await.map(|_| ())
}

pub async fn delete_dir(base: &str, path: &str) -> ApiResult<()> {
    let u = url(base, &format!("/fs/dir?path={}&recursive=true", enc(path)));
    check(Request::delete(&u).send().await).await.map(|_| ())
}

pub async fn record_action(base: &str, action_name: &str, payload: &Value, timestamp: i64) -> ApiResult<()> {
    let body = json!({
        "source_application": SOURCE_APPLICATION,
        "action_name": action_name,
        "payload": payload,
        "timestamp": timestamp,
    });
    send(Request::post(&url(base, "/actions")).json(&body)).await.map(|_| ())
}
