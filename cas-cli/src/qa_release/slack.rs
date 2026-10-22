//! Posts a sealed pack to Slack through Violet from the `cas serve` driver
//! (cas-efb8), once per pack generation, when `qa.release.slack_channel` is
//! set. Violet only: the call goes through the serve process's MCP proxy and
//! its normal policy and audience checks.
//!
//! The driver is not a registered agent, so it calls as the fixed identity
//! [`DRIVER_AGENT`] with the `Standard` role (Violet routes are not
//! supervisor-only, and `Standard` is not subject to worker read-only access).
//! That caller is private to this module and hard-limited: it can call only
//! Violet's post tool, only to the configured channel, and every call is
//! audit-logged with the pack sha256 and channel.
use super::ReleaseConfig;
use super::media::{Ffmpeg, post_pack, slack_bundle};
use super::pack::{PACK_FILE, Pack, write_atomic};
use super::printable;
use crate::artifacts::slack::{PostError, PostOptions, VioletPoster};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};

/// The identity the driver's Violet calls carry and are audit-logged under.
pub const DRIVER_AGENT: &str = "release-gate-driver";
/// The only Violet tool the driver may call.
const POST_TOOL: &str = "violet_post";
/// Pack-dir record of the generations already posted.
pub const SLACK_FILE: &str = "slack.json";

type Reply = Pin<Box<dyn Future<Output = Result<Value, PostError>> + Send>>;

/// Where the driver's `violet_post` calls go: the serve process's proxy in
/// production, a fake in tests.
pub(crate) trait Transport: Send + Sync {
    fn post(&self, args: Value) -> Reply;
}

struct Installed {
    transport: Arc<dyn Transport>,
    runtime: tokio::runtime::Handle,
}

fn installed() -> &'static Mutex<Option<Installed>> {
    static INSTALLED: OnceLock<Mutex<Option<Installed>>> = OnceLock::new();
    INSTALLED.get_or_init(|| Mutex::new(None))
}

/// Installs the transport the driver posts through and the runtime its
/// futures run on (the seal runs on a blocking thread).
pub(crate) fn install(transport: Arc<dyn Transport>, runtime: tokio::runtime::Handle) {
    *installed().lock().unwrap_or_else(|e| e.into_inner()) = Some(Installed { transport, runtime });
}

/// Installs the serve process's proxy as the driver's transport.
#[cfg(feature = "mcp-proxy")]
pub(crate) fn install_proxy(engine: Arc<cmcp_core::ProxyEngine>) {
    struct Proxy(Arc<cmcp_core::ProxyEngine>);
    impl Transport for Proxy {
        fn post(&self, args: Value) -> Reply {
            let engine = Arc::clone(&self.0);
            Box::pin(async move {
                // Built here, for this one call: never a general caller.
                let caller = cmcp_core::ProxyCaller {
                    agent_id: DRIVER_AGENT.into(),
                    role: cas_types::AgentRole::Standard,
                    session_id: DRIVER_AGENT.into(),
                    factory_session: None,
                    active_task_ids: Vec::new(),
                };
                engine
                    .call_tool(&caller, cmcp_core::config::VIOLET_SERVER, POST_TOOL, args.as_object().cloned())
                    .await
                    .map_err(|error| {
                        PostError::new(
                            "violet_unavailable",
                            format!("the Violet call failed: {}", cmcp_core::describe_upstream_call_error(&error)),
                        )
                    })
            })
        }
    }
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        install(Arc::new(Proxy(engine)), runtime);
    }
}

/// The driver's Violet poster: only `violet_post`, only to `channel`, and
/// every call audit-logged.
struct DriverViolet {
    transport: Arc<dyn Transport>,
    channel: String,
    pack_sha256: String,
}

impl DriverViolet {
    fn call(&self, tool: &str, args: Value) -> Reply {
        if tool != POST_TOOL {
            let refusal = PostError::new("release_gate_tool_refused", format!("the release-gate driver may call only violet.{POST_TOOL}, not {}", printable(tool)));
            return Box::pin(async move { Err(refusal) });
        }
        if args.get("channel").and_then(Value::as_str) != Some(self.channel.as_str()) {
            let refusal = PostError::new("release_gate_channel_refused", "the release-gate driver posts only to qa.release.slack_channel");
            return Box::pin(async move { Err(refusal) });
        }
        let step = args.get("step").and_then(|s| s.as_str()).unwrap_or("").to_owned();
        tracing::info!(
            target: "cas::audit",
            agent_id = DRIVER_AGENT,
            pack_sha256 = %self.pack_sha256,
            channel = %self.channel,
            step = %step,
            "release-gate Violet post"
        );
        self.transport.post(args)
    }
}

impl VioletPoster for DriverViolet {
    fn violet_post(&self, args: Value) -> impl Future<Output = Result<Value, PostError>> + Send {
        self.call(POST_TOOL, args)
    }
}

/// One posted pack generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posted {
    pub pack_sha256: String,
    pub agent_id: String,
    pub channel: String,
    pub permalink: String,
    pub files: usize,
    pub posted_at: DateTime<Utc>,
}

/// The generations already posted from the pack in `dir`.
pub fn posted(dir: &Path) -> Vec<Posted> {
    std::fs::read(dir.join(SLACK_FILE)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// Posts the sealed pack in `dir` once per generation (keyed by the sha256 of
/// `pack.json`) when `qa.release.slack_channel` is set. Returns run notes:
/// the permalink, or why nothing was posted. A failed post is not recorded,
/// so the next generation's seal posts.
pub(crate) fn post_sealed(config: &ReleaseConfig, dir: &Path) -> Vec<String> {
    let channel = config.slack_channel.trim();
    if channel.is_empty() {
        return Vec::new();
    }
    let Ok(bytes) = std::fs::read(dir.join(PACK_FILE)) else {
        return vec!["Slack post skipped: pack.json is unreadable".into()];
    };
    let pack_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let mut ledger = posted(dir);
    if ledger.iter().any(|p| p.pack_sha256 == pack_sha256) {
        return Vec::new();
    }
    let Some((transport, runtime)) = installed()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|i| (Arc::clone(&i.transport), i.runtime.clone()))
    else {
        return vec!["Slack post skipped: no Violet proxy in this serve process".into()];
    };
    let outcome = Pack::load(dir).map_err(|e| e.to_string()).and_then(|pack| {
        let bundle = slack_bundle(dir, cas_types::ARTIFACT_MAX_BYTES, &Ffmpeg).map_err(|e| e.to_string())?;
        let word = serde_json::to_value(pack.gate.verdict).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
        let summary = format!(
            "Cassy release gate: {word} for `{}` ({} rows, {} failed, {} owed)",
            &pack.sha[..pack.sha.len().min(12)],
            pack.rows.len(),
            pack.gate.failed.len(),
            pack.gate.owed.len()
        );
        let violet = DriverViolet { transport, channel: channel.into(), pack_sha256: pack_sha256.clone() };
        let options = PostOptions { channel: channel.into(), ..PostOptions::default() };
        let receipts = runtime.block_on(post_pack(&violet, &bundle, &summary, &options)).map_err(|e| e.to_string())?;
        Ok((receipts, bundle.files.len()))
    });
    match outcome {
        Ok((receipts, files)) => {
            let permalink = receipts.first().map(|r| r.permalink.clone()).unwrap_or_default();
            ledger.push(Posted {
                pack_sha256: pack_sha256.clone(),
                agent_id: DRIVER_AGENT.into(),
                channel: channel.into(),
                permalink: permalink.clone(),
                files,
                posted_at: Utc::now(),
            });
            let recorded = serde_json::to_vec_pretty(&ledger)
                .map_err(|e| e.to_string())
                .and_then(|json| write_atomic(dir, SLACK_FILE, &json).map_err(|e| e.to_string()));
            let mut notes = vec![format!(
                "Slack: posted pack {} ({files} files) to {channel} as {DRIVER_AGENT}: {permalink}",
                &pack_sha256[..12]
            )];
            if let Err(error) = recorded {
                notes.push(format!("Slack post not recorded in {SLACK_FILE}: {}", printable(&error)));
            }
            notes
        }
        Err(error) => vec![format!("Slack post failed: {}", printable(&error))],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Recording(Mutex<Vec<Value>>);
    impl Transport for Recording {
        fn post(&self, args: Value) -> Reply {
            self.0.lock().unwrap().push(args);
            Box::pin(async { Ok(Value::Null) })
        }
    }

    /// The driver's caller reaches the transport only for violet_post to the
    /// configured channel; any other tool or channel is refused first.
    #[test]
    fn driver_violet_refuses_other_tools_and_channels() {
        let transport = Arc::new(Recording(Mutex::default()));
        let violet = DriverViolet { transport: transport.clone(), channel: "qa-release".into(), pack_sha256: "a".repeat(64) };
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let read = runtime.block_on(violet.call("violet_read", serde_json::json!({"channel": "qa-release"})));
        assert_eq!(read.unwrap_err().code, "release_gate_tool_refused");
        let elsewhere = runtime.block_on(violet.call(POST_TOOL, serde_json::json!({"channel": "general"})));
        assert_eq!(elsewhere.unwrap_err().code, "release_gate_channel_refused");
        assert!(transport.0.lock().unwrap().is_empty(), "refused calls never reach the proxy");
        runtime.block_on(violet.violet_post(serde_json::json!({"channel": "qa-release"}))).unwrap();
        assert_eq!(transport.0.lock().unwrap().len(), 1);
    }
}
