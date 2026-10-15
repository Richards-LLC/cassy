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
fn producer_ids(repo: &Path, base: &str, head: &str) -> Vec<String> {
    let mut command = Command::new("python3");
    command.arg(repo.join("scripts/journeys-for-diff.py"));
    command.args([base, head]);
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
            journeys_for_paths_at(repo, &head, &["hub-web/src/theme.ts".into()]).is_err(),
            "broken committed selector cannot use a healthy checkout selector"
        );
    }
    git_ok(repo, &["checkout", "-q", &base]);
    git_ok(repo, &["rm", "-q", "scripts/journeys-for-diff.py"]);
    let head = commit(repo);
    git_ok(repo, &["checkout", "-q", &older]);
    assert!(journeys_for_paths_at(repo, &head, &["hub-web/src/theme.ts".into()]).is_err());
}

/// cas-8132: a delivery's "journeys:" reason (qa_pass::catalog_journeys_for)
/// read the store checkout's working tree. The store sat on an old main whose
/// catalog listed fewer journeys, so the gate named HUB-J1..J11 while
/// journey-eval at the tip selected others. With the delivered head it now
/// agrees with the producer at that head, from an older checkout.
#[test]
fn cas_8132_journeys_reason_comes_from_the_delivered_head() {
    let (temp, older, base) = fixture();
    let repo = temp.path();
    for (path, body) in [
        ("hub-web/src/delivery.ts", "export const delivery = 'held';\n"),
        ("hub-web/src/theme.ts", "export const theme = 'dark';\n"),
        ("hub-web/e2e/delivery.journey.ts", "test('HUB-J12 delivery changed', () => {});\n"),
        ("hub-web/dist/app.js", "derived bundle\n"),
    ] {
        git_ok(repo, &["checkout", "-q", &base]);
        write(repo, path, body);
        let head = commit(repo);
        let producer = producer_ids(repo, &base, &head);
        let paths = vec![path.to_string()];
        git_ok(repo, &["checkout", "-q", &older]);
        assert_eq!(
            super::journeys_for_paths_at(repo, &head, &paths).unwrap(),
            producer,
            "journeys reason for {path} must come from the delivered head"
        );
        // The "journeys:" eligibility reason judges surface paths only: a
        // spec-only change is a test (cas-0cd5 is_non_surface_path), never on
        // its own a user-facing delivery, though journey-eval still runs it.
        let expected_reason: Vec<String> = if crate::qa_pass::is_non_surface_path(path) { Vec::new() } else { producer.clone() };
        assert_eq!(
            crate::qa_pass::catalog_journeys_for(repo, Some(&head), &paths),
            expected_reason,
            "catalog_journeys_for with a head agrees with the producer for surface path {path}"
        );
        // The old checkout's own selector would name its whole two-journey
        // catalog for any surface path: the skew this pins against.
        let unpinned = crate::qa_pass::catalog_journeys_for(repo, None, &paths);
        if path != "hub-web/dist/app.js" && !crate::qa_pass::is_non_surface_path(path) {
            assert_eq!(unpinned, ["HUB-J1", "HUB-J10"], "older checkout working tree for {path}");
        }
    }
    // A head without a catalog names no journeys rather than borrowing the checkout's.
    git_ok(repo, &["checkout", "-q", &base]);
    git_ok(repo, &["rm", "-q", "docs/qa/journeys.md"]);
    let head = commit(repo);
    git_ok(repo, &["checkout", "-q", &older]);
    assert!(super::journeys_for_paths_at(repo, &head, &["hub-web/src/theme.ts".into()]).unwrap().is_empty());
}
