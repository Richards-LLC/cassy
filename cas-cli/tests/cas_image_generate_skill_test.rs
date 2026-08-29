//! Distribution and routing contracts for the built-in cas-image-generate skill.

use assert_cmd::Command;
use predicates::prelude::*;
use quick_xml::Reader;
use quick_xml::events::Event;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn repo_root() -> PathBuf {
    cas::test_paths::workspace_root()
}

fn load(relative: &str) -> String {
    let path = repo_root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
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
        assert!(
            body.contains("name: cas-image-generate"),
            "{path} has wrong name"
        );
        assert!(
            body.contains("managed_by: cas"),
            "{path} is not CAS-managed"
        );
        for marker in [
            "logo",
            "icon set",
            "hero",
            "background",
            "texture",
            "OG/social card",
            "report cover",
            "illustration",
            "favicon",
            "GEMINI_API_KEY",
            "style token block",
            "manual vectorization",
            "cas-image-generate/scripts/generate-image.sh",
            "agent-authored SVG",
            "svg-web-assets.md",
            "raster-to-vector bridge",
        ] {
            assert!(
                body.to_ascii_lowercase()
                    .contains(&marker.to_ascii_lowercase()),
                "{path} missing workflow marker {marker:?}"
            );
        }
    }
}

#[test]
fn svg_web_assets_reference_is_mirrored_and_covers_the_pipeline() {
    let paths = [
        "cas-cli/src/builtins/skills/cas-image-generate/references/svg-web-assets.md",
        "cas-cli/src/builtins/codex/skills/cas-image-generate/references/svg-web-assets.md",
        "cas-cli/src/builtins/grok/skills/cas-image-generate/references/svg-web-assets.md",
    ];

    for path in paths {
        let body = load(path);
        for marker in [
            "Agent-authored SVG",
            "author directly",
            "24px",
            "viewBox",
            "currentColor",
            "--color-",
            "favicon.svg",
            "raster-to-vector",
            "vtracer",
            "potrace",
            "Inkscape",
            "public/",
            "assets/brand/",
            "1200x630",
            "srcset",
            "cwebp",
            "ImageMagick",
            "output-checklist.md",
        ] {
            assert!(
                body.to_ascii_lowercase().contains(&marker.to_ascii_lowercase()),
                "{path} missing SVG/web-asset marker {marker:?}"
            );
        }
    }
}

#[test]
fn worked_svg_examples_are_well_formed_and_use_palette_tokens() {
    let reference = load(
        "cas-cli/src/builtins/skills/cas-image-generate/references/svg-web-assets.md",
    );
    let examples = fenced_svg_examples(&reference);
    assert_eq!(examples.len(), 3, "expected icon, divider, and favicon examples");

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
        assert!(example.contains("viewBox"), "example {index} needs a viewBox");
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
fn references_document_research_and_unwired_provider_boundaries() {
    let playbook =
        load("cas-cli/src/builtins/skills/cas-image-generate/references/asset-playbook.md");
    let providers = load("cas-cli/src/builtins/skills/cas-image-generate/references/providers.md");
    let output =
        load("cas-cli/src/builtins/skills/cas-image-generate/references/output-checklist.md");
    let style = load("cas-cli/src/builtins/skills/cas-image-generate/references/style-harvest.md");

    for marker in [
        "image-generation-dossier.md",
        "gemini-3.1-flash-image",
        "gemini-3-pro-image",
        "Recraft",
        "gpt-image-2",
        "Ideogram 3.0",
        "FLUX.2",
        "unwired",
        "Imagen",
    ] {
        assert!(
            providers.contains(marker) || playbook.contains(marker),
            "research/provider reference missing {marker:?}"
        );
    }
    for marker in [
        "DESIGN.md",
        "Tailwind",
        "CSS custom properties",
        "palette",
        "typography",
        "motif",
    ] {
        assert!(style.contains(marker), "style reference missing {marker:?}");
    }
    let output_lower = output.to_ascii_lowercase();
    for marker in [
        "1200x630",
        "transparent",
        "license",
        "favicon",
        "webp",
        "svg",
    ] {
        assert!(
            output_lower.contains(marker),
            "output reference missing {marker:?}"
        );
    }
}

#[test]
fn builtin_catalog_registers_all_mirror_entries() {
    let builtins = load("cas-cli/src/builtins.rs");
    for include_path in [
        "builtins/skills/cas-image-generate/SKILL.md",
        "builtins/codex/skills/cas-image-generate/SKILL.md",
        "builtins/grok/skills/cas-image-generate/SKILL.md",
        "builtins/skills/cas-image-generate/references/asset-playbook.md",
        "builtins/codex/skills/cas-image-generate/references/asset-playbook.md",
        "builtins/grok/skills/cas-image-generate/references/asset-playbook.md",
        "builtins/skills/cas-image-generate/references/svg-web-assets.md",
        "builtins/codex/skills/cas-image-generate/references/svg-web-assets.md",
        "builtins/grok/skills/cas-image-generate/references/svg-web-assets.md",
        "builtins/skills/cas-image-generate/scripts/generate-image.sh",
        "builtins/codex/skills/cas-image-generate/scripts/generate-image.sh",
        "builtins/grok/skills/cas-image-generate/scripts/generate-image.sh",
    ] {
        assert!(
            builtins.contains(include_path),
            "missing catalog include {include_path}"
        );
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
    let script = repo_root()
        .join("cas-cli/src/builtins/skills/cas-image-generate/scripts/generate-image.sh");
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
    let script = repo_root()
        .join("cas-cli/src/builtins/skills/cas-image-generate/scripts/generate-image.sh");
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
