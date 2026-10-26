//! `cas deliverable` verbs through the real CLI.

use std::fs;
use std::path::Path;
use std::process::Command as ProcessCommand;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn cas_cmd(project: &Path) -> Command {
    let home = project.join(".test-home");
    let xdg = project.join(".test-xdg-config");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&xdg).unwrap();
    let mut command = Command::new(cas::test_paths::cas_binary());
    command
        .current_dir(project)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", xdg)
        .env_remove("CAS_ROOT")
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    command
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = ProcessCommand::new("git")
        .current_dir(root)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.org"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// Pulse Card's shape: DESIGN.md under apps/frontend, SCSS sources (no CSS
/// the report can inline), design-spec value keys and no local .woff2. It must
/// resolve to the project's own tokens, type and mark, never Petrastella's.
#[test]
fn deliverable_brand_resolves_pulse_card_shaped_fixture() {
    let project = TempDir::new().unwrap();
    let root = project.path();
    let app = root.join("apps/frontend");
    fs::create_dir_all(app.join("styles")).unwrap();
    fs::create_dir_all(app.join("public")).unwrap();
    fs::write(app.join("styles/theme.scss"), "$primary: #232124;\n").unwrap();
    fs::write(
        app.join("public/mark.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .unwrap();
    fs::write(
        app.join("DESIGN.md"),
        "---\nversion: alpha\nname: Pulsecard 2026\nsource:\n  - apps/frontend/styles/theme.scss\n\
         inherits: none\nbrand-mark: apps/frontend/public/mark.svg\ncolors:\n  bg: '#fffdf3'\n\
         \x20 surface: '#fffdf3'\n  line: 'rgba(35, 33, 36, 0.14)'\n  ink: '#232124'\n  ink-muted: '#6b6670'\n\
         \x20 verdict: '#ff682f'\n  action: '#232124'\ntypography:\n\
         \x20 body1: { fontFamily: Satoshi, fontSize: 1rem, fontWeight: 400 }\n\
         \x20 display: { fontFamily: Bely, fontWeight: 400 }\nrounded:\n  card: 16px\n  pill: 999px\n\
         maps:\n  bg: --pc-bg\n  action: $primary\n---\n\n# Pulsecard\n",
    )
    .unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "design"]);
    let revision = git(
        root,
        &["log", "-1", "--format=%h", "--", "apps/frontend/DESIGN.md"],
    );

    let output = cas_cmd(root)
        .args([
            "--json",
            "deliverable",
            "brand",
            "--audience",
            "customer",
            "--css-out",
            "out/brand.css",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let receipt: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(receipt["source"], "design-md-values", "{receipt}");
    assert_eq!(receipt["design"]["path"], "apps/frontend/DESIGN.md");
    assert_eq!(receipt["design"]["revision"], revision.as_str());
    assert_eq!(receipt["mark"]["path"], "apps/frontend/public/mark.svg");
    assert_eq!(receipt["findings"], Value::Array(vec![]));
    let body = &receipt["type"]["body"];
    assert_eq!(body["stack"][0], "Satoshi");
    assert_eq!(body["face"], Value::Null);
    assert_eq!(receipt["type"]["display"]["stack"][0], "Bely");
    // With no local .woff2 the receipt names the stack the text falls back to.
    assert!(
        receipt["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains(
                "no local .woff2 for Satoshi; body text falls back to: Satoshi, ui-sans-serif"
            )),
        "{receipt}"
    );
    let css = fs::read_to_string(root.join("out/brand.css")).unwrap();
    assert!(css.contains("--verdict:#ff682f"), "{css}");
    assert!(css.contains("--line:rgba(35, 33, 36, 0.14)"), "{css}");
    assert!(css.contains("--font-body:Satoshi,ui-sans-serif"), "{css}");
    assert!(css.contains("--radius-panel:16px"), "{css}");
    assert!(
        !css.contains("#F7F4EE"),
        "Petrastella default leaked: {css}"
    );
}

/// A customer deliverable never wears the bundled defaults: with no DESIGN.md
/// it exits 1 with a finding, where an internal one falls back.
#[test]
fn deliverable_brand_customer_without_design_md_is_a_finding() {
    let project = TempDir::new().unwrap();
    let assert = cas_cmd(project.path())
        .args(["--json", "deliverable", "brand", "--audience", "customer"])
        .assert()
        .code(1);
    let receipt: Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(receipt["source"], Value::Null, "{receipt}");
    assert_eq!(receipt["findings"][0]["code"], "no-design-md", "{receipt}");

    let internal = cas_cmd(project.path())
        .args(["--json", "deliverable", "brand"])
        .assert()
        .success();
    let receipt: Value = serde_json::from_slice(&internal.get_output().stdout).unwrap();
    assert_eq!(receipt["source"], "defaults", "{receipt}");
}
