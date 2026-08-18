//! Isolation contract for Cassy-owned, headless LLM subprocesses.
//!
//! These subprocesses are implementation details (knowledge distillation,
//! prompt classification, session summaries), not new user or factory-agent
//! sessions.  They must not inherit the caller's durable Cassy identity.

use std::process::Command;

/// Marks a process as an internal completion whose Claude hooks must be inert.
pub const INTERNAL_LLM_ENV: &str = "CAS_INTERNAL_LLM";

/// Factory identity inherited by a nested harness would make it impersonate
/// the parent worker.  Keep this list narrow: credentials and ordinary Cassy
/// configuration are intentionally unaffected.
pub const FACTORY_IDENTITY_ENV: &[&str] = &[
    "CAS_AGENT_NAME",
    "CAS_AGENT_ROLE",
    "CAS_SESSION_ID",
    "CAS_FACTORY_MODE",
    "CAS_FACTORY_SESSION",
    "CAS_CLONE_PATH",
    "CAS_SUPERVISOR_NAME",
    "CAS_FACTORY_WORKER_CLI",
    "CAS_FACTORY_WORKER_MODEL",
    "CAS_FACTORY_WORKER_EFFORT",
];

/// Apply the isolation contract to a directly-spawned command.
pub fn isolate_command(command: &mut Command) {
    command.env(INTERNAL_LLM_ENV, "1");
    for key in FACTORY_IDENTITY_ENV {
        command.env_remove(key);
    }
}

/// Environment overrides for SDKs which can add/override child variables but
/// cannot express `env_remove`. Empty values are rejected by every factory
/// identity reader and prevent eager MCP registration (`CAS_SESSION_ID`).
pub fn sdk_environment() -> std::collections::HashMap<String, String> {
    let mut env = std::collections::HashMap::from([(INTERNAL_LLM_ENV.to_string(), "1".into())]);
    env.extend(
        FACTORY_IDENTITY_ENV
            .iter()
            .map(|key| ((*key).to_string(), String::new())),
    );
    env
}

/// True only inside a Cassy-owned headless completion.
pub fn is_internal_invocation() -> bool {
    std::env::var(INTERNAL_LLM_ENV).as_deref() == Ok("1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_commands_remove_factory_identity_and_mark_internal() {
        let mut command = Command::new("claude");
        isolate_command(&mut command);
        let env: std::collections::HashMap<_, _> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();

        assert_eq!(env.get(INTERNAL_LLM_ENV), Some(&Some("1".to_string())));
        for key in FACTORY_IDENTITY_ENV {
            assert_eq!(env.get(*key), Some(&None), "{key} must be removed");
        }
    }

    #[test]
    fn sdk_environment_blanks_every_factory_identity() {
        let env = sdk_environment();
        assert_eq!(env.get(INTERNAL_LLM_ENV).map(String::as_str), Some("1"));
        for key in FACTORY_IDENTITY_ENV {
            assert_eq!(env.get(*key).map(String::as_str), Some(""), "{key}");
        }
    }
}
