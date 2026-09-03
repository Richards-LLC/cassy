use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

pub(crate) mod adapters;

const MIN_P95_SAMPLE_COUNT: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ProbeAdapterKind {
    Fake,
    Claude,
    Codex,
    Grok,
    All,
}

#[derive(Clone, Debug)]
pub(crate) struct ProbeThresholds {
    pub delivery_ms: u64,
    pub selection_ms: u64,
    pub normal_transport_p95_ms: u64,
    pub normal_transport_max_ms: u64,
    pub urgent_transport_p95_ms: u64,
    pub urgent_transport_max_ms: u64,
    pub wake_p95_ms: u64,
    pub reaction_p95_ms: u64,
}

impl ProbeThresholds {
    #[cfg(test)]
    pub(crate) fn epic_defaults() -> Self {
        Self {
            delivery_ms: 2_000,
            selection_ms: 250,
            normal_transport_p95_ms: 2_000,
            normal_transport_max_ms: 10_000,
            urgent_transport_p95_ms: 2_000,
            urgent_transport_max_ms: 5_000,
            wake_p95_ms: 5_000,
            reaction_p95_ms: 15_000,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProbeFailure {
    Transport {
        scenario: String,
        message_id: String,
    },
    Slo {
        scenario: String,
        message_id: String,
        delivered_after_ms: u64,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct ProbeCommConfig {
    pub jsonl: PathBuf,
    pub cas_root: Option<PathBuf>,
    pub artifact_root: Option<PathBuf>,
    pub allow_active_cas_root: bool,
    pub adapter: Option<ProbeAdapterKind>,
    pub thresholds: ProbeThresholds,
    pub failure: Option<ProbeFailure>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProbeReport {
    pub passed: bool,
    pub scenarios: Vec<ScenarioReport>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ScenarioReport {
    #[serde(rename = "scenario")]
    pub name: &'static str,
    pub passed: bool,
    pub failed_stage: Option<&'static str>,
    pub message_ids: Vec<String>,
    pub duplicates_suppressed: u32,
    pub malformed_targets_rejected: u32,
    pub stages: Vec<MessageStageEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slo_contract: Option<SloContractReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aggregate_slos: Vec<AggregateSloEvidence>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SloContractReport {
    pub units: &'static str,
    pub normal_transport_p95_ms: u64,
    pub normal_transport_max_ms: u64,
    pub urgent_transport_p95_ms: u64,
    pub urgent_transport_max_ms: u64,
    pub wake_p95_ms: u64,
    pub reaction_p95_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AggregateSloEvidence {
    pub metric: &'static str,
    pub gate: &'static str,
    pub sample_count: usize,
    pub observed_ms: u64,
    pub threshold_ms: u64,
    pub passed: bool,
    pub provenance: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct MessageStageEvidence {
    pub message_id: String,
    pub target: String,
    pub enqueued_at_ms: Option<u64>,
    pub selected_at_ms: Option<u64>,
    pub delivered_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_reaction_at_ms: Option<u64>,
    pub enqueued_status: &'static str,
    pub selected_status: &'static str,
    pub delivered_status: &'static str,
    pub wake_status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reaction_status: Option<String>,
    pub stage_statuses: Vec<StageStatusEvidence>,
    pub terminal: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct StageStatusEvidence {
    pub stage: String,
    pub status: String,
    pub provenance: String,
}

#[derive(Clone, Copy, Debug)]
struct ScenarioDef {
    name: &'static str,
    messages: &'static [&'static str],
    duplicate_of: Option<&'static str>,
    malformed_target: bool,
}

const SCENARIOS: &[ScenarioDef] = &[
    ScenarioDef {
        name: "startup",
        messages: &["startup-0"],
        duplicate_of: None,
        malformed_target: false,
    },
    ScenarioDef {
        name: "serial_10",
        messages: &[
            "serial-0", "serial-1", "serial-2", "serial-3", "serial-4", "serial-5", "serial-6",
            "serial-7", "serial-8", "serial-9",
        ],
        duplicate_of: None,
        malformed_target: false,
    },
    ScenarioDef {
        name: "fifo_burst",
        messages: &["fifo-0", "fifo-1", "fifo-2", "fifo-3", "fifo-4"],
        duplicate_of: None,
        malformed_target: false,
    },
    ScenarioDef {
        name: "urgent",
        messages: &["urgent-0"],
        duplicate_of: None,
        malformed_target: false,
    },
    ScenarioDef {
        name: "duplicate_replay",
        messages: &["replay-original"],
        duplicate_of: Some("replay-original"),
        malformed_target: false,
    },
    ScenarioDef {
        name: "malformed_target",
        messages: &["malformed-0"],
        duplicate_of: None,
        malformed_target: true,
    },
    ScenarioDef {
        name: "lifecycle",
        messages: &["lifecycle-start", "lifecycle-stop"],
        duplicate_of: None,
        malformed_target: false,
    },
];

pub(crate) fn execute_probe_comm(
    jsonl: PathBuf,
    cas_root: Option<PathBuf>,
    artifact_root: Option<PathBuf>,
    parent_cas_root: Option<&Path>,
    allow_active_cas_root: bool,
    adapter: ProbeAdapterKind,
    delivery_slo_ms: u64,
    selection_slo_ms: u64,
    normal_transport_p95_slo_ms: u64,
    normal_transport_max_slo_ms: u64,
    urgent_transport_p95_slo_ms: u64,
    urgent_transport_max_slo_ms: u64,
    wake_p95_slo_ms: u64,
    reaction_p95_slo_ms: u64,
    failure: Option<ProbeFailure>,
) -> Result<()> {
    let report = run_probe_comm_with_parent(
        ProbeCommConfig {
            jsonl,
            cas_root,
            artifact_root,
            allow_active_cas_root,
            adapter: Some(adapter),
            thresholds: ProbeThresholds {
                delivery_ms: delivery_slo_ms,
                selection_ms: selection_slo_ms,
                normal_transport_p95_ms: normal_transport_p95_slo_ms,
                normal_transport_max_ms: normal_transport_max_slo_ms,
                urgent_transport_p95_ms: urgent_transport_p95_slo_ms,
                urgent_transport_max_ms: urgent_transport_max_slo_ms,
                wake_p95_ms: wake_p95_slo_ms,
                reaction_p95_ms: reaction_p95_slo_ms,
            },
            failure,
        },
        parent_cas_root,
    )?;

    println!(
        "probe-comm passed: {} scenarios",
        report
            .scenarios
            .iter()
            .filter(|scenario| scenario.passed)
            .count()
    );
    Ok(())
}

pub(crate) fn parse_probe_failure(
    transport: Option<&str>,
    slo: Option<&str>,
) -> Result<Option<ProbeFailure>> {
    match (transport, slo) {
        (Some(_), Some(_)) => {
            bail!("use only one probe failure injection flag");
        }
        (Some(spec), None) => {
            let (scenario, message_id) = spec
                .split_once(':')
                .context("--inject-transport-failure must be SCENARIO:MESSAGE_ID")?;
            if scenario.is_empty() || message_id.is_empty() {
                bail!("--inject-transport-failure must be SCENARIO:MESSAGE_ID");
            }
            Ok(Some(ProbeFailure::Transport {
                scenario: scenario.to_string(),
                message_id: message_id.to_string(),
            }))
        }
        (None, Some(spec)) => {
            let mut parts = spec.split(':');
            let scenario = parts.next().unwrap_or_default();
            let message_id = parts.next().unwrap_or_default();
            let delivered_after_ms = parts.next().unwrap_or_default();
            if scenario.is_empty()
                || message_id.is_empty()
                || delivered_after_ms.is_empty()
                || parts.next().is_some()
            {
                bail!("--inject-slo-failure must be SCENARIO:MESSAGE_ID:MS");
            }
            Ok(Some(ProbeFailure::Slo {
                scenario: scenario.to_string(),
                message_id: message_id.to_string(),
                delivered_after_ms: delivered_after_ms
                    .parse()
                    .context("--inject-slo-failure MS must be an integer")?,
            }))
        }
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
pub(crate) fn run_probe_comm(config: ProbeCommConfig) -> Result<ProbeReport> {
    run_probe_comm_with_parent(config, None)
}

pub(crate) fn run_probe_comm_with_parent(
    config: ProbeCommConfig,
    parent_cas_root: Option<&Path>,
) -> Result<ProbeReport> {
    let ProbeCommConfig {
        jsonl,
        cas_root,
        artifact_root,
        allow_active_cas_root,
        adapter,
        thresholds,
        failure,
    } = config;

    let adapter = adapter.context("probe adapter is required")?;

    if jsonl.is_dir() {
        bail!("jsonl output path is a directory: {}", jsonl.display());
    }
    if let Some(parent) = jsonl.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create jsonl output directory {}",
                parent.display()
            )
        })?;
    }

    // GH #704: a generated root is ours and must not outlive the run — on the
    // success path or any of the error paths below. `_scratch` owns that
    // removal; an operator-supplied `--cas-root` is left alone.
    let (run_root, _scratch) = match cas_root {
        Some(path) => (path, ScratchRootGuard::borrowed()),
        None => {
            let container = generated_scratch_root_path();
            let root = container.join(".cas");
            (root, ScratchRootGuard::owning(container))
        }
    };
    guard_active_parent_root(&run_root, parent_cas_root, allow_active_cas_root)?;
    crate::temp_hygiene::guard_isolated_root(&run_root, &crate::temp_hygiene::HostMountProbe)?;
    fs::create_dir_all(&run_root)
        .with_context(|| format!("failed to create isolated Cassy root {}", run_root.display()))?;

    let mut writer = BufWriter::new(
        File::create(&jsonl)
            .with_context(|| format!("failed to create jsonl output path {}", jsonl.display()))?,
    );

    let scenarios_to_write = match adapter {
        ProbeAdapterKind::Fake => run_fake_scenarios(&thresholds, failure.as_ref()),
        ProbeAdapterKind::Claude
        | ProbeAdapterKind::Codex
        | ProbeAdapterKind::Grok
        | ProbeAdapterKind::All => {
            run_recorded_adapter_scenarios(adapter, artifact_root.as_deref(), &thresholds)
        }
    };

    let mut scenarios = Vec::with_capacity(scenarios_to_write.len());
    for report in scenarios_to_write {
        serde_json::to_writer(&mut writer, &report)
            .context("failed to serialize probe scenario evidence")?;
        writer
            .write_all(b"\n")
            .context("failed to write probe scenario evidence")?;
        scenarios.push(report);
    }
    writer
        .flush()
        .context("failed to flush probe scenario evidence")?;

    let report = ProbeReport {
        passed: scenarios.iter().all(|scenario| scenario.passed),
        scenarios,
    };

    if let Some(failed) = report.scenarios.iter().find(|scenario| !scenario.passed) {
        bail!(
            "probe-comm scenario '{}' failed at stage '{}'",
            failed.name,
            failed.failed_stage.unwrap_or("unknown")
        );
    }

    Ok(report)
}

fn run_fake_scenarios(
    thresholds: &ProbeThresholds,
    failure: Option<&ProbeFailure>,
) -> Vec<ScenarioReport> {
    let mut clock = FakeClock::default();
    SCENARIOS
        .iter()
        .map(|scenario| run_scenario(*scenario, thresholds, failure, &mut clock))
        .collect()
}

fn run_recorded_adapter_scenarios(
    adapter: ProbeAdapterKind,
    artifact_root: Option<&Path>,
    thresholds: &ProbeThresholds,
) -> Vec<ScenarioReport> {
    let mut scenarios = Vec::new();
    if matches!(adapter, ProbeAdapterKind::Claude | ProbeAdapterKind::All) {
        scenarios.push(adapter_scenario_from_result(
            "claude_adapter",
            "claude-1",
            "worker-a",
            artifact_root.map(|root| {
                adapters::ClaudeAdapter::extract_fixture(&adapters::ClaudeFixture {
                    inbox_path: root.join("claude").join("inbox.json"),
                    transcript_path: Some(root.join("claude").join("transcript.jsonl")),
                    message_id: "claude-1".to_string(),
                    target: "worker-a".to_string(),
                })
            }),
        ));
    }
    if matches!(adapter, ProbeAdapterKind::Codex | ProbeAdapterKind::All) {
        scenarios.push(adapter_scenario_from_result(
            "codex_adapter",
            "codex-1",
            "worker-a",
            artifact_root.map(|root| {
                adapters::CodexAdapter::extract_fixture(&adapters::CodexFixture {
                    rollout_path: root.join("codex").join("rollout.jsonl"),
                    message_id: "codex-1".to_string(),
                    target: "worker-a".to_string(),
                })
            }),
        ));
    }
    if matches!(adapter, ProbeAdapterKind::Grok | ProbeAdapterKind::All) {
        scenarios.push(adapter_scenario_from_result(
            "grok_adapter",
            "grok-1",
            "worker-a",
            artifact_root.map(|root| {
                adapters::GrokAdapter::extract_fixture(&adapters::GrokFixture {
                    updates_path: root.join("grok").join("updates.jsonl"),
                    events_path: Some(root.join("grok").join("events.jsonl")),
                    message_id: "grok-1".to_string(),
                    target: "worker-a".to_string(),
                })
            }),
        ));
    }
    if matches!(adapter, ProbeAdapterKind::All) {
        scenarios.push(routing_matrix_evidence_scenario(artifact_root));
        scenarios.push(merge_reclose_lifecycle_evidence_scenario(artifact_root));
        scenarios.push(slo_aggregate_evidence_scenario(artifact_root, thresholds));
    }
    scenarios
}

fn adapter_scenario_from_result(
    name: &'static str,
    message_id: &str,
    target: &str,
    result: Option<Result<MessageStageEvidence>>,
) -> ScenarioReport {
    match result {
        Some(Ok(stage)) => adapter_scenario(name, stage),
        Some(Err(error)) => {
            let detail = format!("{error:?}");
            adapter_failure_scenario(
                name,
                message_id,
                target,
                classify_artifact_error(&detail),
                detail,
            )
        }
        None => adapter_failure_scenario(
            name,
            message_id,
            target,
            "artifact_missing",
            "recorded harness adapter requires --artifact-root".to_string(),
        ),
    }
}

fn adapter_scenario(name: &'static str, stage: MessageStageEvidence) -> ScenarioReport {
    // A recorded adapter supplies one wake/reaction observation, not a latency
    // distribution. Keep it as provenance and reserve p95 evaluation for the
    // aggregate receipt, which enforces the minimum sample count below.
    let failed_stage = (stage.terminal != "delivered").then_some(stage.terminal);

    let passed =
        stage.terminal == "delivered" && stage.reaction_status.as_deref() != Some("UNKNOWN");
    ScenarioReport {
        name,
        passed,
        failed_stage: if passed {
            None
        } else {
            failed_stage.or(Some("reaction_unknown"))
        },
        message_ids: vec![stage.message_id.clone()],
        duplicates_suppressed: 0,
        malformed_targets_rejected: 0,
        stages: vec![stage],
        slo_contract: None,
        aggregate_slos: Vec::new(),
    }
}

fn classify_artifact_error(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("transcript_parse_failed") {
        "transcript_parse_failed"
    } else if lower.contains("events_parse_failed") {
        "events_parse_failed"
    } else if lower.contains("no such file")
        || lower.contains("not found")
        || lower.contains("os error 2")
        || lower.contains("requires --artifact-root")
    {
        "artifact_missing"
    } else if lower.contains("did not contain correlated") {
        "correlation_unknown"
    } else if lower.contains("parse") || lower.contains("expected") {
        "artifact_parse_failed"
    } else {
        "correlation_unknown"
    }
}

fn adapter_failure_scenario(
    name: &'static str,
    message_id: &str,
    target: &str,
    failed_stage: &'static str,
    detail: String,
) -> ScenarioReport {
    ScenarioReport {
        name,
        passed: false,
        failed_stage: Some(failed_stage),
        message_ids: Vec::new(),
        duplicates_suppressed: 0,
        malformed_targets_rejected: 0,
        stages: vec![MessageStageEvidence {
            message_id: message_id.to_string(),
            target: target.to_string(),
            enqueued_at_ms: None,
            selected_at_ms: None,
            delivered_at_ms: None,
            wake_at_ms: None,
            first_reaction_at_ms: None,
            enqueued_status: "UNKNOWN",
            selected_status: "UNKNOWN",
            delivered_status: if failed_stage == "correlation_unknown" {
                "UNKNOWN"
            } else {
                "FAILED"
            },
            wake_status: "UNKNOWN",
            reaction_status: Some("UNKNOWN".to_string()),
            stage_statuses: vec![StageStatusEvidence {
                stage: "artifact".to_string(),
                status: if failed_stage == "correlation_unknown" {
                    "UNKNOWN"
                } else {
                    "FAILED"
                }
                .to_string(),
                provenance: detail,
            }],
            terminal: failed_stage,
        }],
        slo_contract: None,
        aggregate_slos: Vec::new(),
    }
}

#[derive(Debug, Deserialize)]
struct ReceiptDocument {
    receipt_type: String,
    #[serde(default)]
    contracts: Vec<ReceiptEntry>,
    #[serde(default)]
    receipts: Vec<ReceiptEntry>,
}

#[derive(Debug, Deserialize)]
struct ReceiptEntry {
    message_id: String,
    target: String,
    stage: String,
    status: String,
    provenance: String,
}

#[derive(Debug, Deserialize)]
struct SloSamplesDocument {
    receipt_type: String,
    provenance: String,
    #[serde(default)]
    normal_transport_ms: Vec<u64>,
    #[serde(default)]
    urgent_transport_ms: Vec<u64>,
    #[serde(default)]
    wake_ms: Vec<u64>,
    #[serde(default)]
    reaction_ms: Vec<u64>,
}

fn routing_matrix_evidence_scenario(artifact_root: Option<&Path>) -> ScenarioReport {
    let expected = expected_routing_contracts();
    receipt_backed_scenario(
        "routing_matrix_evidence",
        artifact_root,
        "receipts/routing_matrix.json",
        "routing_matrix",
        "contracts",
        &expected,
        "routing_receipt_missing",
        "routing_receipt_malformed",
        "routing_receipt_incomplete",
    )
}

fn merge_reclose_lifecycle_evidence_scenario(artifact_root: Option<&Path>) -> ScenarioReport {
    let expected = vec![
        (
            "cas-126b-merge-reclose-halt-exemption".to_string(),
            "awaiting-merge-worker".to_string(),
            "merge_reclose".to_string(),
        ),
        (
            "cas-062d-owner-lifecycle-transitions".to_string(),
            "owning-supervisor".to_string(),
            "lifecycle_transition".to_string(),
        ),
        (
            "cas-ecff-lifecycle-outbox-recovery".to_string(),
            "owning-supervisor".to_string(),
            "lifecycle_outbox_recovery".to_string(),
        ),
    ];
    receipt_backed_scenario(
        "merge_reclose_lifecycle_evidence",
        artifact_root,
        "receipts/lifecycle.json",
        "merge_reclose_lifecycle",
        "receipts",
        &expected,
        "lifecycle_receipt_missing",
        "lifecycle_receipt_malformed",
        "lifecycle_receipt_incomplete",
    )
}

fn expected_routing_contracts() -> Vec<(String, String, String)> {
    let harnesses = ["claude", "codex", "grok"];
    let mut expected = Vec::new();
    for supervisor in harnesses {
        for worker in harnesses {
            expected.push((
                format!("{supervisor}-supervisor-to-{worker}-worker"),
                format!("{worker}-worker"),
                "routing_matrix".to_string(),
            ));
            expected.push((
                format!("{worker}-worker-to-{supervisor}-supervisor"),
                format!("{supervisor}-supervisor"),
                "routing_matrix".to_string(),
            ));
        }
    }
    expected
}

fn receipt_backed_scenario(
    name: &'static str,
    artifact_root: Option<&Path>,
    receipt_relpath: &str,
    receipt_type: &str,
    entry_field: &str,
    expected: &[(String, String, String)],
    missing_stage: &'static str,
    malformed_stage: &'static str,
    incomplete_stage: &'static str,
) -> ScenarioReport {
    let Some(root) = artifact_root else {
        return receipt_failure_scenario(name, missing_stage, "no artifact root for receipts");
    };
    let path = root.join(receipt_relpath);
    if !path.exists() {
        return receipt_failure_scenario(
            name,
            missing_stage,
            &format!("missing receipt {}", path.display()),
        );
    }
    let data = match fs::read_to_string(&path) {
        Ok(data) => data,
        Err(error) => {
            return receipt_failure_scenario(
                name,
                missing_stage,
                &format!("read receipt {}: {error}", path.display()),
            );
        }
    };
    let document: ReceiptDocument = match serde_json::from_str(&data) {
        Ok(document) => document,
        Err(error) => {
            return receipt_failure_scenario(
                name,
                malformed_stage,
                &format!("parse receipt {}: {error}", path.display()),
            );
        }
    };
    if document.receipt_type != receipt_type {
        return receipt_failure_scenario(
            name,
            malformed_stage,
            &format!(
                "receipt {} type '{}' did not match expected '{}'",
                path.display(),
                document.receipt_type,
                receipt_type
            ),
        );
    }
    let entries = if entry_field == "contracts" {
        document.contracts
    } else {
        document.receipts
    };
    let mut stages = Vec::new();
    for (message_id, target, stage) in expected {
        let Some(entry) = entries
            .iter()
            .find(|entry| entry.message_id == *message_id && entry.target == *target)
        else {
            return receipt_failure_scenario(
                name,
                incomplete_stage,
                &format!(
                    "receipt {} missing expected message_id={message_id}",
                    path.display()
                ),
            );
        };
        if entry.stage != *stage || entry.status != "OBSERVED" || entry.provenance.is_empty() {
            return receipt_failure_scenario(
                name,
                malformed_stage,
                &format!(
                    "receipt {} has invalid entry for {message_id}",
                    path.display()
                ),
            );
        }
        stages.push(composed_evidence_stage(
            &entry.message_id,
            &entry.target,
            stage,
            "OBSERVED",
            &entry.provenance,
        ));
    }

    ScenarioReport {
        name,
        passed: true,
        failed_stage: None,
        message_ids: stages
            .iter()
            .map(|stage| stage.message_id.clone())
            .collect(),
        duplicates_suppressed: 0,
        malformed_targets_rejected: 0,
        stages,
        slo_contract: None,
        aggregate_slos: Vec::new(),
    }
}

fn receipt_failure_scenario(
    name: &'static str,
    failed_stage: &'static str,
    provenance: &str,
) -> ScenarioReport {
    ScenarioReport {
        name,
        passed: false,
        failed_stage: Some(failed_stage),
        message_ids: Vec::new(),
        duplicates_suppressed: 0,
        malformed_targets_rejected: 0,
        stages: vec![composed_evidence_stage(
            failed_stage,
            "receipt",
            failed_stage,
            if failed_stage.ends_with("_malformed") {
                "FAILED"
            } else {
                "BLOCKED"
            },
            provenance,
        )],
        slo_contract: None,
        aggregate_slos: Vec::new(),
    }
}

fn slo_aggregate_evidence_scenario(
    artifact_root: Option<&Path>,
    thresholds: &ProbeThresholds,
) -> ScenarioReport {
    let Some(root) = artifact_root else {
        return slo_failure_scenario(
            "slo_receipt_missing",
            "no artifact root for SLO sample receipt",
            thresholds,
            Vec::new(),
        );
    };
    let path = root.join("receipts/slo_samples.json");
    if !path.exists() {
        return slo_failure_scenario(
            "slo_receipt_missing",
            &format!("missing SLO sample receipt {}", path.display()),
            thresholds,
            Vec::new(),
        );
    }
    let data = match fs::read_to_string(&path) {
        Ok(data) => data,
        Err(error) => {
            return slo_failure_scenario(
                "slo_receipt_missing",
                &format!("read SLO sample receipt {}: {error}", path.display()),
                thresholds,
                Vec::new(),
            );
        }
    };
    let document: SloSamplesDocument = match serde_json::from_str(&data) {
        Ok(document) => document,
        Err(error) => {
            return slo_failure_scenario(
                "slo_receipt_malformed",
                &format!("parse SLO sample receipt {}: {error}", path.display()),
                thresholds,
                Vec::new(),
            );
        }
    };
    if document.receipt_type != "probe_comm_slo_samples" {
        return slo_failure_scenario(
            "slo_receipt_malformed",
            &format!(
                "SLO sample receipt {} type '{}' did not match expected 'probe_comm_slo_samples'",
                path.display(),
                document.receipt_type
            ),
            thresholds,
            Vec::new(),
        );
    }
    if document.provenance.is_empty() {
        return slo_failure_scenario(
            "slo_receipt_malformed",
            &format!("SLO sample receipt {} missing provenance", path.display()),
            thresholds,
            Vec::new(),
        );
    }
    if document.reaction_ms.is_empty() {
        return slo_failure_scenario(
            "reaction_unknown",
            &format!(
                "SLO sample receipt {} has no observable reaction samples",
                path.display()
            ),
            thresholds,
            Vec::new(),
        );
    }

    let mut aggregate_slos = Vec::new();
    for (metric, gate, samples, threshold) in [
        (
            "normal_transport",
            "p95",
            document.normal_transport_ms.as_slice(),
            thresholds.normal_transport_p95_ms,
        ),
        (
            "normal_transport",
            "max",
            document.normal_transport_ms.as_slice(),
            thresholds.normal_transport_max_ms,
        ),
        (
            "urgent_transport",
            "p95",
            document.urgent_transport_ms.as_slice(),
            thresholds.urgent_transport_p95_ms,
        ),
        (
            "urgent_transport",
            "max",
            document.urgent_transport_ms.as_slice(),
            thresholds.urgent_transport_max_ms,
        ),
        (
            "wake",
            "p95",
            document.wake_ms.as_slice(),
            thresholds.wake_p95_ms,
        ),
        (
            "reaction",
            "p95",
            document.reaction_ms.as_slice(),
            thresholds.reaction_p95_ms,
        ),
    ] {
        let Some(evidence) = aggregate_slo_evidence(
            metric,
            gate,
            samples,
            threshold,
            document.provenance.clone(),
        ) else {
            return slo_failure_scenario(
                "slo_sample_count_insufficient",
                &format!(
                    "SLO sample receipt {} has fewer than {MIN_P95_SAMPLE_COUNT} samples for {metric}_{gate}",
                    path.display()
                ),
                thresholds,
                aggregate_slos,
            );
        };
        aggregate_slos.push(evidence);
    }

    let failed_stage = aggregate_slos
        .iter()
        .find(|evidence| !evidence.passed)
        .map(|evidence| slo_failed_stage(evidence.metric, evidence.gate));
    let passed = failed_stage.is_none();
    ScenarioReport {
        name: "slo_aggregate_evidence",
        passed,
        failed_stage,
        message_ids: aggregate_slos
            .iter()
            .map(|evidence| format!("{}_{}", evidence.metric, evidence.gate))
            .collect(),
        duplicates_suppressed: 0,
        malformed_targets_rejected: 0,
        stages: vec![composed_evidence_stage(
            "slo-aggregate-receipt",
            "probe-comm",
            failed_stage.unwrap_or("slo_aggregate"),
            if passed { "OBSERVED" } else { "FAILED" },
            &document.provenance,
        )],
        slo_contract: Some(slo_contract_report(thresholds)),
        aggregate_slos,
    }
}

fn aggregate_slo_evidence(
    metric: &'static str,
    gate: &'static str,
    samples: &[u64],
    threshold_ms: u64,
    provenance: String,
) -> Option<AggregateSloEvidence> {
    if samples.len() < MIN_P95_SAMPLE_COUNT {
        return None;
    }
    let observed_ms = if gate == "max" {
        *samples.iter().max()?
    } else {
        percentile_95_ms(samples)?
    };
    Some(AggregateSloEvidence {
        metric,
        gate,
        sample_count: samples.len(),
        observed_ms,
        threshold_ms,
        passed: observed_ms <= threshold_ms,
        provenance,
    })
}

fn percentile_95_ms(samples: &[u64]) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();

    // Nearest-rank percentile: rank = ceil(p/100 * n), then convert to 0-based index.
    let index = (sorted.len() * 95).div_ceil(100).saturating_sub(1);
    sorted.get(index).copied()
}

fn slo_failed_stage(metric: &'static str, gate: &'static str) -> &'static str {
    match (metric, gate) {
        ("normal_transport", "p95") => "normal_transport_p95",
        ("normal_transport", "max") => "normal_transport_max",
        ("urgent_transport", "p95") => "urgent_transport_p95",
        ("urgent_transport", "max") => "urgent_transport_max",
        ("wake", "p95") => "wake_p95",
        ("reaction", "p95") => "reaction_p95",
        _ => "slo_aggregate",
    }
}

fn slo_contract_report(thresholds: &ProbeThresholds) -> SloContractReport {
    SloContractReport {
        units: "milliseconds",
        normal_transport_p95_ms: thresholds.normal_transport_p95_ms,
        normal_transport_max_ms: thresholds.normal_transport_max_ms,
        urgent_transport_p95_ms: thresholds.urgent_transport_p95_ms,
        urgent_transport_max_ms: thresholds.urgent_transport_max_ms,
        wake_p95_ms: thresholds.wake_p95_ms,
        reaction_p95_ms: thresholds.reaction_p95_ms,
    }
}

fn slo_failure_scenario(
    failed_stage: &'static str,
    provenance: &str,
    thresholds: &ProbeThresholds,
    aggregate_slos: Vec<AggregateSloEvidence>,
) -> ScenarioReport {
    ScenarioReport {
        name: "slo_aggregate_evidence",
        passed: false,
        failed_stage: Some(failed_stage),
        message_ids: Vec::new(),
        duplicates_suppressed: 0,
        malformed_targets_rejected: 0,
        stages: vec![composed_evidence_stage(
            failed_stage,
            "probe-comm",
            failed_stage,
            if failed_stage.ends_with("_missing")
                || failed_stage == "slo_sample_count_insufficient"
                || failed_stage == "reaction_unknown"
            {
                "BLOCKED"
            } else {
                "FAILED"
            },
            provenance,
        )],
        slo_contract: Some(slo_contract_report(thresholds)),
        aggregate_slos,
    }
}

fn composed_evidence_stage(
    message_id: &str,
    target: &str,
    stage: &str,
    status: &'static str,
    provenance: &str,
) -> MessageStageEvidence {
    MessageStageEvidence {
        message_id: message_id.to_string(),
        target: target.to_string(),
        enqueued_at_ms: None,
        selected_at_ms: None,
        delivered_at_ms: None,
        wake_at_ms: None,
        first_reaction_at_ms: None,
        enqueued_status: "UNKNOWN",
        selected_status: "UNKNOWN",
        delivered_status: status,
        wake_status: "UNKNOWN",
        reaction_status: Some(status.to_string()),
        stage_statuses: vec![StageStatusEvidence {
            stage: stage.to_string(),
            status: status.to_string(),
            provenance: provenance.to_string(),
        }],
        terminal: "evidence_composed",
    }
}

/// Location of a probe-generated disposable Cassy root: disk-backed
/// `~/.cas/scratch/cas-probe-comm-<uuid>`, never `$TMPDIR` (GH #704).
fn generated_scratch_root_path() -> PathBuf {
    crate::temp_hygiene::default_scratch_root(&format!(
        "cas-probe-comm-{}",
        uuid::Uuid::new_v4()
    ))
}

/// Removes a probe-generated disposable root when the run ends, however it
/// ends. A root the operator named stays put: it is theirs, not ours.
struct ScratchRootGuard {
    owned: Option<PathBuf>,
}

impl ScratchRootGuard {
    fn owning(path: PathBuf) -> Self {
        Self { owned: Some(path) }
    }

    fn borrowed() -> Self {
        Self { owned: None }
    }
}

impl Drop for ScratchRootGuard {
    fn drop(&mut self) {
        let Some(path) = self.owned.take() else {
            return;
        };
        if let Err(error) = fs::remove_dir_all(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "warning: failed to remove probe-comm scratch root {}: {error}",
                path.display()
            );
        }
    }
}

fn guard_active_parent_root(
    run_root: &Path,
    parent_cas_root: Option<&Path>,
    allow_active_cas_root: bool,
) -> Result<()> {
    if allow_active_cas_root {
        return Ok(());
    }

    let Some(parent) = parent_cas_root else {
        return Ok(());
    };

    if equivalent_paths(run_root, parent) {
        bail!(
            "refusing to run probe-comm in the active parent Cassy root {}; pass the explicit safe override only for disposable roots",
            run_root.display()
        );
    }

    Ok(())
}

fn equivalent_paths(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn run_scenario(
    scenario: ScenarioDef,
    thresholds: &ProbeThresholds,
    failure: Option<&ProbeFailure>,
    clock: &mut FakeClock,
) -> ScenarioReport {
    let mut stages = Vec::with_capacity(scenario.messages.len());
    let mut failed_stage = None;

    for message_id in scenario.messages {
        let evidence = run_message(scenario, message_id, thresholds, failure, clock);
        if evidence.terminal == "transport_failed" {
            failed_stage = Some("delivered");
        } else if evidence.terminal == "delivery_slo_failed" {
            failed_stage = Some("delivery_slo");
        } else if let (Some(selected), Some(enqueued)) =
            (evidence.selected_at_ms, evidence.enqueued_at_ms)
            && selected.saturating_sub(enqueued) > thresholds.selection_ms
        {
            failed_stage = Some("selected");
        }
        stages.push(evidence);
    }

    let duplicates_suppressed = u32::from(scenario.duplicate_of.is_some());
    let malformed_targets_rejected = u32::from(scenario.malformed_target);
    let message_ids = stages
        .iter()
        .filter(|stage| stage.terminal == "delivered")
        .map(|stage| stage.message_id.clone())
        .collect();

    ScenarioReport {
        name: scenario.name,
        passed: failed_stage.is_none(),
        failed_stage,
        message_ids,
        duplicates_suppressed,
        malformed_targets_rejected,
        stages,
        slo_contract: None,
        aggregate_slos: Vec::new(),
    }
}

fn run_message(
    scenario: ScenarioDef,
    message_id: &str,
    thresholds: &ProbeThresholds,
    failure: Option<&ProbeFailure>,
    clock: &mut FakeClock,
) -> MessageStageEvidence {
    let enqueued_at_ms = clock.tick(1);
    let selected_at_ms = clock.tick(1);
    let target = if scenario.malformed_target {
        "missing-worker"
    } else {
        "worker-a"
    };

    if scenario.malformed_target {
        return MessageStageEvidence {
            message_id: message_id.to_string(),
            target: target.to_string(),
            enqueued_at_ms: Some(enqueued_at_ms),
            selected_at_ms: Some(selected_at_ms),
            delivered_at_ms: None,
            wake_at_ms: None,
            first_reaction_at_ms: None,
            enqueued_status: "OBSERVED",
            selected_status: "OBSERVED",
            delivered_status: "FAILED",
            wake_status: "UNKNOWN",
            reaction_status: Some("UNKNOWN".to_string()),
            stage_statuses: fake_stage_statuses(
                "target_rejected",
                enqueued_at_ms,
                selected_at_ms,
                None,
            ),
            terminal: "target_rejected",
        };
    }

    if matches!(
        failure,
        Some(ProbeFailure::Transport { scenario: s, message_id: id })
            if s == scenario.name && id == message_id
    ) {
        return MessageStageEvidence {
            message_id: message_id.to_string(),
            target: target.to_string(),
            enqueued_at_ms: Some(enqueued_at_ms),
            selected_at_ms: Some(selected_at_ms),
            delivered_at_ms: None,
            wake_at_ms: None,
            first_reaction_at_ms: None,
            enqueued_status: "OBSERVED",
            selected_status: "OBSERVED",
            delivered_status: "FAILED",
            wake_status: "UNKNOWN",
            reaction_status: Some("UNKNOWN".to_string()),
            stage_statuses: fake_stage_statuses(
                "transport_failed",
                enqueued_at_ms,
                selected_at_ms,
                None,
            ),
            terminal: "transport_failed",
        };
    }

    let delivery_delta = match failure {
        Some(ProbeFailure::Slo {
            scenario: s,
            message_id: id,
            delivered_after_ms,
        }) if s == scenario.name && id == message_id => *delivered_after_ms,
        _ => thresholds.delivery_ms.min(4),
    };
    let delivered_at_ms = clock.advance_to(enqueued_at_ms + delivery_delta);
    let terminal = if delivered_at_ms.saturating_sub(enqueued_at_ms) > thresholds.delivery_ms {
        "delivery_slo_failed"
    } else {
        "delivered"
    };

    MessageStageEvidence {
        message_id: message_id.to_string(),
        target: target.to_string(),
        enqueued_at_ms: Some(enqueued_at_ms),
        selected_at_ms: Some(selected_at_ms),
        delivered_at_ms: Some(delivered_at_ms),
        wake_at_ms: None,
        first_reaction_at_ms: None,
        enqueued_status: "OBSERVED",
        selected_status: "OBSERVED",
        delivered_status: if terminal == "delivered" {
            "OBSERVED"
        } else {
            "FAILED"
        },
        wake_status: "UNKNOWN",
        reaction_status: None,
        stage_statuses: fake_stage_statuses(
            terminal,
            enqueued_at_ms,
            selected_at_ms,
            Some(delivered_at_ms),
        ),
        terminal,
    }
}

fn fake_stage_statuses(
    terminal: &'static str,
    enqueued_at_ms: u64,
    selected_at_ms: u64,
    delivered_at_ms: Option<u64>,
) -> Vec<StageStatusEvidence> {
    let mut statuses = vec![
        StageStatusEvidence {
            stage: "enqueued".to_string(),
            status: "OBSERVED".to_string(),
            provenance: format!("fake adapter clock observed enqueue at {enqueued_at_ms}ms"),
        },
        StageStatusEvidence {
            stage: "selected".to_string(),
            status: "OBSERVED".to_string(),
            provenance: format!("fake adapter clock observed selection at {selected_at_ms}ms"),
        },
    ];
    statuses.push(match delivered_at_ms {
        Some(ts) => StageStatusEvidence {
            stage: "delivered".to_string(),
            status: if terminal == "delivered" {
                "OBSERVED"
            } else {
                "FAILED"
            }
            .to_string(),
            provenance: format!("fake adapter clock observed delivery at {ts}ms"),
        },
        None => StageStatusEvidence {
            stage: "delivered".to_string(),
            status: "FAILED".to_string(),
            provenance: format!("fake adapter terminal outcome {terminal}"),
        },
    });
    statuses
}

#[derive(Default)]
struct FakeClock {
    now_ms: u64,
}

impl FakeClock {
    fn tick(&mut self, delta_ms: u64) -> u64 {
        self.now_ms += delta_ms;
        self.now_ms
    }

    fn advance_to(&mut self, timestamp_ms: u64) -> u64 {
        self.now_ms = self.now_ms.max(timestamp_ms);
        self.now_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn read_jsonl(path: &std::path::Path) -> Vec<Value> {
        let data = std::fs::read_to_string(path).expect("jsonl should be written");
        data.lines()
            .map(|line| serde_json::from_str(line).expect("line should be valid json"))
            .collect()
    }

    fn base_config(jsonl: std::path::PathBuf, cas_root: std::path::PathBuf) -> ProbeCommConfig {
        ProbeCommConfig {
            jsonl,
            cas_root: Some(cas_root),
            artifact_root: None,
            allow_active_cas_root: true,
            adapter: Some(ProbeAdapterKind::Fake),
            thresholds: ProbeThresholds::epic_defaults(),
            failure: None,
        }
    }

    #[test]
    fn percentile_95_uses_nearest_rank_boundaries() {
        assert_eq!(percentile_95_ms(&[]), None);
        assert_eq!(percentile_95_ms(&[42]), Some(42));
        assert_eq!(percentile_95_ms(&[100, 300, 500, 800, 2500]), Some(2500));

        let twenty: Vec<u64> = (1..=20).collect();
        assert_eq!(percentile_95_ms(&twenty), Some(19));

        let twenty_one: Vec<u64> = (1..=21).collect();
        assert_eq!(percentile_95_ms(&twenty_one), Some(20));
    }

    #[test]
    fn fake_adapter_happy_path_writes_all_stage_evidence_jsonl() {
        let temp = tempfile::tempdir().unwrap();
        let jsonl = temp.path().join("probe.jsonl");

        let report = run_probe_comm(base_config(jsonl.clone(), temp.path().join(".cas")))
            .expect("happy path should pass");

        assert!(report.passed, "report should pass: {report:?}");
        assert_eq!(report.scenarios.len(), 7);
        assert!(report.scenarios.iter().all(|scenario| scenario.passed));

        let lines = read_jsonl(&jsonl);
        assert_eq!(lines.len(), 7);

        let names: Vec<_> = lines
            .iter()
            .map(|line| line["scenario"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "startup",
                "serial_10",
                "fifo_burst",
                "urgent",
                "duplicate_replay",
                "malformed_target",
                "lifecycle",
            ]
        );

        let serial = lines
            .iter()
            .find(|line| line["scenario"] == "serial_10")
            .expect("serial scenario should be present");
        assert_eq!(serial["message_ids"].as_array().unwrap().len(), 10);
        for stage in serial["stages"].as_array().unwrap() {
            assert!(stage["enqueued_at_ms"].as_u64().is_some());
            assert!(stage["selected_at_ms"].as_u64().is_some());
            assert!(stage["delivered_at_ms"].as_u64().is_some());
        }
    }

    #[test]
    fn fifo_and_duplicate_replay_assertions_are_deterministic() {
        let temp = tempfile::tempdir().unwrap();
        let jsonl = temp.path().join("probe.jsonl");

        let report = run_probe_comm(base_config(jsonl.clone(), temp.path().join(".cas")))
            .expect("happy path should pass");
        let fifo = report
            .scenarios
            .iter()
            .find(|scenario| scenario.name == "fifo_burst")
            .unwrap();
        assert_eq!(
            fifo.message_ids,
            vec!["fifo-0", "fifo-1", "fifo-2", "fifo-3", "fifo-4"]
        );

        let replay = report
            .scenarios
            .iter()
            .find(|scenario| scenario.name == "duplicate_replay")
            .unwrap();
        assert_eq!(replay.message_ids, vec!["replay-original"]);

        let lines = read_jsonl(&jsonl);
        let replay_json = lines
            .iter()
            .find(|line| line["scenario"] == "duplicate_replay")
            .unwrap();
        assert_eq!(replay_json["duplicates_suppressed"], 1);
    }

    #[test]
    fn active_parent_cas_root_is_rejected_without_override() {
        let temp = tempfile::tempdir().unwrap();
        let jsonl = temp.path().join("probe.jsonl");
        let parent_root = temp.path().join(".cas");
        std::fs::create_dir_all(&parent_root).unwrap();

        let mut config = base_config(jsonl, parent_root.clone());
        config.allow_active_cas_root = false;

        let err = run_probe_comm_with_parent(config, Some(&parent_root))
            .expect_err("active parent root should be rejected");
        assert!(err.to_string().contains("active parent Cassy root"));
    }

    #[test]
    fn missing_adapter_and_malformed_output_path_fail_clearly() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = base_config(temp.path().join("probe.jsonl"), temp.path().join(".cas"));
        config.adapter = None;
        let err = run_probe_comm(config).expect_err("missing adapter should fail");
        assert!(err.to_string().contains("probe adapter is required"));

        let dir_output = temp.path().join("jsonl-dir");
        std::fs::create_dir_all(&dir_output).unwrap();
        let config = base_config(dir_output, temp.path().join(".cas"));
        let err = run_probe_comm(config).expect_err("directory output path should fail");
        assert!(err.to_string().contains("jsonl output path"));
    }

    /// GH #704: the probe leaked 1,242 `cas-probe-comm-*` roots into a
    /// 32 GB tmpfs `/tmp` before it filled and broke every live session.
    /// A run that generates its own root must take it with it.
    #[test]
    fn generated_scratch_root_is_removed_after_a_successful_run() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let temp = tempfile::tempdir().unwrap();
            let before = probe_root_snapshot(home);
            let mut config = base_config(temp.path().join("probe.jsonl"), temp.path().join(".cas"));
            config.cas_root = None;

            run_probe_comm(config).expect("happy path should pass");

            assert_eq!(
                probe_root_snapshot(home),
                before,
                "probe-comm must not leave its generated root behind"
            );
        });
    }

    #[test]
    fn generated_scratch_root_is_removed_after_a_failed_run() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let temp = tempfile::tempdir().unwrap();
            let before = probe_root_snapshot(home);
            let mut config = base_config(temp.path().join("probe.jsonl"), temp.path().join(".cas"));
            config.cas_root = None;
            config.failure = Some(ProbeFailure::Transport {
                scenario: "urgent".to_string(),
                message_id: "urgent-0".to_string(),
            });

            run_probe_comm(config).expect_err("injected transport failure should fail the run");

            assert_eq!(
                probe_root_snapshot(home),
                before,
                "the failure path must clean up too"
            );
        });
    }

    /// The default lives on disk (`~/.cas/scratch/<name>`), not in RAM.
    #[test]
    fn generated_scratch_root_defaults_under_home_cas_scratch() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let root = generated_scratch_root_path();
            assert!(
                root.starts_with(home.join(".cas").join("scratch")),
                "generated root {} should live under ~/.cas/scratch",
                root.display()
            );
            assert!(
                root.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("cas-probe-comm-"),
                "generated root should stay identifiable: {}",
                root.display()
            );
        });
    }

    /// An operator-supplied root is theirs: never removed.
    #[test]
    fn explicit_cas_root_is_left_in_place() {
        let temp = tempfile::tempdir().unwrap();
        let explicit = temp.path().join("mine").join(".cas");
        let config = base_config(temp.path().join("probe.jsonl"), explicit.clone());

        run_probe_comm(config).expect("happy path should pass");

        assert!(
            explicit.exists(),
            "an explicit --cas-root must survive the run"
        );
    }

    /// Every `cas-probe-comm-*` directory in the two places a generated root
    /// could land: the scratch base under HOME (current) and `$TMPDIR`
    /// (pre-fix). Compared before/after a run so pre-existing strays on the
    /// host cannot make the assertion lie in either direction.
    fn probe_root_snapshot(home: &std::path::Path) -> std::collections::BTreeSet<PathBuf> {
        [home.join(".cas").join("scratch"), std::env::temp_dir()]
            .into_iter()
            .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().starts_with("cas-probe-comm-"))
                    .unwrap_or(false)
            })
            .collect()
    }

    #[test]
    fn injected_transport_and_slo_failures_return_failed_stage() {
        let temp = tempfile::tempdir().unwrap();

        let mut transport = base_config(
            temp.path().join("transport.jsonl"),
            temp.path().join(".cas"),
        );
        transport.failure = Some(ProbeFailure::Transport {
            scenario: "urgent".to_string(),
            message_id: "urgent-0".to_string(),
        });
        let err = run_probe_comm(transport).expect_err("transport failure should fail run");
        assert!(err.to_string().contains("urgent"));
        assert!(err.to_string().contains("delivered"));

        let mut slo = base_config(temp.path().join("slo.jsonl"), temp.path().join(".cas"));
        slo.thresholds.delivery_ms = 1;
        slo.failure = Some(ProbeFailure::Slo {
            scenario: "serial_10".to_string(),
            message_id: "serial-0".to_string(),
            delivered_after_ms: 50,
        });
        let err = run_probe_comm(slo).expect_err("SLO failure should fail run");
        assert!(err.to_string().contains("serial_10"));
        assert!(err.to_string().contains("delivery_slo"));
    }
}
