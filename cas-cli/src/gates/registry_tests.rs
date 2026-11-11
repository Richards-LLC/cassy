#[path = "../../gate_audit.rs"]
mod gate_audit;

use cas_store::{EventStore, SqliteEventStore};
use chrono::Utc;

use super::{Family, REGISTRY, UNCODED, facts, tag, override_tag, codes_in};

#[test]
fn approval_stamp_applies_registry_policy_and_survives_carry_cas_278a() {
    use super::{ApprovalPolicy, DELIVERY_STAGES, RegistryVersion, check_applies, registry_version_in_force};

    #[derive(Clone)]
    struct Approval { stamp: RegistryVersion }

    let current = registry_version_in_force();
    let mut old = current.clone();
    // The registry itself supplies both the new checks and expected policy;
    // no handwritten gate list can conceal an unclassified delivery row.
    for policy in [ApprovalPolicy::AsOf, ApprovalPolicy::Integrity] {
        assert!(REGISTRY.iter().any(|row| row.approval_policy == Some(policy)));
    }
    old.0.retain(|code| super::find(code).unwrap().approval_policy.is_none());
    let approved = Approval { stamp: old };
    let carried = approved.clone();
    let persisted = serde_json::to_string(&approved.stamp).unwrap();
    let restored: RegistryVersion = serde_json::from_str(&persisted).unwrap();
    assert_eq!(carried.stamp, restored);
    assert_eq!(current, registry_version_in_force());

    for row in REGISTRY {
        assert_eq!(row.approval_policy.is_some(), DELIVERY_STAGES.contains(&row.stage), "{}", row.code);
        let expected = row.approval_policy != Some(ApprovalPolicy::AsOf)
            || approved.stamp.0.contains(row.code);
        assert_eq!(check_applies(row.code, Some(&carried.stamp)).unwrap(), expected, "{}", row.code);
        assert!(check_applies(row.code, Some(&current)).unwrap());
        assert!(check_applies(row.code, None).unwrap());
    }
    assert!(check_applies("unknown", Some(&carried.stamp)).is_err());
}

#[test]
fn audited_retirements_match_the_measured_registry() {
    let audit: serde_json::Value = serde_json::from_str(include_str!("retirements.json")).unwrap();
    use std::collections::BTreeSet;
    let baseline: BTreeSet<_> = audit["baseline_codes"].as_array().unwrap().iter()
        .map(|code| code.as_str().unwrap()).collect();
    let removed: BTreeSet<_> = audit["rows"].as_array().unwrap().iter()
        .filter(|row| row["action"] == "remove").map(|row| row["code"].as_str().unwrap()).collect();
    let live: BTreeSet<_> = REGISTRY.iter().map(|row| row.code).collect();
    assert_eq!(baseline.len(), audit["baseline_total"].as_u64().unwrap() as usize);
    assert!(removed.is_subset(&baseline));
    assert!(removed.is_disjoint(&live), "{:?}", removed.intersection(&live).collect::<Vec<_>>());
    // Later independent additions/deletions must not invalidate this task's
    // historical retirement receipt. Its own removed IDs can never reappear.
    let unrelated_removed = baseline.difference(&live).filter(|code| !removed.contains(*code)).count();
    let unrelated_added = live.difference(&baseline).count();
    assert_eq!(REGISTRY.len(), baseline.len() - removed.len() - unrelated_removed + unrelated_added);
    eprintln!("registry: baseline={} retired={} other_removed={} other_added={} measured={}",
        baseline.len(), removed.len(), unrelated_removed, unrelated_added, REGISTRY.len());
}

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
    // The same inventory that audits the checkout must reject a new bypass
    // without a code, and a declared bypass without its successful tag.
    let fixture = tempfile::tempdir().unwrap();
    let source_dir = fixture.path().join("cas-cli/src");
    std::fs::create_dir_all(&source_dir).unwrap();
    let fixture_registry = vec![("G-01".into(), "park".into(), "Fixture policy".into(), "cas-cli/src/probe.rs:1".into())];
    for (refusal, accepted, covered) in [
        (r#"Err("REFUSED")"#, r#"Ok("accepted")"#, false),
        (r#"Err(tag("G-01", "REFUSED"))"#, r#"Ok("accepted")"#, false),
        (r#"Err(tag("G-01", "REFUSED"))"#, r#"Ok(override_tag("G-01", "accepted"))"#, true),
    ] {
        let source = format!("fn status(verbose: bool) {{ if verbose {{ println!(\"REFUSED\"); }} }}\nfn unrelated() {{ override_tag(\"G-01\", \"accepted\"); }}\n// gate-audit[supervisor_override]: bypass G-01 Accepts an otherwise refused request.\nfn probe(supervisor_override: bool) {{ if !supervisor_override {{ {refusal} }} else {{ {accepted} }} }}");
        std::fs::write(source_dir.join("probe.rs"), source).unwrap();
        let result = gate_audit::scan(fixture.path(), &fixture_registry);
        assert_eq!(result.gaps.is_empty(), covered, "{:?}", result.gaps);
    }
    let registry = REGISTRY.iter().map(|r| (r.code.into(), r.stage.into(), r.title.into(), r.source.into())).collect::<Vec<_>>();
    let root = crate::test_paths::crate_root().join("..");
    let audit = gate_audit::scan(&root, &registry);
    let report = root.join("target/gate-audit.md");
    std::fs::create_dir_all(report.parent().unwrap()).unwrap();
    std::fs::write(&report, audit.markdown(REGISTRY.len())).unwrap();
    std::fs::write(root.join("target/gate-audit-gaps.txt"), audit.gaps.join("\n")).unwrap();
    assert!(audit.gaps.is_empty(), "mechanical gate audit gaps:\n{}", audit.gaps.join("\n"));
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

    // Replay the denied CLI command through its parser, handler and existing
    // shared refusal observer; the authority refusal must count exactly once.
    use clap::Parser;
    env.set("CAS_AGENT_ROLE", "worker");
    let cli = crate::cli::Cli::try_parse_from([
        "cas", "gates", "false-positive", code, "--reason", "fixture",
    ]).unwrap();
    let Some(crate::cli::Commands::Gates(command)) = &cli.command else { panic!("not a gates command"); };
    let error = crate::cli::gates_cmd::execute(command, &cli, Some(dir.path())).unwrap_err();
    facts::observe_refusal(dir.path(), None, &format!("{error:#}"));
    assert_eq!(facts::summarize(&store, before).unwrap().get("G-153"),
        Some(&facts::Counts { refused: 1, overridden: 0, false_positive: 0 }));

    // Exercise the real MCP transport/router with a non-task tool, so a
    // per-task observer cannot make the boundary test pass.
    use rmcp::ServiceExt;
    let core = crate::mcp::CasCore::with_daemon(dir.path().to_path_buf(), None, None);
    use cas_store::{AgentStore, TaskStore};
    use cas_types::{Agent, AgentRole, Task, TaskStatus, Priority};
    env.set("CAS_AGENT_ROLE", "supervisor");
    env.set("CAS_FACTORY_MODE", "1");
    std::fs::write(dir.path().join("config.toml"), "[verification]\nenabled=true\n[qa]\nmode='per_task'\n").unwrap();
    crate::store::open_store(dir.path()).unwrap().init().unwrap();
    crate::store::open_rule_store(dir.path()).unwrap().init().unwrap();
    crate::store::open_skill_store(dir.path()).unwrap().init().unwrap();
    let agents = crate::store::open_agent_store(dir.path()).unwrap();
    agents.init().unwrap();
    agents.register(&Agent::new_with_role("gate-supervisor".into(), "supervisor".into(), AgentRole::Supervisor)).unwrap();
    core.set_agent_id_for_testing("gate-supervisor".into());
    assert!(core.load_config().verification_enabled());
    crate::store::open_verification_store(dir.path()).unwrap();
    let tasks = crate::store::open_task_store(dir.path()).unwrap();
    tasks.init().unwrap();
    let mut task = Task::new("cas-override".into(), "External repair measured".into());
    task.priority = Priority(0);
    task.execution_note = Some("no-code".into());
    task.external_ref = Some("https://example.test/proof".into());
    tasks.add(&task).unwrap();
    let service = crate::mcp::tools::service::CasService::new(core, #[cfg(feature = "mcp-proxy")] None);
    let work = service.tool_work.clone();
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
    // The real task-close producer must identify the verification gate it
    // waived, and the common MCP observer must persist exactly one fact.
    let request = serde_json::from_value(serde_json::json!({
        "name":"task", "arguments":{"action":"close", "id":task.id,
            "supervisor_override":true, "reason":"External repair measured"},
    })).unwrap();
    let response = client.call_tool(request).await.unwrap();
    assert_ne!(response.is_error, Some(true), "{response:?}");
    assert_eq!(tasks.get(&task.id).unwrap().status, TaskStatus::Closed);
    // COMMITTED can precede the final router result. Await the production
    // handler-drain signal, including its observer, rather than polling facts.
    work.wait_idle().await;
    assert_eq!(facts::summarize(&store, before).unwrap().get("G-68"),
        Some(&facts::Counts { refused: 0, overridden: 1, false_positive: 0 }), "{response:?}; {:?}", tasks.get(&task.id).unwrap().notes);
    client.cancel().await.unwrap();
    server.await.unwrap();
    assert_eq!(facts::summarize(&store, before).unwrap().get("G-116"),
        Some(&facts::Counts { refused: 1, overridden: 0, false_positive: 0 }));

    // Invalid native input returns H-01 before the inner handle_hook route.
    let output = crate::cli::hook::process_hook_input("PreToolUse", Ok("{".into()), true).unwrap();
    assert!(facts::hook_denial(&output).is_some_and(|reason| codes_in(reason).contains(&"H-01")));
    assert_eq!(facts::summarize(&store, before).unwrap().get("H-01"),
        Some(&facts::Counts { refused: 1, overridden: 0, false_positive: 0 }));

    // Drive an allowed stored publication override through the real outer
    // hook path; no Violet operation executes in this fixture.
    let mut epic = Task::new("cas-publication".into(), "Publication fixture".into());
    epic.task_type = cas_types::TaskType::Epic;
    epic.pending_verification = true;
    epic.notes = format!("[{}] DECISION: PUBLICATION OVERRIDE: fixture approval", Utc::now().format("%Y-%m-%d %H:%M"));
    tasks.add(&epic).unwrap();
    let input = serde_json::json!({"session_id":"gate-fixture", "cwd":dir.path(), "tool_name":"violet_post",
        "tool_input":{"kind":"file", "deliverable":true, "epic_id":epic.id}}).to_string();
    let output = crate::cli::hook::process_hook_input("PreToolUse", Ok(input.clone()), false).unwrap();
    assert!(facts::hook_denial(&output).is_none());
    assert_eq!(facts::summarize(&store, before).unwrap().get("H-07"),
        Some(&facts::Counts { refused: 0, overridden: 1, false_positive: 0 }));
    // Task notes are append-preserving; use a fresh epic with no approval.
    epic.id = "cas-publication-held".into();
    epic.notes.clear();
    tasks.add(&epic).unwrap();
    let mut input: serde_json::Value = serde_json::from_str(&input).unwrap();
    input["tool_input"]["epic_id"] = serde_json::json!(epic.id);
    let output = crate::cli::hook::process_hook_input("PreToolUse", Ok(input.to_string()), false).unwrap();
    assert!(facts::hook_denial(&output).is_some());
    assert_eq!(facts::summarize(&store, before).unwrap().get("H-07"),
        Some(&facts::Counts { refused: 1, overridden: 1, false_positive: 0 }));

    // The shared observer distinguishes a tagged accepted override from a
    // successful read quoting the same code.
    let override_reply = Ok(rmcp::model::CallToolResult::success(vec![rmcp::model::Content::text(tag("G-108", "accepted"))]));
    facts::observe_tool_result(dir.path(), None, &override_reply);
    assert!(!facts::summarize(&store, before).unwrap().contains_key("G-108"));
    let arguments = serde_json::json!({"supervisor_override":true,"task_id":"cas-ci","reason":"reviewed"});
    facts::observe_tool_result(dir.path(), arguments.as_object(), &override_reply);
    assert!(!facts::summarize(&store, before).unwrap().contains_key("G-108"));
    let override_reply = Ok(rmcp::model::CallToolResult::success(vec![rmcp::model::Content::text(
        override_tag("G-108", override_tag("G-108", tag("G-114", "accepted"))))]));
    facts::observe_tool_result(dir.path(), None, &override_reply);
    assert_eq!(facts::summarize(&store, before).unwrap()["G-108"],
        facts::Counts { refused: 0, overridden: 1, false_positive: 0 });

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
