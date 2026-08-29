//! Bounded validation for opt-in skill availability checks.

use std::process::Command;
use std::time::Duration;

use crate::bounded_process::{BoundedCommandError, Deadline, run_command};
use crate::types::Skill;

/// Validation scripts are availability probes, not general-purpose jobs.
/// Keeping this bound short prevents a create/update request from holding the
/// MCP server on an unavailable dependency.
pub(crate) const VALIDATION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_VALIDATION_OUTPUT_BYTES: usize = 8 * 1024;

/// Run a skill's optional validation script before it is persisted.
///
/// The probe runs from a fresh temporary directory with a scrubbed environment
/// (only PATH is retained for executable lookup). This keeps relative writes
/// out of the project and avoids leaking CAS credentials. The process boundary
/// does not promise network isolation, so validation scripts must be local,
/// deterministic availability checks that do not require network access.
pub(crate) fn validate_skill(skill: &Skill) -> Result<(), String> {
    if skill.validation_script.trim().is_empty() {
        return Ok(());
    }

    let cwd = tempfile::tempdir()
        .map_err(|error| format!("could not create validation sandbox: {error}"))?;

    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("sh");
        command.args(["-c", &skill.validation_script]);
        command
    };
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", &skill.validation_script]);
        command
    };

    command.current_dir(cwd.path()).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }

    let output = match run_command(
        &mut command,
        Deadline::after(VALIDATION_TIMEOUT),
        VALIDATION_TIMEOUT,
    ) {
        Ok(output) => output,
        Err(BoundedCommandError::TimedOut) => {
            return Err(format!(
                "validation script timed out after {} seconds",
                VALIDATION_TIMEOUT.as_secs()
            ));
        }
        Err(BoundedCommandError::Io) => {
            return Err("validation script could not be started".to_string());
        }
    };

    if output.status.success() {
        return Ok(());
    }

    let stdout = bounded_output(&output.stdout);
    let stderr = bounded_output(&output.stderr);
    let details = match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => format!("; stdout: {stdout}"),
        (true, false) => format!("; stderr: {stderr}"),
        (false, false) => format!("; stdout: {stdout}; stderr: {stderr}"),
    };
    let status = output.status.code().map_or_else(
        || "terminated by signal".to_string(),
        |code| code.to_string(),
    );
    Err(format!("validation script failed (exit {status}){details}"))
}

fn bounded_output(bytes: &[u8]) -> String {
    let output = String::from_utf8_lossy(bytes);
    let mut bounded: String = output.chars().take(MAX_VALIDATION_OUTPUT_BYTES).collect();
    if output.chars().count() > MAX_VALIDATION_OUTPUT_BYTES {
        bounded.push_str("…");
    }
    bounded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_with_script(script: &str) -> Skill {
        let mut skill = Skill::new("validation-test".to_string(), "Validation".to_string());
        skill.validation_script = script.to_string();
        skill
    }

    #[test]
    fn no_script_is_valid() {
        assert!(validate_skill(&skill_with_script("")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn failure_includes_script_output() {
        let result = validate_skill(&skill_with_script("printf probe-failed >&2; exit 23"));
        let error = result.expect_err("failed probe should reject");
        assert!(error.contains("exit 23"));
        assert!(error.contains("probe-failed"));
    }

    #[cfg(unix)]
    #[test]
    fn probe_has_sandboxed_cwd_and_environment() {
        let result = validate_skill(&skill_with_script(
            "test ! -e .git && test -z \"$HOME\" && test -z \"$CAS_ROOT\"",
        ));
        assert!(result.is_ok(), "probe was not isolated: {result:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn probe_has_no_network_by_default() {
        let result = validate_skill(&skill_with_script(
            "test \"$(grep -c '^[^[:space:]]' /proc/net/route)\" -eq 1",
        ));
        assert!(result.is_ok(), "probe had a network route: {result:?}");
    }

    #[cfg(unix)]
    #[test]
    fn timeout_is_reported_and_bounded() {
        let started = std::time::Instant::now();
        let result = validate_skill(&skill_with_script("sleep 30"));
        let error = result.expect_err("long-running probe should time out");
        assert!(error.contains("timed out"));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timeout exceeded the test bound: {:?}",
            started.elapsed()
        );
    }
}
