//! Flavor drift guard (cas-703a).
//!
//! The builtin skills/agents ship in three harness flavors that are meant to be
//! the SAME document under a small set of mechanical per-harness spellings:
//!
//!   claude  cas-cli/src/builtins/<path>          tools `mcp__cas__*`
//!   codex   cas-cli/src/builtins/codex/<path>    tools `mcp__cs__*`
//!   grok    cas-cli/src/builtins/grok/<path>     tools `cas__*`
//!
//! Before this guard, coverage was spot checks only (keyword bans, marker
//! presence, catalog-presence parity). That let real contradictions live for
//! months with the whole suite green: the codex task-verifier kept a
//! close-reason keyword blacklist that claude had deleted in April 2026 and was
//! missing its entire Epic Verification section (cas-48aa), the codex
//! supervisor checklist froze in April (cas-59ee), and the "Valid Actions"
//! sections never propagated to the twins at all (fixed in the commit preceding
//! this one).
//!
//! This test normalizes the sanctioned per-harness spellings and then asserts
//! full content equality, section by section. A substantive edit landing in one
//! flavor but not its twins fails here with a readable diff.
//!
//! HOW TO RESOLVE A FAILURE — in order of preference:
//!   1. The divergence is unintentional drift (the common case): port the
//!      change to the other flavors. Pure prefix substitution from the claude
//!      flavor is the established way to do it.
//!   2. The divergence is a NEW mechanical per-harness spelling that applies
//!      corpus-wide: add a rule to `canonicalize`, so the rest of the file
//!      stays guarded.
//!   3. The divergence is genuinely intentional and local: add an
//!      `ALLOWED_SECTION_DIVERGENCE` entry naming the file, flavor, section and
//!      a rationale. Entries are section-level on purpose — exempting a whole
//!      file blinds the guard to everything else in it.
//!
//! Runs under `cargo test --test builtin_flavor_drift_test` (and in any full
//! integration-test run). Repo-local filesystem reads only; no network.
//!
//! Coverage note (cas-5787): `test_skills_document_context_budgeting_cas_5787`
//! in `cas-cli/src/builtins.rs` asserts context-budgeting markers for claude and
//! codex only. Grok is covered here instead, and more strictly: cas-supervisor.md
//! and cas-worker.md are guarded triples, so every marker required of the claude
//! body must appear verbatim in the grok body or this test fails. That test was
//! also extended to name grok directly, so the two mechanisms overlap rather
//! than leaving grok to a single point of failure.

use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Flavors
// ---------------------------------------------------------------------------

struct Flavor {
    /// Human label used in assertion messages.
    name: &'static str,
    /// Subdirectory under `cas-cli/src/builtins` ("" for the claude baseline).
    subdir: &'static str,
}

const CLAUDE: Flavor = Flavor { name: "claude", subdir: "" };
const CODEX: Flavor = Flavor { name: "codex", subdir: "codex" };
const GROK: Flavor = Flavor { name: "grok", subdir: "grok" };

/// Flavors compared against the claude baseline.
const TWINS: [&Flavor; 2] = [&CODEX, &GROK];

// ---------------------------------------------------------------------------
// Sanctioned divergences
// ---------------------------------------------------------------------------

/// Section-level exemptions: (claude-relative path, flavor, section heading, rationale).
///
/// The heading is matched exactly as it appears in the file (after
/// canonicalization). Sections listed here may differ in body OR be absent in
/// that flavor. Everything else in the file is still compared.
const ALLOWED_SECTION_DIVERGENCE: &[(&str, &str, &str, &str)] = &[(
    "skills/cas-worker/references/recovery.md",
    "codex",
    "## Close requires task-scoped verification",
    "The claude body enumerates the per-CLI tool spellings as an audience-facing \
     list ('Claude workers: ... / Codex workers: ...') because a claude worker may \
     be reading on behalf of either. A codex worker has exactly one spelling, so \
     the list collapses to a single inline sentence. Both were written in the same \
     commit; this is presentation, not content.",
), (
    "skills/cas-worker/references/recovery.md",
    "grok",
    "## Close requires task-scoped verification",
    "Same rationale as the codex entry above: the per-CLI enumeration collapses to \
     one line for a single-spelling harness.",
)];

/// Claude files with no counterpart in a given flavor: (path, flavor, rationale).
const ALLOWED_MISSING_TWIN: &[(&str, &str, &str)] = &[(
    "skills/cas-supervisor-checklist.md",
    "codex",
    "Codex ships a deliberately renamed variant, skills/cas-codex-supervisor-checklist.md, \
     whose 'Session Start (No Hooks)' adaptation exists because Codex has no SessionStart \
     hook banner (cas-59ee). Whole-file divergence is sanctioned, so it is compared to \
     nothing rather than to the claude checklist. The grok twin IS held identical to \
     claude, and is guarded normally by this test plus \
     test_grok_supervisor_checklist_is_not_the_no_hooks_variant in builtins.rs.",
)];

/// Files that exist only in a twin flavor: (flavor, flavor-relative path, rationale).
const ALLOWED_FLAVOR_ONLY: &[(&str, &str, &str)] = &[
    (
        "codex",
        "agents/factory-supervisor.md",
        "Codex-only supervisor agent. Claude and grok drive supervision through the \
         cas-supervisor skill instead of a dedicated agent file, so there is no twin \
         to hold it to.",
    ),
    (
        "codex",
        "skills/cas-codex-supervisor-checklist.md",
        "The renamed No-Hooks checklist variant; see the ALLOWED_MISSING_TWIN entry \
         for skills/cas-supervisor-checklist.md.",
    ),
];

// ---------------------------------------------------------------------------
// Canonicalization of mechanical per-harness spellings
// ---------------------------------------------------------------------------

const CANON_TOOL: &str = "<CAS_TOOL_PREFIX>";
const CANON_CFG: &str = "<HARNESS_CFG_DIR>";
const CANON_CATALOG: &str = "<AGENT_CATALOG_CONST>";
const CANON_HETERO: &str = "## Heterogeneous Teams (<FLAVOR_MIX>)";

/// Rewrite the sanctioned per-harness spellings to flavor-neutral tokens.
///
/// Every rule here is mechanical: the same sentence, spelled the way a given
/// harness must spell it. Rules are applied longest-match-first where one
/// pattern is a substring of another (`mcp__cas__` before `cas__`,
/// `CODEX_BUILTIN_AGENTS` before `BUILTIN_AGENTS`) so a shorter rule cannot
/// corrupt a longer match. Rules are applied to all flavors regardless of which
/// one the content came from, which makes canonicalization idempotent and means
/// a file that leaks another harness's spelling still normalizes to the same
/// text (the dedicated prefix guardrails in factory_codex_skill_guardrails.rs
/// are what catch leaked spellings; that is deliberately not this test's job).
fn canonicalize(content: &str) -> String {
    let mut out = content.to_string();

    // CAS tool prefix. Longest first: `mcp__cas__` and `mcp__cs__` both end in
    // a string containing `cas__`/`cs__`.
    for pat in ["mcp__cas__", "mcp__cs__", "cas__"] {
        out = out.replace(pat, CANON_TOOL);
    }

    // Agent-catalog constant name. Longest first.
    for pat in ["CODEX_BUILTIN_AGENTS", "GROK_BUILTIN_AGENTS", "BUILTIN_AGENTS"] {
        out = out.replace(pat, CANON_CATALOG);
    }

    // Per-harness config/skill directory.
    for pat in [".claude/", ".codex/", ".grok/"] {
        out = out.replace(pat, CANON_CFG);
    }

    // The Heterogeneous Teams heading names the supervisor/worker mix, which is
    // necessarily per-flavor ("Claude supervisor + Codex workers" vs "Grok
    // supervisor + Claude/Codex workers"). Canonicalize the heading line only —
    // the section body stays under guard.
    out = out
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("## Heterogeneous Teams") {
                CANON_HETERO
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    out
}

// ---------------------------------------------------------------------------
// Section splitting
// ---------------------------------------------------------------------------

const PREAMBLE: &str = "<preamble/frontmatter>";

/// Collapse blank-line-delimited prose blocks into single logical lines so that
/// pure line-wrapping differences are not mistaken for content differences.
///
/// This is necessary because canonicalized tokens have different lengths in
/// different flavors: `CODEX_BUILTIN_AGENTS` is six characters longer than
/// `BUILTIN_AGENTS`, so a hand-wrapped paragraph mentioning it reflows around
/// the substitution. The words are identical; only the line breaks moved.
///
/// Every word still participates in the comparison — a dropped sentence, a
/// removed bullet or a changed action name all change the joined text — so this
/// widens tolerance for layout without weakening the content guard. Fenced code
/// blocks are preserved verbatim, since line structure is meaningful there.
fn reflow(body: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut in_fence = false;

    for line in body {
        let trimmed = line.trim();
        let is_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");

        if is_fence || in_fence {
            if !paragraph.is_empty() {
                out.push(paragraph.join(" "));
                paragraph.clear();
            }
            if is_fence {
                in_fence = !in_fence;
            }
            out.push(line.trim_end().to_string());
            continue;
        }

        if trimmed.is_empty() {
            if !paragraph.is_empty() {
                out.push(paragraph.join(" "));
                paragraph.clear();
            }
            continue;
        }

        // Normalize runs of internal whitespace too, so a double space or a
        // tab/space swap doesn't register as drift.
        paragraph.push(trimmed.split_whitespace().collect::<Vec<_>>().join(" "));
    }

    if !paragraph.is_empty() {
        out.push(paragraph.join(" "));
    }
    out
}

struct Section {
    heading: String,
    body: Vec<String>,
}

fn is_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('#')
        && trimmed
            .trim_start_matches('#')
            .starts_with(|c: char| c == ' ' || c == '\t')
}

/// Split into sections keyed by heading line. Content before the first heading
/// (YAML frontmatter, intro prose) becomes the PREAMBLE section.
fn split_sections(content: &str) -> Vec<Section> {
    let mut sections = vec![Section { heading: PREAMBLE.to_string(), body: Vec::new() }];
    for line in content.lines() {
        if is_heading(line) {
            sections.push(Section { heading: line.trim_end().to_string(), body: Vec::new() });
        } else {
            sections
                .last_mut()
                .expect("sections always has the preamble")
                .body
                .push(line.trim_end().to_string());
        }
    }
    // Drop an empty preamble so a file starting immediately with a heading
    // doesn't report a phantom section.
    if sections[0].body.iter().all(|l| l.is_empty()) {
        sections.remove(0);
    }
    sections
}

// ---------------------------------------------------------------------------
// Diff rendering
// ---------------------------------------------------------------------------

/// Trim the common prefix/suffix and render the differing middle from both
/// sides, capped so a large divergence stays readable.
fn render_diff(claude: &[String], twin: &[String], claude_label: &str, twin_label: &str) -> String {
    const MAX: usize = 12;

    let mut start = 0;
    while start < claude.len() && start < twin.len() && claude[start] == twin[start] {
        start += 1;
    }
    let mut back = 0;
    while back < claude.len() - start && back < twin.len() - start
        && claude[claude.len() - 1 - back] == twin[twin.len() - 1 - back]
    {
        back += 1;
    }

    let c_mid = &claude[start..claude.len() - back];
    let t_mid = &twin[start..twin.len() - back];

    let mut out = String::new();
    out.push_str(&format!(
        "\n    (first divergence at line {} of the section)\n",
        start + 1
    ));
    for (label, lines, marker) in [(claude_label, c_mid, '-'), (twin_label, t_mid, '+')] {
        if lines.is_empty() {
            out.push_str(&format!("    {label}: (nothing here)\n"));
            continue;
        }
        out.push_str(&format!("    {label}:\n"));
        for line in lines.iter().take(MAX) {
            out.push_str(&format!("      {marker} {line}\n"));
        }
        if lines.len() > MAX {
            out.push_str(&format!("      ... {} more line(s)\n", lines.len() - MAX));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cas-cli must live under repo root")
        .to_path_buf()
}

fn builtins_root() -> PathBuf {
    repo_root().join("cas-cli/src/builtins")
}

/// All `.md` files under `dir`, returned as paths relative to `dir`.
/// `skip_top_level` names immediate subdirectories to exclude (the twin trees).
fn markdown_files(dir: &Path, skip_top_level: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let is_skipped = path
                    .strip_prefix(dir)
                    .ok()
                    .and_then(|rel| rel.to_str())
                    .is_some_and(|rel| skip_top_level.contains(&rel));
                if !is_skipped {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if let Ok(rel) = path.strip_prefix(dir) {
                if let Some(rel) = rel.to_str() {
                    found.push(rel.replace('\\', "/"));
                }
            }
        }
    }
    found.sort();
    found
}

fn flavor_path(rel: &str, flavor: &Flavor) -> PathBuf {
    if flavor.subdir.is_empty() {
        builtins_root().join(rel)
    } else {
        builtins_root().join(flavor.subdir).join(rel)
    }
}

fn section_is_allowed(rel: &str, flavor: &str, heading: &str) -> bool {
    ALLOWED_SECTION_DIVERGENCE
        .iter()
        .any(|(p, f, h, _)| *p == rel && *f == flavor && *h == heading)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The guard: normalized three-way content comparison across all flavor triples.
#[test]
fn builtin_flavors_stay_content_identical_after_normalization() {
    let root = builtins_root();
    let claude_files = markdown_files(&root, &[CODEX.subdir, GROK.subdir]);

    assert!(
        claude_files.len() > 40,
        "expected the claude builtin corpus to be discovered (found {}); \
         the walk or the builtins path is wrong",
        claude_files.len()
    );

    let mut failures: Vec<String> = Vec::new();
    let mut compared = 0usize;

    for rel in &claude_files {
        let claude_raw = fs::read_to_string(flavor_path(rel, &CLAUDE))
            .unwrap_or_else(|e| panic!("failed to read claude {rel}: {e}"));
        let claude_sections = split_sections(&canonicalize(&claude_raw));

        for twin in TWINS {
            let twin_path = flavor_path(rel, twin);

            if !twin_path.exists() {
                let exempt = ALLOWED_MISSING_TWIN
                    .iter()
                    .any(|(p, f, _)| *p == rel && *f == twin.name);
                if !exempt {
                    failures.push(format!(
                        "MISSING TWIN: {rel} exists for claude but not for {}.\n    \
                         Port it, or add an ALLOWED_MISSING_TWIN entry with a rationale.",
                        twin.name
                    ));
                }
                continue;
            }

            let twin_raw = fs::read_to_string(&twin_path)
                .unwrap_or_else(|e| panic!("failed to read {} {rel}: {e}", twin.name));
            let twin_sections = split_sections(&canonicalize(&twin_raw));
            compared += 1;

            // Compare the heading sequence first — a wholly missing or added
            // section is the most common drift shape (it is exactly how the
            // "Valid Actions" gap and the task-verifier's absent Epic
            // Verification section presented).
            let claude_headings: Vec<&str> = claude_sections
                .iter()
                .map(|s| s.heading.as_str())
                .filter(|h| !section_is_allowed(rel, twin.name, h))
                .collect();
            let twin_headings: Vec<&str> = twin_sections
                .iter()
                .map(|s| s.heading.as_str())
                .filter(|h| !section_is_allowed(rel, twin.name, h))
                .collect();

            if claude_headings != twin_headings {
                let only_claude: Vec<&&str> =
                    claude_headings.iter().filter(|h| !twin_headings.contains(h)).collect();
                let only_twin: Vec<&&str> =
                    twin_headings.iter().filter(|h| !claude_headings.contains(h)).collect();
                failures.push(format!(
                    "SECTION SET DIFFERS: {rel} (claude vs {})\n    \
                     only in claude: {:?}\n    only in {}: {:?}\n    \
                     (if the order changed but the set matches, the sections were reordered \
                     in one flavor only)",
                    twin.name, only_claude, twin.name, only_twin
                ));
                continue;
            }

            // Heading sequences match: compare bodies pairwise.
            for (c_sec, t_sec) in claude_sections.iter().zip(twin_sections.iter()) {
                if section_is_allowed(rel, twin.name, &c_sec.heading) {
                    continue;
                }
                let c_body = reflow(&c_sec.body);
                let t_body = reflow(&t_sec.body);
                if c_body != t_body {
                    failures.push(format!(
                        "CONTENT DRIFT: {rel} (claude vs {}) in section {:?}{}",
                        twin.name,
                        c_sec.heading,
                        render_diff(&c_body, &t_body, "claude", twin.name)
                    ));
                }
            }
        }
    }

    assert!(
        compared > 80,
        "expected to compare well over 80 flavor pairs, only compared {compared}"
    );

    assert!(
        failures.is_empty(),
        "\n\nBuiltin flavor drift detected ({} issue(s)) across {} compared pairs.\n\
         The claude/codex/grok flavors must stay content-identical apart from the \
         mechanical per-harness spellings normalized by this test.\n\n{}\n\n\
         Fix by porting the change to the other flavors (pure prefix substitution from \
         the claude flavor is the established method). If the divergence is genuinely \
         intentional, add an ALLOWED_SECTION_DIVERGENCE entry naming the file, flavor, \
         section and rationale — see the module docs in this file.\n",
        failures.len(),
        compared,
        failures.join("\n\n")
    );
}

/// cas-c7c2: memory lifecycle guidance is intentionally a reference file so
/// the always-loaded skill body stays compact. Keep that file's decision table
/// present and normalized across Claude, Codex, and Grok rather than relying on
/// the broad corpus walk alone to make its contract visible.
#[test]
fn memory_lifecycle_reference_stays_three_way_synchronized() {
    const REL: &str = "skills/cas-memory-management/references/lifecycle-and-storage.md";
    let claude = canonicalize(
        &fs::read_to_string(flavor_path(REL, &CLAUDE)).expect("claude memory lifecycle reference"),
    );

    for required in [
        "recent_at desc, id desc",
        "valid_until",
        "| Need | Use | Why |",
        "**Memory**",
        "**Task**",
        "**Knowledge**",
        "**Spec / ADR**",
    ] {
        assert!(
            claude.contains(required),
            "memory lifecycle reference missing required marker: {required:?}"
        );
    }

    for twin in TWINS {
        let twin_content = canonicalize(
            &fs::read_to_string(flavor_path(REL, twin))
                .unwrap_or_else(|e| panic!("{} memory lifecycle reference: {e}", twin.name)),
        );
        assert_eq!(
            twin_content, claude,
            "memory lifecycle reference drifted between claude and {}",
            twin.name
        );
    }
}

/// cas-462a: one-shot CLI routing is a cross-harness operational contract.
/// Keep the compact body and detailed routing reference explicitly guarded,
/// in addition to the broad corpus walk above.
#[test]
fn cli_routing_skill_stays_three_way_synchronized() {
    for rel in [
        "skills/cli-routing/SKILL.md",
        "skills/cli-routing/references/routing.md",
    ] {
        let claude = canonicalize(
            &fs::read_to_string(flavor_path(rel, &CLAUDE))
                .unwrap_or_else(|e| panic!("claude {rel} missing: {e}")),
        );
        for required in [
            "codex exec",
            "pippenz@gmail.com",
            "unapproved account",
            "CLAUDE_CONFIG_DIR",
            "docs/SLACK_POSTING_RUNBOOK.md",
        ] {
            assert!(
                claude.contains(required),
                "claude {rel} missing {required:?}"
            );
        }
        assert!(
            !claude.contains("daniel@petrastella.io"),
            "claude {rel} retains the stale Daniel-only account gate"
        );
        for twin in TWINS {
            let body = canonicalize(
                &fs::read_to_string(flavor_path(rel, twin))
                    .unwrap_or_else(|e| panic!("{} {rel} missing: {e}", twin.name)),
            );
            assert_eq!(claude, body, "{rel} drifted for {}", twin.name);
        }
    }
}

/// A file present only in a twin flavor must be an explicitly sanctioned
/// flavor-only file. Without this, drift could hide by adding a codex-only or
/// grok-only document that the claude-rooted walk never visits.
#[test]
fn flavor_only_builtin_files_are_explicitly_sanctioned() {
    let root = builtins_root();
    let claude_files = markdown_files(&root, &[CODEX.subdir, GROK.subdir]);

    let mut unexpected = Vec::new();
    for twin in TWINS {
        let twin_root = root.join(twin.subdir);
        for rel in markdown_files(&twin_root, &[]) {
            if claude_files.contains(&rel) {
                continue;
            }
            let sanctioned = ALLOWED_FLAVOR_ONLY
                .iter()
                .any(|(f, p, _)| *f == twin.name && *p == rel);
            if !sanctioned {
                unexpected.push(format!("{}/{rel}", twin.name));
            }
        }
    }

    assert!(
        unexpected.is_empty(),
        "\n\nFlavor-only builtin file(s) with no claude counterpart and no exemption:\n  {}\n\n\
         Either add the claude (and other-flavor) twin, or add an ALLOWED_FLAVOR_ONLY entry \
         explaining why this file is intentionally single-flavor.\n",
        unexpected.join("\n  ")
    );
}

/// Guard the guard: every exemption must point at a file that actually exists,
/// so entries cannot silently outlive the divergence they were written for and
/// quietly widen coverage gaps.
#[test]
fn drift_guard_exemptions_are_live() {
    let mut stale = Vec::new();

    for (rel, flavor, heading, _) in ALLOWED_SECTION_DIVERGENCE {
        if !flavor_path(rel, &CLAUDE).exists() {
            stale.push(format!(
                "ALLOWED_SECTION_DIVERGENCE names missing claude file {rel}"
            ));
            continue;
        }
        let twin = TWINS
            .iter()
            .find(|f| f.name == *flavor)
            .unwrap_or_else(|| panic!("unknown flavor {flavor} in ALLOWED_SECTION_DIVERGENCE"));
        let Ok(content) = fs::read_to_string(flavor_path(rel, twin)) else {
            stale.push(format!(
                "ALLOWED_SECTION_DIVERGENCE names missing {flavor} file {rel}"
            ));
            continue;
        };
        // The heading is stored canonicalized; compare against canonicalized content.
        let has_heading = split_sections(&canonicalize(&content))
            .iter()
            .any(|s| s.heading == *heading);
        let claude_has_heading = fs::read_to_string(flavor_path(rel, &CLAUDE))
            .map(|c| split_sections(&canonicalize(&c)).iter().any(|s| s.heading == *heading))
            .unwrap_or(false);
        if !has_heading && !claude_has_heading {
            stale.push(format!(
                "ALLOWED_SECTION_DIVERGENCE entry for {rel} ({flavor}) names section {heading:?}, \
                 which no longer exists in either flavor — remove the entry"
            ));
        }
    }

    for (rel, flavor, _) in ALLOWED_MISSING_TWIN {
        if !flavor_path(rel, &CLAUDE).exists() {
            stale.push(format!("ALLOWED_MISSING_TWIN names missing claude file {rel}"));
            continue;
        }
        let twin = TWINS
            .iter()
            .find(|f| f.name == *flavor)
            .unwrap_or_else(|| panic!("unknown flavor {flavor} in ALLOWED_MISSING_TWIN"));
        if flavor_path(rel, twin).exists() {
            stale.push(format!(
                "ALLOWED_MISSING_TWIN says {flavor} has no {rel}, but the file now exists — \
                 remove the entry so the twin is compared"
            ));
        }
    }

    for (flavor, rel, _) in ALLOWED_FLAVOR_ONLY {
        let twin = TWINS
            .iter()
            .find(|f| f.name == *flavor)
            .unwrap_or_else(|| panic!("unknown flavor {flavor} in ALLOWED_FLAVOR_ONLY"));
        if !flavor_path(rel, twin).exists() {
            stale.push(format!(
                "ALLOWED_FLAVOR_ONLY names missing file {flavor}/{rel} — remove the entry"
            ));
        }
    }

    assert!(
        stale.is_empty(),
        "\n\nStale drift-guard exemption(s):\n  {}\n",
        stale.join("\n  ")
    );
}

/// The normalization rules must be idempotent and must not collapse text that
/// carries meaning. Guards against a future rule that over-normalizes and
/// silently blinds the comparison.
#[test]
fn canonicalization_is_idempotent_and_prefix_safe() {
    let claude = "Call `mcp__cas__task action=show` and see `.claude/skills/x` in `BUILTIN_AGENTS`.";
    let codex = "Call `mcp__cs__task action=show` and see `.codex/skills/x` in `CODEX_BUILTIN_AGENTS`.";
    let grok = "Call `cas__task action=show` and see `.grok/skills/x` in `GROK_BUILTIN_AGENTS`.";

    let c = canonicalize(claude);
    assert_eq!(c, canonicalize(codex), "codex spelling must canonicalize to the claude form");
    assert_eq!(c, canonicalize(grok), "grok spelling must canonicalize to the claude form");
    assert_eq!(c, canonicalize(&c), "canonicalization must be idempotent");

    // The longest-first ordering must not let `cas__` corrupt `mcp__cas__`.
    assert!(
        !c.contains("mcp__") && !c.contains("cas__"),
        "no raw tool prefix should survive canonicalization: {c}"
    );
    assert_eq!(
        c.matches(CANON_TOOL).count(),
        1,
        "a single tool reference must produce exactly one canonical token: {c}"
    );

    // Substantive text must survive untouched.
    assert!(
        c.contains("action=show"),
        "canonicalization must not eat surrounding content: {c}"
    );
    assert_ne!(
        canonicalize("reject the close reason"),
        canonicalize("accept the close reason"),
        "canonicalization must not collapse genuinely different prose"
    );
}

/// The section splitter must key on markdown headings and keep bodies intact —
/// the drift comparison is only as granular as this.
#[test]
fn section_splitting_keys_on_markdown_headings() {
    let doc = "---\nname: x\n---\n\nintro line\n\n## Alpha\na1\na2\n\n### Beta\nb1\n";
    let sections = split_sections(doc);

    let headings: Vec<&str> = sections.iter().map(|s| s.heading.as_str()).collect();
    assert_eq!(headings, vec![PREAMBLE, "## Alpha", "### Beta"]);
    assert!(sections[0].body.contains(&"intro line".to_string()));
    assert!(sections[1].body.contains(&"a1".to_string()));
    assert!(sections[2].body.contains(&"b1".to_string()));

    // A '#' that is not a heading (no space after the hashes) must not split.
    assert_eq!(split_sections("## Real\ntext\n#hashtag\nmore\n").len(), 1);
}

/// Reflow must absorb line-wrapping differences without absorbing content
/// differences. This rule exists because canonicalized tokens differ in length
/// between flavors and reflow the prose around them; if it ever over-collapsed,
/// the whole guard would quietly stop catching drift.
#[test]
fn reflow_tolerates_rewrapping_but_not_content_change() {
    let wrapped_a: Vec<String> = "This file remains in `X` solely so `cas sync` can overwrite any\nstale downstream copies. It will be removed\nin a future release."
        .lines()
        .map(str::to_string)
        .collect();
    let wrapped_b: Vec<String> = "This file remains in `X` solely so `cas sync` can overwrite\nany stale downstream copies. It will be\nremoved in a future release."
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        reflow(&wrapped_a),
        reflow(&wrapped_b),
        "identical prose wrapped differently must compare equal"
    );

    // A dropped sentence must still be caught.
    let shortened: Vec<String> = "This file remains in `X` solely so `cas sync` can overwrite any\nstale downstream copies."
        .lines()
        .map(str::to_string)
        .collect();
    assert_ne!(reflow(&wrapped_a), reflow(&shortened), "dropped content must be detected");

    // A dropped bullet must still be caught.
    let three: Vec<String> = vec!["- a".into(), "- b".into(), "- c".into()];
    let two: Vec<String> = vec!["- a".into(), "- b".into()];
    assert_ne!(reflow(&three), reflow(&two), "a removed bullet must be detected");

    // Line structure inside fenced code blocks must be preserved verbatim.
    let fenced: Vec<String> = vec!["```bash".into(), "cmd one".into(), "cmd two".into(), "```".into()];
    let joined: Vec<String> = vec!["```bash".into(), "cmd one cmd two".into(), "```".into()];
    assert_ne!(
        reflow(&fenced),
        reflow(&joined),
        "code-fence line structure is meaningful and must not be collapsed"
    );

    // Paragraph separation must not let text migrate between paragraphs unnoticed.
    let two_paras: Vec<String> = vec!["alpha".into(), String::new(), "beta".into()];
    let one_para: Vec<String> = vec!["alpha".into(), "beta".into()];
    assert_ne!(
        reflow(&two_paras),
        reflow(&one_para),
        "a paragraph break carries structure and must not be normalized away"
    );
}

/// The guard must actually fail on injected drift — a comparison test that can
/// only pass is worthless. Exercises the three shapes it is meant to catch.
#[test]
fn guard_detects_injected_drift() {
    let claude = "# Doc\n\n## Alpha\nshared line\nclaude-only detail\n\n## Beta\nb\n";

    // 1. A changed line inside a shared section.
    let changed = "# Doc\n\n## Alpha\nshared line\ncodex-only detail\n\n## Beta\nb\n";
    let c_sections = split_sections(&canonicalize(claude));
    let t_sections = split_sections(&canonicalize(changed));
    assert!(
        c_sections.iter().zip(t_sections.iter()).any(|(a, b)| a.body != b.body),
        "body drift must be detected"
    );

    // 2. A wholly missing section (the "Valid Actions" / Epic Verification shape).
    let dropped = "# Doc\n\n## Alpha\nshared line\nclaude-only detail\n";
    let d_headings: Vec<String> = split_sections(&canonicalize(dropped))
        .iter()
        .map(|s| s.heading.clone())
        .collect();
    let c_headings: Vec<String> = c_sections.iter().map(|s| s.heading.clone()).collect();
    assert_ne!(c_headings, d_headings, "a missing section must be detected");

    // 3. Drift that hides behind a legitimate prefix difference: same tool
    //    prefix spelling, different action — must still fail.
    let a = canonicalize("run `mcp__cas__task action=close`");
    let b = canonicalize("run `mcp__cs__task action=reopen`");
    assert_ne!(a, b, "prefix normalization must not mask a changed action name");
}
