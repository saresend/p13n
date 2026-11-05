//! Saved login: the dsys server URL and the exe.dev token.
//!
//! Stored as JSON at `<config dir>/dsys-cli/config.json` (on Linux
//! `~/.config/dsys-cli/config.json`), readable only by the owner.
//! `DSYS_CLI_CONFIG` overrides the location.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_SERVER: &str = "http://127.0.0.1:8081";

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

pub fn path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("DSYS_CLI_CONFIG") {
        return Ok(PathBuf::from(p));
    }
    let dir = dirs::config_dir().context("can't find a config directory for this user; set DSYS_CLI_CONFIG")?;
    Ok(dir.join("dsys-cli").join("config.json"))
}

pub fn load() -> Result<Config> {
    let p = path()?;
    match fs::read_to_string(&p) {
        Ok(s) => serde_json::from_str(&s).with_context(|| format!("{} is not valid config JSON", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", p.display())),
    }
}

/// Write the config, readable and writable by the owner only.
pub fn save(cfg: &Config) -> Result<PathBuf> {
    let p = path()?;
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(cfg)? + "\n";
    write_private(&p, json.as_bytes()).with_context(|| format!("writing {}", p.display()))?;
    Ok(p)
}

#[cfg(unix)]
fn write_private(p: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(p)?;
    // Tighten an existing file too (mode() only applies on creation).
    f.set_permissions(fs::Permissions::from_mode(0o600))?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(p: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::write(p, bytes)
}

/// Normalize a server address: trim, drop trailing `/`, default to https
/// for remote hosts and http for localhost.
pub fn normalize_server(raw: &str) -> String {
    let s = raw.trim().trim_end_matches('/');
    if s.contains("://") {
        return s.to_string();
    }
    let host = s.split(['/', ':']).next().unwrap_or("");
    let local = matches!(host, "localhost" | "127.0.0.1" | "[") || host.starts_with("127.");
    format!("{}://{s}", if local { "http" } else { "https" })
}

/// Show a token without revealing it: first and last few characters.
pub fn mask(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 12 {
        return "•".repeat(chars.len().max(4));
    }
    let head: String = chars[..6].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_servers() {
        assert_eq!(normalize_server("myvm.exe.xyz/"), "https://myvm.exe.xyz");
        assert_eq!(normalize_server("127.0.0.1:8081"), "http://127.0.0.1:8081");
        assert_eq!(normalize_server("localhost:8081"), "http://localhost:8081");
        assert_eq!(normalize_server(" http://x.y:9 "), "http://x.y:9");
    }

    #[test]
    fn masks_tokens() {
        assert_eq!(mask("abcdefghijklmnopqrstuvwxyz"), "abcdef…wxyz");
        assert_eq!(mask("short"), "•••••");
    }

    #[test]
    fn saves_privately_and_loads() {
        let dir = std::env::temp_dir().join(format!("dsys-cli-test-{}", std::process::id()));
        let file = dir.join("config.json");
        // SAFETY: single-threaded within this test; other tests don't read it.
        unsafe { std::env::set_var("DSYS_CLI_CONFIG", &file) };
        let cfg = Config { server: Some("https://vm.exe.xyz".into()), token: Some("t0k".into()) };
        save(&cfg).unwrap();
        assert_eq!(load().unwrap(), cfg);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = fs::remove_dir_all(dir);
    }
}
