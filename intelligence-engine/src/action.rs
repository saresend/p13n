use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Who took an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    User,
    Agent,
    System,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::User => "user",
            Source::Agent => "agent",
            Source::System => "system",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Source::User),
            "agent" => Some(Source::Agent),
            "system" => Some(Source::System),
            _ => None,
        }
    }
}

/// One action taken in a client app (the body of `POST /action`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub user_id: String,
    pub session_id: String,
    /// e.g. `save`, `write_text`, `delete` for the notes app.
    pub action_name: String,
    /// Arbitrary, app-defined; checked against the vocabulary from `POST /init`.
    #[serde(default = "empty_object")]
    pub payload: Value,
    pub source: Source,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

impl Action {
    /// The part of an action the vocabulary describes: `{ action_name, payload }`.
    pub fn vocabulary_instance(&self) -> Value {
        ProposedAction::new(&self.action_name, self.payload.clone()).to_value()
    }
}

/// An action the model proposes; the server sends it with `source: "system"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposedAction {
    pub action_name: String,
    pub payload: Value,
}

impl ProposedAction {
    pub fn new(action_name: &str, payload: Value) -> Self {
        Self {
            action_name: action_name.to_owned(),
            payload,
        }
    }

    pub fn to_value(&self) -> Value {
        serde_json::json!({ "action_name": self.action_name, "payload": self.payload })
    }
}

/// An action inside a suggestion, as streamed over SSE.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuggestedAction {
    pub action_name: String,
    pub payload: Value,
    pub source: Source,
}

/// One message on `GET /sse/suggestion`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    /// In `[0.0, 1.0]`.
    pub confidence: f32,
    pub user_id: String,
    pub actions: Vec<SuggestedAction>,
}
