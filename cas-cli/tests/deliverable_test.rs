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

/// A small project whose DESIGN.md gives a customer brand, plus a draft with
/// the brand marker and relative images beside it.
fn finalize_fixture(body: &str, images: &[(&str, usize)]) -> TempDir {
    let project = TempDir::new().unwrap();
    let root = project.path();
    fs::write(
        root.join("DESIGN.md"),
        "---\nname: Northwind\ncolors:\n  bg: '#fffdf3'\n  ink: '#232124'\n  verdict: '#ff682f'\n---\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("report/shots")).unwrap();
    for (name, bytes) in images {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.resize(*bytes, b'x');
        fs::write(root.join("report/shots").join(name), png).unwrap();
    }
    fs::write(
        root.join("report/draft.html"),
        format!("<!doctype html><html><head><style>/*cas:brand*/</style></head><body>{body}</body></html>"),
    )
    .unwrap();
    project
}

/// Customer hygiene refuses a ticket label that internal output may carry;
/// both runs brand and inline the draft into one file.
#[test]
fn deliverable_finalize_customer_hygiene_refuses_a_planted_ticket_label() {
    let project = finalize_fixture(
        "<h1>Round 2</h1><p>The export shipped in cas-1a2b.</p><figure><img src=\"shots/r1.png\" alt=\"Export\"></figure>",
        &[("r1.png", 2_000)],
    );
    let run = |audience: &str| {
        cas_cmd(project.path())
            .args([
                "--json",
                "deliverable",
                "finalize",
                "report/draft.html",
                "--audience",
                audience,
            ])
            .args(["--out", &format!("report/{audience}.html")])
            .assert()
    };
    let customer = run("customer").code(1);
    let receipt: Value = serde_json::from_slice(&customer.get_output().stdout).unwrap();
    assert_eq!(receipt["findings"][0]["code"], "hygiene", "{receipt}");
    let message = receipt["findings"][0]["message"].as_str().unwrap();
    assert!(message.contains("ticket label 'cas-1a2b'"), "{message}");
    // A refused customer file never sits at the deliverable path.
    assert_eq!(receipt["html"], Value::Null, "{receipt}");
    assert!(!project.path().join("report/customer.html").exists());

    let internal = run("internal").success();
    let receipt: Value = serde_json::from_slice(&internal.get_output().stdout).unwrap();
    assert_eq!(receipt["findings"], Value::Array(vec![]), "{receipt}");
    assert_eq!(receipt["inlined"], 1);
    let html = fs::read_to_string(project.path().join("report/internal.html")).unwrap();
    assert!(
        html.contains("--verdict:#ff682f") && !html.contains("/*cas:brand*/"),
        "{html}"
    );
    assert!(html.contains("src=\"data:image/png;base64,") && !html.contains("shots/r1.png"));
}

/// Over the report image budget, the lowest-priority pair goes as a whole and
/// the receipt names it; a pair never leaves one side behind.
#[test]
fn deliverable_finalize_drops_the_lowest_priority_pair_over_budget() {
    let pair = |id: &str, priority: u8| {
        format!(
            "<p>Change {id}</p><figure class=\"captures\" data-priority=\"{priority}\">\
             <img src=\"shots/{id}.before.png\" alt=\"Before\"><img src=\"shots/{id}.after.png\" alt=\"After\"></figure>"
        )
    };
    let project = finalize_fixture(
        &(pair("a", 1) + &pair("b", 0) + &pair("c", 1)),
        &[
            ("a.before.png", 30_000),
            ("a.after.png", 30_000),
            ("b.before.png", 30_000),
            ("b.after.png", 30_000),
            ("c.before.png", 30_000),
            ("c.after.png", 30_000),
        ],
    );
    let output = cas_cmd(project.path())
        .args([
            "--json",
            "deliverable",
            "finalize",
            "report/draft.html",
            "--audience",
            "internal",
        ])
        .args(["--out", "report/final.html", "--max-images-kb", "150"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let receipt: Value = serde_json::from_slice(&output).unwrap();
    let dropped = receipt["dropped"].as_array().unwrap();
    assert_eq!(dropped.len(), 1, "{receipt}");
    assert_eq!(
        dropped[0]["refs"],
        serde_json::json!(["shots/b.before.png", "shots/b.after.png"])
    );
    let html = fs::read_to_string(project.path().join("report/final.html")).unwrap();
    assert!(
        html.contains("<p>Change b</p><div class=\"missing\">"),
        "{html}"
    );
    assert_eq!(html.matches("data:image/png;base64,").count(), 4);
}
