//! Recorded pane identity shared by the catalog, status and native hooks.
//! Launch defaults seed the row; native observations win for the rest of its life.
use std::path::Path;

use cas_types::{Agent, AgentRole};
use serde::{Deserialize, Serialize};

pub(crate) fn metadata_prefix(role: AgentRole) -> Option<&'static str> {
    match role {
        AgentRole::Worker => Some("worker"),
        AgentRole::Supervisor | AgentRole::Director => Some("supervisor"),
        AgentRole::Standard => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentModel {
    pub id: String,
    pub name: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// Includes the model version, exactly as reported by the harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl AgentModel {
    pub(crate) fn from_agent(agent: &Agent) -> Self {
        let get = |suffix| {
            nonempty(
                agent
                    .metadata
                    .get(&format!("{}_{suffix}", metadata_prefix(agent.role)?))
                    .map(String::as_str),
            )
            .map(str::to_owned)
        };
        Self {
            id: agent.id.clone(),
            name: agent.name.clone(),
            role: agent.role.to_string(),
            harness: get("cli")
                .and_then(|cli| crate::harness_policy::parse_harness(&cli))
                .map(|cli| cli.backend().name().to_owned()),
            model: get("model"),
            effort: get("effort"),
        }
    }
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Record only the native session's durable row, including a rebound hook ID.
/// Missing observations cannot invent a model; a different model invalidates its
/// predecessor's effort until the harness reports the new effective level.
pub(crate) fn record(root: &Path, session_id: &str, model: Option<&str>, effort: Option<&str>) {
    let model = nonempty(model);
    let effort = nonempty(effort);
    if model.is_none() && effort.is_none() {
        return;
    }
    let Ok(store) = crate::store::open_agent_store(root) else {
        return;
    };
    let agent = store.get(session_id).ok().or_else(|| {
        store
            .list(None)
            .ok()?
            .into_iter()
            .filter(|agent| agent.cc_session_id.as_deref() == Some(session_id))
            .max_by_key(|agent| agent.registered_at)
    });
    let Some(mut agent) = agent else { return };
    let Some(prefix) = metadata_prefix(agent.role) else {
        return;
    };
    let model_key = format!("{prefix}_model");
    let effort_key = format!("{prefix}_effort");
    if let Some(model) = model {
        if agent.metadata.get(&model_key).map(String::as_str) != Some(model) {
            agent.metadata.remove(&effort_key);
            agent
                .metadata
                .insert(format!("{prefix}_effort_observed"), "true".into());
        }
        agent.metadata.insert(model_key, model.to_owned());
        agent
            .metadata
            .insert(format!("{prefix}_model_observed"), "true".into());
    }
    if let Some(effort) = effort {
        agent.metadata.insert(effort_key, effort.to_owned());
        agent
            .metadata
            .insert(format!("{prefix}_effort_observed"), "true".into());
    }
    if let Err(error) = store.update(&agent) {
        eprintln!("cas: could not record native model observation: {error}");
    }
}

pub(crate) fn record_hook(root: &Path, event: &str, input: &crate::hooks::HookInput) {
    let model = match event {
        "SessionStart" => input.model.as_deref(),
        "PostModelSwitch" => input.to_model.as_deref(),
        _ => None,
    };
    // PreModelSwitch is only an attempted selection. Other events' arbitrary
    // model fields are not current-model observations either.
    let observes_effort = matches!(
        event,
        "SessionStart" | "PostModelSwitch" | "PreToolUse" | "PostToolUse" | "Stop"
    );
    let native_effort = input
        .effort
        .as_ref()
        .and_then(|effort| effort.get("level"))
        .and_then(|level| level.as_str());
    let claude_effort = (observes_effort
        && crate::harness_policy::own_harness_from_env() == cas_mux::SupervisorCli::Claude)
        .then(|| std::env::var("CLAUDE_EFFORT").ok())
        .flatten();
    let effort = observes_effort
        .then_some(native_effort.or(claude_effort.as_deref()))
        .flatten();
    record(root, &input.session_id, model, effort);
}

#[cfg(test)]
mod tests {
    #[test]
    fn session_start_records_native_model_over_launch_default() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let project = env.home().join("native-model");
        let root = crate::store::init_cas_dir(&project).unwrap();
        env.set("CAS_ROOT", &root);
        env.set("CAS_AGENT_NAME", "native-lead");
        env.set("CAS_AGENT_ROLE", "supervisor");
        env.set("CAS_FACTORY_SUPERVISOR_CLI", "claude");
        env.set("CAS_FACTORY_SUPERVISOR_MODEL", "stale-launch");
        env.set("CAS_FACTORY_SUPERVISOR_EFFORT", "low");
        env.remove("CLAUDE_EFFORT");
        let input = serde_json::from_value(serde_json::json!({
            "session_id":"native-start", "hook_event_name":"SessionStart", "cwd":project,
            "model":"claude-opus-5-5", "effort":{"level":"high"}
        }))
        .unwrap();
        crate::hooks::handle_session_start(&input, Some(&root)).unwrap();
        let agent = crate::store::open_agent_store(&root)
            .unwrap()
            .get("native-start")
            .unwrap();
        let identity = super::AgentModel::from_agent(&agent);
        assert_eq!(identity.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(identity.effort.as_deref(), Some("high"));
    }
}
