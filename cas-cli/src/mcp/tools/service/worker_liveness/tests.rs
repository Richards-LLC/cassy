use super::*;
fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-10T19:01:00Z")
        .unwrap()
        .with_timezone(&Utc)
}
fn process(alive: bool) -> ProcessEvidence {
    ProcessEvidence {
        alive: Some(alive),
        cpu_busy: Some(false),
        detail: "pid 42 S, cpu idle, stdin_read yes".into(),
    }
}
fn run(cli: SupervisorCli, text: &str, alive: bool) -> Observation {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("transcript.jsonl");
    std::fs::write(&path, text).unwrap();
    observe(cli, Some(&path), process(alive), now(), 300)
}
fn run_with_unresolved_process(cli: SupervisorCli, text: &str) -> Observation {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("transcript.jsonl");
    std::fs::write(&path, text).unwrap();
    observe(
        cli,
        Some(&path),
        ProcessEvidence {
            alive: None,
            cpu_busy: None,
            detail: "worker harness pid unresolved; registered pid is MCP server".into(),
        },
        now(),
        300,
    )
}
#[test]
fn recorded_codex_completion_beats_fresh_heartbeat_and_file_write() {
    let fixture =
        include_str!("../../../../../tests/fixtures/worker-liveness/codex-completed.jsonl");
    let mut agent = cas_types::Agent::new("test".into(), "worker".into());
    agent.last_heartbeat = now();
    let got = run(SupervisorCli::Codex, fixture, true);
    assert_eq!(got.state, Liveness::WaitingForInput);
    assert!(
        got.evidence.contains("task_complete 1215s ago"),
        "{}",
        got.evidence
    );
}
#[test]
fn codex_all_states_and_terminal_aliases() {
    for (kind, time, alive, expected) in [
        ("turn_started", "19:00:59", true, Liveness::Executing),
        ("task_started", "18:00:00", true, Liveness::Stalled),
        (
            "turn_completed",
            "18:00:00",
            true,
            Liveness::WaitingForInput,
        ),
        ("task_complete", "18:00:00", true, Liveness::WaitingForInput),
        ("error", "19:00:59", true, Liveness::Stalled),
        ("turn_started", "19:00:59", false, Liveness::Dead),
    ] {
        let text = format!(
            "{{\"timestamp\":\"2026-09-10T{time}Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"{kind}\"}}}}\n"
        );
        assert_eq!(
            run(SupervisorCli::Codex, &text, alive).state,
            expected,
            "{kind}"
        );
    }
}

#[test]
fn fixed_cpu_evidence_agrees_while_cold_to_warm_sampling_changes_liveness() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("transcript.jsonl");
    let at = (now() - chrono::Duration::seconds(394)).to_rfc3339();
    for (cli, event) in [
        (SupervisorCli::Codex, serde_json::json!({
            "timestamp": at, "type": "response_item",
            "payload": {"type": "function_call_output"},
        })),
        (SupervisorCli::Claude, serde_json::json!({
            "timestamp": at, "type": "assistant",
            "message": {"role": "assistant", "stop_reason": "tool_use"},
        })),
    ] {
        std::fs::write(&path, format!("{event}\n")).unwrap();
        let mut samples = std::collections::HashMap::new();
        let cold = record_cpu_sample(&mut samples, 42, "original-process", 100);
        let warm = record_cpu_sample(&mut samples, 42, "original-process", 110);
        assert_eq!(cold, None);
        assert_eq!(warm, Some(true));
        for (cpu_busy, expected) in [(cold, Liveness::Stalled), (warm, Liveness::Executing)] {
            let evidence = ProcessEvidence {
                alive: Some(true), cpu_busy, detail: format!("fixed cpu sample: {cpu_busy:?}"),
            };
            let full = observe(cli, Some(&path), evidence.clone(), now(), 300);
            let same_snapshot = observe(cli, Some(&path), evidence, now(), 300);
            let summary = super::super::factory_ops::render_worker_liveness_summary(&[
                ("worker".into(), same_snapshot.clone()),
            ]);
            assert_eq!(full.state, expected, "{cli:?}: {}", full.detail());
            assert_eq!(same_snapshot.state, full.state);

            let _ = full.detail();
        }
        // PID reuse cannot borrow the prior process's CPU activity.
        assert_eq!(record_cpu_sample(&mut samples, 42, "replacement", 900), None);
    }
}

#[test]
fn codex_budget_abort_is_distinct_from_stall_and_idle() {
    let start = "{\"timestamp\":\"2026-09-10T19:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n";
    let budget = "{\"timestamp\":\"2026-09-10T19:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"turn_aborted\",\"reason\":\"budget_limited\"}}\n";
    let got = run(SupervisorCli::Codex, &format!("{start}{budget}"), true);
    assert_eq!(got.state, Liveness::BudgetAborted);
    assert_eq!(got.summary("worker"), "liveness: budget_aborted | worker");
    assert!(got.detail().contains("turn_aborted"));

    let interrupted = budget.replace("budget_limited", "interrupted");
    assert_eq!(
        run(SupervisorCli::Codex, &format!("{start}{interrupted}"), true).state,
        Liveness::WaitingForInput
    );
    let late_tool = "{\"timestamp\":\"2026-09-10T19:00:02Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\"}}\n";
    assert_eq!(
        run(
            SupervisorCli::Codex,
            &format!("{start}{budget}{late_tool}"),
            true
        )
        .state,
        Liveness::BudgetAborted
    );
    let budget_error = "{\"timestamp\":\"2026-09-10T19:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"error\",\"error\":{\"message\":\"shared rollout token budget exhausted\",\"codexErrorInfo\":\"rolloutBudgetExceeded\"}}}\n";
    assert_eq!(
        run(
            SupervisorCli::Codex,
            &format!("{start}{budget_error}"),
            true
        )
        .state,
        Liveness::BudgetAborted
    );
    let restarted = start.replace("19:00:00", "19:00:03");
    assert_eq!(
        run(
            SupervisorCli::Codex,
            &format!("{start}{budget}{restarted}"),
            true
        )
        .state,
        Liveness::Executing
    );
}
#[test]
fn claude_all_states() {
    for (reason, time, alive, expected) in [
        ("tool_use", "19:00:59", true, Liveness::Executing),
        ("tool_use", "18:00:00", true, Liveness::Stalled),
        ("end_turn", "18:00:00", true, Liveness::WaitingForInput),
        ("end_turn", "19:00:59", false, Liveness::Dead),
    ] {
        let text = format!(
            "{{\"timestamp\":\"2026-09-10T{time}Z\",\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"stop_reason\":\"{reason}\"}}}}\n"
        );
        assert_eq!(run(SupervisorCli::Claude, &text, alive).state, expected);
    }
}
#[test]
fn latest_event_wins_even_at_equal_timestamps_and_partial_append_is_ignored() {
    let end = "{\"timestamp\":\"2026-09-10T19:00:59Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n";
    let start = end.replace("task_complete", "task_started");
    assert_eq!(
        run(SupervisorCli::Codex, &format!("{end}{start}"), true).state,
        Liveness::Executing
    );
    assert_eq!(
        run(
            SupervisorCli::Codex,
            &format!("{start}{end}{{\"timestamp\":"),
            true
        )
        .state,
        Liveness::WaitingForInput
    );
    assert_eq!(
        run(
            SupervisorCli::Codex,
            &format!("{end}{}", start.trim()),
            true
        )
        .state,
        Liveness::WaitingForInput
    );
}
#[test]
fn missing_evidence_never_claims_execution_or_death() {
    let got = observe(
        SupervisorCli::Codex,
        None,
        ProcessEvidence {
            alive: None,
            cpu_busy: None,
            detail: "pid unavailable".into(),
        },
        now(),
        300,
    );
    assert_eq!(got.state, Liveness::Stalled);
    assert!(got.evidence.contains("unavailable"));
}
/// cas-5129 (GH #1054): a live Claude harness with no transcript has never
/// received a prompt. worker_status says so instead of `executing` (a booting
/// TUI's CPU) or `stalled`; a dead one is still dead, and other harnesses
/// keep their existing reading.
#[test]
fn claude_without_a_transcript_awaits_its_first_prompt_cas_5129() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("never-written.jsonl");
    let busy = |alive: Option<bool>| ProcessEvidence {
        alive,
        cpu_busy: Some(true),
        detail: "pid 42 R, cpu busy".into(),
    };
    for path in [None, Some(missing.as_path())] {
        let got = observe(SupervisorCli::Claude, path, busy(Some(true)), now(), 300);
        assert_eq!(got.state, Liveness::AwaitingFirstPrompt, "{path:?}");
        assert_eq!(got.state.as_str(), "awaiting_first_prompt");
        assert_eq!(
            observe(SupervisorCli::Claude, path, busy(Some(false)), now(), 300).state,
            Liveness::Dead
        );
    }
    // A transcript that exists is read as before, even with no turn yet.
    let empty = dir.path().join("empty.jsonl");
    std::fs::write(&empty, "").unwrap();
    assert_eq!(
        observe(SupervisorCli::Claude, Some(&empty), busy(Some(true)), now(), 300).state,
        Liveness::Executing
    );
    // Codex rollouts are resolved by cwd; a missing one is unchanged.
    assert_eq!(
        observe(SupervisorCli::Codex, None, busy(Some(true)), now(), 300).state,
        Liveness::Executing
    );
}

#[test]
fn unresolved_codex_process_keeps_recent_execution_evidence() {
    let started = "{\"timestamp\":\"2026-09-10T19:00:59Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n";
    assert_eq!(
        run_with_unresolved_process(SupervisorCli::Codex, started).state,
        Liveness::Executing
    );

    let completed = "{\"timestamp\":\"2026-09-10T19:00:59Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n";
    assert_eq!(
        run_with_unresolved_process(SupervisorCli::Codex, completed).state,
        Liveness::WaitingForInput
    );
}
#[test]
fn bounded_tail_recovers_after_large_utf8_record_and_ignores_sidechain() {
    let mut text = "λ".repeat(TAIL_BYTES as usize);
    text.push_str("\n{\"timestamp\":\"2026-09-10T18:00:00Z\",\"type\":\"system\",\"subtype\":\"turn_duration\"}\n");
    text.push_str("{\"timestamp\":\"2026-09-10T19:00:59Z\",\"type\":\"assistant\",\"isSidechain\":true,\"message\":{\"stop_reason\":\"tool_use\"}}\n");
    assert_eq!(
        run(SupervisorCli::Claude, &text, true).state,
        Liveness::WaitingForInput
    );
}

#[test]
fn recorded_claude_tool_use_can_execute_stall_or_die() {
    let fixture = include_str!("../../../../../tests/fixtures/worker-liveness/claude-turn.jsonl");
    let last = fixture.lines().last().unwrap();
    let mut value: Value = serde_json::from_str(last).unwrap();
    value["timestamp"] = Value::String("2026-09-10T19:00:59Z".into());
    let fresh = format!("{value}\n");
    assert_eq!(
        run(SupervisorCli::Claude, &fresh, true).state,
        Liveness::Executing
    );
    assert_eq!(
        run(SupervisorCli::Claude, fixture, true).state,
        Liveness::Stalled
    );
    assert_eq!(
        run(SupervisorCli::Claude, &fresh, false).state,
        Liveness::Dead
    );
    value["message"]["stop_reason"] = Value::String("end_turn".into());
    assert_eq!(
        run(SupervisorCli::Claude, &format!("{value}\n"), true).state,
        Liveness::WaitingForInput
    );
    assert_eq!(
        run(
            SupervisorCli::Claude,
            include_str!("../../../../../tests/fixtures/worker-liveness/claude-completed.jsonl"),
            true
        )
        .state,
        Liveness::WaitingForInput
    );
}

#[test]
fn shared_codex_observation_recognizes_recorded_completion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout.jsonl");
    std::fs::write(
        &path,
        include_str!("../../../../../tests/fixtures/worker-liveness/codex-completed.jsonl"),
    )
    .unwrap();
    let observed = crate::mcp::tools::service::harness_observation::latest_turn_observations(
        &path,
        SupervisorCli::Codex,
    );
    assert_eq!(
        observed.completion.unwrap().at.to_rfc3339(),
        "2026-09-10T18:40:44.469+00:00"
    );
}

#[test]
fn terminal_event_survives_late_tool_flush_until_a_new_turn() {
    let end = "{\"timestamp\":\"2026-09-10T18:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n";
    let tool = "{\"timestamp\":\"2026-09-10T19:00:59Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\"}}\n";
    assert_eq!(
        run(SupervisorCli::Codex, &format!("{end}{tool}"), true).state,
        Liveness::WaitingForInput
    );
    let end = "{\"timestamp\":\"2026-09-10T18:00:00Z\",\"type\":\"system\",\"subtype\":\"turn_duration\"}\n";
    let tool = "{\"timestamp\":\"2026-09-10T19:00:59Z\",\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\"}]}}\n";
    assert_eq!(
        run(SupervisorCli::Claude, &format!("{end}{tool}"), true).state,
        Liveness::WaitingForInput
    );
    let start = tool.replace("tool_result", "text");
    assert_eq!(
        run(SupervisorCli::Claude, &format!("{end}{start}"), true).state,
        Liveness::Executing
    );
}

#[test]
fn sidecars_cannot_stand_in_for_the_interactive_harness() {
    assert!(is_harness_command(b"/vendor/bin/codex\0--model\0test\0"));
    assert!(is_harness_command(
        b"/home/test/.local/bin/claude\0--agent-name\0wolf\0"
    ));
    assert!(!is_harness_command(b"/vendor/bin/codex-code-mode-host\0"));
    assert!(!is_harness_command(b"cas\0serve\0"));
    assert!(!is_harness_command(b"cargo\0test\0codex\0"));
}

#[test]
fn unresolved_mcp_pid_is_not_a_dead_harness() {
    let selection = select_harness_process(
        Some(ProcessCandidate {
            pid: 11,
            alive: false,
            harness: false,
        }),
        None,
        None,
    );
    assert_eq!(
        selection,
        ProcessSelection::Exited("registered worker harness exited"),
        "a direct hook row still preserves genuine harness exit"
    );

    let selection = select_harness_process(
        None,
        Some(ProcessCandidate {
            pid: 22,
            alive: true,
            harness: false,
        }),
        None,
    );
    assert_eq!(
        selection,
        ProcessSelection::Unavailable("worker harness parent is not an identified harness"),
        "an alive but unrecognized parent must not be declared dead"
    );

    let selection = select_harness_process(
        Some(ProcessCandidate {
            pid: 33,
            alive: true,
            harness: false,
        }),
        None,
        None,
    );
    assert_eq!(
        selection,
        ProcessSelection::Unavailable(
            "worker harness unavailable; registered process is not an identified harness",
        ),
        "a recycled live registered PID must remain unresolved"
    );
}

#[test]
fn mcp_parent_ownership_preserves_live_and_dead_classifications() {
    let live = select_harness_process(
        None,
        Some(ProcessCandidate {
            pid: 42,
            alive: true,
            harness: true,
        }),
        Some(ProcessCandidate {
            pid: 43,
            alive: true,
            harness: false,
        }),
    );
    assert_eq!(live, ProcessSelection::Found(42));

    let dead = select_harness_process(
        None,
        Some(ProcessCandidate {
            pid: 42,
            alive: false,
            harness: false,
        }),
        None,
    );
    assert_eq!(
        dead,
        ProcessSelection::Exited("worker harness parent exited")
    );
}

#[test]
fn process_evidence_adapter_rejects_a_reused_parent_pid() {
    let mut agent = cas_types::Agent::new("session".into(), "worker".into());
    agent.pid = Some(10);
    agent.ppid = Some(20);
    agent.pid_starttime = Some(123);

    let pid_alive = |pid| matches!(pid, 10 | 20);
    let pid_matches_fingerprint = |pid, starttime| pid == 10 && starttime == 123;
    let is_harness = |pid| matches!(pid, 10 | 20);
    let reused_parent_pid = |_child| Some(99);
    let probes = ProcessProbes {
        pid_alive: &pid_alive,
        pid_matches_fingerprint: &pid_matches_fingerprint,
        is_harness: &is_harness,
        parent_pid: &reused_parent_pid,
    };

    assert_eq!(
        process_selection_for_agent(&agent, None, &probes),
        ProcessSelection::Unavailable("worker harness parent is not an identified harness"),
        "a live parent with a generic harness command is not enough without the MCP child relation"
    );

    let current_parent_pid = |_child| Some(20);
    let probes = ProcessProbes {
        pid_alive: &pid_alive,
        pid_matches_fingerprint: &pid_matches_fingerprint,
        is_harness: &is_harness,
        parent_pid: &current_parent_pid,
    };
    assert_eq!(
        process_selection_for_agent(&agent, None, &probes),
        ProcessSelection::Found(20),
        "the recorded MCP child's current parent may prove the live harness"
    );
}

#[test]
fn live_codex_mcp_child_prevents_false_dead_after_parent_changes_gh_1033() {
    let mut agent = cas_types::Agent::new("session".into(), "codex-worker".into());
    agent.pid = Some(10); // registered cas serve child
    agent.ppid = Some(20); // former Codex parent
    agent.pid_starttime = Some(123);
    agent.metadata.insert("worker_cli".into(), "codex".into());

    let pid_alive = |pid| pid == 10;
    let fingerprint = |pid, starttime| pid == 10 && starttime == 123;
    let is_harness = |pid| pid == 20;
    let reparented = |_child| Some(1);
    let probes = ProcessProbes {
        pid_alive: &pid_alive,
        pid_matches_fingerprint: &fingerprint,
        is_harness: &is_harness,
        parent_pid: &reparented,
    };
    assert_eq!(
        process_selection_for_agent(&agent, None, &probes),
        ProcessSelection::Unavailable("registered MCP child alive; harness parent identity changed")
    );
    assert_eq!(
        run_with_unresolved_process(
            SupervisorCli::Codex,
            "{\"timestamp\":\"2026-09-10T19:00:59Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"turn_started\"}}\n",
        )
        .state,
        Liveness::Executing
    );

    let child_gone = |_: u32| false;
    let probes = ProcessProbes {
        pid_alive: &child_gone,
        pid_matches_fingerprint: &fingerprint,
        is_harness: &is_harness,
        parent_pid: &reparented,
    };
    assert_eq!(
        process_selection_for_agent(&agent, None, &probes),
        ProcessSelection::Exited("worker harness parent exited"),
    );
}

#[test]
fn repeated_fatal_codex_model_errors_override_busy_cpu_and_retries() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("rollout.jsonl");
    let error = serde_json::json!({"type": "error", "status": 400, "error": {
        "type": "invalid_request_error",
        "message": "The 'gpt-6.1-sol' model is not supported when using Codex with a ChatGPT account."
    }});
    let start = "{\"timestamp\":\"2026-09-10T19:00:59Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n";
    let text = format!("{start}{error}\n{start}{error}\n{start}");
    std::fs::write(&path, &text).unwrap();
    let busy = ProcessEvidence { alive: Some(true), cpu_busy: Some(true), detail: "busy harness with live MCP child".into() };
    let got = observe(SupervisorCli::Codex, Some(&path), busy.clone(), now(), 300);
    assert_eq!(got.state, Liveness::Stalled);
    assert!(got.evidence.contains("not supported"), "{}", got.evidence);
    std::fs::write(&path, format!("{text}{{\"timestamp\":\"2026-09-10T19:01:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_complete\",\"last_agent_message\":\"working\"}}}}\n")).unwrap();
    assert_eq!(observe(SupervisorCli::Codex, Some(&path), busy, now(), 300).state, Liveness::WaitingForInput);
}

#[test]
fn claude_background_job_keeps_a_completed_turn_busy_until_its_notification() {
    let launch = concat!(
        r#"{"timestamp":"2026-09-10T19:00:00Z","type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"tool-shell","name":"Bash","input":{"command":"affected tests","run_in_background":true}}]}}"#,
        "\n",
        r#"{"timestamp":"2026-09-10T19:00:01Z","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-shell","content":"Command running in background with ID: job-42. Output is being written to a file."}]}}"#,
        "\n",
        r#"{"timestamp":"2026-09-10T19:00:02Z","type":"assistant","message":{"stop_reason":"end_turn"}}"#,
        "\n",
    );
    assert_eq!(
        run(SupervisorCli::Claude, launch, true).state,
        Liveness::Executing
    );
    assert_eq!(
        run(SupervisorCli::Claude, launch, false).state,
        Liveness::Dead
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("silent-background.jsonl");
    std::fs::write(&path, launch).unwrap();
    // No terminal notification: foreground end_turn must not own busy forever.
    let launched_at = now() - chrono::Duration::seconds(59);
    for (elapsed, cpu_busy, expected) in [
        (299, Some(false), Liveness::Executing),
        (300, Some(false), Liveness::Stalled),
        (300, None, Liveness::Stalled),
        (900, Some(true), Liveness::Executing),
    ] {
        let mut evidence = process(true);
        evidence.cpu_busy = cpu_busy;
        assert_eq!(observe(SupervisorCli::Claude, Some(&path), evidence,
            launched_at + chrono::Duration::seconds(elapsed), 300).state, expected,
            "unconfirmed background job at {elapsed}s, cpu busy: {cpu_busy:?}");
    }
    for (notice, expected) in [
        (
            serde_json::json!({"type":"user","message":{"role":"user","content":"<task-notification><task-id>job-42</task-id><status>completed</status></task-notification>"}}),
            Liveness::WaitingForInput,
        ),
        (
            serde_json::json!({"type":"attachment","attachment":{"type":"queued_command","prompt":"<task-notification><task-id>job-42</task-id><status>completed</status></task-notification>"}}),
            Liveness::WaitingForInput,
        ),
        (
            serde_json::json!({"type":"attachment","attachment":{"type":"queued_command","prompt":"<task-notification><task-id>another-job</task-id><status>completed</status></task-notification>"}}),
            Liveness::Executing,
        ),
    ] {
        let finished = format!(
            "{notice}\n{{\"timestamp\":\"2026-09-10T19:00:51Z\",\"type\":\"assistant\",\"message\":{{\"stop_reason\":\"end_turn\"}}}}\n"
        );
        assert_eq!(
            run(SupervisorCli::Claude, &format!("{launch}{finished}"), true).state,
            expected,
            "completion notice: {notice}"
        );
    }
}

// Drive real /proc evidence with acknowledged CPU work, not sleeps or a
// fabricated cpu_busy. Every process stays alive and quiescent between commands.
#[cfg(target_os = "linux")]
struct CpuTree {
    harness: std::process::Child,
    output: std::io::BufReader<std::process::ChildStdout>,
    agent: cas_types::Agent,
    log_dir: tempfile::TempDir,
}

#[cfg(target_os = "linux")]
impl CpuTree {
    fn new(depth: u32) -> Self {
        use std::os::unix::process::CommandExt;
        let script = r#"
import subprocess, sys, time
script, depth = sys.argv[1], int(sys.argv[2])
child = None
if depth:
    child = subprocess.Popen(['python3', '-u', '-c', script, script, str(depth - 1), sys.argv[3]],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    assert child.stdout.readline().strip() == 'ready'
print('ready', flush=True)
for line in sys.stdin:
    command = line.strip()
    if command == 'harness' or (command == 'work' and child is None):
        end = time.process_time() + 0.15
        while time.process_time() < end:
            pass
    elif command == 'write' and child is None:
        with open(sys.argv[3], 'a') as log:
            log.write('passing test\n')
    elif child:
        child.stdin.write(command + '\n')
        child.stdin.flush()
        assert child.stdout.readline().strip() == 'done'
    print('done', flush=True)
if child:
    child.stdin.close()
    assert child.wait(timeout=5) == 0
"#;
        let log_dir = tempfile::tempdir().unwrap();
        let mut harness = std::process::Command::new("python3")
            .arg0("claude")
            .args(["-u", "-c", script, script, &depth.to_string()])
            .arg(log_dir.path().join("tests.log"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let output = std::io::BufReader::new(harness.stdout.take().unwrap());
        let mut agent =
            cas_types::Agent::new(format!("cpu-tree-{}", harness.id()), "worker".into());
        agent.pid = Some(harness.id());
        agent.pid_starttime = crate::mcp::daemon::read_pid_starttime(harness.id());
        let mut tree = Self {
            harness,
            output,
            agent,
            log_dir,
        };
        tree.read_ack("ready");
        tree
    }

    fn read_ack(&mut self, expected: &str) {
        use std::io::BufRead;
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), expected);
    }

    fn work(&mut self, command: &str) {
        use std::io::Write;
        writeln!(self.harness.stdin.as_mut().unwrap(), "{command}").unwrap();
        self.read_ack("done");
    }

    fn observe(&self) -> Observation {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("background.jsonl");
        std::fs::write(&path, concat!(
            r#"{"timestamp":"2026-09-10T18:59:59Z","type":"assistant","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","id":"shell","name":"Bash","input":{"command":"background job","run_in_background":true}}]}}"#,
            "\n",
            r#"{"timestamp":"2026-09-10T19:00:00Z","type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"shell","content":"Command running in background with ID: job-42."}]}}"#,
            "\n",
            r#"{"timestamp":"2026-09-10T19:00:02Z","type":"assistant","message":{"stop_reason":"end_turn"}}"#,
            "\n",
        )).unwrap();
        observe(
            SupervisorCli::Claude,
            Some(&path),
            process_evidence(&self.agent),
            now() + chrono::Duration::seconds(900),
            120,
        )
    }
}

#[cfg(target_os = "linux")]
impl Drop for CpuTree {
    fn drop(&mut self) {
        drop(self.harness.stdin.take());
        self.harness.wait().unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn process_evidence_busy_background_child_and_grandchild_keep_executing() {
    for depth in [1, 2] {
        let mut tree = CpuTree::new(depth);
        tree.observe(); // Cold CPU sample.
        tree.work("work");
        let got = tree.observe();
        assert_eq!(
            got.state,
            Liveness::Executing,
            "depth {depth}: {}",
            got.detail()
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn process_evidence_silent_background_child_ignores_harness_cpu() {
    let mut tree = CpuTree::new(1);
    tree.observe();
    tree.work("harness");
    let got = tree.observe();
    assert_eq!(got.state, Liveness::Stalled, "{}", got.detail());
}


// GH #1606: a live child writes passing tests without updating its owner's
// transcript. Exercise status, director health and the progress watchdog on
// the same owned tree; then leave it alive but silent until recovery is due.
#[cfg(target_os = "linux")]
#[test]
fn advancing_child_logs_prevent_false_recovery_cas_8a87() {
    use crate::ui::factory::{daemon::runtime::progress_watchdog, run_worker_health_probe, WorkerHealthRequest};
    use cas_types::{Agent, AgentRole, Task, TaskStatus};
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    const SESSION: &str = "child-progress-cas-8a87";
    env.set("CAS_FACTORY_SESSION", SESSION);
    let repo = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(repo.path()).unwrap();
    let mut tree = CpuTree::new(2);
    tree.agent.role = AgentRole::Worker;
    tree.agent.factory_session = Some(SESSION.into());
    tree.agent.metadata.insert("worker_cli".into(), "codex".into());
    tree.agent.metadata.insert("clone_path".into(), repo.path().display().to_string());
    tree.agent.metadata.insert("worker_account_dir".into(), env.home().display().to_string());
    let agents = crate::store::open_agent_store(&root).unwrap();
    agents.register(&tree.agent).unwrap();
    let mut supervisor = Agent::new_with_role("supervisor-8a87".into(), "supervisor".into(), AgentRole::Supervisor);
    supervisor.factory_session = Some(SESSION.into());
    agents.register(&supervisor).unwrap();
    let tasks = crate::store::open_task_store_local(&root).unwrap();
    let mut task = Task::new("cas-8a87-fixture".into(), "Long non-Rust gate".into());
    task.status = TaskStatus::InProgress;
    task.assignee = Some(tree.agent.id.clone());
    tasks.add(&task).unwrap();
    let rollout = env.home().join("sessions/2026/11/10/rollout-child-progress.jsonl");
    std::fs::create_dir_all(rollout.parent().unwrap()).unwrap();
    let now = Utc::now();
    let cold = now - chrono::Duration::hours(3);
    let meta = serde_json::json!({"type":"session_meta","payload":{"session_id":tree.agent.id,"cwd":repo.path(),"originator":"codex_cli_rs","source":"cli"}});
    let start = serde_json::json!({"timestamp":cold.to_rfc3339(),"type":"event_msg","payload":{"type":"task_started"}});
    std::fs::write(&rollout, format!("{meta}\n{start}\n")).unwrap();
    std::fs::File::open(&rollout).unwrap().set_modified(cold.into()).unwrap();
    let owner = tree.agent.clone();
    let status = |at| super::super::factory_ops::worker_liveness_for_agent(&root, &owner, at, 300);
    assert_eq!(status(now).state, Liveness::Stalled); // Cold sample never proves progress.
    progress_watchdog::sweep(&root, SESSION, now);
    let queue = crate::store::open_prompt_queue_store(&root).unwrap();
    for minutes in [44, 88, 132] {
        tree.work("write");
        assert!(std::fs::metadata(tree.log_dir.path().join("tests.log")).unwrap().len() > 0);
        let at = now + chrono::Duration::minutes(minutes);
        let got = status(at);
        assert_eq!(got.state, Liveness::Executing, "{}", got.detail());
        // A second consumer must not steal the first consumer's progress delta.
        assert_eq!(status(at).state, Liveness::Executing);
        let request = WorkerHealthRequest { worker: tree.agent.name.clone(), task: task.id.clone(), threshold_secs: 300, delay_secs: 5, requested_at: at };
        assert!(!run_worker_health_probe(Some(&root), &request).confirmed);
        progress_watchdog::sweep(&root, SESSION, at);
    }
    assert!(queue.peek_all(50).unwrap().is_empty());
    let silent = now + chrono::Duration::minutes(138);
    assert_eq!(status(silent).state, Liveness::Stalled);
    let request = WorkerHealthRequest { worker: tree.agent.name.clone(), task: task.id.clone(), threshold_secs: 300, delay_secs: 5, requested_at: silent };
    assert!(run_worker_health_probe(Some(&root), &request).confirmed, "a sleeping descendant cannot suppress recovery forever");
    // Neither a new session nor a reused root PID may borrow cached progress.
    tree.work("write");
    assert_eq!(status(silent).state, Liveness::Executing);
    progress_watchdog::sweep(&root, SESSION, silent);
    let mut foreign = tree.agent.clone();
    foreign.factory_session = Some("another-session".into());
    assert_eq!(super::super::factory_ops::worker_liveness_for_agent(&root, &foreign, silent, 300).state, Liveness::Stalled);
    foreign.pid_starttime = Some(0);
    assert_ne!(super::super::factory_ops::worker_liveness_for_agent(&root, &foreign, silent, 300).state, Liveness::Executing);
    for minutes in [184, 229] {
        progress_watchdog::sweep(&root, SESSION, now + chrono::Duration::minutes(minutes));
    }
    let rows = queue.peek_all(50).unwrap();
    assert_eq!(rows.iter().filter(|row| row.target == "supervisor").count(), 1);
    assert_eq!(rows.iter().filter(|row| row.target == "operator").count(), 1);
}
