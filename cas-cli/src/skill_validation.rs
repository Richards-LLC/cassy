//! Bounded validation for opt-in skill availability checks.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::bounded_process::{BoundedCommandError, Deadline, run_command};
use crate::types::Skill;

/// Validation scripts are availability probes, not general-purpose jobs.
/// Keeping this bound short prevents a create/update request from holding the
/// MCP server on an unavailable dependency.
pub(crate) const VALIDATION_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(target_os = "linux")]
const BUBBLEWRAP_PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_VALIDATION_OUTPUT_BYTES: usize = 8 * 1024;

/// User-visible notice attached to successful and failed degraded probes.
pub(crate) const NETWORK_ISOLATION_WARNING: &str = "WARNING: network isolation is unavailable; bubblewrap is missing or could not create a network namespace, so validation ran in degraded plain-shell mode";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidationReport {
    pub(crate) warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ValidationMode {
    #[cfg(target_os = "linux")]
    Bubblewrap(PathBuf),
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BubblewrapCapability {
    Missing,
    Usable(PathBuf),
    Unusable(PathBuf),
}

/// Run a skill's optional validation script before it is persisted.
///
/// The probe runs from a fresh temporary directory with a scrubbed environment
/// (only PATH is retained for executable lookup). On Linux, bubblewrap adds a
/// network namespace with no routes. Hosts without bubblewrap use a plain-shell
/// fallback by default and receive an explicit warning; `require_sandbox` can
/// fail closed instead. This keeps relative writes out of the project and
/// avoids leaking CAS credentials. Validation scripts must be local,
/// deterministic availability checks that do not require network access.
pub(crate) fn validate_skill(skill: &Skill) -> Result<ValidationReport, String> {
    validate_skill_with_policy(skill, false)
}

pub(crate) fn validate_skill_with_policy(
    skill: &Skill,
    require_sandbox: bool,
) -> Result<ValidationReport, String> {
    if skill.validation_script.trim().is_empty() {
        return Ok(ValidationReport { warning: None });
    }

    let capability = bubblewrap_capability();
    let mode = select_validation_mode(require_sandbox, capability)?;
    validate_skill_with_mode(skill, mode)
}

fn select_validation_mode(
    require_sandbox: bool,
    capability: BubblewrapCapability,
) -> Result<ValidationMode, String> {
    #[cfg(target_os = "linux")]
    let unavailable_reason = match &capability {
        BubblewrapCapability::Missing => {
            "bubblewrap was not found, so network isolation cannot be provided"
        }
        BubblewrapCapability::Unusable(_) => {
            "bubblewrap could not create the required network namespace, so network isolation cannot be provided"
        }
        BubblewrapCapability::Usable(_) => "bubblewrap is available",
    };

    #[cfg(target_os = "linux")]
    match capability {
        BubblewrapCapability::Usable(bubblewrap) => {
            return Ok(ValidationMode::Bubblewrap(bubblewrap));
        }
        BubblewrapCapability::Missing | BubblewrapCapability::Unusable(_) if !require_sandbox => {
            return Ok(ValidationMode::Plain);
        }
        BubblewrapCapability::Unusable(_) | BubblewrapCapability::Missing => {}
    }

    #[cfg(not(target_os = "linux"))]
    let unavailable_reason = {
        let _ = capability;
        "bubblewrap is unavailable on this platform"
    };

    if require_sandbox {
        return Err(format!(
            "validation sandbox unavailable: skill_validation.require_sandbox is enabled, but {unavailable_reason}"
        ));
    }

    Ok(ValidationMode::Plain)
}

fn bubblewrap_capability() -> BubblewrapCapability {
    #[cfg(target_os = "linux")]
    {
        let Some(path) = find_executable("bwrap") else {
            return BubblewrapCapability::Missing;
        };
        if probe_bubblewrap(&path) {
            BubblewrapCapability::Usable(path)
        } else {
            BubblewrapCapability::Unusable(path)
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        BubblewrapCapability::Missing
    }
}

#[cfg(target_os = "linux")]
fn probe_bubblewrap(bubblewrap: &Path) -> bool {
    let Ok(cwd) = tempfile::tempdir() else {
        return false;
    };
    let mode = ValidationMode::Bubblewrap(bubblewrap.to_path_buf());
    let Ok(mut command) = validation_command("true", cwd.path(), &mode) else {
        return false;
    };
    command.current_dir(cwd.path()).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    run_command(
        &mut command,
        Deadline::after(BUBBLEWRAP_PROBE_TIMEOUT),
        BUBBLEWRAP_PROBE_TIMEOUT,
    )
    .is_ok_and(|output| output.status.success())
}

fn validate_skill_with_mode(
    skill: &Skill,
    mode: ValidationMode,
) -> Result<ValidationReport, String> {
    let cwd = tempfile::tempdir()
        .map_err(|error| format!("could not create validation sandbox: {error}"))?;

    let warning =
        matches!(mode, ValidationMode::Plain).then(|| NETWORK_ISOLATION_WARNING.to_string());
    let mut command = validation_command(&skill.validation_script, cwd.path(), &mode)?;

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
            return Err(with_warning(
                warning.as_deref(),
                format!(
                    "validation script timed out after {} seconds",
                    VALIDATION_TIMEOUT.as_secs()
                ),
            ));
        }
        Err(BoundedCommandError::Io) => {
            return Err(with_warning(
                warning.as_deref(),
                "validation script could not be started".to_string(),
            ));
        }
    };

    if output.status.success() {
        return Ok(ValidationReport { warning });
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
    Err(with_warning(
        warning.as_deref(),
        format!("validation script failed (exit {status}){details}"),
    ))
}

fn with_warning(warning: Option<&str>, message: String) -> String {
    warning
        .map(|warning| format!("{warning}; {message}"))
        .unwrap_or(message)
}

fn validation_command(script: &str, cwd: &Path, mode: &ValidationMode) -> Result<Command, String> {
    #[cfg(target_os = "linux")]
    {
        if let ValidationMode::Bubblewrap(bubblewrap) = mode {
            let mut command = Command::new(bubblewrap);
            command.args(["--unshare-net", "--die-with-parent", "--new-session"]);
            for system_path in ["/usr", "/bin", "/lib", "/lib64", "/etc"] {
                if Path::new(system_path).exists() {
                    command.args(["--ro-bind", system_path, system_path]);
                }
            }
            command.args([
                "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--bind",
            ]);
            command.arg(cwd);
            command.arg(cwd);
            command.args(["--chdir"]);
            command.arg(cwd);
            command.args(["--clearenv"]);
            if let Some(path) = std::env::var_os("PATH") {
                command.args(["--setenv", "PATH"]);
                command.arg(path);
            }
            command.args(["--", "/bin/sh", "-c"]);
            command.arg(script);
            return Ok(command);
        }
    }

    #[cfg(unix)]
    {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command.current_dir(cwd);
        return Ok(command);
    }

    #[cfg(windows)]
    {
        let mut command = Command::new("cmd");
        command.args(["/C", script]);
        command.current_dir(cwd);
        return Ok(command);
    }

    #[allow(unreachable_code)]
    Err("validation sandbox unsupported on this platform".to_string())
}

#[cfg(target_os = "linux")]
fn find_executable(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
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
        let result = validate_skill_with_mode(
            &skill_with_script("printf probe-failed >&2; exit 23"),
            ValidationMode::Plain,
        );
        let error = result.expect_err("failed probe should reject");
        assert!(error.contains("exit 23"));
        assert!(error.contains("probe-failed"));
    }

    #[cfg(unix)]
    #[test]
    fn probe_has_sandboxed_cwd_and_environment() {
        let result = validate_skill_with_mode(
            &skill_with_script("test ! -e .git && test -z \"$HOME\" && test -z \"$CAS_ROOT\""),
            ValidationMode::Plain,
        );
        assert!(result.is_ok(), "probe was not isolated: {result:?}");
    }

    #[cfg(unix)]
    #[test]
    fn fallback_reports_missing_network_isolation() {
        let report = validate_skill_with_mode(&skill_with_script("true"), ValidationMode::Plain)
            .expect("plain fallback should execute the probe");
        assert_eq!(report.warning.as_deref(), Some(NETWORK_ISOLATION_WARNING));
    }

    #[cfg(unix)]
    #[test]
    fn required_sandbox_rejects_unavailable_fallback() {
        assert_eq!(
            select_validation_mode(false, BubblewrapCapability::Missing)
                .expect("default policy should use fallback"),
            ValidationMode::Plain
        );
        assert_eq!(
            select_validation_mode(
                false,
                BubblewrapCapability::Unusable("/usr/bin/bwrap".into())
            )
            .expect("default policy should fall back when bwrap is unusable"),
            ValidationMode::Plain
        );
        let error = select_validation_mode(
            true,
            BubblewrapCapability::Unusable("/usr/bin/bwrap".into()),
        )
        .expect_err("required sandbox must fail closed when bwrap is unusable");
        assert!(error.contains("require_sandbox"));
        assert!(error.contains("network isolation"));
        let missing = select_validation_mode(true, BubblewrapCapability::Missing)
            .expect_err("required sandbox must fail closed when bwrap is missing");
        assert!(missing.contains("bubblewrap"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn probe_has_no_network_by_default() {
        let BubblewrapCapability::Usable(path) = bubblewrap_capability() else {
            return;
        };
        let result = validate_skill_with_mode(
            &skill_with_script("test \"$(grep -c '^[^[:space:]]' /proc/net/route)\" -eq 1"),
            ValidationMode::Bubblewrap(path),
        );
        assert!(result.is_ok(), "probe had a network route: {result:?}");
    }

    #[cfg(unix)]
    #[test]
    fn timeout_is_reported_and_bounded() {
        let started = std::time::Instant::now();
        let result =
            validate_skill_with_mode(&skill_with_script("sleep 30"), ValidationMode::Plain);
        let error = result.expect_err("long-running probe should time out");
        assert!(error.contains("timed out"));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timeout exceeded the test bound: {:?}",
            started.elapsed()
        );
    }
}
