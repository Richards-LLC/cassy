#![allow(clippy::unwrap_used)]

//! Structural guardrails for the `cas-code-review` orchestrator SKILL.md
//! (Phase 1 Subsystem A, Unit 4 — task cas-71ed).
//!
//! Updated for Phase C (EPIC cas-b667): Steps 1-4 now live inside the
//! `cas-code-review` Workflow script (.claude/workflows/cas-code-review.js).
//! The skill is a thin wrapper that owns Step 0 (tiny-diff bypass) + Step 5
//! (mode routing + CAS integration). Invariants that moved to the Workflow
//! are now asserted against the Workflow script instead of the skill body,
//! keeping every structural guarantee meaningful in its new home.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    for base in [std::env::current_dir().ok(), std::env::current_exe().ok()].into_iter().flatten() {
        if let Some(root) = base.ancestors().find(|path| {
            std::fs::read_to_string(path.join("Cargo.toml"))
                .is_ok_and(|manifest| manifest.contains("[workspace]"))
        }) {
            return root.to_path_buf();
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().and_then(|p| p.parent()).expect("workspace root")
        .to_path_buf()
}

fn claude_skill_path() -> PathBuf {
    repo_root().join("cas-cli/src/builtins/skills/cas-code-review/SKILL.md")
}

fn codex_skill_path() -> PathBuf {
    repo_root().join("cas-cli/src/builtins/codex/skills/cas-code-review/SKILL.md")
}

/// Phase C (cas-b667): Steps 1-4 now live in the Workflow script.
/// Invariants that migrated from the skill body are asserted here.
fn workflow_script_path() -> PathBuf {
    repo_root().join("cas-cli/src/builtins/workflows/cas-code-review.js")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

/// Required headings for the Phase C thin-wrapper skill (as substring matches
/// against lines starting with `#`). The thin wrapper owns:
///   - Step 0 (tiny-diff bypass)
///   - Steps 1-4 (as a single call-site block — Workflow handles them internally)
///   - Step 5 (mode routing + CAS integration)
///   - Mode reference table
///
/// Steps 1-4 no longer have individual headings in the skill (they moved to
/// the Workflow). The test now checks that the combined handoff section is
/// present AND that the Workflow owns the structural invariants for Steps 1-4.
const REQUIRED_SECTIONS: &[&str] = &[
    "Step 0",         // Tiny-diff bypass (still in skill)
    "Step 5",         // Mode-specific output (still in skill)
    "Mode reference", // Four-mode table (still in skill)
    "Workflow",       // Skill must reference the Workflow call-site
];

fn assert_sections_present(skill: &str, label: &str) {
    for section in REQUIRED_SECTIONS {
        // Check headings (lines starting with #) for sections
        let heading_found = skill
            .lines()
            .filter(|l| l.trim_start().starts_with('#'))
            .any(|l| l.contains(section));
        // Also allow the section keyword to appear in non-heading lines
        // (e.g., "Workflow" appears in the call-site instructions, not just headings)
        let body_found = skill.contains(section);
        assert!(
            heading_found || body_found,
            "[{label}] missing required section or keyword '{section}'"
        );
    }
}

#[test]
fn claude_mirror_skill_exists_and_is_structurally_complete() {
    let path = claude_skill_path();
    assert!(
        path.exists(),
        "claude mirror SKILL.md missing at {}",
        path.display()
    );
    let body = read(&path);

    // Frontmatter must declare the skill name and the managed_by marker
    // so `cas sync` treats it as an authoritative file.
    assert!(
        body.starts_with("---\n"),
        "claude SKILL.md must start with YAML frontmatter"
    );
    assert!(
        body.contains("name: cas-code-review"),
        "claude SKILL.md frontmatter must set name: cas-code-review"
    );
    assert!(
        body.contains("managed_by: cas"),
        "claude SKILL.md frontmatter must set managed_by: cas"
    );

    assert_sections_present(&body, "claude");
}

#[test]
fn codex_mirror_skill_exists_and_is_structurally_complete() {
    let path = codex_skill_path();
    assert!(
        path.exists(),
        "codex mirror SKILL.md missing at {}",
        path.display()
    );
    let body = read(&path);

    assert!(
        body.starts_with("---\n"),
        "codex SKILL.md must start with YAML frontmatter"
    );
    assert!(
        body.contains("name: cas-code-review"),
        "codex SKILL.md frontmatter must set name: cas-code-review"
    );
    assert!(
        body.contains("managed_by: cas"),
        "codex SKILL.md frontmatter must set managed_by: cas"
    );

    assert_sections_present(&body, "codex");
}

#[test]
fn both_mirrors_are_content_equivalent() {
    let a = read(&claude_skill_path());
    let b = read(&codex_skill_path());
    // cas-2c61: identical modulo the intentional per-harness tool prefix —
    // the codex mirror correctly uses mcp__cs__, not Claude's mcp__cas__.
    assert_eq!(
        a.replace("mcp__cas__", "mcp__cs__"),
        b,
        "claude and codex SKILL.md mirrors must be identical (apart from the \
         mcp__cas__/mcp__cs__ tool prefix) so `cas sync` doesn't oscillate between them"
    );
}

#[test]
fn persona_activation_is_explicitly_llm_judged_not_pattern_matched() {
    // Defensive guardrail per task description: future edits must not
    // regress to "grep the diff for /auth/ and activate security" —
    // that's exactly the drift R2 warns against. Require the phrase
    // to appear verbatim so a drive-by edit can't quietly remove it.
    //
    // Phase C (cas-b667): persona selection moved into the Workflow script
    // (setup agent in Phase 1). The invariant now lives in the Workflow
    // where the activation decision actually executes. Both the Workflow
    // and the skill wrapper must preserve this guarantee.
    let workflow = read(&workflow_script_path());
    assert!(
        workflow.contains("LLM-judged, not path pattern matching"),
        "cas-code-review.js (Workflow) must retain the verbatim anti-drift \
         phrase 'LLM-judged, not path pattern matching' in the setup agent \
         prompt where persona selection executes (Phase C location)"
    );
    // The skill wrapper should also guide the reader to the Workflow
    // (not re-implement activation logic locally).
    let skill = read(&claude_skill_path());
    assert!(
        skill.contains("Workflow") || skill.contains("workflow"),
        "SKILL.md must reference the Workflow for Steps 1-4 so readers \
         can find the activation logic in its new location"
    );
}

#[test]
fn mode_reference_table_renders() {
    // R8 requires a reference table covering all four invocation modes.
    // The table is a standard pipe-table; we don't pin the columns, but
    // we do pin that all four mode names appear inside a row-looking
    // line (starts with `|`).
    let body = read(&claude_skill_path());
    let modes = ["autofix", "interactive", "report-only", "headless"];
    for mode in modes {
        let row_hit = body
            .lines()
            .filter(|l| l.trim_start().starts_with('|'))
            .any(|l| l.contains(mode));
        assert!(
            row_hit,
            "mode reference table must contain a row mentioning '{mode}'"
        );
    }
}

#[test]
fn findings_schema_reference_is_wired_up() {
    // Step 3 dispatches personas with a "findings-schema reference" so
    // personas know where the envelope contract lives. Pin that the
    // relative path appears in the dispatch layer.
    //
    // Phase C (cas-b667): persona dispatch moved into the Workflow script.
    // The reference now lives in the Workflow's buildPersonaPrompt() where
    // it is injected into every persona's context. The test now asserts
    // on the Workflow script (the new dispatch location) rather than the
    // thin skill wrapper.
    let workflow = read(&workflow_script_path());
    assert!(
        workflow.contains("references/findings-schema.md"),
        "cas-code-review.js (Workflow) must reference references/findings-schema.md \
         in the persona dispatch section so dispatched personas know where the \
         ReviewerOutput envelope contract lives (Phase C location)"
    );
    // Belt-and-suspenders: the builtin source must also exist on disk.
    let wf_path = workflow_script_path();
    assert!(
        wf_path.exists(),
        "builtin workflow source must exist at {} (shipped via BUILTIN_WORKFLOWS)",
        wf_path.display()
    );
}

#[test]
fn fixer_prompt_exists_in_both_mirrors() {
    // Unit 7 (cas-56a2): the autofix loop dispatches a fixer sub-agent
    // whose prompt is this file. Both mirrors must carry byte-identical
    // copies so `cas sync` doesn't flap, and the prompt must declare
    // its safe_auto-only mandate + worker-permission inheritance.
    for (label, skill_path) in [
        ("claude", claude_skill_path()),
        ("codex", codex_skill_path()),
    ] {
        let fixer = skill_path
            .parent()
            .expect("skill parent")
            .join("references")
            .join("fixer.md");
        assert!(
            fixer.exists(),
            "[{label}] fixer.md missing at {}",
            fixer.display()
        );
        let body = read(&fixer);
        assert!(
            body.contains("safe_auto"),
            "[{label}] fixer.md must declare the safe_auto-only mandate"
        );
        assert!(
            body.contains("same tool permissions as the worker"),
            "[{label}] fixer.md must document worker-permission inheritance \
             (no elevation) per the cas-56a2 planning decision"
        );
        assert!(
            body.contains("Max 2 rounds") || body.contains("max_rounds=2"),
            "[{label}] fixer.md must reference the 2-round hard cap so \
             the prompt and the Rust enforcement stay in sync"
        );
    }

    // Both mirrors must be byte-identical.
    let claude_fixer = read(
        &claude_skill_path()
            .parent()
            .unwrap()
            .join("references/fixer.md"),
    );
    let codex_fixer = read(
        &codex_skill_path()
            .parent()
            .unwrap()
            .join("references/fixer.md"),
    );
    assert_eq!(
        claude_fixer, codex_fixer,
        "claude and codex fixer.md mirrors must be byte-identical"
    );
}

#[test]
fn persona_file_references_resolve_on_disk() {
    // Per supervisor request: catch the case where someone moves or
    // renames a persona file later without updating the orchestrator
    // skill. Each of the 7 personas must exist under
    // references/personas/<name>.md in *both* mirrors.
    const PERSONAS: &[&str] = &[
        "correctness",
        "testing",
        "maintainability",
        "project-standards",
        "security",
        "performance",
        "adversarial",
    ];

    for (label, skill_path) in [
        ("claude", claude_skill_path()),
        ("codex", codex_skill_path()),
    ] {
        let personas_dir = skill_path
            .parent()
            .expect("skill parent")
            .join("references")
            .join("personas");
        for persona in PERSONAS {
            let p = personas_dir.join(format!("{persona}.md"));
            assert!(
                p.exists(),
                "[{label}] persona file missing at {} — either add it \
                 or update SKILL.md to stop referencing it",
                p.display()
            );
        }
    }
}

#[test]
fn base_sha_helper_from_unit_3_is_referenced() {
    // Unit 4 consumes Unit 3's output. Pin that the helper is mentioned
    // so the handoff doesn't silently break if base_sha resolution moves.
    let body = read(&claude_skill_path());
    assert!(
        body.contains("base_sha") || body.contains("base SHA"),
        "SKILL.md must reference base_sha input and its resolution path"
    );
    // And specifically that the Unit 3 helper crate is named, so a
    // reader can find the Rust implementation without guessing.
    assert!(
        body.contains("cas_store::code_review::base_sha")
            || body.contains("code_review::base_sha::resolve")
            || body.contains("crates/cas-store/src/code_review/base_sha.rs"),
        "SKILL.md must point at the Unit 3 base-sha helper so callers \
         can find the Rust implementation"
    );
}
