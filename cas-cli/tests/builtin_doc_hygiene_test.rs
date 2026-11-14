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
    BUILTIN_AGENTS, BUILTIN_SKILLS, BUILTIN_WORKFLOWS, BuiltinFile, CODEX_BUILTIN_AGENTS,
    CODEX_BUILTIN_SKILLS, GROK_BUILTIN_AGENTS, GROK_BUILTIN_SKILLS, REQUIRED_FACTORY_AGENTS,
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



// ---------------------------------------------------------------------------
// 1. Doc family: shared hygiene reference, real signals only
// ---------------------------------------------------------------------------



#[test]
fn doc_hygiene_reference_is_registered_in_every_flavor() {
    let rel = "skills/codemap/references/doc-hygiene.md";
    // Audit D1: one tree. The canonical file exists on disk, and every harness
    // catalog registers it with exactly that content (no twin copy).
    let canonical = std::fs::read_to_string(
        checkout_builtins_root()
            .map(|root| root.join(rel))
            .unwrap_or_default(),
    )
    .ok();
    if let Some(root) = checkout_builtins_root() {
        assert!(root.join(rel).is_file(), "{rel} must exist on disk");
        for twin in ["codex", "grok"] {
            assert!(
                !root.join(twin).join(rel).exists(),
                "{twin}/{rel} must not reappear as a twin copy"
            );
        }
    }
    for (name, catalog) in [
        ("BUILTIN_SKILLS", BUILTIN_SKILLS),
        ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
        ("GROK_BUILTIN_SKILLS", GROK_BUILTIN_SKILLS),
    ] {
        let entry = catalog.iter().find(|b| b.path == rel).unwrap_or_else(|| {
            panic!("{name} must register {rel}; an unregistered reference is never installed")
        });
        assert_eq!(
            entry.content,
            load(rel),
            "{name} {rel} must embed the canonical file"
        );
        if let Some(canonical) = &canonical {
            assert_eq!(
                entry.content,
                canonical.as_str(),
                "{name} {rel} drifted from the file on disk"
            );
        }
    }
}




// ---------------------------------------------------------------------------
// 2. Merges, inlines, retirements
// ---------------------------------------------------------------------------

/// Files removed by this wave: the merged skill and the sub-13-line references
/// that sat behind a pointer, which the yardstick's own rule forbids.
const REMOVED_BUILTIN_PATHS: [&str; 8] = [
    "skills/cas-domain-modeling/SKILL.md",
    "skills/cas-codebase-design/DEEPENING.md",
    "skills/cas-codebase-design/DESIGN-IT-TWICE.md",
    "skills/cas-writing-for-agents/SKILL-MECHANICS.md",
    // The cas-html-reports before/after exemplar was a real operator report
    // (e-mail identities, local account paths, costs, task ids).
    "skills/cas-html-reports/references/examples/before-after/rubric-review-before.html",
    "skills/cas-html-reports/references/examples/before-after/rubric-review-after.html",
    "skills/cas-html-reports/references/examples/before-after/rubric-review.brief.md",
    "skills/cas-html-reports/references/examples/before-after/rubric-review.why.md",
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



// ---------------------------------------------------------------------------
// 4. Stance after procedure
// ---------------------------------------------------------------------------



// ---------------------------------------------------------------------------
// 5. P3 hygiene
// ---------------------------------------------------------------------------




// ============================================================================
// Operator-data lint
// ============================================================================

/// Patterns that mark operator-private or cas-src-only data. Every builtin
/// ships into every downstream project, so none of these may appear unless
/// the file is allowlisted below with a reason.
// Shared with the no-build release row: lane admission and the shipped
// catalog test read the same patterns, synthetic values and exemptions.
static OPERATOR_DATA_POLICY: std::sync::LazyLock<serde_json::Value> =
    std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../../scripts/builtin-doc-hygiene.json"))
            .expect("valid builtin-doc-hygiene policy")
    });

static OPERATOR_DATA_RULES: std::sync::LazyLock<Vec<(&'static str, &'static str)>> =
    std::sync::LazyLock::new(|| {
        OPERATOR_DATA_POLICY["rules"]
            .as_array()
            .expect("hygiene rules")
            .iter()
            .map(|rule| {
                (
                    rule["name"].as_str().unwrap(),
                    rule["pattern"].as_str().unwrap(),
                )
            })
            .collect()
    });

/// Obvious placeholders are not operator data: documentation e-mail domains
/// (RFC 2606 / RFC 6761) and synthetic task ids used in worked examples.
fn is_synthetic(rule: &str, value: &str) -> bool {
    OPERATOR_DATA_POLICY["synthetic"][rule]
        .as_str()
        .is_some_and(|pattern| regex::Regex::new(pattern).unwrap().is_match(value))
}

/// `(catalog path, rule, reason)`. The path is flavour-agnostic: an entry
/// covers the Claude, Codex and Grok copies of that file.
static OPERATOR_DATA_ALLOWLIST: std::sync::LazyLock<
    Vec<(&'static str, &'static str, &'static str)>,
> = std::sync::LazyLock::new(|| {
    OPERATOR_DATA_POLICY["allowlist"]
        .as_array()
        .expect("hygiene allowlist")
        .iter()
        .map(|entry| {
            (
                entry["path"].as_str().unwrap(),
                entry["rule"].as_str().unwrap(),
                entry["reason"].as_str().unwrap(),
            )
        })
        .collect()
});

fn shipped_catalogs() -> [(&'static str, &'static [BuiltinFile]); 7] {
    [
        ("BUILTIN_AGENTS", BUILTIN_AGENTS),
        ("CODEX_BUILTIN_AGENTS", CODEX_BUILTIN_AGENTS),
        ("GROK_BUILTIN_AGENTS", GROK_BUILTIN_AGENTS),
        ("BUILTIN_SKILLS", BUILTIN_SKILLS),
        ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
        ("GROK_BUILTIN_SKILLS", GROK_BUILTIN_SKILLS),
        ("BUILTIN_WORKFLOWS", BUILTIN_WORKFLOWS),
    ]
}

#[test]
fn shipped_builtins_carry_no_operator_data() {
    let rules: Vec<(&str, regex::Regex)> = OPERATOR_DATA_RULES
        .iter()
        .map(|(rule, pattern)| (*rule, regex::Regex::new(pattern).expect("valid rule")))
        .collect();
    let mut violations = Vec::new();
    let mut allowlist_used = vec![false; OPERATOR_DATA_ALLOWLIST.len()];

    for (catalog_name, catalog) in shipped_catalogs() {
        for file in catalog {
            for (rule, pattern) in &rules {
                let hits: Vec<&str> = pattern
                    .find_iter(file.content)
                    .map(|m| m.as_str())
                    .filter(|value| !is_synthetic(rule, value))
                    .collect();
                if hits.is_empty() {
                    continue;
                }
                if let Some(index) = OPERATOR_DATA_ALLOWLIST
                    .iter()
                    .position(|(path, allowed, _)| *path == file.path && allowed == rule)
                {
                    allowlist_used[index] = true;
                    continue;
                }
                violations.push(format!("{catalog_name} {}: {rule} {hits:?}", file.path));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "operator data in shipped builtins (replace with synthetic values such as acme-web, \
         cas-1234 or user@example.com, or allowlist the file with a reason):\n{}",
        violations.join("\n")
    );
    let stale: Vec<_> = OPERATOR_DATA_ALLOWLIST
        .iter()
        .zip(&allowlist_used)
        .filter(|(_, used)| !**used)
        .map(|((path, rule, _), _)| format!("{path} ({rule})"))
        .collect();
    assert!(
        stale.is_empty(),
        "operator-data allowlist entries no longer match anything; delete them: {stale:?}"
    );
    for (path, rule, reason) in OPERATOR_DATA_ALLOWLIST.iter() {
        assert!(
            reason.len() >= 40,
            "allowlist entry {path} ({rule}) needs a stated reason"
        );
    }
}

#[test]
fn operator_data_lint_catches_what_it_names() {
    let rules: Vec<(&str, regex::Regex)> = OPERATOR_DATA_RULES
        .iter()
        .map(|(rule, pattern)| (*rule, regex::Regex::new(pattern).unwrap()))
        .collect();
    let caught = |text: &str| -> Vec<&'static str> {
        rules
            .iter()
            .filter(|(rule, pattern)| {
                pattern
                    .find_iter(text)
                    .any(|m| !is_synthetic(rule, m.as_str()))
            })
            .map(|(rule, _)| *rule)
            .collect()
    };
    assert_eq!(caught("mail ops@acme-corp.io"), ["e-mail"]);
    assert_eq!(caught("see /home/alice/.cas"), ["home-path"]);
    assert_eq!(
        caught("rollout at ~/.codex-work/sessions"),
        ["codex-account-dir"]
    );
    assert_eq!(caught("fixed by cas-4df0"), ["task-id"]);
    assert_eq!(caught("fixed by cas-5c02a"), ["task-id"]);
    assert_eq!(caught("github.com/Richards-LLC/cassy"), ["operator-org"]);
    assert_eq!(caught("project gabber-studio"), ["operator-project"]);
    assert_eq!(caught("run cargo nextest -p cas"), ["cas-src-test-command"]);
    for synthetic in [
        "user@example.com",
        "playwright@yourapp.test",
        "task cas-1234",
        "epic cas-4444",
        "cas-cli and cas-core crates",
        "acme-web",
    ] {
        assert!(
            caught(synthetic).is_empty(),
            "{synthetic:?} is not operator data"
        );
    }
}
