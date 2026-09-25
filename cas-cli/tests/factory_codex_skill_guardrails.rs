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
    // cas-5b4f, audit D1: a Codex worker cannot call `mcp__cas__` tools. The
    // one recovery guide every harness installs names tools by bare name and
    // spells no prefix literal; the role guidance states each prefix once.
    let root = source_root();
    for flavor in ["", "codex/", "grok/"] {
        let recovery = load(&root.join(format!(
            "cas-cli/src/builtins/{flavor}skills/cas-worker/references/recovery.md"
        )));
        assert!(recovery.contains("coordination action=message target=supervisor"));
        let offenders = cas::builtins::unsanctioned_prefixed_tool_lines(recovery);
        assert!(
            offenders.is_empty(),
            "{flavor} worker recovery.md spells a harness prefix outside the naming rule: {offenders:?}"
        );
    }
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
        content.contains(cas::builtins::TOOL_NAMING_LINE),
        "codex supervisor guide should state the per-harness tool prefix"
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
            "remind_delay_secs",
            "remind_event=task_completed",
            "remind_ttl_secs",
            "remind_cancel",
            "MERGE REQUIRED",
        ] {
            assert!(
                content.contains(required),
                "{label} reminder reference missing {required:?}"
            );
        }
    }

    assert_eq!(claude, codex);
    assert_eq!(claude, grok);

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

/// cas-2c61/cas-62ab, audit D1: no Codex catalog entry hardcodes Claude's
/// `mcp__cas__` alias as an instruction. Tool names are bare; a prefix is
/// spelled only in the naming line and generated agent `tools:` frontmatter.
#[test]
fn codex_builtin_skills_and_agents_never_hardcode_claude_alias() {
    for builtin in builtin_catalog::skills(builtin_catalog::Flavor::Codex)
        .iter()
        .chain(builtin_catalog::agents(builtin_catalog::Flavor::Codex))
    {
        if !matches!(builtin.path.rsplit('.').next(), Some("md" | "yaml")) {
            continue;
        }
        let offenders = cas::builtins::unsanctioned_prefixed_tool_lines(builtin.content);
        assert!(
            offenders.is_empty(),
            "codex {} spells a harness prefix outside the naming rule: {offenders:?}",
            builtin.path
        );
    }
}

#[test]
fn codex_worker_runtime_instruction_allows_close_then_escalate() {
    // cas-8563b: the Codex worker contract is rendered by the shared
    // `worker_contract` renderer with the `mcp__cs__` prefix, so check the
    // rendered launch surface rather than a literal in the source file.
    let content = cas_mux::rendered_contract_surface("codex", cas_mux::ContractRole::Worker);

    // The rendered call shape is the contract; surrounding prose may change.
    assert!(
        content.contains("`mcp__cs__task action=close"),
        "runtime worker instruction should instruct workers to close tasks"
    );
}

#[test]
fn worker_failure_recovery_reference_and_merge_check_are_linked() {
    let root = source_root();
    for flavor in ["", "codex/", "grok/"] {
        let base = root.join(format!("cas-cli/src/builtins/{flavor}skills/cas-worker"));
        let worker = load(&base.with_extension("md"));
        let close_gate = load(&base.join("references/close-gate.md"));

        assert!(worker.contains("references/recovery.md"));
        // WP2 (audit cas-1660 M51): the cas-src surface checklist moved from
        // the always-loaded body into the on-demand close-gate reference.
        for marker in ["git merge-base --is-ancestor <delivered-tip> <target-tip>"] {
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
    // Audit D1: every flavor names tools by bare name.
    let flavors = [("", ""), ("codex/", ""), ("grok/", "")];

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

        for action in [
            "task action=start",
            "task action=close",
            "task action=notes",
        ] {
            assert!(details.contains(action), "{flavor} details lacks {action}");
        }
        assert_eq!(
            reference.matches("## Supervisor override").count(),
            1,
            "{flavor} reference.md must document supervisor_override once"
        );
        assert!(
            supervisor.contains("](references/reference.md#supervisor-override)"),
            "{flavor} supervisor guide must link supervisor_override reference"
        );
        assert!(
            checklist.contains("](../cas-supervisor/references/reference.md#supervisor-override)"),
            "{flavor} checklist must link supervisor_override reference"
        );
        assert!(
            workflow.contains(&format!("{tool_prefix}factory action=worktree_merge")),
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

    // Audit D6 (cas-6b97): Codex ignores `.md` agents, so the inert
    // factory-supervisor agent is gone and its constraints live in the
    // Codex supervisor checklist.
    assert!(
        !builtins.contains("builtins/codex/agents/"),
        "builtins.rs must not register Codex .md agents"
    );
    let checklist =
        load(&root.join("cas-cli/src/builtins/codex/skills/cas-codex-supervisor-checklist.md"));
    for required in [
        "## Codex constraints",
        "no session hooks",
        "never implement a worker's task yourself",
        "`cli=`, `model=`, and `effort=`",
        "cas-supervisor/references/workflow.md",
    ] {
        assert!(
            checklist.contains(required),
            "cas-codex-supervisor-checklist missing {required:?}"
        );
    }
}
