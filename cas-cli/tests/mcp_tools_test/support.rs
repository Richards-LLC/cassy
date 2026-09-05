use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tempfile::TempDir;

use cas::mcp::CasCore;
use cas::store::{
    open_agent_store, open_rule_store, open_skill_store, open_store, open_task_store,
};
use cas::types::{Agent, AgentRole};

/// Shared process-wide lock for tests that mutate environment variables
/// (`CAS_AGENT_ROLE`, factory harness vars, etc.). Cargo runs integration
/// tests concurrently by default and env vars are a global, so without
/// this lock one test's `ScopedSupervisorEnv::new()` can be clobbered by
/// another test's `setup_cas()` mid-flight, silently flipping the first
/// test into the non-supervisor branch and producing nondeterministic
/// failures (cas-3bd4: the bypass close test was flaking as the Skipped
/// verification row lost the race to the dispatch Error row).
///
/// All env-sensitive tests must acquire this guard for the full duration
/// of their test body, **after** calling `setup_cas` — see the doc on
/// [`setup_cas`] for the ordering contract.
///
/// The guard also recovers from poisoning: a prior panic must not prevent
/// subsequent tests from acquiring the lock.
pub(crate) fn env_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Helper to create an initialized CAS environment.
///
/// **Ordering contract with `env_test_lock()`:** Tests that need to hold
/// the env lock MUST call `setup_cas()` first and acquire
/// `env_test_lock()` immediately afterwards. `setup_cas` briefly acquires
/// the lock itself for its env mutations, so acquiring it in the test
/// body before calling `setup_cas` would deadlock (std `Mutex` is not
/// re-entrant). With the setup-then-lock order, the two acquisitions
/// happen in series and tests composing `setup_cas` with
/// `ScopedSupervisorEnv` stay race-free relative to other tests that
/// also call `setup_cas`.
/// Repoint `HOME` at a throwaway directory for the whole test process.
///
/// Several production paths resolve the *host* store from `HOME` rather than
/// from the core's `cas_root` — `hooks::context::build_host_constraints_section`
/// reads `~/.cas/cas.db` on every `cas_context` call, which is correct in
/// production and catastrophic in a test: `cas_context` with `scope = "all"`
/// opened the developer's real global store (cas-78c8 / GH #156, caught by the
/// `CAS_TEST_PROTECTED_DBS` tripwire). A temp `.cas` project directory does not
/// protect against this, because the host lookup never consults it.
///
/// Set once per process and never changed, so parallel tests cannot observe a
/// torn value. The directory is deliberately leaked: it must outlive every
/// test, and the OS reclaims it with the rest of `/tmp`.
fn pin_home_to_a_sandbox() {
    static SANDBOX_HOME: OnceLock<PathBuf> = OnceLock::new();
    SANDBOX_HOME.get_or_init(|| {
        let _env_guard = env_test_lock();
        let home = TempDir::new()
            .expect("sandbox HOME should be created")
            .keep();
        let xdg = home.join(".config");
        std::fs::create_dir_all(&xdg).expect("sandbox XDG_CONFIG_HOME should be created");
        // SAFETY: performed once, under the process-wide env lock, before any
        // store in this process resolves a host path from HOME.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("XDG_CONFIG_HOME", &xdg);
        }
        home
    });
}

pub(crate) fn setup_cas() -> (TempDir, CasCore) {
    setup_cas_as(AgentRole::Standard)
}

/// Helper to create an initialized CAS environment and register the test
/// session agent with the requested role.
pub(crate) fn setup_cas_as(role: AgentRole) -> (TempDir, CasCore) {
    pin_home_to_a_sandbox();

    // Clear factory env vars that leak from parent process (e.g., running
    // inside a factory supervisor session). Without this, is_supervisor_from_env()
    // returns true and the assignee_inactive bypass skips verification checks.
    //
    // cas-3bd4: acquire the shared env lock for the duration of these
    // mutations. Other tests that also call `setup_cas` will serialize
    // through this brief critical section, and any test that holds the
    // lock for its body (see `env_test_lock` docs) will block a
    // competing `setup_cas` from clearing env vars mid-test.
    {
        let _env_guard = env_test_lock();
        // SAFETY: we hold the process-wide env lock for the duration of
        // this block; no other test thread can observe a torn env read.
        unsafe {
            std::env::remove_var("CAS_AGENT_ROLE");
            std::env::remove_var("CAS_FACTORY_MODE");
            std::env::remove_var("CAS_FACTORY_SUPERVISOR_CLI");
            std::env::remove_var("CAS_FACTORY_WORKER_CLI");
        }
    }

    let temp = TempDir::new().expect("temp dir should be created");
    let cas_dir = temp.path().join(".cas");
    std::fs::create_dir_all(&cas_dir).expect(".cas dir should be created");

    let store = open_store(&cas_dir).expect("entry store should open");
    store.init().expect("entry store should initialize");

    let task_store = open_task_store(&cas_dir).expect("task store should open");
    task_store.init().expect("task store should initialize");

    let rule_store = open_rule_store(&cas_dir).expect("rule store should open");
    rule_store.init().expect("rule store should initialize");

    let skill_store = open_skill_store(&cas_dir).expect("skill store should open");
    skill_store.init().expect("skill store should initialize");

    let agent_store = open_agent_store(&cas_dir).expect("agent store should open");
    agent_store.init().expect("agent store should initialize");

    // Stable id format `test-session-{pid}` — several tests hardcode this
    // (e.g. operations force-transfer, verification cas-7998 self-assignee).
    // Secondary/rebuilt cores must use `core_with_test_agent` instead of bare
    // `CasCore::with_daemon` (cas-48e6).
    let session_id = format!("test-session-{}", std::process::id());
    let agent = Agent::new_with_role(session_id.clone(), "test-agent".to_string(), role);
    agent_store
        .register(&agent)
        .expect("test agent should register");

    let core = CasCore::with_daemon(cas_dir, None, None);
    core.set_agent_id_for_testing(session_id);

    (temp, core)
}

/// Build a `CasCore` on an existing `.cas` directory with a **registered** test
/// agent identity.
///
/// Use this whenever a test rebuilds a core after `setup_cas()` (e.g. after
/// writing `config.toml` so `OnceLock` config reloads, or to model a second
/// worker process). Bare `CasCore::with_daemon` has no `agent_id` and no
/// SessionStart mapping file, so `task.start` fails with:
///
/// ```text
/// Agent not registered. … Original error: No such file or directory (os error 2)
/// ```
///
/// when `CAS_SESSION_ID` is unset (cas-48e6). The ENOENT is from
/// `agent_id::read_session_for_mcp` — the production SessionStart hook path.
/// Ambient `CAS_SESSION_ID` masks the bug via env auto-register, which is why
/// the suite can stay green inside a factory session and red on a clean
/// checkout.
pub(crate) fn core_with_test_agent(cas_dir: impl AsRef<Path>) -> CasCore {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let cas_dir: PathBuf = cas_dir.as_ref().to_path_buf();
    let session_id = format!(
        "test-session-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );

    let agent_store = open_agent_store(&cas_dir).expect("agent store should open");
    // Idempotent if setup_cas already initialized the store.
    let _ = agent_store.init();
    let agent = Agent::new(session_id.clone(), "test-agent".to_string());
    agent_store
        .register(&agent)
        .expect("test agent should register");

    let core = CasCore::with_daemon(cas_dir, None, None);
    core.set_agent_id_for_testing(session_id);
    core
}

/// Extract text from a tool result.
pub(crate) fn extract_text(result: rmcp::model::CallToolResult) -> String {
    result
        .content
        .into_iter()
        .filter_map(|content| match content.raw {
            rmcp::model::RawContent::Text(text) => Some(text.text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract entry ID from "Created entry: {id} - {preview}" format.
pub(crate) fn extract_entry_id(text: &str) -> Option<&str> {
    text.split("Created entry: ")
        .nth(1)
        .and_then(|part| part.split(" - ").next())
}

/// Extract task ID from "Created task: {id} - {title}" output.
pub(crate) fn extract_task_id(text: &str) -> Option<&str> {
    text.split("Created task: ")
        .nth(1)
        .and_then(|part| part.split(" - ").next())
        .or_else(|| {
            text.split('[')
                .nth(1)
                .and_then(|part| part.split(']').next())
        })
}

/// Extract rule ID from output.
pub(crate) fn extract_rule_id(text: &str) -> Option<String> {
    text.split('[')
        .nth(1)
        .and_then(|part| part.split(']').next())
        .map(ToString::to_string)
        .or_else(|| {
            text.split("rule-")
                .nth(1)
                .and_then(|part| part.split(|c: char| !c.is_alphanumeric()).next())
                .map(|id| format!("rule-{id}"))
        })
}

/// Extract skill ID from output.
pub(crate) fn extract_skill_id(text: &str) -> Option<String> {
    text.split("Created skill: ")
        .nth(1)
        .and_then(|part| part.split(" - ").next())
        .and_then(|part| part.split_whitespace().next())
        .filter(|id| id.starts_with("cas-"))
        .map(ToString::to_string)
        .or_else(|| {
            text.split('[')
                .nth(1)
                .and_then(|part| part.split(']').next())
                .map(ToString::to_string)
        })
}

#[cfg(test)]
mod tests {
    use super::extract_skill_id;

    #[test]
    fn extract_skill_id_stops_before_degraded_validation_warning() {
        let response = "Created skill: cas-skf7\nWARNING: network isolation is unavailable; bubblewrap was not found, so validation ran in degraded plain-shell mode";

        assert_eq!(extract_skill_id(response).as_deref(), Some("cas-skf7"));
    }
}
