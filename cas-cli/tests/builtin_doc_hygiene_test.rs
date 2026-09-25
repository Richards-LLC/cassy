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
use cas::maintenance_jobs::MAINTENANCE_JOBS;

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
        // The one-tree rule (audit D1): one canonical copy with bare tool
        // names, no codex/grok twin trees, and the named per-harness files.
        for marker in [
            "is the one copy",
            "no `codex/` or `grok/` twin tree",
            "Name Cassy tools by bare name",
            "three Codex-only ones under `builtins/codex/`",
        ] {
            assert!(
                body.contains(marker),
                "{flavor_rel} must state the one-tree rule ({marker:?} missing)"
            );
        }
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

// ============================================================================
// Operator-data lint
// ============================================================================

/// Patterns that mark operator-private or cas-src-only data. Every builtin
/// ships into every downstream project, so none of these may appear unless
/// the file is allowlisted below with a reason.
const OPERATOR_DATA_RULES: &[(&str, &str)] = &[
    (
        "e-mail",
        r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}",
    ),
    ("home-path", r"/home/[a-z_][a-z0-9_-]*"),
    ("codex-account-dir", r"~/\.codex-"),
    ("task-id", r"\bcas-[0-9a-f]{4,5}\b"),
    ("operator-org", r"Richards-LLC"),
    ("operator-project", r"(?i)gabber"),
    ("cas-src-test-command", r"nextest -p cas\b"),
];

/// Obvious placeholders are not operator data: documentation e-mail domains
/// (RFC 2606 / RFC 6761) and synthetic task ids used in worked examples.
fn is_synthetic(rule: &str, value: &str) -> bool {
    match rule {
        "e-mail" => {
            regex::Regex::new(r"@(example\.(com|org|net)|[a-z0-9.-]+\.(test|example|invalid))$")
                .unwrap()
                .is_match(value)
        }
        "task-id" => {
            let id = &value["cas-".len()..];
            matches!(id, "1234" | "abcd" | "abc1" | "a1b2")
                || id.chars().all(|c| Some(c) == id.chars().next())
        }
        _ => false,
    }
}

/// `(catalog path, rule, reason)`. The path is flavour-agnostic: an entry
/// covers the Claude, Codex and Grok copies of that file.
const OPERATOR_DATA_ALLOWLIST: &[(&str, &str, &str)] = &[
    (
        "skills/cas-cut-release/references/failure-log.md",
        "task-id",
        "cas-src release-train failure log; each entry cites the ticket that fixed the failure. \
         The release trio is cas-src-only content that moves out of universal builtins (audit M51).",
    ),
    (
        "skills/cas-release-report/references/exemplar.md",
        "operator-org",
        "Links to the Cassy repo's own release-report sources the exemplar was rendered from; \
         cas-src-only release content (audit M51).",
    ),
    (
        "skills/cas-qa-craft/references/matrix-builder.md",
        "operator-org",
        "Attribution for the Cassy issue the matrix guidance was adapted from; pinned by the \
         builtins and agent_definition_contract_test markers.",
    ),
    (
        "skills/cas-worker/references/close-gate.md",
        "task-id",
        "Factory-core reference owned by the WP7 accuracy rewrite; ids tag the close gates it documents.",
    ),
    (
        "skills/cas-supervisor/references/reference.md",
        "task-id",
        "Factory-core reference owned by the WP7 accuracy rewrite; ids tag the guards it documents.",
    ),
    (
        "skills/cas-supervisor/references/worker-recovery.md",
        "task-id",
        "Factory-core reference owned by the WP7 accuracy rewrite; ids tag the recovery incidents it documents.",
    ),
    (
        "skills/cas-supervisor/references/workflow.md",
        "task-id",
        "Factory-core reference owned by the WP7 accuracy rewrite; ids tag the guards it documents.",
    ),
];

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
    for (path, rule, reason) in OPERATOR_DATA_ALLOWLIST {
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

// ============================================================================
// AI-vocabulary and abstract-metaphor lint
// ============================================================================

/// This is a word-list lint over shipped Markdown, not a collection of
/// sentence pins. Review a hit in context before adding a file/phrase
/// exception: `leverage` can be an architecture noun, `journey` a named QA
/// flow, and `landscape` a physical page orientation.
const AI_VOCABULARY_RULES: &[(&str, &str)] = &[
    ("delve", r"\bdelv(?:e|es|ed|ing)\b"),
    ("leverage", r"\bleverag(?:e|es|ed|ing)\b"),
    ("seamless", r"\bseamless(?:ly)?\b"),
    ("robust", r"\brobust(?:ly)?\b"),
    ("tapestry", r"\btapestr(?:y|ies)\b"),
    ("landscape", r"\blandscapes?\b"),
    ("realm", r"\brealms?\b"),
    ("journey", r"\bjourneys?\b"),
    ("serves as", r"\bserves?\s+as\b"),
    ("it's worth noting", r"\bit['’]s\s+worth\s+noting\b"),
    ("unlock", r"\bunlock(?:s|ed|ing)?\b"),
    ("empower", r"\bempower(?:s|ed|ing)?\b"),
    ("transformative", r"\btransformative\b"),
    ("game-changer", r"\bgame[ -]chang(?:er|ers|ing)\b"),
];

/// Exact catalog path, phrase name, and why that use is literal or a term of
/// art. A file-level exception is intentionally visible and checked for drift.
const AI_VOCABULARY_ALLOWLIST: &[(&str, &str, &str)] = &[
    (
        "skills/cas-codebase-design/SKILL.md",
        "leverage",
        "Architecture term for caller benefit from a deeper module interface.",
    ),
    (
        "skills/cas-tdd/SKILL.md",
        "leverage",
        "Names the architecture vocabulary taught by cas-codebase-design.",
    ),
    (
        "skills/cas-cut-release/SKILL.md",
        "journey",
        "Names the journey-eval QA command and required release evidence.",
    ),
    (
        "skills/cas-frontend-engineering/SKILL.md",
        "journey",
        "A Playwright journey is a concrete end-to-end test scenario.",
    ),
    (
        "skills/cas-html-reports/references/report-types.md",
        "journey",
        "Product journey is a defined report diagram type and reader path.",
    ),
    (
        "skills/cas-html-reports/references/review-checklist.md",
        "journey",
        "Journey diagram is a specific visual checked for print legibility.",
    ),
    (
        "skills/cas-qa-craft/SKILL.md",
        "journey",
        "User journey is the QA evidence unit defined by this skill.",
    ),
    (
        "skills/cas-qa-craft/references/evidence-bundle.md",
        "journey",
        "Names the journey evidence bundle and its producer field.",
    ),
    (
        "skills/cas-qa-craft/references/independent-pass.md",
        "journey",
        "Names the journey suite and its scored QA path.",
    ),
    (
        "skills/cas-qa-craft/references/journeys.md",
        "journey",
        "Defines the project's end-to-end user-flow terminology and file names.",
    ),
    (
        "skills/cas-worker/SKILL.md",
        "journey",
        "Refers to the catalog journey QA trigger in the worker contract.",
    ),
    (
        "skills/cas-worker/references/close-gate.md",
        "journey",
        "Names a catalog journey as a user-facing QA evidence trigger.",
    ),
    (
        "skills/codemap/SKILL.md",
        "journey",
        "Identifies product journeys as content owned by project-overview.",
    ),
    (
        "skills/project-overview/SKILL.md",
        "journey",
        "A journey is a named product user flow in the overview template.",
    ),
    (
        "skills/project-overview/SKILL.md",
        "empower",
        "Appears only as a marked bad example of vague product copy.",
    ),
    (
        "skills/verify-before-claim/SKILL.md",
        "journey",
        "Refers to the end-to-end user-path verification rung.",
    ),
    (
        "skills/cas-technical-drawing/SKILL.md",
        "landscape",
        "Landscape means a physical page orientation for the drawing sheet.",
    ),
    (
        "skills/cas-technical-drawing/references/drafting-conventions.md",
        "landscape",
        "Landscape means a physical page orientation in drafting conventions.",
    ),
    (
        "skills/cas-technical-drawing/references/model-schema.md",
        "landscape",
        "Landscape is a literal value of the drawing sheet schema.",
    ),
];

#[test]
fn shipped_builtin_markdown_has_no_unapproved_ai_vocabulary() {
    let rules: Vec<(&str, regex::Regex)> = AI_VOCABULARY_RULES
        .iter()
        .map(|(phrase, pattern)| {
            (
                *phrase,
                regex::RegexBuilder::new(pattern)
                    .case_insensitive(true)
                    .build()
                    .unwrap(),
            )
        })
        .collect();
    let mut violations = Vec::new();
    let mut allowlist_used = vec![false; AI_VOCABULARY_ALLOWLIST.len()];

    let mut inspect =
        |source: &str, path: &str, content: &str| {
            for (line_number, line) in content.lines().enumerate() {
                for (phrase, pattern) in &rules {
                    if !pattern.is_match(line) {
                        continue;
                    }
                    if let Some(index) = AI_VOCABULARY_ALLOWLIST.iter().position(
                        |(allowed_path, allowed_phrase, _)| {
                            *allowed_path == path && allowed_phrase == phrase
                        },
                    ) {
                        allowlist_used[index] = true;
                    } else {
                        violations.push(format!("{path}:{}:{phrase} ({source})", line_number + 1));
                    }
                }
            }
        };

    for (catalog_name, catalog) in shipped_catalogs() {
        for file in catalog.iter().filter(|file| file.path.ends_with(".md")) {
            inspect(catalog_name, file.path, file.content);
        }
    }
    for job in MAINTENANCE_JOBS {
        inspect(
            "MAINTENANCE_JOBS",
            &format!("jobs/{}.md", job.name),
            job.body,
        );
    }

    assert!(
        violations.is_empty(),
        "AI-vocabulary hits in shipped builtin Markdown; rewrite plainly or add an exact (file, phrase, reason) exception to AI_VOCABULARY_ALLOWLIST:\n{}",
        violations.join("\n")
    );
    let stale: Vec<_> = AI_VOCABULARY_ALLOWLIST
        .iter()
        .zip(&allowlist_used)
        .filter(|(_, used)| !**used)
        .map(|((path, phrase, _), _)| format!("{path}: {phrase}"))
        .collect();
    assert!(
        stale.is_empty(),
        "stale AI-vocabulary allowlist entries: {stale:?}"
    );
    for (path, phrase, reason) in AI_VOCABULARY_ALLOWLIST {
        assert!(
            !reason.trim().is_empty(),
            "{path}: {phrase} needs an allowlist reason"
        );
    }
}

#[test]
fn ai_vocabulary_word_list_matches_case_insensitively_and_at_word_boundaries() {
    let patterns: Vec<_> = AI_VOCABULARY_RULES
        .iter()
        .map(|(phrase, regex)| {
            (
                *phrase,
                regex::RegexBuilder::new(regex)
                    .case_insensitive(true)
                    .build()
                    .unwrap(),
            )
        })
        .collect();
    let catches = |text: &str, phrase: &str| {
        patterns
            .iter()
            .any(|(name, pattern)| *name == phrase && pattern.is_match(text))
    };
    assert!(catches("DELVE into this", "delve"));
    assert!(catches("Leverage the API", "leverage"));
    assert!(catches("user journeys", "journey"));
    assert!(catches("It's worth noting", "it's worth noting"));
    assert!(!catches("unleveraged", "leverage"));
    assert!(!catches("subrealm", "realm"));
}
