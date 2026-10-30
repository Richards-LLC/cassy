//! How much independent review a delivery needs, from the paths it changes.
//!
//! Tier 0 changes nothing that runs: CI and the close's mechanical gates are its
//! review. Tier 1 gets one verifier. Tier 2 changes what enforces the rules
//! (guards, hooks, CI, release and close-gate code), so its verifier also probes
//! adversarially. The highest tier any path reaches applies to the delivery.
//! Anything unmeasurable stays at today's path, one verifier.
use super::*;

/// A supervisor's label that keeps a verifier on an otherwise Tier 0 delivery.
pub(super) const FORCE_VERIFIER_LABEL: &str = "force-verifier";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum VerificationTier {
    Mechanical,
    Standard,
    Adversarial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TierDecision {
    pub(super) tier: VerificationTier,
    pub(super) reason: String,
    pub(super) tip: String,
}

impl TierDecision {
    /// One note per delivered tip, so a retried close does not repeat it.
    pub(super) fn note(&self) -> String {
        let (number, review) = match self.tier {
            VerificationTier::Mechanical => (0, "no independent verifier; CI and mechanical gates only"),
            VerificationTier::Standard => (1, "one independent verifier"),
            VerificationTier::Adversarial => {
                (2, "independent verifier plus an adversarial probe of the changed enforcement")
            }
        };
        format!("🧭 VERIFICATION TIER {number} at {}: {review}. {}", self.tip, self.reason)
    }
}

/// Text that decides what agents do, even where it is only Markdown.
fn is_agent_instruction(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(name, "AGENTS.md" | "CLAUDE.md" | "GROK.md")
        || path.split('/').any(|part| {
            matches!(part, "builtins" | "skills" | "agents" | ".claude" | ".codex" | ".grok" | ".opencode" | ".agents")
        })
}

fn changes_nothing_that_runs(path: &str) -> bool {
    if is_agent_instruction(path) {
        return false;
    }
    let mut directories = path.split('/').collect::<Vec<_>>();
    directories.pop();
    path.starts_with("docs/")
        || path.ends_with(".md")
        || path.starts_with("hub-web/e2e/")
        || directories.contains(&"tests")
}

fn enforces_the_rules(path: &str) -> bool {
    path.starts_with(".github/workflows/")
        || path.starts_with("scripts/release")
        || path.contains("merge-queue")
        || path.contains("merge_queue")
        || path.starts_with("cas-cli/src/hooks/")
        || path.starts_with("cas-cli/src/mcp/tools/core/task/lifecycle/close_ops")
        || path.ends_with("qa_evidence_gate.rs")
        || path.ends_with("verification_tools.rs")
        || path.ends_with("worktree_ops.rs")
}

/// The tier a set of changed paths reaches, or `None` when there are no paths.
pub(super) fn tier_for_paths(paths: &[String]) -> Option<(VerificationTier, String)> {
    if paths.is_empty() {
        return None;
    }
    if let Some(path) = paths.iter().find(|path| enforces_the_rules(path)) {
        return Some((VerificationTier::Adversarial, format!("{path} is guard, CI, release or close-gate code.")));
    }
    if let Some(path) = paths.iter().find(|path| !changes_nothing_that_runs(path)) {
        return Some((VerificationTier::Standard, format!("{path} can change behavior.")));
    }
    Some((VerificationTier::Mechanical, format!("All {} changed paths are docs or tests.", paths.len())))
}

/// The delivered tier, measured from the task's recorded start to its anchor.
/// Commits merged in from the target can only raise the tier, never lower it.
pub(super) fn tier_for_delivery(task: &Task, repo: &std::path::Path) -> Option<TierDecision> {
    if task.task_type == TaskType::Epic {
        return None;
    }
    let tip = task.deliverables.factory_branch_anchor.as_deref()?;
    let start = task.deliverables.factory_branch_start.as_ref()?.base_sha.as_str();
    let paths = git_stdout_lines(repo, &["diff", "--name-only", "--no-renames", start, tip]).ok()?;
    let (mut tier, mut reason) = tier_for_paths(&paths)?;
    if tier == VerificationTier::Mechanical && task.labels.iter().any(|label| label == FORCE_VERIFIER_LABEL) {
        tier = VerificationTier::Standard;
        reason = format!("A supervisor set the {FORCE_VERIFIER_LABEL} label.");
    }
    Some(TierDecision { tier, reason, tip: tip.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier(paths: &[&str]) -> VerificationTier {
        let paths: Vec<String> = paths.iter().map(|path| path.to_string()).collect();
        tier_for_paths(&paths).unwrap().0
    }

    #[test]
    fn docs_and_tests_need_no_independent_verifier() {
        assert_eq!(
            tier(&[
                "CHANGELOG.md",
                "docs/qa/bug-families/x.yaml",
                "docs/release-notes/x-slack.md",
                "hub-web/e2e/journeys/x.journey.ts",
                "crates/cas-mux/tests/x.rs",
            ]),
            VerificationTier::Mechanical
        );
    }

    /// Markdown that instructs agents and ordinary source both get a verifier.
    #[test]
    fn product_source_and_agent_instructions_get_one_verifier() {
        for path in ["cas-cli/src/lib.rs", "cas-cli/src/builtins/skills/cas-worker.md", "AGENTS.md", ".claude/agents/x.md"] {
            assert_eq!(tier(&[path]), VerificationTier::Standard, "{path}");
        }
    }

    /// A test or doc path inside enforcement code does not lower its tier.
    #[test]
    fn enforcement_code_gets_the_adversarial_probe() {
        for path in [
            "cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs",
            ".github/workflows/ci.yml",
            "scripts/release-gate.sh",
            "cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/verification_tier.rs",
            "cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/merge_queue_proof.rs",
        ] {
            assert_eq!(tier(&[path]), VerificationTier::Adversarial, "{path}");
        }
    }

    #[test]
    fn a_mixed_diff_takes_its_highest_tier() {
        assert_eq!(tier(&["CHANGELOG.md", "cas-cli/src/lib.rs"]), VerificationTier::Standard);
        assert_eq!(
            tier(&["CHANGELOG.md", "cas-cli/src/lib.rs", ".github/workflows/ci.yml"]),
            VerificationTier::Adversarial
        );
    }

    /// Through a real diff: an unmeasurable delivery keeps today's verifier, and
    /// the supervisor's label overrides Tier 0.
    #[test]
    fn delivery_is_measured_from_its_recorded_start() {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").arg("-C").arg(repo.path()).args(args).output().unwrap();
            assert!(output.status.success(), "{args:?}");
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "-q"]);
        let commit = |path: &str| {
            let file = repo.path().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, path).unwrap();
            git(&["add", "-A"]);
            git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", path]);
            git(&["rev-parse", "HEAD"])
        };
        let start = commit("src/lib.rs");
        let tip = commit("docs/guide.md");

        let mut task = Task::new("cas-tier".to_string(), "tier".to_string());
        assert_eq!(tier_for_delivery(&task, repo.path()), None, "no anchor stays on today's path");
        task.deliverables.factory_branch_anchor = Some(tip.clone());
        task.deliverables.factory_branch_start =
            Some(cas_types::FactoryBranchStart {
                work_target: cas_types::WorkTarget { repo_selector: "project:test".into(), target_branch: "main".into() },
                assignee: "worker".into(),
                branch: "factory/worker".into(),
                base_sha: start,
            });
        let measured = tier_for_delivery(&task, repo.path()).unwrap();
        assert_eq!(measured.tier, VerificationTier::Mechanical);
        assert!(measured.note().contains(&tip), "{}", measured.note());

        task.labels.push(FORCE_VERIFIER_LABEL.to_string());
        assert_eq!(tier_for_delivery(&task, repo.path()).unwrap().tier, VerificationTier::Standard);
    }
}
