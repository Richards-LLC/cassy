//! Contracts for the built-in skill hygiene wave (cas-6cba).

use std::fs;

use cas::builtins::sync_all_builtins_for_project;
use cas_mux::SupervisorCli;
use tempfile::TempDir;

#[path = "support/builtin_catalog.rs"]
mod builtin_catalog;

fn source(flavor: &str, relative: &str) -> &'static str {
    let flavor = match flavor {
        "" => builtin_catalog::Flavor::Claude,
        "codex" => builtin_catalog::Flavor::Codex,
        "grok" => builtin_catalog::Flavor::Grok,
        other => panic!("unknown builtin flavor {other}"),
    };
    builtin_catalog::find(flavor, relative)
}

#[test]
fn mcp_and_viktor_guidance_use_the_cassy_surface() {
    // Audit D1: every flavor names Cassy tools by bare name.
    let prefix = "";
    for flavor in ["", "codex", "grok"] {
        let mcp = source(flavor, "skills/mcp-integration/SKILL.md");
        for marker in [
            "cas mcp add",
            "cas mcp list --json",
            "cas mcp import",
            ".cas/proxy.toml",
            "proxy_add",
            "proxy_remove",
            "proxy_list",
            "proxy_health",
        ] {
            assert!(
                mcp.contains(marker),
                "{flavor:?} mcp guidance missing {marker:?}"
            );
        }
        for action in ["proxy_add", "proxy_remove", "proxy_list", "proxy_health"] {
            assert!(
                mcp.contains(&format!("{prefix}system action={action}")),
                "{flavor:?} mcp guidance missing worked {action} call"
            );
        }

        let viktor = source(flavor, "skills/cas-viktor/SKILL.md");
        assert!(
            viktor.contains("mcp_execute"),
            "{flavor:?} Viktor guidance lacks mcp_execute"
        );
        assert!(
            viktor.contains(&format!("{prefix}mcp_execute")),
            "{flavor:?} Viktor guidance lacks its mcp_execute namespace"
        );
        assert!(
            viktor.contains(r#"\"server\":\"viktor\""#)
                && viktor.contains(r#"\"tool\":\"whoami\""#)
                && viktor.contains(r#"\"args\":{}"#),
            "{flavor:?} Viktor guidance lacks the JSON dispatch shape"
        );
    }

    let diagnosis = source("", "skills/mcp-integration/references/diagnosis.md");
    assert!(diagnosis.contains("## Symptom → cause"));
    assert!(!diagnosis.contains("cas mcp add"));
    assert!(!diagnosis.contains("mcp__cas__system"));
}

#[test]
fn release_notes_are_generic_procedure_and_rubric_driven() {
    for flavor in ["", "codex", "grok"] {
        let skill = source(flavor, "skills/cas-release-notes/SKILL.md");
        for marker in [
            "ensure the rubric exists",
            "gather the merge",
            "draft",
            "save the draft",
            "post",
            "receipt",
            "docs/release-notes/rubric.md",
        ] {
            assert!(
                skill.to_ascii_lowercase().contains(marker),
                "{flavor:?} release notes missing {marker:?}"
            );
        }
        for banned in [
            "docs/SLACK_POSTING_RUNBOOK.md",
            "pippenz@gmail.com",
            "claude.ai",
            "transport",
            "profile",
        ] {
            assert!(
                !skill.contains(banned),
                "{flavor:?} release notes contains transport/account text {banned:?}"
            );
        }
        assert!(!skill.contains("exactly one threaded reply"));
    }

    let rubric = source("", "skills/cas-release-notes/references/RUBRIC-template.md");
    assert!(rubric.contains("Default: one threaded reply per thread"));

    let init = include_str!("../src/cli/init/docs_and_skill.rs");
    assert!(init.contains("follow docs/release-notes/RUBRIC.md"));
    assert!(!init.contains("Slack per docs/release-notes/RUBRIC.md"));
}

#[test]
fn fallow_examples_honor_machine_output_rule() {
    let skill = source("", "skills/fallow/SKILL.md");
    let mut command_count = 0;
    for line in skill.lines() {
        let command = line.trim_start();
        if command.starts_with("fallow ") {
            command_count += 1;
            assert!(
                command.contains("--format json"),
                "example lacks JSON output: {command}"
            );
            assert!(
                command.contains("--quiet"),
                "example lacks quiet output: {command}"
            );
            // `|| true` forces status 0, so a missing binary or a failed
            // `npx` looked like a clean pass; the status must stay visible.
            assert!(
                command.contains("2>/dev/null; echo \"exit=$?\""),
                "example must print its exit status: {command}"
            );
            assert!(
                !command.contains("|| true"),
                "example swallows the exit status: {command}"
            );
        }
    }
    assert!(
        command_count >= 10,
        "expected the workflow examples to be guarded"
    );
    assert!(skill.contains("## Procedure"));
    assert!(skill.contains("Preserve and read the exit status"));
    // Counts and tool tables drift between fallow releases; the skill points
    // at `fallow schema` instead of copying them.
    assert!(skill.contains("fallow schema"));
    for stale in [
        "91 framework plugins",
        "90 auto-detecting",
        "## Node.js Bindings",
        "| `trace_clone` |",
    ] {
        assert!(!skill.contains(stale), "stale fallow content: {stale}");
    }
}

#[test]
fn optional_stack_skills_follow_detection_and_explicit_enable() {
    for harness in [
        SupervisorCli::Claude,
        SupervisorCli::Codex,
        SupervisorCli::Grok,
    ] {
        let rust_project = TempDir::new().unwrap();
        fs::create_dir_all(rust_project.path().join(".cas")).unwrap();
        sync_all_builtins_for_project(harness, rust_project.path()).unwrap();
        let prefix = match harness {
            SupervisorCli::Claude => ".claude",
            SupervisorCli::Codex => ".codex",
            SupervisorCli::Grok => ".grok",
            SupervisorCli::OpenCode => unreachable!(),
        };
        assert!(
            !rust_project
                .path()
                .join(prefix)
                .join("skills/fallow")
                .exists()
        );
        assert!(
            !rust_project
                .path()
                .join(prefix)
                .join("skills/cas-nuxt-playwright")
                .exists()
        );

        fs::write(
            rust_project.path().join("package.json"),
            r#"{"dependencies":{"nuxt":"^3.0.0"}}"#,
        )
        .unwrap();
        sync_all_builtins_for_project(harness, rust_project.path()).unwrap();
        assert!(
            rust_project
                .path()
                .join(prefix)
                .join("skills/fallow/SKILL.md")
                .is_file()
        );
        assert!(
            rust_project
                .path()
                .join(prefix)
                .join("skills/cas-nuxt-playwright/SKILL.md")
                .is_file()
        );

        let explicit_project = TempDir::new().unwrap();
        fs::create_dir_all(explicit_project.path().join(".cas")).unwrap();
        fs::write(
            explicit_project.path().join(".cas/config.toml"),
            "[skills]\noptional = [\"fallow\"]\n",
        )
        .unwrap();
        sync_all_builtins_for_project(harness, explicit_project.path()).unwrap();
        assert!(
            explicit_project
                .path()
                .join(prefix)
                .join("skills/fallow/SKILL.md")
                .is_file()
        );
        assert!(
            !explicit_project
                .path()
                .join(prefix)
                .join("skills/cas-nuxt-playwright")
                .exists()
        );
    }
}

/// cas-5e54: the Playwright debugging skill teaches the 1.59+ agent tooling
/// (trace CLI, `--debug=cli` + session-scoped `playwright cli`) with command
/// shapes verified against @playwright/test 1.63, and never recommends the
/// idioms Playwright discourages inside a runnable example.
#[test]
fn playwright_debug_teaches_trace_cli_and_agent_debugger() {
    let skill = source("", "skills/cas-playwright-debug/SKILL.md");
    for flavor in ["codex", "grok"] {
        assert_eq!(
            source(flavor, "skills/cas-playwright-debug/SKILL.md"),
            skill,
            "{flavor} mirror must be byte-identical"
        );
    }

    for required in [
        "npx playwright trace open ",
        "npx playwright trace actions --errors-only",
        "npx playwright trace action <id>",
        "npx playwright trace snapshot <id> --phase before",
        "npx playwright trace requests --failed",
        "--debug=cli",
        "npx playwright cli attach tw-",
        "-s=tw-XXXXXX step-over",
        "pause-at",
        "retryStrategy: 'isolated'",
        "failOnFlakyTests",
        "{ lock: '",
        "snapshots: { dom: true, aria: true, screen: true }",
        ".visible()",
        "page.frameLocator()",
    ] {
        assert!(skill.contains(required), "skill must teach {required:?}");
    }
    // 1.63's snapshot phase flag is --phase; --name does not exist.
    assert!(!skill.contains("--name before"));

    // Discouraged idioms may appear only as the left column of the
    // "Instead of" table or in prose that forbids them — never in a code block.
    let mut in_code = false;
    for line in skill.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            for banned in ["networkidle", "waitForTimeout", ":visible"] {
                assert!(
                    !line.contains(banned),
                    "code example recommends {banned}: {line}"
                );
            }
        }
    }
    assert!(skill.contains("| Instead of | Write |"));
}

/// cas-5e54: cas-playwright-debug is an optional stack skill selected by a
/// Playwright Test dependency, a playwright config file, or explicit opt-in.
#[test]
fn playwright_debug_is_selected_by_playwright_detection() {
    let installed = |root: &std::path::Path| {
        sync_all_builtins_for_project(SupervisorCli::Claude, root).unwrap();
        root.join(".claude/skills/cas-playwright-debug/SKILL.md")
            .is_file()
    };

    let plain = TempDir::new().unwrap();
    fs::create_dir_all(plain.path().join(".cas")).unwrap();
    fs::write(
        plain.path().join("package.json"),
        r#"{"dependencies":{"vite":"^5.0.0"}}"#,
    )
    .unwrap();
    assert!(!installed(plain.path()), "no Playwright, no skill");

    let dependency = TempDir::new().unwrap();
    fs::create_dir_all(dependency.path().join(".cas")).unwrap();
    fs::write(
        dependency.path().join("package.json"),
        r#"{"devDependencies":{"@playwright/test":"^1.63.0"}}"#,
    )
    .unwrap();
    assert!(installed(dependency.path()), "@playwright/test selects it");

    let config_only = TempDir::new().unwrap();
    fs::create_dir_all(config_only.path().join(".cas")).unwrap();
    fs::write(config_only.path().join("playwright.config.ts"), "").unwrap();
    assert!(
        installed(config_only.path()),
        "playwright.config.ts selects it"
    );

    let explicit = TempDir::new().unwrap();
    fs::create_dir_all(explicit.path().join(".cas")).unwrap();
    fs::write(
        explicit.path().join(".cas/config.toml"),
        "[skills]\noptional = [\"playwright-debug\"]\n",
    )
    .unwrap();
    assert!(installed(explicit.path()), "explicit opt-in selects it");
}
