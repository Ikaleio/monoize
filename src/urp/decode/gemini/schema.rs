//! Gemini native `Schema` type-name casing (GEM-4). Conversion visits schema positions only, so
//! property names, defaults, examples, enum strings, and constants keep their payload.

use serde_json::{Value, json};

/// `schema` with every `type` at a schema position upper-cased (native `Schema`) or lower-cased
/// (JSON Schema).
pub(crate) fn native_schema_types(schema: &Value, uppercase: bool) -> Value {
    let Some(object) = schema.as_object() else {
        return schema.clone();
    };
    let mut result = object.clone();
    if let Some(kind) = object.get("type").and_then(Value::as_str) {
        result.insert(
            "type".into(),
            json!(if uppercase {
                kind.to_ascii_uppercase()
            } else {
                kind.to_ascii_lowercase()
            }),
        );
    }
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if let Some(properties) = object.get(key).and_then(Value::as_object) {
            result.insert(
                key.into(),
                Value::Object(
                    properties
                        .iter()
                        .map(|(name, schema)| {
                            (name.clone(), native_schema_types(schema, uppercase))
                        })
                        .collect(),
                ),
            );
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "additionalItems",
        "contains",
        "not",
        "if",
        "then",
        "else",
        "propertyNames",
        "unevaluatedProperties",
        "unevaluatedItems",
    ] {
        if let Some(schema) = object.get(key) {
            result.insert(
                key.into(),
                if let Some(items) = schema.as_array() {
                    Value::Array(
                        items
                            .iter()
                            .map(|schema| native_schema_types(schema, uppercase))
                            .collect(),
                    )
                } else {
                    native_schema_types(schema, uppercase)
                },
            );
        }
    }
    for key in ["anyOf", "allOf", "oneOf", "prefixItems"] {
        if let Some(items) = object.get(key).and_then(Value::as_array) {
            result.insert(
                key.into(),
                Value::Array(
                    items
                        .iter()
                        .map(|schema| native_schema_types(schema, uppercase))
                        .collect(),
                ),
            );
        }
    }
    Value::Object(result)
}
