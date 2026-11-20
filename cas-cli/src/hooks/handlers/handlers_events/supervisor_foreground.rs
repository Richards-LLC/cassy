//! Keep supervisors reachable (cas-803f). While a supervisor sits in a long
//! foreground Bash call, operator and Commander messages wait until it
//! returns. A call expected to run long must use `run_in_background` or go to
//! a worker. Codex and Grok shells have no `run_in_background`, and a
//! subagent's foreground call does not hold the supervisor's pane.
use super::pre_tool::{shell_invocations, shell_statement_words, shell_word_basename, worker_command_rust_build};
use cas_core::hooks::types::HookInput;
use std::path::Path;

/// The one list of commands known to run for minutes: a basename, then the
/// words that must follow it in order (other words may sit between them).
/// Cargo builds and tests are recognized by `worker_command_rust_build`.
const LONG_COMMANDS: &[&[&str]] = &[
    &["release.sh"],
    &["release-gate.sh"],
    &["release-publish.sh"],
    &["release-portable.sh"],
    &["release-zig.sh"],
    &["test-release-gate.sh"],
    &["journey-eval.sh"],
    &["npm", "test"],
    &["npm", "ci"],
    &["npm", "install"],
    &["npm", "run", "test"],
    &["npm", "run", "build"],
    &["pnpm", "test"],
    &["pnpm", "build"],
    &["pnpm", "install"],
    &["pnpm", "run", "test"],
    &["pnpm", "run", "build"],
    &["yarn", "test"],
    &["yarn", "build"],
    &["npx", "playwright", "test"],
    &["playwright", "test"],
    &["gh", "run", "watch"],
    &["gh", "pr", "checks", "--watch"],
    &["docker", "build"],
];

/// An explicit timeout above this many seconds marks the call as long.
const FOREGROUND_LIMIT_SECS: f64 = 60.0;

/// SSH options that take a separate value, so the host is found correctly.
const SSH_VALUE_OPTIONS: &[&str] = &[
    "-B", "-b", "-c", "-D", "-E", "-e", "-F", "-I", "-i", "-J", "-L", "-l", "-m", "-O", "-o", "-p",
    "-Q", "-R", "-S", "-W", "-w",
];

pub(super) fn denial(input: &HookInput) -> Option<String> {
    if true // red-run stub (cas-803f): H-49 disabled
        || input.tool_name.as_deref() != Some("Bash")
        || !crate::harness_policy::is_supervisor(input)
        || input.agent_id.as_deref().is_some_and(|id| !id.is_empty())
        || input.workspace_root.is_some()
        || cas_core::env_overlay::var("CAS_HOOK_HARNESS").is_ok_and(|harness| harness.eq_ignore_ascii_case("codex"))
    {
        return None;
    }
    let tool_input = input.tool_input.as_ref()?;
    if tool_input.get("run_in_background").and_then(|value| value.as_bool()) == Some(true) {
        return None;
    }
    let explicit_timeout = tool_input
        .get("timeout")
        .and_then(|value| value.as_f64())
        .filter(|millis| *millis > FOREGROUND_LIMIT_SECS * 1000.0)
        .map(|millis| format!("a {}-second timeout", (millis / 1000.0).round()));
    let command = tool_input.get("command").and_then(|value| value.as_str())?;
    let what = explicit_timeout.or_else(|| long_command(command, Path::new(&input.cwd), 0))?;
    Some(format!(
        "🚫 SUPERVISOR LONG FOREGROUND CALL: {what} can run past a minute, and operator and Commander messages cannot reach you until it returns. \
         Rerun with run_in_background: true (you are woken when it exits), or delegate to a worker."
    ))
}

fn long_command(command: &str, cwd: &Path, depth: usize) -> Option<String> {
    if let Some(build) = worker_command_rust_build(command, cwd) {
        return Some(format!("`{build}`"));
    }
    let invocations = shell_invocations(command);
    let words: Vec<&str> = command.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').collect();
    let has_loop = words.contains(&"do") && words.contains(&"done");
    if has_loop && invocations.iter().any(|(name, _)| name == "sleep") {
        return Some("a loop with sleep".to_string());
    }
    for (name, args) in &invocations {
        // `bash scripts/release.sh` runs the script itself.
        let (name, args) = match (name.as_str(), args.first()) {
            ("sh" | "bash" | "zsh" | "dash", Some(script)) if !script.starts_with('-') => {
                (shell_word_basename(script), &args[1..])
            }
            _ => (name.as_str(), &args[..]),
        };
        if let Some(entry) = LONG_COMMANDS.iter().find(|entry| entry[0] == name && follows_in_order(&entry[1..], args)) {
            return Some(format!("`{}`", entry.join(" ")));
        }
        if name == "ssh"
            && depth < 2
            && let Some(remote) = ssh_remote_command(args)
            && let Some(what) = long_command(&remote, cwd, depth + 1)
        {
            return Some(format!("ssh running {what}"));
        }
    }
    shell_statement_words(command).iter().find_map(|words| {
        let mut rest = words.iter().skip_while(|word| shell_word_basename(word) != "timeout").skip(1);
        let duration = loop {
            match rest.next()?.as_str() {
                "-s" | "-k" | "--signal" | "--kill-after" => _ = rest.next(),
                option if option.starts_with('-') => {}
                duration => break duration,
            }
        };
        (duration_secs(duration)? > FOREGROUND_LIMIT_SECS).then(|| format!("`timeout {duration}`"))
    })
}

fn follows_in_order(expected: &[&str], args: &[String]) -> bool {
    let mut args = args.iter();
    expected.iter().all(|want| args.any(|arg| arg == want || arg.strip_prefix(want).is_some_and(|rest| rest.starts_with(':'))))
}

fn ssh_remote_command(args: &[String]) -> Option<String> {
    let mut index = 0;
    while index < args.len() && args[index].starts_with('-') {
        index += if SSH_VALUE_OPTIONS.contains(&args[index].as_str()) { 2 } else { 1 };
    }
    let remote = args.get(index + 1..)?;
    (!remote.is_empty()).then(|| remote.join(" "))
}

/// `timeout` durations: `590`, `90s`, `2m`, `1h`, `1d`.
fn duration_secs(word: &str) -> Option<f64> {
    let (digits, scale) = match word.chars().last()? {
        's' => (&word[..word.len() - 1], 1.0),
        'm' => (&word[..word.len() - 1], 60.0),
        'h' => (&word[..word.len() - 1], 3600.0),
        'd' => (&word[..word.len() - 1], 86400.0),
        _ => (word, 1.0),
    };
    digits.parse::<f64>().ok().map(|value| value * scale)
}
