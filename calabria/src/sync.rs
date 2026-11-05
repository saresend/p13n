//! An ordered queue of server operations. The UI updates immediately and
//! pushes operations here; one worker sends them in order, so a create is
//! always sent before the rename or write that follows it. Requests that
//! can't reach the server are retried with backoff; requests the server
//! rejects are dropped and reported.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gloo_timers::future::TimeoutFuture;
use serde_json::Value;
use wasm_bindgen_futures::spawn_local;
use yew::Callback;

use crate::api::{self, ApiError};

#[derive(Clone, Debug)]
pub enum Op {
    Write { path: String, body: String, create_only: bool },
    /// With `if_exists`, a missing source is not an error (used for a
    /// note's optional progress report).
    Move { from: String, to: String, if_exists: bool },
    DeleteFile { path: String, if_exists: bool },
    Record { action_name: &'static str, payload: Value, timestamp: i64 },
}

impl Op {
    async fn perform(&self, base: &str) -> Result<(), ApiError> {
        match self {
            Op::Write { path, body, create_only } => api::write_file(base, path, body, *create_only).await,
            Op::Move { from, to, if_exists } => tolerate_missing(api::move_entry(base, from, to).await, *if_exists),
            Op::DeleteFile { path, if_exists } => tolerate_missing(api::delete_file(base, path).await, *if_exists),
            Op::Record { action_name, payload, timestamp } => {
                api::record_action(base, action_name, payload, *timestamp).await
            }
        }
    }

    fn describe(&self) -> String {
        match self {
            Op::Write { path, .. } => format!("Couldn't save {path}"),
            Op::Move { from, to, .. } => format!("Couldn't move {from} to {to}"),
            Op::DeleteFile { path, .. } => format!("Couldn't delete {path}"),
            Op::Record { action_name, .. } => format!("Couldn't record {action_name}"),
        }
    }
}

fn tolerate_missing(r: Result<(), ApiError>, if_exists: bool) -> Result<(), ApiError> {
    match r {
        Err(ApiError::Status(404, _)) if if_exists => Ok(()),
        other => other,
    }
}

/// What the worker tells the UI after each step.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncStatus {
    pub pending: usize,
    /// Last failure; cleared by the next success.
    pub error: Option<String>,
    /// The server is unreachable and the head of the queue is being retried.
    pub retrying: bool,
}

#[derive(Default)]
struct Inner {
    queue: VecDeque<Op>,
    running: bool,
    /// Bumped by `reset`; a worker from an older epoch stops.
    epoch: u64,
}

#[derive(Clone, Default)]
pub struct Outbox(Rc<RefCell<Inner>>);

impl PartialEq for Outbox {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Outbox {
    pub fn push(&self, op: Op) {
        self.0.borrow_mut().queue.push_back(op);
    }

    pub fn len(&self) -> usize {
        self.0.borrow().queue.len()
    }

    /// Drop everything queued (used when switching servers).
    pub fn reset(&self) {
        let mut inner = self.0.borrow_mut();
        inner.queue.clear();
        inner.epoch += 1;
    }

    /// Start the worker if there is work and it isn't already running.
    pub fn run(&self, base: String, report: Callback<SyncStatus>) {
        let epoch = {
            let mut inner = self.0.borrow_mut();
            if inner.running || inner.queue.is_empty() {
                return;
            }
            inner.running = true;
            inner.epoch
        };
        report.emit(SyncStatus { pending: self.len(), ..Default::default() });
        let this = self.clone();
        spawn_local(async move {
            let mut error = None;
            let mut backoff_ms = 1_000;
            loop {
                let op = {
                    let inner = this.0.borrow();
                    if inner.epoch != epoch {
                        break;
                    }
                    match inner.queue.front() {
                        Some(op) => op.clone(),
                        None => break,
                    }
                };
                let result = op.perform(&base).await;
                if this.0.borrow().epoch != epoch {
                    break;
                }
                match result {
                    Ok(()) => {
                        this.0.borrow_mut().queue.pop_front();
                        error = None;
                        backoff_ms = 1_000;
                    }
                    Err(e) if e.is_retryable() => {
                        report.emit(SyncStatus {
                            pending: this.len(),
                            error: Some(format!("{} — {}; retrying", op.describe(), e.message())),
                            retrying: true,
                        });
                        TimeoutFuture::new(backoff_ms).await;
                        backoff_ms = (backoff_ms * 2).min(30_000);
                        continue;
                    }
                    Err(e) => {
                        this.0.borrow_mut().queue.pop_front();
                        error = Some(format!("{}: {}", op.describe(), e.message()));
                    }
                }
                report.emit(SyncStatus { pending: this.len(), error: error.clone(), retrying: false });
            }
            this.0.borrow_mut().running = false;
            let pending = if this.0.borrow().epoch == epoch { this.len() } else { 0 };
            report.emit(SyncStatus { pending, error, retrying: false });
        });
    }
}
