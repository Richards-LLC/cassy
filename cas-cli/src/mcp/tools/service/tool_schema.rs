//! Compact the tool list sent on `tools/list`.
//!
//! rmcp generates every input schema with schemars' `AddNullable` transform,
//! and every optional field carries `#[serde(default)]`. That adds
//! `"default": null` and `"nullable": true` (an OpenAPI 3.0 keyword, not JSON
//! Schema 2020-12) to each optional parameter, plus schemars' non-standard
//! integer `format`s and a per-tool `$schema`, `title` and root description
//! that repeats the tool description. None of it changes what a client may
//! send, and it was about 16% of the payload. [`compact_tool`] strips it.

use rmcp::model::{JsonObject, Tool};
use serde_json::Value;
use std::sync::Arc;

/// Formats defined by JSON Schema 2020-12. Any other `format` (schemars'
/// `uint`, `int64`, `float`, ...) is dropped; `minimum`/`maximum` stay.
const STANDARD_FORMATS: &[&str] = &[
    "date-time",
    "date",
    "time",
    "duration",
    "email",
    "idn-email",
    "hostname",
    "idn-hostname",
    "ipv4",
    "ipv6",
    "uri",
    "uri-reference",
    "iri",
    "iri-reference",
    "uuid",
    "uri-template",
    "json-pointer",
    "relative-json-pointer",
    "regex",
];

/// Keywords whose value is a map from names to subschemas; the map keys are
/// property names, never keywords, so they must not be stripped.
const SCHEMA_MAP_KEYWORDS: &[&str] = &["properties", "patternProperties", "$defs", "definitions"];

/// Return `tool` with a compact input schema.
pub(crate) fn compact_tool(mut tool: Tool) -> Tool {
    let mut schema = tool.input_schema.as_ref().clone();
    for keyword in ["$schema", "title", "description"] {
        schema.remove(keyword);
    }
    compact_schema(&mut schema);
    tool.input_schema = Arc::new(schema);
    tool
}

fn compact_schema(schema: &mut JsonObject) {
    schema.remove("nullable");
    schema.remove("title");
    if schema.get("default").is_some_and(Value::is_null) {
        schema.remove("default");
    }
    if schema
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|format| !STANDARD_FORMATS.contains(&format))
    {
        schema.remove("format");
    }

    for (keyword, value) in schema.iter_mut() {
        if SCHEMA_MAP_KEYWORDS.contains(&keyword.as_str()) {
            if let Value::Object(named) = value {
                for subschema in named.values_mut() {
                    compact_value(subschema);
                }
            }
        } else if keyword != "enum" && keyword != "const" && keyword != "default" {
            compact_value(value);
        }
    }
}

fn compact_value(value: &mut Value) {
    match value {
        Value::Object(schema) => compact_schema(schema),
        Value::Array(items) => items.iter_mut().for_each(compact_value),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_with(schema: Value) -> Tool {
        let Value::Object(schema) = schema else {
            panic!("fixture must be an object");
        };
        Tool::new("fixture", "fixture tool", Arc::new(schema))
    }

    #[test]
    fn strips_boilerplate_but_keeps_property_names_and_constraints() {
        let tool = compact_tool(tool_with(json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "FixtureRequest",
            "description": "Repeats the tool description",
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {"type": "string", "enum": ["a", "b"], "description": "Operation"},
                "title": {"default": null, "nullable": true, "type": "string", "description": "A property named title"},
                "limit": {"default": null, "nullable": true, "type": "integer", "format": "uint", "minimum": 0},
                "when": {"type": "string", "format": "date-time"},
                "flag": {"type": "boolean", "default": false}
            }
        })));
        let schema = Value::Object(tool.input_schema.as_ref().clone());
        assert_eq!(
            schema,
            json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": {"type": "string", "enum": ["a", "b"], "description": "Operation"},
                    "title": {"type": "string", "description": "A property named title"},
                    "limit": {"type": "integer", "minimum": 0},
                    "when": {"type": "string", "format": "date-time"},
                    "flag": {"type": "boolean", "default": false}
                }
            })
        );
        assert_eq!(tool.description.as_deref(), Some("fixture tool"));
    }
}
