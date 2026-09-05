//! Bounded, read-only factory readiness report shared by CLI and MCP.
//!
//! The collector deliberately consumes existing typed evidence. It never
//! connects optional upstreams, launches a harness model turn, or spawns a
//! factory worker.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use cas_pty::{
    ConformanceStatus, Harness, HarnessConformanceReceipt, harness_conformance_receipts,
};
use serde::{Deserialize, Serialize};

use crate::bounded_process::{BoundedCommandError, Deadline, run_command};
use crate::cloud::{QueueHealth, SyncQueue};
use crate::config::Config;

const SCHEMA_VERSION: u32 = 2;
const RUNTIME_BOUND: Duration = Duration::from_millis(6_500);
const COLLECTION_BUDGET: Duration = Duration::from_millis(6_000);
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const GIT_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const KNOWN_REPO_LIMIT: usize = 256;
const REQUIRED_CAS_TOOLS: [&str; 2] = ["coordination", "task"];

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

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeState {
    Observed,
    Unavailable,
    TimedOut,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
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
    pub required: bool,
    pub state: ComponentState,
    pub capability: cas_factory::CapabilityAvailability,
    pub capability_stale: bool,
    pub capability_observed_at_ms: Option<u64>,
    pub capability_expires_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validated_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_version: Option<String>,
    pub default_probe: ProbeState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_observed_default_version: Option<String>,
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
    pub runtime_elapsed_ms: u64,
    pub timed_out_components: Vec<String>,
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
        let snapshot = snapshot.sanitized();
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
                    state: match server.state {
                        cmcp_core::UpstreamState::ExecutableMissing => {
                            "executable_missing".to_string()
                        }
                        state => format!("{:?}", state).to_ascii_lowercase(),
                    },
                    executable: server.executable,
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
    source_probe_timed_out: bool,
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
    StaleBinding,
    TimedOut,
    CandidateLimit,
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
    evidence_failure: Option<ProxyEvidenceFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProxyEvidenceFailure {
    Missing,
    Invalid,
    Stale,
    Unavailable,
}

#[derive(Debug, Clone)]
struct PreflightFacts {
    binary: BinaryFacts,
    repository: Result<RepositoryFacts, RepositoryFailure>,
    mcp: McpFacts,
    proxy: ProxyFacts,
    receipts: Vec<HarnessConformanceReceipt>,
    default_versions: HashMap<Harness, VersionProbe>,
    required_harnesses: HashSet<Harness>,
    capability_snapshot: cas_factory::CapabilitySnapshot,
    cloud_queue: CloudQueueFacts,
    runtime_elapsed_ms: u64,
}

#[derive(Debug, Clone)]
struct CloudQueueFacts {
    health: Option<QueueHealth>,
    pending_warning: usize,
    oldest_warning_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VersionProbe {
    Observed(String),
    Unavailable,
    TimedOut,
}

/// Collect one bounded report. `live_proxy` must be an already-collected
/// in-memory snapshot; this function never connects an upstream.
pub fn collect_factory_preflight(
    invocation_root: &Path,
    cas_root: &Path,
    observed_via_mcp: bool,
    live_proxy: Option<ProxySnapshotInput>,
) -> FactoryPreflightReport {
    let started = Instant::now();
    let deadline = Deadline::after(COLLECTION_BUDGET);
    // `cas_root` is resolved by the CLI/MCP boundary and is authoritative for
    // project-scoped evidence. `invocation_root` remains separate only for the
    // repo-binding check, so an explicit --cas-root for another checkout fails
    // closed instead of normalizing the mismatch away.
    let project_root = cas_root.parent().unwrap_or(invocation_root);
    let repo_probe = crate::mcp::tools::core::task::repo_context::BoundedRepoProbe::new(
        deadline,
        GIT_PROBE_TIMEOUT,
        KNOWN_REPO_LIMIT,
    );
    let default_versions = probe_default_harness_versions(deadline);
    let receipts = harness_conformance_receipts().unwrap_or_default();
    let facts = PreflightFacts {
        binary: collect_binary_facts(project_root, &repo_probe),
        repository: collect_repository_facts(invocation_root, cas_root, &repo_probe),
        mcp: McpFacts {
            cas_initialized: cas_root.is_dir(),
            configured: cas_mcp_is_configured(project_root),
            observed: observed_via_mcp,
            tools: compiled_cas_tool_names(),
        },
        proxy: collect_proxy_facts(cas_root, live_proxy),
        capability_snapshot: collect_capability_snapshot(&default_versions, &receipts, deadline),
        receipts,
        default_versions,
        required_harnesses: required_harnesses(project_root),
        cloud_queue: collect_cloud_queue_facts(cas_root),
        runtime_elapsed_ms: started.elapsed().as_millis() as u64,
    };
    build_report(facts)
}

fn build_report(facts: PreflightFacts) -> FactoryPreflightReport {
    let mut findings = Vec::new();
    let mut timed_out_components = Vec::new();
    if facts.binary.source_probe_timed_out {
        timed_out_components.push("binary.source".to_string());
    }
    if matches!(&facts.repository, Err(RepositoryFailure::TimedOut)) {
        timed_out_components.push("repository".to_string());
    }
    timed_out_components.extend(
        facts
            .default_versions
            .iter()
            .filter_map(|(harness, probe)| {
                matches!(probe, VersionProbe::TimedOut)
                    .then(|| format!("harness.{}", harness_name(*harness)))
            }),
    );
    timed_out_components.sort();
    let binary = classify_binary(facts.binary, &mut findings);
    let repository = classify_repository(facts.repository, &mut findings);
    let cas_mcp = classify_mcp(facts.mcp, &mut findings);
    let optional_upstreams = classify_proxy(facts.proxy, &mut findings);
    classify_cloud_queue(facts.cloud_queue, &mut findings);
    let harnesses = classify_harnesses(
        facts.receipts,
        facts.default_versions,
        &facts.required_harnesses,
        &facts.capability_snapshot,
        &mut findings,
    );

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
        runtime_bound_ms: RUNTIME_BOUND.as_millis() as u64,
        runtime_elapsed_ms: facts.runtime_elapsed_ms,
        timed_out_components,
        binary,
        repository,
        cas_mcp,
        optional_upstreams,
        harnesses,
        findings,
    }
}

/// Read persisted queue evidence without creating a database or reaching the
/// cloud. Preflight remains a bounded, read-only local diagnostic.
fn collect_cloud_queue_facts(cas_root: &Path) -> CloudQueueFacts {
    let cloud = Config::load(cas_root)
        .unwrap_or_default()
        .cloud
        .unwrap_or_default();
    let health = if cas_root.join("cas.db").is_file() {
        SyncQueue::open_read_only(cas_root)
            .and_then(|queue| queue.health(cloud.max_retries.max(1), chrono::Utc::now()))
            .ok()
    } else {
        None
    };
    CloudQueueFacts {
        health,
        pending_warning: cloud.queue_pending_warning.max(1),
        oldest_warning_secs: cloud.queue_oldest_warning_secs.max(1),
    }
}

fn classify_cloud_queue(facts: CloudQueueFacts, findings: &mut Vec<PreflightFinding>) {
    let Some(health) = facts.health else {
        return;
    };
    let pending_wedged = health.pending >= facts.pending_warning;
    let age_wedged = health
        .oldest_age_secs
        .is_some_and(|age| age >= facts.oldest_warning_secs as i64);
    if !pending_wedged && !age_wedged && health.unreviewed_conflicts == 0 {
        return;
    }

    let oldest = health
        .oldest_age_secs
        .map(format_queue_age)
        .unwrap_or_else(|| "none".to_string());
    let last_error = health.last_error.as_deref().unwrap_or("none recorded");
    let message = format!(
        "Cloud sync queue warning: {} pending; oldest pending age {}; last push error: {}; unreviewed conflicts: {}.",
        health.pending, oldest, last_error, health.unreviewed_conflicts
    );
    findings.push(warning(
        "cloud.queue_unhealthy",
        "cloud_queue",
        &message,
        "Check `cas cloud queue` and the daemon logs; pending sync is a warning and does not block factory launch.",
        health.oldest_item.map(|at| at.to_rfc3339()),
    ));
}

fn format_queue_age(age_secs: i64) -> String {
    if age_secs >= 3600 {
        format!("{}h {}m", age_secs / 3600, (age_secs % 3600) / 60)
    } else if age_secs >= 60 {
        format!("{}m", age_secs / 60)
    } else {
        format!("{}s", age_secs)
    }
}

fn classify_binary(facts: BinaryFacts, findings: &mut Vec<PreflightFinding>) -> BinaryPreflight {
    let running_clean = facts.running_sha.trim_end_matches("-dirty");
    let dirty = facts.running_sha.ends_with("-dirty");
    let expected = facts
        .configured_sha
        .as_deref()
        .or(facts.source_sha.as_deref());
    let matches = expected.is_some_and(|sha| shas_compatible(sha, running_clean));

    let (state, remediation) = if facts.configured_sha_invalid {
        let remediation =
            "Set CAS_EXPECTED_DEPLOYMENT_SHA to a 7-40 character hexadecimal commit SHA."
                .to_string();
        findings.push(warning(
            "binary.expected_sha_invalid",
            "binary",
            "Configured Cassy deployment SHA is invalid.",
            &remediation,
            None,
        ));
        (ComponentState::Stale, Some(remediation))
    } else if running_clean == "unknown" || dirty {
        let remediation =
            "Rebuild Cassy from a clean source checkout and restart `cas serve`.".to_string();
        findings.push(warning(
            "binary.identity_untrusted",
            "binary",
            "Running Cassy binary identity is unknown or dirty.",
            &remediation,
            None,
        ));
        (ComponentState::Stale, Some(remediation))
    } else if facts.source_probe_timed_out && expected.is_none() {
        let remediation =
            "Retry preflight after local Git is responsive, or set CAS_EXPECTED_DEPLOYMENT_SHA."
                .to_string();
        findings.push(warning(
            "binary.source_probe_timed_out",
            "binary",
            "Cassy source identity did not complete within the shared preflight deadline.",
            &remediation,
            None,
        ));
        (ComponentState::Stale, Some(remediation))
    } else if expected.is_none() {
        let remediation = "Run preflight from the Cassy source checkout, set CAS_SOURCE_DIR, or set CAS_EXPECTED_DEPLOYMENT_SHA."
            .to_string();
        findings.push(warning(
            "binary.source_evidence_missing",
            "binary",
            "No Cassy source or configured deployment SHA is available for comparison.",
            &remediation,
            None,
        ));
        (ComponentState::Stale, Some(remediation))
    } else if !matches {
        let remediation =
            "Rebuild Cassy from the expected source commit and restart `cas serve`.".to_string();
        findings.push(warning(
            "binary.deployment_stale",
            "binary",
            "Running Cassy binary SHA differs from expected deployment/source SHA.",
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

fn shas_compatible(left: &str, right: &str) -> bool {
    valid_sha(left) && valid_sha(right) && (left.starts_with(right) || right.starts_with(left))
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
                    "The active checkout and resolved Cassy project identify different repositories.",
                    "Change to the intended project checkout and rerun preflight before spawning workers.",
                ),
                RepositoryFailure::Ambiguous => (
                    "repository.ambiguous",
                    "The canonical repository selector matches multiple host checkouts.",
                    "Run `cas known-repos status`, then `cas known-repos bind --repo <path>` to make an explicit host-local choice.",
                ),
                RepositoryFailure::StaleBinding => (
                    "repository.binding_stale",
                    "The explicit host-local repository binding no longer matches live Git identity.",
                    "Run `cas known-repos status`, explicitly unbind the stale selector, then bind the intended canonical repository root.",
                ),
                RepositoryFailure::TimedOut => (
                    "repository.probe_timed_out",
                    "Repository identity did not complete within the shared preflight deadline.",
                    "Make local Git and the host known-repo registry responsive, then rerun preflight.",
                ),
                RepositoryFailure::CandidateLimit => (
                    "repository.candidate_limit",
                    "The host known-repo registry exceeds the bounded preflight candidate limit.",
                    "Run `cas known-repos prune-missing --dry-run`, then `cas known-repos prune-missing`; rerun preflight afterward.",
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
            "Rebuild/reinstall Cassy with the complete MCP tool registry, then restart `cas serve`."
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
                "Cassy is not initialized in the resolved project."
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
        let remediation =
            "Restart `cas serve` to regenerate credential-free optional-upstream health."
                .to_string();
        let (code, summary) = match facts.evidence_failure {
            Some(ProxyEvidenceFailure::Invalid) => (
                "optional_upstreams.health_invalid",
                "Optional upstream health evidence is invalid.",
            ),
            Some(ProxyEvidenceFailure::Stale) => (
                "optional_upstreams.health_stale",
                "Optional upstream health evidence does not match current configuration.",
            ),
            Some(ProxyEvidenceFailure::Unavailable) => (
                "optional_upstreams.startup_unavailable",
                "Optional upstream startup did not produce usable health evidence.",
            ),
            _ => (
                "optional_upstreams.health_missing",
                "Optional upstream health evidence is unavailable.",
            ),
        };
        findings.push(warning(
            code,
            "optional_upstreams",
            summary,
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

fn registered_harnesses() -> Vec<Harness> {
    cas_factory::registered_harnesses()
        .expect("embedded lane registry must expose a valid harness catalog")
        .into_iter()
        .map(|harness| match harness {
            cas_mux::SupervisorCli::Claude => Harness::ClaudeCode,
            cas_mux::SupervisorCli::Codex => Harness::CodexCli,
            cas_mux::SupervisorCli::Grok => Harness::GrokBuild,
            cas_mux::SupervisorCli::OpenCode => Harness::OpenCode,
        })
        .collect()
}

fn supervisor_harness(harness: Harness) -> cas_mux::SupervisorCli {
    match harness {
        Harness::ClaudeCode => cas_mux::SupervisorCli::Claude,
        Harness::CodexCli => cas_mux::SupervisorCli::Codex,
        Harness::GrokBuild => cas_mux::SupervisorCli::Grok,
        Harness::OpenCode => cas_mux::SupervisorCli::OpenCode,
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
    default_versions: &HashMap<Harness, VersionProbe>,
    receipts: &[HarnessConformanceReceipt],
    deadline: Deadline,
) -> cas_factory::CapabilitySnapshot {
    let registry = cas_factory::registry()
        .expect("embedded lane registry must expose valid capability routes");
    let mut jobs = Vec::new();
    for harness in registered_harnesses() {
        let binary = match default_versions.get(&harness) {
            Some(VersionProbe::Observed(version)) => {
                crate::capability::BinaryObservation::Observed(version.clone())
            }
            Some(VersionProbe::TimedOut) => crate::capability::BinaryObservation::TimedOut,
            Some(VersionProbe::Unavailable) | None => {
                crate::capability::BinaryObservation::Unavailable
            }
        };
        let account_dir = account_dir_for_harness(harness);
        let receipt = receipts
            .iter()
            .find(|receipt| receipt.harness == harness)
            .cloned();
        let default_model = cas_factory::default_worker_model_for_cli(supervisor_harness(harness));
        let mut models = vec![default_model.to_string()];
        for recipe in registry.recipes.values() {
            if recipe.harness == supervisor_harness(harness)
                && !models.iter().any(|model| model == &recipe.model)
            {
                models.push(recipe.model.clone());
            }
        }
        for model in models {
            jobs.push((
                harness,
                model,
                account_dir.clone(),
                binary.clone(),
                receipt.clone(),
            ));
        }
    }

    let now_ms = cas_factory::CapabilitySnapshot::now_ms();
    let mut snapshot = cas_factory::CapabilitySnapshot::default();
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
            });
        for handle in handles {
            if let Ok((identity, evidence)) = handle.join() {
                snapshot.record(identity, evidence);
            }
        }
    });
    snapshot
}

/// Collect the current capability evidence used by an explicit lane spawn.
///
/// This is intentionally separate from session startup: lane selection is an
/// explicit factory operation, so it may use the same bounded provider probes
/// as `cas factory doctor`/`preflight` without turning ordinary MCP startup
/// into a network or credential probe.
pub fn collect_live_capability_snapshot() -> cas_factory::CapabilitySnapshot {
    let deadline = Deadline::after(COLLECTION_BUDGET);
    let default_versions = probe_default_harness_versions(deadline);
    let receipts = harness_conformance_receipts().unwrap_or_default();
    collect_capability_snapshot(&default_versions, &receipts, deadline)
}

fn classify_harnesses(
    receipts: Vec<HarnessConformanceReceipt>,
    default_versions: HashMap<Harness, VersionProbe>,
    required_harnesses: &HashSet<Harness>,
    capability_snapshot: &cas_factory::CapabilitySnapshot,
    findings: &mut Vec<PreflightFinding>,
) -> Vec<HarnessPreflight> {
    registered_harnesses()
        .into_iter()
        .map(|harness| {
            let receipt = receipts.iter().find(|receipt| receipt.harness == harness);
            let required = required_harnesses.contains(&harness);
            let probe = default_versions
                .get(&harness)
                .cloned()
                .unwrap_or(VersionProbe::Unavailable);
            let (default_version, default_probe) = match probe {
                VersionProbe::Observed(version) => (Some(version), ProbeState::Observed),
                VersionProbe::Unavailable => (None, ProbeState::Unavailable),
                VersionProbe::TimedOut => (None, ProbeState::TimedOut),
            };
            let harness_name = harness_name(harness).to_string();
            let model = cas_factory::default_worker_model_for_cli(supervisor_harness(harness));
            let account_profile = account_dir_for_harness(harness).unwrap_or_else(|| "default".into());
            let identity = crate::capability::harness_route_identity(
                harness,
                model,
                &account_profile,
            );
            let capability_status = capability_snapshot.status_at(
                &identity,
                cas_factory::CapabilitySnapshot::now_ms(),
            );
            let capability = capability_status
                .as_ref()
                .map_or(cas_factory::CapabilityAvailability::Unknown, |status| {
                    status.availability
                });
            let capability_stale = capability_status.as_ref().is_some_and(|status| status.stale);
            let capability_observed_at_ms = capability_status
                .as_ref()
                .map(|status| status.observed_at_ms);
            let capability_expires_at_ms = capability_status
                .as_ref()
                .map(|status| status.expires_at_ms);
            let Some(receipt) = receipt else {
                let remediation = format!(
                    "Run and persist the typed {harness_name} factory conformance matrix."
                );
                if required {
                    findings.push(warning(
                        "harness.receipt_missing",
                        &format!("harness.{harness_name}"),
                        &format!(
                            "Required {harness_name} has no typed conformance receipt."
                        ),
                        &remediation,
                        None,
                    ));
                }
                return HarnessPreflight {
                    harness: harness_name,
                    required,
                    state: ComponentState::Missing,
                    capability,
                    capability_stale,
                    capability_observed_at_ms,
                    capability_expires_at_ms,
                    validated_version: None,
                    default_version,
                    default_probe,
                    receipt_observed_default_version: None,
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
                if required {
                    findings.push(warning(
                        "harness.validation_failed",
                        &format!("harness.{harness_name}"),
                        &format!("{harness_name} receipt does not pass every required check."),
                        &action,
                        Some(receipt.validated_at.clone()),
                    ));
                }
                remediation = Some(action);
            } else if drift {
                state = ComponentState::Stale;
                let action = format!(
                    "Use validated {harness_name} {} or rerun the full matrix for the current default before updating the pin.",
                    receipt.harness_version
                );
                if required {
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
                }
                remediation = Some(action);
            } else if default_missing {
                state = ComponentState::Stale;
                let action =
                    format!("Install {harness_name} or make its default binary available on PATH.");
                if required {
                    findings.push(warning(
                        "harness.default_unavailable",
                        &format!("harness.{harness_name}"),
                        &format!("{harness_name} default version could not be observed."),
                        &action,
                        Some(receipt.validated_at.clone()),
                    ));
                }
                remediation = Some(action);
            }

            if capability != cas_factory::CapabilityAvailability::Available {
                let action = capability_status
                    .as_ref()
                    .and_then(|status| status.remediation.clone())
                    .unwrap_or_else(|| {
                        "Rerun `cas factory doctor` to refresh route capability evidence."
                            .to_string()
                    });
                if required {
                    findings.push(warning(
                        "harness.capability_unavailable",
                        &format!("harness.{harness_name}"),
                        &format!(
                            "{harness_name} capability is {:?}{}.",
                            capability,
                            capability_stale.then_some(" (evidence is stale)").unwrap_or_default()
                        ),
                        &action,
                        capability_observed_at_ms.and_then(|timestamp| {
                            chrono::DateTime::from_timestamp_millis(timestamp as i64)
                                .map(|time| time.to_rfc3339())
                        }),
                    ));
                }
                if state == ComponentState::Ready {
                    state = if capability == cas_factory::CapabilityAvailability::Unavailable {
                        ComponentState::Missing
                    } else {
                        ComponentState::Stale
                    };
                }
                if remediation.is_none() {
                    remediation = Some(action);
                }
            }

            HarnessPreflight {
                harness: harness_name,
                required,
                state,
                capability,
                capability_stale,
                capability_observed_at_ms,
                capability_expires_at_ms,
                validated_version: Some(receipt.harness_version.clone()),
                default_version,
                default_probe,
                receipt_observed_default_version: receipt
                    .observed_default_harness_version
                    .clone(),
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
        Harness::OpenCode => "opencode",
    }
}

fn collect_binary_facts(
    project_root: &Path,
    probe: &crate::mcp::tools::core::task::repo_context::BoundedRepoProbe,
) -> BinaryFacts {
    use crate::mcp::tools::core::task::repo_context::BoundedRepoError;

    let configured_raw = std::env::var("CAS_EXPECTED_DEPLOYMENT_SHA").ok();
    let configured_sha = configured_raw
        .as_deref()
        .filter(|sha| valid_sha(sha))
        .map(ToOwned::to_owned);
    let configured_sha_invalid = configured_raw.is_some() && configured_sha.is_none();
    let source_root = std::env::var_os("CAS_SOURCE_DIR")
        .map(PathBuf::from)
        .or_else(|| is_cas_source_checkout(project_root).then(|| project_root.to_path_buf()));
    let source_result = source_root
        .as_deref()
        .map(|root| probe.output(root, &["rev-parse", "HEAD"]));
    let source_probe_timed_out = matches!(&source_result, Some(Err(BoundedRepoError::TimedOut)));
    let source_sha = source_result
        .and_then(Result::ok)
        .filter(|sha| valid_sha(sha));
    BinaryFacts {
        running_sha: option_env!("CAS_GIT_HASH").unwrap_or("unknown").to_string(),
        source_sha,
        configured_sha,
        configured_sha_invalid,
        source_probe_timed_out,
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

fn collect_repository_facts(
    project_root: &Path,
    cas_root: &Path,
    probe: &crate::mcp::tools::core::task::repo_context::BoundedRepoProbe,
) -> Result<RepositoryFacts, RepositoryFailure> {
    use crate::mcp::tools::core::task::repo_context::BoundedRepoError;
    use cas_types::WorkTarget;

    let map_error = |error| match error {
        BoundedRepoError::TimedOut => RepositoryFailure::TimedOut,
        BoundedRepoError::CandidateLimit => RepositoryFailure::CandidateLimit,
        BoundedRepoError::Unavailable => RepositoryFailure::Missing,
        BoundedRepoError::Ambiguous => RepositoryFailure::Ambiguous,
        BoundedRepoError::StaleBinding => RepositoryFailure::StaleBinding,
    };
    let branch = probe
        .output(project_root, &["symbolic-ref", "--short", "HEAD"])
        .map_err(map_error)?;
    if branch.is_empty() {
        return Err(RepositoryFailure::Missing);
    }
    let active = crate::mcp::tools::core::task::repo_context::resolve_path_context_bounded(
        project_root,
        &branch,
        probe,
    )
    .map_err(map_error)?;
    let cas_project = cas_root.parent().unwrap_or(cas_root);
    let configured = crate::mcp::tools::core::task::repo_context::resolve_path_context_bounded(
        cas_project,
        &branch,
        probe,
    )
    .map_err(map_error)?;
    if active.repo_selector != configured.repo_selector || active.repo_root != configured.repo_root
    {
        return Err(RepositoryFailure::Wrong);
    }
    let target = WorkTarget {
        repo_selector: active.repo_selector.clone(),
        target_branch: branch.clone(),
    };
    match crate::mcp::tools::core::task::repo_context::resolve_repo_context_bounded(
        cas_root, &target, probe,
    ) {
        Ok(context)
            if context.repo_selector == active.repo_selector
                && context.repo_root == active.repo_root =>
        {
            Ok(RepositoryFacts {
                selector: context.repo_selector,
                target_branch: branch,
            })
        }
        Ok(_) => Err(RepositoryFailure::Wrong),
        Err(BoundedRepoError::TimedOut) => Err(RepositoryFailure::TimedOut),
        Err(BoundedRepoError::CandidateLimit) => Err(RepositoryFailure::CandidateLimit),
        Err(BoundedRepoError::Ambiguous) => Err(RepositoryFailure::Ambiguous),
        Err(BoundedRepoError::StaleBinding) => Err(RepositoryFailure::StaleBinding),
        Err(BoundedRepoError::Unavailable) => Err(RepositoryFailure::Wrong),
    }
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

fn compiled_cas_tool_names() -> Vec<String> {
    crate::mcp::CasService::registered_tool_names_for_build()
}

fn collect_proxy_facts(cas_root: &Path, live: Option<ProxySnapshotInput>) -> ProxyFacts {
    #[cfg(feature = "mcp-proxy")]
    {
        let proxy_path = cas_root.join("proxy.toml");
        let _ = live;
        let (configured, invalid_config) = match cmcp_core::config::Config::load_merged(
            proxy_path.exists().then_some(proxy_path.as_path()),
        ) {
            Ok(config) => (config.servers.len(), false),
            Err(_) => (0, true),
        };
        let (snapshot, evidence_failure) = match crate::mcp::read_proxy_snapshot_cache(cas_root) {
            Ok(snapshot) => (Some(snapshot.health.into()), None),
            Err(error) => {
                use crate::mcp::ProxySnapshotReadErrorKind as Kind;
                let failure = match error.kind {
                    Kind::Missing => ProxyEvidenceFailure::Missing,
                    Kind::Invalid => ProxyEvidenceFailure::Invalid,
                    Kind::ConfigInvalid => {
                        return ProxyFacts {
                            configured,
                            invalid_config: true,
                            snapshot: None,
                            evidence_failure: Some(ProxyEvidenceFailure::Invalid),
                        };
                    }
                    Kind::ConfigMismatch => ProxyEvidenceFailure::Stale,
                    Kind::Unavailable => ProxyEvidenceFailure::Unavailable,
                };
                (None, Some(failure))
            }
        };
        ProxyFacts {
            configured,
            invalid_config,
            snapshot,
            evidence_failure,
        }
    }
    #[cfg(not(feature = "mcp-proxy"))]
    {
        let _ = (cas_root, live);
        ProxyFacts {
            configured: 0,
            invalid_config: false,
            snapshot: None,
            evidence_failure: Some(ProxyEvidenceFailure::Missing),
        }
    }
}

fn required_harnesses(project_root: &Path) -> HashSet<Harness> {
    let config = std::fs::read_to_string(project_root.join(".cas/config.toml"))
        .ok()
        .and_then(|raw| toml::from_str::<crate::config::Config>(&raw).ok())
        .unwrap_or_default();
    let llm = config.llm();
    ["supervisor", "worker"]
        .into_iter()
        .filter_map(|role| harness_from_config(llm.harness_for_role(role)))
        .collect()
}

fn harness_from_config(value: &str) -> Option<Harness> {
    match value.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude-code" => Some(Harness::ClaudeCode),
        "codex" | "codex-cli" => Some(Harness::CodexCli),
        "grok" | "grok-build" => Some(Harness::GrokBuild),
        "opencode" => Some(Harness::OpenCode),
        _ => None,
    }
}

fn probe_default_harness_versions(deadline: Deadline) -> HashMap<Harness, VersionProbe> {
    std::thread::scope(|scope| {
        let handles = registered_harnesses()
            .into_iter()
            .map(|harness| {
                (
                    harness,
                    scope.spawn(move || probe_version(harness_program(harness), deadline)),
                )
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|(harness, handle)| (harness, handle.join().unwrap_or(VersionProbe::Unavailable)))
            .collect()
    })
}

fn harness_program(harness: Harness) -> &'static str {
    match harness {
        Harness::ClaudeCode => "claude",
        Harness::CodexCli => "codex",
        Harness::GrokBuild => "grok",
        Harness::OpenCode => "opencode",
    }
}

fn probe_version(program: &str, deadline: Deadline) -> VersionProbe {
    probe_command_version(program, &["--version"], deadline)
}

fn probe_command_version(program: &str, args: &[&str], deadline: Deadline) -> VersionProbe {
    match run_command(
        Command::new(program).args(args),
        deadline,
        VERSION_PROBE_TIMEOUT,
    ) {
        Ok(output) if output.status.success() => {
            parse_version(&String::from_utf8_lossy(&output.stdout))
                .map(VersionProbe::Observed)
                .unwrap_or(VersionProbe::Unavailable)
        }
        Ok(_) | Err(BoundedCommandError::Io) => VersionProbe::Unavailable,
        Err(BoundedCommandError::TimedOut) => VersionProbe::TimedOut,
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
            "  {}: {} required={} capability={:?}{} observed={} expires={} validated={} live_default={} probe={} receipt_observed={} evidence={}",
            harness.harness,
            state_label(harness.state),
            harness.required,
            harness.capability,
            if harness.capability_stale { " (stale)" } else { "" },
            harness
                .capability_observed_at_ms
                .map_or_else(|| "none".to_string(), |value| value.to_string()),
            harness
                .capability_expires_at_ms
                .map_or_else(|| "none".to_string(), |value| value.to_string()),
            harness.validated_version.as_deref().unwrap_or("none"),
            harness.default_version.as_deref().unwrap_or("unavailable"),
            probe_label(harness.default_probe),
            harness
                .receipt_observed_default_version
                .as_deref()
                .unwrap_or("none"),
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

fn probe_label(state: ProbeState) -> &'static str {
    match state {
        ProbeState::Observed => "observed",
        ProbeState::Unavailable => "unavailable",
        ProbeState::TimedOut => "timed_out",
    }
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
            route: None,
            serving_identity: None,
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

    #[test]
    fn doctor_preflight_snapshot_includes_fable_taste_route() {
        let _env =
            crate::test_support::TestEnvGuard::with_optional_vars(&[("CLAUDE_CONFIG_DIR", None)]);
        // Missing binaries produce deterministic unavailable evidence without auth/network probes.
        let snapshot = collect_capability_snapshot(
            &HashMap::new(),
            &[],
            Deadline::after(Duration::from_secs(1)),
        );
        let (identity, evidence) = snapshot
            .availability
            .iter()
            .find(|(identity, _)| identity.model == "claude-fable-5-1")
            .expect("doctor/preflight must collect the registry's taste recipe");
        assert_eq!(identity.harness, "claude");
        assert_eq!(identity.provider, "anthropic");
        assert_eq!(
            evidence.availability,
            cas_factory::CapabilityAvailability::Unavailable
        );
        assert!(cas_factory::resolve_lane("taste", &snapshot).is_err());
    }

    fn healthy_facts() -> PreflightFacts {
        let receipts = vec![
            receipt(Harness::ClaudeCode, "2.1.0"),
            receipt(Harness::CodexCli, "0.146.0"),
            receipt(Harness::GrokBuild, "0.2.114"),
        ];
        let default_versions = receipts
            .iter()
            .map(|receipt| {
                (
                    receipt.harness,
                    VersionProbe::Observed(receipt.harness_version.clone()),
                )
            })
            .collect();
        PreflightFacts {
            binary: BinaryFacts {
                running_sha: "abcdef0".to_string(),
                source_sha: Some("abcdef0123456789".to_string()),
                configured_sha: None,
                configured_sha_invalid: false,
                source_probe_timed_out: false,
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
                snapshot: Some(ProxySnapshotInput {
                    generated_at_ms: 42,
                    healthy: 0,
                    degraded: 0,
                    servers: Vec::new(),
                }),
                evidence_failure: None,
            },
            receipts,
            default_versions,
            required_harnesses: [Harness::ClaudeCode, Harness::CodexCli, Harness::GrokBuild]
                .into_iter()
                .collect(),
            capability_snapshot: {
                let mut snapshot = cas_factory::CapabilitySnapshot::default();
                for harness in [
                    Harness::ClaudeCode,
                    Harness::CodexCli,
                    Harness::GrokBuild,
                    Harness::OpenCode,
                ] {
                    let model =
                        cas_factory::default_worker_model_for_cli(supervisor_harness(harness));
                    let account_profile =
                        account_dir_for_harness(harness).unwrap_or_else(|| "default".into());
                    let identity =
                        crate::capability::harness_route_identity(harness, model, &account_profile);
                    snapshot.record(
                        identity,
                        cas_factory::CapabilityEvidence::new(
                            cas_factory::CapabilityAvailability::Available,
                            cas_factory::CapabilitySnapshot::now_ms(),
                        ),
                    );
                }
                snapshot
            },
            cloud_queue: CloudQueueFacts {
                health: None,
                pending_warning: 200,
                oldest_warning_secs: 6 * 60 * 60,
            },
            runtime_elapsed_ms: 12,
        }
    }

    #[test]
    fn healthy_report_is_ready_and_unblocked() {
        let report = build_report(healthy_facts());
        assert_eq!(report.schema_version, 2);
        assert_eq!(report.overall, PreflightOverall::Ready);
        assert!(!report.factory_blocked);
        assert!(report.findings.is_empty());
        assert!(report
            .harnesses
            .iter()
            .all(|harness| harness.capability == cas_factory::CapabilityAvailability::Available));
    }

    #[test]
    fn unhealthy_cloud_queue_warns_but_never_blocks_factory() {
        let mut facts = healthy_facts();
        facts.cloud_queue.health = Some(QueueHealth {
            pending: 201,
            oldest_item: Some(chrono::Utc::now() - chrono::Duration::hours(7)),
            oldest_age_secs: Some(7 * 60 * 60),
            last_error: Some("Network error: offline".to_string()),
            unreviewed_conflicts: 3,
        });

        let report = build_report(facts);
        let finding = report
            .findings
            .iter()
            .find(|finding| finding.code == "cloud.queue_unhealthy")
            .expect("cloud queue health warning");
        assert_eq!(finding.severity, PreflightSeverity::Warning);
        assert!(finding.message.contains("201 pending"));
        assert!(finding.message.contains("7h 0m"));
        assert!(finding.message.contains("Network error: offline"));
        assert!(finding.message.contains("unreviewed conflicts: 3"));
        assert!(!report.factory_blocked);
    }

    #[test]
    fn healthy_cloud_queue_stays_silent() {
        let mut facts = healthy_facts();
        facts.cloud_queue.health = Some(QueueHealth {
            pending: 2,
            oldest_item: Some(chrono::Utc::now()),
            oldest_age_secs: Some(5),
            last_error: None,
            unreviewed_conflicts: 0,
        });

        let report = build_report(facts);
        assert!(
            report
                .findings
                .iter()
                .all(|finding| finding.code != "cloud.queue_unhealthy")
        );
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
    fn short_and_full_shas_match_symmetrically() {
        let full = "abcdef0123456789abcdef0123456789abcdef01";
        assert!(shas_compatible(full, "abcdef0"));
        assert!(shas_compatible("abcdef0", full));
        assert!(shas_compatible("abcdef01", "abcdef0"));
        assert!(!shas_compatible(full, "1234567"));
        assert!(!shas_compatible("abcdef", full));
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
    fn candidate_limit_names_the_supported_registry_only_recovery() {
        let mut facts = healthy_facts();
        facts.repository = Err(RepositoryFailure::CandidateLimit);
        let report = build_report(facts);
        let finding = report
            .findings
            .iter()
            .find(|finding| finding.code == "repository.candidate_limit")
            .expect("candidate-limit finding");

        assert!(report.factory_blocked);
        assert!(
            finding
                .remediation
                .contains("cas known-repos prune-missing --dry-run")
        );
        assert!(
            finding
                .remediation
                .contains("cas known-repos prune-missing")
        );
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
            evidence_failure: None,
            snapshot: Some(ProxySnapshotInput {
                generated_at_ms: 42,
                healthy: 0,
                degraded: 1,
                servers: vec![OptionalUpstreamPreflight {
                    name: "optional".to_string(),
                    transport: "http".to_string(),
                    state: "backoff".to_string(),
                    executable: None,
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

    #[cfg(feature = "mcp-proxy")]
    #[test]
    fn forged_cached_proxy_health_is_sanitized_before_preflight_json() {
        // cas-c220 (GH #89): the snapshot below is only readable while its
        // `config_fingerprint` still matches the merged USER + project proxy
        // config, and the user half resolves through XDG_CONFIG_HOME/HOME at
        // call time. Pin both for the whole body (fixture write through
        // `collect_proxy_facts`) so a concurrent test repointing HOME cannot
        // invalidate the fingerprint and empty out `optional_upstreams`.
        let config_root = tempfile::tempdir().unwrap();
        let config_home = config_root.path().join(".config");
        std::fs::create_dir_all(&config_home).unwrap();
        let _env = crate::test_support::TestEnvGuard::with_optional_vars(&[
            ("HOME", Some(config_root.path().to_str().unwrap())),
            ("XDG_CONFIG_HOME", Some(config_home.to_str().unwrap())),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let first_name = "https://user:token@example.invalid/private";
        let second_name = "/home/operator/.config/secret-token";
        let forged = cmcp_core::ProxyHealthSnapshot {
            session_id: "forged-cache".to_string(),
            generated_at_ms: 42,
            healthy: 0,
            degraded: 2,
            servers: vec![
                cmcp_core::UpstreamHealth {
                    name: first_name.to_string(),
                    transport: "Bearer cache-secret".to_string(),
                    state: cmcp_core::UpstreamState::Backoff,
                    executable: None,
                    attempts: 1,
                    consecutive_failures: 1,
                    tool_count: 0,
                    last_error_code: Some("token=cache-secret\ncontrol".to_string()),
                    last_attempt_at_ms: Some(40),
                    next_retry_at_ms: Some(50),
                },
                cmcp_core::UpstreamHealth {
                    name: second_name.to_string(),
                    transport: "http".to_string(),
                    state: cmcp_core::UpstreamState::Backoff,
                    executable: None,
                    attempts: 1,
                    consecutive_failures: 1,
                    tool_count: 0,
                    last_error_code: Some("timeout".to_string()),
                    last_attempt_at_ms: Some(40),
                    next_retry_at_ms: Some(50),
                },
            ],
        };
        let mut config = cmcp_core::config::Config::default();
        for name in [first_name, second_name] {
            config.add_server(
                name.to_string(),
                cmcp_core::config::ServerConfig::Http {
                    url: "https://example.invalid/mcp".to_string(),
                    auth: None,
                    headers: std::collections::HashMap::new(),
                    oauth: false,
                },
            );
        }
        config.save_to(&temp.path().join("proxy.toml")).unwrap();
        let merged =
            cmcp_core::config::Config::load_merged(Some(temp.path().join("proxy.toml").as_path()))
                .unwrap();
        let generated_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let snapshot = crate::mcp::ProxySnapshotCache {
            schema_version: 1,
            generation: "snapshot-forged-1".to_string(),
            generated_at_ms,
            config_fingerprint: Some(crate::mcp::proxy_config_fingerprint(&merged)),
            state: crate::mcp::ProxySnapshotState::Ready,
            failure: None,
            catalog: std::collections::BTreeMap::new(),
            health: cmcp_core::ProxyHealthSnapshot {
                generated_at_ms,
                ..forged
            },
        };
        std::fs::write(
            temp.path().join("proxy_snapshot.json"),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();

        let mut facts = healthy_facts();
        facts.proxy = collect_proxy_facts(temp.path(), None);
        let report = build_report(facts);
        let servers = &report.optional_upstreams.servers;
        assert_eq!(servers.len(), 2);
        assert_ne!(servers[0].name, servers[1].name);
        for server in servers {
            assert!(server.name.starts_with("upstream-"));
            assert_eq!(server.name.len(), "upstream-".len() + 32);
        }
        assert_eq!(servers[0].transport, "unknown");
        assert_eq!(servers[0].last_error_code.as_deref(), Some("unknown"));
        assert_eq!(servers[1].transport, "http");
        assert_eq!(servers[1].last_error_code.as_deref(), Some("timeout"));

        let json = serde_json::to_string(&report).unwrap();
        for forbidden in [
            first_name,
            second_name,
            "Bearer cache-secret",
            "token=cache-secret",
            "control",
        ] {
            assert!(!json.contains(forbidden), "{forbidden:?} leaked: {json}");
        }
    }

    #[test]
    fn validated_default_version_drift_is_stale_warning_not_blocker() {
        let mut facts = healthy_facts();
        facts.default_versions.insert(
            Harness::GrokBuild,
            VersionProbe::Observed("0.2.117".to_string()),
        );
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
    fn receipt_time_observation_never_substitutes_for_live_default() {
        let mut facts = healthy_facts();
        facts
            .default_versions
            .insert(Harness::CodexCli, VersionProbe::Unavailable);
        let report = build_report(facts);
        let codex = report
            .harnesses
            .iter()
            .find(|harness| harness.harness == "codex")
            .unwrap();
        assert_eq!(codex.default_version, None);
        assert_eq!(codex.default_probe, ProbeState::Unavailable);
        assert_eq!(
            codex.receipt_observed_default_version.as_deref(),
            Some("0.146.0")
        );
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.code == "harness.default_unavailable")
        );
    }

    #[test]
    fn configured_role_policy_makes_optional_unvalidated_harnesses_informational() {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join(".cas")).unwrap();
        std::fs::write(
            project.path().join(".cas/config.toml"),
            "[llm.supervisor]\nharness = \"codex\"\n[llm.worker]\nharness = \"codex\"\n",
        )
        .unwrap();
        let required = required_harnesses(project.path());
        assert_eq!(required, [Harness::CodexCli].into_iter().collect());

        let mut facts = healthy_facts();
        facts.required_harnesses = required;
        facts
            .receipts
            .retain(|receipt| receipt.harness == Harness::CodexCli);
        facts
            .default_versions
            .retain(|harness, _| *harness == Harness::CodexCli);
        let report = build_report(facts);
        assert_eq!(report.overall, PreflightOverall::Ready);
        assert!(report.findings.is_empty());
        assert!(
            report
                .harnesses
                .iter()
                .find(|harness| harness.harness == "claude")
                .is_some_and(
                    |harness| !harness.required && harness.state == ComponentState::Missing
                )
        );
    }

    #[test]
    fn default_role_policy_requires_real_supervisor_and_worker_harnesses() {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join(".cas")).unwrap();
        std::fs::write(project.path().join(".cas/config.toml"), "").unwrap();
        assert_eq!(
            required_harnesses(project.path()),
            [Harness::ClaudeCode, Harness::CodexCli]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn opencode_role_policy_requires_its_typed_receipt_and_version_probe() {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join(".cas")).unwrap();
        std::fs::write(
            project.path().join(".cas/config.toml"),
            "[llm.supervisor]\nharness = \"opencode\"\n[llm.worker]\nharness = \"opencode\"\n",
        )
        .unwrap();
        assert_eq!(
            required_harnesses(project.path()),
            [Harness::OpenCode].into_iter().collect()
        );

        let mut facts = healthy_facts();
        facts.required_harnesses = [Harness::OpenCode].into_iter().collect();
        facts.default_versions.insert(
            Harness::OpenCode,
            VersionProbe::Observed("1.18.23".to_string()),
        );
        let report = build_report(facts);
        let opencode = report
            .harnesses
            .iter()
            .find(|harness| harness.harness == "opencode")
            .unwrap();
        assert!(opencode.required);
        assert_eq!(opencode.default_version.as_deref(), Some("1.18.23"));
        assert_eq!(opencode.state, ComponentState::Missing);
        assert!(report.findings.iter().any(|finding| {
            finding.code == "harness.receipt_missing" && finding.component == "harness.opencode"
        }));
    }

    #[test]
    fn typed_deadline_evidence_has_no_path_or_command_content() {
        let mut facts = healthy_facts();
        facts.repository = Err(RepositoryFailure::TimedOut);
        facts
            .default_versions
            .insert(Harness::CodexCli, VersionProbe::TimedOut);
        let report = build_report(facts);
        assert_eq!(
            report.timed_out_components,
            vec!["harness.codex", "repository"]
        );
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("repository.probe_timed_out"));
        for forbidden in ["git -C", "/home/", "CAS_SOURCE_DIR="] {
            assert!(!json.contains(forbidden), "{forbidden} leaked: {json}");
        }
    }

    #[test]
    fn downstream_project_head_is_not_used_as_cas_source_sha() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("Cargo.toml"), "[package]\nname='app'\n").unwrap();
        let probe = crate::mcp::tools::core::task::repo_context::BoundedRepoProbe::new(
            Deadline::after(Duration::from_secs(1)),
            Duration::from_millis(200),
            8,
        );
        let facts = collect_binary_facts(temp.path(), &probe);
        assert_eq!(facts.source_sha, None);
    }

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

        let probe = crate::mcp::tools::core::task::repo_context::BoundedRepoProbe::new(
            Deadline::after(Duration::from_secs(2)),
            Duration::from_millis(500),
            8,
        );
        let facts = collect_repository_facts(project.path(), &project.path().join(".cas"), &probe);
        assert!(
            matches!(facts, Ok(RepositoryFacts { ref selector, ref target_branch })
                if selector == "project:preflight-boundary" && target_branch == "main"),
            "unexpected repository facts: {facts:?}"
        );
    }

    #[test]
    fn repository_preflight_honors_binding_without_disclosing_host_paths() {
        use cas_store::KnownRepoStore;

        let _env = crate::test_support::TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().unwrap();
        let home = dirs::home_dir().unwrap();
        let clone_a = home.join("clone-a");
        let clone_b = home.join("clone-b");
        let git = |repo: &Path, args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(repo)
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .env("GIT_CONFIG_SYSTEM", "/dev/null")
                    .status()
                    .unwrap()
                    .success()
            );
        };
        for repo in [&clone_a, &clone_b] {
            std::fs::create_dir(repo).unwrap();
            git(repo, &["init", "-q", "-b", "main"]);
            git(
                repo,
                &[
                    "remote",
                    "add",
                    "origin",
                    "git@github.com:org/preflight-shared.git",
                ],
            );
            std::fs::create_dir(repo.join(".cas")).unwrap();
            crate::store::known_repos::register_repo_strict(repo).unwrap();
        }
        let (selector, root_b, common_b) =
            crate::mcp::tools::core::task::repo_context::binding_identity_for_path(&clone_b)
                .unwrap();
        crate::store::known_repos::open_host_known_repo_store()
            .unwrap()
            .bind(&selector, &root_b, &common_b)
            .unwrap();

        let probe = crate::mcp::tools::core::task::repo_context::BoundedRepoProbe::new(
            Deadline::after(Duration::from_secs(2)),
            Duration::from_millis(500),
            8,
        );
        let ready = collect_repository_facts(&clone_b, &clone_b.join(".cas"), &probe);
        assert!(matches!(ready, Ok(RepositoryFacts { ref selector, .. })
            if selector == "remote:github.com/org/preflight-shared"));
        assert!(matches!(
            collect_repository_facts(&clone_a, &clone_a.join(".cas"), &probe),
            Err(RepositoryFailure::Wrong)
        ));

        git(
            &clone_b,
            &[
                "remote",
                "set-url",
                "origin",
                "git@github.com:org/changed.git",
            ],
        );
        let stale = collect_repository_facts(&clone_a, &clone_a.join(".cas"), &probe);
        assert!(matches!(stale, Err(RepositoryFailure::StaleBinding)));
        let mut findings = Vec::new();
        let report = classify_repository(stale, &mut findings);
        let serialized = serde_json::to_string(&(report, findings)).unwrap();
        assert!(serialized.contains("repository.binding_stale"));
        assert!(!serialized.contains(home.to_string_lossy().as_ref()));
        assert!(!serialized.contains(clone_a.to_string_lossy().as_ref()));
        assert!(!serialized.contains(clone_b.to_string_lossy().as_ref()));
    }

    #[cfg(unix)]
    #[test]
    fn failed_version_probe_is_killed_within_bound() {
        let started = Instant::now();
        let result = probe_command_version(
            "sh",
            &["-c", "sleep 10"],
            Deadline::after(Duration::from_millis(75)),
        );
        assert_eq!(result, VersionProbe::TimedOut);
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
        assert!(human.contains("capability=Available"));
        assert!(!human.contains("/home/"));
    }
}
