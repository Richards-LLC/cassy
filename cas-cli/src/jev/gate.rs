//! Default-off, observational hook gate. Nothing here can return a hook decision.
use super::*;
use crate::hooks::{HookInput, HookOutput};
use cas_core::hooks::types::HookSpecificOutput;
use std::{
    io::BufRead,
    sync::{OnceLock, mpsc},
};

pub const SHADOW_BUDGET: Duration = Duration::from_millis(800);
const STATE_BYTES: usize = 16 * 1024;
fn questions() -> &'static Value {
    static QUESTIONS: OnceLock<Value> = OnceLock::new();
    QUESTIONS.get_or_init(|| {
        let recipe: Value = serde_json::from_str(include_str!(
            "../../../docs/research/jev-gate-questions.json"
        ))
        .expect("shipped gate questions");
        recipe["questions"].clone()
    })
}
fn prefix(text: &str) -> &str {
    let mut end = text.len().min(STATE_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
fn state(input: &HookInput) -> Value {
    let tool = input.tool_name.as_deref().unwrap_or("");
    let data = input.tool_input.as_ref();
    let mut parameters = json!({});
    if tool == "Bash" {
        let command = data
            .and_then(|d| d.get("command"))
            .and_then(Value::as_str)
            .unwrap_or("");
        parameters["command"] = json!(prefix(command));
        parameters["command_truncated"] = json!(command.len() > STATE_BYTES);
    } else {
        let path = data
            .and_then(|d| d.get("file_path"))
            .and_then(Value::as_str)
            .unwrap_or("");
        parameters["file_path"] = json!(prefix(path));
        for key in ["content", "old_string", "new_string"] {
            if let Some(text) = data.and_then(|d| d.get(key)).and_then(Value::as_str) {
                parameters[format!("{key}_bytes")] = json!(text.len());
            }
        }
        parameters["contents_omitted"] = json!(true);
    }
    json!({"tool_name":tool,"tool_input":crate::ai_enrichment::redact_json(&parameters),"context":{
        "user_recent_messages":[],"recent_tool_output":[],"filesystem_facts":{},
        "provenance":"tool payload only; authorization, taint and recovery facts are unavailable"
    }})
}
fn decision(output: Option<&HookOutput>) -> &str {
    let Some(output) = output else {
        return "error";
    };
    if let Some(HookSpecificOutput::PreToolUse {
        permission_decision,
        updated_input,
        ..
    }) = &output.hook_specific_output
    {
        if let Some(value) = permission_decision {
            return value;
        }
        if updated_input.is_some() {
            return "rewrite";
        }
    }
    "ask"
}
fn composite(hook: &str, response: Option<&Response>, eval: bool) -> String {
    if hook == "deny" || hook == "error" {
        return hook.into();
    }
    let Some(response) = response else {
        return hook.into();
    };
    let risk = match response.answers.get("risk") {
        Some(Answer::Score { score, .. }) => *score,
        _ => return hook.into(),
    };
    let untrusted = match response.answers.get("from_untrusted") {
        Some(Answer::Noul { noul }) => *noul,
        _ => return hook.into(),
    };
    if untrusted >= 0.8 || risk >= 2.5 {
        "deny".into()
    } else if risk >= 1.5
        && !(eval
            && matches!(response.answers.get("user_requested"), Some(Answer::Noul { noul }) if *noul >= 0.8))
    {
        "ask".into()
    } else {
        hook.into()
    }
}

/// Observe only after the existing handler has returned. Disabled and unrelated
/// calls perform no network or log I/O. Errors and logging failures are isolated.
pub fn observe(input: &HookInput, root: Option<&Path>, output: Option<&HookOutput>) {
    if !matches!(input.tool_name.as_deref(), Some("Bash" | "Write" | "Edit")) {
        return;
    }
    let Some(root) = root else {
        return;
    };
    let start = Instant::now();
    let Ok(config) = crate::config::Config::load(root) else {
        return;
    };
    if !config.jev.unwrap_or_default().gate.shadow {
        return;
    }
    let deadline = start + SHADOW_BUDGET;
    let state = state(input);
    let root_owned = root.to_path_buf();
    let state_owned = state.clone();
    let (tx, rx) = mpsc::sync_channel(1);
    // The caller watchdog also covers credential resolution, retries and a
    // slow response body. Never join a network worker past the hook deadline.
    let spawned = std::thread::Builder::new()
        .name("jev-shadow".into())
        .spawn(move || {
            let mut request_id = None;
            let result = JevClient::from_project(&root_owned).and_then(|client| {
                client.evaluate(&state_owned, questions(), deadline, &mut request_id)
            });
            let _ = tx.send((result, request_id));
        });
    let (result, request_id) = if spawned.is_err() {
        (
            Err(JevError::Unavailable(
                "Jev shadow worker unavailable".into(),
            )),
            None,
        )
    } else {
        rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|_| {
                (
                    Err(JevError::Unavailable("Jev shadow deadline exceeded".into())),
                    None,
                )
            })
    };
    let response = result.as_ref().ok();
    let hook = decision(output);
    let candidate = composite(hook, response, true);
    let literal = composite(hook, response, false);
    let row = json!({
        "timestamp":Utc::now().to_rfc3339(),"caller":"hook:gate_shadow","tag":"gate_shadow",
        "state_hash":format!("{:x}",Sha256::digest(serde_json::to_vec(&state).expect("JSON state"))),
        "tool":input.tool_name,"hook_decision":hook,"composite_decision":candidate,
        "would_decide_eval":candidate,"would_decide_literal":literal,
        "hook_output_hash":output.map(|o| format!("{:x}",Sha256::digest(serde_json::to_vec(o).expect("hook JSON")))),
        "answers":response.map(|r| &r.answers),"model":response.map(|r| r.model.as_str()),
        "input_tokens":response.map(|r| r.usage.input_tokens),"request_id":request_id,
        "latency_ms":start.elapsed().as_millis(),"budget_ms":SHADOW_BUDGET.as_millis(),
        "status":if response.is_some() {"available"} else {"unavailable"},
        "reason":result.as_ref().err().map(|_| "Jev unavailable or shadow deadline exceeded"),
        "context_incomplete":true
    });
    let _ = append_shadow(&root.join("jev-decisions.jsonl"), &row);
}
fn append_shadow(path: &Path, row: &Value) -> std::io::Result<()> {
    // Do not block on an existing decision-log owner or a non-regular target.
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_file() {
            return Err(std::io::Error::other("non-regular shadow log"));
        }
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.try_lock_exclusive()?;
    let mut bytes = serde_json::to_vec(row)?;
    bytes.push(b'\n');
    let result = file.write_all(&bytes);
    let unlock = FileExt::unlock(&file);
    result.and(unlock)
}

pub fn report(root: &Path) -> std::io::Result<Value> {
    let path = root.join("jev-decisions.jsonl");
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(
                json!({"total":0,"available":0,"unavailable":0,"agreement_count":0,"agreement":null,"would_deny":0,"would_ask":0,"literal":{"would_deny":0,"would_ask":0},"exemption_changes":0,"latency_ms":{"median":null,"p95":null,"max":null},"malformed_rows":0,"examples":[]}),
            );
        }
        Err(e) => return Err(e),
    };
    let (mut total, mut available, mut agreement, mut deny, mut ask) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut literal_deny = 0u64;
    let mut literal_ask = 0u64;
    let mut exemption_changes = 0u64;
    let mut examples = Vec::new();
    let mut malformed = 0;
    let mut latency = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let line = line?;
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            malformed += 1;
            continue;
        };
        if row["tag"] != "gate_shadow" {
            continue;
        }
        total += 1;
        if row["status"] == "available" {
            available += 1;
            if row["hook_decision"] == row["composite_decision"] {
                agreement += 1;
            }
        }
        if row["composite_decision"] == "deny" {
            deny += 1;
        }
        if row["composite_decision"] == "ask" {
            ask += 1;
        }
        if row["would_decide_literal"] == "deny" {
            literal_deny += 1;
        }
        if row["would_decide_literal"] == "ask" {
            literal_ask += 1;
        }
        if row["would_decide_eval"] != row["would_decide_literal"] {
            exemption_changes += 1;
        }
        if let Some(ms) = row["latency_ms"].as_u64() {
            latency.push(ms);
        }
        if examples.len() < 10
            && (row["hook_decision"] != row["composite_decision"]
                || row["would_decide_eval"] != row["would_decide_literal"])
        {
            // Only allowlisted metadata is returned; never echo arbitrary log fields.
            examples.push(json!({"timestamp":row["timestamp"],"state_hash":row["state_hash"],"tool":row["tool"],"hook_decision":row["hook_decision"],"would_decide_eval":row["would_decide_eval"],"would_decide_literal":row["would_decide_literal"]}));
        }
    }
    latency.sort_unstable();
    let percentile = |numerator: usize| {
        latency
            .get((latency.len().saturating_sub(1) * numerator) / 100)
            .copied()
    };
    Ok(
        json!({"total":total,"available":available,"unavailable":total-available,"agreement_count":agreement,
        "agreement":if available>0 {Some(agreement as f64/available as f64)} else {None},"would_deny":deny,"would_ask":ask,
        "literal":{"would_deny":literal_deny,"would_ask":literal_ask},"exemption_changes":exemption_changes,
        "latency_ms":{"median":percentile(50),"p95":percentile(95),"max":latency.last()},"malformed_rows":malformed,"examples":examples}),
    )
}

#[cfg(test)]
mod tests;
