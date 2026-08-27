//! Read-only readiness checks for `cas factory doctor` (cas-a487).
//!
//! This intentionally stays smaller and more actionable than the broad
//! `cas factory preflight` report.  It answers the question an operator has
//! immediately before selecting a harness: can this project launch Claude or
//! Codex workers, and does Codex have the project-local CAS MCP registration
//! it needs?  It never spawns workers or starts an MCP server.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Result;
use cas_factory::routing::CapabilitySnapshot;
use cas_factory::spec_resolver::{ConfigSources, resolve_specs, resolve_supervisor_spec};
use cas_mux::SupervisorCli;
use serde::Serialize;

use crate::bounded_process::{BoundedCommandError, Deadline, run_command};
use crate::cli::Cli;

use super::FactoryArgs;

const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DoctorRow {
    name: &'static str,
    state: DoctorState,
    required: bool,
    detail: String,
    remediation: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DoctorState {
    Ok,
    Missing,
    Stale,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct FactoryDoctorReport {
    rows: Vec<DoctorRow>,
}

impl FactoryDoctorReport {
    fn has_required_failure(&self) -> bool {
        self.rows
            .iter()
            .any(|row| row.required && row.state != DoctorState::Ok)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliProbe {
    Ok { path: PathBuf, version: String },
    Missing,
    TimedOut { path: PathBuf },
}

/// Execute the narrowly-scoped, no-spawn factory readiness report.
pub(super) fn execute(args: &FactoryArgs, cli: &Cli, cas_root: Option<&Path>) -> Result<()> {
    let project_root = std::env::current_dir()?;
    let required = required_harnesses(args, &project_root)?;
    let report = collect_report(
        &project_root,
        required.contains(&SupervisorCli::Claude),
        required.contains(&SupervisorCli::Codex),
        &probe_cli("claude"),
        &probe_cli("codex"),
        cas_factory::probe::codex_auth_present(),
        cas_mcp_registration(&project_root, cas_root),
    );

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report);
    }

    if report.has_required_failure() {
        anyhow::bail!(
            "factory doctor found missing required backend(s); fix the issue above and rerun `cas factory doctor`"
        );
    }
    Ok(())
}

/// Resolve the same cascade used for a factory launch, but do not apply the
/// Codex fallback: doctor must name Codex as *required* when configuration
/// explicitly requests it, rather than hiding the condition it is reporting.
fn required_harnesses(args: &FactoryArgs, project_root: &Path) -> Result<Vec<SupervisorCli>> {
    let worker_cli = parse_cli(&args.worker_cli)?;
    let supervisor_cli = parse_cli(&args.supervisor_cli)?;
    let project_config = Some(project_root.join(".cas").join("config.toml"));
    let sources = ConfigSources {
        cli_flag: (worker_cli != SupervisorCli::Claude).then_some(worker_cli),
        worker_spec_jsons: args.worker_spec.clone(),
        project_config: project_config.clone(),
        ..ConfigSources::default()
    };
    let worker = resolve_specs(1, sources).map_err(|error| {
        anyhow::anyhow!("failed to resolve worker config for factory doctor: {error}")
    })?;
    for spec in &worker {
        cas_factory::validate_explicit(spec, &CapabilitySnapshot::default()).map_err(|error| {
            anyhow::anyhow!("failed to validate worker config for factory doctor: {error}")
        })?;
    }
    let sources = ConfigSources {
        cli_flag: (supervisor_cli != SupervisorCli::Claude).then_some(supervisor_cli),
        supervisor_spec_json: args.supervisor_spec.clone(),
        project_config,
        ..ConfigSources::default()
    };
    let supervisor = resolve_supervisor_spec(sources).map_err(|error| {
        anyhow::anyhow!("failed to resolve supervisor config for factory doctor: {error}")
    })?;
    cas_factory::validate_explicit(&supervisor, &CapabilitySnapshot::default()).map_err(
        |error| anyhow::anyhow!("failed to validate supervisor config for factory doctor: {error}"),
    )?;

    let mut required = Vec::new();
    for harness in worker
        .into_iter()
        .map(|spec| spec.cli)
        .chain(std::iter::once(supervisor.cli))
    {
        if !required.contains(&harness) {
            required.push(harness);
        }
    }
    Ok(required)
}

fn parse_cli(value: &str) -> Result<SupervisorCli> {
    value.parse::<SupervisorCli>().map_err(|_| {
        anyhow::anyhow!("invalid CLI {value:?}; expected 'claude', 'codex', or 'grok'")
    })
}

fn collect_report(
    project_root: &Path,
    claude_required: bool,
    codex_required: bool,
    claude: &CliProbe,
    codex: &CliProbe,
    codex_auth_present: bool,
    cas_mcp_registered: bool,
) -> FactoryDoctorReport {
    // A project-local Codex registration is required only when Codex is one
    // of the resolved harnesses. Claude reads `.mcp.json` instead, so a
    // Claude-only factory must not fail this Codex-specific setup check.
    let cas_mcp_required = codex_required;
    FactoryDoctorReport {
        rows: vec![
            cli_row(
                "Claude",
                claude_required,
                claude,
                "Install Claude Code, then rerun `cas factory doctor`.",
            ),
            codex_row(codex_required, codex, codex_auth_present),
            cas_mcp_row(project_root, cas_mcp_required, cas_mcp_registered),
        ],
    }
}

fn cli_row(
    name: &'static str,
    required: bool,
    probe: &CliProbe,
    missing_remediation: &str,
) -> DoctorRow {
    match probe {
        CliProbe::Ok { path, version } => DoctorRow {
            name,
            state: DoctorState::Ok,
            required,
            detail: format!("{} ({version})", path.display()),
            remediation: None,
        },
        CliProbe::Missing => DoctorRow {
            name,
            state: DoctorState::Missing,
            required,
            detail: format!("missing {name} on PATH"),
            remediation: Some(missing_remediation.to_string()),
        },
        CliProbe::TimedOut { path } => DoctorRow {
            name,
            state: DoctorState::Stale,
            required,
            detail: format!(
                "stale {} at {} (version probe timed out)",
                name.to_ascii_lowercase(),
                path.display()
            ),
            remediation: Some(format!(
                "Repair or reinstall {name}, then rerun `cas factory doctor`."
            )),
        },
    }
}

fn codex_row(required: bool, probe: &CliProbe, auth_present: bool) -> DoctorRow {
    // Name a missing/broken executable before discussing its credential file:
    // a host that has neither needs installation, not a login attempt against
    // a binary it cannot invoke.
    if !matches!(probe, CliProbe::Ok { .. }) {
        return cli_row(
            "Codex",
            required,
            probe,
            "Install Codex, then rerun `cas factory doctor`.",
        );
    }
    if !auth_present {
        return DoctorRow {
            name: "Codex",
            state: DoctorState::Missing,
            required,
            detail: "missing ~/.codex/auth.json (ChatGPT login)".to_string(),
            remediation: Some(
                "Run `codex login` (an OPENAI_API_KEY alone is not accepted for factory workers)."
                    .to_string(),
            ),
        };
    }
    cli_row(
        "Codex",
        required,
        probe,
        "Install Codex, then rerun `cas factory doctor`.",
    )
}

fn cas_mcp_row(project_root: &Path, required: bool, registered: bool) -> DoctorRow {
    let path = project_root.join(".codex").join("config.toml");
    if registered {
        DoctorRow {
            name: "CAS MCP",
            state: DoctorState::Ok,
            required,
            detail: format!("{} registered (server=cs)", path.display()),
            remediation: None,
        }
    } else {
        DoctorRow {
            name: "CAS MCP",
            state: if path.exists() { DoctorState::Stale } else { DoctorState::Missing },
            required,
            detail: format!(
                "{} is missing [mcp_servers.cs] command = \"cas\" with args including \"serve\"",
                path.display()
            ),
            remediation: Some("Run `cas sync codex` (or register the project-local `cs` MCP server) and rerun `cas factory doctor`.".to_string()),
        }
    }
}

fn probe_cli(program: &str) -> CliProbe {
    let Some(path) = find_on_path(program) else {
        return CliProbe::Missing;
    };
    match run_command(
        Command::new(&path).arg("--version"),
        Deadline::after(VERSION_PROBE_TIMEOUT),
        VERSION_PROBE_TIMEOUT,
    ) {
        Ok(output) if output.status.success() => CliProbe::Ok {
            path,
            version: first_line(&output.stdout),
        },
        Ok(_) | Err(BoundedCommandError::Io) => CliProbe::Missing,
        Err(BoundedCommandError::TimedOut) => CliProbe::TimedOut { path },
    }
}

fn find_on_path(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|directory| directory.join(program))
            .find(|candidate| candidate.is_file())
    })
}

fn first_line(output: &[u8]) -> String {
    String::from_utf8_lossy(output)
        .lines()
        .next()
        .unwrap_or("version unavailable")
        .trim()
        .to_string()
}

fn cas_mcp_registration(project_root: &Path, _cas_root: Option<&Path>) -> bool {
    let config_path = project_root.join(".codex").join("config.toml");
    let Ok(contents) = std::fs::read_to_string(config_path) else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&contents) else {
        return false;
    };
    let Some(server) = value
        .get("mcp_servers")
        .and_then(|servers| servers.get("cs"))
    else {
        return false;
    };
    let command_ok = server.get("command").and_then(toml::Value::as_str) == Some("cas");
    let serve_arg = server
        .get("args")
        .and_then(toml::Value::as_array)
        .is_some_and(|args| args.iter().any(|arg| arg.as_str() == Some("serve")));
    command_ok && serve_arg
}

fn print_human(report: &FactoryDoctorReport) {
    for row in &report.rows {
        let status = match row.state {
            DoctorState::Ok => "ok",
            DoctorState::Missing => "missing",
            DoctorState::Stale => "stale",
        };
        println!(
            "{:<9} {:<7} {}",
            format!("{}:", row.name),
            status,
            row.detail
        );
        if let Some(remediation) = &row.remediation {
            println!("           hint: {remediation}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(path: &str) -> CliProbe {
        CliProbe::Ok {
            path: PathBuf::from(path),
            version: "v1.2.3".to_string(),
        }
    }

    #[test]
    fn doctor_is_green_when_all_required_codex_components_are_ready() {
        let report = collect_report(
            Path::new("/project"),
            true,
            true,
            &ready("/bin/claude"),
            &ready("/bin/codex"),
            true,
            true,
        );
        assert!(!report.has_required_failure());
        assert_eq!(report.rows[1].state, DoctorState::Ok);
        assert_eq!(report.rows[2].state, DoctorState::Ok);
    }

    #[test]
    fn optional_missing_codex_does_not_fail_a_claude_only_factory() {
        let report = collect_report(
            Path::new("/project"),
            true,
            false,
            &ready("/bin/claude"),
            &CliProbe::Missing,
            false,
            false,
        );
        assert!(!report.has_required_failure());
        assert!(!report.rows[1].required);
        assert!(!report.rows[2].required);
    }

    #[test]
    fn required_missing_codex_auth_fails_with_chatgpt_login_hint() {
        let report = collect_report(
            Path::new("/project"),
            false,
            true,
            &CliProbe::Missing,
            &ready("/bin/codex"),
            false,
            true,
        );
        assert!(report.has_required_failure());
        assert_eq!(report.rows[1].state, DoctorState::Missing);
        assert!(
            report.rows[1]
                .remediation
                .as_deref()
                .unwrap()
                .contains("codex login")
        );
    }

    #[test]
    fn codex_mcp_registration_requires_the_cs_server_command() {
        let directory = tempfile::tempdir().unwrap();
        let codex_dir = directory.path().join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();
        std::fs::write(
            codex_dir.join("config.toml"),
            "[mcp_servers.cs]\ncommand = \"other\"\n",
        )
        .unwrap();
        assert!(!cas_mcp_registration(directory.path(), None));
        std::fs::write(
            codex_dir.join("config.toml"),
            "[mcp_servers.cs]\ncommand = \"cas\"\nargs = [\"serve\"]\n",
        )
        .unwrap();
        assert!(cas_mcp_registration(directory.path(), None));
    }

    fn doctor_routing_error(worker_spec: &str) -> String {
        let _home = crate::test_support::TestEnvGuard::temp_home();
        let directory = tempfile::tempdir().unwrap();
        let args = FactoryArgs {
            workers: 1,
            worker_spec: vec![worker_spec.to_string()],
            ..FactoryArgs::default()
        };
        required_harnesses(&args, directory.path())
            .expect_err("doctor must reject an invalid explicit routing spec")
            .to_string()
    }

    #[test]
    fn doctor_rejects_suspended_terra_with_registry_alternatives() {
        let error = doctor_routing_error(
            r#"{"cli":"codex","model":"gpt-5.6-terra","effort":"xhigh"}"#,
        );
        assert!(error.contains("Terra is suspended"), "{error}");
        assert!(error.contains("routing rule 'suspended recipe'"), "{error}");
        assert!(error.contains("codex_luna"), "{error}");
    }

    #[test]
    fn doctor_rejects_luna_high_with_registry_alternatives() {
        let error = doctor_routing_error(
            r#"{"cli":"codex","model":"gpt-5.6-luna","effort":"high"}"#,
        );
        assert!(error.contains("Luna is only permitted"), "{error}");
        assert!(error.contains("routing rule 'allowed effort'"), "{error}");
        assert!(error.contains("codex_luna"), "{error}");
        assert!(error.contains("effort=xhigh"), "{error}");
    }
}
