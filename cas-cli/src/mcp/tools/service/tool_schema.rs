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

/// Parameters the agent-facing `coordination` tool publishes (D2 split,
/// cas-8563b). The tool shares `CoordinationRequest` with `factory`, so it
/// still deserializes the rest during the one-release alias window; it just
/// stops loading the supervisor parameters into every worker's context.
pub(crate) const COORDINATION_FIELDS: &[&str] = &[
    "action",
    "id",
    "target",
    "message",
    "summary",
    "blocker",
    "merge_request",
    "urgent",
    "in_reply_to",
    "kind",
    "attachment",
    "task_id",
    "notification_id",
    "limit",
    "name",
    "agent_type",
    "parent_id",
    "session_id",
    "remind_message",
    "remind_delay_secs",
    "remind_event",
    "remind_filter",
    "remind_id",
    "remind_ttl_secs",
    "cross_session",
];

/// Messaging and reminder parameters the supervisor `factory` tool does not
/// publish: they belong to `coordination`.
pub(crate) const FACTORY_HIDDEN_FIELDS: &[&str] = &[
    "message",
    "summary",
    "blocker",
    "merge_request",
    "urgent",
    "in_reply_to",
    "kind",
    "attachment",
    "remind_message",
    "remind_delay_secs",
    "remind_event",
    "remind_filter",
    "remind_id",
    "remind_ttl_secs",
    "cross_session",
];

/// Narrow the shared `CoordinationRequest` schema to the tool publishing it:
/// its own action enum and its own parameters.
fn narrow_split_tool(tool_name: &str, schema: &mut JsonObject) {
    fn coordination_keeps(field: &str) -> bool {
        COORDINATION_FIELDS.contains(&field)
    }
    fn factory_keeps(field: &str) -> bool {
        !FACTORY_HIDDEN_FIELDS.contains(&field)
    }
    let (actions, keep): (&[&str], fn(&str) -> bool) = match tool_name {
        "coordination" => (cas_mcp::actions::COORDINATION_ACTIONS, coordination_keeps),
        "factory" => (cas_mcp::actions::FACTORY_ACTIONS, factory_keeps),
        _ => return,
    };
    let Some(Value::Object(properties)) = schema.get_mut("properties") else {
        return;
    };
    let hidden: Vec<String> = properties
        .keys()
        .filter(|name| !keep(name))
        .cloned()
        .collect();
    for name in hidden {
        properties.remove(&name);
    }
    if let Some(Value::Object(action)) = properties.get_mut("action") {
        action.insert(
            "enum".to_string(),
            Value::Array(
                actions
                    .iter()
                    .map(|action| Value::String((*action).to_string()))
                    .collect(),
            ),
        );
    }
}

/// Return `tool` with a compact input schema.
pub(crate) fn compact_tool(mut tool: Tool) -> Tool {
    let mut schema = tool.input_schema.as_ref().clone();
    narrow_split_tool(tool.name.as_ref(), &mut schema);
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

    /// cas-8563b (D2): the two tools sharing `CoordinationRequest` publish
    /// disjoint action enums, and `coordination` drops the supervisor params.
    #[test]
    fn coordination_and_factory_publish_their_own_actions_and_params() {
        let tools = crate::mcp::tools::CasService::tool_definitions_for_build();
        let schema_of = |name: &str| {
            tools
                .iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("{name} tool is not registered"))
                .input_schema
                .as_ref()
                .clone()
        };
        let enum_of = |schema: &JsonObject| -> Vec<String> {
            schema["properties"]["action"]["enum"]
                .as_array()
                .expect("action enum")
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        };
        let coordination = schema_of("coordination");
        let factory = schema_of("factory");
        assert_eq!(enum_of(&coordination), cas_mcp::actions::COORDINATION_ACTIONS);
        assert_eq!(enum_of(&factory), cas_mcp::actions::FACTORY_ACTIONS);

        let coordination_params: Vec<&String> =
            coordination["properties"].as_object().unwrap().keys().collect();
        for param in &coordination_params {
            assert!(COORDINATION_FIELDS.contains(&param.as_str()), "{param}");
        }
        for supervisor_only in ["count", "worker_names", "config_dir", "workers", "allow_trunk", "command", "port"] {
            assert!(
                !coordination["properties"].as_object().unwrap().contains_key(supervisor_only),
                "coordination still publishes {supervisor_only}"
            );
            assert!(
                factory["properties"].as_object().unwrap().contains_key(supervisor_only),
                "factory must publish {supervisor_only}"
            );
        }
        for messaging in ["message", "summary", "remind_message"] {
            assert!(
                !factory["properties"].as_object().unwrap().contains_key(messaging),
                "factory still publishes {messaging}"
            );
        }
        let bytes = |schema: &JsonObject| serde_json::to_string(schema).unwrap().len();
        assert!(
            bytes(&factory) - bytes(&coordination) >= 6_000,
            "the worker tool must shed the supervisor params: coordination {} B, factory {} B",
            bytes(&coordination),
            bytes(&factory)
        );
    }
}
