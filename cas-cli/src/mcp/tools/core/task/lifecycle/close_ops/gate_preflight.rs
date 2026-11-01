//! Gate preflight: what close will check, listed at task start, and every park
//! refusal in one reply. Most close bounces were computable before close, and a
//! gate that stops at its first failure costs one attempt per failure.
use super::*;
use crate::config::{QaMode, RegistryEnforcement, UpkeepStage};

/// One reply for every refusal of a park attempt. A lone refusal keeps its own
/// text, which workers and the classifiers that count bounces already read.
pub(super) fn park_reply(mut refusals: Vec<String>) -> String {
    refusals.dedup();
    if refusals.len() == 1 {
        return refusals.remove(0);
    }
    format!(
        "PARK REFUSED: more than one close gate failed, and every failure is listed below. Fix them all, then retry close.\n\n{}",
        refusals.join("\n\n")
    )
}

pub(super) fn is_merge_handoff(refusal: &str) -> bool {
    refusal.trim_start_matches("⚠️").trim_start().starts_with("MERGE REQUIRED")
}

impl CasCore {
    /// The close requirements knowable at task start, for the start context.
    /// Each line is conditional on the same inputs its gate reads, so a project
    /// or task the gate would skip does not list it.
    pub(crate) fn close_requirements(&self, task: &Task) -> String {
        if std::env::var_os("CAS_FACTORY_MODE").is_none()
            || matches!(task.task_type, TaskType::Epic | TaskType::Gate)
            || task.execution_note.as_deref() == Some("no-code")
        {
            return String::new();
        }
        let Some(assignee) = task.assignee.as_deref() else {
            return String::new();
        };
        let Some(repo) = self
            .resolve_worker_worktree_path(task, None)
            .ok()
            .flatten()
            .or_else(|| resolve_close_gate_repo_root(&self.cas_root).ok())
        else {
            return String::new();
        };
        let Ok(config) = crate::config::Config::load(&self.cas_root) else {
            return String::new();
        };
        let qa = config.qa();
        let upkeep = config.upkeep();
        let parent_epic = self
            .open_task_store()
            .ok()
            .and_then(|store| store.get_parent_epic(&task.id).ok().flatten());
        let id = &task.id;
        let mut lines = Vec::new();

        let branch = close_measured_factory_branch(&repo, task, assignee);
        let target = effective_close_work_target(task, parent_epic.as_ref())
            .map(|target| target.target_branch)
            .or_else(|| task.branch.clone());
        let base = task.deliverables.factory_branch_start.as_ref().map(|start| start.base_sha.as_str());
        lines.push(format!(
            "Branch: {branch}{}, pushed at the delivered tip.{}",
            target.map(|target| format!(" to {target}")).unwrap_or_default(),
            base.map(|base| format!(" Scoped proof base {}.", &base[..base.len().min(12)])).unwrap_or_default(),
        ));
        lines.push(
            "Close reason: cite a run id, artifact id or receipt path beside a third-party or future proof claim, or write \"deferred to <stage>\"."
                .to_string(),
        );
        if receipt_scope::has_unreleased_section_at_head(&repo, "HEAD") {
            lines.push(
                "Changelog: product or developer source needs a titled `## [Unreleased]` entry, or a decision note \"no changelog: <reason>\"."
                    .to_string(),
            );
        }
        if !receipt_scope::workspace_package_dirs(&repo).is_empty() {
            lines.push("Focused proof: each changed Rust crate needs a passing capped test receipt at the tip.".to_string());
        }
        if qa.evidence_gate {
            lines.push(format!(
                "Tier 2 (guard, hook, CI, release or close-gate paths): `cas qa ledger init --task {id}`, then a row starting `Adversarial probe:`."
            ));
            if !task.demo_statement.trim().is_empty() && qa.mode != QaMode::Batch {
                lines.push(format!(
                    "Ledger: the demo needs at least {} rows passing `cas qa ledger check`.",
                    crate::qa_ledger::minimum_rows(task)
                ));
            }
        }
        let registry_on = qa.registry.enforce != RegistryEnforcement::Off
            || crate::qa_impact::io::git(&repo, &["cat-file", "-e", &format!("HEAD:{}", qa.registry.path)]).is_ok();
        if registry_on && task.task_type == TaskType::Bug {
            lines.push(format!(
                "Bug case: a live journey this fix touches needs a bug family naming {id}, or the refusal's `no case:` decision."
            ));
        }
        let tracked: Vec<&str> = [
            registry_on.then_some(qa.registry.path.as_str()),
            (upkeep.codemap() != UpkeepStage::Off).then_some(".claude/CODEMAP.md"),
            (upkeep.product_overview() != UpkeepStage::Off).then_some("docs/PRODUCT_OVERVIEW.md"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !tracked.is_empty() {
            lines.push(format!("Upkeep: keep {} true for changed paths.", tracked.join(", ")));
        }
        if !task.proof_targets.is_empty() || task.risk.contains(&TaskRisk::BlastRadius) {
            lines.push(format!(
                "Proof targets: {}{}.",
                if task.proof_targets.is_empty() { "none".to_string() } else { task.proof_targets.join(", ") },
                if task.risk.contains(&TaskRisk::BlastRadius) {
                    "; these or test receipts must cover every delivered source module"
                } else {
                    ""
                },
            ));
        }
        for (risk, proof) in [(TaskRisk::Concurrency, "loaded_proof"), (TaskRisk::Platform, "platform_proof")] {
            if task.risk.contains(&risk) {
                lines.push(format!("Risk: supervisor assembly owns the {proof}."));
            }
        }
        match task.execution_note.as_deref() {
            Some("additive-only") => lines.push("Method: additive-only refuses modified, deleted or renamed files.".to_string()),
            Some("value-only") => lines.push("Method: value-only refuses added, deleted, copied or renamed files.".to_string()),
            _ => {}
        }
        format!("\n\n📋 Close checks (park reports every failure at once):\n- {}", lines.join("\n- "))
    }
}
