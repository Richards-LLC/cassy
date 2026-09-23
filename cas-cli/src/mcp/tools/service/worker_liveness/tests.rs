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
