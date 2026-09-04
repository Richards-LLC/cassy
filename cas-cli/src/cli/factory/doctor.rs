//! Read-only readiness checks for `cas factory doctor` (cas-a487).
//!
//! This intentionally stays smaller and more actionable than the broad
//! `cas factory preflight` report.  It answers the question an operator has
//! immediately before selecting a harness: can this project launch the
//! registered workers, and does Codex have the project-local CAS MCP
//! registration it needs? It never spawns workers or starts an MCP server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Result;
use cas_factory::spec_resolver::{ConfigSources, resolve_specs, resolve_supervisor_spec};
use cas_factory::{CapabilityAvailability, CapabilitySnapshot, CapabilityStatus};
use cas_mux::SupervisorCli;
use cas_pty::{Harness, HarnessConformanceReceipt, harness_conformance_receipts};
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
    capability: CapabilityAvailability,
    capability_stale: bool,
    capability_observed_at_ms: Option<u64>,
    capability_expires_at_ms: Option<u64>,
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
    let catalog = cas_factory::registered_harnesses()
        .map_err(|error| anyhow::anyhow!("failed to load harness catalog: {error}"))?;
    let probes = catalog
        .iter()
        .copied()
        .map(|harness| (harness, probe_cli(harness_binary(harness))))
        .collect::<BTreeMap<_, _>>();
    let receipts = harness_conformance_receipts().unwrap_or_default();
    let capability_snapshot = collect_capability_snapshot(&catalog, &probes, &receipts);
    let report = collect_report(
        &project_root,
        &required,
        &probes,
        &capability_snapshot,
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
    let worker = if let Some(lane) = args.lane.as_deref() {
        super::resolve_lane_worker_specs(
            lane,
            1,
            &[],
            worker_cli,
            &args.worker_spec,
            &CapabilitySnapshot::default(),
        )?
        .0
    } else {
        resolve_specs(1, sources).map_err(|error| {
            anyhow::anyhow!("failed to resolve worker config for factory doctor: {error}")
        })?
    };
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
    required: &[SupervisorCli],
    probes: &BTreeMap<SupervisorCli, CliProbe>,
    capability_snapshot: &CapabilitySnapshot,
    cas_mcp_registered: bool,
) -> FactoryDoctorReport {
    // A project-local Codex registration is required only when Codex is one
    // of the resolved harnesses. Claude reads `.mcp.json` instead, so a
    // Claude-only factory must not fail this Codex-specific setup check.
    let cas_mcp_required = required.contains(&SupervisorCli::Codex);
    let mut rows = cas_factory::registered_harnesses()
        .expect("harness catalog was loaded before report collection")
        .into_iter()
        .map(|harness| {
            let probe = probes.get(&harness).cloned().unwrap_or(CliProbe::Missing);
            harness_row(
                harness,
                required.contains(&harness),
                &probe,
                capability_snapshot,
            )
        })
        .collect::<Vec<_>>();
    rows.push(cas_mcp_row(
        project_root,
        cas_mcp_required,
        cas_mcp_registered,
    ));
    FactoryDoctorReport { rows }
}

fn harness_row(
    harness: SupervisorCli,
    required: bool,
    probe: &CliProbe,
    capability_snapshot: &CapabilitySnapshot,
) -> DoctorRow {
    let pty_harness = pty_harness(harness);
    let model = cas_factory::default_worker_model_for_cli(harness);
    let account_profile = account_dir_for_harness(pty_harness).unwrap_or_else(|| "default".into());
    let identity = crate::capability::harness_route_identity(pty_harness, model, &account_profile);
    let capability_status = capability_snapshot.status_at(&identity, CapabilitySnapshot::now_ms());
    let capability = capability_status
        .as_ref()
        .map_or(CapabilityAvailability::Unknown, |status| {
            status.availability
        });
    let capability_stale = capability_status
        .as_ref()
        .is_some_and(|status| status.stale);
    let capability_observed_at_ms = capability_status
        .as_ref()
        .map(|status| status.observed_at_ms);
    let capability_expires_at_ms = capability_status
        .as_ref()
        .map(|status| status.expires_at_ms);
    let mut row = cli_row(
        harness_name(pty_harness),
        required,
        probe,
        harness_install_remediation(harness),
    );
    row.name = harness_name(pty_harness);
    row.capability = capability;
    row.capability_stale = capability_stale;
    row.capability_observed_at_ms = capability_observed_at_ms;
    row.capability_expires_at_ms = capability_expires_at_ms;
    if matches!(probe, CliProbe::Ok { .. }) && capability != CapabilityAvailability::Available {
        row.state = match capability {
            CapabilityAvailability::Unavailable => DoctorState::Missing,
            CapabilityAvailability::Unknown => DoctorState::Stale,
            CapabilityAvailability::Available => DoctorState::Ok,
        };
        if let Some(status) = capability_status {
            row.detail = capability_detail(harness, &status);
            row.remediation = status.remediation.or_else(|| {
                Some(
                    match capability {
                        CapabilityAvailability::Unavailable => match harness {
                            SupervisorCli::Claude => "Run `claude login`, then rerun doctor.",
                            SupervisorCli::Codex => "Run `codex login`, then rerun doctor.",
                            SupervisorCli::Grok => "Sign in to Grok Build, then rerun doctor.",
                            SupervisorCli::OpenCode => {
                                "Set QWENCLOUD_TOKEN_PLAN_API_KEY, then rerun doctor."
                            }
                        },
                        CapabilityAvailability::Unknown => {
                            "Retry doctor; a transient probe failure is not an unavailable account."
                        }
                        CapabilityAvailability::Available => {
                            "Rerun doctor to refresh capability evidence."
                        }
                    }
                    .to_string(),
                )
            });
        } else {
            row.detail = format!(
                "{} capability evidence is unknown",
                harness_name(pty_harness)
            );
            row.remediation = Some(
                "Rerun `cas factory doctor` to collect fresh route capability evidence."
                    .to_string(),
            );
        }
    }
    row
}

fn pty_harness(harness: SupervisorCli) -> Harness {
    match harness {
        SupervisorCli::Claude => Harness::ClaudeCode,
        SupervisorCli::Codex => Harness::CodexCli,
        SupervisorCli::Grok => Harness::GrokBuild,
        SupervisorCli::OpenCode => Harness::OpenCode,
    }
}

fn harness_name(harness: Harness) -> &'static str {
    match harness {
        Harness::ClaudeCode => "Claude",
        Harness::CodexCli => "Codex",
        Harness::GrokBuild => "Grok",
        Harness::OpenCode => "OpenCode",
    }
}

fn harness_binary(harness: SupervisorCli) -> &'static str {
    match harness {
        SupervisorCli::Claude => "claude",
        SupervisorCli::Codex => "codex",
        SupervisorCli::Grok => "grok",
        SupervisorCli::OpenCode => "opencode",
    }
}

fn account_dir_for_harness(harness: Harness) -> Option<String> {
    let variable = match harness {
        Harness::ClaudeCode => "CLAUDE_CONFIG_DIR",
        Harness::CodexCli => "CODEX_HOME",
        Harness::GrokBuild => "GROK_HOME",
        Harness::OpenCode => return None,
    };
    std::env::var(variable)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn collect_capability_snapshot(
    catalog: &[SupervisorCli],
    probes: &BTreeMap<SupervisorCli, CliProbe>,
    receipts: &[HarnessConformanceReceipt],
) -> CapabilitySnapshot {
    let jobs = catalog
        .iter()
        .copied()
        .map(|harness| {
            let pty = pty_harness(harness);
            let binary = match probes.get(&harness) {
                Some(CliProbe::Ok { version, .. }) => {
                    crate::capability::BinaryObservation::Observed(version.clone())
                }
                Some(CliProbe::TimedOut { .. }) => crate::capability::BinaryObservation::TimedOut,
                Some(CliProbe::Missing) | None => crate::capability::BinaryObservation::Unavailable,
            };
            let model = cas_factory::default_worker_model_for_cli(harness).to_string();
            let account_dir = account_dir_for_harness(pty);
            let receipt = receipts
                .iter()
                .find(|receipt| receipt.harness == pty)
                .cloned();
            (pty, model, account_dir, binary, receipt)
        })
        .collect::<Vec<_>>();

    let now_ms = CapabilitySnapshot::now_ms();
    let deadline = crate::bounded_process::Deadline::after(Duration::from_secs(6));
    let mut snapshot = CapabilitySnapshot::default();
    std::thread::scope(|scope| {
        let handles = jobs
            .into_iter()
            .map(|(harness, model, account_dir, binary, receipt)| {
                scope.spawn(move || {
                    crate::capability::probe_harness(
                        harness,
                        &model,
                        account_dir.as_deref(),
                        &binary,
                        receipt.as_ref(),
                        now_ms,
                        deadline,
                    )
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            if let Ok((identity, evidence)) = handle.join() {
                snapshot.record(identity, evidence);
            }
        }
    });
    snapshot
}

fn capability_detail(harness: SupervisorCli, status: &CapabilityStatus) -> String {
    let stale = if status.stale {
        " (evidence is stale)"
    } else {
        ""
    };
    let reason = status
        .reason
        .as_deref()
        .map_or(String::new(), |reason| format!(": {reason}"));
    format!(
        "{} capability {:?}{}{reason}",
        harness_name(pty_harness(harness)),
        status.availability,
        stale,
    )
}

fn harness_install_remediation(harness: SupervisorCli) -> &'static str {
    match harness {
        SupervisorCli::Claude => "Install Claude Code, then rerun `cas factory doctor`.",
        SupervisorCli::Codex => "Install Codex, then rerun `cas factory doctor`.",
        SupervisorCli::Grok => "Install Grok Build, then rerun `cas factory doctor`.",
        SupervisorCli::OpenCode => "Install OpenCode, then rerun `cas factory doctor`.",
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
            capability: CapabilityAvailability::Unknown,
            capability_stale: false,
            capability_observed_at_ms: None,
            capability_expires_at_ms: None,
            detail: format!("{} ({version})", path.display()),
            remediation: None,
        },
        CliProbe::Missing => DoctorRow {
            name,
            state: DoctorState::Missing,
            required,
            capability: CapabilityAvailability::Unknown,
            capability_stale: false,
            capability_observed_at_ms: None,
            capability_expires_at_ms: None,
            detail: format!("missing {name} on PATH"),
            remediation: Some(missing_remediation.to_string()),
        },
        CliProbe::TimedOut { path } => DoctorRow {
            name,
            state: DoctorState::Stale,
            required,
            capability: CapabilityAvailability::Unknown,
            capability_stale: false,
            capability_observed_at_ms: None,
            capability_expires_at_ms: None,
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

fn cas_mcp_row(project_root: &Path, required: bool, registered: bool) -> DoctorRow {
    let path = project_root.join(".codex").join("config.toml");
    if registered {
        DoctorRow {
            name: "CAS MCP",
            state: DoctorState::Ok,
            required,
            capability: CapabilityAvailability::Available,
            capability_stale: false,
            capability_observed_at_ms: None,
            capability_expires_at_ms: None,
            detail: format!("{} registered (server=cs)", path.display()),
            remediation: None,
        }
    } else {
        DoctorRow {
            name: "CAS MCP",
            state: if path.exists() { DoctorState::Stale } else { DoctorState::Missing },
            required,
            capability: CapabilityAvailability::Unavailable,
            capability_stale: false,
            capability_observed_at_ms: None,
            capability_expires_at_ms: None,
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
            "{:<9} {:<7} capability={:?}{} {}",
            format!("{}:", row.name),
            status,
            row.capability,
            if row.capability_stale { " (stale)" } else { "" },
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

    fn probes(
        claude: CliProbe,
        codex: CliProbe,
        grok: CliProbe,
    ) -> BTreeMap<SupervisorCli, CliProbe> {
        [
            (SupervisorCli::Claude, claude),
            (SupervisorCli::Codex, codex),
            (SupervisorCli::Grok, grok),
        ]
        .into_iter()
        .collect()
    }

    fn capabilities(codex: CapabilityAvailability) -> CapabilitySnapshot {
        let mut snapshot = CapabilitySnapshot::default();
        for (harness, model, availability) in [
            (
                Harness::ClaudeCode,
                "opus",
                CapabilityAvailability::Available,
            ),
            (Harness::CodexCli, "gpt-5.6-luna", codex),
            (
                Harness::GrokBuild,
                "grok-4.5",
                CapabilityAvailability::Available,
            ),
        ] {
            let account_profile =
                account_dir_for_harness(harness).unwrap_or_else(|| "default".into());
            snapshot.record(
                crate::capability::harness_route_identity(harness, model, &account_profile),
                cas_factory::CapabilityEvidence::new(availability, CapabilitySnapshot::now_ms()),
            );
        }
        snapshot
    }

    #[test]
    fn doctor_is_green_when_all_required_codex_components_are_ready() {
        let report = collect_report(
            Path::new("/project"),
            &[SupervisorCli::Claude, SupervisorCli::Codex],
            &probes(
                ready("/bin/claude"),
                ready("/bin/codex"),
                ready("/bin/grok"),
            ),
            &capabilities(CapabilityAvailability::Available),
            true,
        );
        assert!(!report.has_required_failure());
        assert_eq!(report.rows[1].state, DoctorState::Ok);
        assert_eq!(
            report
                .rows
                .iter()
                .find(|row| row.name == "CAS MCP")
                .unwrap()
                .state,
            DoctorState::Ok
        );
    }

    #[test]
    fn optional_missing_codex_does_not_fail_a_claude_only_factory() {
        let report = collect_report(
            Path::new("/project"),
            &[SupervisorCli::Claude],
            &probes(ready("/bin/claude"), CliProbe::Missing, ready("/bin/grok")),
            &capabilities(CapabilityAvailability::Available),
            false,
        );
        assert!(!report.has_required_failure());
        assert!(!report.rows[1].required);
        assert!(
            !report
                .rows
                .iter()
                .find(|row| row.name == "CAS MCP")
                .unwrap()
                .required
        );
    }

    #[test]
    fn required_missing_codex_auth_fails_with_chatgpt_login_hint() {
        let report = collect_report(
            Path::new("/project"),
            &[SupervisorCli::Codex],
            &probes(CliProbe::Missing, ready("/bin/codex"), ready("/bin/grok")),
            &capabilities(CapabilityAvailability::Unavailable),
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
    fn doctor_does_not_reuse_evidence_for_a_different_route_identity() {
        let mut snapshot = CapabilitySnapshot::default();
        snapshot.record(
            crate::capability::harness_route_identity(
                Harness::ClaudeCode,
                "different-model",
                "default",
            ),
            cas_factory::CapabilityEvidence::new(
                CapabilityAvailability::Available,
                CapabilitySnapshot::now_ms(),
            ),
        );
        let report = collect_report(
            Path::new("/project"),
            &[SupervisorCli::Claude],
            &probes(ready("/bin/claude"), CliProbe::Missing, CliProbe::Missing),
            &snapshot,
            false,
        );
        assert_eq!(report.rows[0].capability, CapabilityAvailability::Unknown);
        assert_eq!(report.rows[0].state, DoctorState::Stale);
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
    fn doctor_taste_lane_requires_codex_even_with_claude_project_defaults() {
        let _home = crate::test_support::TestEnvGuard::temp_home();
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join(".cas")).unwrap();
        std::fs::write(
            directory.path().join(".cas/config.toml"),
            r#"[factory.defaults]
cli = "claude"
model = "opus"
effort = "high"
"#,
        )
        .unwrap();
        let mut args = FactoryArgs {
            workers: 1,
            ..FactoryArgs::default()
        };
        assert!(
            !required_harnesses(&args, directory.path())
                .unwrap()
                .contains(&SupervisorCli::Codex)
        );
        args.lane = Some("taste".to_string());
        assert!(
            required_harnesses(&args, directory.path())
                .unwrap()
                .contains(&SupervisorCli::Codex)
        );
        args.worker_spec = vec![r#"{"model":"claude-opus-5"}"#.to_string()];
        assert!(required_harnesses(&args, directory.path()).is_err());
    }

    #[test]
    fn doctor_accepts_astra_taste_spec_and_requires_codex() {
        let _home = crate::test_support::TestEnvGuard::temp_home();
        let directory = tempfile::tempdir().unwrap();
        let decision = cas_factory::resolve_lane("taste", &CapabilitySnapshot::default()).unwrap();
        let args = FactoryArgs {
            workers: 1,
            worker_spec: vec![serde_json::to_string(&decision.spec).unwrap()],
            ..FactoryArgs::default()
        };
        let required = required_harnesses(&args, directory.path()).unwrap();
        assert!(required.contains(&SupervisorCli::Codex));
    }

    #[test]
    fn doctor_rejects_astra_outside_taste_effort() {
        let error =
            doctor_routing_error(r#"{"cli":"codex","model":"gpt-6-astra","effort":"high"}"#);
        assert!(error.contains("allowed efforts are medium"), "{error}");
    }

    #[test]
    fn doctor_rejects_suspended_terra_with_registry_alternatives() {
        let error =
            doctor_routing_error(r#"{"cli":"codex","model":"gpt-5.6-terra","effort":"xhigh"}"#);
        assert!(error.contains("Terra is suspended"), "{error}");
        assert!(error.contains("routing rule 'suspended recipe'"), "{error}");
        assert!(error.contains("codex_luna"), "{error}");
    }

    #[test]
    fn doctor_rejects_luna_high_with_registry_alternatives() {
        let error =
            doctor_routing_error(r#"{"cli":"codex","model":"gpt-5.6-luna","effort":"high"}"#);
        assert!(error.contains("Luna is only permitted"), "{error}");
        assert!(error.contains("routing rule 'allowed effort'"), "{error}");
        assert!(error.contains("codex_luna"), "{error}");
        assert!(error.contains("effort=xhigh"), "{error}");
    }
}
