//! Recovery hints are commands for the caller, not aliases in user-authored text.
use super::*;
use crate::test_env_guard::TestEnvGuard;

const HARNESSES: [(&str, &str); 4] = [
    ("claude", "mcp__cas__"),
    ("codex", "mcp__cs__"),
    ("grok", "cas__"),
    ("opencode", "cas_"),
];

fn service(harness: &str, role: crate::types::AgentRole) -> (tempfile::TempDir, CasService) {
    let dir = tempfile::TempDir::new().unwrap();
    let core = CasCore::with_daemon(dir.path().join(".cas"), None, None);
    core.register_agent("recovery-caller".into(), "recovery-caller".into(), None).unwrap();
    let store = core.open_agent_store().unwrap();
    let mut agent = store.get("recovery-caller").unwrap();
    agent.role = role;
    agent.metadata.insert("worker_cli".into(), harness.into());
    store.update(&agent).unwrap();
    (dir, CasService::new(core, None))
}

#[tokio::test]
async fn recovery_guidance_missing_parameters_four_harnesses() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_AGENT_ROLE", "worker");
    // Registered caller evidence must win over a stale process hint.
    env.set("CAS_FACTORY_WORKER_CLI", "claude");
    for (harness, prefix) in HARNESSES {
        let (_dir, service) = service(harness, crate::types::AgentRole::Worker);
        for action in ["show", "start", "close", "notes", "claim", "reset", "transfer"] {
            let error = service.task(Parameters(TaskRequest { action: action.into(), ..Default::default() })).await.unwrap_err();
            assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
            assert!(error.data.is_none());
            assert!(error.message.contains(&format!("{prefix}task action={action}")), "{harness}/{action}: {error}");
        }
        let error = service.memory(Parameters(MemoryRequest { action: "get".into(), ..Default::default() })).await.unwrap_err();
        assert!(error.message.contains(&format!("{prefix}memory action=get")), "{error}");
        assert!(!error.message.contains("task ID"), "{error}");
        let error = service.factory_epic_status(FactoryRequest { action: "epic_status".into(), ..Default::default() }).await.unwrap_err();
        assert!(error.message.contains(&format!("{prefix}coordination action=epic_status")), "{error}");
    }
}
