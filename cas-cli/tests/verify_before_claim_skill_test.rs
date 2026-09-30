//! Install-path proof for the `verify-before-claim` skill (cas-5b2a).
//!
//! Acceptance criteria for cas-5b2a require the skill file to ship in both
//! the Claude and Codex builtin trees, with `managed_by: cas` frontmatter
//! (so `cas update --sync` propagates it) and the four-step protocol body.
//! These tests fail loudly if a future refactor renames the directory,
//! drops the frontmatter, deletes a protocol step, or forgets to register
//! the skill in `BUILTIN_SKILLS` / `CODEX_BUILTIN_SKILLS`.

#[path = "support/builtin_catalog.rs"]
mod builtin_catalog;

// pin: Registration completeness includes checked-in source entries, so omitted skills cannot pass by reading only the installed catalog.
fn load(rel: &str) -> &'static str {
    match rel {
        "cas-cli/src/builtins.rs" => include_str!("../src/builtins.rs"),
        _ => builtin_catalog::find_source_path(rel),
    }
}

#[test]
fn claude_builtin_skill_exists_and_is_managed_by_cas() {
    let content = load("cas-cli/src/builtins/skills/verify-before-claim/SKILL.md");
    assert!(
        content.starts_with("---\n"),
        "verify-before-claim SKILL.md must start with YAML frontmatter"
    );
}

#[test]
fn codex_builtin_skill_exists_and_is_managed_by_cas() {
    let content = load("cas-cli/src/builtins/codex/skills/verify-before-claim/SKILL.md");
    assert!(
        content.starts_with("---\n"),
        "verify-before-claim codex SKILL.md must start with YAML frontmatter"
    );
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

    // Codex and Grok embed the same prefix-neutral file (audit D1).
    for flavor in [builtin_catalog::Flavor::Codex, builtin_catalog::Flavor::Grok] {
        assert_eq!(
            builtin_catalog::try_find(flavor, "skills/verify-before-claim/SKILL.md"),
            Some(include_str!("../src/builtins/skills/verify-before-claim/SKILL.md")),
            "{flavor:?} catalog must register verify-before-claim"
        );
    }

    // And the destination path the syncer writes to must be the canonical
    // `skills/verify-before-claim/SKILL.md` form.
    assert!(
        builtins.contains("skills/verify-before-claim/SKILL.md"),
        "cas-cli/src/builtins.rs must declare the destination path \
         `skills/verify-before-claim/SKILL.md`"
    );
}

#[test]
fn tests_pass_link_resolves_to_worker_close_gate_in_all_flavors() {
    for path in [
        "cas-cli/src/builtins/skills/verify-before-claim/SKILL.md",
        "cas-cli/src/builtins/codex/skills/verify-before-claim/SKILL.md",
        "cas-cli/src/builtins/grok/skills/verify-before-claim/SKILL.md",
    ] {
        let close_gate_path = path.replace(
            "verify-before-claim/SKILL.md",
            "cas-worker/references/close-gate.md",
        );
        assert!(
            !load(&close_gate_path).is_empty(),
            "{path} close-gate link target must be embedded"
        );
    }
}
