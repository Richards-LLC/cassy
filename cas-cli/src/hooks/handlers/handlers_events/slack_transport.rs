//! Default-on Slack write policy, shared by every pre-tool hook caller.
use std::path::Path;

use serde_json::Value;

use crate::config::{Config, SlackTransport, load_global_config};

pub(super) fn denial(tool: &str, input: Option<&Value>, cas_root: Option<&Path>) -> Option<String> {
    if !is_non_violet_slack_write(tool, input) {
        return None;
    }
    let project = cas_root.and_then(|root| Config::load(root).ok());
    let transport = project
        .and_then(|config| config.slack)
        .or_else(|| load_global_config().slack)
        .map(|config| config.transport)
        .unwrap_or_default();
    if transport == SlackTransport::Any {
        return None;
    }
    Some(format!(
        "Slack writes through `{tool}` are denied by slack.transport=violet. \
         Use violet.violet_post through the Violet proxy and follow the `violet` skill. \
         For Slack reads use violet.violet_read. The Claude Slack connector and Codex Slack app \
         are not approved transports. An operator can explicitly opt out with \
         `cas config set slack.transport any`."
    ))
}

fn is_non_violet_slack_write(tool: &str, input: Option<&Value>) -> bool {
    let lower = tool.to_ascii_lowercase();
    if lower.ends_with("mcp_execute") {
        return input
            .and_then(|input| input.get("code"))
            .is_some_and(proxy_dispatch_writes);
    }
    if let Some(rest) = lower.strip_prefix("mcp__") {
        if let Some((server, method)) = rest.split_once("__") {
            return dispatch_writes(server, method);
        }
    }
    // Grok/OpenCode sanitize MCP names to server_tool; direct dot-call names
    // and unqualified Violet methods also remain exempt.
    if lower.starts_with("violet_") || lower.starts_with("violet.") {
        return false;
    }
    lower.contains("slack") && !is_read(&lower)
}

fn dispatch_writes(server: &str, tool: &str) -> bool {
    let server = server.to_ascii_lowercase();
    let tool = tool.to_ascii_lowercase();
    server != "violet" && (server.contains("slack") || tool.contains("slack")) && !is_read(&tool)
}

fn proxy_dispatch_writes(code: &Value) -> bool {
    match code {
        Value::Array(calls) => calls.iter().any(proxy_dispatch_writes),
        Value::Object(call) => {
            let server = call
                .get("server")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let tool = call.get("tool").and_then(Value::as_str).unwrap_or_default();
            dispatch_writes(server, tool)
        }
        Value::String(code) => {
            if let Ok(dispatch) = serde_json::from_str::<Value>(code) {
                return proxy_dispatch_writes(&dispatch);
            }
            // mcp_execute's other supported syntax: server.tool({...}).
            code.trim().split_once('.').is_some_and(|(server, rest)| {
                let tool = rest.split_once('(').map_or(rest, |(tool, _)| tool);
                dispatch_writes(server.trim(), tool.trim())
            })
        }
        _ => false,
    }
}

fn is_read(tool: &str) -> bool {
    let words: Vec<_> = tool.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    // Mutation wins over read-shaped words, e.g. get_file_upload_url.
    if words.iter().any(|word| {
        matches!(
            *word,
            "send"
                | "post"
                | "schedule"
                | "create"
                | "update"
                | "edit"
                | "delete"
                | "remove"
                | "add"
                | "upload"
                | "write"
                | "set"
                | "invite"
                | "join"
                | "leave"
                | "archive"
                | "unarchive"
                | "rename"
                | "pin"
                | "unpin"
                | "mark"
                | "complete"
                | "save"
                | "publish"
                | "share"
                | "revoke"
                | "open"
                | "close"
                | "reply"
        )
    }) {
        return false;
    }
    // Unknown Slack operations fail closed, so future writes cannot bypass
    // policy merely because their spelling was absent from a mutation list.
    words.iter().any(|word| {
        matches!(
            *word,
            "read"
                | "get"
                | "list"
                | "search"
                | "fetch"
                | "info"
                | "history"
                | "replies"
                | "lookup"
                | "find"
                | "view"
                | "download"
                | "resolve"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn slack_write_classifier_covers_connectors_and_mutation_families() {
        for method in [
            "slack_send_message",
            "slack_send_message_draft",
            "slack_schedule_message",
            "slack_create_canvas",
            "slack_update_canvas",
            "slack_add_reaction",
            "slack_create_conversation",
            "slack_lists_create",
            "slack_lists_items_update",
            "slack_upload_file",
            "get_file_upload_url",
            "unknown_operation",
        ] {
            assert!(
                is_non_violet_slack_write(&format!("mcp__claude_ai_Slack__{method}"), None),
                "{method}"
            );
        }
        for tool in [
            "mcp__other_slack__send_message",
            "mcp__codex_apps__slack_send_message",
            "codex_apps__slack._slack_send_message",
            "slack_send_message",
            "slack_create_canvas",
        ] {
            assert!(is_non_violet_slack_write(tool, None), "{tool}");
        }
        for tool in [
            "mcp__claude_ai_Slack__slack_read_channel",
            "mcp__slack__conversations_history",
            "mcp__slack__slack_search",
            "mcp__slack__slack_list_channels",
            "mcp__slack__slack_get_canvas",
            "mcp__violet__violet_post",
            "violet.violet_post",
            "violet_violet_post",
            "violet_post",
            "Bash",
        ] {
            assert!(!is_non_violet_slack_write(tool, None), "{tool}");
        }
    }

    #[test]
    fn proxy_dispatches_cannot_hide_slack_writes_but_violet_is_exempt() {
        for code in [
            json!(r#"{"server":"slack","tool":"send_message","args":{}}"#),
            json!("slack.send_message({})"),
            json!("slack.send_message"),
            json!([{"server":"violet","tool":"violet_post"}, {"server":"slack","tool":"add_reaction"}]),
        ] {
            assert!(is_non_violet_slack_write(
                "mcp__cs__mcp_execute",
                Some(&json!({"code":code}))
            ));
        }
        for code in [
            json!("violet.violet_post({})"),
            json!("violet.slack_send_message"),
            json!(r#"{"server":"violet","tool":"slack_send_message"}"#),
            json!("slack.read_channel({})"),
            json!("github.create_issue({})"),
        ] {
            assert!(!is_non_violet_slack_write(
                "cas_mcp_execute",
                Some(&json!({"code":code}))
            ));
        }
    }
}
