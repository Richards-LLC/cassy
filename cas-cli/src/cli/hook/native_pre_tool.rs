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
        return HookOutput::with_pre_tool_permission("deny", UNAVAILABLE);
    }
    let Ok(Ok(output)) = receiver.recv_timeout(budget) else {
        return HookOutput::with_pre_tool_permission("deny", UNAVAILABLE);
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
        HookOutput::with_pre_tool_permission("deny", UNAVAILABLE)
    }
}

pub fn execute_native_pre_tool_use() -> anyhow::Result<()> {
    let mut text = String::new();
    io::stdin().read_to_string(&mut text)?;
    let mut input: HookInput = match serde_json::from_str(&text) {
        Ok(input) => input,
        Err(_) => {
            println!(
                "{}",
                serde_json::to_string(&HookOutput::with_pre_tool_permission(
                    "deny",
                    "CASSY GUARD UNAVAILABLE: invalid hook input; retry after correcting the hook configuration."
                ))?
            );
            return Ok(());
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
        handle_hook("PreToolUse", input)?
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}
