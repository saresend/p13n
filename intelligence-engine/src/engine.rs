//! Shared server state and the per-user worker threads.
//!
//! The first action from a new `user_id` starts a thread for that user. Every `tick` it checks
//! for at least `min_new_actions` actions newer than the last one it processed; if there are,
//! it asks the model for a suggestion and pushes it onto the user's queue for SSE.

use crate::action::{Action, Source, SuggestedAction, Suggestion};
use crate::db;
use crate::model::DecisionModel;
use crate::queue::SuggestionQueue;
use crate::vocabulary::Vocabulary;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub db_path: PathBuf,
    /// How often each user's worker checks for new actions.
    pub tick: Duration,
    /// New actions needed before the model runs again.
    pub min_new_actions: usize,
    /// Most actions passed to one `predict` call (the newest ones).
    pub max_window: usize,
    pub queue_capacity: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_path: PathBuf::from("data/engine.sqlite"),
            tick: Duration::from_secs(5),
            min_new_actions: 5,
            max_window: 100,
            queue_capacity: 32,
        }
    }
}

#[derive(Debug)]
pub enum EngineError {
    /// `POST /action` before any `POST /init`.
    NotInitialized,
    InvalidSchema(String),
    InvalidAction(Vec<String>),
    Db(rusqlite::Error),
}

impl From<rusqlite::Error> for EngineError {
    fn from(error: rusqlite::Error) -> Self {
        EngineError::Db(error)
    }
}

pub struct Engine {
    pub config: Config,
    db: Mutex<Connection>,
    model: Arc<dyn DecisionModel>,
    vocabulary: RwLock<Option<Arc<Vocabulary>>>,
    queues: Mutex<HashMap<String, Arc<SuggestionQueue>>>,
    workers: Mutex<HashSet<String>>,
    stopping: AtomicBool,
}

impl Engine {
    /// Opens the database and restores the most recent vocabulary, if any.
    pub fn open(config: Config, model: Arc<dyn DecisionModel>) -> Result<Arc<Self>, EngineError> {
        let conn = db::open(&config.db_path)?;
        let vocabulary = match db::latest_schema(&conn)? {
            Some(schema) => Some(Arc::new(
                Vocabulary::new(schema).map_err(EngineError::InvalidSchema)?,
            )),
            None => None,
        };
        Ok(Arc::new(Self {
            config,
            db: Mutex::new(conn),
            model,
            vocabulary: RwLock::new(vocabulary),
            queues: Mutex::default(),
            workers: Mutex::default(),
            stopping: AtomicBool::new(false),
        }))
    }

    /// `POST /init`: replace the vocabulary. Persisted, so it survives restarts.
    pub fn init(&self, schema: Value) -> Result<(), EngineError> {
        let vocabulary = Vocabulary::new(schema).map_err(EngineError::InvalidSchema)?;
        db::insert_schema(&self.db.lock().unwrap(), &vocabulary.schema)?;
        *self.vocabulary.write().unwrap() = Some(Arc::new(vocabulary));
        Ok(())
    }

    pub fn vocabulary(&self) -> Option<Arc<Vocabulary>> {
        self.vocabulary.read().unwrap().clone()
    }

    /// `POST /action`: validate, store, and make sure the user has a worker. Returns the row id.
    pub fn record(self: &Arc<Self>, action: &Action) -> Result<i64, EngineError> {
        let mut problems = Vec::new();
        for (field, value) in [
            ("user_id", &action.user_id),
            ("session_id", &action.session_id),
            ("action_name", &action.action_name),
        ] {
            if value.trim().is_empty() {
                problems.push(format!("{field} must not be empty"));
            }
        }
        let vocabulary = self.vocabulary().ok_or(EngineError::NotInitialized)?;
        problems.extend(vocabulary.errors(&action.vocabulary_instance()));
        if !problems.is_empty() {
            return Err(EngineError::InvalidAction(problems));
        }
        let id = db::insert_action(&self.db.lock().unwrap(), action)?;
        self.ensure_worker(&action.user_id);
        Ok(id)
    }

    /// The user's suggestion queue, created on first use (an SSE client may connect first).
    pub fn queue(&self, user_id: &str) -> Arc<SuggestionQueue> {
        let mut queues = self.queues.lock().unwrap();
        Arc::clone(
            queues
                .entry(user_id.to_owned())
                .or_insert_with(|| Arc::new(SuggestionQueue::new(self.config.queue_capacity))),
        )
    }

    /// Asks every worker thread to exit after its current tick.
    pub fn shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
    }

    fn ensure_worker(self: &Arc<Self>, user_id: &str) {
        if !self.workers.lock().unwrap().insert(user_id.to_owned()) {
            return;
        }
        let engine = Arc::clone(self);
        let user_id = user_id.to_owned();
        let spawned = std::thread::Builder::new()
            .name(format!("worker-{user_id}"))
            .spawn({
                let user_id = user_id.clone();
                move || engine.run_worker(&user_id)
            });
        if let Err(error) = spawned {
            log::error!("could not start worker for {user_id}: {error}");
            self.workers.lock().unwrap().remove(&user_id);
        }
    }

    fn run_worker(self: Arc<Self>, user_id: &str) {
        log::info!("worker started for {user_id}");
        let conn = match db::open(&self.config.db_path) {
            Ok(conn) => conn,
            Err(error) => {
                log::error!("worker for {user_id} could not open the database: {error}");
                self.workers.lock().unwrap().remove(user_id);
                return;
            }
        };
        let queue = self.queue(user_id);
        // Latest processed action id; None until the first suggestion run.
        let mut last_processed: Option<i64> = None;
        while !self.stopping.load(Ordering::SeqCst) {
            std::thread::sleep(self.config.tick);
            let Some(vocabulary) = self.vocabulary() else {
                continue;
            };
            match step(
                &conn,
                user_id,
                last_processed,
                &vocabulary,
                self.model.as_ref(),
                &self.config,
            ) {
                Ok(Step::Waiting) => {}
                Ok(Step::Processed { last, suggestion }) => {
                    last_processed = Some(last);
                    if let Some(suggestion) = suggestion {
                        queue.push(suggestion);
                    }
                }
                Err(error) => log::error!("worker for {user_id}: {error}"),
            }
        }
        log::info!("worker stopped for {user_id}");
    }
}

#[derive(Debug, PartialEq)]
pub enum Step {
    /// Fewer than `min_new_actions` new actions; nothing done.
    Waiting,
    /// The model ran over actions up to `last`. `suggestion` is None if it proposed nothing valid.
    Processed {
        last: i64,
        suggestion: Option<Suggestion>,
    },
}

/// One worker tick, separated from the thread so it can be tested directly.
pub fn step(
    conn: &Connection,
    user_id: &str,
    last_processed: Option<i64>,
    vocabulary: &Vocabulary,
    model: &dyn DecisionModel,
    config: &Config,
) -> rusqlite::Result<Step> {
    let after = last_processed.unwrap_or(0);
    if db::count_after(conn, user_id, after)? < config.min_new_actions.max(1) {
        return Ok(Step::Waiting);
    }
    let rows = db::latest_after(conn, user_id, after, config.max_window.max(1))?;
    let Some(&(last, _)) = rows.last() else {
        return Ok(Step::Waiting);
    };
    let actions: Vec<Action> = rows.into_iter().map(|(_, action)| action).collect();

    let (confidence, proposed) = model.predict(&vocabulary.schema, &actions);
    let actions: Vec<SuggestedAction> = proposed
        .into_iter()
        .filter(|action| {
            let ok = vocabulary.accepts(&action.to_value());
            if !ok {
                log::warn!(
                    "model proposed an action outside the vocabulary: {}",
                    action.action_name
                );
            }
            ok
        })
        .map(|action| SuggestedAction {
            action_name: action.action_name,
            payload: action.payload,
            source: Source::System,
        })
        .collect();
    let confidence = if confidence.is_finite() {
        confidence.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let suggestion = (!actions.is_empty()).then(|| Suggestion {
        confidence,
        user_id: user_id.to_owned(),
        actions,
    });
    Ok(Step::Processed { last, suggestion })
}
