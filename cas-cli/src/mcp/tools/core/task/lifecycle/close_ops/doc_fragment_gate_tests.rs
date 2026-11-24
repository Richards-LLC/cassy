use super::super::receipt_scope::park_refusal;
use super::*;

const REGISTRY: &str = "version: 1\njourneys:\n  - id: J-home\n    name: Home\n    status: live\n    owners: {frontend: [\"hub-web/src/home/**\"]}\n";
const FAMILY: &str = "title: Home lost its state\ntasks: [cas-3cf7x]\njourneys: [J-home]\n";

/// cas-faa9 at accb4b2bce parked product source with no overview fragment and
/// the verifier refused it a round later. Each row replays one delivery
/// through the park reader and names the G-167/G-168 codes it must get.
#[test]
fn park_names_every_missing_doc_fragment_cas_3cf7() {
    let overview = ("docs/product-overview.d/cas-3cf7x.md", "## Factory\n\n- Fixture.\n");
    let family = ("docs/qa/bug-families/home-state.yaml", FAMILY);
    let decision = |text: &str| format!("[2026-11-24 13:00] ✅ DECISION {text}");
    let rows: &[(&str, TaskType, &[(&str, &str)], String, &[&str])] = &[
        ("product, no overview", TaskType::Task, &[("hub-web/src/app.ts", "2")], String::new(), &["G-167"]),
        ("product with overview", TaskType::Task, &[("hub-web/src/app.ts", "2"), overview], String::new(), &[]),
        ("no-overview decision", TaskType::Task, &[("hub-web/src/app.ts", "2")], decision("no overview: internal refactor"), &[]),
        ("docs and tests only", TaskType::Bug, &[("README.md", "x"), ("hub-web/src/home/view.test.ts", "x")], String::new(), &[]),
        ("feature on a live journey", TaskType::Feature, &[("hub-web/src/home/view.ts", "2"), overview], String::new(), &[]),
        ("bug on a live journey, both missing", TaskType::Bug, &[("hub-web/src/home/view.ts", "2")], String::new(), &["G-167", "G-168"]),
        ("bug with both fragments", TaskType::Bug, &[("hub-web/src/home/view.ts", "2"), overview, family], String::new(), &[]),
        ("no-bug-family decision", TaskType::Bug, &[("hub-web/src/home/view.ts", "2"), overview], decision("no bug family: log line only"), &[]),
    ];
    for (case, task_type, files, notes, expected) in rows {
        let repo = tempfile::tempdir().unwrap();
        let cas_root = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").args(args).current_dir(repo.path()).output().unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        let write = |path: &str, body: &str| {
            let path = repo.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        for (path, body) in [("hub-web/src/app.ts", "1"), ("hub-web/src/home/view.ts", "1"),
            ("docs/product-overview.d/README.md", "Fragments.\n"), ("docs/qa/journeys.yaml", REGISTRY)] {
            write(path, body);
        }
        git(&["add", "."]);
        git(&["commit", "-qm", "base"]);
        git(&["checkout", "-qb", "delivery"]);
        for (path, body) in *files {
            write(path, body);
        }
        git(&["add", "."]);
        git(&["commit", "-qm", "delivery"]);
        let head = git(&["rev-parse", "HEAD"]);
        let mut task = Task::new("cas-3cf7x".into(), "fixture".into());
        task.task_type = *task_type;
        task.notes = notes.clone();
        let paths: Vec<String> = files.iter().map(|(path, _)| path.to_string()).collect();
        let reply = park_refusal(cas_root.path(), repo.path(), &task, &paths, &head, "main", None).unwrap_or_default();
        let codes: Vec<_> = crate::gates::codes_in(&reply).into_iter().filter(|code| matches!(*code, "G-167" | "G-168")).collect();
        assert_eq!(codes, *expected, "{case}: {reply}");
    }
}
