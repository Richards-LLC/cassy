//! Contracts for the built-in skill hygiene wave (cas-6cba).

use std::fs;
use std::path::Path;

use cas::builtins::sync_all_builtins_for_project;
use cas_mux::SupervisorCli;
use tempfile::TempDir;

fn source_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/builtins")
        .leak()
}

fn source(flavor: &str, relative: &str) -> String {
    let root = source_root();
    let path = if flavor.is_empty() {
        root.join(relative)
    } else {
        root.join(flavor).join(relative)
    };
    fs::read_to_string(path).expect("builtin source must exist")
}

#[test]
fn mcp_and_viktor_guidance_use_the_cassy_surface() {
    for (flavor, prefix) in [
        ("", "mcp__cas__"),
        ("codex", "mcp__cs__"),
        ("grok", "cas__"),
    ] {
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
        let skill = source(flavor, "skills/release-notes/SKILL.md");
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

    let rubric = source("", "skills/release-notes/references/RUBRIC-template.md");
    assert!(rubric.contains("Default: one threaded reply per thread"));

    let init = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli/init/docs_and_skill.rs"),
    )
    .expect("init source must exist");
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
            assert!(
                command.contains("2>/dev/null || true"),
                "example lacks safe exit handling: {command}"
            );
        }
    }
    assert!(
        command_count >= 10,
        "expected the workflow examples to be guarded"
    );
    assert!(skill.contains("## Procedure"));
    assert!(skill.contains("91 framework plugins"));
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
