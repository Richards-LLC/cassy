//! Structural evidence checks, not a semantic truth classifier. Claims cite
//! `[evidence:<tool-use-id>]`; uncertain paragraphs start with `[unverified]`.
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::HookInput;

static CITATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[evidence:([^\]\s]+)\]").unwrap());
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+(?:[.:]\d+)*").unwrap());
static CLAIM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?i)\b(merged|published|saved|deleted|completed|finished|done|passed|shipped|released|clipboard)\b|\b(one|two|three|four|five|six|seven|eight|nine|ten|half|several|few)\s+(seconds?|minutes?|hours?|days?|tasks?|branches|deliveries|tests?)\b"
).unwrap()
});

#[derive(Default)]
struct Turn {
    started: bool,
    calls: HashMap<String, Call>,
    reply: Option<String>,
}

struct Call {
    clipboard_read: bool,
    output: Option<String>,
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
                        turn.call(block["id"].as_str(), &block["input"]);
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
                    turn.call(payload["call_id"].as_str(), &args);
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
    fn call(&mut self, id: Option<&str>, input: &Value) {
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
                clipboard_read,
                output: None,
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
    }
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

// gate[H-47 pre-tool]: Supervisor operator claims require current-turn evidence
pub(super) fn denial(input: &HookInput, text: &str) -> Option<String> {
    if !crate::harness_policy::is_supervisor(input) {
        return None;
    }
    let plain = CITATION.replace_all(text, "");
    if !NUMBER.is_match(&plain) && !CLAIM.is_match(&plain) {
        return None;
    }
    let turn = current_turn(input);
    for paragraph in text.split("\n\n") {
        // A stable, explicit uncertainty marker qualifies the whole paragraph.
        let unverified = paragraph.trim_start().starts_with("[unverified]");
        let plain = CITATION.replace_all(paragraph, "");
        if unverified || (!NUMBER.is_match(&plain) && !CLAIM.is_match(&plain)) {
            continue;
        }
        let evidence = CITATION
            .captures_iter(paragraph)
            .filter_map(|capture| {
                turn.calls
                    .get(&capture[1])
                    .filter(|call| call.output.is_some())
            })
            .collect::<Vec<_>>();
        let numbers_match = NUMBER.find_iter(&plain).all(|number| {
            evidence.iter().any(|call| {
                NUMBER
                    .find_iter(call.output.as_deref().unwrap_or_default())
                    .any(|seen| seen.as_str() == number.as_str())
            })
        });
        let clipboard = plain.to_ascii_lowercase().contains("clipboard");
        if evidence.is_empty()
            || !numbers_match
            || (clipboard && !evidence.iter().any(|call| call.clipboard_read))
        {
            return Some(String::from(
                "Operator claim lacks matching same-turn evidence. Cite [evidence:<tool-use-id>] from a successful tool result in this turn for each factual paragraph, or prefix that paragraph with [unverified]. Numbers must occur in the cited result; calculate conversions with a tool. Clipboard claims need a wl-paste or pbpaste read after the latest overwrite.",
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
