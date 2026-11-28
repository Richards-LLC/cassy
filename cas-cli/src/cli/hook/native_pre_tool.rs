//! Native Codex hook entry before CLI telemetry/store initialization.
use crate::hooks::{HookInput, HookOutput, handle_hook};
use std::io::{self, Read};
use std::time::Duration;

const VALIDATION_BUDGET: Duration = Duration::from_secs(2);
const UNAVAILABLE: &str = "CASSY CARGO GUARD UNAVAILABLE: policy validation did not finish successfully within its budget; retry the capped Cargo command when the host is responsive.";

pub fn is_native_codex_pre_tool(args: &[String]) -> bool {
    args.len() == 3
        && args[1] == "hook"
        && args[2] == "PreToolUse"
        && std::env::var("CAS_HOOK_HARNESS").as_deref() == Ok("codex")
}

fn bounded_cargo_validation<F>(budget: Duration, validate: F) -> HookOutput
where
    F: FnOnce() -> Result<HookOutput, crate::error::MemError> + Send + 'static,
{
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    if std::thread::Builder::new()
        .spawn(move || {
            let _ = sender.send(validate());
        })
        .is_err()
    {
        return HookOutput::with_pre_tool_permission("deny", &crate::gates::tag("H-02", UNAVAILABLE));
    }
    let Ok(Ok(output)) = receiver.recv_timeout(budget) else {
        return HookOutput::with_pre_tool_permission("deny", &crate::gates::tag("H-02", UNAVAILABLE));
    };
    // A successful callback must carry the rewrite Codex actually applies;
    // an empty/invalid output would otherwise execute the original Cargo.
    let value = serde_json::to_value(&output).unwrap_or_default();
    let specific = &value["hookSpecificOutput"];
    if specific["permissionDecision"] == "deny"
        || (specific["permissionDecision"] == "allow"
            && specific["updatedInput"]["command"]
                .as_str()
                .is_some_and(|command| command.contains(" factory worker-check --cas-root ")))
    {
        output
    } else {
        HookOutput::with_pre_tool_permission("deny", &crate::gates::tag("H-02", UNAVAILABLE))
    }
}

// gate[H-01 pre-tool]: Codex native entry fail-closed
fn record_unavailable(input: &HookInput, message: &str) {
    if let Ok(root) = crate::store::find_cas_root() {
        crate::hooks::handlers::handlers_events::record_demoted(Some(input), &root, &crate::gates::tag("H-01", message));
    }
}

pub fn execute_native_pre_tool_use() -> anyhow::Result<()> {
    let mut text = String::new();
    let input = io::stdin().read_to_string(&mut text).map(|_| text);
    let output = super::process_hook_input("PreToolUse", input, true)?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

pub(super) fn process_native_input(text: &str) -> anyhow::Result<HookOutput> {
    let mut input: HookInput = match serde_json::from_str(&text) {
        Ok(input) => input,
        Err(_) => {
            return Ok(HookOutput::with_pre_tool_permission(
                "deny",
                &crate::gates::tag("H-01", "CASSY GUARD UNAVAILABLE: invalid hook input; retry after correcting the hook configuration.")
            ));
        }
    };
    if input.agent_role.is_none() {
        input.agent_role = std::env::var("CAS_AGENT_ROLE").ok();
    }
    let cargo = crate::hooks::handlers::handlers_events::worker_cargo_request(&input);
    let _ = rustls::crypto::ring::default_provider().install_default();
    let output = if cargo {
        bounded_cargo_validation(VALIDATION_BUDGET, move || handle_hook("PreToolUse", input))
    } else {
        super::init_hook_logging(false);
        match handle_hook("PreToolUse", input.clone()) {
            Ok(output) => output,
            Err(_) => {
                record_unavailable(&input, "CASSY GUARD UNAVAILABLE: hook processing failed.");
                crate::hooks::handlers::handlers_events::pre_tool_without_stores(&input)
            }
        }
    };
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use cas_store::EventStore;
    use cas_types::EventType;

    #[test]
    fn native_entry_runs_the_store_free_guards_and_stays_closed_on_unreadable_input() {
        let mut env = TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        env.set("CAS_ROOT", &cas_root);
        env.set("CAS_HOOK_HARNESS", "codex");
        env.set("CAS_AGENT_NAME", "native-fixture");

        let truncated = format!(
            r#"{{"session_id":"019a6c1e-7d2b-7c90-8c1e-0f3a5b7d9e21","turn_id":"turn-1","cwd":"{}","hook_event_name":"PreToolUse","model":"gpt-5","permission_mode":"default","tool_name":"Bash","tool_input":{{"command":"ls"#,
            project.path().display()
        );
        let output = process_native_input(&truncated).unwrap();
        let reason = crate::gates::facts::hook_denial(&output).expect("an unreadable payload cannot be checked, so it stays closed");
        assert!(crate::gates::codes_in(reason).contains(&"H-01"), "{reason}");

        let call = |tool: &str, tool_input: serde_json::Value| -> HookInput {
            serde_json::from_value(serde_json::json!({
                "session_id": "019a6c1e-7d2b-7c90-8c1e-0f3a5b7d9e21", "turn_id": "turn-1", "cwd": project.path(),
                "hook_event_name": "PreToolUse", "model": "gpt-5", "permission_mode": "default",
                "tool_name": tool, "tool_input": tool_input, "tool_use_id": "call_FixtureNative",
            })).unwrap()
        };
        let secret = crate::hooks::handlers::handlers_events::pre_tool_without_stores(
            &call("Read", serde_json::json!({"file_path": project.path().join(".env")})),
        );
        let reason = crate::gates::facts::hook_denial(&secret).expect("a secret path stays closed");
        assert!(crate::gates::codes_in(reason).contains(&"H-33"), "{reason}");
        let ordinary = crate::hooks::handlers::handlers_events::pre_tool_without_stores(
            &call("Bash", serde_json::json!({"command": "ls"})),
        );
        assert!(crate::gates::facts::hook_denial(&ordinary).is_none());

        let input = call("Bash", serde_json::json!({"command": "ls"}));
        record_unavailable(&input, "CASSY GUARD UNAVAILABLE: hook processing failed.");
        let facts = crate::store::open_event_store(&cas_root).unwrap().list_by_type(EventType::GateRefused, 10).unwrap();
        assert_eq!(facts.len(), 1, "{facts:?}");
        let metadata = facts[0].metadata.as_ref().unwrap();
        assert_eq!((facts[0].entity_id.as_str(), metadata["outcome"].as_str(), metadata["harness"].as_str(), metadata["actor"].as_str()),
            ("H-01", Some("allowed"), Some("codex"), Some("native-fixture")));
    }
}
