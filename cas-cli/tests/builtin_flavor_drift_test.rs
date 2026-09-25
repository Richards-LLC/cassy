//! Catalog drift guard (cas-703a, redesigned for audit D1 in cas-a638).
//!
//! Builtin skill and agent text is prefix-neutral: it names Cassy tools by bare
//! name (`task`, `coordination`, …), and the role guidance states each
//! harness's prefix once (`cas::builtins::TOOL_NAMING_LINE`). One canonical
//! file therefore serves every harness:
//!
//!   claude   cas-cli/src/builtins/<path>          the canonical catalog
//!   codex    CODEX_BUILTIN_*   embed the canonical files
//!   grok     GROK_BUILTIN_*    embed the canonical files
//!   opencode process-local projection of the canonical catalog
//!
//! Per-harness differences are kept out of shared text (pstack D1 note):
//!   - `TAILORED` lists entries whose content is the canonical text after
//!     exact replacements. Today that is only the Grok task-verifier, whose
//!     `tools:` frontmatter builtins.rs generates around the one shared body.
//!   - `ALLOWED_FLAVOR_ONLY` lists files with no canonical counterpart, which
//!     are the only files left under `builtins/codex/`: the Codex no-hooks
//!     checklist and two `agents/openai.yaml` policies.
//!
//! This test asserts:
//!   1. every other twin-catalog entry is byte-identical to the canonical one;
//!   2. every tailored entry equals its canonical text after exactly its
//!      listed replacements, and each replacement still applies;
//!   3. every file under `builtins/{codex,grok}/` is sanctioned, so the twin
//!      trees cannot grow back;
//!   4. no catalog spells a harness prefix (`mcp__cas__`, `mcp__cs__`,
//!      `cas__`) outside the single naming line and generated agent `tools:`
//!      frontmatter (`cas::builtins::unsanctioned_prefixed_tool_lines`);
//!   5. the OpenCode projection differs from the canonical catalog only in
//!      agent `tools:` allowlists.
//!
//! HOW TO RESOLVE A FAILURE:
//!   - A twin catalog entry differs from the canonical file: point its
//!     `include_str!` at the canonical file and delete the twin.
//!   - A tool name carries a prefix: use the bare name; the role guidance
//!     states each harness's prefix.
//!   - A per-harness difference is genuinely needed: generate it in
//!     builtins.rs and add a `TAILORED` entry with a rationale, or add an
//!     `ALLOWED_FLAVOR_ONLY` file.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use cas::builtins::{
    BUILTIN_AGENTS, BUILTIN_SKILLS, TOOL_NAMING_LINE, agent_catalog_for_harness,
    skill_catalog_for_harness, unsanctioned_prefixed_tool_lines,
};
use cas_mux::SupervisorCli;
use serde_json::Value;
use tempfile::TempDir;

#[path = "support/builtin_catalog.rs"]
mod builtin_catalog;

use builtin_catalog::Flavor;

/// Twin catalogs compared against the canonical (Claude) catalog.
const TWINS: [(Flavor, &str); 2] = [(Flavor::Codex, "codex"), (Flavor::Grok, "grok")];

// ---------------------------------------------------------------------------
// Sanctioned per-harness files
// ---------------------------------------------------------------------------

/// A per-harness variant of a canonical entry: (flavor, catalog path, source
/// file under `builtins/` if the variant has its own file, replacements
/// applied to the canonical text, rationale). The variant must equal the
/// canonical text after exactly these replacements. Prefer generated
/// frontmatter over a source file.
struct Tailored {
    flavor: &'static str,
    path: &'static str,
    source: Option<&'static str>,
    replacements: &'static [(&'static str, &'static str)],
    rationale: &'static str,
}

const TAILORED: &[Tailored] = &[Tailored {
    flavor: "grok",
    path: "agents/task-verifier.md",
    source: None,
    replacements: &[("mcp__cas__", "cas__")],
    rationale: "An agent `tools:` allowlist must spell the harness's own tool names. The \
                body is shared (`agents/task-verifier.body.md`); builtins.rs generates \
                the frontmatter per harness.",
}];

/// Canonical files a twin catalog deliberately omits: (catalog path, flavor, rationale).
const ALLOWED_MISSING_TWIN: &[(&str, &str, &str)] = &[(
    "skills/cas-supervisor-checklist/SKILL.md",
    "codex",
    "Codex ships the renamed no-hooks variant skills/cas-codex-supervisor-checklist instead \
     (cas-59ee): Codex has no SessionStart hook banner.",
), (
    "agents/task-verifier.md",
    "codex",
    "Audit D6 (cas-6b97): Codex custom agents are TOML with developer_instructions, and \
     Codex ignores .codex/agents/*.md, so Cassy installs no .md agents for Codex.",
)];

/// Files only a twin catalog ships: (flavor, catalog path, source path, rationale).
const ALLOWED_FLAVOR_ONLY: &[(&str, &str, &str, &str)] = &[
    (
        "codex",
        "skills/cas-codex-supervisor-checklist/SKILL.md",
        "codex/skills/cas-codex-supervisor-checklist.md",
        "The renamed no-hooks checklist; see ALLOWED_MISSING_TWIN.",
    ),
    (
        "codex",
        "skills/cas-nuxt-playwright/agents/openai.yaml",
        "codex/skills/cas-nuxt-playwright/agents/openai.yaml",
        "Codex's implicit-invocation policy; only Codex reads agents/openai.yaml.",
    ),
    (
        "codex",
        "skills/cas-to-questionnaire/agents/openai.yaml",
        "codex/skills/cas-to-questionnaire/agents/openai.yaml",
        "Codex's implicit-invocation policy; only Codex reads agents/openai.yaml.",
    ),
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn canonical(path: &str) -> Option<&'static str> {
    builtin_catalog::try_find(Flavor::Claude, path)
}

fn tailored(flavor: &str, path: &str) -> Option<&'static Tailored> {
    TAILORED
        .iter()
        .find(|entry| entry.flavor == flavor && entry.path == path)
}

fn flavor_only(flavor: &str, path: &str) -> bool {
    ALLOWED_FLAVOR_ONLY
        .iter()
        .any(|(f, p, _, _)| *f == flavor && *p == path)
}

/// Apply a tailoring's replacements, reporting any that no longer match.
fn apply_tailoring(canonical: &str, entry: &Tailored) -> Result<String, String> {
    let mut out = canonical.to_string();
    for (from, to) in entry.replacements {
        if !out.contains(from) {
            return Err(format!(
                "TAILORED {} {}: replacement source {from:?} no longer occurs in the canonical \
                 file — update or remove the entry",
                entry.flavor, entry.path
            ));
        }
        out = out.replace(from, to);
    }
    Ok(out)
}

/// The first differing line of two texts, for a readable failure.
fn first_difference(expected: &str, actual: &str) -> String {
    for (index, (e, a)) in expected.lines().zip(actual.lines()).enumerate() {
        if e != a {
            return format!("line {}:\n      - {e}\n      + {a}", index + 1);
        }
    }
    format!(
        "length differs ({} vs {} lines)",
        expected.lines().count(),
        actual.lines().count()
    )
}

/// Catalog failures for one twin catalog. Separated from the test so the guard
/// can be exercised against injected drift.
fn twin_catalog_failures(
    flavor_name: &str,
    entries: &[(&'static str, &'static str)],
    canonical_of: &dyn Fn(&str) -> Option<&'static str>,
) -> Vec<String> {
    let mut failures = Vec::new();
    for &(path, content) in entries {
        if flavor_only(flavor_name, path) {
            continue;
        }
        let Some(source) = canonical_of(path) else {
            failures.push(format!(
                "UNSANCTIONED FLAVOR-ONLY: {flavor_name} ships {path} with no canonical file; \
                 add it to the canonical catalog or to ALLOWED_FLAVOR_ONLY"
            ));
            continue;
        };
        match tailored(flavor_name, path) {
            Some(entry) => match apply_tailoring(source, entry) {
                Ok(expected) if expected == content => {}
                Ok(expected) => failures.push(format!(
                    "TAILORED DRIFT: {flavor_name} {path} is more than its listed replacements \
                     away from the canonical file; {}",
                    first_difference(&expected, content)
                )),
                Err(stale) => failures.push(stale),
            },
            None if source == content => {}
            None => failures.push(format!(
                "TWIN DRIFT: {flavor_name} {path} differs from the canonical file; embed the \
                 canonical file instead; {}",
                first_difference(source, content)
            )),
        }
    }
    failures
}

fn twin_entries(flavor: Flavor) -> Vec<(&'static str, &'static str)> {
    builtin_catalog::skills(flavor)
        .iter()
        .chain(builtin_catalog::agents(flavor))
        .map(|builtin| (builtin.path, builtin.content))
        .collect()
}

fn checkout_root() -> Option<PathBuf> {
    let root = cas::test_paths::workspace_root();
    if !root.join("cas-cli/src/builtins").is_dir() {
        eprintln!(
            "SKIP checkout checks: source checkout is absent at {}",
            root.display()
        );
        return None;
    }
    Some(root)
}

// ---------------------------------------------------------------------------
// Catalog tests
// ---------------------------------------------------------------------------

/// Guard 1 and 2: twin catalogs embed the canonical files; tailored twins are
/// exactly their listed replacements away from them.
#[test]
fn twin_catalogs_embed_the_canonical_catalog() {
    let mut failures = Vec::new();
    let mut identical = 0usize;
    for (flavor, name) in TWINS {
        let entries = twin_entries(flavor);
        failures.extend(twin_catalog_failures(name, &entries, &canonical));
        identical += entries
            .iter()
            .filter(|(path, content)| canonical(path) == Some(*content))
            .count();

        for builtin in BUILTIN_SKILLS.iter().chain(BUILTIN_AGENTS) {
            let present = entries.iter().any(|(path, _)| *path == builtin.path);
            let exempt = ALLOWED_MISSING_TWIN
                .iter()
                .any(|(p, f, _)| *p == builtin.path && *f == name);
            if !present && !exempt {
                failures.push(format!(
                    "MISSING: {name} catalog lacks {}; register the canonical file or add an \
                     ALLOWED_MISSING_TWIN entry",
                    builtin.path
                ));
            }
            if present && exempt {
                failures.push(format!(
                    "STALE EXEMPTION: ALLOWED_MISSING_TWIN says {name} omits {}, but it ships",
                    builtin.path
                ));
            }
        }
    }
    assert!(
        identical > 2 * 130,
        "expected both twin catalogs to embed well over 130 canonical files each; got {identical}"
    );
    assert!(
        failures.is_empty(),
        "\n\nBuiltin catalog drift ({} issue(s)):\n\n{}\n",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Guard 3: the per-harness source trees hold only tailored and flavor-only
/// files, and every exemption names a real catalog entry.
#[test]
fn twin_source_trees_hold_only_sanctioned_files() {
    for entry in TAILORED {
        let (flavor, _) = TWINS
            .iter()
            .find(|(_, name)| *name == entry.flavor)
            .unwrap_or_else(|| panic!("unknown TAILORED flavor {}", entry.flavor));
        assert!(
            builtin_catalog::try_find(*flavor, entry.path).is_some(),
            "TAILORED names {} {} but the catalog does not ship it",
            entry.flavor,
            entry.path
        );
        assert!(!entry.rationale.is_empty() && !entry.replacements.is_empty());
    }
    for (flavor_name, path, _, rationale) in ALLOWED_FLAVOR_ONLY {
        let (flavor, _) = TWINS
            .iter()
            .find(|(_, name)| name == flavor_name)
            .unwrap_or_else(|| panic!("unknown ALLOWED_FLAVOR_ONLY flavor {flavor_name}"));
        assert!(
            builtin_catalog::try_find(*flavor, path).is_some(),
            "ALLOWED_FLAVOR_ONLY names {flavor_name} {path} but the catalog does not ship it"
        );
        assert!(
            canonical(path).is_none(),
            "ALLOWED_FLAVOR_ONLY {flavor_name} {path} now has a canonical file; tailor or collapse it"
        );
        assert!(!rationale.is_empty());
    }

    let Some(root) = checkout_root() else {
        return;
    };
    let builtins = root.join("cas-cli/src/builtins");
    let mut sanctioned: Vec<String> = TAILORED
        .iter()
        .filter_map(|t| t.source.map(str::to_string))
        .collect();
    sanctioned.extend(ALLOWED_FLAVOR_ONLY.iter().map(|(_, _, source, _)| source.to_string()));
    let mut on_disk = Vec::new();
    for (_, name) in TWINS {
        for entry in walkdir::WalkDir::new(builtins.join(name))
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let relative = entry
                .path()
                .strip_prefix(&builtins)
                .expect("walked path is under builtins")
                .to_string_lossy()
                .replace('\\', "/");
            on_disk.push(relative);
        }
    }
    let unexpected: Vec<&String> = on_disk.iter().filter(|p| !sanctioned.contains(p)).collect();
    assert!(
        unexpected.is_empty(),
        "per-harness source files outside TAILORED/ALLOWED_FLAVOR_ONLY: {unexpected:?} — \
         twins of canonical files are embedded from the canonical tree, not copied"
    );
    for source in &sanctioned {
        assert!(
            on_disk.contains(source),
            "sanctioned per-harness source {source} is missing from the checkout"
        );
    }
    assert!(
        on_disk.len() <= 3,
        "the per-harness source trees must stay small; found {} files",
        on_disk.len()
    );
}

/// Guard 4: no catalog spells a harness prefix outside the naming rule, and
/// both role files carry the naming line.
#[test]
fn every_catalog_names_tools_by_bare_name() {
    let mut failures = Vec::new();
    for harness in [
        SupervisorCli::Claude,
        SupervisorCli::Codex,
        SupervisorCli::Grok,
        SupervisorCli::OpenCode,
    ] {
        for builtin in skill_catalog_for_harness(harness)
            .iter()
            .chain(agent_catalog_for_harness(harness))
        {
            for (line, text) in unsanctioned_prefixed_tool_lines(builtin.content) {
                failures.push(format!("{harness:?} {}:{line}: {text}", builtin.path));
            }
        }
        let skills = skill_catalog_for_harness(harness);
        for role in ["skills/cas-worker/SKILL.md", "skills/cas-supervisor/SKILL.md"] {
            let body = skills
                .iter()
                .find(|b| b.path == role)
                .unwrap_or_else(|| panic!("{harness:?} catalog lacks {role}"));
            assert_eq!(
                body.content.matches(TOOL_NAMING_LINE).count(),
                1,
                "{harness:?} {role} must state the per-harness prefix exactly once"
            );
        }
    }
    assert!(
        failures.is_empty(),
        "shipped text spells a harness tool prefix outside the naming line (use the bare \
         name):\n  {}",
        failures.join("\n  ")
    );
}

/// Guard 5: the OpenCode projection is the canonical catalog with only agent
/// `tools:` allowlists respelled.
#[test]
fn opencode_projection_is_the_canonical_catalog() {
    let mut compared = 0usize;
    for (catalog, projected) in [
        (BUILTIN_SKILLS, skill_catalog_for_harness(SupervisorCli::OpenCode)),
        (BUILTIN_AGENTS, agent_catalog_for_harness(SupervisorCli::OpenCode)),
    ] {
        assert_eq!(catalog.len(), projected.len());
        for builtin in catalog {
            let projection = projected
                .iter()
                .find(|candidate| candidate.path == builtin.path)
                .unwrap_or_else(|| panic!("missing OpenCode projection {}", builtin.path));
            let expected: Vec<String> = builtin
                .content
                .split('\n')
                .map(|line| {
                    if line.starts_with("tools:") {
                        line.replace("mcp__cas__", "cas_")
                    } else {
                        line.to_string()
                    }
                })
                .collect();
            assert_eq!(
                projection.content,
                expected.join("\n"),
                "OpenCode {} drifted",
                builtin.path
            );
            compared += 1;
        }
    }
    assert!(compared > 130, "expected over 130 OpenCode projections, got {compared}");
}

/// The guard must fail on injected drift: an edited twin, a tailored twin
/// with an extra change, a stale tailoring, and an unsanctioned twin-only file.
#[test]
fn guard_detects_injected_drift() {
    let canonical_of = |path: &str| -> Option<&'static str> {
        match path {
            "skills/x/SKILL.md" => Some("# X\ncall `task action=close`\n"),
            "agents/task-verifier.md" => Some("---\ntools: Read, mcp__cas__task\n---\nbody\n"),
            "agents/stale.md" => Some("---\ntools: Read\n---\nbody\n"),
            _ => None,
        }
    };

    let edited = twin_catalog_failures(
        "codex",
        &[("skills/x/SKILL.md", "# X\ncall `task action=reopen`\n")],
        &canonical_of,
    );
    assert!(edited.iter().any(|f| f.starts_with("TWIN DRIFT")), "{edited:?}");

    let identical = twin_catalog_failures(
        "codex",
        &[("skills/x/SKILL.md", "# X\ncall `task action=close`\n")],
        &canonical_of,
    );
    assert!(identical.is_empty(), "{identical:?}");

    let tailored_ok = twin_catalog_failures(
        "grok",
        &[("agents/task-verifier.md", "---\ntools: Read, cas__task\n---\nbody\n")],
        &canonical_of,
    );
    assert!(tailored_ok.is_empty(), "{tailored_ok:?}");

    let tailored_extra = twin_catalog_failures(
        "grok",
        &[("agents/task-verifier.md", "---\ntools: Read, cas__task\n---\nother body\n")],
        &canonical_of,
    );
    assert!(
        tailored_extra.iter().any(|f| f.starts_with("TAILORED DRIFT")),
        "{tailored_extra:?}"
    );

    // A tailoring whose replacement no longer applies is stale.
    let stale_entry = Tailored {
        flavor: "grok",
        path: "agents/stale.md",
        source: None,
        replacements: &[("mcp__cas__", "cas__")],
        rationale: "fixture",
    };
    let stale = apply_tailoring(canonical_of("agents/stale.md").unwrap(), &stale_entry);
    assert!(
        stale.as_ref().is_err_and(|e| e.contains("no longer occurs")),
        "{stale:?}"
    );

    let orphan = twin_catalog_failures("grok", &[("skills/y/SKILL.md", "y")], &canonical_of);
    assert!(
        orphan.iter().any(|f| f.starts_with("UNSANCTIONED FLAVOR-ONLY")),
        "{orphan:?}"
    );

    // The naming rule itself.
    assert!(unsanctioned_prefixed_tool_lines("run `mcp__cas__task action=show`").len() == 1);
    assert!(unsanctioned_prefixed_tool_lines("run `mcp__cs__task action=show`").len() == 1);
    assert!(unsanctioned_prefixed_tool_lines("run `cas__task action=show`").len() == 1);
    assert!(unsanctioned_prefixed_tool_lines("run `task action=show`").is_empty());
    assert!(unsanctioned_prefixed_tool_lines(TOOL_NAMING_LINE).is_empty());
    // pstack denylist: naming the harness on the line does not excuse a prefix.
    assert!(unsanctioned_prefixed_tool_lines("In Claude Code, call `mcp__cas__task`.").len() == 1);
    assert!(unsanctioned_prefixed_tool_lines("run `ToolSearch(select:mcp__cas__task)`").len() == 1);
    assert!(
        unsanctioned_prefixed_tool_lines("tools: Read, mcp__cas__task").is_empty()
    );
}

/// cas-c7c2: memory lifecycle guidance is a reference file so the always-loaded
/// skill body stays compact. Keep its decision table present in every catalog.
#[test]
fn memory_lifecycle_reference_is_shared_by_every_harness() {
    const REL: &str = "skills/cas-memory-management/references/lifecycle-and-storage.md";
    let claude = builtin_catalog::find(Flavor::Claude, REL);
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
    for (flavor, name) in TWINS {
        assert_eq!(builtin_catalog::find(flavor, REL), claude, "{name}");
    }
}

/// cas-462a: one-shot CLI routing is a cross-harness operational contract.
#[test]
fn cli_routing_skill_is_shared_by_every_harness() {
    for rel in [
        "skills/cli-routing/SKILL.md",
        "skills/cli-routing/references/routing.md",
    ] {
        let claude = builtin_catalog::find(Flavor::Claude, rel);
        for required in [
            "codex exec",
            "release.claude_account_allowlist",
            "unapproved account",
            "CLAUDE_CONFIG_DIR",
        ] {
            assert!(claude.contains(required), "{rel} missing {required:?}");
        }
        // cas-37f6: operator policy lives in config and the project rubric,
        // never in the shipped skill text.
        for banned in [
            "@gmail.com",
            "@petrastella.io",
            "docs/SLACK_POSTING_RUNBOOK.md",
        ] {
            assert!(!claude.contains(banned), "{rel} ships operator-specific text: {banned:?}");
        }
        for (flavor, name) in TWINS {
            assert_eq!(builtin_catalog::find(flavor, rel), claude, "{rel} drifted for {name}");
        }
    }
}

// ---------------------------------------------------------------------------
// Codemap build contract
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
    for (flavor, name) in builtin_catalog::FLAVORS {
        let rel = CODEMAP_SKILL_REL;
        let content = builtin_catalog::find(*flavor, rel);
        let violations = codemap_build_contract_violations(content);
        assert!(
            violations.is_empty(),
            "{name} {rel} violates codemap build contract: {violations:?}"
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


// ---------------------------------------------------------------------------
// Root projections and bundled resources
// ---------------------------------------------------------------------------

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
    // Audit D6: Codex installs no .md agents, so the root projection has none.
    let stray_codex_agents: Vec<String> = fs::read_dir(root.join(".codex").join("agents"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(".md"))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        stray_codex_agents.is_empty(),
        "root .codex/agents still carries .md agents: {stray_codex_agents:?}"
    );
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
    // Audit D3 (cas-6930d): AGENTS.md carries the harness-neutral directive
    // and CLAUDE.md imports it; the root copy is the repository's own
    // projection of the same template.
    let root_agents = fs::read_to_string(root.join("AGENTS.md")).expect("root AGENTS.md");
    let generated_agents =
        fs::read_to_string(project.join("AGENTS.md")).expect("generated AGENTS.md");
    assert_eq!(
        managed_block(&root_agents),
        managed_block(&generated_agents),
        "root AGENTS.md managed block diverged from the generated template"
    );
    assert!(
        root_claude.lines().any(|line| line == "@AGENTS.md"),
        "root CLAUDE.md must import AGENTS.md"
    );
    for (name, content) in [("AGENTS.md", &root_agents), ("CLAUDE.md", &root_claude)] {
        assert!(
            content.chars().count() < 10_000,
            "root {name} must stay under Grok's 10,000-character cap"
        );
    }

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


/// Release-report templates and Python helpers must be installed with the skill,
/// including assets outside the markdown corpus.
#[test]
fn release_report_bundle_is_complete_for_every_harness() {
    let files = [
        "SKILL.md",
        "references/brief-template.md",
        "references/default-tokens.json",
        "references/exemplar.md",
        "references/pdf.md",
        "references/template.html",
        "scripts/render.py",
    ];
    let opencode = skill_catalog_for_harness(SupervisorCli::OpenCode);
    for file in files {
        let path = format!("skills/cas-release-report/{file}");
        let source = builtin_catalog::find(Flavor::Claude, &path);
        assert!(!source.is_empty(), "empty report resource: {path}");
        for (flavor, name) in TWINS {
            assert_eq!(builtin_catalog::find(flavor, &path), source, "{name} {path}");
        }
        let projected = opencode
            .iter()
            .find(|entry| entry.path == path)
            .unwrap_or_else(|| panic!("missing OpenCode resource: {path}"));
        assert_eq!(projected.content, source, "{path}");
    }
}

#[test]
fn release_report_fresh_builtin_sync_installs_runnable_bundle() {
    let fixture = TempDir::new_in(std::env::current_dir().unwrap()).unwrap();
    for (harness, directory) in [
        (SupervisorCli::Claude, ".claude"),
        (SupervisorCli::Codex, ".codex"),
        (SupervisorCli::Grok, ".grok"),
    ] {
        let destination = fixture.path().join(directory);
        cas::builtins::sync_all_builtins_for_project(harness, fixture.path()).unwrap();
        let skill = destination.join("skills/cas-release-report");
        for file in [
            "SKILL.md",
            "references/brief-template.md",
            "references/default-tokens.json",
            "references/exemplar.md",
            "references/pdf.md",
            "references/template.html",
            "scripts/render.py",
        ] {
            assert!(skill.join(file).is_file(), "missing installed {file}");
            println!("installed {directory}/skills/cas-release-report/{file}");
        }
        // Exercise installed resources, not the source copy, from a new project.
        let source = fixture.path().join("release.md");
        fs::write(
            &source,
            include_str!("../../docs/release-reports/2026-09-08-v3.19.0.md"),
        )
        .unwrap();
        let output = fixture.path().join("release.html");
        let rendered = Command::new("python3")
            .arg(skill.join("scripts/render.py"))
            .arg(&source)
            .arg("--output")
            .arg(&output)
            .arg("--project-root")
            .arg(fixture.path())
            .output()
            .expect("run installed release renderer");
        assert!(
            rendered.status.success(),
            "{}",
            String::from_utf8_lossy(&rendered.stderr)
        );
        assert_eq!(
            fs::read_to_string(&output)
                .unwrap()
                .matches("data-issue=\"")
                .count(),
            21
        );
    }
}

#[test]
fn supervisor_worker_liveness_contract_is_pinned_in_every_mirror() {
    for (flavor, text) in [
        ("", include_str!("../src/builtins/skills/cas-supervisor.md")),
        (
            "codex/",
            include_str!("../src/builtins/skills/cas-supervisor.md"),
        ),
        (
            "grok/",
            include_str!("../src/builtins/skills/cas-supervisor.md"),
        ),
    ] {
        for marker in [
            "worker_status summary_mode=true",
            "Trust `liveness`",
            "`executing`",
            "`waiting_for_input`",
            "`stalled`",
            "`dead`",
            "heartbeat and registry status do not prove execution",
        ] {
            assert!(text.contains(marker), "{flavor}: missing {marker}");
        }
        assert!(!text.contains("fresh heartbeat **or** live OS process"));
    }
}
