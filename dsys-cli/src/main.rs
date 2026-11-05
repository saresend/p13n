//! `dsys-cli`: a command-line client for the dsys API (actions, tasks, files).

mod client;
mod config;

use std::io::{IsTerminal, Read, Write};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use reqwest::Method;
use serde_json::{Map, Value, json};

use client::{Body, Client, query};

#[derive(Parser)]
#[command(
    name = "dsys-cli",
    version,
    about = "Command-line client for dsys: actions, tasks and files.",
    after_help = "Log in once with `dsys-cli auth <TOKEN> --server https://<vm>.exe.xyz`; \
                  every command after that uses the saved server and token."
)]
struct Cli {
    /// dsys server URL (default: the saved one, else http://127.0.0.1:8081)
    #[arg(long, global = true, env = "DSYS_URL")]
    server: Option<String>,
    /// exe.dev token for this command only (default: the saved one)
    #[arg(long, global = true, env = "DSYS_TOKEN", hide_env_values = true)]
    token: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Save the exe.dev token (and optionally the server) used for every request
    Auth(AuthArgs),
    /// Check that the server is up
    Health,
    /// Actions: the record of what you did, across applications
    #[command(subcommand)]
    Actions(ActionsCmd),
    /// Tasks, with their description and progress log files
    #[command(subcommand)]
    Tasks(TasksCmd),
    /// Files in the dsys file tree (where notes live)
    #[command(subcommand)]
    Files(FilesCmd),
}

#[derive(Args)]
struct AuthArgs {
    /// The token, from `ssh exe.dev ssh-key generate-api-key --vm <vm>`. Omit it, or pass `-`,
    /// to read it from stdin so it stays out of your shell history.
    token: Option<String>,
    /// Show the saved server and token, and check them against the server
    #[arg(long, conflicts_with_all = ["token", "logout"])]
    status: bool,
    /// Forget the saved token
    #[arg(long, conflicts_with = "token")]
    logout: bool,
}

#[derive(Subcommand)]
enum ActionsCmd {
    /// List actions, newest first
    List {
        #[arg(long, short = 's')]
        source_application: Option<String>,
        #[arg(long, short = 'n')]
        action_name: Option<String>,
        /// Only actions at or after this time (Unix epoch ms)
        #[arg(long)]
        since: Option<i64>,
        /// Only actions before this time (Unix epoch ms)
        #[arg(long)]
        until: Option<i64>,
        /// At most this many (1–1000, default 100)
        #[arg(long)]
        limit: Option<i64>,
        #[arg(long)]
        offset: Option<i64>,
    },
    /// Show one action
    Get { id: i64 },
    /// Record an action
    Create {
        #[arg(long, short = 's')]
        source_application: String,
        #[arg(long, short = 'n')]
        action_name: String,
        /// JSON payload, `@file` to read it from a file, or `-` for stdin
        #[arg(long, short = 'p')]
        payload: Option<String>,
        /// Unix epoch ms (default: now)
        #[arg(long)]
        timestamp: Option<i64>,
    },
    /// Change fields of an action
    Update {
        id: i64,
        #[arg(long, short = 's')]
        source_application: Option<String>,
        #[arg(long, short = 'n')]
        action_name: Option<String>,
        /// JSON payload, `@file`, or `-` for stdin
        #[arg(long, short = 'p')]
        payload: Option<String>,
        #[arg(long)]
        timestamp: Option<i64>,
    },
    /// Delete an action
    Delete { id: i64 },
}

#[derive(Subcommand)]
enum TasksCmd {
    /// List tasks
    List {
        /// Available, In-Progress or Complete
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        model: Option<String>,
    },
    /// Show one task
    Get { id: i64 },
    /// Create a task (and its description and progress log files)
    Create {
        #[arg(long, short = 't')]
        title: String,
        /// Description text
        #[arg(long, short = 'd', conflicts_with = "description_file")]
        description: Option<String>,
        /// Read the description from a file (`-` for stdin)
        #[arg(long)]
        description_file: Option<String>,
        #[arg(long)]
        model: Option<String>,
    },
    /// Change fields of a task
    Update {
        id: i64,
        #[arg(long, short = 't')]
        title: Option<String>,
        /// Available, In-Progress or Complete
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        taken_by_pid: Option<i32>,
        #[arg(long)]
        taken_by_start: Option<i32>,
        /// Only update if the task currently has this status (else 409); use to claim a task
        #[arg(long)]
        expected_status: Option<String>,
    },
    /// Delete a task and its files
    Delete {
        id: i64,
        /// Keep description.md and progress_log.md on disk
        #[arg(long)]
        keep_files: bool,
    },
    /// Print a task's description.md
    Description { id: i64 },
    /// Replace a task's description.md from a file (default: stdin)
    SetDescription { id: i64, file: Option<String> },
    /// Print a task's progress log
    Log { id: i64 },
    /// Append a dated entry to a task's progress log (text, or stdin)
    LogAppend { id: i64, text: Option<String> },
    /// Replace a task's progress log from a file (default: stdin)
    SetLog { id: i64, file: Option<String> },
}

#[derive(Subcommand)]
enum FilesCmd {
    /// List a directory recursively (default: the root)
    Tree {
        #[arg(default_value = "")]
        path: String,
        /// Include the contents of small text files
        #[arg(long)]
        content: bool,
        /// Include hidden files
        #[arg(long)]
        hidden: bool,
        /// Print an indented outline instead of JSON
        #[arg(long)]
        plain: bool,
    },
    /// Print a file
    Get { path: String },
    /// Write a file from a local file (default: stdin)
    Put {
        path: String,
        source: Option<String>,
        /// Fail if the file already exists
        #[arg(long, conflicts_with = "if_match")]
        create_only: bool,
        /// Only replace the file if its ETag still matches
        #[arg(long)]
        if_match: Option<String>,
    },
    /// Delete a file
    Rm {
        path: String,
        #[arg(long)]
        if_match: Option<String>,
    },
    /// Create a directory (and its parents)
    Mkdir { path: String },
    /// Delete a directory
    Rmdir {
        path: String,
        /// Also delete everything in it
        #[arg(long, short = 'r')]
        recursive: bool,
    },
    /// Move or rename a file or directory
    Mv { from: String, to: String },
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let saved = config::load()?;
    let server = config::normalize_server(
        cli.server.as_deref().or(saved.server.as_deref()).unwrap_or(config::DEFAULT_SERVER),
    );
    let token = cli.token.clone().or_else(|| saved.token.clone());

    match cli.cmd {
        Cmd::Auth(a) => auth(a, cli.server.as_deref(), saved),
        cmd => {
            let c = Client::new(&server, token)?;
            match cmd {
                Cmd::Health => print(c.send(c.request(Method::GET, "/health"))?),
                Cmd::Actions(a) => actions(&c, a),
                Cmd::Tasks(t) => tasks(&c, t),
                Cmd::Files(f) => files(&c, f),
                Cmd::Auth(_) => unreachable!(),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------------

fn auth(a: AuthArgs, server_flag: Option<&str>, mut saved: config::Config) -> Result<()> {
    if a.logout {
        saved.token = None;
        let p = config::save(&saved)?;
        eprintln!("Logged out. Removed the token from {}.", p.display());
        return Ok(());
    }
    if let Some(s) = server_flag {
        saved.server = Some(config::normalize_server(s));
    }
    if a.status {
        let server = saved.server.clone().unwrap_or_else(|| config::DEFAULT_SERVER.into());
        println!("config: {}", config::path()?.display());
        println!("server: {server}{}", if saved.server.is_none() { " (default)" } else { "" });
        println!("token:  {}", saved.token.as_deref().map_or("none".into(), config::mask));
        let c = Client::new(&server, saved.token.clone())?;
        match c.send(c.request(Method::GET, "/health")) {
            Ok(_) => println!("check:  ok, the server accepted the request"),
            Err(e) => println!("check:  failed: {e:#}"),
        }
        return Ok(());
    }

    let token = match a.token.as_deref() {
        Some("-") | None => read_token()?,
        Some(t) => t.trim().to_string(),
    };
    if token.is_empty() {
        bail!("no token given");
    }
    saved.token = Some(token.clone());
    let p = config::save(&saved)?;
    let server = saved.server.clone().unwrap_or_else(|| config::DEFAULT_SERVER.into());
    eprintln!("Saved token {} for {server} to {}.", config::mask(&token), p.display());

    let c = Client::new(&server, Some(token))?;
    match c.send(c.request(Method::GET, "/health")) {
        Ok(_) => eprintln!("Checked: {server} accepted the token."),
        Err(e) => eprintln!("Warning: couldn't confirm the token works yet: {e:#}"),
    }
    Ok(())
}

fn read_token() -> Result<String> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        eprint!("Paste your exe.dev token and press Enter: ");
        std::io::stderr().flush().ok();
        let mut line = String::new();
        stdin.read_line(&mut line)?;
        Ok(line.trim().to_string())
    } else {
        let mut s = String::new();
        stdin.read_to_string(&mut s)?;
        Ok(s.trim().to_string())
    }
}

// ---------------------------------------------------------------------------
// actions
// ---------------------------------------------------------------------------

fn actions(c: &Client, cmd: ActionsCmd) -> Result<()> {
    match cmd {
        ActionsCmd::List { source_application, action_name, since, until, limit, offset } => {
            let q = query(&[
                ("source_application", source_application),
                ("action_name", action_name),
                ("since", since.map(|v| v.to_string())),
                ("until", until.map(|v| v.to_string())),
                ("limit", limit.map(|v| v.to_string())),
                ("offset", offset.map(|v| v.to_string())),
            ]);
            print(c.send(c.request(Method::GET, &format!("/actions{q}")))?)
        }
        ActionsCmd::Get { id } => print(c.send(c.request(Method::GET, &format!("/actions/{id}")))?),
        ActionsCmd::Create { source_application, action_name, payload, timestamp } => {
            let mut body = json!({ "source_application": source_application, "action_name": action_name });
            if let Some(p) = payload {
                body["payload"] = parse_payload(&p)?;
            }
            if let Some(t) = timestamp {
                body["timestamp"] = json!(t);
            }
            print(c.send(c.request(Method::POST, "/actions").json(&body))?)
        }
        ActionsCmd::Update { id, source_application, action_name, payload, timestamp } => {
            let mut body = Map::new();
            if let Some(v) = source_application {
                body.insert("source_application".into(), json!(v));
            }
            if let Some(v) = action_name {
                body.insert("action_name".into(), json!(v));
            }
            if let Some(p) = payload {
                body.insert("payload".into(), parse_payload(&p)?);
            }
            if let Some(t) = timestamp {
                body.insert("timestamp".into(), json!(t));
            }
            if body.is_empty() {
                bail!("nothing to update; pass at least one of --source-application, --action-name, --payload, --timestamp");
            }
            print(c.send(c.request(Method::PATCH, &format!("/actions/{id}")).json(&body))?)
        }
        ActionsCmd::Delete { id } => {
            c.send(c.request(Method::DELETE, &format!("/actions/{id}")))?;
            eprintln!("Deleted action {id}.");
            Ok(())
        }
    }
}

/// `--payload` value: inline JSON, `@file`, or `-` for stdin.
fn parse_payload(arg: &str) -> Result<Value> {
    let text = if arg == "-" {
        read_stdin()?
    } else if let Some(path) = arg.strip_prefix('@') {
        std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?
    } else {
        arg.to_string()
    };
    serde_json::from_str(&text).context("--payload must be JSON (wrap a plain string in quotes: '\"text\"')")
}

// ---------------------------------------------------------------------------
// tasks
// ---------------------------------------------------------------------------

fn tasks(c: &Client, cmd: TasksCmd) -> Result<()> {
    match cmd {
        TasksCmd::List { status, model } => {
            let q = query(&[("status", status), ("model", model)]);
            print(c.send(c.request(Method::GET, &format!("/tasks{q}")))?)
        }
        TasksCmd::Get { id } => print(c.send(c.request(Method::GET, &format!("/tasks/{id}")))?),
        TasksCmd::Create { title, description, description_file, model } => {
            let description = match (description, description_file) {
                (Some(d), _) => Some(d),
                (None, Some(f)) => Some(read_source(Some(&f))?),
                (None, None) => None,
            };
            let mut body = json!({ "title": title });
            if let Some(d) = description {
                body["description"] = json!(d);
            }
            if let Some(m) = model {
                body["model"] = json!(m);
            }
            print(c.send(c.request(Method::POST, "/tasks").json(&body))?)
        }
        TasksCmd::Update { id, title, status, model, taken_by_pid, taken_by_start, expected_status } => {
            let mut body = Map::new();
            if let Some(v) = title {
                body.insert("title".into(), json!(v));
            }
            if let Some(v) = status {
                body.insert("status".into(), json!(v));
            }
            if let Some(v) = model {
                body.insert("model".into(), json!(v));
            }
            if let Some(v) = taken_by_pid {
                body.insert("taken_by_pid".into(), json!(v));
            }
            if let Some(v) = taken_by_start {
                body.insert("taken_by_start".into(), json!(v));
            }
            if body.is_empty() {
                bail!("nothing to update; pass at least one field such as --status or --title");
            }
            let q = query(&[("expected_status", expected_status)]);
            print(c.send(c.request(Method::PATCH, &format!("/tasks/{id}{q}")).json(&body))?)
        }
        TasksCmd::Delete { id, keep_files } => {
            let q = query(&[("keep_files", keep_files.then(|| "true".to_string()))]);
            c.send(c.request(Method::DELETE, &format!("/tasks/{id}{q}")))?;
            eprintln!("Deleted task {id}{}.", if keep_files { " (kept its files)" } else { "" });
            Ok(())
        }
        TasksCmd::Description { id } => print(c.send(c.request(Method::GET, &format!("/tasks/{id}/description")))?),
        TasksCmd::SetDescription { id, file } => {
            let text = read_source(file.as_deref())?;
            c.send(c.request(Method::PUT, &format!("/tasks/{id}/description")).body(text))?;
            eprintln!("Updated the description of task {id}.");
            Ok(())
        }
        TasksCmd::Log { id } => print(c.send(c.request(Method::GET, &format!("/tasks/{id}/log")))?),
        TasksCmd::LogAppend { id, text } => {
            let text = match text {
                Some(t) => t,
                None => read_stdin()?,
            };
            c.send(c.request(Method::POST, &format!("/tasks/{id}/log")).body(text))?;
            eprintln!("Added a log entry to task {id}.");
            Ok(())
        }
        TasksCmd::SetLog { id, file } => {
            let text = read_source(file.as_deref())?;
            c.send(c.request(Method::PUT, &format!("/tasks/{id}/log")).body(text))?;
            eprintln!("Replaced the progress log of task {id}.");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// files
// ---------------------------------------------------------------------------

fn files(c: &Client, cmd: FilesCmd) -> Result<()> {
    match cmd {
        FilesCmd::Tree { path, content, hidden, plain } => {
            let q = query(&[
                ("path", (!path.is_empty()).then_some(path)),
                ("include_content", content.then(|| "true".to_string())),
                ("include_hidden", hidden.then(|| "true".to_string())),
            ]);
            let body = c.send(c.request(Method::GET, &format!("/fs/tree{q}")))?;
            match (plain, body) {
                (true, Body::Json(v)) => {
                    let mut out = String::new();
                    outline(&v, 0, &mut out);
                    print!("{out}");
                    Ok(())
                }
                (_, b) => print(b),
            }
        }
        FilesCmd::Get { path } => {
            // Raw bytes, so binary files come through intact.
            let resp = c
                .request(Method::GET, &format!("/fs/file{}", query(&[("path", Some(path))])))
                .send()
                .with_context(|| format!("can't reach dsys at {}", c.base()))?;
            if !resp.status().is_success() {
                return error_from(resp).map(|_| ());
            }
            let bytes = resp.bytes()?;
            std::io::stdout().write_all(&bytes)?;
            Ok(())
        }
        FilesCmd::Put { path, source, create_only, if_match } => {
            let bytes = match source.as_deref() {
                None | Some("-") => {
                    let mut b = Vec::new();
                    std::io::stdin().read_to_end(&mut b)?;
                    b
                }
                Some(f) => std::fs::read(f).with_context(|| format!("reading {f}"))?,
            };
            let mut req = c.request(Method::PUT, &format!("/fs/file{}", query(&[("path", Some(path))]))).body(bytes);
            if create_only {
                req = req.header("If-None-Match", "*");
            }
            if let Some(etag) = if_match {
                req = req.header("If-Match", etag);
            }
            print(c.send(req)?)
        }
        FilesCmd::Rm { path, if_match } => {
            let mut req = c.request(Method::DELETE, &format!("/fs/file{}", query(&[("path", Some(path.clone()))])));
            if let Some(etag) = if_match {
                req = req.header("If-Match", etag);
            }
            c.send(req)?;
            eprintln!("Deleted {path}.");
            Ok(())
        }
        FilesCmd::Mkdir { path } => {
            print(c.send(c.request(Method::POST, &format!("/fs/dir{}", query(&[("path", Some(path))]))))?)
        }
        FilesCmd::Rmdir { path, recursive } => {
            let q = query(&[("path", Some(path.clone())), ("recursive", recursive.then(|| "true".to_string()))]);
            c.send(c.request(Method::DELETE, &format!("/fs/dir{q}")))?;
            eprintln!("Deleted {path}{}.", if recursive { " and everything in it" } else { "" });
            Ok(())
        }
        FilesCmd::Mv { from, to } => {
            print(c.send(c.request(Method::POST, "/fs/move").json(&json!({ "from": from, "to": to })))?)
        }
    }
}

fn error_from(resp: reqwest::blocking::Response) -> Result<Body> {
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    let msg = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or(text);
    bail!("{} {}: {}", status.as_u16(), status.canonical_reason().unwrap_or(""), msg.trim())
}

/// An indented outline of a `/fs/tree` listing: directories end in `/`.
fn outline(entry: &Value, depth: usize, out: &mut String) {
    for child in entry["children"].as_array().into_iter().flatten() {
        let name = child["name"].as_str().unwrap_or("?");
        let is_dir = child["kind"] == "dir";
        out.push_str(&"  ".repeat(depth));
        out.push_str(name);
        if is_dir {
            out.push('/');
        } else if let Some(size) = child["size"].as_u64() {
            out.push_str(&format!("  ({size} B)"));
        }
        out.push('\n');
        if is_dir {
            outline(child, depth + 1, out);
        }
    }
}

// ---------------------------------------------------------------------------
// io helpers
// ---------------------------------------------------------------------------

fn read_stdin() -> Result<String> {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).context("reading stdin")?;
    Ok(s)
}

/// Text from a file path, or stdin for `None` / `-`.
fn read_source(src: Option<&str>) -> Result<String> {
    match src {
        None | Some("-") => read_stdin(),
        Some(f) => std::fs::read_to_string(f).with_context(|| format!("reading {f}")),
    }
}

fn print(body: Body) -> Result<()> {
    match body {
        Body::Json(v) => println!("{}", serde_json::to_string_pretty(&v)?),
        Body::Text(t) => {
            print!("{t}");
            if !t.ends_with('\n') {
                println!();
            }
        }
        Body::Empty => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn payloads_must_be_json() {
        assert_eq!(parse_payload(r#"{"a":1}"#).unwrap(), json!({"a": 1}));
        assert_eq!(parse_payload(r#""text""#).unwrap(), json!("text"));
        assert!(parse_payload("text").is_err());
    }

    #[test]
    fn outlines_trees() {
        let tree = json!({"children": [
            {"name": "Archive", "kind": "dir", "children": [{"name": "a.md", "kind": "file", "size": 3}]},
            {"name": "b.md", "kind": "file", "size": 10}
        ]});
        let mut out = String::new();
        outline(&tree, 0, &mut out);
        assert_eq!(out, "Archive/\n  a.md  (3 B)\nb.md  (10 B)\n");
    }
}
