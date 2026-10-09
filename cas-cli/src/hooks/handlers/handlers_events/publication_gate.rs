//! cas-f6ad (GH #1057): a deliverable is not shared before its epic's
//! verification passes. A supervisor posted a client-bound PDF through Violet
//! while the epic's verification task was still open, with a "final check
//! still running" caveat; verification then found 18 wrong statements and the
//! post had to be retracted. This pre-tool check refuses a `violet_post` that
//! shares a file (`kind: "file"`, `"file_external"`, or a `"thread"` reply with
//! `files`), or any post marked `deliverable: true`, while the epic has
//! open verification-type tasks. An operator's `PUBLICATION OVERRIDE:` note on
//! the epic, recorded within the last few hours, lets one through, and each use
//! is logged back onto the epic.
use std::path::Path;

use cas_types::{Task, TaskType};
use serde_json::Value;

/// How long a recorded `PUBLICATION OVERRIDE:` note authorizes posting.
const OVERRIDE_WINDOW_HOURS: i64 = 6;
/// The note marker an operator's override carries on the epic.
pub(super) const OVERRIDE_MARKER: &str = "PUBLICATION OVERRIDE:";

pub(super) fn denial(tool: &str, input: Option<&Value>, cas_root: Option<&Path>) -> Option<String> {
    let gated: Vec<Value> = violet_posts(tool, input)
        .into_iter()
        .filter(is_deliverable_post)
        .collect();
    if gated.is_empty() {
        return None;
    }
    let cas_root = cas_root?;
    let store = crate::store::open_task_store(cas_root).ok()?;
    // The post may name its epic (or a task under it); otherwise the
    // session's own focused epic is the one whose verification it waits on.
    let epic_id = gated
        .iter()
        .find_map(|post| explicit_epic(post, store.as_ref()))
        .or_else(crate::ui::factory::preferred_epic_id_from_session_metadata)?;
    let epic = store.get(&epic_id).ok()?;
    let mut open: Vec<String> = store
        .get_subtasks(&epic.id)
        .unwrap_or_default()
        .iter()
        .filter(|task| !task.is_terminal() && is_verification_task(task))
        .map(|task| format!("{} ({}): {}", task.id, task.status, task.title))
        .collect();
    if epic.pending_verification {
        open.push(format!("{}: the epic's own verification is pending", epic.id));
    }
    if open.is_empty() {
        return None;
    }
    if let Some(override_line) = recent_override(&epic.notes, chrono::Utc::now()) {
        let note = format!(
            "[{}] DECISION: publication gate overridden for a Violet deliverable post by the recorded operator override ({override_line}). Verification still open: {}.",
            chrono::Utc::now().format("%Y-%m-%d %H:%M"),
            open.join("; ")
        );
        let _ = store.append_note(&epic.id, &note);
        tracing::warn!(epic = %epic.id, "cas-f6ad: Violet deliverable post allowed by operator publication override");
        return None;
    }
    Some(format!(
        "PUBLICATION BLOCKED (verification_pending): this Violet post shares a deliverable \
         (kind=file or file_external, a thread reply with files, or deliverable=true) while epic {epic} still has open verification:\n  - {list}\n\n\
         Do not share a deliverable before its verification passes; a \"final check still running\" \
         caveat is not enough. Wait for those tasks to close, then post. Only the operator can \
         authorize an earlier share: record their words on the epic with \
         `task action=notes id={epic} note_type=decision notes=\"{OVERRIDE_MARKER} <operator's words>\"`. \
         An override is valid for {OVERRIDE_WINDOW_HOURS} hours, and each post it lets through is logged on the epic.",
        epic = epic.id,
        list = open.join("\n  - "),
    ))
}

/// Every `violet_post` argument object this tool call would send.
fn violet_posts(tool: &str, input: Option<&Value>) -> Vec<Value> {
    let lower = tool.to_ascii_lowercase();
    let direct = lower == "violet_post"
        || lower == "violet.violet_post"
        || lower == "violet_violet_post"
        || lower
            .strip_prefix("mcp__")
            .and_then(|rest| rest.split_once("__"))
            .is_some_and(|(server, method)| server == "violet" && method == "violet_post");
    if direct {
        return input.cloned().into_iter().collect();
    }
    if lower.ends_with("mcp_execute") {
        let mut posts = Vec::new();
        if let Some(code) = input.and_then(|input| input.get("code")) {
            collect_dispatch(code, &mut posts);
        }
        return posts;
    }
    Vec::new()
}

fn collect_dispatch(code: &Value, posts: &mut Vec<Value>) {
    match code {
        Value::Array(calls) => calls.iter().for_each(|call| collect_dispatch(call, posts)),
        Value::Object(call) => {
            let server = call.get("server").and_then(Value::as_str).unwrap_or_default();
            let tool = call.get("tool").and_then(Value::as_str).unwrap_or_default();
            if server.eq_ignore_ascii_case("violet") && tool.eq_ignore_ascii_case("violet_post") {
                let args = ["arguments", "args", "input", "params"]
                    .iter()
                    .find_map(|key| call.get(*key))
                    .cloned()
                    .unwrap_or(Value::Null);
                posts.push(args);
            }
        }
        Value::String(text) => {
            if let Ok(dispatch) = serde_json::from_str::<Value>(text) {
                collect_dispatch(&dispatch, posts);
                return;
            }
            // mcp_execute's other syntax: violet.violet_post({...}).
            let text = text.trim();
            if let Some(rest) = text.strip_prefix("violet.violet_post(")
                && let Some(body) = rest.trim_end().strip_suffix(')')
            {
                posts.push(serde_json::from_str(body.trim()).unwrap_or(Value::Null));
            }
        }
        _ => {}
    }
}

/// `kind: "file"` and `kind: "file_external"` always share an artifact, as
/// does a `kind: "thread"` whose `replies[]` carry `files` (cas-1206, GH
/// #1154: the 2026-10-09 contract added both routes); any other post shares
/// one when it is marked `deliverable: true`. An unparseable dispatch fails
/// toward the gate.
fn is_deliverable_post(post: &Value) -> bool {
    if post.is_null() {
        return true;
    }
    let shares_file = match post.get("kind").and_then(Value::as_str) {
        Some("file" | "file_external") => true,
        Some("thread") => post
            .get("replies")
            .and_then(Value::as_array)
            .is_some_and(|replies| replies.iter().any(|reply| reply.get("files").is_some())),
        _ => false,
    };
    shares_file || post.get("deliverable").and_then(Value::as_bool) == Some(true)
}

fn explicit_epic(post: &Value, store: &dyn cas_store::TaskStore) -> Option<String> {
    if let Some(epic) = post.get("epic_id").and_then(Value::as_str) {
        return Some(epic.to_string());
    }
    let task_id = post.get("task_id").and_then(Value::as_str)?;
    let task = store.get(task_id).ok()?;
    if task.task_type == TaskType::Epic {
        return Some(task.id);
    }
    store.get_parent_epic(task_id).ok().flatten().map(|epic| epic.id)
}

/// A verification-type task: a gate, or a task whose labels or title say it
/// verifies or reviews the delivery (`verification`, `verify`, `qa-pass`).
fn is_verification_task(task: &Task) -> bool {
    if task.task_type == TaskType::Gate {
        return true;
    }
    let labelled = task.labels.iter().any(|label| {
        let label = label.to_ascii_lowercase();
        label.contains("verif") || label == "qa-pass"
    });
    let title = task.title.trim().to_ascii_lowercase();
    labelled
        || title.starts_with("verify")
        || title.starts_with("verification")
        || title.starts_with("qa pass")
        || title.starts_with("independent verification")
}

/// The latest `PUBLICATION OVERRIDE:` note line recorded within the window.
fn recent_override(notes: &str, now: chrono::DateTime<chrono::Utc>) -> Option<String> {
    notes.lines().rev().find_map(|line| {
        let at = line.find(OVERRIDE_MARKER)?;
        let stamp = line.trim_start().strip_prefix('[')?.get(..16)?;
        let when = chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M").ok()?;
        let age = now.naive_utc() - when;
        (age >= chrono::Duration::zero() - chrono::Duration::minutes(5)
            && age <= chrono::Duration::hours(OVERRIDE_WINDOW_HOURS))
        .then(|| line[at..].trim().chars().take(200).collect())
    })
}
