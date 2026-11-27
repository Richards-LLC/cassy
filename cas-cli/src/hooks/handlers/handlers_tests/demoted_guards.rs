//! A demoted pre-tool guard records its refusal and lets the call proceed;
//! the kept guards still deny. Payloads are Claude's PreToolUse wire shape.
use crate::hooks::handlers::handle_pre_tool_use;
use crate::test_support::TestEnvGuard;
use cas_core::hooks::types::{HookInput, HookOutput};
use cas_store::EventStore;
use cas_types::{EventType, Task, TaskStatus};
use serde_json::{Value, json};

const WORKER: &str = "quiet-wren-12";

fn hook_input(tool: &str, tool_input: Value, cwd: &std::path::Path) -> HookInput {
    serde_json::from_value(json!({
        "session_id": "demoted-guard-session",
        "transcript_path": "/tmp/demoted-guard/transcript.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "PreToolUse",
        "tool_name": tool,
        "tool_input": tool_input,
        "tool_use_id": "toolu_01DemotedGuard",
    }))
    .expect("Claude's PreToolUse payload must deserialize")
}

fn bash(command: &str, cwd: &std::path::Path) -> HookInput {
    hook_input("Bash", json!({ "command": command }), cwd)
}

struct Worker {
    _env: TestEnvGuard,
    project: tempfile::TempDir,
    cas_root: std::path::PathBuf,
}

fn worker() -> Worker {
    let mut env = TestEnvGuard::temp_home();
    let project = tempfile::tempdir().unwrap();
    let config_home = project.path().join("xdg");
    std::fs::create_dir_all(&config_home).unwrap();
    env.set("XDG_CONFIG_HOME", &config_home);
    env.set("CAS_CLONE_PATH", project.path());
    env.set("CAS_AGENT_ROLE", "worker");
    env.set("CAS_AGENT_NAME", WORKER);
    env.set("CAS_FACTORY_MODE", "1");
    env.set("CAS_FACTORY_WORKER_CLI", "claude");
    env.remove("CAS_HOOK_HARNESS");
    let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
    Worker { _env: env, project, cas_root }
}

impl Worker {
    fn run(&self, input: &HookInput) -> HookOutput {
        handle_pre_tool_use(input, Some(&self.cas_root)).unwrap()
    }

    fn denied_by(&self, input: &HookInput) -> Vec<&'static str> {
        let output = self.run(input);
        assert!(crate::test_support::pretool_blocked(&output), "not denied: {:?}", input.tool_input);
        let value = serde_json::to_value(&output).unwrap();
        crate::gates::codes_in(value["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap())
    }

    fn proceeds(&self, input: &HookInput) {
        let output = self.run(input);
        assert!(!crate::test_support::pretool_blocked(&output), "denied: {:?}: {output:?}", input.tool_input);
    }
}

#[test]
fn a_demoted_guard_proceeds_and_records_one_refusal_fact() {
    let worker = worker();
    let tasks = crate::store::open_task_store(&worker.cas_root).unwrap();
    let mut task = Task::new("cas-demo".into(), "Demo task".into());
    task.status = TaskStatus::InProgress;
    task.assignee = Some(WORKER.into());
    tasks.add(&task).unwrap();

    let output = worker.run(&bash("cargo fmt --all", worker.project.path()));
    assert!(!crate::test_support::pretool_blocked(&output));
    assert_eq!(crate::test_support::pretool_warned(&output), ["H-05"]);

    let events = crate::store::open_event_store(&worker.cas_root).unwrap();
    let facts = events.list_by_type(EventType::GateRefused, 10).unwrap();
    assert_eq!(facts.len(), 1, "{facts:?}");
    assert_eq!(facts[0].entity_id, "H-05");
    let metadata = facts[0].metadata.as_ref().unwrap();
    assert_eq!(metadata["outcome"], "allowed");
    assert_eq!(metadata["actor"], WORKER);
    assert_eq!(metadata["harness"], "claude");
    assert_eq!(metadata["task_id"], "cas-demo");
}

#[test]
fn kept_guards_still_deny_over_real_hook_json() {
    let worker = worker();
    let project = worker.project.path();
    let unparseable = "n=$(cat src/n); printf '%s' $((n+1))";
    for (input, code) in [
        (hook_input("Read", json!({"file_path": project.join(".env")}), project), "H-33"),
        (hook_input("Write", json!({"file_path": project.join(".env.production"), "content": "K=v"}), project), "H-33"),
        (bash("cat ~/.config/neonctl/credentials.json", project), "H-09"),
        (bash(&format!("{unparseable}; cat ~/.config/neonctl/credentials.json"), project), "H-09"),
        (bash(&format!("{unparseable}; echo x > /etc/cas-demo-outside"), project), "H-29"),
        (bash(&format!("{unparseable}; cat .env"), project), "H-33"),
        (bash(&format!("echo hi # <<'X'\ntouch /etc/cas-demo-outside\nX\n{unparseable}"), project), "H-29"),
        (bash(&format!("echo \"$(touch /etc/cas-demo-outside)\"; {unparseable}"), project), "H-29"),
        (bash("export CAS_AGENT_ROLE=supervisor", project), "H-12"),
        (bash("vercel ls; export CAS_AGENT_ROLE=supervisor", project), "H-12"),
        (bash("neonctl branches reset production", project), "H-17"),
        (bash("git push --no-verify origin HEAD; pkill -f cargo", project), "H-16"),
        (bash("cargo fmt --all; echo x > /etc/cas-demo-outside", project), "H-29"),
    ] {
        assert!(worker.denied_by(&input).contains(&code), "{:?}", input.tool_input);
    }
}

#[test]
fn unparseable_commands_without_secrets_or_outside_writes_proceed() {
    let worker = worker();
    let project = worker.project.path();
    let unparseable = "n=$(cat src/n); printf '%s' $((n+1))";
    for command in [
        unparseable.to_string(),
        format!("{unparseable} > src/count.txt"),
        "echo x > \"$UNSET_ROOT/out.txt\"".to_string(),
    ] {
        worker.proceeds(&bash(&command, project));
    }
}

#[test]
fn only_gitignored_test_env_files_pass_while_other_env_files_stay_protected() {
    let worker = worker();
    let project = worker.project.path();
    assert!(std::process::Command::new("git").args(["init", "-q"]).arg(project).status().unwrap().success());
    std::fs::write(project.join(".gitignore"), "/.env.test\n/.env.test.local\n").unwrap();
    std::fs::create_dir_all(project.join("sub")).unwrap();
    let write = |path: std::path::PathBuf| hook_input("Write", json!({"file_path": path, "content": "K=v"}), project);
    for name in [".env.test", ".env.test.local"] {
        worker.proceeds(&write(project.join(name)));
    }
    for path in [
        project.join(".env"),
        project.join(".env.example"),
        project.join(".env.production.testing"),
        project.join("sub/.env.test"),
    ] {
        assert!(worker.denied_by(&write(path.clone())).contains(&"H-33"), "{path:?}");
    }
}
