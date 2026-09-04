//! Flavor drift guard (cas-703a).
//!
//! The builtin skills/agents ship in four harness flavors that are meant to be
//! the SAME document under a small set of mechanical per-harness spellings:
//!
//!   claude  cas-cli/src/builtins/<path>          tools `mcp__cas__*`
//!   codex   cas-cli/src/builtins/codex/<path>    tools `mcp__cs__*`
//!   grok    cas-cli/src/builtins/grok/<path>     tools `cas__*`
//!   opencode process-local projection            tools `cas_*`
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
use std::process::Command;

use cas::builtins::{
    BUILTIN_AGENTS, BUILTIN_SKILLS, agent_catalog_for_harness, skill_catalog_for_harness,
};
use cas_mux::SupervisorCli;
use serde_json::Value;
use tempfile::TempDir;

#[path = "support/builtin_catalog.rs"]
mod builtin_catalog;

// ---------------------------------------------------------------------------
// Flavors
// ---------------------------------------------------------------------------

struct Flavor {
    /// Human label used in assertion messages.
    name: &'static str,
    /// Subdirectory under `cas-cli/src/builtins` ("" for the claude baseline).
    subdir: &'static str,
}

const CLAUDE: Flavor = Flavor {
    name: "claude",
    subdir: "",
};
const CODEX: Flavor = Flavor {
    name: "codex",
    subdir: "codex",
};
const GROK: Flavor = Flavor {
    name: "grok",
    subdir: "grok",
};

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
const ALLOWED_SECTION_DIVERGENCE: &[(&str, &str, &str, &str)] = &[
    (
        "skills/cas-worker/references/recovery.md",
        "codex",
        "## Close requires task-scoped verification",
        "The claude body enumerates the per-CLI tool spellings as an audience-facing \
     list ('Claude workers: ... / Codex workers: ...') because a claude worker may \
     be reading on behalf of either. A codex worker has exactly one spelling, so \
     the list collapses to a single inline sentence. Both were written in the same \
     commit; this is presentation, not content.",
    ),
    (
        "skills/cas-worker/references/recovery.md",
        "grok",
        "## Close requires task-scoped verification",
        "Same rationale as the codex entry above: the per-CLI enumeration collapses to \
     one line for a single-spelling harness.",
    ),
];

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
    for pat in [
        "CODEX_BUILTIN_AGENTS",
        "GROK_BUILTIN_AGENTS",
        "BUILTIN_AGENTS",
    ] {
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

/// OpenCode receives a process-local projection rather than a filesystem
/// mirror. Normalize its server-sanitized `cas_<tool>` calls to the same token
/// used by the three source trees before comparing content.
fn canonicalize_opencode(content: &str) -> String {
    canonicalize(content).replace("cas_", CANON_TOOL)
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
    let mut sections = vec![Section {
        heading: PREAMBLE.to_string(),
        body: Vec::new(),
    }];
    for line in content.lines() {
        if is_heading(line) {
            sections.push(Section {
                heading: line.trim_end().to_string(),
                body: Vec::new(),
            });
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
    while back < claude.len() - start
        && back < twin.len() - start
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

/// Non-markdown builtin payloads that ship alongside the skill bodies. These
/// are mirrored per flavor exactly like the `.md` files, but until cas-ef87a
/// the walk below hard-filtered `extension == "md"`, so
/// `skills/cas-wizard/template.sh` sat six lines short in both twins
/// (the whole `# Example:` block was missing) without the guard noticing.
const ASSET_EXTENSIONS: &[&str] = &["sh", "js", "yaml", "yml"];

/// These legacy skill bodies remain flat in the source tree while their
/// catalog destination uses the conventional `<skill>/SKILL.md` path.
fn source_relative(catalog_path: &str) -> String {
    match catalog_path {
        "skills/cas-search/SKILL.md"
        | "skills/cas-task-tracking/SKILL.md"
        | "skills/cas-supervisor/SKILL.md"
        | "skills/cas-supervisor-checklist/SKILL.md"
        | "skills/cas-codex-supervisor-checklist/SKILL.md"
        | "skills/cas-worker/SKILL.md" => {
            let stem = catalog_path
                .strip_prefix("skills/")
                .and_then(|path| path.strip_suffix("/SKILL.md"))
                .expect("flat legacy skill path");
            format!("skills/{stem}.md")
        }
        _ => catalog_path.to_string(),
    }
}

/// All files under `dir` whose extension is in `extensions`, returned as paths
/// relative to `dir`. `skip_top_level` names immediate subdirectories to
/// exclude (the twin trees).
fn files_with_extensions(dir: &Path, skip_top_level: &[&str], extensions: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    let flavor = catalog_flavor(dir);
    for builtin in builtin_catalog::skills(flavor)
        .iter()
        .chain(builtin_catalog::agents(flavor))
    {
        let Some(extension) = builtin.path.rsplit('.').next() else {
            continue;
        };
        if extensions.contains(&extension) {
            found.push(source_relative(builtin.path));
        }
    }
    found.sort();
    found
}

/// All `.md` files under `dir`, returned as paths relative to `dir`.
/// `skip_top_level` names immediate subdirectories to exclude (the twin trees).
fn markdown_files(dir: &Path, skip_top_level: &[&str]) -> Vec<String> {
    files_with_extensions(dir, skip_top_level, &["md"])
}

fn builtins_root() -> PathBuf {
    PathBuf::from("cas-cli/src/builtins")
}

fn checkout_root() -> Option<PathBuf> {
    let root = cas::test_paths::workspace_root();
    if !root.join("cas-cli/src/builtins").is_dir() {
        eprintln!(
            "SKIP root projection checks: source checkout is absent at {}",
            root.display()
        );
        return None;
    }
    Some(root)
}

fn catalog_flavor(dir: &Path) -> builtin_catalog::Flavor {
    match dir.to_string_lossy().as_ref() {
        path if path.contains("cas-cli/src/builtins/codex") => builtin_catalog::Flavor::Codex,
        path if path.contains("cas-cli/src/builtins/grok") => builtin_catalog::Flavor::Grok,
        _ => builtin_catalog::Flavor::Claude,
    }
}

fn catalog_for(flavor: &Flavor) -> builtin_catalog::Flavor {
    match flavor.name {
        "claude" => builtin_catalog::Flavor::Claude,
        "codex" => builtin_catalog::Flavor::Codex,
        "grok" => builtin_catalog::Flavor::Grok,
        other => panic!("unknown builtin flavor {other}"),
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

const CODEMAP_SKILL_REL: &str = "skills/codemap/SKILL.md";

/// Return the missing semantic requirements for the codemap knowledge-build
/// workflow. This intentionally checks behavior (a Rust-enforced <=90-second
/// command, portable status capture, continuation, and explicit
/// prohibitions) rather than requiring one exact prose spelling.
fn codemap_build_contract_violations(content: &str) -> Vec<&'static str> {
    let lower = content.to_ascii_lowercase();
    let mut violations = Vec::new();

    let Some(build_line) = lower
        .lines()
        .find(|line| line.contains("cas knowledge build"))
    else {
        return vec!["knowledge-build command"];
    };

    let build_tokens: Vec<&str> = build_line
        .split(|character: char| character.is_whitespace() || matches!(character, ';' | '`'))
        .filter(|token| !token.is_empty())
        .collect();
    let Some(cas_index) = build_tokens.iter().position(|token| *token == "cas") else {
        violations.push("knowledge-build command");
        return violations;
    };
    let command_tokens = &build_tokens[cas_index..];

    if build_tokens[..cas_index]
        .iter()
        .any(|token| matches!(*token, "timeout" | "/usr/bin/timeout" | "gtimeout"))
    {
        violations.push("portable Rust timeout");
    }

    let mut bound_seconds = None;
    for (index, token) in command_tokens.iter().enumerate() {
        if *token == "--timeout-secs" {
            bound_seconds = command_tokens
                .get(index + 1)
                .and_then(|value| value.parse().ok());
        } else if let Some(value) = token.strip_prefix("--timeout-secs=") {
            bound_seconds = value.parse().ok();
        }
    }
    match bound_seconds {
        Some(seconds) if (1..=90).contains(&seconds) => {}
        Some(_) => violations.push("<=90-second Rust timeout bound"),
        None => violations.push("--timeout-secs bound"),
    }
    if !build_line.contains("--max-sources 5") {
        violations.push("max-sources limit");
    }
    if !lower.contains("wall-clock")
        || !(lower.contains("complete build")
            || lower.contains("entire build")
            || lower.contains("whole build"))
    {
        violations.push("single wall-clock deadline for the complete build");
    }
    if !lower.contains("stops later completions")
        && !lower.contains("stop later completions")
        && !lower.contains("no later completions")
    {
        violations.push("stop scheduling after deadline exhaustion");
    }
    if !lower.contains("process group")
        || !(lower.contains("terminate") || lower.contains("kill"))
        || !lower.contains("reap")
    {
        violations.push("terminate and reap the active provider group");
    }
    if build_tokens[cas_index..].iter().any(|token| *token == "&")
        || build_line.trim_end().ends_with('&')
        || build_line.contains("nohup")
        || build_line.contains("setsid")
        || build_line.contains("disown")
    {
        violations.push("no detached/background command");
    }

    let has_negated_directive = |terms: &[&str]| {
        lower.lines().any(|line| {
            let negated = ["do not", "never", "must not", "prohibit", "forbid"]
                .iter()
                .any(|marker| line.contains(marker));
            negated && terms.iter().all(|term| line.contains(term))
        })
    };

    if !(lower.contains("non-zero") || lower.contains("nonzero"))
        || !(lower.contains("non-blocking") || lower.contains("must not block"))
        || !lower.contains("continue")
    {
        violations.push("non-zero failure is non-blocking and continues");
    }
    if !(lower.contains("record") || lower.contains("capture"))
        || !(lower.contains("durable receipt") || lower.contains("task notes"))
        || !(lower.contains("exit status") || lower.contains("exit code") || lower.contains("$?"))
    {
        violations.push("durable exit receipt");
    }
    if lower.contains("set +e") || lower.contains("set -e") {
        violations.push("no caller-shell errexit mutation");
    }
    if !lower.contains("if cas knowledge build")
        || !lower.contains("else")
        || !lower.contains("build_exit_status=$?")
    {
        violations.push("failure-tolerant status capture");
    }
    if !has_negated_directive(&["detach", "background", "build"])
        || !has_negated_directive(&["poll"])
        || !has_negated_directive(&["wait", "90-second"])
    {
        violations.push("explicit no-detach/no-poll/no-unbounded-wait directives");
    }

    violations
}

/// The codemap skill must make knowledge distillation best-effort without
/// allowing a model/upstream stall to hold the commit and status proof.
#[test]
fn codemap_build_contract_is_bounded_non_blocking_and_non_detached() {
    for flavor in [&CLAUDE, &CODEX, &GROK] {
        let rel = CODEMAP_SKILL_REL;
        let content = builtin_catalog::find(catalog_for(flavor), rel);
        let violations = codemap_build_contract_violations(&content);
        assert!(
            violations.is_empty(),
            "{} {rel} violates codemap build contract: {violations:?}",
            flavor.name
        );
    }
}

/// The semantic guard must reject the portability and lifecycle regressions that motivated it even
/// when all three flavors would otherwise remain textually synchronized.
#[test]
fn codemap_build_contract_rejects_portability_and_lifecycle_variants() {
    let valid = r#"
```bash
if cas knowledge build --timeout-secs 90 --max-sources 5; then
  build_exit_status=0
else
  build_exit_status=$?
fi
```

If the command returns a non-zero exit status, record the durable receipt in task notes and continue with the CODEMAP commit and cas codemap status proof; this is non-blocking. Rust enforces one 90-second wall-clock deadline across the complete build, stops later completions after exhaustion, and terminates/reaps the active provider process group so a stalled build leaves no ordinary orphan descendant.
Do not detach or background the build, run a manual polling loop, or wait beyond the 90-second bound.
"#;
    assert!(
        codemap_build_contract_violations(valid).is_empty(),
        "the checker must accept equivalent valid contract prose"
    );

    let unbounded = valid.replace("--timeout-secs 90", "");
    assert!(
        codemap_build_contract_violations(&unbounded).contains(&"--timeout-secs bound"),
        "an unbounded knowledge build must be rejected"
    );

    let per_completion_only = valid.replace(
        "one 90-second wall-clock deadline across the complete build",
        "a 90-second wall-clock deadline for each provider completion",
    );
    assert!(
        codemap_build_contract_violations(&per_completion_only)
            .contains(&"single wall-clock deadline for the complete build"),
        "a per-completion-only bound must not satisfy the whole-build contract"
    );

    let detached = valid.replace(
        "cas knowledge build --timeout-secs 90 --max-sources 5",
        "cas knowledge build --timeout-secs 90 --max-sources 5 &",
    );
    assert!(
        codemap_build_contract_violations(&detached).contains(&"no detached/background command"),
        "a detached/background knowledge build must be rejected"
    );

    let gnu_timeout = valid.replace(
        "if cas knowledge build",
        "if timeout 90s cas knowledge build",
    );
    assert!(
        codemap_build_contract_violations(&gnu_timeout).contains(&"portable Rust timeout"),
        "a GNU timeout wrapper must be rejected"
    );

    let shell_mutation = valid.replace(
        "if cas knowledge build --timeout-secs 90 --max-sources 5; then",
        "set +e\ncas knowledge build --timeout-secs 90 --max-sources 5\nset -e\nif true; then",
    );
    assert!(
        codemap_build_contract_violations(&shell_mutation)
            .contains(&"no caller-shell errexit mutation"),
        "status capture must not mutate the caller shell's errexit mode"
    );
}

/// The guard: normalized three-way content comparison across all filesystem
/// flavor triples. OpenCode's process-local catalog is checked below.
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
        let claude_raw = builtin_catalog::find(builtin_catalog::Flavor::Claude, rel);
        let claude_sections = split_sections(&canonicalize(&claude_raw));

        for twin in TWINS {
            let Some(twin_raw) = builtin_catalog::try_find(catalog_for(twin), rel) else {
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
            };
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
                let only_claude: Vec<&&str> = claude_headings
                    .iter()
                    .filter(|h| !twin_headings.contains(h))
                    .collect();
                let only_twin: Vec<&&str> = twin_headings
                    .iter()
                    .filter(|h| !claude_headings.contains(h))
                    .collect();
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

/// The same guard for the non-markdown payloads (`.sh`, `.js`, `.yaml`).
///
/// These have no markdown section structure, so the comparison is whole-file
/// after the same canonicalization the markdown guard uses — `schema.yaml`
/// legitimately spells the tool prefix per harness (`mcp__cas__` / `mcp__cs__`
/// / `cas__`), and nothing else in these files may differ.
#[test]
fn non_markdown_builtin_twins_stay_identical_after_normalization() {
    let root = builtins_root();
    let claude_files = files_with_extensions(&root, &[CODEX.subdir, GROK.subdir], ASSET_EXTENSIONS);

    assert!(
        !claude_files.is_empty(),
        "expected the claude builtin corpus to contain non-markdown payloads; \
         the walk or the builtins path is wrong"
    );

    let mut failures: Vec<String> = Vec::new();
    let mut compared = 0usize;

    for rel in &claude_files {
        let claude_raw = builtin_catalog::find(builtin_catalog::Flavor::Claude, rel);
        let claude_body = canonicalize(&claude_raw);

        for twin in TWINS {
            let Some(twin_raw) = builtin_catalog::try_find(catalog_for(twin), rel) else {
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
            };
            let twin_body = canonicalize(&twin_raw);
            compared += 1;

            if claude_body != twin_body {
                let claude_lines: Vec<String> = claude_body.lines().map(str::to_string).collect();
                let twin_lines: Vec<String> = twin_body.lines().map(str::to_string).collect();
                failures.push(format!(
                    "CONTENT DRIFT: {rel} (claude vs {}){}",
                    twin.name,
                    render_diff(&claude_lines, &twin_lines, "claude", twin.name)
                ));
            }
        }
    }

    assert!(
        compared >= 2 * claude_files.len(),
        "expected to compare both twins for every non-markdown payload \
         ({} files), only compared {compared} pairs",
        claude_files.len()
    );

    assert!(
        failures.is_empty(),
        "\n\nBuiltin non-markdown flavor drift detected ({} issue(s)) across {} compared \
         pairs.\nScripts, palettes and schemas shipped with a skill are mirrored exactly \
         like its prose; only the per-harness tool prefix may differ.\n\n{}\n",
        failures.len(),
        compared,
        failures.join("\n\n")
    );
}

/// Flavor-only non-markdown payloads must be sanctioned too, for the same
/// reason as their markdown counterparts: an extra twin-only script is drift
/// the claude-rooted walk would otherwise never visit.
#[test]
fn flavor_only_non_markdown_builtin_files_are_explicitly_sanctioned() {
    let root = builtins_root();
    let claude_files = files_with_extensions(&root, &[CODEX.subdir, GROK.subdir], ASSET_EXTENSIONS);

    let mut unexpected = Vec::new();
    for twin in TWINS {
        let twin_root = root.join(twin.subdir);
        for rel in files_with_extensions(&twin_root, &[], ASSET_EXTENSIONS) {
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
        "\n\nFlavor-only non-markdown builtin file(s) with no claude counterpart and no \
         exemption:\n  {}\n\nEither add the claude (and other-flavor) twin, or add an \
         ALLOWED_FLAVOR_ONLY entry explaining why this file is intentionally \
         single-flavor.\n",
        unexpected.join("\n  ")
    );
}

/// The files checked into the cas-src root are the committed projections that
/// keep the authoring checkout clean after `cas update`. Skills are deliberately
/// different: the operator directive makes every project skill a generated,
/// ignored projection, including database-backed `cas-*` skills with no
/// embedded source such as `cas-seo-expert`.
#[test]
fn root_managed_projections_stay_synced_and_project_skills_stay_ignored() {
    let Some(root) = checkout_root() else {
        return;
    };

    for builtin in BUILTIN_AGENTS {
        let path = root.join(".claude").join(builtin.path);
        let actual = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("missing root Claude projection {}: {error}", path.display())
        });
        assert_eq!(
            actual,
            builtin.content,
            "root Claude projection {} diverged from its embedded template",
            path.display()
        );
    }
    for builtin in cas::builtins::CODEX_BUILTIN_AGENTS {
        let path = root.join(".codex").join(builtin.path);
        let actual = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("missing root Codex projection {}: {error}", path.display())
        });
        assert_eq!(
            actual,
            builtin.content,
            "root Codex projection {} diverged from its embedded template",
            path.display()
        );
    }

    // Generate the current project-level settings and CLAUDE.md block in an
    // isolated project. Keeping this behavioral avoids duplicating the large
    // hook JSON and the managed block's prose in the drift test itself.
    // `cas init` registers this project fixture; resolve its parent at runtime
    // because archive-mode tests must not depend on the source checkout path.
    let fixture_parent = std::env::current_dir().expect("test current directory");
    let fixture = TempDir::new_in(fixture_parent).expect("temporary projection fixture");
    // Keep the project beneath its isolated HOME so ancestor-based managed
    // document detection stops at the fixture HOME before reaching this
    // checkout's own CLAUDE.md.
    let home = fixture.path().join("home");
    let project = home.join("project");
    let xdg = home.join("xdg");
    fs::create_dir_all(project.join(".claude")).expect("create fixture Claude dir");
    fs::create_dir_all(&home).expect("create fixture home");
    fs::create_dir_all(&xdg).expect("create fixture XDG dir");
    let init = Command::new(cas::test_paths::cas_binary())
        .current_dir(&project)
        .args(["--json", "init", "--yes", "--no-integrations"])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("CAS_SKIP_FACTORY_TOOLING", "1")
        .env("CAS_ROOT", project.join(".cas"))
        .env_remove("CAS_CLOUD_TOKEN")
        .env_remove("CAS_FACTORY_MODE")
        .env_remove("CAS_FACTORY_SESSION")
        .output()
        .expect("run source-built cas initializer");
    assert!(
        init.status.success(),
        "source-built cas init failed (status {:?}): {}",
        init.status,
        String::from_utf8_lossy(&init.stderr)
    );

    let generated_settings: Value = serde_json::from_str(
        &fs::read_to_string(project.join(".claude/settings.json"))
            .expect("generated settings.json"),
    )
    .expect("generated settings JSON");
    let root_settings: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".claude/settings.json")).expect("root settings.json"),
    )
    .expect("root settings JSON");

    for key in ["hooks", "statusLine"] {
        assert_eq!(
            root_settings.get(key),
            generated_settings.get(key),
            "root .claude/settings.json managed key {key:?} diverged"
        );
    }
    let generated_permissions = generated_settings
        .pointer("/permissions/allow")
        .and_then(Value::as_array)
        .expect("generated Cassy permission list");
    let root_permissions = root_settings
        .pointer("/permissions/allow")
        .and_then(Value::as_array)
        .expect("root Cassy permission list");
    for permission in generated_permissions {
        assert!(
            root_permissions.contains(permission),
            "root .claude/settings.json is missing managed permission {permission}"
        );
    }
    assert_eq!(
        root_settings.get("enableArtifact"),
        Some(&Value::Bool(false)),
        "root .claude/settings.json must retain the managed enableArtifact=false key"
    );

    let root_claude = fs::read_to_string(root.join("CLAUDE.md")).expect("root CLAUDE.md");
    let generated_claude =
        fs::read_to_string(project.join("CLAUDE.md")).expect("generated CLAUDE.md");
    fn managed_block(content: &str) -> &str {
        let begin = content
            .find("<!-- CAS:BEGIN")
            .expect("CLAUDE.md managed block begin marker");
        let end_marker = "<!-- CAS:END -->";
        let end = content[begin..]
            .find(end_marker)
            .map(|offset| begin + offset + end_marker.len())
            .expect("CLAUDE.md managed block end marker");
        &content[begin..end]
    }
    assert_eq!(
        managed_block(&root_claude),
        managed_block(&generated_claude),
        "root CLAUDE.md managed block diverged from the generated template"
    );

    let gitignore = fs::read_to_string(root.join(".gitignore")).expect("root .gitignore");
    assert!(
        gitignore
            .lines()
            .any(|line| line.trim() == "/.claude/skills/*"),
        ".gitignore must ignore the complete project Claude skills tree"
    );
    assert!(
        !gitignore
            .lines()
            .any(|line| line.trim_start().starts_with("!") && line.contains(".claude/skills/")),
        ".gitignore must not re-include a project Claude skill"
    );
    for path in [
        ".claude/skills/cas/SKILL.md",
        ".claude/skills/cas-servers/SKILL.md",
        ".claude/skills/cas-seo-expert/SKILL.md",
    ] {
        let check = Command::new("git")
            .current_dir(&root)
            .args(["check-ignore", "--no-index", "--", path])
            .output()
            .expect("run git check-ignore");
        assert!(
            check.status.success(),
            "project skill path {path} is not covered by .gitignore: {}",
            String::from_utf8_lossy(&check.stdout)
        );
    }
    let tracked_skills = Command::new("git")
        .current_dir(&root)
        .args(["ls-files", "--", ".claude/skills"])
        .output()
        .expect("list tracked project skills");
    assert!(
        tracked_skills.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&tracked_skills.stderr)
    );
    assert!(
        String::from_utf8_lossy(&tracked_skills.stdout)
            .trim()
            .is_empty(),
        "Cassy project skills must not remain tracked: {}",
        String::from_utf8_lossy(&tracked_skills.stdout)
    );
}

/// The fourth flavor is generated in-process, not written under a user-level
/// `.opencode` tree. Compare every projected catalog entry to the Claude
/// source after normalizing OpenCode's `cas_<tool>` sanitizer spelling.
#[test]
fn opencode_projection_stays_content_identical_after_normalization() {
    let skills = skill_catalog_for_harness(SupervisorCli::OpenCode);
    let agents = agent_catalog_for_harness(SupervisorCli::OpenCode);
    let mut compared = 0usize;
    let mut failures = Vec::new();

    for (kind, catalog) in [("skill", BUILTIN_SKILLS), ("agent", BUILTIN_AGENTS)] {
        for builtin in catalog {
            let rel = builtin.path;
            let source = builtin_catalog::find(builtin_catalog::Flavor::Claude, rel);
            let projection = if kind == "skill" {
                skills.iter().find(|candidate| candidate.path == rel)
            } else {
                agents.iter().find(|candidate| candidate.path == rel)
            };
            let Some(projection) = projection else {
                failures.push(format!("MISSING OPENCODE {kind} PROJECTION: {rel}"));
                continue;
            };
            compared += 1;
            let source_sections = split_sections(&canonicalize_opencode(&source));
            let projection_sections = split_sections(&canonicalize_opencode(projection.content));
            if source_sections.len() != projection_sections.len() {
                failures.push(format!("SECTION SET DIFFERS: {rel} (claude vs opencode)"));
                continue;
            }
            for (source_section, projection_section) in
                source_sections.iter().zip(projection_sections.iter())
            {
                if source_section.heading != projection_section.heading
                    || reflow(&source_section.body) != reflow(&projection_section.body)
                {
                    failures.push(format!(
                        "CONTENT DRIFT: {rel} (claude vs opencode) in section {:?}",
                        source_section.heading
                    ));
                }
            }
        }
    }

    assert!(
        compared > 80,
        "expected over 80 OpenCode projections, got {compared}"
    );
    assert!(
        failures.is_empty(),
        "OpenCode builtin projection drifted ({} issue(s)):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// cas-c7c2: memory lifecycle guidance is intentionally a reference file so
/// the always-loaded skill body stays compact. Keep that file's decision table
/// present and normalized across Claude, Codex, and Grok rather than relying on
/// the broad corpus walk alone to make its contract visible.
#[test]
fn memory_lifecycle_reference_stays_three_way_synchronized() {
    const REL: &str = "skills/cas-memory-management/references/lifecycle-and-storage.md";
    let claude = canonicalize(builtin_catalog::find(builtin_catalog::Flavor::Claude, REL));

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
        let twin_content = canonicalize(builtin_catalog::find(catalog_for(twin), REL));
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
        let claude = canonicalize(builtin_catalog::find(builtin_catalog::Flavor::Claude, rel));
        for required in [
            "codex exec",
            "release.claude_account_allowlist",
            "unapproved account",
            "CLAUDE_CONFIG_DIR",
        ] {
            assert!(
                claude.contains(required),
                "claude {rel} missing {required:?}"
            );
        }
        // cas-37f6: operator policy lives in config and the project rubric,
        // never in the shipped skill text.
        for banned in [
            "@gmail.com",
            "@petrastella.io",
            "docs/SLACK_POSTING_RUNBOOK.md",
        ] {
            assert!(
                !claude.contains(banned),
                "claude {rel} ships operator-specific text: {banned:?}"
            );
        }
        for twin in TWINS {
            let body = canonicalize(builtin_catalog::find(catalog_for(twin), rel));
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
        if builtin_catalog::try_find(builtin_catalog::Flavor::Claude, rel).is_none() {
            stale.push(format!(
                "ALLOWED_SECTION_DIVERGENCE names missing claude file {rel}"
            ));
            continue;
        }
        let twin = TWINS
            .iter()
            .find(|f| f.name == *flavor)
            .unwrap_or_else(|| panic!("unknown flavor {flavor} in ALLOWED_SECTION_DIVERGENCE"));
        let Some(content) = builtin_catalog::try_find(catalog_for(twin), rel) else {
            stale.push(format!(
                "ALLOWED_SECTION_DIVERGENCE names missing {flavor} file {rel}"
            ));
            continue;
        };
        // The heading is stored canonicalized; compare against canonicalized content.
        let has_heading = split_sections(&canonicalize(content))
            .iter()
            .any(|s| s.heading == *heading);
        let claude_has_heading = builtin_catalog::try_find(builtin_catalog::Flavor::Claude, rel)
            .map(|c| {
                split_sections(&canonicalize(c))
                    .iter()
                    .any(|s| s.heading == *heading)
            })
            .unwrap_or(false);
        if !has_heading && !claude_has_heading {
            stale.push(format!(
                "ALLOWED_SECTION_DIVERGENCE entry for {rel} ({flavor}) names section {heading:?}, \
                 which no longer exists in either flavor — remove the entry"
            ));
        }
    }

    for (rel, flavor, _) in ALLOWED_MISSING_TWIN {
        if builtin_catalog::try_find(builtin_catalog::Flavor::Claude, rel).is_none() {
            stale.push(format!(
                "ALLOWED_MISSING_TWIN names missing claude file {rel}"
            ));
            continue;
        }
        let twin = TWINS
            .iter()
            .find(|f| f.name == *flavor)
            .unwrap_or_else(|| panic!("unknown flavor {flavor} in ALLOWED_MISSING_TWIN"));
        if builtin_catalog::try_find(catalog_for(twin), rel).is_some() {
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
        if builtin_catalog::try_find(catalog_for(twin), rel).is_none() {
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
    let claude =
        "Call `mcp__cas__task action=show` and see `.claude/skills/x` in `BUILTIN_AGENTS`.";
    let codex =
        "Call `mcp__cs__task action=show` and see `.codex/skills/x` in `CODEX_BUILTIN_AGENTS`.";
    let grok = "Call `cas__task action=show` and see `.grok/skills/x` in `GROK_BUILTIN_AGENTS`.";

    let c = canonicalize(claude);
    assert_eq!(
        c,
        canonicalize(codex),
        "codex spelling must canonicalize to the claude form"
    );
    assert_eq!(
        c,
        canonicalize(grok),
        "grok spelling must canonicalize to the claude form"
    );
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
    let shortened: Vec<String> =
        "This file remains in `X` solely so `cas sync` can overwrite any\nstale downstream copies."
            .lines()
            .map(str::to_string)
            .collect();
    assert_ne!(
        reflow(&wrapped_a),
        reflow(&shortened),
        "dropped content must be detected"
    );

    // A dropped bullet must still be caught.
    let three: Vec<String> = vec!["- a".into(), "- b".into(), "- c".into()];
    let two: Vec<String> = vec!["- a".into(), "- b".into()];
    assert_ne!(
        reflow(&three),
        reflow(&two),
        "a removed bullet must be detected"
    );

    // Line structure inside fenced code blocks must be preserved verbatim.
    let fenced: Vec<String> = vec![
        "```bash".into(),
        "cmd one".into(),
        "cmd two".into(),
        "```".into(),
    ];
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
        c_sections
            .iter()
            .zip(t_sections.iter())
            .any(|(a, b)| a.body != b.body),
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
    assert_ne!(
        a, b,
        "prefix normalization must not mask a changed action name"
    );
}
