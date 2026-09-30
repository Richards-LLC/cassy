pub(crate) use crate::test_env_guard::TestEnvGuard;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::TempDir;

use cas::mcp::CasCore;
use cas::store::{
    open_agent_store, open_rule_store, open_skill_store, open_store, open_task_store,
};
use cas::types::{Agent, AgentRole};

/// Initialize a CAS fixture under the caller's canonical environment guard.
/// Acquire `TestEnvGuard::temp_home()` before setup and retain it through all
/// handler awaits. Helpers borrow this owner; none acquires another lock.
/// The temporary HOME protects host stores and is restored even on unwind.
pub(crate) fn setup_cas(env: &mut TestEnvGuard) -> (TempDir, CasCore) {
    setup_cas_as(env, AgentRole::Standard)
}

/// Clear inherited factory identity before registering a fixture agent.
fn scrub_factory_identity_env(env: &mut TestEnvGuard) {
    for key in [
        "CAS_FACTORY_SESSION",
        "CAS_AGENT_ROLE",
        "CAS_AGENT_NAME",
        "CAS_SUPERVISOR_NAME",
        "CAS_AGENT_ID",
    ] {
        env.remove(key);
    }
}

/// Register the test session agent with the requested role, borrowing the
/// test's environment owner for setup and every later handler call.
pub(crate) fn setup_cas_as(env: &mut TestEnvGuard, role: AgentRole) -> (TempDir, CasCore) {
    let xdg = env.home().join(".config");
    std::fs::create_dir_all(&xdg).expect("sandbox XDG_CONFIG_HOME should be created");
    env.set("XDG_CONFIG_HOME", &xdg);
    scrub_factory_identity_env(env);
    for key in [
        "CAS_FACTORY_MODE",
        "CAS_FACTORY_SUPERVISOR_CLI",
        "CAS_FACTORY_WORKER_CLI",
    ] {
        env.remove(key);
    }

    let temp = TempDir::new().expect("temp dir should be created");
    let cas_dir = temp.path().join(".cas");
    std::fs::create_dir_all(&cas_dir).expect(".cas dir should be created");
    env.set("CAS_ROOT", &cas_dir);

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
pub(crate) fn core_with_test_agent(env: &mut TestEnvGuard, cas_dir: impl AsRef<Path>) -> CasCore {
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
    scrub_factory_identity_env(env);
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

/// Register `path` in the sandboxed host known-repos registry, the list the
/// cas-caae foreign-project write guard reads. Call after `setup_cas` so HOME
/// already points at the sandbox.
pub(crate) fn register_host_project(path: &str) {
    use cas::store::KnownRepoStore;
    cas::store::known_repos::ensure_host_schema().expect("sandbox host registry schema");
    cas::store::known_repos::open_host_known_repo_store()
        .expect("sandbox host registry")
        .upsert(std::path::Path::new(path))
        .expect("register sandbox host project");
}

/// A temporary override inside a test that already owns the canonical guard.
/// Borrowing prevents the owner from dropping while these values are active.
pub(crate) struct ScopedFactoryEnv<'a> {
    env: &'a mut TestEnvGuard,
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl<'a> ScopedFactoryEnv<'a> {
    pub(crate) fn apply(env: &'a mut TestEnvGuard, vars: &[(&'static str, Option<&str>)]) -> Self {
        let mut saved = Vec::with_capacity(vars.len());
        for (key, desired) in vars {
            saved.push((*key, std::env::var_os(key)));
            match desired {
                Some(value) => env.set(key, value),
                None => env.remove(key),
            }
        }
        Self { env, saved }
    }

    pub(crate) fn guard(&mut self) -> &mut TestEnvGuard {
        self.env
    }
}

fn restore_scoped_env(env: &mut TestEnvGuard, saved: &[(&str, Option<std::ffi::OsString>)]) {
    for (key, prior) in saved.iter().rev() {
        match prior {
            Some(value) => env.set(key, value),
            None => env.remove(key),
        }
    }
}

impl Drop for ScopedFactoryEnv<'_> {
    fn drop(&mut self) {
        restore_scoped_env(self.env, &self.saved);
    }
}
