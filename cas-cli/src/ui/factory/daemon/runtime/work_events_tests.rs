use super::*;
use cas_store::{EventStore, NotificationPriority};
use serde_json::json;
use std::io::Write;

fn source(cli: cas_mux::SupervisorCli) -> SupervisorSource {
    SupervisorSource {
        agent_id: "sup-1".into(),
        agent_name: "calm-otter-1".into(),
        harness_session: "harness-1".into(),
        cli,
        account_dir: None,
    }
}

fn scope(cli: cas_mux::SupervisorCli) -> WorkEventScope {
    WorkEventScope {
        session: "factory-1".into(),
        supervisors: vec![source(cli)],
        workers: Vec::new(),
        agents: HashMap::from([
            ("sup-1".to_owned(), "calm-otter-1".to_owned()),
            ("worker-1".to_owned(), "swift-fox-2".to_owned()),
        ]),
        floor: "2026-10-27T00:00:00Z".parse().unwrap(),
    }
}

fn write_lines(path: &Path, lines: &[Value]) {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    for line in lines {
        writeln!(file, "{line}").unwrap();
    }
}

fn at(second: u32) -> String {
    format!("2026-10-27T12:00:{second:02}Z")
}

fn page(
    index: &mut WorkEventIndex,
    scope: &WorkEventScope,
    stores: &WorkEventStores,
    path: &Path,
    before: Option<&str>,
    limit: u16,
) -> (Vec<WorkEvent>, bool, Option<String>) {
    let resolve = |_: &SupervisorSource| Some(path.to_path_buf());
    match index.page(scope, stores, &resolve, "page".into(), before, limit, None).unwrap() {
        DaemonMessage::WorkEvents { events, has_earlier, next_before, schema_version, .. } => {
            assert_eq!(schema_version, WORK_EVENT_SCHEMA_VERSION);
            (events, has_earlier, next_before)
        }
        other => panic!("not a page: {other:?}"),
    }
}

fn detail(index: &mut WorkEventIndex, scope: &WorkEventScope, stores: &WorkEventStores, path: &Path, id: &str) -> DaemonMessage {
    let resolve = |_: &SupervisorSource| Some(path.to_path_buf());
    index.detail(scope, stores, &resolve, "detail".into(), id.into())
}

fn claude_turn(path: &Path) {
    write_lines(path, &[
        json!({"type":"api-request-blob","message":{"content":[{"type":"tool_use","id":"x","name":"Bash","input":{}}]}}),
        json!({"type":"user","promptId":"prompt-1","timestamp":at(1),"message":{"role":"user","content":"ship it"}}),
        json!({"type":"assistant","timestamp":at(2),"message":{"role":"assistant","usage":{"input_tokens":120,"output_tokens":30},"content":[
            {"type":"thinking","thinking":"**Checking the lane**\nthen merge","signature":"sig"},
            {"type":"tool_use","id":"call-1","name":"Bash","input":{"command":"cargo check -p cas","description":"Check the crate"}}
        ]}}),
        json!({"type":"user","timestamp":at(3),"message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"call-1","is_error":true,"content":"error[E0425]"}
        ]}}),
        json!({"type":"assistant","timestamp":at(4),"message":{"role":"assistant","usage":{"input_tokens":200,"output_tokens":12},"content":[
            {"type":"thinking","thinking":"","signature":"signed-only"},
            {"type":"tool_use","id":"call-2","name":"TodoWrite","input":{"todos":[
                {"content":"Fix","status":"completed","activeForm":"Fixing"},
                {"content":"Test","status":"in_progress","activeForm":"Running tests"}
            ]}}
        ]}}),
    ]);
}

/// Claude thinking, tool calls with their results and todo updates become
/// typed events, paged newest-first by cursor, and reading them changes
/// nothing on disk: the agent's transcript and its recorded token usage are
/// byte-for-byte what they were.
#[test]
fn claude_transcript_pages_by_cursor_without_touching_the_transcript() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("supervisor.jsonl");
    claude_turn(&path);
    let before = std::fs::read(&path).unwrap();
    let (scope, stores, mut index) = (scope(cas_mux::SupervisorCli::Claude), WorkEventStores::default(), WorkEventIndex::default());

    let (newest, has_earlier, cursor) = page(&mut index, &scope, &stores, &path, None, 2);
    assert!(has_earlier);
    let shapes: Vec<_> = newest.iter().map(|event| (event.kind, event.label.as_str(), event.error)).collect();
    assert_eq!(shapes, vec![
        (WorkEventKind::Tool, "Bash: Check the crate", true),
        (WorkEventKind::Todo, "Todos 1/2 · Running tests", false),
    ]);
    assert!(newest.iter().all(|event| event.turn.as_deref() == Some("prompt-1")));
    let (older, has_earlier, _) = page(&mut index, &scope, &stores, &path, cursor.as_deref(), 2);
    assert!(!has_earlier);
    assert_eq!(older.len(), 1);
    assert_eq!((older[0].kind, older[0].label.as_str()), (WorkEventKind::Thinking, "Checking the lane"));

    let DaemonMessage::WorkEventDetail { detail: Some(text), truncated: false, .. } =
        detail(&mut index, &scope, &stores, &path, &newest[0].id)
    else {
        panic!("tool detail resolves");
    };
    assert!(text.contains("cargo check -p cas") && text.contains("Result (error):\nerror[E0425]"), "{text}");

    // A turn appended later is indexed incrementally.
    write_lines(&path, &[json!({"type":"assistant","timestamp":at(9),"message":{"role":"assistant","content":[
        {"type":"tool_use","id":"call-3","name":"mcp__cas__task","input":{"action":"close","id":"cas-1"}}
    ]}})]);
    let after_append = std::fs::read(&path).unwrap();
    let (latest, _, _) = page(&mut index, &scope, &stores, &path, None, 1);
    assert_eq!(latest[0].label, "cas.task: close cas-1");

    assert_eq!(std::fs::read(&path).unwrap(), after_append, "serving never writes the transcript");
    assert!(after_append.starts_with(&before));
    let usage = |bytes: &[u8]| -> u64 {
        String::from_utf8_lossy(bytes)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|value| value.pointer("/message/usage").cloned())
            .map(|usage| usage["input_tokens"].as_u64().unwrap_or(0) + usage["output_tokens"].as_u64().unwrap_or(0))
            .sum()
    };
    assert_eq!(usage(&after_append), 362, "the recorded model token count is unchanged");
}

/// Codex reasoning summaries, function calls, custom tool calls and plan
/// updates become the same typed events; a non-zero exit marks the call failed.
#[test]
fn codex_rollout_reasoning_and_calls_become_events() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rollout.jsonl");
    write_lines(&path, &[
        json!({"timestamp":at(1),"type":"event_msg","payload":{"type":"task_started","turn_id":"turn-7"}}),
        json!({"timestamp":at(2),"type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"**Reading the queue**"}],"encrypted_content":"opaque"}}),
        json!({"timestamp":at(3),"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"git\",\"status\"]}","call_id":"c1"}}),
        json!({"timestamp":at(4),"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"{\"output\":\"fatal\",\"metadata\":{\"exit_code\":128}}"}}),
        json!({"timestamp":at(5),"type":"response_item","payload":{"type":"function_call","name":"update_plan","arguments":"{\"plan\":[{\"step\":\"Read\",\"status\":\"completed\"},{\"step\":\"Merge\",\"status\":\"in_progress\"}]}","call_id":"c2"}}),
        json!({"timestamp":at(6),"type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"*** Begin Patch","call_id":"c3"}}),
    ]);
    let (scope, stores, mut index) = (scope(cas_mux::SupervisorCli::Codex), WorkEventStores::default(), WorkEventIndex::default());
    let (events, has_earlier, _) = page(&mut index, &scope, &stores, &path, None, 10);
    assert!(!has_earlier);
    let shapes: Vec<_> = events.iter().map(|event| (event.kind, event.label.as_str(), event.error)).collect();
    assert_eq!(shapes, vec![
        (WorkEventKind::Notice, "setup", false),
        (WorkEventKind::Thinking, "Reading the queue", false),
        (WorkEventKind::Tool, "shell: git status", true),
        (WorkEventKind::Todo, "Todos 1/2 · Merge", false),
        (WorkEventKind::Tool, "apply_patch: *** Begin Patch", false),
    ]);
    assert!(events.iter().all(|event| event.turn.as_deref() == Some("turn-7")));
}

/// A credential in a tool's input or output never leaves the daemon: neither
/// the page label nor the on-demand detail carries it.
#[test]
fn a_token_in_tool_output_never_leaves_the_daemon() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("supervisor.jsonl");
    let token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    write_lines(&path, &[
        json!({"type":"assistant","timestamp":at(1),"message":{"role":"assistant","content":[
            {"type":"tool_use","id":"call-1","name":"Bash","input":{"command":format!("gh auth status --token {token}")}}
        ]}}),
        json!({"type":"user","timestamp":at(2),"message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"call-1","content":[{"type":"text","text":format!("Logged in\nGH_TOKEN={token}\nAuthorization: Bearer sk-live-secret-value")}]}
        ]}}),
    ]);
    let (scope, stores, mut index) = (scope(cas_mux::SupervisorCli::Claude), WorkEventStores::default(), WorkEventIndex::default());
    let resolve = |_: &SupervisorSource| Some(path.clone());
    let page = index.page(&scope, &stores, &resolve, "page".into(), None, 10, None).unwrap();
    let DaemonMessage::WorkEvents { events, .. } = &page else { panic!("page") };
    let detail = index.detail(&scope, &stores, &resolve, "detail".into(), events[0].id.clone());
    let wire = format!("{}{}", serde_json::to_string(&page).unwrap(), serde_json::to_string(&detail).unwrap());
    for secret in [token, "sk-live-secret-value"] {
        assert!(!wire.contains(secret), "{secret} left the daemon: {wire}");
    }
    assert!(wire.contains("Logged in") && wire.contains("[REDACTED"), "{wire}");
}

/// Task transitions and worker notices for this session, and its workers'
/// hook events, join the supervisor's transcript in one time-ordered stream.
/// Another session's rows never appear, and their detail does not resolve.
#[test]
fn task_db_and_hook_events_join_the_session_stream() {
    let temp = tempfile::tempdir().unwrap();
    let cas_dir = temp.path();
    let path = cas_dir.join("supervisor.jsonl");
    claude_turn(&path);
    let queue = SqliteSupervisorQueueStore::open(cas_dir).unwrap();
    queue.init().unwrap();
    let insert = |supervisor: &str, event_type: &str, payload: Value, second: u32| {
        let id = queue.notify(supervisor, event_type, &payload.to_string(), NotificationPriority::High).unwrap();
        let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
        conn.execute("UPDATE supervisor_queue SET created_at = ?1 WHERE id = ?2", rusqlite::params![at(second), id]).unwrap();
        id
    };
    insert("sup-1", "task_lifecycle", json!({"factory_session":"factory-1","task_id":"cas-42","title":"Ship the stream","transition":"task_awaiting_merge","actor":"swift-fox-2"}), 5);
    insert("other-sup", "worker_died", json!({"factory_session":"factory-1","worker_name":"swift-fox-2","detail":"Worker swift-fox-2 died"}), 6);
    let foreign = insert("other-sup", "task_lifecycle", json!({"factory_session":"factory-2","task_id":"cas-99","new_status":"closed"}), 7);
    let events = SqliteEventStore::open(cas_dir).unwrap();
    let commit = cas_types::Event::new(EventType::WorkerGitCommit, cas_types::EventEntityType::Agent, "worker-1", "Committed changes")
        .with_session("worker-1");
    let commit = cas_types::Event { created_at: at(8).parse().unwrap(), ..commit };
    events.record(&commit).unwrap();
    let stranger = cas_types::Event::new(EventType::WorkerGitCommit, cas_types::EventEntityType::Agent, "x", "Committed elsewhere")
        .with_session("someone-else");
    events.record(&cas_types::Event { created_at: at(8).parse().unwrap(), ..stranger }).unwrap();

    let (scope, stores, mut index) = (scope(cas_mux::SupervisorCli::Claude), WorkEventStores::open(cas_dir), WorkEventIndex::default());
    let (events, _, _) = page(&mut index, &scope, &stores, &path, None, 4);
    let shapes: Vec<_> = events.iter().map(|event| (event.kind, event.agent.as_deref(), event.label.as_str())).collect();
    assert_eq!(shapes, vec![
        (WorkEventKind::Todo, Some("calm-otter-1"), "Todos 1/2 · Running tests"),
        (WorkEventKind::Task, Some("swift-fox-2"), "cas-42 parked for merge: Ship the stream"),
        (WorkEventKind::Worker, Some("swift-fox-2"), "Worker swift-fox-2 died"),
        (WorkEventKind::Worker, Some("swift-fox-2"), "Committed changes"),
    ]);
    let DaemonMessage::WorkEventDetail { detail: Some(text), .. } = detail(&mut index, &scope, &stores, &path, &events[1].id) else {
        panic!("queue detail resolves");
    };
    assert!(text.contains("\"task_id\": \"cas-42\""), "{text}");
    let DaemonMessage::WorkEventDetail { detail, .. } = detail(&mut index, &scope, &stores, &path, &format!("q:{foreign:015}")) else {
        unreachable!()
    };
    assert_eq!(detail, None, "another session's row does not resolve");
}

/// cas-ff22: a worker's timeline reads that worker's own transcript and
/// rows, never the supervisor's; the session stream still reads only the
/// supervisor. The change probe moves when either transcript grows, and only
/// a changed probe tells the stream's followers, once each.
#[test]
fn a_worker_timeline_reads_its_own_transcript_and_the_probe_tells_followers_of_changes() {
    let temp = tempfile::tempdir().unwrap();
    let supervisor_path = temp.path().join("supervisor.jsonl");
    let worker_path = temp.path().join("worker.jsonl");
    claude_turn(&supervisor_path);
    write_lines(&worker_path, &[json!({"type":"assistant","timestamp":at(6),"message":{"role":"assistant","content":[
        {"type":"tool_use","id":"w-1","name":"Read","input":{"file_path":"/repo/src/limits.rs"}}
    ]}})]);
    let mut scope = scope(cas_mux::SupervisorCli::Claude);
    scope.workers.push(SupervisorSource {
        agent_id: "worker-1".into(),
        agent_name: "swift-fox-2".into(),
        harness_session: "harness-2".into(),
        cli: cas_mux::SupervisorCli::Claude,
        account_dir: None,
    });
    let resolve = |source: &SupervisorSource| {
        Some(if source.agent_id == "worker-1" { worker_path.clone() } else { supervisor_path.clone() })
    };
    let (stores, mut index) = (WorkEventStores::default(), WorkEventIndex::default());
    let events = |index: &mut WorkEventIndex, agent: Option<&str>| match index
        .page(&scope, &stores, &resolve, "page".into(), None, 10, agent)
        .unwrap()
    {
        DaemonMessage::WorkEvents { events, .. } => events,
        other => panic!("not a page: {other:?}"),
    };

    let timeline = events(&mut index, Some("swift-fox-2"));
    assert_eq!(timeline.len(), 1);
    assert_eq!((timeline[0].agent.as_deref(), timeline[0].label.as_str()), (Some("swift-fox-2"), "Read: /repo/src/limits.rs"));
    assert!(events(&mut index, None).iter().all(|event| event.agent.as_deref() == Some("calm-otter-1")));
    let DaemonMessage::WorkEventDetail { detail: Some(text), .. } =
        index.detail(&scope, &stores, &resolve, "detail".into(), timeline[0].id.clone())
    else {
        panic!("a worker's own event resolves");
    };
    assert!(text.contains("/repo/src/limits.rs"), "{text}");

    let mut service = WorkEventService::default();
    service.subscribers.extend([3, 1]);
    let first = index.fingerprint(&scope, &stores, &resolve).unwrap();
    assert!(service.settle_probe(Some(first.clone())).is_empty(), "the first probe is the baseline");
    assert!(service.settle_probe(Some(index.fingerprint(&scope, &stores, &resolve).unwrap())).is_empty(), "nothing new, nothing sent");
    write_lines(&worker_path, &[json!({"type":"assistant","timestamp":at(7),"message":{"role":"assistant","content":[
        {"type":"tool_use","id":"w-2","name":"Bash","input":{"command":"cargo test","description":"Run the tests"}}
    ]}})]);
    let grown = index.fingerprint(&scope, &stores, &resolve).unwrap();
    assert_ne!(grown, first, "a worker's transcript growing moves the probe without indexing it");
    let notices = service.settle_probe(Some(grown));
    let told: Vec<usize> = notices.iter().map(|notice| notice.client_id).collect();
    assert_eq!(told, vec![1, 3]);
    assert!(notices.iter().all(|notice| matches!(notice.message, DaemonMessage::WorkEventsAppended { .. })));
    assert!(service.probe_due(Instant::now(), |client| client == 3), "a live follower keeps the probe running");
    assert!(!service.probe_due(Instant::now(), |_| false), "no live follower, no probe");
}

/// Measurement on a real transcript (cas-8f88 acceptance): run with
/// `CAS_WORK_EVENTS_MEASURE=<transcript.jsonl>` and `--ignored`. It prints
/// the transcript's recorded token usage before and after serving every page
/// and the newest details, plus timings.
#[test]
#[ignore = "measures a real transcript named by CAS_WORK_EVENTS_MEASURE"]
fn measure_real_transcript() {
    let Ok(live) = std::env::var("CAS_WORK_EVENTS_MEASURE") else { return };
    // A copy, so a live supervisor appending to its transcript cannot move
    // the before/after comparison.
    let temp = tempfile::tempdir().unwrap();
    let live = PathBuf::from(live);
    let path = temp.path().join(live.file_name().unwrap());
    std::fs::copy(&live, &path).unwrap();
    let cli = if path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("rollout-")) {
        cas_mux::SupervisorCli::Codex
    } else {
        cas_mux::SupervisorCli::Claude
    };
    let usage = |path: &Path| -> (u64, usize, String) {
        let bytes = std::fs::read(path).unwrap();
        let tokens = String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|value| {
                value.pointer("/message/usage").or_else(|| value.pointer("/payload/info/last_token_usage")).cloned()
            })
            .map(|usage| {
                ["input_tokens", "output_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"]
                    .iter()
                    .map(|key| usage[*key].as_u64().unwrap_or(0))
                    .sum::<u64>()
            })
            .sum();
        use sha2::Digest;
        (tokens, bytes.len(), format!("{:x}", sha2::Sha256::digest(&bytes)))
    };
    let before = usage(&path);
    let (scope, stores, mut index) = (scope(cli), WorkEventStores::default(), WorkEventIndex::default());
    let started = std::time::Instant::now();
    let (newest, mut has_earlier, mut cursor) = page(&mut index, &scope, &stores, &path, None, 100);
    let first_page = started.elapsed();
    let (mut pages, mut events) = (1, newest.len());
    while has_earlier {
        let (older, more, next) = page(&mut index, &scope, &stores, &path, cursor.as_deref(), 100);
        (pages, events, has_earlier, cursor) = (pages + 1, events + older.len(), more, next);
    }
    let all_pages = started.elapsed();
    let detail_started = std::time::Instant::now();
    for event in newest.iter().rev().take(20) {
        detail(&mut index, &scope, &stores, &path, &event.id);
    }
    let details = detail_started.elapsed();
    let after = usage(&path);
    println!(
        "transcript_bytes={} recorded_tokens_before={} recorded_tokens_after={} sha256_unchanged={} pages={pages} events={events} first_page_ms={} all_pages_ms={} twenty_details_ms={}",
        before.1,
        before.0,
        after.0,
        before.2 == after.2,
        first_page.as_millis(),
        all_pages.as_millis(),
        details.as_millis()
    );
    assert_eq!(before, after);
}

#[test]
fn v2_tool_payloads_use_explicit_inputs_and_results() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("claude.jsonl");
    write_lines(
        &path,
        &[
            json!({"type":"assistant","timestamp":at(1),"message":{"content":[
                {"type":"tool_use","id":"r","name":"Read","input":{"file_path":"src/lib.rs"}},
                {"type":"tool_use","id":"s","name":"Grep","input":{"pattern":"needle","path":"src"}},
                {"type":"tool_use","id":"c","name":"Bash","input":{"command":"git status","cwd":"/repo"}},
                {"type":"tool_use","id":"u","name":"future_tool","input":{}}
            ]}}),
            json!({"type":"user","timestamp":at(2),"toolUseResult":{"exitCode":7},"message":{"content":[
                {"type":"tool_result","tool_use_id":"c","is_error":true,"content":"failed"}
            ]}}),
        ],
    );
    let (scope, stores, mut index) = (
        scope(cas_mux::SupervisorCli::Claude),
        WorkEventStores::default(),
        WorkEventIndex::default(),
    );
    let (events, _, _) = page(&mut index, &scope, &stores, &path, None, 10);
    let wire = serde_json::to_value(events).unwrap();
    assert_eq!(wire[0]["tool"]["operation"], "read");
    assert_eq!(wire[0]["tool"]["targets"], json!(["src/lib.rs"]));
    assert_eq!(wire[0]["tool"]["counts"]["files"], 1);
    assert_eq!(wire[1]["tool"]["operation"], "search");
    assert_eq!(wire[2]["tool"]["command"], "git status");
    assert_eq!(wire[2]["tool"]["exit_code"], 7);
    assert_eq!(wire[3]["tool"]["operation"], "other");
}

#[test]
fn v2_lifecycle_updates_existing_calls_and_notifies_on_completion() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rollout.jsonl");
    write_lines(
        &path,
        &[
            json!({"timestamp":at(1),"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"git status\"}","call_id":"c"}}),
        ],
    );
    let (scope, stores, mut index) = (
        scope(cas_mux::SupervisorCli::Codex),
        WorkEventStores::default(),
        WorkEventIndex::default(),
    );
    let resolve = |_: &SupervisorSource| Some(path.clone());
    let (started, _, _) = page(&mut index, &scope, &stores, &path, None, 10);
    let before = index.fingerprint(&scope, &stores, &resolve).unwrap();
    assert_eq!(
        serde_json::to_value(&started[0]).unwrap()["lifecycle"]["stage"],
        "start"
    );
    write_lines(
        &path,
        &[
            json!({"timestamp":at(2),"type":"response_item","payload":{"type":"function_call_output","call_id":"c","output":"{\"exit_code\":0,\"output\":\"clean\"}"}}),
        ],
    );
    let (ended, _, _) = page(&mut index, &scope, &stores, &path, None, 10);
    assert_eq!(started[0].id, ended[0].id);
    let wire = serde_json::to_value(&ended[0]).unwrap();
    assert_eq!(wire["lifecycle"]["stage"], "end");
    assert_eq!(wire["lifecycle"]["outcome"], "success");
    assert_ne!(
        index.fingerprint(&scope, &stores, &resolve).unwrap(),
        before,
        "settling the newest row must wake live clients"
    );
    write_lines(
        &path,
        &[
            json!({"timestamp":at(3),"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"long command\"}","call_id":"run"}}),
            json!({"timestamp":at(4),"type":"response_item","payload":{"type":"function_call_output","call_id":"run","output":[{"type":"input_text","text":"{\"session_id\":42,\"output\":\"working\"}"}]}}),
        ],
    );
    let (running, _, _) = page(&mut index, &scope, &stores, &path, None, 10);
    assert_eq!(
        serde_json::to_value(&running[1]).unwrap()["lifecycle"]["phase"],
        "running"
    );
    write_lines(
        &path,
        &[
            json!({"timestamp":at(5),"type":"response_item","payload":{"type":"function_call","name":"write_stdin","arguments":"{\"session_id\":42,\"chars\":\"\"}","call_id":"poll"}}),
            json!({"timestamp":at(6),"type":"response_item","payload":{"type":"function_call_output","call_id":"poll","output":"Chunk ID: test\nProcess exited with code 3\nFinal output:\nfailed"}}),
        ],
    );
    let (ended, _, _) = page(&mut index, &scope, &stores, &path, None, 10);
    let wire = serde_json::to_value(&ended[1]).unwrap();
    assert_eq!(wire["lifecycle"]["stage"], "end");
    assert_eq!(wire["lifecycle"]["outcome"], "error");
    assert_eq!(wire["tool"]["exit_code"], 3);
}

#[test]
fn v2_reasoning_publishes_only_recorded_summary_and_detail() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rollout.jsonl");
    write_lines(
        &path,
        &[
            json!({"timestamp":at(1),"type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"Published heading\nPublished detail"}],"encrypted_content":"PRIVATE_SENTINEL","content":[{"text":"PRIVATE_SENTINEL"}]}}),
        ],
    );
    let (scope, stores, mut index) = (
        scope(cas_mux::SupervisorCli::Codex),
        WorkEventStores::default(),
        WorkEventIndex::default(),
    );
    let (events, _, _) = page(&mut index, &scope, &stores, &path, None, 10);
    let wire = serde_json::to_value(&events[0]).unwrap();
    assert_eq!(wire["reasoning"]["summary"], "Published heading");
    assert_eq!(wire["reasoning"]["source"], "codex_summary");
    let detail =
        serde_json::to_string(&detail(&mut index, &scope, &stores, &path, &events[0].id)).unwrap();
    assert!(detail.contains("Published detail"));
    assert!(!detail.contains("PRIVATE_SENTINEL"));
}

#[test]
fn v2_changed_files_are_per_turn_successful_edit_provenance() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("claude.jsonl");
    write_lines(
        &path,
        &[
            json!({"type":"user","promptId":"p1","timestamp":at(1),"message":{"content":"edit"}}),
            json!({"type":"assistant","timestamp":at(2),"message":{"content":[
                {"type":"tool_use","id":"a","name":"Edit","input":{"file_path":"a.rs"}},
                {"type":"tool_use","id":"b","name":"Write","input":{"file_path":"b.rs"}}
            ]}}),
            json!({"type":"user","timestamp":at(3),"message":{"content":[
                {"type":"tool_result","tool_use_id":"a","content":"done"},
                {"type":"tool_result","tool_use_id":"b","is_error":true,"content":"refused"}
            ]}}),
        ],
    );
    let (scope, stores, mut index) = (
        scope(cas_mux::SupervisorCli::Claude),
        WorkEventStores::default(),
        WorkEventIndex::default(),
    );
    let (events, _, _) = page(&mut index, &scope, &stores, &path, None, 10);
    let wire = serde_json::to_value(&events).unwrap();
    assert_eq!(
        wire[0]["changed_files"]["files"],
        json!([{"path":"a.rs","operation":"edit","source_event_id":events[0].id}])
    );
    assert_eq!(
        wire[0]["changed_files"]["provenance"],
        "successful_tool_calls"
    );
    assert_eq!(wire[0]["changed_files"]["complete"], false);
    assert!(
        wire[1].get("changed_files").is_none(),
        "failed edit is not a changed file"
    );
}
