use std::path::{Path, PathBuf};

use cas_store::{EventStore, SqliteEventStore};
use chrono::Utc;

use super::{Family, REGISTRY, UNCODED, facts, find, tag, codes_in};

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name != "target") {
                rust_sources(&path, out);
            }
        } else if path.extension().is_some_and(|ext| matches!(ext.to_str(), Some("rs" | "sh" | "py"))) {
            out.push(path);
        }
    }
}

fn script_emits(row: &super::GateRow) -> bool {
    let (path, _) = row.source.rsplit_once(':').unwrap();
    let source = std::fs::read_to_string(crate::test_paths::crate_root().join("..").join(path)).unwrap();
    source.lines().filter(|line| !line.trim_start().starts_with('#'))
        .any(|line| line.contains(&format!("[{}]", row.code)))
}

/// Codes a production line emits: `gates::tag("CODE"` calls and `[CODE]` literals.
fn emitted_codes() -> std::collections::BTreeSet<String> {
    let manifest = crate::test_paths::crate_root();
    let manifest = manifest.as_path();
    let mut files = Vec::new();
    rust_sources(&manifest.join("src"), &mut files);
    rust_sources(&manifest.join("../crates"), &mut files);
    rust_sources(&manifest.join("../scripts"), &mut files);
    let mut emitted = std::collections::BTreeSet::new();
    for file in files {
        if file.starts_with(manifest.join("src/gates")) {
            continue;
        }
        let source = std::fs::read_to_string(&file).unwrap();
        let code_text: String = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//") && !line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        for (start, _) in code_text.match_indices("gates::tag(") {
            let rest = code_text[start + 11..].trim_start();
            if let Some(code) = rest.strip_prefix('"').and_then(|rest| rest.split('"').next()) {
                assert!(
                    find(code).is_some(),
                    "{}: gates::tag({code:?}) names no gate[...] declaration",
                    file.display()
                );
                emitted.insert(code.to_string());
            }
        }
        for row in REGISTRY {
            if code_text.contains(&format!("[{}]", row.code)) {
                emitted.insert(row.code.to_string());
            }
        }
    }
    emitted
}

/// Checks that only grant an exemption and never refuse on their own.
const NON_REFUSING: &[&str] = &["G-57"];

/// The registry is measured from code: every row comes from one declaration,
/// and every refusing check (gates and guards declared in Rust) emits its
/// code, so its refusals are recorded under that code and not as uncoded.
#[test]
fn every_registered_refusal_site_emits_its_code() {
    for family in [Family::Gate, Family::Guard, Family::Watchdog, Family::Notice, Family::Ci] {
        assert!(REGISTRY.iter().any(|row| row.family() == family), "no {family:?} row registered");
    }
    if !crate::test_paths::crate_root().join("src/gates").is_dir() {
        eprintln!("SKIP gate emission scan: source checkout unavailable in this test archive");
        return;
    }
    let emitted = emitted_codes();
    let silent: Vec<&str> = REGISTRY
        .iter()
        .filter(|row| (matches!(row.family(), Family::Gate | Family::Guard) && row.source.contains(".rs:"))
            || row.source.contains(".sh:") || row.source.contains(".py:"))
        .filter(|row| if row.source.contains(".sh:") || row.source.contains(".py:") {
            !script_emits(row)
        } else {
            !emitted.contains(row.code) && !NON_REFUSING.contains(&row.code)
        })
        .map(|row| row.source)
        .collect();
    assert!(silent.is_empty(), "refusing checks that never emit their code: {silent:#?}");

    // Replay the two uncoded script boundaries from the rejected delivery.
    let fixture = tempfile::tempdir().unwrap();
    let repo = fixture.path();
    assert!(std::process::Command::new("git").args(["init", "-q"]).arg(repo).status().unwrap().success());
    assert!(std::process::Command::new("git").current_dir(repo)
        .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "commit", "--allow-empty", "-qm", "fixture"])
        .status().unwrap().success());
    assert!(std::process::Command::new("git").current_dir(repo).args(["checkout", "--detach", "-q"]).status().unwrap().success());
    let scripts = crate::test_paths::crate_root().join("../scripts");
    let release = std::process::Command::new("bash").arg(scripts.join("release.sh"))
        .current_dir(repo).env("CAS_BIN", repo.join("missing-cas")).output().unwrap();
    assert!(!release.status.success());
    assert!(codes_in(&String::from_utf8_lossy(&release.stderr)).contains(&"C-34"));
    let mut hook = std::process::Command::new("bash").arg(scripts.join("fast-rows-pre-push.sh"))
        .current_dir(repo).env_remove("CAS_SKIP_FAST_ROWS").env("CAS_BIN", repo.join("missing-cas"))
        .stdin(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
    use std::io::Write;
    writeln!(hook.stdin.take().unwrap(), "refs/heads/factory/fixture {} refs/heads/factory/fixture {}", "1".repeat(40), "0".repeat(40)).unwrap();
    let hook = hook.wait_with_output().unwrap();
    assert!(!hook.status.success());
    assert!(codes_in(&String::from_utf8_lossy(&hook.stderr)).contains(&"H-38"));
}

/// A refusal, its override and a declared false positive are counted under
/// the refused code; a refusal with no code is counted as uncoded.
#[tokio::test]
async fn refusals_overrides_and_false_positives_are_counted_per_code() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let dir = tempfile::tempdir().unwrap();
    env.set("CAS_ROOT", dir.path());
    let store = SqliteEventStore::open(dir.path()).unwrap();
    store.init().unwrap();
    let before = Utc::now() - chrono::Duration::seconds(1);
    let code = REGISTRY.iter().find(|row| row.family() == Family::Gate).unwrap().code;

    facts::record_refusal(&store, Some("cas-1"), &tag(code, "REFUSED")).unwrap();
    facts::record_refusal(&store, Some("cas-1"), "a refusal without a code").unwrap();
    facts::record_refusal(&store, Some("cas-1"), &tag(code, "REFUSED again")).unwrap();
    assert_eq!(facts::record_override(&store, "cas-1", "reviewed-drop: abc").unwrap(), vec![code]);
    facts::record_false_positive(&store, code, Some("cas-1"), "fixture was right").unwrap();
    assert!(facts::record_false_positive(&store, "G-999", None, "x").is_err());

    let counts = facts::summarize(&store, before).unwrap();
    assert_eq!(counts[code], facts::Counts { refused: 2, overridden: 1, false_positive: 1 });
    assert_eq!(counts[UNCODED].refused, 1);

    // Exercise the real MCP transport/router with a non-task tool, so a
    // per-task observer cannot make the boundary test pass.
    use rmcp::ServiceExt;
    let core = crate::mcp::CasCore::with_daemon(dir.path().to_path_buf(), None, None);
    let service = crate::mcp::tools::service::CasService::new(core, #[cfg(feature = "mcp-proxy")] None);
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        service.serve(server_io).await.unwrap().waiting().await.unwrap();
    });
    let client = ().serve(client_io).await.unwrap();
    let request = serde_json::from_value(serde_json::json!({
        "name":"verification", "arguments":{"action":"batch_receipt"},
    })).unwrap();
    let response = client.call_tool(request).await;
    assert!(response.is_err());
    assert!(codes_in(&format!("{response:?}")).contains(&"G-116"));
    client.cancel().await.unwrap();
    server.await.unwrap();
    assert_eq!(facts::summarize(&store, before).unwrap().get("G-116"),
        Some(&facts::Counts { refused: 1, overridden: 0, false_positive: 0 }));

    // Invalid native input returns H-01 before the inner handle_hook route.
    let output = crate::cli::hook::process_hook_input("PreToolUse", Ok("{".into()), true).unwrap();
    assert!(facts::hook_denial(&output).is_some_and(|reason| codes_in(reason).contains(&"H-01")));
    assert_eq!(facts::summarize(&store, before).unwrap().get("H-01"),
        Some(&facts::Counts { refused: 1, overridden: 0, false_positive: 0 }));

    // G-92 returns before CI admission: exercise the public merge API.
    let operator = dir.path().join("operator");
    std::fs::create_dir_all(&operator).unwrap();
    let hold = operator.join("project-hold.json");
    std::fs::write(&hold, serde_json::to_vec(&cas_store::project_hold::State {
        active: Some(cas_store::project_hold::Hold {
            reason: "fixture".into(), operator: "fixture".into(), set_at: Utc::now(),
        }), history: vec![],
    }).unwrap()).unwrap();
    let core = crate::mcp::CasCore::with_daemon(dir.path().to_path_buf(), None, None);
    let result = core.worktree_merge("fixture", false, Some("cas-hold"), false, None, false, None).await;
    assert!(result.is_err());
    std::fs::remove_file(hold).unwrap();
    assert_eq!(facts::summarize(&store, before).unwrap().get("G-92"),
        Some(&facts::Counts { refused: 1, overridden: 0, false_positive: 0 }));

    // Exercise the same CI admission and accepted-override boundaries used
    // by worktree_merge, outside the task-tool dispatcher.
    crate::mcp::tools::core::workflow::exercise_ci_gate_facts(dir.path());
    assert_eq!(facts::summarize(&store, before).unwrap()["G-108"],
        facts::Counts { refused: 1, overridden: 1, false_positive: 0 });

    // Replay CAS_SKIP_FAST_ROWS through the real hook. Capture only the
    // executable boundary, then dispatch its exact argv through the real CLI
    // parser/handler, so the test needs no separately built cas binary.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::process::{Command, Stdio};
        use clap::Parser;

        assert!(Command::new("git").args(["init", "-q"]).arg(dir.path()).status().unwrap().success());
        let fake_cas = dir.path().join("fixture-cas");
        let argv = dir.path().join("argv");
        std::fs::write(&fake_cas, "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$FACT_ARGS\"\n").unwrap();
        std::fs::set_permissions(&fake_cas, std::fs::Permissions::from_mode(0o755)).unwrap();
        let script = dir.path().join("fast-rows-pre-push.sh");
        std::fs::write(&script, include_str!("../../../scripts/fast-rows-pre-push.sh")).unwrap();
        for accepts_fact in [true, false] {
            if !accepts_fact {
                std::fs::write(&fake_cas, "#!/bin/sh\nexit 1\n").unwrap();
            }
            let mut hook = Command::new("bash").arg(&script).current_dir(dir.path())
                .env("CAS_BIN", &fake_cas).env("FACT_ARGS", &argv)
                .env("CAS_SKIP_FAST_ROWS", "fixture hotfix")
                .stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            writeln!(hook.stdin.take().unwrap(), "refs/heads/factory/fixture {} refs/heads/factory/fixture {}", "1".repeat(40), "0".repeat(40)).unwrap();
            let result = hook.wait_with_output().unwrap();
            assert_eq!(result.status.success(), accepts_fact, "{}", String::from_utf8_lossy(&result.stderr));
            if accepts_fact {
                let captured = std::fs::read(&argv).unwrap();
                let args = std::iter::once("cas".to_string()).chain(captured.split(|byte| *byte == 0)
                    .filter(|arg| !arg.is_empty()).map(|arg| String::from_utf8(arg.to_vec()).unwrap()));
                let cli = crate::cli::Cli::try_parse_from(args).unwrap();
                let Some(crate::cli::Commands::Gates(command)) = &cli.command else { panic!("hook did not invoke gates"); };
                crate::cli::gates_cmd::execute(command, &cli, Some(dir.path())).unwrap();
            } else {
                assert!(codes_in(&String::from_utf8_lossy(&result.stderr)).contains(&"H-38"));
            }
        }
        // Refusal reporters must call the fact CLI, but neither a missing
        // executable nor a failed recording may replace the original exit.
        std::fs::write(&fake_cas, "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$FACT_ARGS\"\n").unwrap();
        let release = dir.path().join("release.sh");
        std::fs::write(&release, include_str!("../../../scripts/release.sh")).unwrap();
        assert!(Command::new("git").current_dir(dir.path())
            .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "commit", "--allow-empty", "-qm", "fixture"])
            .status().unwrap().success());
        assert!(Command::new("git").current_dir(dir.path()).args(["checkout", "--detach", "-q"]).status().unwrap().success());
        for (reporter, code) in [(&release, "C-34"), (&script, "H-38")] {
            std::fs::remove_file(&argv).ok();
            let mut child = Command::new("bash").arg(reporter).current_dir(dir.path())
                .env("CAS_BIN", &fake_cas).env("FACT_ARGS", &argv).env_remove("CAS_SKIP_FAST_ROWS")
                .stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            if reporter == &script {
                writeln!(child.stdin.take().unwrap(), "factory/f {} refs/heads/factory/f {}", "1".repeat(40), "0".repeat(40)).unwrap();
            }
            let result = child.wait_with_output().unwrap();
            assert_eq!(result.status.code(), Some(1));
            assert!(codes_in(&String::from_utf8_lossy(&result.stderr)).contains(&code));
            let captured = std::fs::read(&argv).expect("refusal reporter did not invoke the fact CLI");
            let args = std::iter::once("cas".to_string()).chain(captured.split(|byte| *byte == 0)
                .filter(|arg| !arg.is_empty()).map(|arg| String::from_utf8(arg.to_vec()).unwrap()));
            let cli = crate::cli::Cli::try_parse_from(args).unwrap();
            let Some(crate::cli::Commands::Gates(command)) = &cli.command else { panic!("reporter did not invoke gates"); };
            crate::cli::gates_cmd::execute(command, &cli, Some(dir.path())).unwrap();
            assert_eq!(facts::summarize(&store, before).unwrap()[code].refused, 1);

            let failed = dir.path().join("failed-cas");
            std::fs::write(&failed, "#!/bin/sh\nexit 7\n").unwrap();
            std::fs::set_permissions(&failed, std::fs::Permissions::from_mode(0o755)).unwrap();
            for bin in [failed, dir.path().join("missing-cas")] {
                let mut child = Command::new("bash").arg(reporter).current_dir(dir.path())
                    .env("CAS_BIN", bin).env_remove("CAS_SKIP_FAST_ROWS")
                    .stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
                if reporter == &script {
                    writeln!(child.stdin.take().unwrap(), "factory/f {} refs/heads/factory/f {}", "1".repeat(40), "0".repeat(40)).unwrap();
                }
                assert_eq!(child.wait_with_output().unwrap().status.code(), Some(1));
            }
        }
        let counts = facts::summarize(&store, before).unwrap();
        for code in ["C-07", "H-38"] {
            assert_eq!(counts[code], facts::Counts { refused: 1, overridden: 1, false_positive: 0 });
        }
    }
    assert!(facts::summarize(&store, Utc::now() + chrono::Duration::seconds(5)).unwrap().is_empty());
}
