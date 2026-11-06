//! Contracts for the built-in skill hygiene wave (cas-6cba).

use std::fs;

use cas::builtins::sync_all_builtins_for_project;
use cas_mux::SupervisorCli;
use tempfile::TempDir;













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
        r#"{"dependencies":{"vite":"^9.99.0"}}"#,
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
