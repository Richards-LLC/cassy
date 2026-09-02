//! Install-path proof for the `verify-before-claim` skill (cas-5b2a).
//!
//! Acceptance criteria for cas-5b2a require the skill file to ship in both
//! the Claude and Codex builtin trees, with `managed_by: cas` frontmatter
//! (so `cas update --sync` propagates it) and the four-step protocol body.
//! These tests fail loudly if a future refactor renames the directory,
//! drops the frontmatter, deletes a protocol step, or forgets to register
//! the skill in `BUILTIN_SKILLS` / `CODEX_BUILTIN_SKILLS`.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    cas::test_paths::workspace_root()
}

fn load(rel: &str) -> String {
    let p = repo_root().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("failed to read {}: {}", p.display(), e))
}

#[test]
fn claude_builtin_skill_exists_and_is_managed_by_cas() {
    let content = load("cas-cli/src/builtins/skills/verify-before-claim/SKILL.md");
    assert!(
        content.starts_with("---\n"),
        "verify-before-claim SKILL.md must start with YAML frontmatter"
    );
    assert!(
        content.contains("managed_by: cas"),
        "verify-before-claim SKILL.md must declare `managed_by: cas` so `cas update --sync` \
         keeps user copies fresh"
    );
    assert!(
        content.contains("name: verify-before-claim"),
        "verify-before-claim SKILL.md frontmatter must declare its name"
    );
}

#[test]
fn codex_builtin_skill_exists_and_is_managed_by_cas() {
    let content = load("cas-cli/src/builtins/codex/skills/verify-before-claim/SKILL.md");
    assert!(
        content.starts_with("---\n"),
        "verify-before-claim codex SKILL.md must start with YAML frontmatter"
    );
    assert!(
        content.contains("managed_by: cas"),
        "verify-before-claim codex SKILL.md must declare `managed_by: cas`"
    );
    assert!(
        content.contains("name: verify-before-claim"),
        "verify-before-claim codex SKILL.md frontmatter must declare its name"
    );
}

#[test]
fn skill_body_carries_the_four_step_protocol() {
    // The skill exists to encode a specific four-step protocol. If any of
    // these step headers disappear, the skill stops being the thing the
    // worker is told to invoke — fail loudly rather than silently drift.
    for path in [
        "cas-cli/src/builtins/skills/verify-before-claim/SKILL.md",
        "cas-cli/src/builtins/codex/skills/verify-before-claim/SKILL.md",
    ] {
        let content = load(path);
        for marker in [
            "### 1. Name the proof command",
            "### 2. Run it FRESH",
            "### 3. Capture exit code",
            "### 4. Only then, close",
        ] {
            assert!(
                content.contains(marker),
                "{path} missing required protocol marker: `{marker}`"
            );
        }
        // The advisory-vs-required decision must remain documented inline
        // per the AC ("decision required and documented inline").
        assert!(
            content.contains("Advisory vs Required-Paste"),
            "{path} missing the advisory-vs-required decision section"
        );
    }
}

#[test]
fn skill_is_registered_in_builtins_rs_for_both_harnesses() {
    let builtins = load("cas-cli/src/builtins.rs");

    // Claude variant must be wired into BUILTIN_SKILLS via include_str!.
    assert!(
        builtins.contains("builtins/skills/verify-before-claim/SKILL.md"),
        "cas-cli/src/builtins.rs must include verify-before-claim in BUILTIN_SKILLS \
         (an `include_str!(\"builtins/skills/verify-before-claim/SKILL.md\")` entry)"
    );

    // Codex variant must be wired into CODEX_BUILTIN_SKILLS via include_str!.
    assert!(
        builtins.contains("builtins/codex/skills/verify-before-claim/SKILL.md"),
        "cas-cli/src/builtins.rs must include verify-before-claim in CODEX_BUILTIN_SKILLS \
         (an `include_str!(\"builtins/codex/skills/verify-before-claim/SKILL.md\")` entry)"
    );

    // And the destination path the syncer writes to must be the canonical
    // `skills/verify-before-claim/SKILL.md` form.
    assert!(
        builtins.contains("skills/verify-before-claim/SKILL.md"),
        "cas-cli/src/builtins.rs must declare the destination path \
         `skills/verify-before-claim/SKILL.md`"
    );
}

#[test]
fn cas_worker_guide_mentions_verify_before_claim_in_pre_close() {
    // AC: cas-worker.md must add a single sentence in the pre-close section
    // instructing the worker to invoke verify-before-claim before close.
    for path in [
        "cas-cli/src/builtins/skills/cas-worker.md",
        "cas-cli/src/builtins/codex/skills/cas-worker.md",
    ] {
        let content = load(path);
        assert!(
            content.contains("verify-before-claim"),
            "{path} must mention the verify-before-claim skill in the pre-close step"
        );
    }
}

#[test]
fn tests_pass_link_resolves_to_worker_close_gate_in_all_flavors() {
    let expected = "[cas-worker/references/close-gate.md](../cas-worker/references/close-gate.md)";
    for path in [
        "cas-cli/src/builtins/skills/verify-before-claim/SKILL.md",
        "cas-cli/src/builtins/codex/skills/verify-before-claim/SKILL.md",
        "cas-cli/src/builtins/grok/skills/verify-before-claim/SKILL.md",
    ] {
        let skill_path = repo_root().join(path);
        let content = load(path);
        assert!(content.contains(expected), "{path} must use the worker close-gate link");
        assert!(
            skill_path
                .parent()
                .expect("skill has a parent")
                .join("../cas-worker/references/close-gate.md")
                .is_file(),
            "{path} close-gate link target must resolve"
        );
    }
}
