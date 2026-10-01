//! Replacement coverage through the factory MCP dispatcher and real Stop handler.

use std::path::PathBuf;

use cas::hooks::{HookInput, HookOutput, handle_stop};
use cas::mcp::{CasCore, CasService};
use cas::store::{init_cas_dir, open_loop_store, open_task_store_local};
use cas::types::{LoopStatus, Task};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorCode};
use serde_json::{Value, json};

use crate::test_env_guard::TestEnvGuard;

struct Fixture {
    root: PathBuf,
    service: CasService,
}

impl Fixture {
    fn new(env: &mut TestEnvGuard) -> Self {
        let project = env.home().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let root = init_cas_dir(&project).unwrap();
        env.set("CAS_ROOT", &root);
        env.set("XDG_CONFIG_HOME", env.home().join(".config"));
        let core = CasCore::with_daemon(root.clone(), None, None);
        #[cfg(feature = "mcp-proxy")]
        let service = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let service = CasService::new(core);
        Self { root, service }
    }

    async fn factory(&self, value: Value) -> Result<CallToolResult, rmcp::ErrorData> {
        self.service
            .factory(Parameters(serde_json::from_value(value).unwrap()))
            .await
    }

    async fn start(&self, session: &str, options: Value) -> String {
        let mut request = options;
        request["action"] = json!("loop_start");
        request["session_id"] = json!(session);
        if request.get("prompt").is_none() {
            request["prompt"] = json!("Implement the feature");
        }
        let output = text(self.factory(request).await.expect("factory loop_start"));
        let stored = open_loop_store(&self.root)
            .unwrap()
            .get_active_for_session(session)
            .unwrap()
            .expect("persisted active loop");
        assert!(output.contains(&stored.id), "{output}");
        stored.id
    }

    fn stop(&self, session: &str, transcript: Option<&std::path::Path>) -> HookOutput {
        let input: HookInput = serde_json::from_value(json!({
            "session_id": session,
            "cwd": self.root.parent().unwrap(),
            "hook_event_name": "Stop",
            "transcript_path": transcript,
        }))
        .unwrap();
        handle_stop(&input, Some(&self.root)).expect("real Stop handler")
    }
}

fn text(result: CallToolResult) -> String {
    assert_ne!(result.is_error, Some(true), "{result:?}");
    result
        .content
        .iter()
        .filter_map(|content| content.as_text().map(|text| text.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn factory_loop_start_and_status_persist_default_and_explicit_options() {
    let mut env = TestEnvGuard::temp_home();
    let fixture = Fixture::new(&mut env);
    for (session, options, prompt, promise, maximum) in [
        ("default", json!({"prompt": "x"}), "x", None, 0),
        (
            "configured",
            json!({"prompt": "Build the thing", "completion_promise": "DONE", "max_iterations": 5}),
            "Build the thing",
            Some("DONE"),
            5,
        ),
    ] {
        let id = fixture.start(session, options).await;
        let stored = open_loop_store(&fixture.root).unwrap().get(&id).unwrap();
        assert_eq!(stored.status, LoopStatus::Active);
        assert_eq!(stored.iteration, 1);
        assert_eq!(stored.prompt, prompt);
        assert_eq!(stored.completion_promise.as_deref(), promise);
        assert_eq!(stored.max_iterations, maximum);
        let status = text(
            fixture
                .factory(json!({"action": "loop_status", "session_id": session}))
                .await
                .unwrap(),
        );
        assert!(
            status.contains(&id)
                && status.contains("Status: active")
                && status.contains("Iteration: 1"),
            "{status}"
        );
    }
}

#[tokio::test]
async fn factory_loop_duplicate_is_refused_without_replacing_the_active_row() {
    let mut env = TestEnvGuard::temp_home();
    let fixture = Fixture::new(&mut env);
    let id = fixture.start("same-session", json!({})).await;
    let error = fixture
        .factory(
            json!({"action": "loop_start", "session_id": "same-session", "prompt": "Replacement"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::INVALID_REQUEST);
    assert!(error.message.contains("already has an active loop"));
    let store = open_loop_store(&fixture.root).unwrap();
    assert_eq!(
        store
            .get_active_for_session("same-session")
            .unwrap()
            .unwrap()
            .id,
        id
    );
    assert_eq!(store.list_recent(10).unwrap().len(), 1);
    assert_eq!(store.get(&id).unwrap().prompt, "Implement the feature");
}

#[tokio::test]
async fn factory_loop_cancel_persists_reason_and_missing_session_is_a_noop() {
    let mut env = TestEnvGuard::temp_home();
    let fixture = Fixture::new(&mut env);
    let id = fixture.start("cancel-session", json!({})).await;
    let result = text(fixture.factory(json!({"action": "loop_cancel", "session_id": "cancel-session", "reason": "User cancelled"})).await.unwrap());
    assert!(
        result.contains("cancelled") && result.contains("User cancelled"),
        "{result}"
    );
    let store = open_loop_store(&fixture.root).unwrap();
    let cancelled = store.get(&id).unwrap();
    assert_eq!(cancelled.status, LoopStatus::Cancelled);
    assert_eq!(cancelled.end_reason.as_deref(), Some("User cancelled"));
    assert!(
        store
            .get_active_for_session("cancel-session")
            .unwrap()
            .is_none()
    );
    for action in ["loop_status", "loop_cancel"] {
        let response = text(
            fixture
                .factory(json!({"action": action, "session_id": "nonexistent-session"}))
                .await
                .unwrap(),
        );
        assert_eq!(response, "No active loop for this session");
    }
    assert_eq!(
        store.list_recent(10).unwrap().len(),
        1,
        "missing-session cancel creates no rows"
    );
}

#[tokio::test]
async fn stop_hook_without_active_loop_allows_exit_and_creates_no_loop() {
    let mut env = TestEnvGuard::temp_home();
    let fixture = Fixture::new(&mut env);
    let output = fixture.stop("no-loop", None);
    assert_eq!(output.decision, None);
    assert_eq!(output.reason, None);
    let store = open_loop_store(&fixture.root).unwrap();
    assert!(store.get_active_for_session("no-loop").unwrap().is_none());
    assert!(store.list_recent(10).unwrap().is_empty());
}

#[tokio::test]
async fn stop_hook_blocks_with_prompt_and_persists_each_iteration() {
    let mut env = TestEnvGuard::temp_home();
    let fixture = Fixture::new(&mut env);
    let id = fixture
        .start("iterate", json!({"prompt": "Keep working on this"}))
        .await;
    for iteration in [2, 3] {
        let output = fixture.stop("iterate", None);
        assert_eq!(output.decision.as_deref(), Some("block"));
        assert!(
            output
                .reason
                .as_deref()
                .unwrap()
                .contains("Keep working on this")
        );
        let stored = open_loop_store(&fixture.root).unwrap().get(&id).unwrap();
        assert_eq!(stored.iteration, iteration);
        assert_eq!(stored.status, LoopStatus::Active);
    }
}

#[tokio::test]
async fn stop_hook_releases_exit_and_persists_max_iteration_terminal_state() {
    let mut env = TestEnvGuard::temp_home();
    let fixture = Fixture::new(&mut env);
    let id = fixture.start("bounded", json!({"max_iterations": 2})).await;
    assert_eq!(
        fixture.stop("bounded", None).decision.as_deref(),
        Some("block")
    );
    assert_eq!(fixture.stop("bounded", None).decision, None);
    let store = open_loop_store(&fixture.root).unwrap();
    let stored = store.get(&id).unwrap();
    assert_eq!(stored.status, LoopStatus::MaxIterations);
    assert_eq!(stored.iteration, 2);
    assert!(stored.ended_at.is_some());
    assert!(store.get_active_for_session("bounded").unwrap().is_none());
}

#[tokio::test]
async fn stop_hook_completes_only_after_the_promised_transcript_output() {
    let mut env = TestEnvGuard::temp_home();
    let fixture = Fixture::new(&mut env);
    let id = fixture
        .start("promise", json!({"completion_promise": "DONE"}))
        .await;
    let transcript = env.home().join("transcript.jsonl");
    std::fs::write(&transcript, "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Still working\"}]}}\n").unwrap();
    assert_eq!(
        fixture
            .stop("promise", Some(&transcript))
            .decision
            .as_deref(),
        Some("block")
    );
    let store = open_loop_store(&fixture.root).unwrap();
    assert_eq!(store.get(&id).unwrap().status, LoopStatus::Active);
    std::fs::write(&transcript, "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<promise>DONE</promise>\"}]}}\n").unwrap();
    assert_eq!(fixture.stop("promise", Some(&transcript)).decision, None);
    let completed = store.get(&id).unwrap();
    assert_eq!(completed.status, LoopStatus::Completed);
    assert_eq!(
        completed.end_reason.as_deref(),
        Some("Promise 'DONE' detected")
    );
    assert!(store.get_active_for_session("promise").unwrap().is_none());
}

#[tokio::test]
async fn loop_linked_task_receives_iteration_and_completion_notes() {
    let mut env = TestEnvGuard::temp_home();
    let fixture = Fixture::new(&mut env);
    let tasks = open_task_store_local(&fixture.root).unwrap();
    tasks
        .add(&Task::new(
            "cas-c034".to_string(),
            "Linked loop task".to_string(),
        ))
        .unwrap();
    let id = fixture
        .start(
            "linked",
            json!({"task_id": "cas-c034", "max_iterations": 2}),
        )
        .await;
    assert_eq!(
        open_loop_store(&fixture.root)
            .unwrap()
            .get(&id)
            .unwrap()
            .task_id
            .as_deref(),
        Some("cas-c034")
    );
    assert_eq!(
        fixture.stop("linked", None).decision.as_deref(),
        Some("block")
    );
    assert!(
        tasks
            .get("cas-c034")
            .unwrap()
            .notes
            .contains("Loop iteration 2 started")
    );
    assert_eq!(fixture.stop("linked", None).decision, None);
    let notes = tasks.get("cas-c034").unwrap().notes;
    assert!(
        notes.contains(&id) && notes.contains("max iterations reached after 2 iterations"),
        "{notes}"
    );
}
