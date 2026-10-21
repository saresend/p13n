//! The JSON Schema from `POST /init`. It describes valid `{ action_name, payload }` objects,
//! for both incoming actions and the model's proposals.

use jsonschema::Validator;
use serde_json::Value;

pub struct Vocabulary {
    pub schema: Value,
    validator: Validator,
}

impl Vocabulary {
    pub fn new(schema: Value) -> Result<Self, String> {
        let validator = jsonschema::validator_for(&schema).map_err(|error| error.to_string())?;
        Ok(Self { schema, validator })
    }

    /// Every way `instance` fails the schema; empty when it's valid.
    pub fn errors(&self, instance: &Value) -> Vec<String> {
        self.validator
            .iter_errors(instance)
            .map(|error| {
                let path = error.instance_path().to_string();
                if path.is_empty() {
                    error.to_string()
                } else {
                    format!("{path}: {error}")
                }
            })
            .collect()
    }

    pub fn accepts(&self, instance: &Value) -> bool {
        self.validator.is_valid(instance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_against_the_schema() {
        let vocabulary = Vocabulary::new(json!({
            "type": "object",
            "required": ["action_name"],
            "properties": { "action_name": { "enum": ["save"] } }
        }))
        .unwrap();
        assert!(vocabulary.accepts(&json!({ "action_name": "save", "payload": {} })));
        let errors = vocabulary.errors(&json!({ "action_name": "explode" }));
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("/action_name"), "{errors:?}");
    }

    #[test]
    fn rejects_invalid_schemas() {
        assert!(Vocabulary::new(json!({ "type": "not-a-type" })).is_err());
    }
}
