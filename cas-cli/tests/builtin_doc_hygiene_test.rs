//! Contract tests for the doc-family hygiene wave (cas-ef87a).
//!
//! These read the checked-in builtin sources rather than only the embedded
//! catalog: an unregistered mirror is exactly the failure mode this wave is
//! cleaning up, so a catalog-only assertion would pass on a stale file.
//!
//! The three-flavor byte parity of these files is owned by
//! `builtin_flavor_drift_test.rs`; here we assert content and registration.

use std::path::PathBuf;

use cas::builtins::{
    BUILTIN_AGENTS, BUILTIN_SKILLS, CODEX_BUILTIN_AGENTS, CODEX_BUILTIN_SKILLS,
    GROK_BUILTIN_AGENTS, GROK_BUILTIN_SKILLS, REQUIRED_FACTORY_AGENTS,
};

#[path = "support/builtin_catalog.rs"]
mod builtin_catalog;

fn checkout_builtins_root() -> Option<PathBuf> {
    let root = cas::test_paths::workspace_root().join("cas-cli/src/builtins");
    if !root.is_dir() {
        eprintln!(
            "SKIP builtin source projection checks: source checkout is absent at {}",
            root.display()
        );
        return None;
    }
    Some(root)
}

/// Read a builtin source path relative to `cas-cli/src/builtins`.
fn load(relative: &str) -> &'static str {
    builtin_catalog::find_source_path(&format!("cas-cli/src/builtins/{relative}"))
}

/// The claude canonical plus both twins for one builtins-relative path.
fn all_flavors(relative: &str) -> [String; 3] {
    [
        relative.to_string(),
        format!("codex/{relative}"),
        format!("grok/{relative}"),
    ]
}

fn line_index(body: &str, needle: &str) -> usize {
    body.lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("expected to find {needle:?} in the document"))
}

#[test]
fn supervisor_guidance_drives_each_turn_to_a_named_exit_rung() {
    for flavor_rel in all_flavors("skills/cas-supervisor.md") {
        let body = load(&flavor_rel);
        for marker in [
            "Drive to the exit",
            "Children merged",
            "Epic assembled",
            "Integration gated",
            "PR queued",
            "On main",
            "Released and deployed",
        ] {
            assert!(
                body.contains(marker),
                "{flavor_rel} must carry supervisor forward-motion marker {marker:?}"
            );
        }
        assert!(
            !body.contains("produce no more output") && !body.contains("wait for events"),
            "{flavor_rel} must not tell the supervisor to stop before owning the next rung"
        );
    }
}

// ---------------------------------------------------------------------------
// 1. Doc family: shared hygiene reference, real signals only
// ---------------------------------------------------------------------------

const DOC_FAMILY: [&str; 3] = ["codemap", "project-overview", "design-spec"];

#[test]
fn doc_family_shares_one_hygiene_reference_instead_of_restating_it() {
    for skill in DOC_FAMILY {
        for flavor_rel in all_flavors(&format!("skills/{skill}/SKILL.md")) {
            let body = load(&flavor_rel);
            assert!(
                body.contains("doc-hygiene.md"),
                "{flavor_rel} must link the shared doc-hygiene reference"
            );
            assert!(
                !body.contains("Preserve any `<!-- keep -->`"),
                "{flavor_rel} still restates the keep-block procedure that moved to \
                 doc-hygiene.md"
            );
            assert!(
                !body.contains("No content duplication."),
                "{flavor_rel} still restates the pointer-memory procedure that moved to \
                 doc-hygiene.md"
            );
        }
    }
}

#[test]
fn doc_hygiene_reference_is_registered_in_every_flavor() {
    let rel = "skills/codemap/references/doc-hygiene.md";
    if let Some(root) = checkout_builtins_root() {
        for flavor_rel in all_flavors(rel) {
            assert!(
                root.join(&flavor_rel).is_file(),
                "{flavor_rel} must exist on disk"
            );
        }
    }
    for (name, catalog) in [
        ("BUILTIN_SKILLS", BUILTIN_SKILLS),
        ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
        ("GROK_BUILTIN_SKILLS", GROK_BUILTIN_SKILLS),
    ] {
        assert!(
            catalog.iter().any(|b| b.path == rel),
            "{name} must register {rel}; an unregistered reference is never installed"
        );
    }
}

#[test]
fn design_spec_drops_the_removed_review_persona_and_the_phantom_drift_signal() {
    for flavor_rel in all_flavors("skills/design-spec/SKILL.md") {
        let body = load(&flavor_rel);
        assert!(
            !body.to_ascii_lowercase().contains("persona"),
            "{flavor_rel} still names the review persona layer removed in v3.10.0"
        );
        assert!(
            !body.contains("staleness signal"),
            "{flavor_rel} still promises a DESIGN.md staleness signal; no hook or CLI \
             reads DESIGN.md"
        );
        assert!(
            body.contains("reviewers can diff"),
            "{flavor_rel} must state the real reason to commit DESIGN.md"
        );
    }
}

#[test]
fn project_overview_tells_the_agent_to_commit_the_doc() {
    for flavor_rel in all_flavors("skills/project-overview/SKILL.md") {
        let body = load(&flavor_rel);
        assert!(
            body.contains("git add docs/PRODUCT_OVERVIEW.md"),
            "{flavor_rel} must include the commit step; git history is the primary \
             freshness signal (project_overview.rs:525-527)"
        );
    }
}

#[test]
fn codemap_states_the_real_missing_codemap_gate_behaviour() {
    for flavor_rel in all_flavors("skills/codemap/SKILL.md") {
        let body = load(&flavor_rel);
        assert!(
            !body.contains("PreToolUse blocks worker dispatch"),
            "{flavor_rel} claims a block the gate never performs: pre_tool.rs:352-356 \
             fires only on SignificantlyStale, for supervisors, on task create / \
             spawn_workers"
        );
        assert!(
            body.contains("SignificantlyStale"),
            "{flavor_rel} must name the real gate condition"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Merges, inlines, retirements
// ---------------------------------------------------------------------------

/// Files removed by this wave: the merged skill and the sub-13-line references
/// that sat behind a pointer, which the yardstick's own rule forbids.
const REMOVED_BUILTIN_PATHS: [&str; 4] = [
    "skills/cas-domain-modeling/SKILL.md",
    "skills/cas-codebase-design/DEEPENING.md",
    "skills/cas-codebase-design/DESIGN-IT-TWICE.md",
    "skills/cas-writing-for-agents/SKILL-MECHANICS.md",
];

#[test]
fn merged_and_inlined_builtins_are_gone_from_disk_and_from_every_catalog() {
    let checkout_root = checkout_builtins_root();
    for rel in REMOVED_BUILTIN_PATHS {
        if let Some(root) = checkout_root.as_ref() {
            for flavor_rel in all_flavors(rel) {
                assert!(
                    !root.join(&flavor_rel).exists(),
                    "{flavor_rel} was merged or inlined and must be deleted"
                );
            }
        }
        for (name, catalog) in [
            ("BUILTIN_SKILLS", BUILTIN_SKILLS),
            ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
            ("GROK_BUILTIN_SKILLS", GROK_BUILTIN_SKILLS),
        ] {
            assert!(
                !catalog.iter().any(|b| b.path == rel),
                "{name} still registers the removed builtin {rel}"
            );
        }
    }
}

#[test]
fn codebase_design_absorbs_domain_modeling_and_its_two_inlined_references() {
    for flavor_rel in all_flavors("skills/cas-codebase-design/SKILL.md") {
        let body = load(&flavor_rel);
        for marker in [
            // the unique cas-domain-modeling content
            "Challenge the language",
            // DEEPENING.md
            "deletion-test",
            "local-substitutable",
            // DESIGN-IT-TWICE.md
            "three materially different interfaces",
        ] {
            assert!(
                body.contains(marker),
                "{flavor_rel} must carry the merged/inlined content marker {marker:?}"
            );
        }
        assert!(
            !body.contains("DEEPENING.md") && !body.contains("DESIGN-IT-TWICE.md"),
            "{flavor_rel} still links references that were inlined and deleted"
        );
        assert!(
            !body.contains("NestJS"),
            "{flavor_rel} still carries a downstream-project carve-out"
        );
    }
}

/// Nothing in the codebase spawns these two: `grep -rn` outside the builtins
/// tree finds only the registry, the marker tests and one CHANGELOG entry.
/// Their bodies also instruct tools their own `tools:` list excludes.
const RETIRED_AGENTS: [&str; 2] = ["git-history-analyzer", "issue-intelligence-analyst"];

#[test]
fn unwired_agents_are_retired_from_every_agent_registry() {
    let checkout_root = checkout_builtins_root();
    for agent in RETIRED_AGENTS {
        let rel = format!("agents/{agent}.md");
        if let Some(root) = checkout_root.as_ref() {
            for flavor_rel in all_flavors(&rel) {
                assert!(
                    !root.join(&flavor_rel).exists(),
                    "{flavor_rel} is retired and must be deleted"
                );
            }
        }
        for (name, catalog) in [
            ("BUILTIN_AGENTS", BUILTIN_AGENTS),
            ("CODEX_BUILTIN_AGENTS", CODEX_BUILTIN_AGENTS),
            ("GROK_BUILTIN_AGENTS", GROK_BUILTIN_AGENTS),
        ] {
            assert!(
                !catalog.iter().any(|b| b.path == rel),
                "{name} still registers the retired agent {rel}"
            );
        }
        assert!(
            !REQUIRED_FACTORY_AGENTS.contains(&rel.as_str()),
            "REQUIRED_FACTORY_AGENTS still requires the retired agent {rel}; a spawn \
             would fail on a missing file"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. The yardstick
// ---------------------------------------------------------------------------

#[test]
fn writing_for_agents_meets_the_bar_it_sets_for_other_skills() {
    for flavor_rel in all_flavors("skills/cas-writing-for-agents/SKILL.md") {
        let body = load(&flavor_rel);

        assert!(
            body.contains("## Steps"),
            "{flavor_rel} must give the steps it demands of every other skill"
        );
        for step in ["1.", "2.", "3.", "4.", "5."] {
            assert!(
                body.contains(step),
                "{flavor_rel} must number its steps ({step} missing)"
            );
        }
        assert!(
            body.contains("Done when"),
            "{flavor_rel} must state an observable completion criterion"
        );
        // Required frontmatter fields, stated as house facts.
        for field in [
            "`name`",
            "`description`",
            "`managed_by`",
            "`disable-model-invocation`",
            "`disallowed-tools`",
        ] {
            assert!(
                body.contains(field),
                "{flavor_rel} must name the frontmatter field {field}"
            );
        }
        assert!(
            body.contains("Use when"),
            "{flavor_rel} must pin the \"Use when …\" description convention"
        );
        // The three-mirror rule.
        assert!(
            body.contains("codex") && body.contains("grok"),
            "{flavor_rel} must state the three-mirror rule (claude canonical + codex + \
             grok twins)"
        );
        // A line budget, and the absorbed skill mechanics.
        assert!(
            body.contains("80 lines"),
            "{flavor_rel} must state a line budget"
        );
        for mechanic in ["disable-model-invocation: true", "router"] {
            assert!(
                body.contains(mechanic),
                "{flavor_rel} must absorb the skill-mechanics marker {mechanic:?}"
            );
        }
        assert!(
            !body.contains("SKILL-MECHANICS.md"),
            "{flavor_rel} still links the reference it absorbed"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. Stance after procedure
// ---------------------------------------------------------------------------

#[test]
fn html_reports_leads_with_the_procedure_not_the_stance() {
    for flavor_rel in all_flavors("skills/cas-html-reports/SKILL.md") {
        let body = load(&flavor_rel);
        assert!(
            line_index(&body, "## The workflow") < line_index(&body, "## What counts as a report"),
            "{flavor_rel}: the first step must precede the stance sections"
        );
    }
}

#[test]
fn github_issues_leads_with_the_sweep_not_the_banner_essay() {
    for flavor_rel in all_flavors("skills/cas-github-issues/SKILL.md") {
        let body = load(&flavor_rel);
        assert!(
            line_index(&body, "## 1. List open issues")
                < line_index(&body, "unfiled-reports banner"),
            "{flavor_rel}: the seventeen-line banner preamble must sit after the steps"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. P3 hygiene
// ---------------------------------------------------------------------------

#[test]
fn methodology_skills_name_the_task_note_type_they_expect() {
    for flavor_rel in all_flavors("skills/cas-diagnosing-bugs/SKILL.md") {
        assert!(
            load(&flavor_rel).contains("note_type=discovery"),
            "{flavor_rel} must name the note type it wants recorded"
        );
    }
    for flavor_rel in all_flavors("skills/cas-resolving-merge-conflicts/SKILL.md") {
        assert!(
            load(&flavor_rel).contains("note_type=decision"),
            "{flavor_rel} must name the note type it wants recorded"
        );
    }
}

#[test]
fn codex_exec_does_not_pin_a_stale_model_slug() {
    for flavor_rel in all_flavors("skills/cas-codex-exec/SKILL.md") {
        let body = load(&flavor_rel);
        assert!(
            !body.contains("gpt-5.5"),
            "{flavor_rel} pins a model slug that is not the box default; omit -m and let \
             the configured default win"
        );
        assert!(
            !body.contains("-m gpt"),
            "{flavor_rel} must not pin any -m model slug in the canonical recipe"
        );
    }
}

#[test]
fn wizard_template_shows_the_safe_confirm_form_under_set_e() {
    for flavor_rel in all_flavors("skills/cas-wizard/template.sh") {
        let body = load(&flavor_rel);
        assert!(
            body.contains("set -euo pipefail"),
            "{flavor_rel} must keep the strict shell mode"
        );
        assert!(
            body.contains("if confirm"),
            "{flavor_rel} must show `confirm` wrapped in an `if`; a bare `confirm` \
             returning 1 aborts the whole wizard under `set -e`"
        );
        assert!(
            body.lines().count() >= 24,
            "{flavor_rel} is truncated: the twins previously dropped the example block \
             because the drift guard compared only .md files"
        );
    }
}
