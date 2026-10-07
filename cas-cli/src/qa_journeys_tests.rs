use super::*;

fn git_ok(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "journey fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "journey fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn write(repo: &Path, path: &str, body: &str) {
    let path = repo.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn commit(repo: &Path) -> String {
    git_ok(repo, &["add", "."]);
    git_ok(
        repo,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "journey fixture",
        ],
    );
    git_ok(repo, &["rev-parse", "HEAD"])
}

fn catalog(ids: &[(u32, &str)]) -> String {
    let mut text = "# Catalog\n\n## hub-web\n\n- **Surface-wide:** `hub-web/src/main.ts`, `hub-web/dist/*`\n\n".to_owned();
    for (id, name) in ids {
        text.push_str(&format!(
            "### HUB-J{id} · {name}\n\n- **Touches:** `hub-web/src/{name}.ts`\n- **Suite:** `hub-web/e2e/{name}.journey.ts`\n\n"
        ));
    }
    text
}

const SELECTOR: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../scripts/journeys-for-diff.py"
));

/// An older checkout has its own selector and catalog. Like the pre-source-
/// graph selector, it reads that checkout and does not understand HEAD/BASE.
const OLD_SELECTOR: &str = r#"
import json, os, re
from pathlib import Path
text = (Path(os.environ['CAS_JOURNEYS_ROOT']) / 'docs/qa/journeys.md').read_text()
print(json.dumps({'journeys': [{'id': ident} for ident in re.findall(r'^### (HUB-J\d+)', text, re.M)]}))
"#;

fn fixture() -> (tempfile::TempDir, String, String) {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    git_ok(repo, &["init", "-q"]);
    write(repo, "scripts/journeys-for-diff.py", OLD_SELECTOR);
    write(
        repo,
        "docs/qa/journeys.md",
        &catalog(&[(1, "reply"), (10, "theme")]),
    );
    let older = commit(repo);
    write(repo, "scripts/journeys-for-diff.py", SELECTOR);
    write(
        repo,
        "docs/qa/journeys.md",
        &catalog(&[(1, "reply"), (10, "theme"), (12, "delivery")]),
    );
    write(
        repo,
        "hub-web/src/reply.ts",
        "import { delivery } from './delivery'; export const reply = `<div class=\"reply-card\">${delivery}</div>`;\n",
    );
    write(
        repo,
        "hub-web/src/delivery.ts",
        "export const delivery = 'sent';\n",
    );
    write(
        repo,
        "hub-web/src/theme.ts",
        "export const theme = 'light';\n",
    );
    write(
        repo,
        "hub-web/src/main.ts",
        "import { reply } from './reply';\nimport { theme } from './theme';\nfunction renderReply() { return reply; }\nfunction renderTheme() { return theme; }\n",
    );
    write(
        repo,
        "hub-web/src/styles.css",
        ".reply-card { color: blue; }\n",
    );
    for (id, name) in [(1, "reply"), (10, "theme"), (12, "delivery")] {
        write(
            repo,
            &format!("hub-web/e2e/{name}.journey.ts"),
            &format!("test('HUB-J{id} {name}', () => {{}});\n"),
        );
    }
    let base = commit(repo);
    (temp, older, base)
}

/// The producer's public diff CLI at the reviewed checkout is an independent
/// oracle for the Rust gate invoked from an older physical checkout.
fn producer_ids(repo: &Path, base: &str, head: &str, full: bool) -> Vec<String> {
    let mut command = Command::new("python3");
    command.arg(repo.join("scripts/journeys-for-diff.py"));
    if full {
        command.arg("--all");
    } else {
        command.args([base, head]);
    }
    let out = command
        .current_dir(repo)
        .env("CAS_JOURNEYS_ROOT", repo)
        .env("CAS_JOURNEYS_BASE", base)
        .env("CAS_JOURNEYS_HEAD", head)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let mut ids: Vec<String> = value["journeys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    ids
}

#[test]
fn cas_f365_gate_ids_match_reviewed_selector_across_diffs_and_checkouts() {
    let (temp, older, base) = fixture();
    let repo = temp.path();
    let changed_catalog = catalog(&[(1, "reply"), (10, "theme"), (12, "delivery"), (99, "new")]);
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "hub-web/src/delivery.ts",
            "export const delivery = 'confirmed';\n",
            &["HUB-J1", "HUB-J12"],
        ),
        (
            "hub-web/src/main.ts",
            "import { reply } from './reply';\nimport { theme } from './theme';\nfunction renderReply() { return reply + 'changed'; }\nfunction renderTheme() { return theme; }\n",
            &["HUB-J1"],
        ),
        (
            "hub-web/src/styles.css",
            ".reply-card { color: red; }\n",
            &["HUB-J1"],
        ),
        (
            "hub-web/src/tokens.css",
            ":root { --ink: black; }\n",
            &["HUB-J1", "HUB-J10", "HUB-J12"],
        ),
        (
            "hub-web/tsconfig.json",
            "{}\n",
            &["HUB-J1", "HUB-J10", "HUB-J12"],
        ),
        (
            "hub-web/src/unknown.ts",
            "export const unknown = true;\n",
            &["HUB-J1", "HUB-J10", "HUB-J12"],
        ),
        (
            "hub-web/e2e/delivery.journey.ts",
            "test('HUB-J12 delivery changed', () => {});\n",
            &["HUB-J12"],
        ),
        (
            "docs/qa/journeys.md",
            &changed_catalog,
            &["HUB-J1", "HUB-J10", "HUB-J12", "HUB-J99"],
        ),
        ("hub-web/dist/app.js", "derived bundle\n", &[]),
        ("docs/change.md", "Documentation only\n", &[]),
    ];
    for (path, body, expected) in cases {
        git_ok(repo, &["checkout", "-q", &base]);
        write(repo, path, body);
        let head = commit(repo);
        let producer = producer_ids(repo, &base, &head, false);
        let expected: Vec<String> = expected.iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(producer, expected, "producer fixture {path}");
        let paths: Vec<String> = git_ok(repo, &["diff", "--name-only", &base, &head])
            .lines()
            .map(str::to_owned)
            .collect();
        git_ok(repo, &["checkout", "-q", &older]);
        assert_eq!(
            select_journeys(repo, &base, &head, Some(&paths)).unwrap(),
            producer,
            "gate/producer skew for {path} with older selector/catalog checkout"
        );
    }
}

#[test]
fn cas_f365_full_catalog_comes_from_reviewed_revision() {
    let (temp, older, head) = fixture();
    let repo = temp.path();
    let producer = producer_ids(repo, &older, &head, true);
    assert_eq!(producer, ["HUB-J1", "HUB-J10", "HUB-J12"]);
    git_ok(repo, &["checkout", "-q", &older]);
    assert_eq!(
        select_journeys(repo, &older, &head, None).unwrap(),
        producer
    );
}

#[test]
fn cas_f365_reviewed_selector_failure_cannot_fall_back_to_checkout() {
    let (temp, older, base) = fixture();
    let repo = temp.path();
    for body in [
        "raise SystemExit(7)\n",
        "print('not JSON')\n",
        "print('{\"journeys\": [{\"id\": \"HUB-J1\"}, {\"id\": \"HUB-J1\"}]}')\n",
    ] {
        git_ok(repo, &["checkout", "-q", &base]);
        write(repo, "scripts/journeys-for-diff.py", body);
        let head = commit(repo);
        git_ok(repo, &["checkout", "-q", &older]);
        assert!(
            select_journeys(repo, &base, &head, None).is_err(),
            "broken committed selector cannot use a healthy checkout selector"
        );
    }
    git_ok(repo, &["checkout", "-q", &base]);
    git_ok(repo, &["rm", "-q", "scripts/journeys-for-diff.py"]);
    let head = commit(repo);
    git_ok(repo, &["checkout", "-q", &older]);
    assert!(select_journeys(repo, &base, &head, None).is_err());
}
