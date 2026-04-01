//! Pipeline contract validation — lightweight JSON Schema checks.
//!
//! Phase 4.1: validates step input/output data against JSON Schema definitions.
//! This module lives in `domain` so `pipeline_service` can use it without
//! depending on `synapse_adapters`.

use serde_json::Value;

/// Validate a JSON value against a JSON Schema.
///
/// Returns `Ok(())` if valid, or a human-readable error string.
///
/// Uses the `jsonschema` crate for validation.
pub fn validate_schema(data: &Value, schema: &Value) -> Result<(), String> {
    let validator =
        jsonschema::validator_for(schema).map_err(|e| format!("invalid JSON Schema: {e}"))?;

    let errors: Vec<String> = validator
        .iter_errors(data)
        .map(|e| {
            let path = e.instance_path();
            if path.as_str().is_empty() {
                e.to_string()
            } else {
                format!("{}: {}", path, e)
            }
        })
        .collect();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_data_passes() {
        let schema = json!({"type": "object", "required": ["name"]});
        let data = json!({"name": "test"});
        assert!(validate_schema(&data, &schema).is_ok());
    }

    #[test]
    fn missing_required_field_fails() {
        let schema = json!({"type": "object", "required": ["name"]});
        let data = json!({"other": "test"});
        let err = validate_schema(&data, &schema).unwrap_err();
        assert!(err.contains("required"));
    }

    #[test]
    fn wrong_type_fails() {
        let schema = json!({"type": "object", "properties": {"count": {"type": "number"}}});
        let data = json!({"count": "not a number"});
        let err = validate_schema(&data, &schema).unwrap_err();
        assert!(!err.is_empty());
    }
}
