//! Structural evidence checks, not a semantic truth classifier. Claims cite
//! successful same-turn results, optionally narrowed by `[evidence:<tool-use-id>]`.
//! Uncertain paragraphs start with `[unverified]`.
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::HookInput;

static CITATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[evidence:([^\]\s]+)\]").unwrap());
// Match SHAs whole, before numbers: equal digit fragments do not prove a SHA.
static LITERAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[0-9a-fA-F]{7,40}\b|\d+(?:[.:]\d+)*").unwrap()
});
static CLAIM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?i)\b(merged|landed|published|saved|deleted|completed|finished|done|passed|fixed|green|shipped|released|clipboard)\b|\b(one|two|three|four|five|six|seven|eight|nine|ten|half|several|few)\s+(seconds?|minutes?|hours?|days?|tasks?|branches|deliveries|tests?)\b"
).unwrap()
});

/// Claims that a delivery reached its target or passed CI. Unlike the rest of
/// `CLAIM`, an unrelated successful tool never satisfies them.
static DELIVERY_CLAIM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(merged|landed|shipped|green)\b").unwrap());
/// A Cassy task result that records a closed delivery.
static TASK_CLOSED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)status:\s*closed|\bclosed\b.*\bmerged\b|\bmerged\b.*\bclosed\b").unwrap());
/// A Cassy merge receipt delivered through the inbox.
static MERGE_RECEIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)merged and closed by cassy").unwrap());

#[derive(Default)]
struct Turn {
    started: bool,
    calls: HashMap<String, Call>,
    reply: Option<String>,
}

struct Call {
    command: String,
    clipboard_read: bool,
    output: Option<String>,
    /// The tool that ran and its `action`, for Cassy tool results.
    tool: String,
    action: String,
    /// The successful result is delivery evidence (see `delivery_evidence`).
    delivery: bool,
}

/// Read a bounded tail. Without a visible user-turn boundary, no citation is
/// accepted. Missing/truncated/unreadable transcripts therefore fail closed.
fn current_turn(input: &HookInput) -> Turn {
    let mut turn = Turn::default();
    let Some(path) = input.transcript_path.as_deref() else {
        return turn;
    };
    let Some(mut file) = File::open(path).ok() else {
        return turn;
    };
    let Ok(size) = file.metadata().map(|m| m.len()) else {
        return turn;
    };
    const LIMIT: u64 = 2 * 1024 * 1024;
    let start = size.saturating_sub(LIMIT);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return turn;
    }
    let mut data = String::new();
    if file.take(LIMIT).read_to_string(&mut data).is_err() {
        return turn;
    }
    for line in data.lines().skip(usize::from(start > 0)) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            turn = Turn::default();
            continue;
        };
        if row["isSidechain"] == true {
            continue;
        }
        if row["sessionId"]
            .as_str()
            .is_some_and(|id| id != input.session_id)
        {
            continue;
        }
        let kind = row["type"].as_str().unwrap_or_default();
        let payload = if kind == "response_item" || kind == "event_msg" {
            &row["payload"]
        } else {
            &row
        };
        let content = &row["message"]["content"];
        let claude_user = kind == "user"
            && row["isMeta"] != true
            && (content.is_string()
                || content
                    .as_array()
                    .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "text")));
        if claude_user
            || (kind == "event_msg"
                && matches!(
                    payload["type"].as_str(),
                    Some("user_message" | "turn_started")
                ))
            || (kind == "response_item"
                && payload["type"] == "message"
                && payload["role"] == "user")
        {
            turn = Turn {
                started: true,
                ..Turn::default()
            };
            continue;
        }
        if !turn.started {
            continue;
        }
        if kind == "assistant" {
            if let Some(blocks) = content.as_array() {
                let text = blocks
                    .iter()
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    turn.reply = Some(text)
                }
                for block in blocks {
                    if block["type"] == "tool_use" {
                        turn.call(block["id"].as_str(), block["name"].as_str(), &block["input"]);
                    }
                }
            }
        } else if kind == "user" {
            if let Some(blocks) = content.as_array() {
                for block in blocks {
                    if block["type"] == "tool_result" {
                        turn.result(
                            block["tool_use_id"].as_str(),
                            &block["content"],
                            block["is_error"] == true,
                        );
                    }
                }
            }
        } else if kind == "response_item" {
            match payload["type"].as_str() {
                Some("function_call" | "custom_tool_call") => {
                    let args = payload["arguments"]
                        .as_str()
                        .and_then(|s| serde_json::from_str::<Value>(s).ok())
                        .unwrap_or_default();
                    turn.call(payload["call_id"].as_str(), payload["name"].as_str(), &args);
                }
                Some("function_call_output" | "custom_tool_call_output") => {
                    turn.result(payload["call_id"].as_str(), &payload["output"], false);
                }
                Some("message")
                    if payload["role"] == "assistant" && payload["channel"] == "final" =>
                {
                    turn.reply = payload["content"].as_array().map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(|b| b["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    });
                }
                _ => {}
            }
        }
    }
    turn
}

impl Turn {
    fn call(&mut self, id: Option<&str>, name: Option<&str>, input: &Value) {
        let Some(id) = id else { return };
        let command = input["command"]
            .as_str()
            .or_else(|| input["cmd"].as_str())
            .unwrap_or_default();
        // An attempted overwrite invalidates a prior read, including a write
        // with an error: it may have changed the clipboard before failing.
        if command.contains("wl-copy")
            || command.contains("pbcopy")
            || command.contains("xclip")
            || command.contains("xsel")
        {
            for call in self.calls.values_mut().filter(|call| call.clipboard_read) {
                call.output = None;
                call.clipboard_read = false;
            }
        }
        let clipboard_read = shell_words::split(command).ok().is_some_and(|words| {
            matches!(
                words.first().map(String::as_str),
                Some("wl-paste" | "pbpaste")
            ) && words
                .iter()
                .skip(1)
                .all(|word| word.starts_with('-') && !word.contains([';', '|', '&']))
        });
        self.calls.insert(
            id.to_owned(),
            Call {
                command: command.to_owned(),
                clipboard_read,
                output: None,
                tool: name.unwrap_or_default().to_owned(),
                action: input["action"].as_str().unwrap_or_default().to_owned(),
                delivery: false,
            },
        );
    }

    fn result(&mut self, id: Option<&str>, output: &Value, error: bool) {
        let Some(call) = id.and_then(|id| self.calls.get_mut(id)) else {
            return;
        };
        if error || failed(output) {
            call.output = None;
            return;
        }
        call.output = Some(
            output
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| output.to_string()),
        );
        call.delivery = delivery_evidence(call);
    }
}

/// A successful result that proves a delivery state, judged by what actually
/// ran rather than by words in its arguments or output:
/// - an executed `git merge-base --is-ancestor` (exit 0, `&&` chains only);
/// - a gh run or check status whose result shows success with nothing pending
///   or failing;
/// - a Cassy task close/show result recording a closed delivery, or a Cassy
///   merge receipt from the inbox.
/// Shell output that mentions a word, a different gh verb, or a Read result
/// never counts.
fn delivery_evidence(call: &Call) -> bool {
    let output = call.output.as_deref().unwrap_or_default();
    if call.command.is_empty() {
        let cassy = |suffix: &str| call.tool.ends_with(suffix);
        return (cassy("task") && matches!(call.action.as_str(), "close" | "show" | "get") && TASK_CLOSED.is_match(output))
            || (cassy("coordination") && matches!(call.action.as_str(), "inbox_poll" | "inbox") && MERGE_RECEIPT.is_match(output));
    }
    let Ok(words) = shell_words::split(&call.command) else { return false };
    // Exit 0 proves every command only when they are joined by `&&`.
    if words.iter().any(|word| word.contains(['|', ';']) || word == "&") {
        return false;
    }
    let lower = output.to_ascii_lowercase();
    words.split(|word| word == "&&").any(|segment| match segment.first().map(String::as_str) {
        Some("git") => {
            let mut rest = segment[1..].iter().map(String::as_str);
            let mut subcommand = None;
            while let Some(word) = rest.next() {
                match word {
                    "-C" | "-c" => { rest.next(); }
                    _ if word.starts_with('-') => {}
                    _ => { subcommand = Some(word); break; }
                }
            }
            subcommand == Some("merge-base") && segment.iter().any(|word| word == "--is-ancestor")
        }
        Some("gh") => {
            let verbs: Vec<&str> = segment[1..].iter().map(String::as_str).filter(|word| !word.starts_with('-')).take(2).collect();
            let status = match verbs.as_slice() {
                ["run", "view" | "list" | "watch"] | ["pr", "checks"] => true,
                ["api", path] => path.contains("check-runs") || path.contains("/status"),
                _ => false,
            };
            status
                && (lower.contains("success") || lower.contains("pass"))
                && !["pending", "in_progress", "queued", "fail", "cancel"].iter().any(|state| lower.contains(state))
        }
        _ => false,
    })
}

fn failed(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.iter().any(|(key, value)| {
            (matches!(key.as_str(), "is_error" | "isError") && value == true)
                || (matches!(key.as_str(), "exit_code" | "exitCode")
                    && value.as_i64().is_some_and(|code| code != 0))
                || failed(value)
        }),
        Value::Array(values) => values.iter().any(failed),
        Value::String(text) => {
            if let Ok(nested) = serde_json::from_str::<Value>(text) {
                return failed(&nested);
            }
            text.lines().any(|line| {
                if line.starts_with("Process running with session ID")
                    || line.starts_with("Script running with cell ID")
                    || line.starts_with("Script error:")
                    || line.starts_with("Script failed")
                {
                    return true;
                }
                line.strip_prefix("Process exited with code ")
                    .or_else(|| line.strip_prefix("Exit code: "))
                    .and_then(|code| code.trim().parse::<i64>().ok())
                    .is_some_and(|code| code != 0)
            })
        }
        _ => false,
    }
}

pub(super) fn outgoing_denial(input: &HookInput) -> Option<String> {
    if !input
        .tool_name
        .as_deref()
        .is_some_and(|name| name.ends_with("coordination"))
    {
        return None;
    }
    let args = input.tool_input.as_ref()?;
    if !matches!(args["action"].as_str(), Some("message" | "interrupt")) {
        return None;
    }
    let target = args["target"].as_str()?.trim();
    if !target.eq_ignore_ascii_case("operator") && !target.starts_with("commander:") {
        return None;
    }
    let body = args["message"].as_str()?;
    let text = format!("{}\n{}", body, args["summary"].as_str().unwrap_or_default());
    denial(input, &text)
}

// gate-audit[forward]: H-47 Pure classifier; PreToolUse, MessageDisplay and Stop boundaries emit the refusal tag.
// gate[H-47 pre-tool]: Supervisor operator claims require current-turn evidence
pub(super) fn denial(input: &HookInput, text: &str) -> Option<String> {
    if !crate::harness_policy::is_supervisor(input) {
        return None;
    }
    let plain = CITATION.replace_all(text, "");
    if !LITERAL.is_match(&plain) && !CLAIM.is_match(&plain) {
        return None;
    }
    let turn = current_turn(input);
    for paragraph in text.split("\n\n") {
        // A stable, explicit uncertainty marker qualifies the whole paragraph.
        let unverified = paragraph.trim_start().starts_with("[unverified]");
        let plain = CITATION.replace_all(paragraph, "");
        if unverified || (!LITERAL.is_match(&plain) && !CLAIM.is_match(&plain)) {
            continue;
        }
        // Claude Code does not expose tool-use IDs to the model. Resolve
        // uncited claims against successful results; explicit IDs still narrow
        // evidence and an invented ID must never fall back to unrelated output.
        let evidence = if CITATION.is_match(paragraph) {
            CITATION
                .captures_iter(paragraph)
                .filter_map(|capture| turn.calls.get(&capture[1]))
                .filter(|call| call.output.is_some())
                .collect::<Vec<_>>()
        } else {
            turn.calls
                .values()
                .filter(|call| call.output.is_some())
                .collect::<Vec<_>>()
        };
        // Exact command references describe what ran, including numeric flags;
        // numbers elsewhere in the claim must still occur in a result.
        let plain = evidence.iter().fold(plain.into_owned(), |text, call| {
            if call.command.is_empty() {
                text
            } else {
                text.replace(&format!("`{}`", call.command), "")
            }
        });
        let literals_match = LITERAL.find_iter(&plain).all(|literal| {
            evidence.iter().any(|call| {
                LITERAL
                    .find_iter(call.output.as_deref().unwrap_or_default())
                    .any(|seen| seen.as_str() == literal.as_str())
            })
        });
        let clipboard = plain.to_ascii_lowercase().contains("clipboard");
        let delivery_unproven = DELIVERY_CLAIM.is_match(&plain) && !evidence.iter().any(|call| call.delivery);
        if evidence.is_empty()
            || delivery_unproven
            || !literals_match
            || (clipboard && !evidence.iter().any(|call| call.clipboard_read))
        {
            return Some(String::from(
                "Operator claim lacks matching same-turn evidence. Numbers and SHAs must occur in successful tool results from this turn; exact commands may be named in backticks. Optional [evidence:<tool-use-id>] citations narrow the results used. Otherwise prefix the paragraph with [unverified]; calculate conversions with a tool. Clipboard claims need a wl-paste or pbpaste read after the latest overwrite. A merged, landed, shipped or green claim needs delivery evidence from this turn: a successful `git merge-base --is-ancestor`, a successful gh check or run result, a Cassy task close or status, or a merge receipt.",
            ));
        }
    }
    None
}

pub(super) fn pane_denial(input: &HookInput) -> Option<String> {
    if !crate::harness_policy::is_supervisor(input) {
        return None;
    }
    let text = input
        .message
        .clone()
        .or_else(|| current_turn(input).reply)?;
    denial(input, &text)
}
