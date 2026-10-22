//! Posts a sealed pack to Slack through Violet from the `cas serve` driver
//! (cas-efb8), once per pack generation, when `qa.release.slack_channel` is
//! set. Violet only: the call goes through the serve process's MCP proxy and
//! its normal policy and audience checks. A seal (in the driver or in the CLI
//! `record` path, which has no proxy) only notes the post; each driver tick
//! posts every sealed generation missing from `slack.json` and retries a
//! failure on the next tick.
//!
//! The driver is not a registered agent, so it calls as the fixed identity
//! [`DRIVER_AGENT`] with the `Standard` role (Violet routes are not
//! supervisor-only, and `Standard` is not subject to worker read-only access).
//! That caller is private to this module and hard-limited: it can call only
//! Violet's post tool, only to the configured channel, and every call is
//! audit-logged with the pack sha256 and channel.
use super::ReleaseConfig;
use super::media::{Ffmpeg, post_pack_each, slack_bundle};
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
                        let described = cmcp_core::describe_upstream_call_error(&error);
                        // A policy denial is permanent; anything else may pass on retry.
                        let code = if described.contains("denied by policy") { "violet_refused" } else { "violet_unavailable" };
                        PostError::new(code, format!("the Violet call failed: {described}"))
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
    /// The top-level message posted but a later file failed. Still final:
    /// retrying would post the top-level message again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    /// Bundle files a partial post never uploaded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
}

/// One pack's unfinished posting: its persisted backoff, and whether it
/// stopped for good or was interrupted mid-post.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub pack_sha256: String,
    pub attempts: u32,
    pub next_at: DateTime<Utc>,
    #[serde(default)]
    pub last_error: String,
    /// A permanent refusal, or an interrupted post: never retried.
    #[serde(default)]
    pub stopped: bool,
    /// Recorded before each upload; still set on the next tick means the
    /// process died mid-post, which may have reached Slack.
    #[serde(default)]
    pub in_flight: bool,
}

/// `slack.json`: posted generations and unfinished attempts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub posted: Vec<Posted>,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
}

/// The generations already posted from the pack in `dir`.
pub fn posted(dir: &Path) -> Vec<Posted> {
    ledger(dir).0.posted
}

/// The record, moving a `slack.json` that does not parse aside (as
/// `slack.json.corrupt-<time>`) rather than overwriting it.
fn ledger(dir: &Path) -> (Ledger, Option<String>) {
    let path = dir.join(SLACK_FILE);
    let Ok(bytes) = std::fs::read(&path) else {
        return (Ledger::default(), None);
    };
    match serde_json::from_slice(&bytes) {
        Ok(ledger) => (ledger, None),
        Err(error) => {
            let aside = dir.join(format!("{SLACK_FILE}.corrupt-{}", Utc::now().format("%Y%m%dT%H%M%S%.f")));
            let note = match std::fs::rename(&path, &aside) {
                Ok(()) => format!("Slack: {SLACK_FILE} did not parse ({error}); moved aside to {}", aside.display()),
                Err(rename) => format!("Slack: {SLACK_FILE} did not parse ({error}) and could not be moved aside: {rename}"),
            };
            (Ledger::default(), Some(note))
        }
    }
}

fn save(dir: &Path, ledger: &Ledger) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(ledger).map_err(|e| e.to_string())?;
    write_atomic(dir, SLACK_FILE, &json).map_err(|e| e.to_string())
}

/// A note for a seal: the driver posts this generation on its next tick.
pub(crate) fn seal_note(config: &ReleaseConfig) -> Option<String> {
    (!config.slack_channel.trim().is_empty())
        .then(|| format!("Slack: the cas serve driver posts this generation to {}", config.slack_channel.trim()))
}

/// Each (pack, outcome) note once per process, so a post that waits or keeps
/// failing is noted once, not every tick.
fn once(key: &str, note: String) -> Option<String> {
    static NOTED: OnceLock<Mutex<std::collections::BTreeSet<(String, String)>>> = OnceLock::new();
    let noted = NOTED.get_or_init(Default::default);
    noted.lock().unwrap_or_else(|e| e.into_inner()).insert((key.into(), note.clone())).then_some(note)
}

/// Packs posted whose `slack.json` write then failed: never reposted by this
/// process.
fn posted_unrecorded() -> &'static Mutex<std::collections::BTreeSet<String>> {
    static POSTED: OnceLock<Mutex<std::collections::BTreeSet<String>>> = OnceLock::new();
    POSTED.get_or_init(Default::default)
}

/// Delay before attempt `n + 1` after `n` attempts: 30 s doubling, 30 min cap.
pub fn backoff(attempts: u32) -> chrono::Duration {
    let seconds = 30i64.saturating_mul(1i64 << attempts.saturating_sub(1).min(16));
    chrono::Duration::seconds(seconds.min(30 * 60))
}

/// Only throttling and unavailability retry; any other refusal (policy,
/// audience, channel, a changed pack) stops so the channel is never spammed.
fn transient(code: &str) -> bool {
    matches!(code, "violet_unavailable" | "violet_upload_failed")
        || ["rate_limit", "ratelimit", "timeout", "timed_out", "unavailable"].iter().any(|t| code.contains(t))
}

/// `<cas_root>/release-gate/slack-channel.json`: when the driver first saw
/// the current `slack_channel`. Packs sealed before it (and over 24 hours
/// old) are never back-posted.
pub const CHANNEL_FILE: &str = "slack-channel.json";

#[derive(Debug, Serialize, Deserialize)]
struct ChannelSince {
    channel: String,
    since: DateTime<Utc>,
}

/// When posting to `channel` began, recording `now` the first time.
pub(crate) fn channel_since(cas_root: &Path, channel: &str, now: DateTime<Utc>) -> DateTime<Utc> {
    let dir = cas_root.join("release-gate");
    let path = dir.join(CHANNEL_FILE);
    if let Some(known) = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ChannelSince>(&bytes).ok())
        .filter(|known| known.channel == channel)
    {
        return known.since;
    }
    let record = ChannelSince { channel: channel.into(), since: now };
    if let Ok(json) = serde_json::to_vec_pretty(&record) {
        let _ = std::fs::create_dir_all(&dir).map(|()| write_atomic(&dir, CHANNEL_FILE, &json));
    }
    now
}

/// Posts the sealed pack in `dir` (keyed by the sha256 of `pack.json`) when
/// `qa.release.slack_channel` is set; the driver calls it every tick for every
/// sealed run with the clock `now`. A pack is posted at most once:
/// - only if sealed at or after `since` (when posting to this channel began)
///   or in the 24 hours before `now`, so setting the channel back-posts nothing old;
/// - each attempt is recorded in `slack.json` before the upload, and if that
///   write fails nothing is posted; an attempt still in flight on a later tick
///   (the process died mid-post) is never retried;
/// - throttling and unavailability retry with a persisted 30 s doubling
///   backoff capped at 30 min; any other refusal stops;
/// - a post whose record then fails to save is never reposted by this process.
/// - once the top-level message is posted the generation is final: a later
///   file failing records a `partial` post (permalink, missing files), never retried.
///
/// Returns run notes: the permalink, or (once per pack and cause) why not.
pub(crate) fn post_sealed(config: &ReleaseConfig, dir: &Path, now: DateTime<Utc>, since: DateTime<Utc>) -> Vec<String> {
    let channel = config.slack_channel.trim();
    if channel.is_empty() {
        return Vec::new();
    }
    let key = dir.to_string_lossy().into_owned();
    let Ok(bytes) = std::fs::read(dir.join(PACK_FILE)) else {
        return once(&key, "Slack post skipped: pack.json is unreadable".into()).into_iter().collect();
    };
    let pack_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let (mut ledger, moved) = ledger(dir);
    let mut notes: Vec<String> = moved.into_iter().collect();
    let unrecorded = posted_unrecorded().lock().unwrap_or_else(|e| e.into_inner()).contains(&pack_sha256);
    if unrecorded || ledger.posted.iter().any(|p| p.pack_sha256 == pack_sha256) {
        return notes;
    }
    #[derive(Deserialize)]
    struct Sealed {
        created_at: DateTime<Utc>,
    }
    let sealed_at = serde_json::from_slice::<Sealed>(&bytes).map(|s| s.created_at).unwrap_or(now);
    if sealed_at < since && sealed_at < now - chrono::Duration::hours(24) {
        notes.extend(once(&pack_sha256, "Slack: not posted; sealed before slack_channel was set and over 24 hours ago".into()));
        return notes;
    }
    let mut attempt = ledger
        .attempts
        .iter()
        .find(|a| a.pack_sha256 == pack_sha256)
        .cloned()
        .unwrap_or(Attempt { pack_sha256: pack_sha256.clone(), attempts: 0, next_at: now, last_error: String::new(), stopped: false, in_flight: false });
    let record = |ledger: &mut Ledger, attempt: &Attempt| {
        ledger.attempts.retain(|a| a.pack_sha256 != attempt.pack_sha256);
        ledger.attempts.push(attempt.clone());
    };
    if attempt.in_flight && !attempt.stopped {
        attempt.stopped = true;
        attempt.in_flight = false;
        attempt.last_error = "an earlier post was interrupted and may have reached Slack".into();
        record(&mut ledger, &attempt);
        let _ = save(dir, &ledger);
    }
    if attempt.stopped {
        notes.extend(once(&pack_sha256, format!("Slack post stopped, not retrying: {}", printable(&attempt.last_error))));
        return notes;
    }
    if now < attempt.next_at {
        return notes;
    }
    let Some((transport, runtime)) = installed()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|i| (Arc::clone(&i.transport), i.runtime.clone()))
    else {
        notes.extend(once(&pack_sha256, "Slack post waiting: no Violet proxy in this serve process".into()));
        return notes;
    };
    attempt.attempts += 1;
    attempt.next_at = now + backoff(attempt.attempts);
    attempt.in_flight = true;
    record(&mut ledger, &attempt);
    if let Err(error) = save(dir, &ledger) {
        notes.extend(once(&pack_sha256, format!("Slack post not attempted: {SLACK_FILE} cannot record it ({})", printable(&error))));
        return notes;
    }
    let outcome = Pack::load(dir)
        .map_err(|e| PostError::new("pack_invalid", e.to_string()))
        .and_then(|pack| {
            let bundle = slack_bundle(dir, cas_types::ARTIFACT_MAX_BYTES, &Ffmpeg).map_err(|e| PostError::new("pack_invalid", e.to_string()))?;
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
            Ok((runtime.block_on(post_pack_each(&violet, &bundle, &summary, &options)), bundle))
        });
    let (posted, bundle) = match outcome {
        Ok(done) => done,
        Err(error) => (super::media::PackPost { receipts: Vec::new(), failed: Some((PACK_FILE.into(), error)) }, Default::default()),
    };
    // Before the top-level message: nothing reached Slack, so retry or stop.
    if posted.receipts.is_empty() {
        let error = posted.failed.map(|(_, error)| error).unwrap_or_else(|| PostError::new("pack_empty", "the bundle has no files"));
        attempt.in_flight = false;
        attempt.last_error = format!("{}: {}", error.code, error.message);
        attempt.stopped = !transient(&error.code);
        record(&mut ledger, &attempt);
        // If this write fails, the in-flight attempt stops the pack next tick.
        let _ = save(dir, &ledger);
        let next = if attempt.stopped { "not retrying" } else { "retrying with backoff" };
        notes.extend(once(&pack_sha256, format!("Slack post failed ({next}): {}", printable(&attempt.last_error))));
        return notes;
    }
    // The top-level message exists: the generation is final, complete or not.
    let permalink = posted.receipts[0].permalink.clone();
    let files = posted.receipts.len();
    let missing: Vec<String> = bundle.files.iter().skip(files).map(|f| f.source.clone()).collect();
    ledger.attempts.retain(|a| a.pack_sha256 != pack_sha256);
    ledger.posted.push(Posted {
        pack_sha256: pack_sha256.clone(),
        agent_id: DRIVER_AGENT.into(),
        channel: channel.into(),
        permalink: permalink.clone(),
        files,
        posted_at: now,
        partial: posted.failed.is_some(),
        missing: missing.clone(),
    });
    match &posted.failed {
        None => notes.push(format!(
            "Slack: posted pack {} ({files} files) to {channel} as {DRIVER_AGENT}: {permalink}",
            &pack_sha256[..12]
        )),
        Some((_, error)) => notes.extend(once(
            &pack_sha256,
            format!(
                "Slack: partial post of pack {} to {channel} ({permalink}); not retried, missing {}: {}: {}",
                &pack_sha256[..12],
                missing.join(", "),
                error.code,
                printable(&error.message)
            ),
        )),
    }
    if let Err(error) = save(dir, &ledger) {
        posted_unrecorded().lock().unwrap_or_else(|e| e.into_inner()).insert(pack_sha256);
        notes.push(format!("Slack post not recorded in {SLACK_FILE} ({}); this process will not repost it", printable(&error)));
    }
    notes
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
