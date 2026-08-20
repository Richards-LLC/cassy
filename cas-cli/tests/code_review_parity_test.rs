//! Parity tests against the legacy `code-reviewer` agent (cas-22fa).
//!
//! Validates EPIC cas-0750 success criterion #1: the new multi-persona
//! pipeline catches every red-flag class the legacy `code-reviewer.md`
//! agent caught. The legacy agent was an LLM driven by a single prompt
//! file, so its red-flag set lives in *prose* — there is no programmatic
//! API to compare against. The closest faithful test we can ship is to
//! assert that every legacy red-flag pattern is *named* in at least one
//! persona prompt file in the new skill, on both the claude and codex
//! mirrors. If a worker reads the personas and the pattern is not
//! mentioned, the LLM will not look for it, and the parity claim breaks.
//!
//! The pattern set was inventoried at cas-1e98 close time from the
//! legacy `cas-cli/src/builtins/agents/code-reviewer.md` (now a
//! deprecation stub) and is the source of truth for "no silent
//! regression". Adding a new red flag to the new pipeline is encouraged;
//! removing one without compensating coverage is the failure mode this
//! test exists to catch.

use std::fs;
use std::path::PathBuf;

/// One legacy red-flag pattern from the old code-reviewer agent. Each
/// entry must be mentioned somewhere in the persona files of the new
/// skill on both mirrors. The matcher is plain `contains` against the
/// raw file text — case sensitive — because the personas are written
/// to be read by an LLM and we want the literal token to be visible.
#[derive(Debug, Clone, Copy)]
struct LegacyPattern {
    /// Human-readable label for the failure message.
    label: &'static str,
    /// Substrings that must appear *somewhere* across the persona set.
    /// At least one of them must hit. Multi-form patterns (e.g.
    /// `catch (e) {}` vs `catch($ERR){}`) are listed as alternates.
    needles: &'static [&'static str],
    /// Which persona is expected to own this pattern. Used only in the
    /// failure message — the test does not enforce ownership, just
    /// presence in the persona set.
    expected_owner: &'static str,
}

const LEGACY_PATTERNS: &[LegacyPattern] = &[
    // ---- Rust correctness red flags ----
    LegacyPattern {
        label: "Rust .unwrap() on fallible call",
        needles: &[".unwrap()"],
        expected_owner: "correctness",
    },
    LegacyPattern {
        label: "Rust .expect() on fallible call",
        needles: &[".expect()"],
        expected_owner: "correctness",
    },
    LegacyPattern {
        label: "Rust todo!() macro",
        needles: &["todo!()"],
        expected_owner: "correctness",
    },
    LegacyPattern {
        label: "Rust unimplemented!() macro",
        needles: &["unimplemented!()"],
        expected_owner: "correctness",
    },
    LegacyPattern {
        label: "Rust #[allow(dead_code)] on new code",
        needles: &["#[allow(dead_code)]"],
        expected_owner: "correctness",
    },
    LegacyPattern {
        label: "Rust let _ = <fallible> (silently dropped Result)",
        needles: &["let _ = "],
        expected_owner: "correctness",
    },
    // ---- TypeScript red flags ----
    LegacyPattern {
        label: "TypeScript $EXPR as any",
        needles: &["as any"],
        expected_owner: "correctness",
    },
    LegacyPattern {
        label: "TypeScript @ts-ignore",
        needles: &["@ts-ignore"],
        expected_owner: "correctness",
    },
    LegacyPattern {
        label: "TypeScript console.log in production",
        needles: &["console.log"],
        expected_owner: "correctness",
    },
    LegacyPattern {
        label: "TypeScript empty catch block",
        needles: &["empty catch", "catch ($ERR) {}", "catch (e) {}", "catch ()"],
        expected_owner: "correctness",
    },
    // ---- Python red flags ----
    LegacyPattern {
        label: "Python bare except:",
        needles: &["bare `except:`", "bare except:", "except:"],
        expected_owner: "correctness",
    },
    LegacyPattern {
        label: "Python # type: ignore",
        needles: &["# type: ignore"],
        expected_owner: "correctness",
    },
    // ---- All-language markers ----
    LegacyPattern {
        label: "TODO / FIXME / HACK / XXX markers",
        needles: &["TODO", "FIXME", "HACK", "XXX"],
        expected_owner: "correctness",
    },
    LegacyPattern {
        label: "temporal placeholder language (\"for now\", \"temporarily\")",
        needles: &["for now", "temporarily", "placeholder"],
        expected_owner: "correctness",
    },
    LegacyPattern {
        label: "dead-or-unwired new code (function/route/MCP tool with zero callers)",
        needles: &["dead-or-unwired", "Dead-or-unwired", "zero references"],
        expected_owner: "correctness",
    },
    // ---- Project standards red flags ----
    LegacyPattern {
        // cas-2c61: the codex mirror correctly uses mcp__cs__ (its own
        // tool alias), not Claude's mcp__cas__ — both needles are the same
        // pattern under each harness's prefix, not two different patterns.
        label: "mcp__cas__rule rule-compliance check",
        needles: &["mcp__cas__rule", "mcp__cs__rule"],
        expected_owner: "project-standards",
    },
    LegacyPattern {
        label: "CLAUDE.md / AGENTS.md project conventions",
        needles: &["CLAUDE.md", "AGENTS.md"],
        expected_owner: "project-standards",
    },
    LegacyPattern {
        label: "managed_by: cas file protection",
        needles: &["managed_by: cas", "managed_by:cas"],
        expected_owner: "project-standards",
    },
];

/// Personas that ship under both mirrors. Kept in lockstep with the
/// distribution registration in `cas-cli/src/builtins.rs`.
const PERSONA_FILES: &[&str] = &[
    "correctness.md",
    "testing.md",
    "maintainability.md",
    "project-standards.md",
    "fallow.md",
    "security.md",
    "performance.md",
    "adversarial.md",
];

fn workspace_root() -> PathBuf {
    cas::test_paths::workspace_root()
}

fn load_persona_corpus(mirror: &str) -> String {
    let mut combined = String::new();
    let base = workspace_root()
        .join("cas-cli/src/builtins")
        .join(if mirror == "claude" { "" } else { "codex" })
        .join("skills/cas-code-review/references/personas");
    for name in PERSONA_FILES {
        let path = base.join(name);
        let body = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("could not read {} mirror persona {}: {e}", mirror, path.display()));
        combined.push_str(&body);
        combined.push('\n');
    }
    combined
}

fn assert_legacy_patterns_present(mirror: &str) {
    let corpus = load_persona_corpus(mirror);
    let mut missing: Vec<String> = Vec::new();
    for pat in LEGACY_PATTERNS {
        let hit = pat.needles.iter().any(|needle| corpus.contains(needle));
        if !hit {
            missing.push(format!(
                "  - {} (expected owner persona: {}; needles tried: {:?})",
                pat.label, pat.expected_owner, pat.needles
            ));
        }
    }
    if !missing.is_empty() {
        panic!(
            "Parity regression on the {} mirror: {} legacy code-reviewer red-flag pattern(s) \
             are not mentioned in any persona prompt. EPIC cas-0750 success criterion #1 \
             requires zero silent drops vs the legacy agent.\n\nMissing:\n{}",
            mirror,
            missing.len(),
            missing.join("\n")
        );
    }
}

#[test]
fn legacy_patterns_present_on_claude_mirror() {
    assert_legacy_patterns_present("claude");
}

#[test]
fn legacy_patterns_present_on_codex_mirror() {
    assert_legacy_patterns_present("codex");
}

/// Sanity check: every persona file we expect actually exists on disk
/// for both mirrors. Catches the "renamed a persona but forgot to
/// update the parity test" footgun.
#[test]
fn all_persona_files_exist_on_both_mirrors() {
    for mirror in ["claude", "codex"] {
        let base = workspace_root()
            .join("cas-cli/src/builtins")
            .join(if mirror == "claude" { "" } else { "codex" })
            .join("skills/cas-code-review/references/personas");
        for name in PERSONA_FILES {
            let p = base.join(name);
            assert!(
                p.exists(),
                "expected persona file missing on {mirror} mirror: {}",
                p.display()
            );
        }
    }
}

/// Documents the inventory itself: a regression in this count means
/// someone added or removed a legacy red-flag pattern without updating
/// the parity surface. Bumping the constant is the right fix; silently
/// adding patterns without bumping is what we want to catch.
#[test]
fn legacy_pattern_inventory_is_pinned() {
    // 17 patterns inventoried from the legacy code-reviewer agent at
    // cas-1e98 close time, plus the project-standards rule wiring.
    assert_eq!(
        LEGACY_PATTERNS.len(),
        18,
        "legacy pattern inventory changed — review whether the new patterns \
         are still covered by personas, then update this expected count"
    );
}
