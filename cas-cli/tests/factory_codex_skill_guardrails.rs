use std::fs;
use std::path::{Path, PathBuf};

#[path = "support/builtin_catalog.rs"]
mod builtin_catalog;

fn load(path: &Path) -> &'static str {
    let path = path.to_string_lossy();
    let relative = path
        .split_once("cas-cli/src/builtins/")
        .map(|(_, relative)| format!("cas-cli/src/builtins/{relative}"))
        .unwrap_or_else(|| panic!("not an embedded builtin source path: {path}"));
    builtin_catalog::find_source_path(&relative)
}

/// Build source-shaped paths for the static loader without resolving them
/// against the checkout. The path is only a catalog lookup key.
fn source_root() -> PathBuf {
    PathBuf::new()
}

#[test]
fn codex_factory_skills_use_cs_prefix_only() {
    let root = cas::test_paths::workspace_root();
    let skills_dir = root.join(".codex/skills");
    if !skills_dir.exists() {
        eprintln!(
            "SKIP Codex projection check: source checkout has no .codex/skills at {}",
            skills_dir.display()
        );
        return;
    }

    let entries = fs::read_dir(&skills_dir).expect("read .codex/skills");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("cas-factory-") {
            continue;
        }
        let skill_path = entry.path().join("SKILL.md");
        if !skill_path.exists() {
            continue;
        }

        let content = fs::read_to_string(&skill_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", skill_path.display()));
        let has_mcp_examples = content.contains("mcp__cs__") || content.contains("mcp__cas__");
        if has_mcp_examples {
            assert!(
                content.contains("mcp__cs__"),
                "{} should include mcp__cs__ examples",
                skill_path.display()
            );
        }
        assert!(
            !content.contains("mcp__cas__"),
            "{} still contains legacy mcp__cas__ references",
            skill_path.display()
        );
        assert!(
            !content.contains("action=prompt"),
            "{} still contains legacy action=prompt usage",
            skill_path.display()
        );
    }
}

#[test]
fn codex_worker_recovery_uses_cs_alias_not_cas(/* cas-5b4f */) {
    // The Codex worker recovery guide is rendered only into Codex worker
    // sessions, where every CAS tool surfaces under the `mcp__cs__` alias.
    // It must never hardcode `mcp__cas__` coordination/task instructions — those
    // are unreachable for a Codex worker. Mirrors the file-level
    // `codex_factory_skills_use_cs_prefix_only` convention.
    let root = source_root();
    let codex_recovery =
        load(&root.join("cas-cli/src/builtins/codex/skills/cas-worker/references/recovery.md"));

    assert!(
        codex_recovery.contains("mcp__cs__coordination"),
        "codex worker recovery.md should give executable mcp__cs__coordination guidance"
    );
    assert!(
        !codex_recovery.contains("mcp__cas__"),
        "codex worker recovery.md must not hardcode the Claude `mcp__cas__` alias \
         (Codex workers only have mcp__cs__ tools)"
    );

    // AC: the Claude worker recovery doc must stay correct for the Claude alias.
    let claude_recovery =
        load(&root.join("cas-cli/src/builtins/skills/cas-worker/references/recovery.md"));
    assert!(
        claude_recovery.contains("mcp__cas__coordination"),
        "claude worker recovery.md should retain mcp__cas__coordination guidance"
    );
}

#[test]
fn codex_builtin_supervisor_guide_includes_core_workflow() {
    let root = source_root();
    let guide = root.join("cas-cli/src/builtins/codex/skills/cas-supervisor.md");
    let content = load(&guide);

    assert!(
        content.contains("spawn_workers"),
        "supervisor guide should include spawn_workers"
    );
    assert!(
        content.contains("Never implement tasks yourself"),
        "supervisor guide should include hard rule about not implementing"
    );
    assert!(
        content.contains("mcp__cs__"),
        "codex supervisor guide should use mcp__cs__ prefix"
    );
}

/// cas-314d: supervisors and workers share one bounded, push-first reminder
/// contract. The on-demand reference avoids inflating the SessionStart skill
/// bodies, but all three installed flavors must retain identical semantics.
#[test]
fn reminder_discipline_reference_is_complete_and_flavor_normalized() {
    let root = source_root();
    let claude =
        load(&root.join("cas-cli/src/builtins/skills/cas-supervisor/references/reminders.md"));
    let codex = load(
        &root.join("cas-cli/src/builtins/codex/skills/cas-supervisor/references/reminders.md"),
    );
    let grok =
        load(&root.join("cas-cli/src/builtins/grok/skills/cas-supervisor/references/reminders.md"));

    for (label, content) in [("claude", &claude), ("codex", &codex), ("grok", &grok)] {
        for required in [
            "## Decision table",
            "Push First, One Bounded Checkpoint",
            "exactly **one** trigger",
            "remind_delay_secs",
            "remind_event=task_completed",
            "remind_ttl_secs",
            "remind_cancel",
            "One active reminder per task/phase",
            "authoritative task/worker state",
            "MERGE REQUIRED",
            "Context-pressure handoff",
            "Blocked worker recovery",
            "detached command",
        ] {
            assert!(
                content.contains(required),
                "{label} reminder reference missing {required:?}"
            );
        }
    }

    assert_eq!(claude.replace("mcp__cas__", "mcp__cs__"), codex);
    assert_eq!(claude.replace("mcp__cas__", "cas__"), grok);

    for path in [
        "cas-cli/src/builtins/skills/cas-supervisor.md",
        "cas-cli/src/builtins/codex/skills/cas-supervisor.md",
        "cas-cli/src/builtins/grok/skills/cas-supervisor.md",
        "cas-cli/src/builtins/skills/cas-worker.md",
        "cas-cli/src/builtins/codex/skills/cas-worker.md",
        "cas-cli/src/builtins/grok/skills/cas-worker.md",
    ] {
        assert!(
            load(&root.join(path)).contains("reminders.md"),
            "{path} must point to the shared reminder discipline reference"
        );
    }
}

#[test]
fn supervisor_epic_driving_reference_is_compact_and_three_way_mirrored() {
    let root = source_root();
    let paths = [
        root.join("cas-cli/src/builtins/skills/cas-supervisor/references/epic-driving.md"),
        root.join("cas-cli/src/builtins/codex/skills/cas-supervisor/references/epic-driving.md"),
        root.join("cas-cli/src/builtins/grok/skills/cas-supervisor/references/epic-driving.md"),
    ];
    let contents: Vec<&'static str> = paths.iter().map(|path| load(path)).collect();

    for (path, content) in paths.iter().zip(&contents) {
        assert!(
            content.len() < 2 * 1024,
            "{} exceeds the 2KB operator budget ({} bytes)",
            path.display(),
            content.len()
        );
        for required in [
            "target_branch",
            "WorkTarget",
            "awaiting_merge",
            "task_id",
            "confirm_warning=true",
            "proof_scope_fix=true",
            "known-repos",
            "CHANGELOG",
            "release-notes draft",
            "integration PR",
            "one tree, one queue cycle",
            "release/vX-prepare",
            "Release Prebuild",
        ] {
            assert!(
                content.contains(required),
                "{} missing epic-driving marker {required:?}",
                path.display()
            );
        }
    }

    assert_eq!(contents[0], contents[1]);
    assert_eq!(contents[0], contents[2]);

    for body_path in [
        "cas-cli/src/builtins/skills/cas-supervisor.md",
        "cas-cli/src/builtins/codex/skills/cas-supervisor.md",
        "cas-cli/src/builtins/grok/skills/cas-supervisor.md",
    ] {
        assert!(
            load(&root.join(body_path)).contains("epic-driving.md"),
            "{body_path} must breadcrumb the epic-driving reference"
        );
    }
}

#[test]
fn supervisor_skill_mirrors_include_implementation_unit_template() {
    // After cas-61af split cas-supervisor.md into a main file + references,
    // the Implementation Unit Template moved to planning.md. The guardrail
    // checks that file instead (both .claude and .codex trees must match).
    let root = source_root();
    let claude =
        load(&root.join("cas-cli/src/builtins/skills/cas-supervisor/references/planning.md"));
    let codex =
        load(&root.join("cas-cli/src/builtins/codex/skills/cas-supervisor/references/planning.md"));

    for (label, content) in [("claude", &claude), ("codex", &codex)] {
        assert!(
            content.contains("## Implementation Unit Template"),
            "{label} planning.md missing '## Implementation Unit Template' heading"
        );
        // Canonical template markers (R1)
        for marker in [
            "**Unit N: [Name]**",
            "**Goal:**",
            "**Requirements:**",
            "**Dependencies:**",
            "**Files:**",
            "**Approach:**",
            "**Execution note:**",
            "**Patterns to follow:**",
            "**Test scenarios:**",
            "**Verification:**",
        ] {
            assert!(
                content.contains(marker),
                "{label} planning.md template missing marker: {marker}"
            );
        }
        // R4 mapping table
        assert!(
            content.contains("| Template field | Maps to |"),
            "{label} planning.md missing template→task schema mapping table"
        );
        // R6/R7 scope note
        assert!(
            content.contains("EPIC subtasks"),
            "{label} planning.md missing EPIC-subtasks-only scope note"
        );
        // R13 cross-link: Spec Requirements section mentions the template
        let spec_idx = content
            .find("## Spec Requirements")
            .unwrap_or_else(|| panic!("{label} missing Spec Requirements heading"));
        let tmpl_idx = content
            .find("## Implementation Unit Template")
            .unwrap_or_else(|| panic!("{label} missing Implementation Unit Template heading"));
        let spec_block = &content[spec_idx..tmpl_idx];
        assert!(
            spec_block.contains("Implementation Unit Template"),
            "{label} Spec Requirements section missing cross-link to Implementation Unit Template"
        );
    }
}

/// cas-2c61/cas-62ab: the ~25-file mcp__cas__ sweep beyond recovery.md
/// (cas-5b4f fixed only that one file; this closes the rest of the list
/// cas-62ab named — workflow.md, details.md, worker-recovery.md,
/// reference.md, cas-worker.md, session-learn/SKILL.md,
/// cas-memory-management/SKILL.md, and the remaining swept files). Every
/// codex builtin skill/agent source must use mcp__cs__ for executable
/// tool instructions, never Claude's mcp__cas__ — extends the
/// `codex_worker_recovery_uses_cs_alias_not_cas` convention corpus-wide.
#[test]
fn codex_builtin_skills_and_agents_never_hardcode_claude_alias() {
    for builtin in builtin_catalog::skills(builtin_catalog::Flavor::Codex)
        .iter()
        .chain(builtin_catalog::agents(builtin_catalog::Flavor::Codex))
    {
        if !matches!(builtin.path.rsplit('.').next(), Some("md" | "yaml")) {
            continue;
        }
        assert!(
            !builtin.content.contains("mcp__cas__"),
            "codex {} still hardcodes the Claude mcp__cas__ alias — Codex entries only \
             surface CAS tools under mcp__cs__ (cas-2c61/cas-62ab)",
            builtin.path
        );
    }
}

#[test]
fn codex_worker_runtime_instruction_allows_close_then_escalate() {
    let content = include_str!("../../crates/cas-pty/src/pty.rs");

    // cas-47b7: the worker instruction phrasing is "close it with
    // `mcp__cs__task action=close ...`" (cas-bbc2 single-task rewrite). Assert on
    // the close-command form itself rather than a fragile leading verb so prose
    // tweaks don't re-break this guardrail; the intent is only "workers ARE told
    // to close their task".
    assert!(
        content.contains("`mcp__cs__task action=close"),
        "runtime worker instruction should instruct workers to close tasks"
    );
    assert!(
        !content.contains("DO NOT close the task yourself"),
        "runtime worker instruction should not forbid close universally"
    );
}

#[test]
fn worker_failure_recovery_guidance_is_pinned_cas_62a9() {
    let root = source_root();
    for flavor in ["", "codex/", "grok/"] {
        let base = root.join(format!("cas-cli/src/builtins/{flavor}skills/cas-worker"));
        let worker = load(&base.with_extension("md"));
        let close_gate = load(&base.join("references/close-gate.md"));

        for marker in [
            "never retry the denied target",
            "A `/dev/null` denial is a guard defect to report",
            "every applicable entry must paste its proving file, command, or test",
            "Bare assertions are non-compliant",
        ] {
            assert!(
                worker.contains(marker),
                "{flavor} worker guidance missing {marker:?}"
            );
        }
        for marker in [
            "Crossed-message freshness handshake",
            "before any corrective commit",
            "git merge-base --is-ancestor <delivered-tip> <target-tip>",
            "re-close or stop; do not edit stale state",
        ] {
            assert!(
                close_gate.contains(marker),
                "{flavor} close gate missing {marker:?}"
            );
        }
    }
}

#[test]
fn supervisor_reference_tree_uses_current_lifecycle_contract() {
    let root = source_root();
    let valid_actions = "`create`, `proposal_inbox`, `proposal_accept`, `proposal_reject`, `proposal_reconcile`, `show`, `update`, `start`, `close`, `cancel`, `reopen`, `request_changes`, `delete`, `list`, `ready`, `blocked`, `notes`, `dep_add`, `dep_remove`, `dep_list`, `claim`, `release`, `reset`, `transfer`, `available`, `mine`";
    let flavors = [
        ("", "mcp__cas__"),
        ("codex/", "mcp__cs__"),
        ("grok/", "cas__"),
    ];

    for (flavor, tool_prefix) in flavors {
        let base = root.join(format!("cas-cli/src/builtins/{flavor}skills"));
        let supervisor = load(&base.join("cas-supervisor.md"));
        let checklist_name = if flavor == "codex/" {
            "cas-codex-supervisor-checklist.md"
        } else {
            "cas-supervisor-checklist.md"
        };
        let checklist = load(&base.join(checklist_name));
        let reference = load(&base.join("cas-supervisor/references/reference.md"));
        let workflow = load(&base.join("cas-supervisor/references/workflow.md"));
        let intake = load(&base.join("cas-supervisor/references/intake.md"));
        let planning = load(&base.join("cas-supervisor/references/planning.md"));
        let model_selection = load(&base.join("cas-supervisor/references/model-selection.md"));
        let close_gate = load(&base.join("cas-worker/references/close-gate.md"));
        let recovery = load(&base.join("cas-worker/references/recovery.md"));
        let details = load(&base.join("cas-worker/references/details.md"));
        let github = load(&base.join("cas-github-issues/SKILL.md"));

        for (label, content) in [
            ("supervisor", &supervisor),
            ("checklist", &checklist),
            ("reference", &reference),
            ("workflow", &workflow),
            ("close-gate", &close_gate),
            ("recovery", &recovery),
            ("details", &details),
            ("github issues", &github),
        ] {
            for retired in [
                "pending_supervisor_review",
                "bypass_code_review",
                "/epic-spec",
                "/epic-breakdown",
                "code-review-queue",
            ] {
                assert!(
                    !content.contains(retired),
                    "{flavor}{label} still teaches retired contract {retired:?}"
                );
            }
        }

        assert!(
            reference.contains(&format!(
                "**Valid `{tool_prefix}task` actions** (do not invent others): {valid_actions}."
            )),
            "{flavor} reference.md does not match the task dispatch action list"
        );
        assert!(
            details.contains(&format!(
                "**Valid `{tool_prefix}task` actions** (do not invent others): {valid_actions}."
            )),
            "{flavor} details.md does not match the task dispatch action list"
        );
        assert_eq!(
            reference.matches("## Supervisor override").count(),
            1,
            "{flavor} reference.md must document supervisor_override once"
        );
        for required in ["registered supervisor", "non-empty reason", "decision note"] {
            assert!(
                reference.contains(required),
                "{flavor} supervisor override documentation missing {required:?}"
            );
        }
        assert!(
            supervisor.contains("cas-supervisor/references/reference.md#supervisor-override"),
            "{flavor} supervisor guide must link supervisor_override reference"
        );
        assert!(
            checklist.contains("cas-supervisor/references/reference.md#supervisor-override"),
            "{flavor} checklist must link supervisor_override reference"
        );
        assert!(
            workflow.contains(&format!("{tool_prefix}coordination action=worktree_merge")),
            "{flavor} workflow must use worktree_merge"
        );
        assert!(
            !workflow.contains("git cherry-pick"),
            "{flavor} workflow must not teach the retired cherry-pick merge procedure"
        );
        assert!(
            !workflow.contains("git checkout <base-branch>"),
            "{flavor} workflow must not teach an untracked raw-git merge fallback"
        );
        assert!(
            !recovery.contains("UPDATE tasks SET"),
            "{flavor} worker-recovery must not teach direct SQL task mutation"
        );
        assert!(
            !intake.contains("AskUserQuestion"),
            "{flavor} intake must not restate the factory AskUserQuestion guard"
        );
        for content in [&reference, &workflow, &planning] {
            assert!(
                !content.to_ascii_lowercase().contains("awaiting review"),
                "{flavor} supervisor references must use awaiting_merge, not awaiting review"
            );
        }
        assert_eq!(
            model_selection.matches("suspended").count(),
            1,
            "{flavor} model-selection must keep one canonical suspension statement"
        );
        assert_eq!(
            model_selection.matches("## Spawn recipes").count(),
            1,
            "{flavor} model-selection must keep one recipe pointer"
        );
        assert!(
            !model_selection.contains("BEGIN GENERATED SPAWN RECIPES"),
            "{flavor} model-selection must not duplicate workflow recipes"
        );
    }

    let builtins = include_str!("../src/builtins.rs");
    assert!(
        !builtins.contains("code-review-queue"),
        "builtins.rs must not register deleted code-review-queue.md"
    );

    let factory_supervisor =
        load(&root.join("cas-cli/src/builtins/codex/agents/factory-supervisor.md"));
    assert!(
        factory_supervisor.lines().count() < 60,
        "Codex factory-supervisor.md exceeds the 60-line prompt budget"
    );
    for required in ["Codex Constraints", "cli=codex", "cas-supervisor"] {
        assert!(
            factory_supervisor.contains(required),
            "factory-supervisor.md missing {required:?}"
        );
    }
}
