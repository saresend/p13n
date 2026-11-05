//! HTTP client for the dsys API. When a token is set, every request carries
//! `X-Exedev-Authorization: Bearer <token>`, which the exe.dev proxy checks
//! and strips before the request reaches dsys.

use anyhow::{Context, Result, bail};
use reqwest::blocking::{Client as Http, RequestBuilder, Response};
use reqwest::{Method, StatusCode};
use serde_json::Value;

pub const AUTH_HEADER: &str = "X-Exedev-Authorization";

pub struct Client {
    base: String,
    token: Option<String>,
    http: Http,
}

/// What a request returned: JSON, text, or nothing (204).
pub enum Body {
    Json(Value),
    Text(String),
    Empty,
}

impl Client {
    pub fn new(base: &str, token: Option<String>) -> Result<Self> {
        let http = Http::builder()
            .user_agent(concat!("dsys-cli/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(60))
            .build()?;
        Ok(Self { base: base.trim_end_matches('/').to_string(), token, http })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let req = self.http.request(method, format!("{}{path}", self.base));
        match &self.token {
            Some(t) => req.header(AUTH_HEADER, format!("Bearer {t}")),
            None => req,
        }
    }

    /// Send and turn the response into a [`Body`], or an error that explains
    /// what went wrong.
    pub fn send(&self, req: RequestBuilder) -> Result<Body> {
        let resp = req.send().with_context(|| format!("can't reach dsys at {}", self.base))?;
        read(resp, self.token.is_some())
    }
}

fn read(resp: Response, had_token: bool) -> Result<Body> {
    let status = resp.status();
    let is_json = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/json"));
    let text = resp.text().context("reading the response")?;

    if !status.is_success() {
        let msg = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_string))
            .unwrap_or_else(|| text.trim().chars().take(300).collect());
        let hint = match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN if !had_token => {
                "\nhint: this server needs a token; run `dsys-cli auth <TOKEN>`"
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                "\nhint: the token was rejected; check it, or save a new one with `dsys-cli auth <TOKEN>`"
            }
            _ => "",
        };
        bail!("{} {}{}{hint}", status.as_u16(), status.canonical_reason().unwrap_or(""), fmt_msg(&msg));
    }
    if status == StatusCode::NO_CONTENT || text.is_empty() {
        return Ok(Body::Empty);
    }
    if is_json {
        return Ok(Body::Json(serde_json::from_str(&text).context("the server sent invalid JSON")?));
    }
    Ok(Body::Text(text))
}

fn fmt_msg(msg: &str) -> String {
    if msg.is_empty() { String::new() } else { format!(": {msg}") }
}

/// Percent-encode a query value.
pub fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Build `?a=1&b=x` from the pairs that have a value.
pub fn query(pairs: &[(&str, Option<String>)]) -> String {
    let parts: Vec<String> = pairs.iter().filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={}", enc(v)))).collect();
    if parts.is_empty() { String::new() } else { format!("?{}", parts.join("&")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_queries() {
        assert_eq!(query(&[("a", None)]), "");
        assert_eq!(
            query(&[("path", Some("My Notes/a b.md".into())), ("x", None), ("n", Some("5".into()))]),
            "?path=My%20Notes/a%20b.md&n=5"
        );
        assert_eq!(enc("a&b=c"), "a%26b%3Dc");
    }
}
