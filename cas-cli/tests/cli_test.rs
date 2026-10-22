//! CLI integration tests

use assert_cmd::Command;
use predicates::prelude::*;
use serde::Deserialize;
use tempfile::TempDir;

fn cas_cmd(root: &std::path::Path) -> Command {
    let mut cmd = Command::new(cas::test_paths::cas_binary());
    let home = root.join(".test-home");
    let xdg = root.join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home).env("XDG_CONFIG_HOME", xdg);
    // Clear CAS_ROOT to prevent env pollution from parent shell
    // Tests should use current_dir() for isolation, not inherit env vars
    cmd.env_remove("CAS_ROOT");
    cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

#[test]
fn test_help() {
    let temp = TempDir::new().unwrap();
    cas_cmd(temp.path())
        .arg("--help")
        .assert()
        .success()
        // cas-a0f8 made global help ASCII so it reads under LC_ALL=C.
        .stdout(predicate::str::contains(
            "Cassy - a multi-agent coding factory with persistent memory and task coordination",
        ));
}

#[test]
fn test_version() {
    let temp = TempDir::new().unwrap();
    cas_cmd(temp.path())
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("cas"));
}

#[test]
fn qa_candidate_plan_and_receipt_use_project_artifacts_cas_31fab() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    assert!(std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    std::fs::write(root.join("README.md"), "candidate fixture\n").unwrap();
    assert!(std::process::Command::new("git")
        .args(["add", "README.md"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    assert!(std::process::Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-qm", "fixture"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    cas_cmd(root).current_dir(root).args(["init", "--yes"]).assert().success();
    let artifact_base = root.join("task-artifacts");
    std::fs::write(
        root.join(".cas/config.toml"),
        format!("[factory]\nartifacts_root = {:?}\n", artifact_base.display().to_string()),
    ).unwrap();
    let output = cas_cmd(root)
        .current_dir(root)
        .args(["qa", "candidate", "build", "--task", "cas-0227", "--plan", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let candidate = std::path::PathBuf::from(plan["candidate_dir"].as_str().unwrap());
    assert!(candidate.starts_with(&artifact_base), "{plan}");
    assert!(candidate.ends_with("cas-0227/candidate"), "{plan}");
    assert_eq!(plan["binary"], candidate.join("cas").display().to_string());
    assert_eq!(plan["version_file"], candidate.join("VERSION.txt").display().to_string());
    assert_eq!(plan["checksums_file"], candidate.join("SHA256SUMS").display().to_string());
    assert_eq!(plan["receipt_file"], candidate.join("RECEIPT.json").display().to_string());
    assert_eq!(plan["revision"].as_str().unwrap().len(), 40);
    assert!(!candidate.exists(), "planning must not build or create evidence");

    // cas-d0e6: the receipt names the short form, and a worker resolves the
    // binary by task id alone instead of typing the hashed artifacts path.
    let text_plan = cas_cmd(root)
        .current_dir(root)
        .args(["qa", "candidate", "build", "--task", "cas-0227", "--plan"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&text_plan.stdout).contains("cas qa candidate path --task cas-0227"),
        "{}",
        String::from_utf8_lossy(&text_plan.stdout)
    );
    let path = |role: &str| {
        cas_cmd(root)
            .current_dir(root)
            .env("CAS_AGENT_ROLE", role)
            .args(["qa", "candidate", "path", "--task", "cas-0227"])
            .output()
            .unwrap()
    };
    let binary = candidate.join("cas").display().to_string();
    let missing = path("worker");
    assert!(!missing.status.success(), "no candidate has been built yet");
    assert!(String::from_utf8_lossy(&missing.stderr).contains(&binary), "{missing:?}");
    std::fs::create_dir_all(&candidate).unwrap();
    std::fs::write(candidate.join("cas"), "built candidate\n").unwrap();
    let found = path("worker");
    assert!(found.status.success(), "{}", String::from_utf8_lossy(&found.stderr));
    assert_eq!(String::from_utf8(found.stdout).unwrap().trim(), binary);

    // No compiler runs: exercise the real wrapper with a builder boundary
    // that publishes the same manifest as the Python builder, plus Git chatter.
    let build = |json: bool| {
        let mut command = cas_cmd(root);
        command.current_dir(root)
            .env("CAS_AGENT_ROLE", "supervisor")
            .args(["qa", "candidate", "build", "--task", "cas-0227"]);
        if json {
            command.arg("--json");
        }
        command.output().unwrap()
    };
    let missing_builder = build(true);
    assert!(!missing_builder.status.success());
    let script = root.join("scripts/qa-candidate-build.py");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, r#"
import argparse, json
from pathlib import Path
parser = argparse.ArgumentParser()
parser.add_argument('--repo')
parser.add_argument('--rev')
parser.add_argument('--candidate-dir')
args = parser.parse_args()
destination = Path(args.candidate_dir)
destination.mkdir(parents=True, exist_ok=True)
receipt = dict(revision=args.rev, version='cas 9.99.7', sha256='fixture-hash')
(destination / 'RECEIPT.json').write_text(json.dumps(receipt))
print('HEAD is now at fixture')
print(json.dumps(dict(receipt, candidate_dir=str(destination))))
"#).unwrap();
    let built = build(true);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let receipt: serde_json::Value = serde_json::from_slice(&built.stdout).unwrap();
    assert_eq!(receipt["revision"], plan["revision"]);
    assert_eq!(receipt["candidate_dir"], plan["candidate_dir"]);
    assert_eq!(receipt["receipt_file"], plan["receipt_file"]);
    assert!(!String::from_utf8_lossy(&built.stderr).contains("[ERROR]"));
    let human = build(false);
    assert!(human.status.success(), "{}", String::from_utf8_lossy(&human.stderr));
    assert!(String::from_utf8_lossy(&human.stdout).contains(&format!(
        "Receipt: {}", candidate.join("RECEIPT.json").display()
    )), "{human:?}");

    let missing_error = String::from_utf8_lossy(&missing_builder.stderr);
    assert!(missing_error.contains("QA candidate builder missing"), "{missing_error}");
    assert!(missing_error.contains("source checkout") && missing_error.contains("rerun"), "{missing_error}");
}

#[test]
fn qa_journeys_check_reports_debt_kinds_and_growth_cas_cb2b() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(root)
            .status()
            .unwrap()
            .success()
    );
    cas_cmd(root)
        .current_dir(root)
        .args(["init", "--yes"])
        .assert()
        .success();
    let registry = root.join("docs/qa/journeys.yaml");
    std::fs::create_dir_all(registry.parent().unwrap()).unwrap();
    std::fs::write(root.join("page.ts"), "export {};\n").unwrap();
    let seeded = "version: 1\nupdated: '2026-10-21'\ninvariants:\n  ACCOUNT: {statement: Account exists, check: Read account}\njourneys:\n  - id: signup\n    status: live\n    seeded: true\n    entry: {url: /signup, trigger: Join}\n    owners: ['page.ts']\n";
    std::fs::write(&registry, seeded).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["add", "page.ts", "docs/qa/journeys.yaml"])
            .current_dir(root)
            .status()
            .unwrap()
            .success()
    );
    let human = |expected: &str| {
        let output = cas_cmd(root)
            .current_dir(root)
            .args(["qa", "journeys", "--check"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let output = String::from_utf8(output).unwrap();
        assert!(output.starts_with(expected), "{output}");
        assert!(
            output.lines().all(|line| line.chars().count() <= 80),
            "{output}"
        );
        output
    };
    let json = || {
        let output = cas_cmd(root)
            .current_dir(root)
            .args(["qa", "journeys", "--check", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice::<serde_json::Value>(&output).unwrap()
    };
    let first = human("DEBT: 0 problem(s), 1 debt item(s)\nDebt by kind:\n  seeded: 1\n");
    assert!(first.contains("first recorded check"), "{first}");
    assert!(
        !first.contains("DEBT signup"),
        "individual rows belong under --verbose: {first}"
    );

    std::fs::write(
        &registry,
        format!("{seeded}    traffic: {{share_pct: null}}\n"),
    )
    .unwrap();
    let increased = json();
    assert_eq!(increased["problems"], serde_json::json!([]));
    assert_eq!(increased["debt"].as_array().unwrap().len(), 2);
    assert_eq!(
        increased["debt_summary"]["by_kind"],
        serde_json::json!({"seeded": 1, "unmeasured_traffic": 1})
    );
    assert_eq!(increased["debt_summary"]["previous_total"], 1);
    assert_eq!(increased["debt_summary"]["delta"], 1);
    assert_eq!(increased["debt_summary"]["grew"], true);
    assert_eq!(increased["debt_summary"]["history_status"], "recorded");

    // Remove and re-add traffic debt so the human renderer also observes growth.
    std::fs::write(&registry, seeded).unwrap();
    let decreased = json();
    assert_eq!(decreased["debt_summary"]["delta"], -1);
    assert_eq!(decreased["debt_summary"]["grew"], false);
    std::fs::write(
        &registry,
        format!("{seeded}    traffic: {{share_pct: null}}\n"),
    )
    .unwrap();
    let grew = human(
        "DEBT: 0 problem(s), 2 debt item(s)\nDebt by kind:\n  seeded: 1\n  unmeasured_traffic: 1\n",
    );
    assert!(
        grew.contains("Debt grew: +1 since previous check (1 -> 2)"),
        "{grew}"
    );
    let unchanged = json();
    assert_eq!(unchanged["debt_summary"]["previous_total"], 2);
    assert_eq!(unchanged["debt_summary"]["delta"], 0);
    assert_eq!(unchanged["debt_summary"]["grew"], false);

    // A corrupt receipt must restart history, not poison every future check.
    let connection = rusqlite::Connection::open(root.join(".cas/cas.db")).unwrap();
    let key: String = connection.query_row(
        "SELECT key FROM metadata WHERE key LIKE 'qa_journeys_check:%'", [], |row| row.get(0),
    ).unwrap();
    connection.execute("UPDATE metadata SET value = '{' WHERE key = ?1", [&key]).unwrap();
    let recovered = cas_cmd(root)
        .current_dir(root)
        .args(["qa", "journeys", "--check", "--verbose"])
        .assert().success().get_output().stdout.clone();
    let recovered = String::from_utf8(recovered).unwrap();
    assert!(recovered.contains("first recorded check")
        && recovered.contains("History reset:")
        && !recovered.contains("Details: cas qa journeys --check --verbose"), "{recovered}");
    assert!(recovered.lines().all(|line| line.chars().count() <= 80), "{recovered}");
    let after_recovery = json();
    assert_eq!(after_recovery["debt_summary"]["history_status"], "recorded");
    assert_eq!(after_recovery["debt_summary"]["previous_total"], 2);
    assert_eq!(after_recovery["debt_summary"]["delta"], 0);

    let alternate = root.join("docs/qa/alternate.yaml");
    std::fs::write(&alternate, seeded).unwrap();
    cas_cmd(root)
        .current_dir(root)
        .args([
            "config",
            "set",
            "qa.registry.path",
            "docs/qa/alternate.yaml",
        ])
        .assert()
        .success();
    let other_registry = json();
    assert!(
        other_registry["debt_summary"]["previous_total"].is_null(),
        "{other_registry}"
    );
    assert!(
        other_registry["debt_summary"]["delta"].is_null(),
        "{other_registry}"
    );
}

#[test]
fn upkeep_status_and_enforce_transition_warn_on_integration_drift_cas_a77e() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "upkeep fixture")
            .env("GIT_COMMITTER_NAME", "upkeep fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let write = |path: &str, body: &str| {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    };
    let commit = || {
        git(&["add", ".claude/CODEMAP.md", "docs", "pages"]);
        git(&["-c", "commit.gpgsign=false", "commit", "-qm", "fixture"]);
        git(&["rev-parse", "HEAD"])
    };
    git(&["init", "-q", "-b", "main"]);
    cas_cmd(root)
        .current_dir(root)
        .args(["init", "--yes"])
        .assert()
        .success();
    write(
        ".cas/config.toml",
        "[factory]\nepic_base_branch = 'main'\n[upkeep]\ncodemap = 'observe'\nproduct_overview = 'observe'\n[qa.registry]\nenforce = 'observe'\n",
    );
    let overview =
        "# Product\n<!-- cas:project-overview\nwatch_dirs = ['pages']\n-->\n";
    let registry = "version: 1\nupdated: '2026-10-21'\nsurfaces: ['pages/**']\ninvariants:\n  FLOW: {statement: Arrived, check: Read state}\njourneys:\n  - id: home\n    status: live\n    entry: {url: /, trigger: Open}\n    steps: [Arrive]\n    owners: ['pages/base.ts']\n    invariants: [FLOW]\n    traffic: {share_pct: 100, basis: all visits}\n";
    write(".claude/CODEMAP.md", "# Map\n- pages/: entry routes\n");
    write("docs/PRODUCT_OVERVIEW.md", overview);
    write("docs/qa/journeys.yaml", registry);
    write("pages/base.ts", "export {};\n");
    let baseline = commit();
    write("pages/new.ts", "export {};\n");
    let integrated = commit();
    git(&["update-ref", "refs/remotes/origin/main", &integrated]);
    git(&["reset", "--hard", &baseline]);
    let status = || {
        let output = cas_cmd(root)
            .current_dir(root)
            .args(["qa", "journeys", "--status", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice::<serde_json::Value>(&output).unwrap()
    };
    let report = status();
    let measured = format!("origin/main@{integrated}");
    assert_eq!(report["upkeep"]["measured_ref"], measured, "{report}");
    for artifact in ["codemap", "product_overview", "registry"] {
        assert_eq!(
            report["upkeep"][artifact]["drift_count"], 1,
            "{artifact}: {report}"
        );
    }
    let human = cas_cmd(root)
        .current_dir(root)
        .args(["qa", "journeys", "--status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        String::from_utf8(human)
            .unwrap()
            .contains(&format!("Measured ref: {measured}"))
    );
    for (index, key) in [
        "upkeep.codemap",
        "upkeep.product_overview",
        "qa.registry.enforce",
    ]
    .iter()
    .enumerate()
    {
        if index == 0 {
            cas_cmd(root)
                .current_dir(root)
                .args(["config", "set", key, "enforce"])
                .assert()
                .success()
                .stdout(predicate::str::contains("WARNING: CODEMAP drift: 1"));
        } else {
            let output = cas_cmd(root)
                .current_dir(root)
                .args(["config", "set", key, "enforce", "--json"])
                .assert()
                .success()
                .get_output()
                .stdout
                .clone();
            let saved: serde_json::Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(saved["warnings"][0]["drift_count"], 1, "{saved}");
            assert_eq!(saved["warnings"][0]["measured_ref"], measured, "{saved}");
        }
        let output = cas_cmd(root)
            .current_dir(root)
            .args(["config", "set", key, "enforce", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let repeated: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(
            repeated["warnings"],
            serde_json::json!([]),
            "repeated write: {repeated}"
        );
    }
    git(&["reset", "--hard", &integrated]);
    write(
        ".claude/CODEMAP.md",
        "# Map\n- pages/: base and new routes\n",
    );
    write(
        "docs/PRODUCT_OVERVIEW.md",
        &format!("{overview}\nNew route documented.\n"),
    );
    write(
        "docs/qa/journeys.yaml",
        &registry.replace("'pages/base.ts'", "'pages/**'"),
    );
    let curated = commit();
    git(&["update-ref", "refs/remotes/origin/main", &curated]);
    // An ahead worker must not substitute its unintegrated drift for this report.
    write("pages/worker-only.ts", "export {};\n");
    commit();
    let clean = status();
    assert_eq!(
        clean["upkeep"]["measured_ref"],
        format!("origin/main@{curated}")
    );
    for (key, artifact) in [
        ("upkeep.codemap", "codemap"),
        ("upkeep.product_overview", "product_overview"),
        ("qa.registry.enforce", "registry"),
    ] {
        assert_eq!(clean["upkeep"][artifact]["drift_count"], 0, "{clean}");
        cas_cmd(root)
            .current_dir(root)
            .args(["config", "set", key, "observe"])
            .assert()
            .success();
        let output = cas_cmd(root)
            .current_dir(root)
            .args(["config", "set", key, "enforce", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let saved: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(
            saved["warnings"],
            serde_json::json!([]),
            "zero drift: {saved}"
        );
    }
}

/// The lib-test probe child does not exercise main.rs's early executable route.
/// Both real-binary phases must preserve locks owned by this parent process.
#[cfg(unix)]
#[test]
fn ownership_probe_binary_route_preserves_parent_locks_cas_b7f9() {
    let temp = TempDir::new().unwrap();
    let db = temp.path().join("cas.db");
    let writer = rusqlite::Connection::open(&db).unwrap();
    writer.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
         CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT);
         INSERT INTO tasks VALUES ('cas-aaaa', 'checkpointed');
         PRAGMA wal_checkpoint(TRUNCATE);
         INSERT INTO tasks VALUES ('cas-bbbb', 'wal');
         BEGIN IMMEDIATE; UPDATE tasks SET title='pending';",
    ).unwrap();
    let held_locks = || {
        let output = std::process::Command::new("python3").args(["-I", "-S", "-c", r#"
import fcntl, json, sys
held = []
for path, offset in [(sys.argv[1], 1073741826), (sys.argv[1] + '-shm', 120)]:
    with open(path, 'r+b') as file:
        try:
            fcntl.lockf(file, fcntl.LOCK_EX | fcntl.LOCK_NB, 1, offset)
        except BlockingIOError:
            held.append(True)
        else:
            held.append(False)
print(json.dumps(held))
"#]).arg(&db).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice::<Vec<bool>>(&output.stdout).unwrap()
    };
    let lengths = || (std::fs::metadata(&db).unwrap().len(),
        std::fs::metadata(temp.path().join("cas.db-wal")).unwrap().len());
    let before = lengths();
    assert_eq!(held_locks(), vec![true, true], "real SQLite shared and WAL writer locks");
    for (phase, variant, id) in [("before", "Before", "cas-bbbb"), ("after", "After", "cas-aaaa")] {
        let response = temp.path().join(format!("{phase}.json"));
        // pin: this internal flag is the executable protocol used by isolated_probe.
        cas_cmd(temp.path()).current_dir(temp.path())
            .arg("--internal-task-ownership-probe").arg(phase).arg(&db).arg(&response)
            .timeout(std::time::Duration::from_secs(10))
            .assert().success().stdout("").stderr("");
        let result: serde_json::Value = serde_json::from_slice(&std::fs::read(response).unwrap()).unwrap();
        assert!(result["Ok"][variant]["ids"].as_array().unwrap().iter().any(|value| value == id),
            "{phase} must execute its snapshot phase: {result}");
        assert_eq!(held_locks(), vec![true, true], "{phase} must not release parent locks");
        assert_eq!(lengths(), before, "the inspected database and WAL must not grow");
    }
    writer.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn test_init_yes_flag() {
    let mut redraws = Vec::new();
    // Both human entry paths share execute_step; exercise completion and an
    // actual filesystem failure through their piped CLI, not a renderer copy.
    for interactive in [false, true] {
        for fail_gitignore in [false, true] {
            let temp = TempDir::new().unwrap();
            assert!(!temp.path().join(".git").exists());
            std::fs::create_dir(temp.path().join(".claude")).unwrap();
            if fail_gitignore {
                std::fs::create_dir(temp.path().join(".gitignore")).unwrap();
            }
            let mut cmd = cas_cmd(temp.path());
            cmd.current_dir(&temp)
                .args(["init", "--no-integrations"])
                .env("LC_ALL", "C")
                .env("NO_COLOR", "1");
            if interactive {
                cmd.write_stdin("y\n");
            } else {
                cmd.arg("--yes");
            }
            let output = cmd.output().unwrap();
            assert_eq!(output.status.success(), !fail_gitignore,
                "interactive={interactive}, fail_gitignore={fail_gitignore}: {}",
                String::from_utf8_lossy(&output.stderr));
            let text = String::from_utf8(output.stdout).unwrap();
            assert!(text.contains("Saving configuration"), "{text}");
            assert!(text.contains("Updating .gitignore"), "{text}");
            if text.bytes().any(|byte| byte.is_ascii_control() && !matches!(byte, b'\n' | b'\t'))
                || text.matches("Saving configuration").count() != 1
                || text.matches("Updating .gitignore").count() != 1
            {
                redraws.push(format!("interactive={interactive}, fail_gitignore={fail_gitignore}: {text:?}"));
            }
            assert!(temp.path().join(".cas/cas.db").exists());
            assert!(temp.path().join(".cas/config.toml").exists());
            if !fail_gitignore {
                assert!(text.contains("Cassy initialized"), "{text}");
                assert!(text.contains("cas hub service install"), "{text}");
                cas_cmd(temp.path()).current_dir(&temp).arg("status").assert().success();
            }
        }
    }
    assert!(redraws.is_empty(), "piped phase output must append each completed phase once without spinner or redraw bytes: {redraws:#?}");
}

#[test]
fn cas_de46_startup_preserves_retired_embedding_artifacts() {
    let temp = TempDir::new().unwrap();
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Old local embedding data is no longer produced or consumed by search.
    // Ordinary startup must leave any remaining files for the owner to manage.
    let cas_root = temp.path().join(".cas");
    let fixtures = [
        ("vectors.hnsw", "legacy vector bytes"),
        ("vectors.meta.json", "legacy metadata bytes"),
        ("models/nested/model.bin", "legacy model bytes"),
    ];
    for (relative, content) in fixtures {
        let path = cas_root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    cas_cmd(temp.path())
        .current_dir(&temp)
        .arg("status")
        .assert()
        .success();

    for (relative, content) in fixtures {
        assert_eq!(
            std::fs::read_to_string(cas_root.join(relative)).unwrap(),
            content,
            "startup must preserve {relative}"
        );
    }
}

#[test]
fn test_init_json() {
    for state in ["missing", "existing", "directory", "malformed", "malformed-grok", "array-root", "array-servers", "null-root", "null-servers"] {
        let temp = TempDir::new().unwrap();
        if state == "malformed-grok" {
            std::fs::create_dir(temp.path().join(".codex")).unwrap();
            std::fs::create_dir(temp.path().join(".grok")).unwrap();
        } else {
            std::fs::create_dir(temp.path().join(".claude")).unwrap();
        }
        let mcp_path = temp.path().join(".mcp.json");
        let other_server = serde_json::json!({"command": "other-mcp", "args": ["start"]});
        let invalid_content: Option<&[u8]> = match state {
            "malformed" | "malformed-grok" => Some(b"{\"mcpServers\": {\"other\":"),
            "array-root" => Some(b"[]"),
            "array-servers" => Some(b"{\"mcpServers\":[\"x\"]}"),
            "null-root" => Some(b"null"),
            "null-servers" => Some(b"{\"mcpServers\":null}"),
            _ => None,
        };
        match state {
            "existing" => std::fs::write(
                &mcp_path,
                serde_json::json!({"mcpServers": {"other": other_server}}).to_string(),
            )
            .unwrap(),
            "directory" => std::fs::create_dir(&mcp_path).unwrap(),
            _ if invalid_content.is_some() => std::fs::write(&mcp_path, invalid_content.unwrap()).unwrap(),
            _ => {}
        }

        let output = cas_cmd(temp.path())
            .current_dir(&temp)
            .env("CODEX_HOME", temp.path().join(".test-home/.codex"))
            .args(["init", "--no-integrations", "--json"])
            .output()
            .unwrap();
        if state == "directory" || invalid_content.is_some() {
            assert!(
                output.status.code() == Some(1),
                "MCP provisioning failure must exit 1 without panicking ({state}): {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                output.stdout.is_empty(),
                "failed init must not report initialized"
            );
            assert!(String::from_utf8_lossy(&output.stderr).contains(".mcp.json"));
            if state == "directory" {
                assert!(mcp_path.is_dir(), "failed init must preserve the obstructing directory");
            } else {
                assert_eq!(std::fs::read(&mcp_path).unwrap(), invalid_content.unwrap());
                let error = String::from_utf8_lossy(&output.stderr);
                assert!(error.contains("JSON"), "malformed config needs an explicit parse error: {error}");
            }
            continue;
        }

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["status"], "initialized");
        assert_eq!(report["agents"]["claude"], true);
        assert_eq!(report["hooks_configured"], true);
        let mcp: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&mcp_path).expect("successful Claude init must create .mcp.json"),
        )
        .unwrap();
        // pin: Claude's MCP launch contract must match the durable server entry.
        assert_eq!(
            mcp["mcpServers"]["cas"],
            serde_json::json!({"command": "cas", "args": ["serve"]})
        );
        if state == "existing" {
            assert_eq!(mcp["mcpServers"]["other"], other_server);
        }
    }
}

#[test]
fn test_init_failure_reports_force_retry() {
    let mut missing_hints = Vec::new();
    for harness in ["claude", "codex"] {
        let clean = TempDir::new().unwrap();
        std::fs::create_dir(clean.path().join(format!(".{harness}"))).unwrap();
        cas_cmd(clean.path()).current_dir(&clean)
            .env("CODEX_HOME", clean.path().join(".test-home/.codex"))
            .args(["init", "--json", "--no-integrations"])
            .assert().success();
        let clean_config: toml::Value = toml::from_str(
            &std::fs::read_to_string(clean.path().join(".cas/config.toml")).unwrap(),
        ).unwrap();
        for (mode, retry_mode) in [
            ("json", "json"), ("json", "yes"), ("yes", "yes"), ("interactive", "yes"),
        ] {
            let temp = TempDir::new().unwrap();
            let config_dir = temp.path().join(format!(".{harness}"));
            std::fs::create_dir(&config_dir).unwrap();
            let blocked = if harness == "claude" {
                temp.path().join(".mcp.json")
            } else {
                config_dir.join("config.toml")
            };
            std::fs::create_dir(&blocked).unwrap();
            let user_codex = temp.path().join(".test-home/.codex");
            let mut command = cas_cmd(temp.path());
            command.current_dir(&temp)
                .env("CODEX_HOME", &user_codex)
                .args(["init", "--no-integrations"]);
            match mode {
                "json" => { command.arg("--json"); }
                "yes" => { command.arg("--yes"); }
                _ => { command.write_stdin("y\n"); }
            }
            let output = command.output().unwrap();
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "{harness}/{mode} must fail provisioning");
            assert!(temp.path().join(".cas/cas.db").is_file());
            assert!(blocked.is_dir(), "the user's obstruction must remain intact");
            if !error.lines().any(|line| line == "Fix the reported error, then re-run: cas init --force")
                || error.lines().any(|line| line.chars().count() > 80)
            {
                missing_hints.push(format!("{harness}/{mode}: {error}"));
            }
            std::fs::remove_dir(&blocked).unwrap();
            // Removing the obstruction alone does not finish an existing
            // store. Prove that the suggested force retry actually repairs it.
            cas_cmd(temp.path()).current_dir(&temp)
                .env("CODEX_HOME", &user_codex)
                .args(["init", "--json", "--no-integrations"])
                .assert().success().stdout(predicate::str::contains("already_initialized"));
            assert!(!blocked.exists());
            cas_cmd(temp.path()).current_dir(&temp)
                .env("CODEX_HOME", &user_codex)
                .args(["init", if retry_mode == "json" { "--json" } else { "--yes" }, "--no-integrations", "--force"])
                .assert().success();
            let recovered_config: toml::Value = toml::from_str(
                &std::fs::read_to_string(temp.path().join(".cas/config.toml")).unwrap(),
            ).unwrap();
            assert_eq!(recovered_config, clean_config,
                "{harness}/{mode} -> {retry_mode}: force recovery must keep the full clean-init config");
            if harness == "claude" {
                let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&blocked).unwrap()).unwrap();
                assert_eq!(value["mcpServers"]["cas"]["command"], "cas");
            } else {
                let value: toml::Value = toml::from_str(&std::fs::read_to_string(&blocked).unwrap()).unwrap();
                assert_eq!(value["mcp_servers"]["cs"]["command"].as_str(), Some("cas"));
            }
        }
    }
    assert!(missing_hints.is_empty(), "failed init must explain force recovery: {missing_hints:#?}");
}

#[test]
fn test_init_already_initialized() {
    let temp = TempDir::new().unwrap();

    // First init
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Second init without force should inform user
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already initialized"));

    let config_path = temp.path().join(".cas/config.toml");
    let custom_config = "# operator config\n[issues]\nrepo = \"acme/widgets\"\n";
    std::fs::write(&config_path, custom_config).unwrap();
    let output = cas_cmd(temp.path()).current_dir(&temp)
        .args(["init", "--no-integrations"]).write_stdin("").output().unwrap();
    let error = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1));
    assert!(error.contains("stdin closed before a selection"), "{error}");
    assert!(!error.contains("Initialization is incomplete") && !error.contains("cas init --force"),
        "a valid pre-existing store must not receive partial-init recovery advice: {error}");
    assert_eq!(std::fs::read_to_string(config_path).unwrap(), custom_config);
}

/// Regression test for cas-bf06: a reinit of `cas init` (interactive, no
/// --yes/--json) with an EOF'd stdin must not hang at 100% CPU. Prior to
/// the fix in cas-cli/src/cli/interactive.rs, `select()` looped forever
/// when stdin returned EOF, burning a CPU core and leaving a 0-byte log.
#[test]
fn test_init_reinit_with_closed_stdin_does_not_hang() {
    use std::io::Write;
    use std::process::{Command as StdCommand, Stdio};
    use std::time::{Duration, Instant};

    let temp = TempDir::new().unwrap();

    // First init to make the reinit branch active.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Now run `cas init` interactively (no --yes / --json / --force) with
    // stdin closed. This is the exact call shape that hung in production:
    // `cas init 2>&1 | tail -5` where stdin is the tty but gets EOF'd.
    let bin = cas::test_paths::cas_binary();
    let started = Instant::now();
    let mut child = StdCommand::new(bin)
        .current_dir(&temp)
        .arg("init")
        .env("HOME", temp.path().join(".test-home"))
        .env("XDG_CONFIG_HOME", temp.path().join(".test-xdg-config"))
        .env(
            "CAS_TEST_PROTECTED_HOME",
            std::env::var_os("HOME").unwrap_or_default(),
        )
        .env_remove("CAS_ROOT")
        .env("CAS_SKIP_FACTORY_TOOLING", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn cas");

    // Close stdin immediately — the child will see EOF on its first read.
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"")
        .expect("close stdin");

    // Poll for exit. Fix causes exit within ~1s. Regression would hang
    // until our watchdog fires (5min) or this loop times out (20s).
    let deadline = started + Duration::from_secs(20);
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(s) => break s,
            None if Instant::now() > deadline => {
                let _ = child.kill();
                panic!(
                    "cas init hung for more than 20s with closed stdin — \
                     regression of cas-bf06 (select() infinite loop on EOF)"
                );
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    // The process must terminate with an exit code (i.e., ran to completion
    // and returned), not by signal (SIGKILL/SIGSEGV). Bounded termination
    // is already proven by the poll loop's deadline panic above; this asserts
    // that the termination path is clean rather than a crash or external kill.
    assert!(
        status.code().is_some(),
        "cas init was terminated by signal rather than exiting: {:?}",
        status
    );
}

#[test]
fn test_init_force_reinit() {
    for mode in ["json", "yes", "interactive"] {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir(temp.path().join(".claude")).unwrap();
        cas_cmd(temp.path()).current_dir(&temp)
            .args(["init", "--yes", "--no-integrations"]).assert().success();
        let config_path = temp.path().join(".cas/config.toml");
        let custom_config = "# operator config\n[issues]\nrepo = \"acme/widgets\"\n[hooks]\ntoken_budget = 1234\n";
        std::fs::write(&config_path, custom_config).unwrap();
        let mcp_path = temp.path().join(".mcp.json");
        std::fs::remove_file(&mcp_path).unwrap();

        let mut command = cas_cmd(temp.path());
        command.current_dir(&temp).args(["init", "--force", "--no-integrations"]);
        match mode {
            "json" => { command.arg("--json"); }
            "yes" => { command.arg("--yes"); }
            _ => { command.write_stdin("y\n"); }
        }
        command.assert().success().stdout(predicate::str::contains(
            if mode == "json" { "initialized" } else { "Cassy initialized" }));
        assert_eq!(std::fs::read_to_string(config_path).unwrap(), custom_config,
            "{mode} force retry must preserve the operator's config bytes");
        let mcp: serde_json::Value = serde_json::from_slice(&std::fs::read(mcp_path).unwrap()).unwrap();
        assert_eq!(mcp["mcpServers"]["cas"]["command"], "cas", "{mode} must repair provisioning");
    }
}

/// EPIC cas-8888 (cas-6f46, Phase 5): a pre-existing `.grok/` dir (the
/// opt-in signal `detect_agent_defaults` looks for) must cause `cas init`
/// to sync the Grok builtin skills, whose role guidance names Grok's `cas__` prefix —
/// end-to-end proof that the config.agents.grok wiring actually runs,
/// not just that the underlying sync function works in isolation.
#[test]
fn test_init_json_syncs_grok_builtins_when_grok_dir_present() {
    let temp = TempDir::new().unwrap();
    std::fs::create_dir_all(temp.path().join(".grok")).unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"grok\":true"));

    let worker_skill = temp.path().join(".grok/skills/cas-worker/SKILL.md");
    assert!(
        worker_skill.exists(),
        "cas init with .grok/ present must sync the Grok cas-worker skill twin"
    );
    let content = std::fs::read_to_string(&worker_skill).unwrap();
    assert!(
        content.contains("`cas__` in Grok"),
        "grok cas-worker skill must state the cas__ tool prefix"
    );
    let offenders = cas::builtins::unsanctioned_prefixed_tool_lines(&content);
    assert!(
        offenders.is_empty(),
        "grok cas-worker skill spells a harness prefix outside the naming rule: {offenders:?}"
    );

    // .mcp.json is reused (no separate Grok config writer) — confirm it
    // was still created so Grok's own MCP doctor can find it.
    assert!(temp.path().join(".mcp.json").exists());
}

#[test]
fn test_init_json_already_initialized() {
    let temp = TempDir::new().unwrap();

    // First init
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--json"])
        .assert()
        .success();

    // Second init in JSON mode
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already_initialized"));
}

#[test]
fn test_status() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("entries"));
}

#[test]
fn test_status_verbose() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["status", "-v"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Configuration"));
}

#[test]
fn test_config() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // List config
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sync.enabled"));

    // Get specific value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "sync.enabled"])
        .assert()
        .success()
        .stdout(predicate::str::contains("true"));

    // Set value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "sync.min_helpful", "5"])
        .assert()
        .success();

    // Verify
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "sync.min_helpful"])
        .assert()
        .success()
        .stdout(predicate::str::contains("5"));
}

#[test]
fn test_doctor() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("cas directory"));
}

#[test]
fn test_not_initialized_error() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .arg("status")
        .assert()
        .failure()
        .stderr(predicate::str::contains("no Cassy store here; run `cas init`"));

    // cas-d6ab: styling follows the destination stream, independently of stdout.
    #[cfg(unix)]
    for (stdout_tty, stderr_tty, no_color) in [
        (true, false, false),
        (false, true, false),
        (true, true, true),
    ] {
        use std::fs::File;
        use std::io::{IsTerminal, Read};
        use std::os::fd::FromRawFd;
        use std::process::{Command as ProcessCommand, Stdio};

        let (mut master_fd, mut slave_fd) = (-1, -1);
        // SAFETY: openpty initializes two fresh owned descriptors on success.
        assert_eq!(unsafe { libc::openpty(
            &mut master_fd, &mut slave_fd, std::ptr::null_mut(),
            std::ptr::null_mut(), std::ptr::null_mut(),
        ) }, 0, "{}", std::io::Error::last_os_error());
        let (mut master, slave) = unsafe {
            (File::from_raw_fd(master_fd), File::from_raw_fd(slave_fd))
        };
        assert!(slave.is_terminal(), "fixture must provide a real TTY");
        let mut command = ProcessCommand::new(cas::test_paths::cas_binary());
        command.env_clear()
            .env("HOME", temp.path().join(".test-home"))
            .env("XDG_CONFIG_HOME", temp.path().join(".test-xdg-config"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("LC_ALL", "C")
            .env("CAS_SKIP_FACTORY_TOOLING", "1")
            .current_dir(temp.path()).arg("status")
            .stdin(Stdio::null())
            .stdout(if stdout_tty { Stdio::from(slave.try_clone().unwrap()) } else { Stdio::piped() })
            .stderr(if stderr_tty { Stdio::from(slave.try_clone().unwrap()) } else { Stdio::piped() });
        if no_color { command.env("NO_COLOR", "1"); }
        let output = command.output().unwrap();
        drop(command);
        drop(slave);
        let mut tty_output = Vec::new();
        if stderr_tty {
            // Linux reports EIO at PTY EOF; macOS returns a zero-byte read.
            if let Err(error) = master.read_to_end(&mut tty_output) {
                assert_eq!(error.raw_os_error(), Some(libc::EIO));
            }
        }
        let error = if stderr_tty { &tty_output } else { &output.stderr };
        assert_eq!(output.status.code(), Some(3), "{output:?}");
        assert!(String::from_utf8_lossy(error).contains("no Cassy store here"));
        assert_eq!(error.contains(&0x1b), stderr_tty && !no_color,
            "stderr styling must follow its own TTY and NO_COLOR: stdout_tty={stdout_tty}, stderr_tty={stderr_tty}, no_color={no_color}, stderr={error:?}");
    }
}

#[test]
fn cas_0f22_cli_does_not_use_home_socket_rendezvous_as_project_store() {
    let temp = TempDir::new().unwrap();
    let home = temp.path().join(".test-home");
    let ipc = home.join(".cas");
    let project = home.join("uninitialized-project");
    std::fs::create_dir_all(&ipc).unwrap();
    std::fs::create_dir(&project).unwrap();
    #[cfg(unix)]
    let _socket = std::os::unix::net::UnixListener::bind(ipc.join("factory.sock")).unwrap();
    #[cfg(not(unix))]
    std::fs::write(ipc.join("factory.sock"), "IPC fixture").unwrap();
    cas_cmd(temp.path())
        .env_remove("CAS_CLONE_PATH")
        .current_dir(&project)
        .arg("status")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no Cassy store here; run `cas init`",
        ));
    assert!(
        !ipc.join("cas.db").exists(),
        "lookup must not initialize the IPC directory"
    );
    assert!(
        !project.join(".cas").exists(),
        "lookup must not initialize the project"
    );
}

#[test]
fn test_config_list_offline_no_auth_required() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Ensure local config command remains available without login state.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sync.enabled"));
}

#[test]
fn test_status_offline_no_auth_required() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Ensure local status command remains available without login state.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("entries"));
}

#[test]
fn test_cloud_command_requires_auth() {
    let temp = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .env("HOME", home.path())
        .args(["cloud", "status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Not logged in"));
}

#[test]
fn test_hook_command_session_start() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Test SessionStart hook with JSON input
    let input = r#"{"session_id":"test123","cwd":"/test","hook_event_name":"SessionStart"}"#;

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["hook", "SessionStart"])
        .write_stdin(input)
        .assert()
        .success();
}

#[test]
fn test_hook_config() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Check default hook config
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "hooks.capture_enabled"])
        .assert()
        .success()
        .stdout(predicate::str::contains("true"));

    // Set hook config
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "hooks.capture_enabled", "false"])
        .assert()
        .success();

    // Verify it was set
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "hooks.capture_enabled"])
        .assert()
        .success()
        .stdout(predicate::str::contains("false"));
}

#[test]
fn test_hook_post_tool_use() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Test PostToolUse hook with Write tool
    let input = r#"{
        "session_id": "tool-use-test",
        "cwd": "/test",
        "hook_event_name": "PostToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": "/test/file.rs", "content": "fn main() {}"}
    }"#;

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["hook", "PostToolUse"])
        .write_stdin(input)
        .assert()
        .success();
}

#[test]
fn test_hook_user_prompt_submit() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Test UserPromptSubmit hook
    let input = r#"{
        "session_id": "prompt-test",
        "cwd": "/test",
        "hook_event_name": "UserPromptSubmit",
        "user_prompt": "Help me write tests"
    }"#;

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["hook", "UserPromptSubmit"])
        .write_stdin(input)
        .assert()
        .success();
}

#[test]
fn test_config_list() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Test config list
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sync.enabled"))
        .stdout(predicate::str::contains("hooks.token_budget"));
}

#[test]
fn test_config_list_json() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Test config list with JSON output
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["--json", "config", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sync"))
        .stdout(predicate::str::contains("hooks"));
}

#[test]
fn test_config_get_set() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Get a value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "sync.enabled"])
        .assert()
        .success()
        .stdout(predicate::str::contains("true"));

    // Set a value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "sync.enabled", "false"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Set sync.enabled"));

    // Verify the value was set
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "sync.enabled"])
        .assert()
        .success()
        .stdout(predicate::str::contains("false"));

    // Hub policy is consumed by the host daemon, even from an initialized
    // project with a conflicting value (cas-e3ed).
    let host = temp.path().join(".test-home/.cas");
    std::fs::create_dir_all(&host).unwrap();
    std::fs::write(
        host.join("config.toml"),
        "[hub]\ntransport_mode = \"required\"\ntailscale_serve = false\n",
    )
    .unwrap();
    let root = temp.path().join(".cas");
    let mut project = cas::config::Config::load(&root).unwrap();
    project.set("hub.transport_mode", "loopback").unwrap();
    project.save(&root).unwrap();
    let project_before = std::fs::read(root.join("config.toml")).unwrap();
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "hub.transport_mode"])
        .assert()
        .success()
        .stdout("required\n");
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "hub.tailscale_serve"])
        .assert()
        .success()
        .stdout("true\n");
    let list = cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["--json", "config", "list", "--section", "hub"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let list: serde_json::Value = serde_json::from_slice(&list).unwrap();
    assert_eq!(list["hub"]["hub.transport_mode"]["value"], "required");
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "hub.transport_mode", "loopback"])
        .assert()
        .success();
    assert_eq!(
        cas::config::Config::load(&host)
            .unwrap()
            .get("hub.transport_mode")
            .as_deref(),
        Some("loopback")
    );
    assert_eq!(
        std::fs::read(root.join("config.toml")).unwrap(),
        project_before,
        "host policy must not rewrite project config"
    );
    let outside = TempDir::new().unwrap();
    let git = std::process::Command::new("git")
        .args(["init", "--initial-branch=main"])
        .current_dir(outside.path())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    assert!(
        git.status.success(),
        "{}",
        String::from_utf8_lossy(&git.stderr)
    );
    cas_cmd(temp.path())
        .current_dir(&outside)
        .args(["config", "get", "hub.transport_mode"])
        .assert()
        .success()
        .stdout("loopback\n");
    let legacy_set = cas_cmd(temp.path())
        .current_dir(&outside)
        .args(["config", "set", "hub.tailscale_serve", "true"])
        .output()
        .unwrap();
    assert!(legacy_set.status.success(), "{}", String::from_utf8_lossy(&legacy_set.stderr));
    let legacy_output = String::from_utf8_lossy(&legacy_set.stdout);
    for expected in [
        "Legacy hub.tailscale_serve = true saved; ignored by hub.transport_mode.",
        "Effective value: hub.tailscale_serve = false (transport_mode = loopback).",
        "Use: cas config set hub.transport_mode auto",
    ] {
        assert!(legacy_output.contains(expected), "missing {expected}: {legacy_output}");
    }
    assert_eq!(
        cas::config::Config::load(&host)
            .unwrap()
            .hub
            .unwrap()
            .tailscale_serve,
        Some(true)
    );
    cas_cmd(temp.path())
        .current_dir(&outside)
        .args(["config", "list", "--section", "hub", "--json"])
        .assert()
        .success();
    let other = temp.path().join("other/.cas");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("config.toml"), "").unwrap();
    cas_cmd(temp.path())
        .current_dir(&temp)
        .env("CAS_ROOT", &other)
        .args(["config", "set", "hub.transport_mode", "required"])
        .assert()
        .success();
    let invalid_mode = cas_cmd(temp.path())
        .current_dir(&outside)
        .args(["config", "set", "hub.transport_mode", "tailnet"])
        .output()
        .unwrap();
    assert_eq!(invalid_mode.status.code(), Some(4));
    let invalid_error = String::from_utf8_lossy(&invalid_mode.stderr);
    assert!(
        invalid_error.contains("hub.transport_mode must be one of: auto, required, loopback"),
        "key-specific validation should name the accepted modes: {invalid_error}"
    );
    assert!(
        !invalid_error.contains("Suggestion:"),
        "do not add the generic parse suggestion: {invalid_error}"
    );
    assert_eq!(
        cas::config::Config::load(&host)
            .unwrap()
            .get("hub.transport_mode")
            .as_deref(),
        Some("required")
    );
    assert_eq!(
        std::fs::read(root.join("config.toml")).unwrap(),
        project_before
    );
    assert_eq!(
        std::fs::read_to_string(other.join("config.toml")).unwrap(),
        ""
    );
    cas_cmd(temp.path())
        .current_dir(&outside)
        .args(["config", "get", "sync.enabled"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no Cassy store here"));
}

#[test]
fn test_config_get_unknown_key() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Try to get unknown key
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "get", "unknown.key"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unknown config key"));
}

#[test]
fn test_config_set_validation() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Try to set invalid boolean value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "sync.enabled", "notabool"])
        .assert()
        .failure();

    // Try to set invalid integer value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "hooks.token_budget", "notanumber"])
        .assert()
        .failure();

    // Refusals under --json still produce one machine-readable error document.
    let invalid = cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["--json", "config", "set", "coordination.mode", "invalid"])
        .output()
        .expect("cas config set must run");
    assert!(!invalid.status.success());
    let error: serde_json::Value = serde_json::from_slice(&invalid.stdout)
        .expect("--json config refusal must emit one JSON document");
    assert_eq!(error["status"], "error", "{error}");
    assert!(
        error["message"].as_str().unwrap_or_default().contains("Value must be one of: local, cloud"),
        "{error}"
    );
}

#[test]
fn config_set_preserves_comments_order_and_only_removes_the_documented_marker() {
    let temp = TempDir::new().unwrap();
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    let config_path = temp.path().join(".cas/config.toml");
    let original = "# hand-maintained configuration\n\
[upkeep]\n\
# codemap policy\n\
codemap = \"off\" # retain this inline comment\n\
product_overview = \"observe\"\n\
\n\
[upkeep.observe_since]\n\
codemap = \"old-codemap-marker\"\n\
product_overview = \"keep-this-marker\"\n\
\n\
[sync]\n\
# unrelated sync policy\n\
min_helpful = 3 # retain this too\n\
target = \"custom-rules\"\n\
\n\
[hub]\n\
tailscale_serve = false\n\
transport_mode = \"required\"\n";
    std::fs::write(&config_path, original).unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "upkeep.codemap", "enforce"])
        .assert()
        .success();

    let expected = original
        .replace("codemap = \"off\"", "codemap = \"enforce\"")
        .replace("codemap = \"old-codemap-marker\"\n", "");
    // pin: config set must retain every unrelated byte, comment and table order.
    assert_eq!(std::fs::read_to_string(config_path).unwrap(), expected);
}

#[test]
fn test_config_list_section_filter() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Filter by section
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "list", "--section", "hooks"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hooks.capture_enabled"))
        .stdout(predicate::str::contains("hooks.token_budget"));
}

#[test]
fn test_config_list_modified() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Modify a value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "hooks.token_budget", "8000"])
        .assert()
        .success();

    // List only modified values
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "list", "--modified"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hooks.token_budget"))
        .stdout(predicate::str::contains("8000"));
}

#[test]
fn test_config_diff() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // After init --yes with defaults, there should be no differences from defaults
    // (mcp.enabled was removed - MCP is always enabled)
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "diff"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No differences"));

    // The default is enabled; an explicit opt-out must remain visible as an override.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "code.enabled", "false"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "diff"])
        .assert()
        .success()
        .stdout(predicate::str::contains("code.enabled"))
        .stdout(predicate::str::contains("false"))
        .stdout(predicate::str::contains("default: true"));

    // Modify a value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "sync.min_helpful", "5"])
        .assert()
        .success();

    // Now there should be differences
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "diff"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sync.min_helpful"))
        .stdout(predicate::str::contains("5"))
        .stdout(predicate::str::contains("default: 1"));
}

#[test]
fn test_config_describe() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Describe a config key
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "describe", "hooks.token_budget"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Token Budget"))
        .stdout(predicate::str::contains("integer"))
        .stdout(predicate::str::contains("4000"));
}

#[test]
fn test_config_export_import() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Modify a value
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["config", "set", "hooks.token_budget", "6000"])
        .assert()
        .success();

    // Export config
    let export_output = cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["--json", "config", "list"])
        .assert()
        .success();

    // Verify exported config contains our modification
    let stdout = String::from_utf8_lossy(&export_output.get_output().stdout);
    assert!(stdout.contains("6000"));
}

#[cfg(feature = "mcp-proxy")]
#[test]
fn cas_8121_cli_names_project_hub_in_integrate_and_doctor() {
    let temp = TempDir::new().unwrap();
    cas_cmd(temp.path())
        .current_dir(&temp)
        .env_remove("CAS_CLONE_PATH")
        .args(["init", "--yes"])
        .assert()
        .success();
    let proxy = temp.path().join(".cas/proxy.toml");
    let original = "allowlist = [\"violet.violet_read\", \"violet.violet_post\"]\n[servers.violet]\ntransport = \"http\"\nurl = \"https://staging.example.test/mcp/slack\"\nauth = \"env:CAS_8121_STAGING_TOKEN\"\n";
    std::fs::write(&proxy, original).unwrap();
    let integrate = cas_cmd(temp.path())
        .current_dir(&temp)
        .env_remove("CAS_CLONE_PATH")
        .env_remove("CAS_8121_STAGING_TOKEN")
        .args([
            "integrate",
            "violet",
            "--dry-run",
            "--skip-verify",
            "--no-harness",
        ])
        .output()
        .unwrap();
    assert!(
        integrate.status.success(),
        "{}",
        String::from_utf8_lossy(&integrate.stderr)
    );
    let report = String::from_utf8(integrate.stdout).unwrap();
    assert!(
        report
            .lines()
            .any(|line| line.trim() == "hub: https://staging.example.test/mcp/slack"),
        "{report}"
    );
    assert_eq!(std::fs::read_to_string(&proxy).unwrap(), original);
    let doctor = cas_cmd(temp.path())
        .current_dir(&temp)
        .env_remove("CAS_CLONE_PATH")
        .env_remove("CAS_8121_STAGING_TOKEN")
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    let checks: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let row = checks
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "violet")
        .expect("project Violet probe must appear in doctor");
    assert_eq!(row["status"], "error");
    assert!(
        row["message"]
            .as_str()
            .unwrap()
            .contains("https://staging.example.test/mcp/slack")
    );
    assert!(
        row["message"]
            .as_str()
            .unwrap()
            .contains("CAS_8121_STAGING_TOKEN")
    );
    assert_eq!(std::fs::read_to_string(&proxy).unwrap(), original);
}

#[test]
fn test_doctor_json() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["doctor", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""name":"cas directory""#))
        .stdout(predicate::str::contains(r#""status":"ok""#));
}

#[test]
fn test_doctor_fix_json_is_one_document_with_startup_checks() {
    for args in [
        &["doctor", "--fix", "--json"][..],
        &["doctor", "--fix", "--yes", "--json"][..],
    ] {
        let temp = TempDir::new().unwrap();
        cas_cmd(temp.path())
            .current_dir(&temp)
            .args(["init", "--yes"])
            .assert()
            .success();

        let output = cas_cmd(temp.path())
            .current_dir(&temp)
            .args(args)
            .output()
            .expect("failed to run doctor --fix --json");
        assert!(
            output.status.success(),
            "doctor failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let mut decoder = serde_json::Deserializer::from_slice(&output.stdout);
        let checks = serde_json::Value::deserialize(&mut decoder)
            .expect("doctor --fix --json must emit valid JSON");
        decoder
            .end()
            .expect("doctor --fix --json must emit exactly one JSON document");
        let checks = checks
            .as_array()
            .expect("doctor JSON output must be a checks array");
        assert!(
            checks.iter().any(|check| check["name"] == "auto-fix"),
            "fix-path checks must remain in the single JSON document: {checks:?}"
        );
    }
}

#[test]
fn test_doctor_verbose_appends_full_check_messages() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    let output = cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["doctor", "--verbose"])
        .output()
        .expect("failed to run verbose doctor");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let verbose = stdout.find("verbose").expect("verbose section");
    assert!(stdout[..verbose].contains("Store"));
    assert!(stdout[verbose..].contains("cas directory: Found at"));
}

#[test]
fn test_doctor_mcp_configured() {
    let temp = TempDir::new().unwrap();
    let cas_root = temp.path().join(".cas");

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Doctor should show MCP is configured after init
    // Use CAS_ROOT to isolate from parent project's .cas
    cas_cmd(temp.path())
        .current_dir(&temp)
        .env("CAS_ROOT", &cas_root)
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("Config"))
        .stdout(predicate::str::contains("[OK] mcp config"));
}

#[test]
fn test_doctor_mcp_not_configured() {
    let temp = TempDir::new().unwrap();
    let cas_root = temp.path().join(".cas");

    // Initialize CAS
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    // Delete the .mcp.json file to simulate MCP not being configured
    std::fs::remove_file(temp.path().join(".mcp.json")).unwrap();

    // Doctor should warn about MCP not being configured
    // Use CAS_ROOT to isolate from parent project's .cas
    cas_cmd(temp.path())
        .current_dir(&temp)
        .env("CAS_ROOT", &cas_root)
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("mcp config"))
        .stdout(predicate::str::contains("MCP not configured"));
}

#[test]
fn test_doctor_fix_initializes_project() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["doctor", "--fix"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Store"))
        .stdout(predicate::str::contains("[OK] auto-fix"));
}

#[test]
fn test_doctor_fix_json_before_init_errors() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["doctor", "--fix", "--json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "`cas doctor --fix --json` is not supported before initialization",
        ));
}

#[test]
fn test_bare_factory_flags_are_parsed() {
    let temp = TempDir::new().unwrap();
    let cas_root = temp.path().join(".cas");

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .env("CAS_ROOT", &cas_root)
        .args(["--new", "-w", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Factory mode requires an interactive terminal",
        ));
}

#[test]
fn test_noninteractive_factory_includes_preflight_hints() {
    let temp = TempDir::new().unwrap();
    let cas_root = temp.path().join(".cas");

    cas_cmd(temp.path())
        .current_dir(&temp)
        .env("CAS_ROOT", &cas_root)
        .args(["--new", "-w", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Initialize Cassy first with `cas doctor --fix` (or `cas init`).",
        ));
}

#[test]
fn test_doctor_not_initialized() {
    let temp = TempDir::new().unwrap();

    // Doctor on uninitialized directory should show error
    // Use CAS_ROOT pointing to non-existent .cas to prevent finding parent project's .cas
    cas_cmd(temp.path())
        .current_dir(&temp)
        .env("CAS_ROOT", temp.path().join(".cas"))
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("Not found"));
}

// ---------------------------------------------------------------------------
// `cas knowledge` — the distilled project wiki (cas-ee3d)
// ---------------------------------------------------------------------------

/// Seed one distilled page directly through the store, so the CLI round-trip
/// can be exercised without an LLM. `cas knowledge build` is the production
/// producer; this stands in for it.
fn seed_knowledge_page(cas_dir: &std::path::Path) {
    use cas_store::{IngestBatch, KnowledgePage, KnowledgeStore, PageWrite, SqliteKnowledgeStore};

    let store = SqliteKnowledgeStore::open(cas_dir).expect("knowledge store should open");
    let mut page = KnowledgePage::new(
        store.generate_id().expect("id"),
        "subsystem",
        "Worktree Manager",
    );
    page.snippet = "Creates and reaps per-worker git worktrees.".to_string();
    page.sources = vec!["docs/worktrees.md".to_string()];

    store
        .commit_ingest(&IngestBatch {
            pages: vec![PageWrite {
                page,
                body: "# Worktree Manager\n\nCreates and reaps per-worker git worktrees.\n\
                       Each factory worker gets an isolated branch.\n"
                    .to_string(),
            }],
            ..IngestBatch::default()
        })
        .expect("ingest should commit");
}

/// Replace a current initialized knowledge schema with the m219 shape that
/// existed before m226 added page-attribution columns. The CLI process must
/// repair this fixture through `SqliteKnowledgeStore::open` before any page
/// command issues a query.
fn seed_pre_m226_knowledge_store(cas_dir: &std::path::Path) {
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).expect("open legacy fixture");
    conn.execute_batch(
        "DROP TABLE knowledge_pages_fts;
         DROP TABLE knowledge_sources;
         DROP TABLE knowledge_page_tombstones;
         DROP TABLE knowledge_pages;
         DELETE FROM cas_migrations WHERE id IN (219, 226, 227);

         CREATE TABLE knowledge_pages (
             row_id INTEGER PRIMARY KEY AUTOINCREMENT,
             id TEXT NOT NULL UNIQUE,
             page_type TEXT NOT NULL,
             title TEXT NOT NULL,
             rel_path TEXT NOT NULL,
             snippet TEXT NOT NULL DEFAULT '',
             locked INTEGER NOT NULL DEFAULT 0,
             sources_json TEXT NOT NULL DEFAULT '[]',
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL,
             pending_embedding INTEGER NOT NULL DEFAULT 1
         );
         CREATE VIRTUAL TABLE knowledge_pages_fts USING fts5(
             title, snippet, body, content='', contentless_delete=1
         );
         INSERT INTO knowledge_pages
             (id, page_type, title, rel_path, snippet, sources_json, created_at, updated_at)
         VALUES (
             'cas-kn001', 'architecture', 'Legacy Store', 'architecture/legacy-store.md',
             'A pre-m226 page.', '[\"README.md\"]',
             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'
         );
         INSERT INTO knowledge_pages_fts (rowid, title, snippet, body)
         VALUES (1, 'Legacy Store', 'A pre-m226 page.', 'The legacy page is searchable.');",
    )
    .expect("create pre-m226 knowledge fixture");
    std::fs::create_dir_all(cas_dir.join("knowledge/architecture")).expect("create body dir");
    std::fs::write(
        cas_dir.join("knowledge/architecture/legacy-store.md"),
        "# Legacy Store\n\nThe legacy page is searchable.\n",
    )
    .expect("write legacy body");
}

#[test]
fn test_knowledge_commands_open_and_upgrade_pre_m226_store() {
    let temp = TempDir::new().unwrap();
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    let cas_dir = temp.path().join(".cas");
    seed_pre_m226_knowledge_store(&cas_dir);

    // Every public CLI entry point opens the shared store. `status` runs
    // first, proving the self-heal happens before its first SELECT; the rest
    // guard against future paths opening a query-capable legacy store.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pages:   1"));
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("architecture/legacy-store.md"));
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "search", "searchable"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Legacy Store"));
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "read", "cas-kn001"])
        .assert()
        .success()
        .stdout(predicate::str::contains("The legacy page is searchable."));
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "build", "--dry-run", "--max-sources", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Knowledge distillation (dry run"));

    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).expect("open upgraded store");
    for column in ["origin", "origin_project_id"] {
        let exists: i64 = conn
            .query_row(
                "SELECT EXISTS (
                     SELECT 1 FROM pragma_table_info('knowledge_pages') WHERE name = ?1
                 )",
                [column],
                |row| row.get(0),
            )
            .expect("query column shape");
        assert_eq!(exists, 1, "m226 column {column} must be restored");
    }
}

#[test]
fn test_knowledge_search_and_read_round_trip() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    seed_knowledge_page(&temp.path().join(".cas"));

    // search surfaces the page by a body word.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "search", "worktrees"])
        .assert()
        .success()
        .stdout(predicate::str::contains("subsystem/worktree-manager.md"))
        .stdout(predicate::str::contains("Worktree Manager"));

    // read by path prints metadata plus the markdown body from disk.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "read", "subsystem/worktree-manager.md"])
        .assert()
        .success()
        .stdout(predicate::str::contains("locked:  false"))
        .stdout(predicate::str::contains("docs/worktrees.md"))
        .stdout(predicate::str::contains(
            "Each factory worker gets an isolated branch.",
        ));
}

#[cfg(unix)]
#[test]
fn knowledge_build_timeout_returns_nonzero_and_does_not_start_another_call() {

    let temp = TempDir::new().unwrap();
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    std::fs::write(temp.path().join("README.md"), "# Slow source\n\ncontent\n").unwrap();
    let provider = temp.path().join("provider");
    let calls = temp.path().join("provider-calls");
    cas::test_paths::warm_stub(
        &provider,
        &format!(
            "#!/bin/sh\nprintf call >> '{}'\nsleep 30\n",
            calls.display()
        ),
    );

    let output = cas_cmd(temp.path())
        .current_dir(&temp)
        .env("CAS_KNOWLEDGE_LLM_BIN", &provider)
        .args([
            "knowledge",
            "build",
            "--timeout-secs",
            "1",
            "--max-sources",
            "5",
        ])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "timeout must be a nonzero CLI result"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("timed out"), "stderr: {stderr}");
    assert!(
        stderr.contains("source '") && stderr.contains(".md'"),
        "timeout must identify the source in flight: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(calls).unwrap().lines().count(),
        1,
        "the command deadline must prevent later stage/source calls"
    );
}

#[cfg(unix)]
#[test]
fn knowledge_build_verbose_and_status_full_explain_a_failed_source() {

    let temp = TempDir::new().unwrap();
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    std::fs::write(
        temp.path().join("README.md"),
        "# Failing source\n\ncontent\n",
    )
    .unwrap();
    let provider = temp.path().join("provider");
    cas::test_paths::warm_stub(
        &provider,
        "#!/bin/sh\ncat >/dev/null\nprintf 'provider refused this source\\n' >&2\nexit 7\n",
    );

    let build = cas_cmd(temp.path())
        .current_dir(&temp)
        .env("CAS_KNOWLEDGE_LLM_BIN", &provider)
        .args([
            "--verbose",
            "knowledge",
            "build",
            "--timeout-secs",
            "5",
            "--max-sources",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "a failed source is recorded for retry"
    );
    let progress = String::from_utf8_lossy(&build.stderr);
    assert!(
        progress.contains("distilling") && progress.contains(".md"),
        "verbose build must name the source it starts: {progress}"
    );
    assert!(
        progress.contains("failed") && progress.contains(".md"),
        "verbose build must name the failed source: {progress}"
    );

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["--full", "knowledge", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            ".claude/agents/task-verifier.md",
        ))
        .stdout(predicate::str::contains("provider refused this source"));

    let status_json = cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["--json", "--full", "knowledge", "status"])
        .output()
        .unwrap();
    assert!(status_json.status.success());
    let status: serde_json::Value = serde_json::from_slice(&status_json.stdout).unwrap();
    assert_eq!(
        status["failed_sources"][0]["path"],
        ".claude/agents/task-verifier.md"
    );
    assert!(
        status["failed_sources"][0]["error"]
            .as_str()
            .is_some_and(|error| error.contains("provider refused this source"))
    );
}

/// cas-461a, through the shipped command rather than the store API.
///
/// `fts_query` joined tokens with a space, which FTS5 reads as an implicit
/// AND, so a multi-term search only matched pages containing *every* term.
/// The cas-d075 measurement
/// (`docs/migration/cas-b129-knowledge-retrieval-verdict.md`) recorded 7 of 10
/// real-vocabulary queries returning zero pages where legacy returned 4–10.
///
/// The regression is asserted at the CLI boundary because that is where it was
/// observed and because the failure was silent: `cas knowledge search` printed
/// a clean "No distilled pages match", not an error, so nothing upstream could
/// notice it.
#[test]
fn test_knowledge_multi_term_search_is_disjunctive() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    seed_knowledge_page(&temp.path().join(".cas"));

    // Every term present: worked before this fix, must keep working.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "search", "worktrees factory branch"])
        .assert()
        .success()
        .stdout(predicate::str::contains("subsystem/worktree-manager.md"));

    // The actual regression: a query where only some terms occur on the page.
    // Under the implicit-AND conjunction this printed "No distilled pages
    // match" and the page was unreachable.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args([
            "knowledge",
            "search",
            "worktrees deployment kubernetes rollback",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("subsystem/worktree-manager.md"))
        .stdout(predicate::str::contains("No distilled pages match").not());

    // Disjunctive must not mean "matches anything": a query sharing no term
    // with the corpus still reports no matches.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "search", "kubernetes rollback helm"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No distilled pages match"));

    // An explicitly quoted phrase keeps adjacency: these two words both appear
    // on the page but never next to each other in this order.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "search", "\"branch worktrees\""])
        .assert()
        .success()
        .stdout(predicate::str::contains("No distilled pages match"));

    // ...while the phrase that does appear verbatim is found, proving the
    // result above is adjacency rather than a tokenisation accident.
    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "search", "\"isolated branch\""])
        .assert()
        .success()
        .stdout(predicate::str::contains("subsystem/worktree-manager.md"));
}

#[test]
fn test_knowledge_search_with_no_match_is_not_an_error() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "search", "nonexistent-subject"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No distilled pages match"));
}

#[test]
fn test_knowledge_read_of_unknown_page_fails_loudly() {
    let temp = TempDir::new().unwrap();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["init", "--yes"])
        .assert()
        .success();

    cas_cmd(temp.path())
        .current_dir(&temp)
        .args(["knowledge", "read", "subsystem/does-not-exist.md"])
        .assert()
        .failure();
}

/// cas-b69a (GH #157): the cas-b129 M3 incident in miniature — copy a project
/// store, `cd` into the copy, and run a MUTATING command while `CAS_ROOT` (as a
/// factory session exports it) still points at the live store.
///
/// The operator must be told, on stderr, that the two stores differ. Since
/// cas-e1c7 a config write no longer lands silently in the CAS_ROOT store: it
/// is refused before writing until `--store` picks one, and with
/// `--store cas-root` CAS_ROOT still wins (precedence is unchanged for every
/// other command — factory workers in clones depend on it).
#[test]
fn cas_root_override_of_a_differing_cwd_root_is_announced_on_stderr() {
    let temp = TempDir::new().unwrap();
    let live = temp.path().join("live");
    let copy = temp.path().join("copy");
    std::fs::create_dir_all(&live).unwrap();
    std::fs::create_dir_all(&copy).unwrap();

    // Two independent projects, each with its own .cas.
    cas_cmd(temp.path())
        .current_dir(&live)
        .args(["init", "--yes"])
        .assert()
        .success();
    cas_cmd(temp.path())
        .current_dir(&copy)
        .args(["init", "--yes"])
        .assert()
        .success();

    let live_root = live.join(".cas");
    let copy_root = copy.join(".cas");

    // A mutating command, run from the copy, with CAS_ROOT aimed at the live store.
    let output = cas_cmd(temp.path())
        .current_dir(&copy)
        .env("CAS_ROOT", &live_root)
        .args(["config", "set", "sync.min_helpful", "7"])
        .output()
        .expect("cas config set must run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // (1) Both roots named, winner stated.
    assert!(
        stderr.contains(&live_root.display().to_string()),
        "stderr must name the CAS_ROOT store.\nstderr: {stderr}"
    );
    assert!(
        stderr.contains(&copy_root.display().to_string()),
        "stderr must name the working-directory store.\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("CAS_ROOT wins"),
        "stderr must state which root won.\nstderr: {stderr}"
    );

    // (3) stderr only — stdout stays parseable.
    assert!(
        !stdout.contains("CAS_ROOT override"),
        "the notice must never reach stdout.\nstdout: {stdout}"
    );

    // (2) cas-e1c7: refused, and neither store was written.
    assert!(
        !output.status.success(),
        "a config write across differing stores must refuse.\nstderr: {stderr}"
    );
    let live_config = std::fs::read_to_string(live_root.join("config.toml")).unwrap();
    let copy_config = std::fs::read_to_string(copy_root.join("config.toml")).unwrap();
    assert!(
        !live_config.contains("min_helpful = 7"),
        "live config: {live_config}"
    );
    assert!(
        !copy_config.contains("min_helpful = 7"),
        "copy config: {copy_config}"
    );

    // With --store cas-root the CAS_ROOT store is written, as before.
    cas_cmd(temp.path())
        .current_dir(&copy)
        .env("CAS_ROOT", &live_root)
        .args([
            "config",
            "set",
            "sync.min_helpful",
            "7",
            "--store",
            "cas-root",
        ])
        .assert()
        .success();
    let live_config = std::fs::read_to_string(live_root.join("config.toml")).unwrap();
    let copy_config = std::fs::read_to_string(copy_root.join("config.toml")).unwrap();
    assert!(
        live_config.contains("min_helpful = 7"),
        "--store cas-root writes the CAS_ROOT store.\nlive config: {live_config}"
    );
    assert!(
        !copy_config.contains("min_helpful = 7"),
        "the working-directory store must be untouched.\ncopy config: {copy_config}"
    );
}

/// cas-e1c7: `cas cloud project set`, run inside another project from a shell
/// whose CAS_ROOT names a different store, rewrote the CAS_ROOT project's
/// canonical_id and only warned beside the success line. It now refuses before
/// writing (and before sign-in), naming both stores and the flags to pick one;
/// `--store here` writes the working-directory project.
#[test]
fn cloud_project_set_across_differing_stores_refuses_and_writes_nothing() {
    let temp = TempDir::new().unwrap();
    let cas_src = temp.path().join("cas-src");
    let violet = temp.path().join("violet_ps");
    std::fs::create_dir_all(&cas_src).unwrap();
    std::fs::create_dir_all(&violet).unwrap();
    for dir in [&cas_src, &violet] {
        cas_cmd(temp.path())
            .current_dir(dir)
            .args(["init", "--yes"])
            .assert()
            .success();
    }
    let cas_src_root = cas_src.join(".cas");
    let violet_root = violet.join(".cas");
    let before_cas_src = std::fs::read_to_string(cas_src_root.join("config.toml")).unwrap();
    let before_violet = std::fs::read_to_string(violet_root.join("config.toml")).unwrap();

    let output = cas_cmd(temp.path())
        .current_dir(&violet)
        .env("CAS_ROOT", &cas_src_root)
        .args([
            "cloud",
            "project",
            "set",
            "github.com/richards-llc/violet_ps",
        ])
        .output()
        .expect("cas cloud project set must run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !output.status.success(),
        "must refuse.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        !stdout.contains("canonical id set"),
        "no success line.\nstdout: {stdout}"
    );
    assert!(
        stderr.contains("`cas cloud project set` not run"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "CAS_ROOT store:          {}",
            cas_src_root.display()
        )),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "this directory's store:  {}",
            violet_root.canonicalize().unwrap().display()
        )),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("Nothing was written"), "stderr: {stderr}");
    assert!(stderr.contains("--store here"), "stderr: {stderr}");
    assert!(stderr.contains("--store cas-root"), "stderr: {stderr}");
    assert_eq!(
        std::fs::read_to_string(cas_src_root.join("config.toml")).unwrap(),
        before_cas_src,
        "the CAS_ROOT store is untouched"
    );
    assert_eq!(
        std::fs::read_to_string(violet_root.join("config.toml")).unwrap(),
        before_violet,
        "the working-directory store is untouched"
    );
}

/// The other half of the honesty contract: when there is nothing to
/// disambiguate — CAS_ROOT pointing at the very store the working directory
/// would have resolved on its own — there must be no notice at all. A banner
/// that fires on every ordinary factory command is a banner nobody reads.
#[test]
fn cas_root_matching_the_cwd_root_produces_no_notice() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    cas_cmd(temp.path())
        .current_dir(&project)
        .args(["init", "--yes"])
        .assert()
        .success();

    let output = cas_cmd(temp.path())
        .current_dir(&project)
        .env("CAS_ROOT", project.join(".cas"))
        .args(["config", "set", "sync.min_helpful", "7"])
        .output()
        .expect("cas config set must run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("CAS_ROOT override"),
        "no conflict means no notice.\nstderr: {stderr}"
    );
}

#[test]
fn worktree_pr_body_reads_task_metadata_and_writes_review_file() {
    use cas_store::TaskStore;
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repository with spaces");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let result = std::process::Command::new("git").current_dir(&repo).args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        String::from_utf8(result.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "fixture@example.test"]);
    git(&["config", "user.name", "Fixture"]);
    cas_cmd(temp.path()).current_dir(&repo).args(["init", "--yes"]).assert().success();
    std::fs::write(repo.join("input.txt"), "before\n").unwrap();
    git(&["add", "input.txt"]);
    git(&["commit", "-qm", "base"]);
    let base = git(&["rev-parse", "HEAD"]);
    std::fs::write(repo.join("input.txt"), "after\n").unwrap();
    git(&["commit", "-qam", "change"]);
    let mut task = cas_types::Task::new("cas-review-fixture".into(), "Review fixture".into());
    task.risk = vec![cas_types::TaskRisk::Platform];
    task.door = Some(cas_types::TaskDoor::TwoWay);
    cas_store::SqliteTaskStore::open(&repo.join(".cas")).unwrap().add(&task).unwrap();
    let output = repo.join("review body.md");
    cas_cmd(temp.path()).current_dir(&repo).args([
        "worktree", "pr-body", "--task", &task.id, "--base", &base, "--head", "HEAD",
        "--before", "command: fails", "--after", "command: passes",
        "--evidence", "/proof/qa/bundle.json", "--output", output.to_str().unwrap(),
    ]).assert().success();
    let body = std::fs::read_to_string(output).unwrap();
    assert!(body.contains("M\tinput.txt"));
    assert!(body.contains("**Before:** command: fails"));
    assert!(body.contains("**After:** command: passes"));
    assert!(body.contains("/proof/qa/bundle.json"));
    assert!(body.contains("**Risk:** platform"));
    assert!(body.contains("**Door:** two-way"));
    cas_cmd(temp.path()).current_dir(&repo).args([
        "worktree", "pr-body", "--base", "missing-ref", "--head", "HEAD",
    ]).assert().failure();
}

#[test]
fn issue_repo_cli_reads_show_origin_without_saving_cas_f398() {
    let temp = TempDir::new().unwrap();
    for args in [
        vec!["init", "-q"],
        vec![
            "remote",
            "add",
            "origin",
            "https://github.com/example/project.git",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .current_dir(temp.path())
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    cas_cmd(temp.path())
        .current_dir(temp.path())
        .args(["init", "--yes"])
        .assert()
        .success();
    let root = temp.path().join(".cas");
    let mut config = cas::config::Config::load(&root).unwrap();
    config.set("issues.repo", "").unwrap();
    config.save(&root).unwrap();
    let before = std::fs::read(root.join("config.toml")).unwrap();

    // pin: get's scalar is interpolated as gh --repo, with no human annotation.
    cas_cmd(temp.path())
        .current_dir(temp.path())
        .args(["config", "get", "issues.repo"])
        .assert()
        .success()
        .stdout("example/project\n");
    for args in [
        vec!["config", "list", "--section", "issues"],
        vec!["config", "describe", "issues.repo"],
        vec!["config", "search", "issues.repo"],
    ] {
        cas_cmd(temp.path())
            .current_dir(temp.path())
            .args(args)
            .assert()
            .success()
            .stdout(predicate::str::contains("example/project"))
            .stdout(predicate::str::contains("(from origin; unset in config)"));
    }
    for (args, value_key) in [
        (vec!["config", "get", "issues.repo", "--json"], "value"),
        (
            vec!["config", "list", "--section", "issues", "--json"],
            "value",
        ),
        (
            vec!["config", "describe", "issues.repo", "--json"],
            "current_value",
        ),
        (
            vec!["config", "search", "issues.repo", "--json"],
            "current_value",
        ),
    ] {
        let output = cas_cmd(temp.path())
            .current_dir(temp.path())
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
        let item = if json.is_array() {
            json.as_array()
                .unwrap()
                .iter()
                .find(|item| item["key"] == "issues.repo")
                .unwrap()
        } else if json.get("issues").is_some() {
            &json["issues"]["issues.repo"]
        } else {
            &json
        };
        assert_eq!(item[value_key], "example/project");
        assert_eq!(item["source"], "origin");
        assert_eq!(item["configured"], false);
    }
    let doctor = cas_cmd(temp.path())
        .current_dir(temp.path())
        .args(["doctor", "--json"])
        .assert()
        .get_output()
        .stdout
        .clone();
    assert!(
        String::from_utf8(doctor)
            .unwrap()
            .contains("issues.repo unset; resolved from origin example/project")
    );
    assert_eq!(std::fs::read(root.join("config.toml")).unwrap(), before);
    assert!(
        cas::config::Config::load(&root)
            .unwrap()
            .issue_repo_registry()
            .project
            .is_none()
    );

    cas_cmd(temp.path())
        .current_dir(temp.path())
        .args(["config", "set", "issues.repo", "custom/override"])
        .assert()
        .success();
    let output = cas_cmd(temp.path())
        .current_dir(temp.path())
        .args(["config", "get", "issues.repo", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let explicit: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(explicit["value"], "custom/override");
    assert_eq!(explicit["source"], "config");
    assert_eq!(explicit["configured"], true);
    cas_cmd(temp.path())
        .current_dir(temp.path())
        .args(["config", "reset", "issues.repo"])
        .assert()
        .success();
    cas_cmd(temp.path())
        .current_dir(temp.path())
        .args(["config", "get", "issues.repo"])
        .assert()
        .success()
        .stdout("example/project\n");
}
