//! Distribution and routing contracts for the built-in cas-image-generate skill.

use assert_cmd::Command;
use predicates::prelude::*;
use quick_xml::Reader;
use quick_xml::events::Event;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[path = "support/builtin_catalog.rs"]
mod builtin_catalog;

fn load(relative: &str) -> &'static str {
    builtin_catalog::find_source_path(relative)
}

fn materialized_script() -> (TempDir, PathBuf) {
    let directory = TempDir::new().expect("temporary image helper directory");
    let script = directory.path().join("generate-image.sh");
    fs::write(
        &script,
        builtin_catalog::find(
            builtin_catalog::Flavor::Claude,
            "skills/cas-image-generate/scripts/generate-image.sh",
        ),
    )
    .expect("write embedded image helper");
    (directory, script)
}

fn skill_paths() -> [&'static str; 3] {
    [
        "cas-cli/src/builtins/skills/cas-image-generate/SKILL.md",
        "cas-cli/src/builtins/codex/skills/cas-image-generate/SKILL.md",
        "cas-cli/src/builtins/grok/skills/cas-image-generate/SKILL.md",
    ]
}

#[test]
fn skill_mirrors_are_managed_and_cover_the_asset_workflow() {
    for path in skill_paths() {
        let body = load(path);
        assert!(body.starts_with("---\n"), "{path} lacks frontmatter");
    }
}

#[test]
fn worked_svg_examples_are_well_formed_and_use_palette_tokens() {
    let reference =
        load("cas-cli/src/builtins/skills/cas-image-generate/references/svg-web-assets.md");
    let examples = fenced_svg_examples(&reference);
    assert_eq!(
        examples.len(),
        3,
        "expected icon, divider, and favicon examples"
    );

    for (index, example) in examples.iter().enumerate() {
        let mut reader = Reader::from_str(example);
        reader.config_mut().trim_text(true);
        loop {
            match reader.read_event() {
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(error) => panic!("worked SVG example {index} is not XML: {error}"),
            }
        }
        assert!(
            example.contains("viewBox"),
            "example {index} needs a viewBox"
        );
        assert!(
            example.contains("--color-") || example.contains("currentColor"),
            "example {index} needs a palette token"
        );
    }
}

fn fenced_svg_examples(body: &str) -> Vec<String> {
    let mut examples = Vec::new();
    let mut current = None;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed == "```svg" {
            assert!(current.is_none(), "nested SVG example fence");
            current = Some(String::new());
        } else if trimmed == "```" {
            if let Some(example) = current.take() {
                examples.push(example);
            }
        } else if let Some(example) = current.as_mut() {
            example.push_str(line);
            example.push('\n');
        }
    }
    assert!(current.is_none(), "unterminated SVG example fence");
    examples
}

#[test]
fn builtin_catalog_registers_all_mirror_entries() {
    // Audit D1: every harness catalog embeds the one prefix-neutral copy.
    for relative in [
        "skills/cas-image-generate/SKILL.md",
        "skills/cas-image-generate/references/asset-playbook.md",
        "skills/cas-image-generate/references/svg-web-assets.md",
        "skills/cas-image-generate/scripts/generate-image.sh",
    ] {
        let claude = builtin_catalog::find(builtin_catalog::Flavor::Claude, relative);
        for flavor in [builtin_catalog::Flavor::Codex, builtin_catalog::Flavor::Grok] {
            assert_eq!(
                builtin_catalog::try_find(flavor, relative),
                Some(claude),
                "missing {flavor:?} catalog entry {relative}"
            );
        }
    }
}

#[test]
fn cas_update_syncs_the_skill_to_all_enabled_harnesses() {
    let project = TempDir::new().unwrap();
    let home = project.path().join("home");
    let xdg = project.path().join("xdg");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&xdg).unwrap();

    let mut command = Command::new(cas::test_paths::cas_binary());
    command
        .current_dir(project.path())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &xdg)
        .env_remove("CAS_ROOT")
        .env_remove("GEMINI_API_KEY")
        .args(["init", "--yes"])
        .assert()
        .success();

    for harness in [".codex", ".grok"] {
        fs::create_dir_all(project.path().join(harness)).unwrap();
    }
    let mut sync = Command::new(cas::test_paths::cas_binary());
    sync.current_dir(project.path())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &xdg)
        .env_remove("CAS_ROOT")
        .args(["update", "--sync"])
        .assert()
        .success();

    for harness in [".claude", ".codex", ".grok"] {
        for relative in [
            "skills/cas-image-generate/SKILL.md",
            "skills/cas-image-generate/references/providers.md",
            "skills/cas-image-generate/references/svg-web-assets.md",
            "skills/cas-image-generate/scripts/generate-image.sh",
        ] {
            assert!(
                project.path().join(harness).join(relative).is_file(),
                "missing {harness}/{relative} after cas update --sync"
            );
        }
    }
}

#[test]
fn generation_helper_reports_missing_key_without_network_access() {
    let (_directory, script) = materialized_script();
    Command::new("bash")
        .arg(&script)
        .args(["--prompt", "test", "--output", "out.png"])
        .env_remove("GEMINI_API_KEY")
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("GEMINI_API_KEY"))
        .stderr(predicate::str::contains("Google AI Studio"));
}

#[test]
fn generation_helper_dry_run_validates_present_key_without_calling_api() {
    let (_directory, script) = materialized_script();
    Command::new("bash")
        .arg(&script)
        .args([
            "--tier",
            "final",
            "--prompt",
            "test",
            "--output",
            "out.png",
            "--dry-run",
        ])
        .env("GEMINI_API_KEY", "test-key-not-sent")
        .assert()
        .success()
        .stdout(predicate::str::contains("provider=google-nano-banana"))
        .stdout(predicate::str::contains("model=gemini-3-pro-image"))
        .stdout(predicate::str::contains("dry_run=true"))
        .stdout(predicate::str::contains("test-key-not-sent").not());
}
