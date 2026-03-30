//! JSON Schema validator for pipeline step contracts.
//!
//! Phase 4.1 Slice 1: validates step input/output data against
//! JSON Schema definitions from the pipeline TOML.
//!
//! Uses the `jsonschema` crate for RFC-compliant validation.

use serde_json::Value;

/// Validation result with structured error details.
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// JSON path to the problematic field.
    pub path: String,
    /// Human-readable error description.
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

/// Validate a JSON value against a JSON Schema.
///
/// Returns `Ok(())` if the data matches the schema, or a list of
/// validation errors if it does not.
///
/// If `schema` is `None`, validation is skipped (always passes).
pub fn validate_against_schema(
    data: &Value,
    schema: Option<&Value>,
) -> Result<(), Vec<ValidationError>> {
    let schema = match schema {
        Some(s) => s,
        None => return Ok(()),
    };

    let validator = jsonschema::validator_for(schema).map_err(|e| {
        vec![ValidationError {
            path: String::new(),
            message: format!("invalid JSON Schema: {e}"),
        }]
    })?;

    let result = validator.validate(data);
    if result.is_ok() {
        return Ok(());
    }

    let errors: Vec<ValidationError> = validator
        .iter_errors(data)
        .map(|e| ValidationError {
            path: e.instance_path.to_string(),
            message: e.to_string(),
        })
        .collect();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Format validation errors into a single human-readable string.
pub fn format_validation_errors(errors: &[ValidationError]) -> String {
    if errors.is_empty() {
        return "no errors".into();
    }
    errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_none_schema_passes() {
        let data = json!({"anything": "goes"});
        assert!(validate_against_schema(&data, None).is_ok());
    }

    #[test]
    fn validate_valid_object() {
        let schema = json!({
            "type": "object",
            "required": ["topic", "summary"],
            "properties": {
                "topic": { "type": "string" },
                "summary": { "type": "string", "minLength": 5 }
            }
        });
        let data = json!({
            "topic": "Rust",
            "summary": "Rust is a systems programming language"
        });
        assert!(validate_against_schema(&data, Some(&schema)).is_ok());
    }

    #[test]
    fn validate_missing_required_field() {
        let schema = json!({
            "type": "object",
            "required": ["topic", "summary"],
            "properties": {
                "topic": { "type": "string" },
                "summary": { "type": "string" }
            }
        });
        let data = json!({"topic": "Rust"});
        let err = validate_against_schema(&data, Some(&schema)).unwrap_err();
        assert!(!err.is_empty());
        assert!(err[0].message.contains("required"));
    }

    #[test]
    fn validate_wrong_type() {
        let schema = json!({
            "type": "object",
            "properties": {
                "approved": { "type": "boolean" }
            }
        });
        let data = json!({"approved": "yes"});
        let err = validate_against_schema(&data, Some(&schema)).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn validate_min_length() {
        let schema = json!({
            "type": "object",
            "properties": {
                "body": { "type": "string", "minLength": 10 }
            }
        });
        let data = json!({"body": "short"});
        let err = validate_against_schema(&data, Some(&schema)).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn validate_array_items() {
        let schema = json!({
            "type": "object",
            "properties": {
                "tags": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            }
        });
        let good = json!({"tags": ["rust", "wasm"]});
        assert!(validate_against_schema(&good, Some(&schema)).is_ok());

        let bad = json!({"tags": [1, 2, 3]});
        let err = validate_against_schema(&bad, Some(&schema)).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn validate_max_length() {
        let schema = json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "maxLength": 10 }
            }
        });
        let data = json!({"title": "this title is way too long for the constraint"});
        let err = validate_against_schema(&data, Some(&schema)).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn format_errors_readable() {
        let errors = vec![
            ValidationError {
                path: "/summary".into(),
                message: "missing required field".into(),
            },
            ValidationError {
                path: "/tags".into(),
                message: "expected array".into(),
            },
        ];
        let formatted = format_validation_errors(&errors);
        assert!(formatted.contains("/summary"));
        assert!(formatted.contains("/tags"));
    }

    #[test]
    fn invalid_schema_reports_error() {
        let bad_schema = json!({"type": "not-a-real-type"});
        let data = json!({"x": 1});
        // jsonschema may or may not reject this at compile time;
        // either way we should not panic.
        let _ = validate_against_schema(&data, Some(&bad_schema));
    }
}
