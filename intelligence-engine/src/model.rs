//! The decision model: given the vocabulary and a window of recent actions, propose the
//! actions the user is likely to take next, with a confidence. See MODEL.md for how a real
//! one could be built; for now there's only the identity stub.

use crate::action::{Action, ProposedAction};
use serde_json::Value;

pub trait DecisionModel: Send + Sync {
    /// `model.predict(json_schema, list[actions]) -> (confidence, list[actions])`.
    /// Proposed actions should match `schema`; the engine drops any that don't.
    fn predict(&self, schema: &Value, actions: &[Action]) -> (f32, Vec<ProposedAction>);
}

/// Stub: proposes exactly the actions it was given. Confidence is 0.0 because it hasn't
/// learned anything, so clients can tell its suggestions apart from a real model's.
pub struct IdentityModel;

impl DecisionModel for IdentityModel {
    fn predict(&self, _schema: &Value, actions: &[Action]) -> (f32, Vec<ProposedAction>) {
        let proposed = actions
            .iter()
            .map(|action| ProposedAction::new(&action.action_name, action.payload.clone()))
            .collect();
        (0.0, proposed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Source;
    use serde_json::json;

    #[test]
    fn identity_returns_its_input() {
        let action = |name: &str, payload| Action {
            user_id: "u".into(),
            session_id: "s".into(),
            action_name: name.into(),
            payload,
            source: Source::User,
        };
        let input = [
            action("create", json!({ "id": "a" })),
            action("save", json!({ "id": "a", "text": "hi" })),
        ];
        let (confidence, out) = IdentityModel.predict(&json!({}), &input);
        assert_eq!(confidence, 0.0);
        assert_eq!(
            out,
            vec![
                ProposedAction::new("create", json!({ "id": "a" })),
                ProposedAction::new("save", json!({ "id": "a", "text": "hi" })),
            ]
        );
    }
}
