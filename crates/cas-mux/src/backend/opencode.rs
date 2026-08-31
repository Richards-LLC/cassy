use super::{
    Backend, SupervisorLaunchConfig, WorkerLaunchConfig, finish_supervisor_config,
    finish_worker_config,
};
use crate::Effort;
use crate::harness::HarnessCapabilities;
use crate::opencode::{
    OPENCODE_PLUGIN_FILE_NAME, OpenCodeProjectionSpec, OpenCodeRole, merge_opencode_projection,
    persist_opencode_plugin,
};
use crate::pty::PtyConfig;

pub(crate) static OPENCODE: OpenCode = OpenCode;

pub(crate) struct OpenCode;

impl Backend for OpenCode {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            // OpenCode 1.18.23 loaded the generated async plugin factory and
            // persisted root session, busy/tool, and idle events during the
            // hosted-token-plan live matrix (cas-a1c9).
            supports_hooks: true,
            // Keep subagents disabled: the gated matrix did not request or
            // attribute a child OpenCode agent.
            supports_subagents: false,
            // Ctrl+C cancellation needs output-quiescence polling before a
            // follow-up submit; the immediate flat-sleep path lost the turn.
            supports_textbox_submit: false,
            // A 1,532-byte raw single-write prompt plus CR submitted and
            // round-tripped in the exported 1.18.23 session. OpenCode does not
            // need Codex's explicit bracketed-paste framing.
            requires_bracketed_paste_injection: false,
            tool_prefix: "cas_",
        }
    }

    fn effort_arg(&self, effort: Effort) -> &'static str {
        // Preserve the caller's requested spelling. Model-aware validation is
        // owned by the spawn/policy layer; this adapter must not silently map
        // one effort to another.
        effort.as_str()
    }

    fn build_worker_config(&self, launch: WorkerLaunchConfig<'_>) -> PtyConfig {
        let projection = projection_inputs(
            launch.name,
            OpenCodeRole::Worker,
            &launch.cwd,
            launch.cas_root,
            launch.model,
            launch.effort,
        );
        let mut config = PtyConfig::opencode(
            launch.name,
            "worker",
            launch.cwd,
            launch.cas_root,
            Some(launch.supervisor_name),
            None,
            launch.model,
            launch.effort,
            launch.teams,
        );
        apply_live_projection(&mut config, projection);
        finish_worker_config(
            &mut config,
            launch.supervisor_cli,
            launch.active_workers,
            None,
        );
        config
    }

    fn build_supervisor_config(&self, launch: SupervisorLaunchConfig<'_>) -> PtyConfig {
        let projection = projection_inputs(
            launch.name,
            OpenCodeRole::Supervisor,
            &launch.cwd,
            launch.cas_root,
            launch.model,
            launch.effort,
        );
        let mut config = PtyConfig::opencode(
            launch.name,
            "supervisor",
            launch.cwd,
            launch.cas_root,
            None,
            Some(launch.worker_cli.backend().name()),
            launch.model,
            launch.effort,
            launch.teams,
        );
        apply_live_projection(&mut config, projection);
        finish_supervisor_config(&mut config, self.name(), launch.worker_names);
        config
    }

    fn turn_cancel_bytes(&self) -> &'static [u8] {
        // Live 1.18.23 PTY evidence: Esc did not interrupt `bash sleep 20`
        // and corrupted the first byte of the queued follow-up; Ctrl+C
        // aborted the tool and retained the TUI process.
        &[0x03]
    }
}

fn projection_inputs(
    name: &str,
    role: OpenCodeRole,
    cwd: &std::path::Path,
    cas_root: Option<&std::path::PathBuf>,
    model: Option<&str>,
    effort: Option<&str>,
) -> (OpenCodeProjectionSpec, std::path::PathBuf) {
    let cas_root = cas_root.cloned().unwrap_or_else(|| cwd.join(".cas"));
    let plugin_path = cas_root.join("opencode").join(OPENCODE_PLUGIN_FILE_NAME);
    let mut spec = OpenCodeProjectionSpec::new(
        role,
        name,
        "pending-pty-session-id",
        &cas_root,
        cwd,
        &plugin_path,
    );
    if let Some(model) = model {
        spec = spec.with_model(model);
    }
    if let Some(effort) = effort {
        spec = spec.with_variant(effort);
    }
    (spec, plugin_path)
}

fn apply_live_projection(
    config: &mut PtyConfig,
    (mut spec, plugin_path): (OpenCodeProjectionSpec, std::path::PathBuf),
) {
    spec.cas_session_id = config
        .env
        .iter()
        .find_map(|(key, value)| (key == "CAS_SESSION_ID").then(|| value.clone()))
        .expect("OpenCode PTY config must carry CAS_SESSION_ID");
    persist_opencode_plugin(&plugin_path).unwrap_or_else(|error| {
        panic!(
            "OpenCode launch could not persist lifecycle plugin at {}: {error}",
            plugin_path.display()
        )
    });
    let inline = config
        .env
        .iter_mut()
        .find(|(key, _)| key == "OPENCODE_CONFIG_CONTENT")
        .expect("OpenCode PTY config must carry OPENCODE_CONFIG_CONTENT");
    inline.1 = merge_opencode_projection(&inline.1, &spec)
        .expect("OpenCode PTY and lifecycle projections must be valid JSON");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_launch_persists_plugin_and_merges_hosted_provider_projection() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        let temp = std::env::temp_dir().join(format!(
            "cas-mux-opencode-launch-{}-{nonce}",
            std::process::id()
        ));
        let cwd = temp.join("repo");
        let cas_root = cwd.join(".cas");
        std::fs::create_dir_all(&cwd).expect("temporary repository");

        let config = OpenCode.build_worker_config(WorkerLaunchConfig {
            name: "opencode-test",
            cwd: cwd.clone(),
            cas_root: Some(&cas_root),
            supervisor_name: "supervisor",
            supervisor_cli: crate::harness::SupervisorCli::Codex,
            model: Some("qwencloud/qwen3.8-max"),
            effort: Some("xhigh"),
            config_dir: None,
            config_dir_source: None,
            secure_storage_dir: None,
            teams: None,
            active_workers: None,
        });

        let inline = config
            .env
            .iter()
            .find_map(|(key, value)| (key == "OPENCODE_CONFIG_CONTENT").then_some(value))
            .expect("inline OpenCode config");
        let inline: serde_json::Value = serde_json::from_str(inline).expect("valid config JSON");
        assert_eq!(
            inline["provider"]["qwencloud"]["options"]["baseURL"],
            "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1"
        );
        assert_eq!(inline["agent"]["cassy-worker"]["variant"], "xhigh");
        assert!(inline["agent"]["cassy-supervisor"].is_object());

        let plugin_path = cas_root.join("opencode").join(OPENCODE_PLUGIN_FILE_NAME);
        let plugin = std::fs::read_to_string(&plugin_path).expect("persisted lifecycle plugin");
        assert!(plugin.contains("export const CassyPlugin = async"));
        assert_eq!(inline["plugin"][0], plugin_path.to_string_lossy().as_ref());

        std::fs::remove_dir_all(&temp).expect("remove temporary OpenCode launch root");
    }
}
