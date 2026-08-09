use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cas-cli must live under repo root")
        .to_path_buf()
}

fn load(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e))
}

#[test]
fn codex_factory_skills_use_cs_prefix_only() {
    let root = repo_root();
    let skills_dir = root.join(".codex/skills");
    if !skills_dir.exists() {
        return;
    }

    let entries = fs::read_dir(&skills_dir).expect("read .codex/skills");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("cas-factory-") {
            continue;
        }
        let skill_path = entry.path().join("SKILL.md");
        if !skill_path.exists() {
            continue;
        }

        let content = load(&skill_path);
        let has_mcp_examples = content.contains("mcp__cs__") || content.contains("mcp__cas__");
        if has_mcp_examples {
            assert!(
                content.contains("mcp__cs__"),
                "{} should include mcp__cs__ examples",
                skill_path.display()
            );
        }
        assert!(
            !content.contains("mcp__cas__"),
            "{} still contains legacy mcp__cas__ references",
            skill_path.display()
        );
        assert!(
            !content.contains("action=prompt"),
            "{} still contains legacy action=prompt usage",
            skill_path.display()
        );
    }
}

#[test]
fn codex_worker_recovery_uses_cs_alias_not_cas(/* cas-5b4f */) {
    // The Codex worker recovery guide is rendered only into Codex worker
    // sessions, where every CAS tool surfaces under the `mcp__cs__` alias.
    // It must never hardcode `mcp__cas__` coordination/task instructions — those
    // are unreachable for a Codex worker. Mirrors the file-level
    // `codex_factory_skills_use_cs_prefix_only` convention.
    let root = repo_root();
    let codex_recovery = load(
        &root.join("cas-cli/src/builtins/codex/skills/cas-worker/references/recovery.md"),
    );

    assert!(
        codex_recovery.contains("mcp__cs__coordination"),
        "codex worker recovery.md should give executable mcp__cs__coordination guidance"
    );
    assert!(
        !codex_recovery.contains("mcp__cas__"),
        "codex worker recovery.md must not hardcode the Claude `mcp__cas__` alias \
         (Codex workers only have mcp__cs__ tools)"
    );

    // AC: the Claude worker recovery doc must stay correct for the Claude alias.
    let claude_recovery = load(
        &root.join("cas-cli/src/builtins/skills/cas-worker/references/recovery.md"),
    );
    assert!(
        claude_recovery.contains("mcp__cas__coordination"),
        "claude worker recovery.md should retain mcp__cas__coordination guidance"
    );
}

#[test]
fn codex_builtin_supervisor_guide_includes_core_workflow() {
    let root = repo_root();
    let guide = root.join("cas-cli/src/builtins/codex/skills/cas-supervisor.md");
    let content = load(&guide);

    assert!(
        content.contains("spawn_workers"),
        "supervisor guide should include spawn_workers"
    );
    assert!(
        content.contains("Never implement tasks yourself"),
        "supervisor guide should include hard rule about not implementing"
    );
    assert!(
        content.contains("mcp__cs__"),
        "codex supervisor guide should use mcp__cs__ prefix"
    );
}

/// cas-314d: supervisors and workers share one bounded, push-first reminder
/// contract. The on-demand reference avoids inflating the SessionStart skill
/// bodies, but all three installed flavors must retain identical semantics.
#[test]
fn reminder_discipline_reference_is_complete_and_flavor_normalized() {
    let root = repo_root();
    let claude = load(
        &root.join("cas-cli/src/builtins/skills/cas-supervisor/references/reminders.md"),
    );
    let codex = load(
        &root.join("cas-cli/src/builtins/codex/skills/cas-supervisor/references/reminders.md"),
    );
    let grok = load(
        &root.join("cas-cli/src/builtins/grok/skills/cas-supervisor/references/reminders.md"),
    );

    for (label, content) in [("claude", &claude), ("codex", &codex), ("grok", &grok)] {
        for required in [
            "## Decision table",
            "Push First, One Bounded Checkpoint",
            "exactly **one** trigger",
            "remind_delay_secs",
            "remind_event=task_completed",
            "remind_ttl_secs",
            "remind_cancel",
            "One active reminder per task/phase",
            "authoritative task/worker state",
            "MERGE REQUIRED",
            "Context-pressure handoff",
            "Blocked worker recovery",
            "detached command",
        ] {
            assert!(content.contains(required), "{label} reminder reference missing {required:?}");
        }
    }

    assert_eq!(claude.replace("mcp__cas__", "mcp__cs__"), codex);
    assert_eq!(claude.replace("mcp__cas__", "cas__"), grok);

    for path in [
        "cas-cli/src/builtins/skills/cas-supervisor.md",
        "cas-cli/src/builtins/codex/skills/cas-supervisor.md",
        "cas-cli/src/builtins/grok/skills/cas-supervisor.md",
        "cas-cli/src/builtins/skills/cas-worker.md",
        "cas-cli/src/builtins/codex/skills/cas-worker.md",
        "cas-cli/src/builtins/grok/skills/cas-worker.md",
    ] {
        assert!(
            load(&root.join(path)).contains("reminders.md"),
            "{path} must point to the shared reminder discipline reference"
        );
    }
}

#[test]
fn supervisor_skill_mirrors_include_implementation_unit_template() {
    // After cas-61af split cas-supervisor.md into a main file + references,
    // the Implementation Unit Template moved to planning.md. The guardrail
    // checks that file instead (both .claude and .codex trees must match).
    let root = repo_root();
    let claude = load(
        &root.join("cas-cli/src/builtins/skills/cas-supervisor/references/planning.md"),
    );
    let codex = load(
        &root.join("cas-cli/src/builtins/codex/skills/cas-supervisor/references/planning.md"),
    );

    for (label, content) in [("claude", &claude), ("codex", &codex)] {
        assert!(
            content.contains("## Implementation Unit Template"),
            "{label} planning.md missing '## Implementation Unit Template' heading"
        );
        // Canonical template markers (R1)
        for marker in [
            "**Unit N: [Name]**",
            "**Goal:**",
            "**Requirements:**",
            "**Dependencies:**",
            "**Files:**",
            "**Approach:**",
            "**Execution note:**",
            "**Patterns to follow:**",
            "**Test scenarios:**",
            "**Verification:**",
        ] {
            assert!(
                content.contains(marker),
                "{label} planning.md template missing marker: {marker}"
            );
        }
        // R4 mapping table
        assert!(
            content.contains("| Template field | Maps to |"),
            "{label} planning.md missing template→task schema mapping table"
        );
        // R6/R7 scope note
        assert!(
            content.contains("EPIC subtasks"),
            "{label} planning.md missing EPIC-subtasks-only scope note"
        );
        // R13 cross-link: Spec Requirements section mentions the template
        let spec_idx = content
            .find("## Spec Requirements")
            .unwrap_or_else(|| panic!("{label} missing Spec Requirements heading"));
        let tmpl_idx = content
            .find("## Implementation Unit Template")
            .unwrap_or_else(|| panic!("{label} missing Implementation Unit Template heading"));
        let spec_block = &content[spec_idx..tmpl_idx];
        assert!(
            spec_block.contains("Implementation Unit Template"),
            "{label} Spec Requirements section missing cross-link to Implementation Unit Template"
        );
    }
}

/// cas-2c61/cas-62ab: the ~25-file mcp__cas__ sweep beyond recovery.md
/// (cas-5b4f fixed only that one file; this closes the rest of the list
/// cas-62ab named — workflow.md, details.md, worker-recovery.md,
/// reference.md, cas-worker.md, session-learn/SKILL.md,
/// cas-memory-management/SKILL.md, and the remaining swept files). Every
/// codex builtin skill/agent source must use mcp__cs__ for executable
/// tool instructions, never Claude's mcp__cas__ — extends the
/// `codex_worker_recovery_uses_cs_alias_not_cas` convention corpus-wide.
#[test]
fn codex_builtin_skills_and_agents_never_hardcode_claude_alias() {
    let root = repo_root();

    for base in [
        root.join("cas-cli/src/builtins/codex/skills"),
        root.join("cas-cli/src/builtins/codex/agents"),
    ] {
        let mut stack = vec![base];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                    continue;
                };
                if ext != "md" && ext != "yaml" {
                    continue;
                }
                let content = load(&path);
                assert!(
                    !content.contains("mcp__cas__"),
                    "{} still hardcodes the Claude mcp__cas__ alias — codex agents only \
                     surface CAS tools under mcp__cs__ (cas-2c61/cas-62ab)",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn codex_worker_runtime_instruction_allows_close_then_escalate() {
    let root = repo_root();
    let pty_rs = root.join("crates/cas-pty/src/pty.rs");
    let content = load(&pty_rs);

    // cas-47b7: the worker instruction phrasing is "close it with
    // `mcp__cs__task action=close ...`" (cas-bbc2 single-task rewrite). Assert on
    // the close-command form itself rather than a fragile leading verb so prose
    // tweaks don't re-break this guardrail; the intent is only "workers ARE told
    // to close their task".
    assert!(
        content.contains("`mcp__cs__task action=close"),
        "runtime worker instruction should instruct workers to close tasks"
    );
    assert!(
        !content.contains("DO NOT close the task yourself"),
        "runtime worker instruction should not forbid close universally"
    );
}
