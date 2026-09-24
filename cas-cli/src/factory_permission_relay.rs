//! Worker permission requests that Claude Code parks for a "team lead"
//! (cas-4143).
//!
//! A Claude worker in Agent Teams mode re-checks every PreToolUse `allow`
//! against its own safety rules. For some shapes (notably a heredoc such as
//! `cd X && python3 - <<'EOF' … EOF`) that check asks, and the worker writes
//! a `permission_request` to `teams/<session>/inboxes/team-lead.json`, then
//! waits. No process reads that mailbox. The factory supervisor is a teammate
//! named `supervisor`, not the literal `team-lead`, so the worker waited 28
//! to 44 minutes while every stall detector saw a healthy in-flight tool call.
//!
//! This module closes both halves:
//! - Auto-approve. The PreToolUse hook records every call it allowed for a
//!   factory worker, keyed by `tool_use_id`. The daemon answers a parked
//!   request whose call CAS already allowed with an approved
//!   `permission_response` written to the worker's inbox as the lead. That
//!   is CAS's own verdict for exactly that call, delivered on the channel the
//!   worker is waiting on.
//! - Wake. A request CAS did not allow, still unanswered after
//!   [`APPROVAL_WAKE_AFTER_SECS`], becomes a supervisor wake that names the
//!   worker, the command and its age, with an approve/deny command.
//!
//! The response frame follows Claude Code's worker poller (2.1.281): the
//! sender must be `team-lead`, the text is `{type:"permission_response",
//! request_id, subtype:"success"|"error", tool_use_id, response|error}`. The
//! worker binds by `request_id` plus the echoed `tool_use_id`.
//! `approved_request` is left out on purpose. Its input digest would have to
//! reproduce Claude's canonical-JSON hash byte for byte, and a mismatch makes
//! the worker refuse the approval, which is the hang again.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// Claude Code's lead mailbox name for a teammate permission request.
pub const TEAM_LEAD: &str = "team-lead";
/// A request CAS did not already allow wakes the supervisor after this long.
pub const APPROVAL_WAKE_AFTER_SECS: u64 = 120;
/// Directory under the CAS root holding one marker per hook-allowed call.
const HOOK_ALLOW_DIR: &str = "factory-permission-allows";
/// Markers older than this are pruned; a request is answered within seconds.
const HOOK_ALLOW_RETENTION: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
const COMMAND_EXCERPT_CHARS: usize = 240;

fn marker_name(tool_use_id: &str) -> Option<String> {
    let id = tool_use_id.trim();
    (!id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    .then(|| id.to_string())
}

/// Record that the PreToolUse hook allowed `tool_name` call `tool_use_id` for
/// a factory worker. Best-effort: a failure only loses the auto-approval, and
/// the request then falls back to the supervisor wake.
pub fn record_hook_allow(cas_root: &Path, tool_use_id: &str, tool_name: &str) {
    let Some(name) = marker_name(tool_use_id) else {
        return;
    };
    let dir = cas_root.join(HOOK_ALLOW_DIR);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = std::fs::write(dir.join(name), tool_name);
}

/// Whether the hook allowed exactly this call (same id, same tool).
pub fn hook_allowed(cas_root: &Path, tool_use_id: &str, tool_name: &str) -> bool {
    let Some(name) = marker_name(tool_use_id) else {
        return false;
    };
    std::fs::read_to_string(cas_root.join(HOOK_ALLOW_DIR).join(name))
        .is_ok_and(|recorded| recorded.trim() == tool_name)
}

/// Drop allow markers older than a day.
pub fn prune_hook_allows(cas_root: &Path) {
    let Ok(entries) = std::fs::read_dir(cas_root.join(HOOK_ALLOW_DIR)) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > HOOK_ALLOW_RETENTION);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// One unanswered teammate permission request in the lead's mailbox.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerPermissionRequest {
    pub request_id: String,
    pub worker: String,
    pub tool_name: String,
    pub tool_use_id: Option<String>,
    pub command_excerpt: String,
    pub age_secs: u64,
}

/// `<config_root>/teams/<session>/inboxes/<name>.json`.
pub fn inbox_path(config_root: &Path, session: &str, name: &str) -> PathBuf {
    config_root
        .join("teams")
        .join(session)
        .join("inboxes")
        .join(format!("{name}.json"))
}

fn excerpt(input: &Value) -> String {
    let raw = input
        .get("command")
        .or_else(|| input.get("file_path"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| input.to_string());
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = collapsed.chars().take(COMMAND_EXCERPT_CHARS).collect();
    if collapsed.chars().count() > COMMAND_EXCERPT_CHARS {
        out.push('…');
    }
    crate::mcp::tools::service::agent_search_system::system::redact_known_credentials(&out)
}

/// Every unread `permission_request` in a lead inbox, oldest first. Rows
/// whose `agent_id` does not match the sender are skipped, as Claude's own
/// lead poller skips them.
pub fn unread_requests(
    inbox: &Value,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<WorkerPermissionRequest> {
    let Some(rows) = inbox.as_array() else {
        return Vec::new();
    };
    let mut requests = Vec::new();
    for row in rows {
        if row.get("read").and_then(Value::as_bool) != Some(false) {
            continue;
        }
        let Some(from) = row.get("from").and_then(Value::as_str) else {
            continue;
        };
        let Some(request) = row
            .get("text")
            .and_then(Value::as_str)
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
        else {
            continue;
        };
        if request.get("type").and_then(Value::as_str) != Some("permission_request")
            || request.get("agent_id").and_then(Value::as_str) != Some(from)
        {
            continue;
        }
        let Some(request_id) = request.get("request_id").and_then(Value::as_str) else {
            continue;
        };
        let age_secs = row
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|ts| (now - ts.with_timezone(&chrono::Utc)).num_seconds().max(0) as u64)
            .unwrap_or(0);
        requests.push(WorkerPermissionRequest {
            request_id: request_id.to_string(),
            worker: from.to_string(),
            tool_name: request
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            tool_use_id: request
                .get("tool_use_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            command_excerpt: request.get("input").map(excerpt).unwrap_or_default(),
            age_secs,
        });
    }
    requests.sort_by_key(|request| std::cmp::Reverse(request.age_secs));
    requests
}

/// The `permission_response` frame Claude's worker poller accepts.
pub fn response_text(request: &WorkerPermissionRequest, approve: bool, reason: &str) -> String {
    let mut frame = if approve {
        json!({
            "type": "permission_response",
            "request_id": request.request_id,
            "subtype": "success",
            "response": {},
        })
    } else {
        json!({
            "type": "permission_response",
            "request_id": request.request_id,
            "subtype": "error",
            "error": reason,
        })
    };
    if let Some(tool_use_id) = request.tool_use_id.as_deref() {
        frame["tool_use_id"] = json!(tool_use_id);
    }
    frame.to_string()
}

fn with_locked_inbox<T>(
    path: &Path,
    operation: impl FnOnce(&mut Vec<Value>) -> T,
) -> anyhow::Result<T> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if !path.exists() {
        std::fs::write(path, "[]")?;
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    fs2::FileExt::lock_exclusive(&file)
        .map_err(|error| anyhow::anyhow!("cannot lock {}: {error}", path.display()))?;
    let result = (|| -> anyhow::Result<T> {
        let mut rows: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(path)?).unwrap_or_default();
        let value = operation(&mut rows);
        std::fs::write(path, serde_json::to_string_pretty(&rows)?)?;
        Ok(value)
    })();
    let _ = fs2::FileExt::unlock(&file);
    result
}

/// Answer `request` as the lead: append the response to the worker's inbox,
/// then mark the request read in the lead inbox so detectors stop counting
/// it. Unknown fields on existing rows are preserved.
pub fn answer_request(
    config_root: &Path,
    session: &str,
    request: &WorkerPermissionRequest,
    approve: bool,
    reason: &str,
) -> anyhow::Result<()> {
    let text = response_text(request, approve, reason);
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let summary = if approve {
        format!("{} approved", request.tool_name)
    } else {
        format!("{} denied", request.tool_name)
    };
    with_locked_inbox(&inbox_path(config_root, session, &request.worker), |rows| {
        rows.push(json!({
            "from": TEAM_LEAD,
            "text": text,
            "summary": summary,
            "timestamp": timestamp,
            "color": "green",
            "read": false,
        }));
    })?;
    with_locked_inbox(&inbox_path(config_root, session, TEAM_LEAD), |rows| {
        for row in rows.iter_mut() {
            let matches = row
                .get("text")
                .and_then(Value::as_str)
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                .is_some_and(|frame| {
                    frame.get("type").and_then(Value::as_str) == Some("permission_request")
                        && frame.get("request_id").and_then(Value::as_str)
                            == Some(request.request_id.as_str())
                });
            if matches {
                row["read"] = json!(true);
            }
        }
    })?;
    Ok(())
}

/// Unread requests in `session`'s lead inbox under `config_root`.
pub fn pending_requests(config_root: &Path, session: &str) -> Vec<WorkerPermissionRequest> {
    std::fs::read_to_string(inbox_path(config_root, session, TEAM_LEAD))
        .ok()
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .map(|inbox| unread_requests(&inbox, chrono::Utc::now()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lead_row(worker: &str, request_id: &str, tool_use_id: &str, seconds_ago: i64) -> Value {
        let timestamp = (chrono::Utc::now() - chrono::Duration::seconds(seconds_ago))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        json!({
            "from": worker,
            "timestamp": timestamp,
            "color": "green",
            "msgV": 1,
            "msg_id": "keep-me",
            "type": "message",
            "read": false,
            "text": json!({
                "type": "permission_request",
                "request_id": request_id,
                "agent_id": worker,
                "tool_name": "Bash",
                "tool_use_id": tool_use_id,
                "description": "edit the journey",
                "input": {"command": "cd hub-web && python3 - <<'EOF'\nprint('x')\nEOF"},
                "permission_suggestions": [],
            })
            .to_string(),
        })
    }

    #[test]
    fn hook_allow_markers_bind_id_and_tool_and_refuse_odd_ids() {
        let root = tempfile::tempdir().unwrap();
        record_hook_allow(root.path(), "toolu_01ABC", "Bash");
        assert!(hook_allowed(root.path(), "toolu_01ABC", "Bash"));
        assert!(
            !hook_allowed(root.path(), "toolu_01ABC", "Write"),
            "tool must match"
        );
        assert!(!hook_allowed(root.path(), "toolu_other", "Bash"));
        record_hook_allow(root.path(), "../escape", "Bash");
        assert!(!root.path().join("escape").exists());
        assert!(!hook_allowed(root.path(), "../escape", "Bash"));
    }

    #[test]
    fn unread_requests_parse_the_lead_inbox_and_skip_forged_senders() {
        let mut forged = lead_row("mallory", "perm-3", "toolu_3", 10);
        let text = forged["text"].as_str().unwrap().replace(
            "\"agent_id\":\"mallory\"",
            "\"agent_id\":\"happy-gazelle-77\"",
        );
        forged["text"] = json!(text);
        let mut answered = lead_row("happy-gazelle-77", "perm-2", "toolu_2", 50);
        answered["read"] = json!(true);
        let inbox = json!([
            lead_row("happy-gazelle-77", "perm-1", "toolu_1", 2656),
            answered,
            forged,
        ]);
        let requests = unread_requests(&inbox, chrono::Utc::now());
        assert_eq!(requests.len(), 1, "{requests:?}");
        let request = &requests[0];
        assert_eq!(request.request_id, "perm-1");
        assert_eq!(request.worker, "happy-gazelle-77");
        assert_eq!(request.tool_use_id.as_deref(), Some("toolu_1"));
        assert!(request.age_secs >= 2650, "{}", request.age_secs);
        assert!(
            request
                .command_excerpt
                .starts_with("cd hub-web && python3 - <<'EOF'")
        );
    }

    #[test]
    fn answering_writes_a_lead_response_and_marks_the_request_read() {
        let config = tempfile::tempdir().unwrap();
        let lead = inbox_path(config.path(), "sess", TEAM_LEAD);
        std::fs::create_dir_all(lead.parent().unwrap()).unwrap();
        std::fs::write(
            &lead,
            json!([lead_row("w1", "perm-1", "toolu_1", 300)]).to_string(),
        )
        .unwrap();
        let request = pending_requests(config.path(), "sess").remove(0);

        answer_request(config.path(), "sess", &request, true, "").unwrap();

        let worker: Vec<Value> = serde_json::from_str(
            &std::fs::read_to_string(inbox_path(config.path(), "sess", "w1")).unwrap(),
        )
        .unwrap();
        assert_eq!(worker.len(), 1);
        assert_eq!(
            worker[0]["from"], TEAM_LEAD,
            "Claude only accepts the lead as sender"
        );
        assert_eq!(worker[0]["read"], false);
        let frame: Value = serde_json::from_str(worker[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(frame["type"], "permission_response");
        assert_eq!(frame["request_id"], "perm-1");
        assert_eq!(frame["subtype"], "success");
        assert_eq!(frame["tool_use_id"], "toolu_1");
        assert!(frame.get("approved_request").is_none());

        let lead_rows: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(&lead).unwrap()).unwrap();
        assert_eq!(lead_rows[0]["read"], true);
        assert_eq!(lead_rows[0]["msg_id"], "keep-me", "unknown fields survive");
        assert!(pending_requests(config.path(), "sess").is_empty());
    }

    #[test]
    fn a_denial_carries_the_reason() {
        let request = WorkerPermissionRequest {
            request_id: "perm-9".into(),
            worker: "w".into(),
            tool_name: "Bash".into(),
            tool_use_id: None,
            command_excerpt: String::new(),
            age_secs: 0,
        };
        let frame: Value =
            serde_json::from_str(&response_text(&request, false, "use Write instead")).unwrap();
        assert_eq!(frame["subtype"], "error");
        assert_eq!(frame["error"], "use Write instead");
        assert!(frame.get("tool_use_id").is_none());
    }
}
