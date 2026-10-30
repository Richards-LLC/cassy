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
    "options",
    "attachment",
    "supersede_key",
    "sticky",
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
    // cas-2a5a: `peers scope=all` lists the account's supervisors.
    "scope",
];

/// Descriptions `coordination` publishes in place of the shared
/// `CoordinationRequest` text, which is written for every action of both
/// tools. Each names only what the parameter does for coordination's own
/// actions.
pub(crate) const COORDINATION_DESCRIPTIONS: &[(&str, &str)] = &[
    ("action", "Operation. `inbox` is an alias of `inbox_poll`; `interrupt` is `message` with urgent=true."),
    ("id", "Agent id (default self); message_status: message id."),
    ("target", "Recipient: agent, supervisor, all_workers, operator, commander:<label>; remind: self."),
    ("message", "Body chars (configurable): 1200; blocker/merge 2500; notes 1500. Over-cap operator/Commander refused; supervisor-to-agent spills to artifact."),
    ("urgent", "Interrupt recipient and inject next; discards in-flight work."),
    ("blocker", "Blocker escalation; wakes idle supervisor."),
    ("merge_request", "Parked task_id merge request."),
    ("in_reply_to", "notification_id of direct message answered."),
    ("kind", "answer|status|receipt|ask|blocker; receipts never wake a supervisor."),
    ("wake", "message: false keeps a directed message inbox-only; broadcasts never wake"),
    ("options", "Operator ask choices; tap returns the label with in_reply_to."),
    ("attachment", "Published artifact id for Commander."),
    // Keep explicit caller contracts within the 3,600 B coordination budget.
    ("supersede_key", "Broadcast: newest per key wins."),
    ("sticky", "Broadcast: late joiners get it."),
    ("task_id", "Parked task for merge_request; remind: bind to task."),
    // cas-6ccb: polled rows can resurface once until acknowledged.
    ("notification_id", "message_ack/message_status: id. inbox_poll: read held row; unacked unnamed batch reads reappear once."),
    ("limit", "inbox_poll: max rows."),
    ("name", "register/session_start: agent name."),
    ("agent_type", "register/session_start: primary|sub_agent|worker|ci."),
    ("parent_id", "register/session_start: parent id."),
    ("session_id", "register/session_start/session_end: harness id."),
    ("remind_message", "Required reminder text; message kind=ask: default at deadline."),
    ("remind_delay_secs", "Delay seconds; message kind=ask: deadline."),
    ("remind_event", "task_completed|task_blocked|worker_idle|epic_completed|epic_split_decided; external: cross_session=true."),
    ("remind_filter", "split {\"split_id\":\"<id>\"}; branch_contained_in {\"commit\":\"<ref>\",\"target_branch\":\"main\"}; tag_exists {\"tag\":\"v1.0.0\"}; ci_completed {\"sha\":\"<40-hex>\",\"workflow\":\"CI\"} (workflow optional); task {\"task_id\":\"cas-1234\"}."),
    ("remind_id", "remind_cancel: reminder id."),
    ("remind_ttl_secs", "TTL seconds: default 3600; external default 604800; 0=never."),
    ("cross_session", "remind: survive session end and task close."),
    ("scope", "peers: all=account-wide."),
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
    "options",
    "attachment",
    "supersede_key",
    "sticky",
    "remind_message",
    "remind_delay_secs",
    "remind_event",
    "remind_filter",
    "remind_id",
    "remind_ttl_secs",
    "cross_session",
    "scope",
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
    if tool_name == "coordination" {
        for (name, description) in COORDINATION_DESCRIPTIONS {
            if let Some(Value::Object(property)) = properties.get_mut(*name) {
                property.insert(
                    "description".to_string(),
                    Value::String((*description).to_string()),
                );
            }
        }
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

    #[test]
    fn cas_3349_served_coordination_explains_limits_and_filters() {
        let tools = crate::mcp::tools::CasService::tool_definitions_for_build();
        let schema = &tools.iter().find(|tool| tool.name == "coordination").unwrap().input_schema;
        let description = |field: &str| schema["properties"][field]["description"].as_str().unwrap();
        // pin: callers discover request limits and examples through the served MCP schema.
        let message = description("message");
        let config = crate::config::FactoryConfig::default();
        for limit in [config.message_max_chars, config.message_max_chars_escalation, config.note_max_chars] {
            assert!(message.contains(&limit.to_string()), "missing cap {limit}: {message}");
        }
        assert!(message.contains("operator") && message.contains("refused"), "{message}");
        assert!(description("remind_event").contains("cross_session=true"));
        let filters = description("remind_filter");
        for example in [r#"{"commit":"<ref>","target_branch":"main"}"#,
            r#"{"tag":"v1.0.0"}"#, r#"{"sha":"<40-hex>","workflow":"CI"}"#,
            r#"{"task_id":"cas-1234"}"#]
        {
            assert!(filters.contains(example), "missing {example}: {filters}");
        }
        assert!(filters.contains("workflow optional"), "{filters}");
        let ttl = description("remind_ttl_secs");
        assert!(ttl.contains("default 3600") && ttl.contains("external default 604800") && ttl.contains("0=never"), "{ttl}");
    }

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

    /// The coordination description documents its actions; factory's schema
    /// enum separately publishes the factory action naming contract.
    #[test]
    fn coordination_description_names_every_registry_action() {
        let tools = crate::mcp::tools::CasService::tool_definitions_for_build();
        let coordination = tools
            .iter()
            .find(|tool| tool.name == "coordination")
            .expect("coordination tool is registered");
        let description = coordination.description.as_deref().unwrap_or_default();
        let words: std::collections::HashSet<&str> = description
            .split(|c: char| !c.is_ascii_lowercase() && c != '_')
            .collect();
        for action in cas_mcp::actions::COORDINATION_ACTIONS {
            assert!(words.contains(action), "coordination description omits `{action}`: {description}");
        }

    }

    /// cas-8563b (D2): the two tools sharing `CoordinationRequest` publish
    /// disjoint action enums. Slack-thread authority actions and their fields
    /// stay on the supervisor factory tool, outside the worker-loaded schema.
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

        for action in cas_mcp::actions::EXTERNAL_WORK_ACTIONS {
            assert!(
                !enum_of(&coordination).iter().any(|published| published == action),
                "coordination still publishes external-work action {action}"
            );
            assert!(
                enum_of(&factory).iter().any(|published| published == action),
                "factory does not publish external-work action {action}"
            );
        }

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
        for external_work_param in [
            "external_ref",
            "request_id",
            "resource_id",
            "handoff_id",
            "expected_generation",
            "to_slot_id",
            "offer_ttl_secs",
        ] {
            assert!(
                !coordination["properties"]
                    .as_object()
                    .unwrap()
                    .contains_key(external_work_param),
                "coordination still publishes external-work parameter {external_work_param}"
            );
            assert!(
                factory["properties"]
                    .as_object()
                    .unwrap()
                    .contains_key(external_work_param),
                "factory does not publish external-work parameter {external_work_param}"
            );
        }
        for messaging in ["message", "summary", "remind_message"] {
            assert!(
                !factory["properties"].as_object().unwrap().contains_key(messaging),
                "factory still publishes {messaging}"
            );
        }
        // Every coordination param carries coordination-only wording.
        for param in &coordination_params {
            if param.as_str() == "action" || param.as_str() == "summary" {
                continue;
            }
            assert!(
                COORDINATION_DESCRIPTIONS.iter().any(|(name, _)| name == param),
                "{param} has no coordination description"
            );
        }
        let bytes = |schema: &JsonObject| serde_json::to_string(schema).unwrap().len();
        eprintln!(
            "MCP serialized bytes: tools/list={}, coordination={}, factory={}",
            serde_json::to_vec(&tools).unwrap().len(),
            bytes(&coordination),
            bytes(&factory),
        );
        assert!(
            bytes(&coordination) <= 3_600,
            "the worker tool must stay small: coordination {} B (factory {} B)",
            bytes(&coordination),
            bytes(&factory)
        );
    }
}
