//! Advisory failure labels. Callers retain every status, receipt and merge rule.
use super::{Answer, JevClient, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

const MAX_BLOCK: usize = 6 * 1024;
const PROBE_BUDGET: Duration = Duration::from_secs(1);
const EVAL_BUDGET: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureEvidence {
    pub source: String,
    pub platform: String,
    pub failing_block: String,
    pub touched_paths: Vec<String>,
    pub known_issue_hints: Vec<String>,
}

fn prefix(text: &str, bytes: usize) -> &str {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
fn suffix(text: &str, bytes: usize) -> &str {
    let mut start = text.len().saturating_sub(bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// Preserve names/first diagnostic plus the step tail, bounded in UTF-8 bytes.
/// Strip ANSI and common credential assignments before any remote evaluation.
pub fn evidence(
    source: &str,
    platform: &str,
    text: &str,
    touched_paths: Vec<String>,
) -> FailureEvidence {
    let ansi = regex::Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").expect("static ANSI regex");
    let secrets = regex::Regex::new(r"(?i)(authorization\s*:\s*bearer\s+|(?:token|password|secret|api[_-]?key)\s*[:=]\s*)[^\s]+")
        .expect("static credential regex");
    let cleaned = ansi.replace_all(text, "");
    let cleaned = secrets.replace_all(&cleaned, "${1}[REDACTED]");
    let lines: Vec<_> = cleaned.lines().collect();
    let names = lines
        .iter()
        .filter(|line| {
            line.contains("FAIL")
                || line.contains('✘')
                || line.contains("HUB-J")
                || line.contains("test result:")
        })
        .take(8)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let first_panic = lines.iter().position(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("panicked") || lower.contains("assertion")
    });
    let diagnostic = first_panic
        .or_else(|| {
            lines.iter().position(|line| {
                let lower = line.to_ascii_lowercase();
                lower.contains("panicked")
                    || lower.contains("assertion")
                    || lower.contains("error")
                    || lower.contains("timeout")
                    || lower.contains("requires")
            })
        })
        .map(|i| lines[i..(i + 8).min(lines.len())].join("\n"))
        .unwrap_or_default();
    let block = if cleaned.len() <= MAX_BLOCK {
        cleaned.into_owned()
    } else {
        format!(
            "{}\n{}\n[step tail]\n{}",
            prefix(&names, 768),
            prefix(&diagnostic, 1280),
            suffix(&cleaned, 4000)
        )
    };
    let mut known_issue_hints = Vec::new();
    if platform == "macos" || platform == "Darwin" {
        if block.contains("realpath") && block.contains("illegal option") {
            known_issue_hints.push(
                "macOS BSD realpath rejects Linux -e/-m flags; historical host portability defect"
                    .into(),
            );
        }
        if block.contains("flock") {
            known_issue_hints.push("Linux shared-rustup/cache fixtures require flock; Darwin needs an explicit platform skip".into());
        }
    }
    let mut bytes = 0;
    let touched_paths = touched_paths
        .into_iter()
        .filter_map(|path| {
            bytes += path.len();
            (bytes <= 2048).then_some(path)
        })
        .collect();
    FailureEvidence {
        source: prefix(source, 128).into(),
        platform: prefix(platform, 32).into(),
        failing_block: prefix(&block, MAX_BLOCK).into(),
        touched_paths,
        known_issue_hints,
    }
}

/// Read bounded head/tail windows; do not load a multi-megabyte CI log into state.
pub fn log_evidence(
    source: &str,
    log: &Path,
    summary: &str,
    touched: Vec<String>,
) -> FailureEvidence {
    let mut text = prefix(summary, 1024).to_owned();
    if let Ok(mut file) = File::open(log) {
        let mut head = Vec::new();
        let _ = (&mut file).take(128 * 1024).read_to_end(&mut head);
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&head));
        if file.metadata().is_ok_and(|m| m.len() > head.len() as u64) {
            let _ = file.seek(SeekFrom::End(-(MAX_BLOCK as i64)));
            let mut tail = Vec::new();
            let _ = file.take(MAX_BLOCK as u64).read_to_end(&mut tail);
            text.push_str("\n[log tail]\n");
            text.push_str(&String::from_utf8_lossy(&tail));
        }
    }
    evidence(source, std::env::consts::OS, &text, touched)
}

/// Code-derived paths only; invalid/missing refs silently provide no overlap evidence.
pub fn touched_paths(repo: &Path, base: &str, head: &str) -> Vec<String> {
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .args(["diff", "--name-only", "-z", base, head, "--"]);
    let Ok(output) = crate::bounded_process::run_command(
        &mut command,
        crate::bounded_process::Deadline::after(PROBE_BUDGET),
        PROBE_BUDGET,
    ) else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        .take(128)
        .map(str::to_owned)
        .collect()
}

pub fn questions() -> Value {
    serde_json::from_str(include_str!("failure_questions.json")).expect("static failure questions")
}

pub fn classify(client: &JevClient, state: &FailureEvidence) -> Option<String> {
    let outcome = client
        .ask_until(
            &json!(state),
            &questions(),
            &format!("failure:{}", state.source),
            true,
            Instant::now() + EVAL_BUDGET,
        )
        .ok()?;
    annotation(&outcome)
}

pub fn annotation(outcome: &Outcome) -> Option<String> {
    let Outcome::Available(response) = outcome else {
        return None;
    };
    let Answer::Choice {
        choice, confidence, ..
    } = response.answers.get("failure_class")?
    else {
        return None;
    };
    if ![
        "real_regression",
        "load_flake_or_timeout",
        "host_or_toolchain_env",
        "known_issue",
        "unknown",
    ]
    .contains(&choice.as_str())
        || !confidence.is_finite()
        || !(0.0..=1.0).contains(confidence)
    {
        return None;
    }
    let Answer::Noul { noul } = response.answers.get("mentions_touched_change")? else {
        return None;
    };
    if !noul.is_finite() || !(0.0..=1.0).contains(noul) {
        return None;
    }
    Some(format!(
        "Jev: {choice} ({confidence:.2}); touched-change mention={noul:.2} (advisory)"
    ))
}

pub fn label(
    cas_root: &Path,
    repo: &Path,
    log: Option<&Path>,
    summary: &str,
    source: &str,
    base: &str,
    head: &str,
) -> Option<String> {
    let client = JevClient::from_project(cas_root).ok()?;
    if !client.config.enabled || client.transport.is_err() {
        return None;
    }
    let touched = touched_paths(repo, base, head);
    let state = match log {
        Some(log) => log_evidence(source, log, summary, touched),
        None => evidence(source, std::env::consts::OS, summary, touched),
    };
    classify(&client, &state)
}

#[cfg(test)]
mod tests;
