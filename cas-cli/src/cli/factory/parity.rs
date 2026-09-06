//! Launched-agent skill + instruction parity conformance gate (cas-bd9d).
//!
//! This is the integration proof for the catalog-parity (cas-cc8c/cas-20f2) and
//! role-instruction-injection (cas-0263) tasks. It builds a deterministic 4×2
//! matrix — Claude / Codex / Grok / OpenCode × supervisor / worker — and, for each cell,
//! evaluates staged evidence drawn from the REAL launch configuration:
//!
//!   1. `catalog`             — every canonical required + general skill twin and
//!                              required agent is discoverable in THAT harness's
//!                              own catalog (never by inheriting another home).
//!   2. `instruction_source`  — the surface the runtime actually consumes exists
//!                              in its launch config (Codex `developer_instructions`,
//!                              Grok `--rules`, Claude the queued launch intro).
//!   3. `launch_args`         — the concrete instruction text is captured from the
//!                              real launch config (PtyConfig argv / queued prompt)
//!                              and is non-empty.
//!   4. `effective_contract`  — every canonical role clause (coordinate-only /
//!                              one-task, async handling, task lifecycle,
//!                              merge/re-close, urgent-stop recovery) is present.
//!   5. `prefix`              — the surface issues tool CALLS with the harness's
//!                              own MCP prefix and never a cross-harness one.
//!
//! Every stage resolves to PASS or a stage-specific FAIL — a stage that cannot be
//! observed is FAIL, never UNKNOWN-reported-as-PASS (AC-4). It composes with, and
//! does not duplicate, the `cas factory probe-comm` COMMUNICATION conformance
//! gate (cas-c4d6): probe-comm proves transport/wake/reaction; this gate proves
//! skill discovery + effective role instructions at launch.

use std::path::PathBuf;

use anyhow::Result;
use cas_mux::{
    ContractRole, OpenCodeProjectionSpec, OpenCodeRole, PtyConfig, SupervisorCli,
    render_opencode_config,
};
use serde::Serialize;
use serde_json::Value;

use crate::builtins::{
    GENERAL_PARITY_CAPABILITIES, REQUIRED_FACTORY_AGENTS, REQUIRED_FACTORY_CAPABILITIES,
    agent_catalog_for_harness, required_dir_for, skill_catalog_for_harness,
};

/// Stable lowercase harness name used by `cas_mux::rendered_contract_surface`.
fn harness_name(h: SupervisorCli) -> &'static str {
    match h {
        SupervisorCli::Claude => "claude",
        SupervisorCli::Codex => "codex",
        SupervisorCli::Grok => "grok",
        // cas-d139 owns OpenCode parity registration.
        SupervisorCli::OpenCode => "opencode",
    }
}

fn role_name(r: ContractRole) -> &'static str {
    match r {
        ContractRole::Supervisor => "supervisor",
        ContractRole::Worker => "worker",
    }
}

/// One stage's outcome. `PASS`/`FAIL` only — there is no `UNKNOWN` that reads as
/// success; an unobservable stage is reported `FAIL` with the reason.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StageResult {
    pub stage: &'static str,
    pub status: &'static str,
    pub detail: String,
}

impl StageResult {
    fn pass(stage: &'static str, detail: impl Into<String>) -> Self {
        Self {
            stage,
            status: "PASS",
            detail: detail.into(),
        }
    }
    fn fail(stage: &'static str, detail: impl Into<String>) -> Self {
        Self {
            stage,
            status: "FAIL",
            detail: detail.into(),
        }
    }
    fn passed(&self) -> bool {
        self.status == "PASS"
    }
}

/// One (harness, role) cell of the parity matrix.
#[derive(Debug, Clone, Serialize)]
pub struct ParityCell {
    pub harness: &'static str,
    pub role: &'static str,
    pub stages: Vec<StageResult>,
    pub passed: bool,
}

/// The full 4×2 machine-readable parity report.
#[derive(Debug, Clone, Serialize)]
pub struct ParityReport {
    pub cells: Vec<ParityCell>,
    pub all_passed: bool,
    /// Links this gate to the sibling communication conformance surface so the
    /// two are composed, not duplicated (cas-c4d6).
    pub composes_with: &'static str,
}

/// Capture the concrete role-instruction text a harness/role actually receives
/// from its REAL launch configuration — the argv value for Codex/Grok, and the
/// queued launch intro prompt for Claude. `Err` when the wiring is missing (that
/// becomes a stage-specific FAIL, never a silent pass).
fn launch_instruction_text(harness: SupervisorCli, role: ContractRole) -> Result<String, String> {
    let role_arg = role_name(role);
    match harness {
        SupervisorCli::Codex => {
            let cfg = PtyConfig::codex(
                "probe",
                role_arg,
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None,
                None,
            );
            // Codex injects the contract as `--config developer_instructions="…"`.
            cfg.args
                .iter()
                .find(|a| a.contains("developer_instructions"))
                .cloned()
                .ok_or_else(|| {
                    "codex launch argv is missing --config developer_instructions".to_string()
                })
        }
        SupervisorCli::Grok => {
            let cfg = PtyConfig::grok(
                "probe",
                role_arg,
                PathBuf::from("/tmp"),
                None,
                None,
                None,
                None,
                None,
                None,
            );
            // Grok injects the contract via `--rules <value>`.
            let pos = cfg
                .args
                .iter()
                .position(|a| a == "--rules")
                .ok_or_else(|| "grok launch argv is missing --rules".to_string())?;
            cfg.args
                .get(pos + 1)
                .cloned()
                .ok_or_else(|| "grok --rules flag has no value".to_string())
        }
        SupervisorCli::Claude => {
            // Claude has no launch instruction flag; the surface it consumes is
            // the launch-time intro prompt Cassy queues. That prompt is exactly the
            // canonical `claude_*_contract` builder output — the queue write is
            // only the delivery step. The runner captures the authoritative
            // CONTENT here; that the app layer actually enqueues it is proven
            // separately by the real-enqueue test
            // `app::tests::test_claude_intro_prompts_carry_full_role_contract`
            // and by this gate's `claude_launch_intro_is_actually_wired` test.
            Ok(match role {
                ContractRole::Supervisor => cas_mux::claude_supervisor_contract("probe-worker"),
                ContractRole::Worker => cas_mux::claude_worker_contract("probe-worker"),
            })
        }
        // cas-d139 owns the retained OpenCode instruction-parity proof.
        SupervisorCli::OpenCode => {
            let selected_role = match role {
                ContractRole::Supervisor => OpenCodeRole::Supervisor,
                ContractRole::Worker => OpenCodeRole::Worker,
            };
            let config = render_opencode_config(&OpenCodeProjectionSpec::new(
                selected_role,
                selected_role.agent_name(),
                "parity-cas-session",
                "/tmp/cas-root",
                "/tmp/worktree",
                "/tmp/cas-root/opencode/cassy-opencode-plugin.mjs",
            ));
            let json: Value = serde_json::from_str(&config)
                .map_err(|error| format!("generated OpenCode config is invalid JSON: {error}"))?;
            json["agent"][selected_role.agent_name()]["prompt"]
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| {
                    format!(
                        "generated OpenCode config is missing {} agent.prompt",
                        selected_role.agent_name()
                    )
                })
        }
    }
}

/// Foreign tool-CALL forms that must never appear in a harness's own surface.
/// Checked on the CALL form (`<prefix>task`), not the bare prefix, so a surface
/// may legitimately name the others in "not mcp__cas__ or mcp__cs__" guidance.
fn foreign_call_forms(harness: SupervisorCli) -> &'static [&'static str] {
    match harness {
        SupervisorCli::Claude => &["mcp__cs__task", "mcp__cs__coordination"],
        SupervisorCli::Codex => &["mcp__cas__task", "mcp__cas__coordination"],
        SupervisorCli::Grok => &[
            "mcp__cas__task",
            "mcp__cas__coordination",
            "mcp__cs__task",
            "mcp__cs__coordination",
        ],
        // cas-d139 owns OpenCode parity; reject every existing foreign prefix.
        SupervisorCli::OpenCode => &[
            "mcp__cas__task",
            "mcp__cas__coordination",
            "mcp__cs__task",
            "mcp__cs__coordination",
            "cas__task",
            "cas__coordination",
        ],
    }
}

fn own_call_forms(harness: SupervisorCli) -> [&'static str; 2] {
    match harness {
        SupervisorCli::Claude => ["mcp__cas__task", "mcp__cas__coordination"],
        SupervisorCli::Codex => ["mcp__cs__task", "mcp__cs__coordination"],
        SupervisorCli::Grok => ["cas__task", "cas__coordination"],
        // cas-d139 owns the full parity catalog; the namespace itself is fixed.
        SupervisorCli::OpenCode => ["cas_task", "cas_coordination"],
    }
}

/// Evaluate one matrix cell against the real launch configuration.
pub fn evaluate_cell(harness: SupervisorCli, role: ContractRole) -> ParityCell {
    let mut stages = Vec::new();

    // --- Stage 1: catalog ---------------------------------------------------
    {
        let skills = skill_catalog_for_harness(harness);
        let agents = agent_catalog_for_harness(harness);
        let mut missing: Vec<String> = Vec::new();
        for cap in REQUIRED_FACTORY_CAPABILITIES
            .iter()
            .chain(GENERAL_PARITY_CAPABILITIES.iter())
        {
            if let Some(dir) = required_dir_for(cap, harness) {
                let skill_md = format!("{dir}/SKILL.md");
                if !skills.iter().any(|b| b.path == skill_md) {
                    missing.push(format!("skill:{}", cap.id));
                }
            }
        }
        for agent in REQUIRED_FACTORY_AGENTS {
            if !agents.iter().any(|b| &b.path == agent) {
                missing.push(format!("agent:{agent}"));
            }
        }
        stages.push(if missing.is_empty() {
            StageResult::pass(
                "catalog",
                format!(
                    "{} required+general skills and {} required agents discoverable in own catalog",
                    REQUIRED_FACTORY_CAPABILITIES.len() + GENERAL_PARITY_CAPABILITIES.len(),
                    REQUIRED_FACTORY_AGENTS.len()
                ),
            )
        } else {
            StageResult::fail(
                "catalog",
                format!("missing from own catalog: {}", missing.join(", ")),
            )
        });
    }

    // --- Resolve the real launch instruction once (used by stages 2-5) ------
    let launch = launch_instruction_text(harness, role);

    // --- Stage 2: instruction_source ---------------------------------------
    let source_name = match harness {
        SupervisorCli::Codex => "--config developer_instructions",
        SupervisorCli::Grok => "--rules",
        SupervisorCli::Claude => "queued launch intro prompt",
        // cas-d139 owns the retained OpenCode parity proof.
        SupervisorCli::OpenCode => "OPENCODE_CONFIG_CONTENT agent.prompt",
    };
    stages.push(match &launch {
        Ok(_) => StageResult::pass(
            "instruction_source",
            format!("contract carried via {source_name}"),
        ),
        Err(e) => StageResult::fail("instruction_source", e.clone()),
    });

    // --- Stage 3: launch_args ----------------------------------------------
    stages.push(match &launch {
        Ok(text) if !text.trim().is_empty() => StageResult::pass(
            "launch_args",
            format!(
                "{} bytes of instruction captured from real launch config",
                text.len()
            ),
        ),
        Ok(_) => StageResult::fail(
            "launch_args",
            "launch instruction text is empty".to_string(),
        ),
        Err(e) => StageResult::fail("launch_args", e.clone()),
    });

    // --- Stage 4: effective_contract ---------------------------------------
    // Evaluate against the SAME text the runtime consumes (rendered surface for
    // Codex/Grok constants; the queued prompt for Claude). Both derive from the
    // canonical contract, so either proves the clauses are effective.
    let surface = rendered_surface_or_launch(harness, role, &launch);
    stages.push(match &surface {
        Ok(text) => {
            let missing = cas_mux::missing_contract_elements(text, role);
            if missing.is_empty() {
                StageResult::pass(
                    "effective_contract",
                    "all canonical role clauses present".to_string(),
                )
            } else {
                StageResult::fail(
                    "effective_contract",
                    format!("missing role clauses: {}", missing.join(", ")),
                )
            }
        }
        Err(e) => StageResult::fail("effective_contract", e.clone()),
    });

    // --- Stage 5: prefix ----------------------------------------------------
    stages.push(match &surface {
        Ok(text) => {
            let own = own_call_forms(harness);
            let has_own = own.iter().any(|m| text.contains(m));
            let leaked: Vec<&str> = foreign_call_forms(harness)
                .iter()
                .copied()
                .filter(|m| text.contains(m))
                .collect();
            if !has_own {
                StageResult::fail(
                    "prefix",
                    format!("surface issues no {}-prefixed tool call", own[0]),
                )
            } else if !leaked.is_empty() {
                StageResult::fail("prefix", format!("leaks cross-harness call(s): {leaked:?}"))
            } else {
                StageResult::pass(
                    "prefix",
                    format!("uses own prefix ({}), no foreign calls", own[0]),
                )
            }
        }
        Err(e) => StageResult::fail("prefix", e.clone()),
    });

    let passed = stages.iter().all(StageResult::passed);
    ParityCell {
        harness: harness_name(harness),
        role: role_name(role),
        stages,
        passed,
    }
}

/// The text to evaluate the contract/prefix against: the launch text when it was
/// captured, else the canonical rendered surface (kept in sync — both derive
/// from the same canonical contract).
fn rendered_surface_or_launch(
    harness: SupervisorCli,
    role: ContractRole,
    launch: &Result<String, String>,
) -> Result<String, String> {
    match launch {
        Ok(text) => Ok(text.clone()),
        // If the launch capture failed, fall back to the canonical rendered
        // surface so the contract/prefix stages still report their own truthful
        // status rather than cascading the launch failure into every stage.
        Err(_) => Ok(cas_mux::rendered_contract_surface(
            harness_name(harness),
            role,
        )),
    }
}

/// Build the full 4×2 parity matrix report.
pub fn run_parity_matrix() -> ParityReport {
    let mut cells = Vec::with_capacity(8);
    for harness in [
        SupervisorCli::Claude,
        SupervisorCli::Codex,
        SupervisorCli::Grok,
        SupervisorCli::OpenCode,
    ] {
        for role in [ContractRole::Supervisor, ContractRole::Worker] {
            cells.push(evaluate_cell(harness, role));
        }
    }
    let all_passed = cells.iter().all(|c| c.passed);
    ParityReport {
        cells,
        all_passed,
        composes_with: "cas factory probe-comm (communication conformance, cas-c4d6)",
    }
}

/// `cas factory parity` entry point. Emits the machine-readable report to stdout
/// (and `--jsonl` if given) and returns an error when any cell fails, so CI gates
/// on the exit code.
pub fn execute_parity(jsonl: Option<PathBuf>) -> Result<()> {
    let report = run_parity_matrix();
    let json = serde_json::to_string_pretty(&report)?;
    println!("{json}");
    if let Some(path) = jsonl {
        // One JSON object per cell (machine-readable, greppable per shape).
        let lines: Vec<String> = report
            .cells
            .iter()
            .map(|c| serde_json::to_string(c).unwrap_or_default())
            .collect();
        std::fs::write(&path, format!("{}\n", lines.join("\n")))?;
    }
    if !report.all_passed {
        let failed: Vec<String> = report
            .cells
            .iter()
            .filter(|c| !c.passed)
            .map(|c| format!("{}/{}", c.harness, c.role))
            .collect();
        anyhow::bail!("parity conformance FAILED for cells: {}", failed.join(", "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The harness-specific ambient-recall delivery contract for a supervisor
    /// launch. This is deliberately separate from the skill contract because
    /// Claude receives its packet through SessionStart while Codex and Grok
    /// receive it through their queued launch intros.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum AmbientRecallDelivery {
        /// Claude reads its SessionStart hook's stdout.
        SessionStart,
        /// Grok ignores SessionStart stdout, so the factory's queued launch
        /// intro is its delivery channel.
        QueuedSupervisorIntro,
    }

    #[derive(Debug, Clone, Copy)]
    struct SupervisorHarnessRequirement {
        supervisor_skills: [&'static str; 3],
        ambient_recall: AmbientRecallDelivery,
    }

    /// The load-bearing exhaustive declaration for supervisor harnesses.
    ///
    /// Do not add a catch-all arm: a new `SupervisorCli` variant must fail to
    /// compile here until its real skill and ambient-recall delivery contract
    /// are registered and tested below.
    fn supervisor_harness_requirement(harness: SupervisorCli) -> SupervisorHarnessRequirement {
        match harness {
            SupervisorCli::Claude => SupervisorHarnessRequirement {
                supervisor_skills: [
                    "cas-supervisor",
                    "cas-supervisor-checklist",
                    "cas-codebase-design",
                ],
                ambient_recall: AmbientRecallDelivery::SessionStart,
            },
            SupervisorCli::Codex => SupervisorHarnessRequirement {
                // Codex's checklist is the intentional no-hooks compensation
                // twin. It is still the universal supervisor-checklist
                // capability, but its shipped skill name is harness-specific.
                supervisor_skills: [
                    "cas-supervisor",
                    "cas-codex-supervisor-checklist",
                    "cas-codebase-design",
                ],
                ambient_recall: AmbientRecallDelivery::QueuedSupervisorIntro,
            },
            SupervisorCli::Grok => SupervisorHarnessRequirement {
                supervisor_skills: [
                    "cas-supervisor",
                    "cas-supervisor-checklist",
                    "cas-codebase-design",
                ],
                ambient_recall: AmbientRecallDelivery::QueuedSupervisorIntro,
            },
            // OpenCode receives the same generated supervisor skill projection
            // and queued ambient-recall delivery as Grok.
            SupervisorCli::OpenCode => SupervisorHarnessRequirement {
                supervisor_skills: [
                    "cas-supervisor",
                    "cas-supervisor-checklist",
                    "cas-codebase-design",
                ],
                ambient_recall: AmbientRecallDelivery::QueuedSupervisorIntro,
            },
        }
    }

    /// Capture the concrete queued supervisor launch payload. The queued intro
    /// is the ambient-recall delivery seam for Codex and Grok; Claude's own
    /// ambient packet is delivered by SessionStart below.
    /// This is a second exhaustive match so a new backend cannot be quietly
    /// covered by a generic fallback.
    fn supervisor_skill_payload(cas_dir: &std::path::Path, harness: SupervisorCli) -> String {
        match harness {
            SupervisorCli::Claude
            | SupervisorCli::Codex
            | SupervisorCli::Grok
            | SupervisorCli::OpenCode => {
                use crate::store::detect::open_prompt_queue_store;
                use crate::ui::factory::queue_supervisor_intro_prompt;

                let target = format!("parity-supervisor-{}", harness.backend().name());
                let session_id = format!("parity-session-{}", harness.backend().name());
                let factory_session = format!("parity-factory-{}", harness.backend().name());
                queue_supervisor_intro_prompt(
                    cas_dir,
                    &target,
                    harness,
                    &[],
                    Some(&session_id),
                    Some(&factory_session),
                );
                let rows = open_prompt_queue_store(cas_dir)
                    .unwrap()
                    .peek_for_targets(&[target.as_str()], Some(&factory_session), 10)
                    .unwrap();
                assert_eq!(
                    rows.len(),
                    1,
                    "{harness:?} must queue exactly one supervisor launch intro"
                );
                rows[0].prompt.clone()
            }
        }
    }

    /// Capture the actual Claude SessionStart payload, whose stdout is the
    /// declared ambient-recall delivery seam. The fixture is hermetic because
    /// session start otherwise consults host-scoped context under `HOME`.
    fn claude_session_start_payload(cas_dir: &std::path::Path) -> String {
        use crate::hooks::{HookInput, handle_session_start};

        let input = HookInput {
            session_id: "parity-session".to_string(),
            cwd: cas_dir.parent().unwrap().to_string_lossy().into_owned(),
            hook_event_name: "SessionStart".to_string(),
            agent_role: Some("supervisor".to_string()),
            ..Default::default()
        };
        let output = handle_session_start(&input, Some(cas_dir)).unwrap();
        serde_json::to_value(output)
            .unwrap()
            .pointer("/hookSpecificOutput/additionalContext")
            .and_then(|value| value.as_str())
            .expect("Claude SessionStart must emit additional context")
            .to_string()
    }

    /// cas-9faa: prove every declared supervisor launch contract against the
    /// real delivery seam. A new `SupervisorCli` variant fails compilation in
    /// `supervisor_harness_requirement` and `supervisor_skill_payload` until
    /// its skill and ambient-recall policy are explicitly declared.
    #[test]
    fn supervisor_launch_context_parity_is_exhaustive_and_declared() {
        use crate::store::{init_cas_dir, open_agent_store, open_store, open_task_store_local};
        use crate::test_support::TestEnvGuard;
        use crate::types::{Agent, AgentRole, Entry, Task, TaskStatus};

        let mut env = TestEnvGuard::temp_home();
        env.set("CAS_AGENT_ROLE", "supervisor");
        env.set("CAS_AGENT_NAME", "parity-supervisor");
        env.set("CAS_FACTORY_SESSION", "parity-factory");
        env.set("CAS_FACTORY_SUPERVISOR_CLI", "claude");
        env.remove("CAS_CLONE_PATH");
        env.remove("CLAUDE_CONFIG_DIR");
        env.remove(crate::internal_llm::INTERNAL_LLM_ENV);

        let project = tempfile::tempdir().unwrap();
        let cas_dir = init_cas_dir(project.path()).unwrap();
        let mut task = Task::new(
            "cas-launch-recovery".to_string(),
            "Coordinate release recovery audit receipts".to_string(),
        );
        task.status = TaskStatus::InProgress;
        open_task_store_local(&cas_dir).unwrap().add(&task).unwrap();
        let agent_store = open_agent_store(&cas_dir).unwrap();
        agent_store
            .register(&Agent::new_with_role(
                "parity-session".to_string(),
                "parity-supervisor".to_string(),
                AgentRole::Supervisor,
            ))
            .unwrap();
        agent_store
            .try_claim(
                "cas-launch-recovery",
                "parity-session",
                600,
                Some("supervisor launch parity fixture"),
            )
            .unwrap();
        let mut sentinel = Entry::new(
            "grok-supervisor-ambient".to_string(),
            "Current release recovery audit receipts preserve operator intent".to_string(),
        );
        sentinel.title = Some("Current supervisor release recovery guidance".to_string());
        sentinel.tags = vec!["release".to_string(), "recovery".to_string()];
        sentinel.importance = 0.95;
        open_store(&cas_dir).unwrap().add(&sentinel).unwrap();

        // Keep this inventory adjacent to the exhaustive declaration above.
        // The declaration match is the compile fence; this loop makes every
        // registered supervisor launch policy observable in one focused test.
        for harness in [
            SupervisorCli::Claude,
            SupervisorCli::Codex,
            SupervisorCli::Grok,
            SupervisorCli::OpenCode,
        ] {
            let requirement = supervisor_harness_requirement(harness);
            let skill_payload = supervisor_skill_payload(&cas_dir, harness);
            for skill in requirement.supervisor_skills {
                assert!(
                    skill_payload.contains(skill),
                    "{harness:?} supervisor launch payload is missing required skill {skill:?}: {skill_payload}"
                );
            }

            match requirement.ambient_recall {
                AmbientRecallDelivery::SessionStart => {
                    let payload = claude_session_start_payload(&cas_dir);
                    assert!(
                        payload.contains("[ambient recall v1 role=supervisor"),
                        "{harness:?} SessionStart payload is missing ambient recall: {payload}"
                    );
                    assert!(
                        payload.contains("grok-supervisor-ambient"),
                        "{harness:?} SessionStart payload omitted the ambient recall sentinel: {payload}"
                    );
                }
                AmbientRecallDelivery::QueuedSupervisorIntro => {
                    assert!(
                        skill_payload.contains("[ambient recall v1 role=supervisor"),
                        "{harness:?} queued supervisor intro is missing ambient recall: {skill_payload}"
                    );
                    assert!(
                        skill_payload.contains("grok-supervisor-ambient"),
                        "{harness:?} queued supervisor intro omitted the ambient recall sentinel: {skill_payload}"
                    );
                }
            }
        }
    }

    /// AC-1/AC-2/AC-3: every one of the eight cells passes every stage against the
    /// real launch configuration.
    #[test]
    fn parity_matrix_all_cells_pass_all_stages() {
        let report = run_parity_matrix();
        assert_eq!(
            report.cells.len(),
            8,
            "matrix must be 4 harnesses × 2 roles"
        );
        assert!(
            report.all_passed,
            "some cell failed: {:#?}",
            report
                .cells
                .iter()
                .filter(|c| !c.passed)
                .collect::<Vec<_>>()
        );
        // Each cell proves all five named stages.
        for cell in &report.cells {
            let names: Vec<&str> = cell.stages.iter().map(|s| s.stage).collect();
            assert_eq!(
                names,
                vec![
                    "catalog",
                    "instruction_source",
                    "launch_args",
                    "effective_contract",
                    "prefix"
                ],
                "{}/{} must report all five stages in order",
                cell.harness,
                cell.role
            );
            assert!(
                cell.stages.iter().all(|s| s.passed()),
                "{}/{} has a failing stage: {:?}",
                cell.harness,
                cell.role,
                cell.stages
            );
        }
    }

    /// AC-3: each cell uses its own prefix and rejects cross-harness prefixes.
    #[test]
    fn each_cell_uses_own_prefix_and_rejects_foreign() {
        for cell in run_parity_matrix().cells {
            let prefix = cell.stages.iter().find(|s| s.stage == "prefix").unwrap();
            assert!(
                prefix.passed(),
                "{}/{} prefix stage failed: {}",
                cell.harness,
                cell.role,
                prefix.detail
            );
        }
    }

    /// AC-4: a missing role clause produces a stage-specific effective_contract
    /// FAIL — never UNKNOWN-as-PASS. Drive the real detector with a stripped
    /// surface to prove the gate actually fails closed.
    #[test]
    fn missing_role_clause_fails_effective_contract_stage_not_unknown() {
        // A surface with the coordinate-only clause but no urgent-stop recovery.
        let broken = "You are the Cassy Factory Supervisor. Coordinate only. Assign tasks. \
             Worker messages arrive asynchronously as injected turns (triage trigger). \
             MERGE REQUIRED: merge factory/<worker> then re-close. \
             Uses mcp__cas__task and mcp__cas__coordination.";
        let missing = cas_mux::missing_contract_elements(broken, ContractRole::Supervisor);
        assert!(
            missing.contains(&"urgent-stop-recovery"),
            "detector must flag the missing clause as a specific failure, got {missing:?}"
        );
    }

    /// AC-2 faithfulness: the Claude launch surface the runner captures matches
    /// what the REAL app enqueue path queues at launch — i.e. the intro prompt is
    /// actually wired, not just derivable. Drives the production
    /// queue_*_intro_prompt into a disposable Cassy root and asserts the queued
    /// prompt carries the full contract with no missing clauses.
    #[test]
    fn claude_launch_intro_is_actually_wired() {
        use crate::store::detect::open_prompt_queue_store;
        use crate::ui::factory::{queue_codex_worker_intro_prompt, queue_supervisor_intro_prompt};

        let tmp = tempfile::tempdir().unwrap();
        let cas_dir = tmp.path();
        queue_supervisor_intro_prompt(
            cas_dir,
            "sup",
            SupervisorCli::Claude,
            &["worker-a".to_string()],
            None,
            None,
        );
        queue_codex_worker_intro_prompt(cas_dir, "worker-a", SupervisorCli::Claude);
        let queue = open_prompt_queue_store(cas_dir).unwrap();

        for (target, role) in [
            ("sup", ContractRole::Supervisor),
            ("worker-a", ContractRole::Worker),
        ] {
            let rows = queue.peek_for_targets(&[target], None, 10).unwrap();
            assert_eq!(rows.len(), 1, "{target} must have a queued launch intro");
            assert!(
                cas_mux::missing_contract_elements(&rows[0].prompt, role).is_empty(),
                "queued Claude {role:?} intro missing clauses: {:?}",
                cas_mux::missing_contract_elements(&rows[0].prompt, role)
            );
        }
    }

    /// AC-5: report is machine-readable and names each stage.
    #[test]
    fn report_is_machine_readable_and_names_stages() {
        let report = run_parity_matrix();
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"catalog\""));
        assert!(json.contains("\"instruction_source\""));
        assert!(json.contains("\"launch_args\""));
        assert!(json.contains("\"effective_contract\""));
        assert!(json.contains("composes_with"));
    }
}
