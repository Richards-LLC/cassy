//! Regression tests for `cas update --sync` reporting (cas-27bf).
//!
//! The bug: the human-readable built-in sync summary (`Updated N built-in
//! files` + the `+ path` list) was rendered in a trailing block that ran AFTER
//! the `Syncing .codex files` / `Syncing .grok files` subheadings had already
//! been printed, and it reported the CLAUDE harness result. A Claude-only write
//! was therefore displayed as though Codex had performed it, while the Codex and
//! Grok counts were never printed in human mode at all. Users saw
//! "[OK] Updated 1 built-in files ... + skills/cas-worker/references/discipline.md"
//! under `.codex` while nothing under `.codex` had been touched.
//!
//! These tests pin the invariant: every claimed write is attributed to the
//! harness directory it actually landed in, and the claimed file exists there.

use assert_cmd::Command;
use std::path::Path;
use tempfile::TempDir;

fn cas_cmd(root: &Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cas"));
    let home = root.join(".test-home");
    let xdg = root.join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home).env("XDG_CONFIG_HOME", xdg);
    cmd.env_remove("CAS_ROOT");
    cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

/// Strip ANSI escape sequences so assertions work regardless of theme/tty.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // consume until a letter terminator (CSI final byte)
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Every `+ <path>` line the sync prints is a claimed write. Return them.
fn claimed_writes(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter_map(|l| l.strip_prefix("+ "))
        .map(str::to_string)
        .collect()
}

fn init_project(temp: &TempDir) {
    cas_cmd(temp.path())
        .current_dir(temp)
        .args(["init", "--yes"])
        .assert()
        .success();
}

/// The reproduction: a pending Claude-side write plus an enabled Codex harness.
/// Pre-fix, the Claude write was printed under the `.codex` heading with a bare
/// relative path that did not exist under `.codex`.
#[test]
fn sync_attributes_every_claimed_write_to_the_directory_it_landed_in() {
    let temp = TempDir::new().unwrap();
    init_project(&temp);

    let project = temp.path();
    // Enable the codex harness (sync is gated on the directory existing).
    std::fs::create_dir_all(project.join(".codex")).unwrap();

    // Force at least one pending CLAUDE-side write: delete a builtin that init
    // already materialized under .claude/.
    let claude_skills = project.join(".claude/skills");
    let victim = std::fs::read_dir(&claude_skills)
        .expect(".claude/skills should exist after init")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.join("SKILL.md").is_file())
        .expect("at least one builtin skill should be installed under .claude/skills");
    std::fs::remove_file(victim.join("SKILL.md")).unwrap();

    let assert = cas_cmd(project)
        .current_dir(project)
        .args(["update", "--sync"])
        .assert()
        .success();
    let stdout = strip_ansi(&String::from_utf8_lossy(&assert.get_output().stdout));

    let writes = claimed_writes(&stdout);
    assert!(
        !writes.is_empty(),
        "expected the deleted builtin to be reported as a write; output was:\n{stdout}"
    );

    // (1) Every claimed write names the harness dir it landed in, and
    // (2) that file actually exists on disk. This is the anti-false-success
    // invariant: the tool may not claim a write it did not perform.
    for w in &writes {
        assert!(
            w.starts_with(".claude/") || w.starts_with(".codex/") || w.starts_with(".grok/"),
            "claimed write `{w}` is not attributed to a harness directory; output was:\n{stdout}"
        );
        assert!(
            project.join(w).exists(),
            "sync claimed to write `{w}` but no such file exists on disk; output was:\n{stdout}"
        );
    }

    // (3) No Claude-directory write may be reported after the `.codex` heading —
    // that misattribution is the exact shape of the original bug.
    if let Some((_, codex_section)) = stdout.split_once("Syncing .codex files") {
        for w in claimed_writes(codex_section) {
            assert!(
                !w.starts_with(".claude/"),
                "a .claude write (`{w}`) was reported under the .codex section; output was:\n{stdout}"
            );
        }
    }

    // The deleted builtin was genuinely restored.
    assert!(victim.join("SKILL.md").is_file());
}

/// The genuine codex sync path works end-to-end in an isolated HOME: an empty
/// project `.codex/` gets real files, they are reported under the `.codex`
/// heading, and a second run reports it as up to date instead of re-claiming
/// writes.
#[test]
fn codex_sync_reports_its_own_result_and_is_idempotent() {
    let temp = TempDir::new().unwrap();
    init_project(&temp);

    let project = temp.path();
    std::fs::create_dir_all(project.join(".codex")).unwrap();

    let first = cas_cmd(project)
        .current_dir(project)
        .args(["update", "--sync"])
        .assert()
        .success();
    let first_out = strip_ansi(&String::from_utf8_lossy(&first.get_output().stdout));

    // Codex is reported in its own right (either it wrote files or it is clean),
    // never silently omitted from human output as it was pre-fix.
    assert!(
        first_out.contains(".codex: updated") || first_out.contains(".codex: built-ins up to date"),
        "codex sync result missing from human output; output was:\n{first_out}"
    );

    // The codex harness ships builtins, so the first run into an empty .codex
    // must actually write them.
    assert!(
        first_out.contains(".codex: updated"),
        "expected real codex writes into an empty .codex; output was:\n{first_out}"
    );
    for w in claimed_writes(&first_out) {
        assert!(
            project.join(&w).exists(),
            "codex sync claimed `{w}` but it does not exist; output was:\n{first_out}"
        );
    }

    // Second run: nothing left to do, and nothing claimed.
    let second = cas_cmd(project)
        .current_dir(project)
        .args(["update", "--sync"])
        .assert()
        .success();
    let second_out = strip_ansi(&String::from_utf8_lossy(&second.get_output().stdout));
    assert!(
        second_out.contains(".codex: built-ins up to date"),
        "second sync should report codex clean; output was:\n{second_out}"
    );
    assert!(
        second_out.contains(".claude: built-ins up to date"),
        "second sync should report claude clean; output was:\n{second_out}"
    );
}
