//! Bounded, read-only factory readiness report shared by CLI and MCP.
//!
//! The collector deliberately consumes existing typed evidence. It never
//! connects optional upstreams, launches a harness model turn, or spawns a
//! factory worker.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cas_pty::{
    ConformanceStatus, Harness, HarnessConformanceReceipt, harness_conformance_receipts,
};
use serde::{Deserialize, Serialize};

const SCHEMA_VERSION: u32 = 1;
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const REQUIRED_CAS_TOOLS: [&str; 2] = ["coordination", "task"];
static VERSION_OUTPUT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreflightOverall {
    Ready,
    Warn,
    Critical,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreflightSeverity {
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComponentState {
    Ready,
    Stale,
    Degraded,
    Missing,
    Critical,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PreflightFinding {
    pub code: String,
    pub severity: PreflightSeverity,
    pub component: String,
    pub message: String,
    pub remediation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_evidence_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct BinaryPreflight {
    pub state: ComponentState,
    pub running_deployment_sha: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configured_deployment_sha: Option<String>,
    pub build_date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RepositoryPreflight {
    pub state: ComponentState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CasMcpPreflight {
    pub state: ComponentState,
    pub cas_initialized: bool,
    pub configured: bool,
    pub observed_via_mcp: bool,
    pub registered_tools: Vec<String>,
    pub required_tools: Vec<String>,
    pub missing_required_tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct OptionalUpstreamPreflight {
    pub name: String,
    pub transport: String,
    pub state: String,
    pub attempts: u32,
    pub consecutive_failures: u32,
    pub tool_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attempt_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_retry_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct OptionalUpstreamsPreflight {
    pub state: ComponentState,
    pub configured: usize,
    pub healthy: usize,
    pub degraded: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_at_ms: Option<u64>,
    pub servers: Vec<OptionalUpstreamPreflight>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct HarnessPreflight {
    pub harness: String,
    pub state: ComponentState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validated_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validated_at: Option<String>,
    pub evidence_refs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FactoryPreflightReport {
    pub schema_version: u32,
    pub generated_at: String,
    pub overall: PreflightOverall,
    pub factory_blocked: bool,
    pub runtime_bound_ms: u64,
    pub binary: BinaryPreflight,
    pub repository: RepositoryPreflight,
    pub cas_mcp: CasMcpPreflight,
    pub optional_upstreams: OptionalUpstreamsPreflight,
    pub harnesses: Vec<HarnessPreflight>,
    pub findings: Vec<PreflightFinding>,
}

#[derive(Debug, Clone)]
pub struct ProxySnapshotInput {
    pub generated_at_ms: u64,
    pub healthy: usize,
    pub degraded: usize,
    pub servers: Vec<OptionalUpstreamPreflight>,
}

#[cfg(feature = "mcp-proxy")]
impl From<cmcp_core::ProxyHealthSnapshot> for ProxySnapshotInput {
    fn from(snapshot: cmcp_core::ProxyHealthSnapshot) -> Self {
        Self {
            generated_at_ms: snapshot.generated_at_ms,
            healthy: snapshot.healthy,
            degraded: snapshot.degraded,
            servers: snapshot
                .servers
                .into_iter()
                .map(|server| OptionalUpstreamPreflight {
                    name: server.name,
                    transport: server.transport,
                    state: format!("{:?}", server.state).to_ascii_lowercase(),
                    attempts: server.attempts,
                    consecutive_failures: server.consecutive_failures,
                    tool_count: server.tool_count,
                    last_error_code: server.last_error_code,
                    last_attempt_at_ms: server.last_attempt_at_ms,
                    next_retry_at_ms: server.next_retry_at_ms,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
struct BinaryFacts {
    running_sha: String,
    source_sha: Option<String>,
    configured_sha: Option<String>,
    configured_sha_invalid: bool,
    build_date: String,
}

#[derive(Debug, Clone)]
struct RepositoryFacts {
    selector: String,
    target_branch: String,
}

#[derive(Debug, Clone, Copy)]
enum RepositoryFailure {
    Missing,
    Wrong,
    Ambiguous,
}

#[derive(Debug, Clone)]
struct McpFacts {
    cas_initialized: bool,
    configured: bool,
    observed: bool,
    tools: Vec<String>,
}

#[derive(Debug, Clone)]
struct ProxyFacts {
    configured: usize,
    invalid_config: bool,
    snapshot: Option<ProxySnapshotInput>,
    invalid_snapshot: bool,
}

#[derive(Debug, Clone)]
struct PreflightFacts {
    binary: BinaryFacts,
    repository: Result<RepositoryFacts, RepositoryFailure>,
    mcp: McpFacts,
    proxy: ProxyFacts,
    receipts: Vec<HarnessConformanceReceipt>,
    default_versions: HashMap<Harness, String>,
}

/// Collect one bounded report. `live_proxy` must be an already-collected
/// in-memory snapshot; this function never connects an upstream.
pub fn collect_factory_preflight(
    project_root: &Path,
    cas_root: &Path,
    observed_via_mcp: bool,
    live_proxy: Option<ProxySnapshotInput>,
) -> FactoryPreflightReport {
    let default_versions = probe_default_harness_versions();
    let facts = PreflightFacts {
        binary: collect_binary_facts(project_root),
        repository: collect_repository_facts(project_root, cas_root),
        mcp: McpFacts {
            cas_initialized: cas_root.is_dir(),
            configured: cas_mcp_is_configured(project_root),
            observed: observed_via_mcp,
            tools: compiled_cas_tool_names(),
        },
        proxy: collect_proxy_facts(cas_root, live_proxy),
        receipts: harness_conformance_receipts().unwrap_or_default(),
        default_versions,
    };
    build_report(facts)
}

fn build_report(facts: PreflightFacts) -> FactoryPreflightReport {
    let mut findings = Vec::new();
    let binary = classify_binary(facts.binary, &mut findings);
    let repository = classify_repository(facts.repository, &mut findings);
    let cas_mcp = classify_mcp(facts.mcp, &mut findings);
    let optional_upstreams = classify_proxy(facts.proxy, &mut findings);
    let harnesses = classify_harnesses(facts.receipts, facts.default_versions, &mut findings);

    let factory_blocked = findings
        .iter()
        .any(|finding| finding.severity == PreflightSeverity::Critical);
    let overall = if factory_blocked {
        PreflightOverall::Critical
    } else if findings.is_empty() {
        PreflightOverall::Ready
    } else {
        PreflightOverall::Warn
    };

    FactoryPreflightReport {
        schema_version: SCHEMA_VERSION,
        generated_at: chrono::Utc::now().to_rfc3339(),
        overall,
        factory_blocked,
        runtime_bound_ms: VERSION_PROBE_TIMEOUT.as_millis() as u64 + 4_500,
        binary,
        repository,
        cas_mcp,
        optional_upstreams,
        harnesses,
        findings,
    }
}

fn classify_binary(facts: BinaryFacts, findings: &mut Vec<PreflightFinding>) -> BinaryPreflight {
    let running_clean = facts.running_sha.trim_end_matches("-dirty");
    let dirty = facts.running_sha.ends_with("-dirty");
    let expected = facts
        .configured_sha
        .as_deref()
        .or(facts.source_sha.as_deref());
    let matches = expected.is_some_and(|sha| sha.starts_with(running_clean));

    let (state, remediation) = if facts.configured_sha_invalid {
        let remediation =
            "Set CAS_EXPECTED_DEPLOYMENT_SHA to a 7-40 character hexadecimal commit SHA."
                .to_string();
        findings.push(warning(
            "binary.expected_sha_invalid",
            "binary",
            "Configured CAS deployment SHA is invalid.",
            &remediation,
            None,
        ));
        (ComponentState::Stale, Some(remediation))
    } else if running_clean == "unknown" || dirty {
        let remediation =
            "Rebuild CAS from a clean source checkout and restart `cas serve`.".to_string();
        findings.push(warning(
            "binary.identity_untrusted",
            "binary",
            "Running CAS binary identity is unknown or dirty.",
            &remediation,
            None,
        ));
        (ComponentState::Stale, Some(remediation))
    } else if expected.is_none() {
        let remediation = "Run preflight from the CAS source checkout, set CAS_SOURCE_DIR, or set CAS_EXPECTED_DEPLOYMENT_SHA."
            .to_string();
        findings.push(warning(
            "binary.source_evidence_missing",
            "binary",
            "No CAS source or configured deployment SHA is available for comparison.",
            &remediation,
            None,
        ));
        (ComponentState::Stale, Some(remediation))
    } else if !matches {
        let remediation =
            "Rebuild CAS from the expected source commit and restart `cas serve`.".to_string();
        findings.push(warning(
            "binary.deployment_stale",
            "binary",
            "Running CAS binary SHA differs from expected deployment/source SHA.",
            &remediation,
            None,
        ));
        (ComponentState::Stale, Some(remediation))
    } else {
        (ComponentState::Ready, None)
    };

    BinaryPreflight {
        state,
        running_deployment_sha: facts.running_sha,
        source_sha: facts.source_sha,
        configured_deployment_sha: facts.configured_sha,
        build_date: facts.build_date,
        remediation,
    }
}

fn classify_repository(
    facts: Result<RepositoryFacts, RepositoryFailure>,
    findings: &mut Vec<PreflightFinding>,
) -> RepositoryPreflight {
    match facts {
        Ok(facts) => RepositoryPreflight {
            state: ComponentState::Ready,
            selector: Some(facts.selector),
            target_branch: Some(facts.target_branch),
            remediation: None,
        },
        Err(reason) => {
            let (code, message, remediation) = match reason {
                RepositoryFailure::Missing => (
                    "repository.unresolved",
                    "Canonical repository identity or active branch cannot be resolved.",
                    "Run from the intended initialized Git checkout and ensure it has a canonical project ID or origin remote.",
                ),
                RepositoryFailure::Wrong => (
                    "repository.wrong",
                    "The active checkout and resolved CAS project identify different repositories.",
                    "Change to the intended project checkout and rerun preflight before spawning workers.",
                ),
                RepositoryFailure::Ambiguous => (
                    "repository.ambiguous",
                    "The canonical repository selector matches multiple host checkouts.",
                    "Remove or re-identify duplicate known-repo entries, then rerun preflight.",
                ),
            };
            findings.push(critical(code, "repository", message, remediation, None));
            RepositoryPreflight {
                state: ComponentState::Critical,
                selector: None,
                target_branch: None,
                remediation: Some(remediation.to_string()),
            }
        }
    }
}

fn classify_mcp(facts: McpFacts, findings: &mut Vec<PreflightFinding>) -> CasMcpPreflight {
    let missing: Vec<String> = REQUIRED_CAS_TOOLS
        .iter()
        .filter(|required| !facts.tools.iter().any(|tool| tool == **required))
        .map(|tool| (*tool).to_string())
        .collect();
    let available =
        facts.cas_initialized && (facts.observed || facts.configured) && missing.is_empty();
    let remediation = if available {
        None
    } else if !facts.cas_initialized {
        Some("Run `cas init` in the intended project, then rerun preflight.".to_string())
    } else if !missing.is_empty() {
        Some(
            "Rebuild/reinstall CAS with the complete MCP tool registry, then restart `cas serve`."
                .to_string(),
        )
    } else {
        Some("Run `cas init` to register `{ \"command\": \"cas\", \"args\": [\"serve\"] }`, then restart the harness.".to_string())
    };
    if !available {
        findings.push(critical(
            if !facts.cas_initialized {
                "cas_mcp.cas_not_initialized"
            } else if missing.is_empty() {
                "cas_mcp.registration_missing"
            } else {
                "cas_mcp.required_tools_missing"
            },
            "cas_mcp",
            if !facts.cas_initialized {
                "CAS is not initialized in the resolved project."
            } else if missing.is_empty() {
                "CAS MCP is neither live-observed nor correctly registered."
            } else {
                "The compiled CAS MCP registry is missing required coordination/task tools."
            },
            remediation.as_deref().unwrap_or_default(),
            None,
        ));
    }
    CasMcpPreflight {
        state: if available {
            ComponentState::Ready
        } else {
            ComponentState::Critical
        },
        cas_initialized: facts.cas_initialized,
        configured: facts.configured,
        observed_via_mcp: facts.observed,
        registered_tools: facts.tools,
        required_tools: REQUIRED_CAS_TOOLS
            .iter()
            .map(|tool| (*tool).to_string())
            .collect(),
        missing_required_tools: missing,
        remediation,
    }
}

fn classify_proxy(
    facts: ProxyFacts,
    findings: &mut Vec<PreflightFinding>,
) -> OptionalUpstreamsPreflight {
    if facts.invalid_config {
        let remediation =
            "Repair `.cas/proxy.toml`, then restart `cas serve` to refresh optional upstream health."
                .to_string();
        findings.push(warning(
            "optional_upstreams.config_invalid",
            "optional_upstreams",
            "Optional upstream configuration is invalid.",
            &remediation,
            None,
        ));
        return OptionalUpstreamsPreflight {
            state: ComponentState::Degraded,
            configured: facts.configured,
            healthy: 0,
            degraded: facts.configured,
            generated_at_ms: None,
            servers: Vec::new(),
            remediation: Some(remediation),
        };
    }

    let Some(snapshot) = facts.snapshot else {
        if facts.configured == 0 && !facts.invalid_snapshot {
            return OptionalUpstreamsPreflight {
                state: ComponentState::Ready,
                configured: 0,
                healthy: 0,
                degraded: 0,
                generated_at_ms: None,
                servers: Vec::new(),
                remediation: None,
            };
        }
        let remediation =
            "Restart `cas serve` to regenerate credential-free optional-upstream health."
                .to_string();
        findings.push(warning(
            if facts.invalid_snapshot {
                "optional_upstreams.health_invalid"
            } else {
                "optional_upstreams.health_missing"
            },
            "optional_upstreams",
            "Optional upstream health evidence is unavailable.",
            &remediation,
            None,
        ));
        return OptionalUpstreamsPreflight {
            state: ComponentState::Degraded,
            configured: facts.configured,
            healthy: 0,
            degraded: facts.configured,
            generated_at_ms: None,
            servers: Vec::new(),
            remediation: Some(remediation),
        };
    };

    let degraded = snapshot.degraded > 0;
    let remediation = degraded.then(|| {
        "Inspect error codes/backoff due times and repair the optional upstream; factory launch may continue."
            .to_string()
    });
    if degraded {
        findings.push(warning(
            "optional_upstreams.degraded",
            "optional_upstreams",
            "One or more optional upstream MCP servers are degraded or backing off.",
            remediation.as_deref().unwrap_or_default(),
            Some(snapshot.generated_at_ms.to_string()),
        ));
    }
    OptionalUpstreamsPreflight {
        state: if degraded {
            ComponentState::Degraded
        } else {
            ComponentState::Ready
        },
        configured: facts.configured.max(snapshot.servers.len()),
        healthy: snapshot.healthy,
        degraded: snapshot.degraded,
        generated_at_ms: Some(snapshot.generated_at_ms),
        servers: snapshot.servers,
        remediation,
    }
}

fn classify_harnesses(
    receipts: Vec<HarnessConformanceReceipt>,
    default_versions: HashMap<Harness, String>,
    findings: &mut Vec<PreflightFinding>,
) -> Vec<HarnessPreflight> {
    [Harness::ClaudeCode, Harness::CodexCli, Harness::GrokBuild]
        .into_iter()
        .map(|harness| {
            let receipt = receipts.iter().find(|receipt| receipt.harness == harness);
            let default_version = default_versions.get(&harness).cloned().or_else(|| {
                receipt.and_then(|receipt| receipt.observed_default_harness_version.clone())
            });
            let harness_name = harness_name(harness).to_string();
            let Some(receipt) = receipt else {
                let remediation = format!(
                    "Run and persist the typed {harness_name} factory conformance matrix."
                );
                findings.push(warning(
                    "harness.receipt_missing",
                    &format!("harness.{harness_name}"),
                    &format!("No typed {harness_name} conformance receipt is available."),
                    &remediation,
                    None,
                ));
                return HarnessPreflight {
                    harness: harness_name,
                    state: ComponentState::Missing,
                    validated_version: None,
                    default_version,
                    receipt_id: None,
                    receipt_result: None,
                    validated_at: None,
                    evidence_refs: Vec::new(),
                    remediation: Some(remediation),
                };
            };

            let validates = receipt.validates_pin();
            let drift = default_version
                .as_ref()
                .is_some_and(|default| default != &receipt.harness_version);
            let default_missing = default_version.is_none();
            let mut state = ComponentState::Ready;
            let mut remediation = None;
            if !validates {
                state = ComponentState::Stale;
                let action = format!(
                    "Repair failed required checks and rerun the typed {harness_name} conformance matrix."
                );
                findings.push(warning(
                    "harness.validation_failed",
                    &format!("harness.{harness_name}"),
                    &format!("{harness_name} receipt does not pass every required check."),
                    &action,
                    Some(receipt.validated_at.clone()),
                ));
                remediation = Some(action);
            } else if drift {
                state = ComponentState::Stale;
                let action = format!(
                    "Use validated {harness_name} {} or rerun the full matrix for the current default before updating the pin.",
                    receipt.harness_version
                );
                findings.push(warning(
                    "harness.version_drift",
                    &format!("harness.{harness_name}"),
                    &format!(
                        "{harness_name} default version differs from validated version {}.",
                        receipt.harness_version
                    ),
                    &action,
                    Some(receipt.validated_at.clone()),
                ));
                remediation = Some(action);
            } else if default_missing {
                state = ComponentState::Stale;
                let action =
                    format!("Install {harness_name} or make its default binary available on PATH.");
                findings.push(warning(
                    "harness.default_unavailable",
                    &format!("harness.{harness_name}"),
                    &format!("{harness_name} default version could not be observed."),
                    &action,
                    Some(receipt.validated_at.clone()),
                ));
                remediation = Some(action);
            }

            HarnessPreflight {
                harness: harness_name,
                state,
                validated_version: Some(receipt.harness_version.clone()),
                default_version,
                receipt_id: Some(receipt.receipt_id.clone()),
                receipt_result: Some(match receipt.result {
                    ConformanceStatus::Pass => "pass",
                    ConformanceStatus::Fail => "fail",
                }
                .to_string()),
                validated_at: Some(receipt.validated_at.clone()),
                evidence_refs: receipt
                    .evidence
                    .iter()
                    .map(|evidence| evidence.reference.clone())
                    .collect(),
                remediation,
            }
        })
        .collect()
}

fn warning(
    code: &str,
    component: &str,
    message: &str,
    remediation: &str,
    last_evidence_at: Option<String>,
) -> PreflightFinding {
    PreflightFinding {
        code: code.to_string(),
        severity: PreflightSeverity::Warning,
        component: component.to_string(),
        message: message.to_string(),
        remediation: remediation.to_string(),
        last_evidence_at,
    }
}

fn critical(
    code: &str,
    component: &str,
    message: &str,
    remediation: &str,
    last_evidence_at: Option<String>,
) -> PreflightFinding {
    PreflightFinding {
        code: code.to_string(),
        severity: PreflightSeverity::Critical,
        component: component.to_string(),
        message: message.to_string(),
        remediation: remediation.to_string(),
        last_evidence_at,
    }
}

fn harness_name(harness: Harness) -> &'static str {
    match harness {
        Harness::ClaudeCode => "claude",
        Harness::CodexCli => "codex",
        Harness::GrokBuild => "grok",
    }
}

fn collect_binary_facts(project_root: &Path) -> BinaryFacts {
    let configured_raw = std::env::var("CAS_EXPECTED_DEPLOYMENT_SHA").ok();
    let configured_sha = configured_raw
        .as_deref()
        .filter(|sha| valid_sha(sha))
        .map(ToOwned::to_owned);
    let configured_sha_invalid = configured_raw.is_some() && configured_sha.is_none();
    let source_root = std::env::var_os("CAS_SOURCE_DIR")
        .map(PathBuf::from)
        .or_else(|| is_cas_source_checkout(project_root).then(|| project_root.to_path_buf()));
    let source_sha = source_root
        .as_deref()
        .and_then(|root| git_output(root, &["rev-parse", "HEAD"]))
        .filter(|sha| valid_sha(sha));
    BinaryFacts {
        running_sha: option_env!("CAS_GIT_HASH").unwrap_or("unknown").to_string(),
        source_sha,
        configured_sha,
        configured_sha_invalid,
        build_date: option_env!("CAS_BUILD_DATE")
            .unwrap_or("unknown")
            .to_string(),
    }
}

fn valid_sha(value: &str) -> bool {
    (7..=40).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_cas_source_checkout(path: &Path) -> bool {
    std::fs::read_to_string(path.join("cas-cli/Cargo.toml"))
        .ok()
        .is_some_and(|manifest| manifest.lines().any(|line| line.trim() == "name = \"cas\""))
}

fn git_output(path: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(feature = "mcp-server")]
fn collect_repository_facts(
    project_root: &Path,
    cas_root: &Path,
) -> Result<RepositoryFacts, RepositoryFailure> {
    use cas_types::WorkTarget;

    let branch = git_output(project_root, &["symbolic-ref", "--short", "HEAD"])
        .filter(|branch| !branch.is_empty())
        .ok_or(RepositoryFailure::Missing)?;
    let active =
        crate::mcp::tools::core::task::repo_context::resolve_path_context(project_root, &branch)
            .map_err(|_| RepositoryFailure::Missing)?;
    let cas_project = cas_root.parent().unwrap_or(cas_root);
    let configured =
        crate::mcp::tools::core::task::repo_context::resolve_path_context(cas_project, &branch)
            .map_err(|_| RepositoryFailure::Missing)?;
    if active.repo_selector != configured.repo_selector {
        return Err(RepositoryFailure::Wrong);
    }
    let target = WorkTarget {
        repo_selector: active.repo_selector.clone(),
        target_branch: branch.clone(),
    };
    match crate::mcp::tools::core::task::repo_context::resolve_repo_context(cas_root, &target) {
        Ok(context) if context.repo_selector == active.repo_selector => Ok(RepositoryFacts {
            selector: context.repo_selector,
            target_branch: branch,
        }),
        Ok(_) => Err(RepositoryFailure::Wrong),
        Err(reason) if reason.contains("AMBIGUOUS WORK TARGET") => {
            Err(RepositoryFailure::Ambiguous)
        }
        Err(_) => Err(RepositoryFailure::Wrong),
    }
}

#[cfg(not(feature = "mcp-server"))]
fn collect_repository_facts(
    _project_root: &Path,
    _cas_root: &Path,
) -> Result<RepositoryFacts, RepositoryFailure> {
    Err(RepositoryFailure::Missing)
}

fn cas_mcp_is_configured(project_root: &Path) -> bool {
    let config = std::fs::read_to_string(project_root.join(".mcp.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
    let Some(config) = config else {
        return false;
    };
    let command_ok = config
        .pointer("/mcpServers/cas/command")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|command| {
            Path::new(command)
                .file_name()
                .is_some_and(|name| name == "cas")
        });
    let serve_arg = config
        .pointer("/mcpServers/cas/args")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|args| args.iter().any(|arg| arg.as_str() == Some("serve")));
    command_ok && serve_arg
}

#[cfg(feature = "mcp-server")]
fn compiled_cas_tool_names() -> Vec<String> {
    crate::mcp::CasService::registered_tool_names_for_build()
}

#[cfg(not(feature = "mcp-server"))]
fn compiled_cas_tool_names() -> Vec<String> {
    Vec::new()
}

fn collect_proxy_facts(cas_root: &Path, live: Option<ProxySnapshotInput>) -> ProxyFacts {
    #[cfg(feature = "mcp-proxy")]
    {
        let proxy_path = cas_root.join("proxy.toml");
        let (configured, invalid_config) = if proxy_path.exists() {
            match cmcp_core::config::Config::load_from(&proxy_path) {
                Ok(config) => (config.servers.len(), false),
                Err(_) => (0, true),
            }
        } else {
            (0, false)
        };
        if live.is_some() {
            return ProxyFacts {
                configured,
                invalid_config,
                snapshot: live,
                invalid_snapshot: false,
            };
        }
        let health_path = cas_root.join("proxy_health.json");
        let (snapshot, invalid_snapshot) = if health_path.exists() {
            match std::fs::read(&health_path)
                .ok()
                .and_then(|raw| serde_json::from_slice::<cmcp_core::ProxyHealthSnapshot>(&raw).ok())
            {
                Some(snapshot) => (Some(snapshot.into()), false),
                None => (None, true),
            }
        } else {
            (None, false)
        };
        ProxyFacts {
            configured,
            invalid_config,
            snapshot,
            invalid_snapshot,
        }
    }
    #[cfg(not(feature = "mcp-proxy"))]
    {
        let _ = (cas_root, live);
        ProxyFacts {
            configured: 0,
            invalid_config: false,
            snapshot: None,
            invalid_snapshot: false,
        }
    }
}

fn probe_default_harness_versions() -> HashMap<Harness, String> {
    std::thread::scope(|scope| {
        let claude = scope.spawn(|| probe_version("claude", VERSION_PROBE_TIMEOUT));
        let codex = scope.spawn(|| probe_version("codex", VERSION_PROBE_TIMEOUT));
        let grok = scope.spawn(|| probe_version("grok", VERSION_PROBE_TIMEOUT));
        [
            (Harness::ClaudeCode, claude.join().ok().flatten()),
            (Harness::CodexCli, codex.join().ok().flatten()),
            (Harness::GrokBuild, grok.join().ok().flatten()),
        ]
        .into_iter()
        .filter_map(|(harness, version)| version.map(|version| (harness, version)))
        .collect()
    })
}

fn probe_version(program: &str, timeout: Duration) -> Option<String> {
    probe_command_version(program, &["--version"], timeout)
}

fn probe_command_version(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let capture = VersionCapture::new()?;
    let output_file = OpenOptions::new().append(true).open(&capture.path).ok()?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        // A regular file keeps a descendant that inherits stdout from holding
        // a pipe open after the bounded parent has exited.
        .stdout(Stdio::from(output_file))
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let output = std::fs::read_to_string(&capture.path).ok()?;
                return parse_version(&output);
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

struct VersionCapture {
    path: PathBuf,
}

impl VersionCapture {
    fn new() -> Option<Self> {
        for _ in 0..8 {
            let sequence = VERSION_OUTPUT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                ".cas-preflight-version-{}-{sequence}",
                std::process::id()
            ));
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            if options.open(&path).is_ok() {
                return Some(Self { path });
            }
        }
        None
    }
}

impl Drop for VersionCapture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn parse_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .map(|token| token.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '.'))
        .find(|token| {
            token.contains('.')
                && token.chars().any(|ch| ch.is_ascii_digit())
                && token
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-')
        })
        .map(ToOwned::to_owned)
}

/// Concise human projection of the stable report.
pub fn render_factory_preflight_human(report: &FactoryPreflightReport) -> String {
    let mut lines = vec![format!(
        "Factory preflight: {}{}",
        overall_label(report.overall),
        if report.factory_blocked {
            " (factory blocked)"
        } else {
            ""
        }
    )];
    lines.push(format!(
        "  binary: {} running={} source={}",
        state_label(report.binary.state),
        report.binary.running_deployment_sha,
        report.binary.source_sha.as_deref().unwrap_or("unavailable")
    ));
    lines.push(format!(
        "  repository: {} selector={} branch={}",
        state_label(report.repository.state),
        report
            .repository
            .selector
            .as_deref()
            .unwrap_or("unresolved"),
        report
            .repository
            .target_branch
            .as_deref()
            .unwrap_or("unresolved")
    ));
    lines.push(format!(
        "  cas mcp: {} configured={} observed={} tools={}",
        state_label(report.cas_mcp.state),
        report.cas_mcp.configured,
        report.cas_mcp.observed_via_mcp,
        report.cas_mcp.registered_tools.len()
    ));
    lines.push(format!(
        "  optional upstreams: {} healthy={} degraded={}",
        state_label(report.optional_upstreams.state),
        report.optional_upstreams.healthy,
        report.optional_upstreams.degraded
    ));
    for harness in &report.harnesses {
        lines.push(format!(
            "  {}: {} validated={} default={} evidence={}",
            harness.harness,
            state_label(harness.state),
            harness.validated_version.as_deref().unwrap_or("none"),
            harness.default_version.as_deref().unwrap_or("unavailable"),
            harness.validated_at.as_deref().unwrap_or("none")
        ));
    }
    if !report.findings.is_empty() {
        lines.push("Remediation:".to_string());
        lines.extend(report.findings.iter().map(|finding| {
            format!(
                "  - [{}] {}: {}",
                finding.code, finding.message, finding.remediation
            )
        }));
    }
    lines.join("\n")
}

fn overall_label(overall: PreflightOverall) -> &'static str {
    match overall {
        PreflightOverall::Ready => "READY",
        PreflightOverall::Warn => "WARN",
        PreflightOverall::Critical => "CRITICAL",
    }
}

fn state_label(state: ComponentState) -> &'static str {
    match state {
        ComponentState::Ready => "ready",
        ComponentState::Stale => "stale",
        ComponentState::Degraded => "degraded",
        ComponentState::Missing => "missing",
        ComponentState::Critical => "critical",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(harness: Harness, version: &str) -> HarnessConformanceReceipt {
        HarnessConformanceReceipt {
            schema_version: 1,
            receipt_id: format!("{}-{version}", harness_name(harness)),
            harness,
            harness_version: version.to_string(),
            observed_default_harness_version: Some(version.to_string()),
            validated_at: "2026-07-30".to_string(),
            result: ConformanceStatus::Pass,
            checklist: vec![cas_pty::ConformanceCheck {
                id: "required".to_string(),
                required: true,
                status: ConformanceStatus::Pass,
                evidence_refs: vec!["evidence".to_string()],
                detail: "passed".to_string(),
            }],
            evidence: vec![cas_pty::ConformanceEvidence {
                id: "evidence".to_string(),
                kind: "test".to_string(),
                reference: "test:factory-preflight".to_string(),
                summary: "passed".to_string(),
            }],
        }
    }

    fn healthy_facts() -> PreflightFacts {
        let receipts = vec![
            receipt(Harness::ClaudeCode, "2.1.0"),
            receipt(Harness::CodexCli, "0.146.0"),
            receipt(Harness::GrokBuild, "0.2.114"),
        ];
        let default_versions = receipts
            .iter()
            .map(|receipt| (receipt.harness, receipt.harness_version.clone()))
            .collect();
        PreflightFacts {
            binary: BinaryFacts {
                running_sha: "abcdef0".to_string(),
                source_sha: Some("abcdef0123456789".to_string()),
                configured_sha: None,
                configured_sha_invalid: false,
                build_date: "2026-07-30".to_string(),
            },
            repository: Ok(RepositoryFacts {
                selector: "project:example".to_string(),
                target_branch: "main".to_string(),
            }),
            mcp: McpFacts {
                cas_initialized: true,
                configured: true,
                observed: false,
                tools: vec!["coordination".to_string(), "task".to_string()],
            },
            proxy: ProxyFacts {
                configured: 0,
                invalid_config: false,
                snapshot: None,
                invalid_snapshot: false,
            },
            receipts,
            default_versions,
        }
    }

    #[test]
    fn healthy_report_is_ready_and_unblocked() {
        let report = build_report(healthy_facts());
        assert_eq!(report.schema_version, 1);
        assert_eq!(report.overall, PreflightOverall::Ready);
        assert!(!report.factory_blocked);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn stale_or_dirty_binary_warns_without_blocking() {
        let mut facts = healthy_facts();
        facts.binary.running_sha = "abcdef0-dirty".to_string();
        let report = build_report(facts);
        assert_eq!(report.binary.state, ComponentState::Stale);
        assert_eq!(report.overall, PreflightOverall::Warn);
        assert!(!report.factory_blocked);
    }

    #[test]
    fn wrong_or_ambiguous_repository_is_critical() {
        for failure in [RepositoryFailure::Wrong, RepositoryFailure::Ambiguous] {
            let mut facts = healthy_facts();
            facts.repository = Err(failure);
            let report = build_report(facts);
            assert_eq!(report.repository.state, ComponentState::Critical);
            assert!(report.factory_blocked);
        }
    }

    #[test]
    fn missing_registration_is_critical_for_cli_but_live_mcp_observation_is_sufficient() {
        let mut cli = healthy_facts();
        cli.mcp.configured = false;
        let cli_report = build_report(cli);
        assert!(cli_report.factory_blocked);

        let mut mcp = healthy_facts();
        mcp.mcp.configured = false;
        mcp.mcp.observed = true;
        let mcp_report = build_report(mcp);
        assert_eq!(mcp_report.cas_mcp.state, ComponentState::Ready);
        assert!(!mcp_report.factory_blocked);
    }

    #[test]
    fn missing_required_cas_tool_is_critical_even_when_observed() {
        let mut facts = healthy_facts();
        facts.mcp.observed = true;
        facts.mcp.tools = vec!["system".to_string()];
        let report = build_report(facts);
        assert!(report.factory_blocked);
        assert_eq!(
            report.cas_mcp.missing_required_tools,
            vec!["coordination", "task"]
        );
    }

    #[test]
    fn optional_degradation_is_redacted_warning_and_never_blocks() {
        let mut facts = healthy_facts();
        facts.proxy = ProxyFacts {
            configured: 1,
            invalid_config: false,
            invalid_snapshot: false,
            snapshot: Some(ProxySnapshotInput {
                generated_at_ms: 42,
                healthy: 0,
                degraded: 1,
                servers: vec![OptionalUpstreamPreflight {
                    name: "optional".to_string(),
                    transport: "http".to_string(),
                    state: "backoff".to_string(),
                    attempts: 2,
                    consecutive_failures: 2,
                    tool_count: 0,
                    last_error_code: Some("authentication_required".to_string()),
                    last_attempt_at_ms: Some(40),
                    next_retry_at_ms: Some(50),
                }],
            }),
        };
        let report = build_report(facts);
        assert_eq!(report.optional_upstreams.state, ComponentState::Degraded);
        assert!(!report.factory_blocked);
        let json = serde_json::to_string(&report).unwrap();
        for forbidden in ["https://", "Bearer ", "token=", "/home/"] {
            assert!(!json.contains(forbidden), "{forbidden} leaked: {json}");
        }
    }

    #[test]
    fn validated_default_version_drift_is_stale_warning_not_blocker() {
        let mut facts = healthy_facts();
        facts
            .default_versions
            .insert(Harness::GrokBuild, "0.2.117".to_string());
        let report = build_report(facts);
        let grok = report
            .harnesses
            .iter()
            .find(|harness| harness.harness == "grok")
            .unwrap();
        assert_eq!(grok.validated_version.as_deref(), Some("0.2.114"));
        assert_eq!(grok.default_version.as_deref(), Some("0.2.117"));
        assert_eq!(grok.state, ComponentState::Stale);
        assert!(!report.factory_blocked);
    }

    #[test]
    fn downstream_project_head_is_not_used_as_cas_source_sha() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("Cargo.toml"), "[package]\nname='app'\n").unwrap();
        let facts = collect_binary_facts(temp.path());
        assert_eq!(facts.source_sha, None);
    }

    #[cfg(feature = "mcp-server")]
    #[test]
    fn repository_collection_keeps_host_home_separate_from_project_cas_root() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(project.path())
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .env("GIT_CONFIG_SYSTEM", "/dev/null")
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&[
            "remote",
            "add",
            "origin",
            "git@example.invalid:org/preflight-boundary.git",
        ]);
        std::fs::create_dir(project.path().join(".cas")).unwrap();
        std::fs::write(
            project.path().join(".cas/config.toml"),
            "[project]\ncanonical_id = \"preflight-boundary\"\n",
        )
        .unwrap();

        let active = crate::mcp::tools::core::task::repo_context::resolve_path_context(
            project.path(),
            "main",
        )
        .unwrap();
        let configured = crate::mcp::tools::core::task::repo_context::resolve_path_context(
            project.path(),
            "main",
        )
        .unwrap();
        assert_eq!(active.repo_selector, "project:preflight-boundary");
        assert_eq!(active.repo_selector, configured.repo_selector);
        let resolved = crate::mcp::tools::core::task::repo_context::resolve_repo_context(
            &project.path().join(".cas"),
            &cas_types::WorkTarget {
                repo_selector: active.repo_selector.clone(),
                target_branch: "main".to_string(),
            },
        );
        assert!(resolved.is_ok(), "direct resolution failed: {resolved:?}");

        let facts = collect_repository_facts(project.path(), &project.path().join(".cas"));
        assert!(
            matches!(facts, Ok(RepositoryFacts { ref selector, ref target_branch })
                if selector == "project:preflight-boundary" && target_branch == "main"),
            "unexpected repository facts: {facts:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_version_probe_is_killed_within_bound() {
        let started = Instant::now();
        let result = probe_command_version("sh", &["-c", "sleep 10"], Duration::from_millis(75));
        assert_eq!(result, None);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "probe exceeded bound: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn human_projection_is_concise_and_uses_same_findings() {
        let mut facts = healthy_facts();
        facts.binary.running_sha = "unknown".to_string();
        let report = build_report(facts);
        let human = render_factory_preflight_human(&report);
        assert!(human.starts_with("Factory preflight: WARN"));
        assert!(human.contains("binary.identity_untrusted"));
        assert!(!human.contains("/home/"));
    }
}
