//! OpenCode role/config and session-activity projection.
//!
//! OpenCode has a different extension boundary from Claude Code: role
//! instructions and MCP wiring are supplied as inline config, while session
//! lifecycle and tool activity arrive through an asynchronous plugin event
//! stream.  This module keeps that translation deterministic and keeps the
//! Cassy↔OpenCode identity mapping independent from OpenCode's shared SQLite
//! database.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const OPENCODE_PLUGIN_FILE_NAME: &str = "cassy-opencode-plugin.mjs";
pub const OPENCODE_STATE_DIRECTORY: &str = "opencode/sessions";
pub const OPENCODE_STATE_SCHEMA_VERSION: u32 = 1;

/// The two generated OpenCode primary agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenCodeRole {
    Worker,
    Supervisor,
}

impl OpenCodeRole {
    pub const fn agent_name(self) -> &'static str {
        match self {
            Self::Worker => "cassy-worker",
            Self::Supervisor => "cassy-supervisor",
        }
    }

    pub const fn role_name(self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::Supervisor => "supervisor",
        }
    }
}

/// Inputs shared by the T1 launch adapter and the deterministic projection.
///
/// `model` and `variant` are kept as complete strings.  The OpenCode adapter
/// must not collapse a provider/model selector or invent a reasoning-effort
/// mapping before the model-aware policy layer has validated it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeProjectionSpec {
    pub role: OpenCodeRole,
    pub name: String,
    pub model: Option<String>,
    pub variant: Option<String>,
    pub cas_session_id: String,
    pub cas_root: PathBuf,
    pub directory: PathBuf,
    pub plugin_path: PathBuf,
}

impl OpenCodeProjectionSpec {
    pub fn new(
        role: OpenCodeRole,
        name: impl Into<String>,
        cas_session_id: impl Into<String>,
        cas_root: impl Into<PathBuf>,
        directory: impl Into<PathBuf>,
        plugin_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            role,
            name: name.into(),
            model: None,
            variant: None,
            cas_session_id: cas_session_id.into(),
            cas_root: cas_root.into(),
            directory: directory.into(),
            plugin_path: plugin_path.into(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_variant(mut self, variant: impl Into<String>) -> Self {
        self.variant = Some(variant.into());
        self
    }

    /// Adapt the shared factory spec without importing OpenCode into the
    /// selector enum.  The T1 adapter supplies the OpenCode selector and
    /// model-aware validation; this helper keeps the projection itself
    /// reusable while preserving the complete model and effort spellings.
    pub fn from_worker_spec(
        spec: &crate::WorkerSpec,
        role: OpenCodeRole,
        cas_session_id: impl Into<String>,
        cas_root: impl Into<PathBuf>,
        directory: impl Into<PathBuf>,
        plugin_path: impl Into<PathBuf>,
    ) -> Self {
        let name = spec
            .name
            .clone()
            .unwrap_or_else(|| role.agent_name().to_string());
        let mut projection =
            Self::new(role, name, cas_session_id, cas_root, directory, plugin_path);
        projection.model = spec.model.clone();
        projection.variant = spec.effort.map(|effort| effort.as_str().to_string());
        projection
    }
}

/// Render the complete process-local `OPENCODE_CONFIG_CONTENT` payload.
///
/// Both role agents are emitted on every launch so a supervisor can inspect
/// the same deterministic projection as a worker.  Only the selected agent
/// is passed through `--agent` by the launch adapter.
pub fn render_opencode_config(spec: &OpenCodeProjectionSpec) -> String {
    let mut agents = BTreeMap::new();
    agents.insert(
        OpenCodeRole::Supervisor.agent_name().to_string(),
        render_agent(OpenCodeRole::Supervisor, spec),
    );
    agents.insert(
        OpenCodeRole::Worker.agent_name().to_string(),
        render_agent(OpenCodeRole::Worker, spec),
    );

    let mut mcp = BTreeMap::new();
    mcp.insert(
        "cas".to_string(),
        json_object([
            ("type", Value::String("local".to_string())),
            (
                "command",
                Value::Array(vec![
                    Value::String("cas".to_string()),
                    Value::String("serve".to_string()),
                ]),
            ),
            ("enabled", Value::Bool(true)),
        ]),
    );

    let mut root = BTreeMap::new();
    root.insert(
        "agent".to_string(),
        serde_json::to_value(agents).expect("agent config is serializable"),
    );
    root.insert(
        "mcp".to_string(),
        serde_json::to_value(mcp).expect("MCP config is serializable"),
    );
    root.insert(
        "plugin".to_string(),
        Value::Array(vec![Value::String(
            spec.plugin_path.to_string_lossy().into_owned(),
        )]),
    );
    let config = serde_json::to_string_pretty(&root).expect("OpenCode config is serializable");
    format!("{config}\n")
}

/// Overlay the live role/plugin projection onto the PTY adapter's inline
/// configuration without dropping route-specific provider settings.
///
/// `PtyConfig::opencode` owns the hosted endpoint and environment-key
/// substitution because the leaf PTY crate cannot depend on this crate. The
/// backend owns the generated role prompts and lifecycle plugin. Merging the
/// two at the backend seam keeps both contracts active in the actual launch
/// configuration instead of only in parity snapshots.
pub fn merge_opencode_projection(
    base: &str,
    spec: &OpenCodeProjectionSpec,
) -> Result<String, serde_json::Error> {
    let mut base: Value = serde_json::from_str(base)?;
    let projected: Value = serde_json::from_str(&render_opencode_config(spec))?;
    for key in ["agent", "mcp", "plugin"] {
        base[key] = projected[key].clone();
    }
    serde_json::to_string(&base)
}

fn render_agent(role: OpenCodeRole, spec: &OpenCodeProjectionSpec) -> Value {
    let mut agent = BTreeMap::new();
    let role_name = if role == spec.role {
        spec.name.as_str()
    } else {
        role.agent_name()
    };
    agent.insert("mode".to_string(), Value::String("primary".to_string()));
    agent.insert(
        "prompt".to_string(),
        Value::String(role_prompt(role, role_name)),
    );
    if let Some(model) = &spec.model {
        agent.insert("model".to_string(), Value::String(model.clone()));
    }
    if let Some(variant) = &spec.variant {
        agent.insert("variant".to_string(), Value::String(variant.clone()));
    }

    let mut permission = BTreeMap::new();
    for (tool, decision) in [
        ("bash", "allow"),
        ("edit", "allow"),
        ("external_directory", "deny"),
        ("glob", "allow"),
        ("grep", "allow"),
        ("question", "deny"),
        ("read", "allow"),
    ] {
        permission.insert(tool.to_string(), Value::String(decision.to_string()));
    }
    agent.insert(
        "permission".to_string(),
        serde_json::to_value(permission).expect("permission config is serializable"),
    );
    serde_json::to_value(agent).expect("agent config is serializable")
}

fn role_prompt(role: OpenCodeRole, name: &str) -> String {
    // Reuse the canonical role contract from the leaf PTY crate so the
    // generated primary-agent prompt cannot drift from Claude/Codex/Grok.
    // OpenCode's MCP server name is `cas`, which the runtime sanitizes to
    // `cas_<tool>`; normalize only that intentional namespace difference.
    let contract = match role {
        OpenCodeRole::Worker => crate::claude_worker_contract(name),
        OpenCodeRole::Supervisor => crate::claude_supervisor_contract("worker-a, worker-b"),
    };
    let contract = contract.replace("mcp__cas__", "cas_");
    format!(
        "{contract} The OpenCode plugin records session identity, awaited tool attribution, and lifecycle signals; permission.ask is not a substitute for Cassy's pre-tool policy."
    )
}

fn json_object(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut object = Map::new();
    for (key, value) in entries {
        object.insert(key.to_string(), value);
    }
    Value::Object(object)
}

/// The generated plugin source installed through the inline OpenCode config.
///
/// The generic `event` callback is fire-and-forget upstream.  Every handler
/// therefore catches delayed-write failures, and the state file is replaced
/// atomically so a supervisor can safely fall back to process evidence.
pub const OPENCODE_PLUGIN_SOURCE: &str = r#"import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { join } from "node:path";

const CAS_SESSION_ID = process.env.CAS_SESSION_ID || "";
const CAS_ROOT = process.env.CAS_ROOT || join(process.env.PWD || process.cwd(), ".cas");
const DIRECTORY = process.env.PWD || process.cwd();
const STATE_DIR = join(CAS_ROOT, "opencode", "sessions");
const STATE_PATH = join(STATE_DIR, `${CAS_SESSION_ID}.json`);
const SCHEMA_VERSION = 1;

// OpenCode invokes the generic event callback without awaiting the returned
// promise.  Serialize all state writes in this process so duplicate and
// delayed callbacks cannot race a read/modify/rename cycle.
let serializedUpdate = Promise.resolve();

// permission.ask is not a substitute for Cassy's pre-tool policy.  It only
// observes requests that reached OpenCode's ask state; policy remains owned by
// Cassy's existing dispatch boundary.

const now = () => Date.now();
const sessionId = (value) => typeof value === "string" && value.startsWith("ses_") ? value : null;
const eventSession = (event) => sessionId(event?.properties?.info?.id || event?.properties?.sessionID || event?.sessionID);
const rootSession = (event) => !event?.properties?.info?.parentID && !event?.properties?.info?.parent_id;

async function readState() {
  try { return JSON.parse(await readFile(STATE_PATH, "utf8")); } catch { return null; }
}

async function replaceState(state) {
  await mkdir(STATE_DIR, { recursive: true });
  const temp = `${STATE_PATH}.tmp-${process.pid}-${now()}`;
  await writeFile(temp, `${JSON.stringify(state, null, 2)}\n`, { mode: 0o600 });
  await rename(temp, STATE_PATH);
}

async function update(mutator) {
  const operation = serializedUpdate.then(async () => {
    if (!CAS_SESSION_ID) return;
    const current = await readState();
    const state = current || { schema_version: SCHEMA_VERSION, cas_session_id: CAS_SESSION_ID, directory: DIRECTORY, opencode_session_id: null, liveness: "unknown", last_event_at: 0, last_activity_at: null, active_tool: null, last_tool: null };
    const next = await mutator(state);
    if (next) await replaceState(next);
  });
  serializedUpdate = operation.catch(() => {});
  return operation;
}

async function applySessionEvent(event) {
  const type = event?.type;
  const observed = eventSession(event);
  if (type === "session.created") {
    if (!rootSession(event) || !observed) return;
    await update((state) => {
      if (state.opencode_session_id && state.opencode_session_id !== observed) return null;
      if (state.opencode_session_id === observed && state.directory === DIRECTORY) return null;
      return { ...state, opencode_session_id: observed, directory: state.directory || DIRECTORY, liveness: "idle", last_event_at: now(), last_activity_at: now() };
    });
    return;
  }
  if (!observed) return;
  await update((state) => {
    if (state.opencode_session_id !== observed) return null;
    const timestamp = now();
    if (type === "session.status") {
      const status = event?.properties?.status || event?.properties?.info?.status;
      const liveness = status === "busy" ? "busy" : status === "idle" ? "idle" : status === "error" ? "error" : state.liveness;
      return { ...state, liveness, last_event_at: timestamp, last_activity_at: timestamp };
    }
    if (type === "session.idle") return { ...state, liveness: "idle", last_event_at: timestamp, last_activity_at: timestamp, active_tool: null };
    if (type === "session.error") return { ...state, liveness: "error", last_event_at: timestamp, last_activity_at: timestamp };
    if (type === "session.deleted") return { ...state, liveness: "deleted", last_event_at: timestamp, last_activity_at: timestamp, active_tool: null };
    return null;
  });
}

export const CassyPlugin = async () => ({
  event: async ({ event }) => { try { await applySessionEvent(event); } catch (error) { console.error("Cassy OpenCode event projection delayed:", error); } },
  "tool.execute.before": async (input) => {
    try { await update((state) => {
      if (state.opencode_session_id !== sessionId(input?.sessionID || input?.session_id)) return null;
      const timestamp = now();
      const tool = { name: input?.tool || input?.toolName || "unknown", call_id: input?.callID || input?.call_id || null, started_at: timestamp, completed_at: null, success: null };
      return { ...state, liveness: "busy", last_event_at: timestamp, last_activity_at: timestamp, active_tool: tool, last_tool: tool };
    }); } catch (error) { console.error("Cassy OpenCode before-hook projection delayed:", error); }
  },
  "tool.execute.after": async (input, output) => {
    try { await update((state) => {
      if (state.opencode_session_id !== sessionId(input?.sessionID || input?.session_id)) return null;
      const timestamp = now();
      const callID = input?.callID || input?.call_id || null;
      const active = state.active_tool && (!callID || state.active_tool.call_id === callID) ? state.active_tool : state.last_tool;
      const tool = active ? { ...active, completed_at: timestamp, success: !output?.error } : null;
      return { ...state, last_event_at: timestamp, last_activity_at: timestamp, active_tool: null, last_tool: tool || state.last_tool };
    }); } catch (error) { console.error("Cassy OpenCode after-hook projection delayed:", error); }
  }
});
"#;

/// A stable filesystem location for one Cassy/OpenCode session's state.
pub fn opencode_session_state_path(cas_root: &Path, cas_session_id: &str) -> PathBuf {
    cas_root
        .join(OPENCODE_STATE_DIRECTORY)
        .join(format!("{cas_session_id}.json"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeToolAttribution {
    pub name: String,
    pub call_id: Option<String>,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub success: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenCodeLiveness {
    Unknown,
    Busy,
    Idle,
    Error,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeLivenessVerdict {
    Signal(OpenCodeLiveness),
    ProcessAliveFallback,
    NotObserved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeSessionState {
    pub schema_version: u32,
    pub cas_session_id: String,
    pub opencode_session_id: Option<String>,
    pub directory: String,
    pub liveness: OpenCodeLiveness,
    pub last_event_at: u64,
    pub last_activity_at: Option<u64>,
    pub active_tool: Option<OpenCodeToolAttribution>,
    pub last_tool: Option<OpenCodeToolAttribution>,
}

impl OpenCodeSessionState {
    pub fn new(cas_session_id: impl Into<String>, directory: impl Into<String>) -> Self {
        Self {
            schema_version: OPENCODE_STATE_SCHEMA_VERSION,
            cas_session_id: cas_session_id.into(),
            opencode_session_id: None,
            directory: directory.into(),
            liveness: OpenCodeLiveness::Unknown,
            last_event_at: 0,
            last_activity_at: None,
            active_tool: None,
            last_tool: None,
        }
    }

    /// Reduce one plugin event.  The first root-session mapping is immutable;
    /// this is what makes delayed fire-and-forget callbacks safe.
    pub fn apply(&mut self, event: OpenCodeSessionEvent) -> OpenCodeEventOutcome {
        if event.at < self.last_event_at {
            return OpenCodeEventOutcome::IgnoredOutOfOrder;
        }
        match event.kind {
            OpenCodeSessionEventKind::RootCreated {
                session_id,
                directory,
            } => {
                if !valid_opencode_session_id(&session_id) {
                    return OpenCodeEventOutcome::IgnoredInvalidSession;
                }
                match &self.opencode_session_id {
                    Some(existing) if existing != &session_id => {
                        OpenCodeEventOutcome::IgnoredConflict
                    }
                    Some(_) if self.directory == directory => OpenCodeEventOutcome::Duplicate,
                    Some(_) => OpenCodeEventOutcome::IgnoredConflict,
                    None => {
                        self.opencode_session_id = Some(session_id);
                        self.directory = directory;
                        self.liveness = OpenCodeLiveness::Idle;
                        self.last_event_at = event.at;
                        self.last_activity_at = Some(event.at);
                        OpenCodeEventOutcome::Applied
                    }
                }
            }
            OpenCodeSessionEventKind::Status { session_id, status } => {
                if !self.matches_session(&session_id) {
                    return OpenCodeEventOutcome::IgnoredUnmapped;
                }
                let next = match status {
                    OpenCodeStatus::Busy => OpenCodeLiveness::Busy,
                    OpenCodeStatus::Idle => OpenCodeLiveness::Idle,
                    OpenCodeStatus::Error => OpenCodeLiveness::Error,
                    OpenCodeStatus::Deleted => OpenCodeLiveness::Deleted,
                };
                if self.liveness == next && self.last_event_at == event.at {
                    return OpenCodeEventOutcome::Duplicate;
                }
                self.liveness = next;
                self.last_event_at = event.at;
                self.last_activity_at = Some(event.at);
                if next != OpenCodeLiveness::Busy {
                    self.active_tool = None;
                }
                OpenCodeEventOutcome::Applied
            }
            OpenCodeSessionEventKind::ToolBefore {
                session_id,
                name,
                call_id,
            } => {
                if !self.matches_session(&session_id) {
                    return OpenCodeEventOutcome::IgnoredUnmapped;
                }
                let tool = OpenCodeToolAttribution {
                    name,
                    call_id,
                    started_at: event.at,
                    completed_at: None,
                    success: None,
                };
                if self.active_tool.as_ref() == Some(&tool) {
                    return OpenCodeEventOutcome::Duplicate;
                }
                self.liveness = OpenCodeLiveness::Busy;
                self.last_event_at = event.at;
                self.last_activity_at = Some(event.at);
                self.active_tool = Some(tool.clone());
                self.last_tool = Some(tool);
                OpenCodeEventOutcome::Applied
            }
            OpenCodeSessionEventKind::ToolAfter {
                session_id,
                call_id,
                success,
            } => {
                if !self.matches_session(&session_id) {
                    return OpenCodeEventOutcome::IgnoredUnmapped;
                }
                let Some(active) = self.active_tool.as_mut() else {
                    return OpenCodeEventOutcome::IgnoredUnmapped;
                };
                if call_id.is_some() && active.call_id != call_id {
                    return OpenCodeEventOutcome::IgnoredConflict;
                }
                active.completed_at = Some(event.at);
                active.success = Some(success);
                self.last_tool = self.active_tool.clone();
                self.active_tool = None;
                self.last_event_at = event.at;
                self.last_activity_at = Some(event.at);
                OpenCodeEventOutcome::Applied
            }
        }
    }

    pub fn liveness_verdict(
        &self,
        now_ms: u64,
        signal_ttl_ms: u64,
        process_alive: bool,
    ) -> OpenCodeLivenessVerdict {
        if self.last_event_at > 0 && now_ms.saturating_sub(self.last_event_at) <= signal_ttl_ms {
            return OpenCodeLivenessVerdict::Signal(self.liveness);
        }
        if process_alive && self.liveness != OpenCodeLiveness::Deleted {
            OpenCodeLivenessVerdict::ProcessAliveFallback
        } else {
            OpenCodeLivenessVerdict::NotObserved
        }
    }

    fn matches_session(&self, session_id: &str) -> bool {
        self.opencode_session_id.as_deref() == Some(session_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeSessionEvent {
    pub at: u64,
    pub kind: OpenCodeSessionEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenCodeSessionEventKind {
    RootCreated {
        session_id: String,
        directory: String,
    },
    Status {
        session_id: String,
        status: OpenCodeStatus,
    },
    ToolBefore {
        session_id: String,
        name: String,
        call_id: Option<String>,
    },
    ToolAfter {
        session_id: String,
        call_id: Option<String>,
        success: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeStatus {
    Busy,
    Idle,
    Error,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeEventOutcome {
    Applied,
    Duplicate,
    IgnoredConflict,
    IgnoredInvalidSession,
    IgnoredOutOfOrder,
    IgnoredUnmapped,
}

pub fn valid_opencode_session_id(session_id: &str) -> bool {
    session_id.starts_with("ses_")
        && session_id.len() > 4
        && !session_id.chars().any(char::is_whitespace)
}

pub fn load_opencode_session_state(path: &Path) -> std::io::Result<OpenCodeSessionState> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// Atomically persist a state snapshot.  The plugin uses the same temporary
/// file + rename contract in its generated JavaScript implementation.
pub fn persist_opencode_session_state(
    path: &Path,
    state: &OpenCodeSessionState,
) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "state path has no parent")
    })?;
    fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temp = path.with_extension(format!("json.tmp-{}-{suffix}", std::process::id()));
    fs::write(&temp, [&bytes[..], b"\n"].concat())?;
    fs::rename(temp, path)
}

/// Write the generated plugin source for a launch adapter.  The source is
/// static and contains no credentials; writing it beside the process-local
/// config keeps a project tree untouched.
pub fn persist_opencode_plugin(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "plugin path has no parent",
        )
    })?;
    fs::create_dir_all(parent)?;
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temp = path.with_extension(format!("mjs.tmp-{}-{suffix}", std::process::id()));
    fs::write(&temp, OPENCODE_PLUGIN_SOURCE.as_bytes())?;
    fs::rename(temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn spec() -> OpenCodeProjectionSpec {
        OpenCodeProjectionSpec::new(
            OpenCodeRole::Worker,
            "worker-1",
            "cas-session-1",
            "/tmp/cas-root",
            "/tmp/worktree",
            "/tmp/cas-root/opencode/cassy-opencode-plugin.mjs",
        )
        .with_model("local/qwen3.8-max")
        .with_variant("xhigh")
    }

    #[test]
    fn projection_config_is_deterministic_and_contains_both_primary_agents() {
        let first = render_opencode_config(&spec());
        let second = render_opencode_config(&spec());
        assert_eq!(first, second, "projection is a deterministic snapshot");
        assert_eq!(first, include_str!("opencode_projection.snapshot.json"));
        let json: Value = serde_json::from_str(&first).expect("valid OPENCODE_CONFIG_CONTENT");
        assert_eq!(json["agent"]["cassy-worker"]["mode"], "primary");
        assert_eq!(json["agent"]["cassy-supervisor"]["mode"], "primary");
        assert_eq!(json["agent"]["cassy-worker"]["model"], "local/qwen3.8-max");
        assert_eq!(json["agent"]["cassy-worker"]["variant"], "xhigh");
        assert_eq!(
            json["mcp"]["cas"]["command"],
            serde_json::json!(["cas", "serve"])
        );
        assert_eq!(
            json["plugin"][0],
            "/tmp/cas-root/opencode/cassy-opencode-plugin.mjs"
        );
        assert!(
            json["agent"]["cassy-worker"]["prompt"]
                .as_str()
                .unwrap()
                .contains("cas_task")
        );
    }

    #[test]
    fn projection_merge_preserves_hosted_provider_and_adds_live_plugin() {
        let base = serde_json::json!({
            "agent": {"cassy-worker": {"mode": "primary"}},
            "mcp": {"cas": {"type": "local", "command": ["cas", "serve"]}},
            "provider": {
                "qwencloud": {
                    "options": {
                        "baseURL": "https://token-plan.example/v1",
                        "apiKey": "{env:QWENCLOUD_TOKEN_PLAN_API_KEY}"
                    }
                }
            }
        })
        .to_string();
        let merged = merge_opencode_projection(&base, &spec()).unwrap();
        let json: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(
            json["provider"]["qwencloud"]["options"]["baseURL"],
            "https://token-plan.example/v1"
        );
        assert_eq!(
            json["provider"]["qwencloud"]["options"]["apiKey"],
            "{env:QWENCLOUD_TOKEN_PLAN_API_KEY}"
        );
        assert_eq!(
            json["plugin"][0],
            "/tmp/cas-root/opencode/cassy-opencode-plugin.mjs"
        );
        assert!(
            json["agent"]["cassy-worker"]["prompt"]
                .as_str()
                .unwrap()
                .contains("cas_task")
        );
    }

    #[test]
    fn generated_plugin_uses_awaited_hooks_and_explicit_permission_boundary() {
        assert!(OPENCODE_PLUGIN_SOURCE.contains("tool.execute.before"));
        assert!(OPENCODE_PLUGIN_SOURCE.contains("tool.execute.after"));
        assert!(OPENCODE_PLUGIN_SOURCE.contains("session.created"));
        assert!(OPENCODE_PLUGIN_SOURCE.contains("session.idle"));
        assert!(OPENCODE_PLUGIN_SOURCE.contains("await update"));
        assert!(OPENCODE_PLUGIN_SOURCE.contains("permission.ask is not"));
        assert!(OPENCODE_PLUGIN_SOURCE.contains("rename(temp, STATE_PATH)"));
        assert!(
            OPENCODE_PLUGIN_SOURCE.contains("export const CassyPlugin = async () => ({"),
            "OpenCode loads named plugin exports as factory functions"
        );
        assert!(!OPENCODE_PLUGIN_SOURCE.contains("export const Hooks = {"));
        assert!(!OPENCODE_PLUGIN_SOURCE.contains("SessionStart"));
        assert!(!OPENCODE_PLUGIN_SOURCE.contains("PreToolUse"));
    }

    #[test]
    fn worker_spec_projection_preserves_selector_fields() {
        let worker = crate::WorkerSpec {
            name: Some("opencode-worker".to_string()),
            cli: crate::SupervisorCli::Codex,
            model: Some("local/qwen3.8-max".to_string()),
            effort: Some(crate::Effort::High),
            config_dir: None,
            requester_config_dir: None,
            requester_secure_storage_dir: None,
        };
        let projection = OpenCodeProjectionSpec::from_worker_spec(
            &worker,
            OpenCodeRole::Worker,
            "cas-session-1",
            "/tmp/cas-root",
            "/tmp/worktree",
            "/tmp/cas-root/opencode/cassy-opencode-plugin.mjs",
        );
        assert_eq!(projection.name, "opencode-worker");
        assert_eq!(projection.model.as_deref(), Some("local/qwen3.8-max"));
        assert_eq!(projection.variant.as_deref(), Some("high"));
    }

    #[test]
    fn root_mapping_is_first_writer_wins_and_idempotent() {
        let mut state = OpenCodeSessionState::new("cas-1", "/tmp/project");
        let first = state.apply(OpenCodeSessionEvent {
            at: 10,
            kind: OpenCodeSessionEventKind::RootCreated {
                session_id: "ses_root".to_string(),
                directory: "/tmp/project".to_string(),
            },
        });
        assert_eq!(first, OpenCodeEventOutcome::Applied);
        assert_eq!(
            state.apply(OpenCodeSessionEvent {
                at: 10,
                kind: OpenCodeSessionEventKind::RootCreated {
                    session_id: "ses_root".to_string(),
                    directory: "/tmp/project".to_string(),
                },
            }),
            OpenCodeEventOutcome::Duplicate
        );
        assert_eq!(
            state.apply(OpenCodeSessionEvent {
                at: 11,
                kind: OpenCodeSessionEventKind::RootCreated {
                    session_id: "ses_child".to_string(),
                    directory: "/tmp/other".to_string(),
                },
            }),
            OpenCodeEventOutcome::IgnoredConflict
        );
        assert_eq!(state.opencode_session_id.as_deref(), Some("ses_root"));
        assert_eq!(state.directory, "/tmp/project");
    }

    #[test]
    fn out_of_order_and_unmapped_events_do_not_bind_or_mutate_identity() {
        let mut state = OpenCodeSessionState::new("cas-1", "/tmp/project");
        assert_eq!(
            state.apply(OpenCodeSessionEvent {
                at: 20,
                kind: OpenCodeSessionEventKind::Status {
                    session_id: "ses_late".to_string(),
                    status: OpenCodeStatus::Busy,
                },
            }),
            OpenCodeEventOutcome::IgnoredUnmapped
        );
        state.apply(OpenCodeSessionEvent {
            at: 30,
            kind: OpenCodeSessionEventKind::RootCreated {
                session_id: "ses_root".to_string(),
                directory: "/tmp/project".to_string(),
            },
        });
        assert_eq!(
            state.apply(OpenCodeSessionEvent {
                at: 29,
                kind: OpenCodeSessionEventKind::Status {
                    session_id: "ses_root".to_string(),
                    status: OpenCodeStatus::Busy,
                },
            }),
            OpenCodeEventOutcome::IgnoredOutOfOrder
        );
        assert_eq!(state.liveness, OpenCodeLiveness::Idle);
    }

    #[test]
    fn tool_before_after_records_attribution_and_status_liveness() {
        let mut state = OpenCodeSessionState::new("cas-1", "/tmp/project");
        state.apply(OpenCodeSessionEvent {
            at: 1,
            kind: OpenCodeSessionEventKind::RootCreated {
                session_id: "ses_root".to_string(),
                directory: "/tmp/project".to_string(),
            },
        });
        assert_eq!(
            state.apply(OpenCodeSessionEvent {
                at: 2,
                kind: OpenCodeSessionEventKind::ToolBefore {
                    session_id: "ses_root".to_string(),
                    name: "cas_task".to_string(),
                    call_id: Some("call-1".to_string()),
                },
            }),
            OpenCodeEventOutcome::Applied
        );
        assert_eq!(state.liveness, OpenCodeLiveness::Busy);
        assert_eq!(state.active_tool.as_ref().unwrap().name, "cas_task");
        state.apply(OpenCodeSessionEvent {
            at: 3,
            kind: OpenCodeSessionEventKind::ToolAfter {
                session_id: "ses_root".to_string(),
                call_id: Some("call-1".to_string()),
                success: true,
            },
        });
        assert!(state.active_tool.is_none());
        assert_eq!(state.last_tool.as_ref().unwrap().success, Some(true));
        assert_eq!(
            state.liveness_verdict(3, 10, false),
            OpenCodeLivenessVerdict::Signal(OpenCodeLiveness::Busy)
        );
        state.apply(OpenCodeSessionEvent {
            at: 4,
            kind: OpenCodeSessionEventKind::Status {
                session_id: "ses_root".to_string(),
                status: OpenCodeStatus::Idle,
            },
        });
        assert_eq!(state.liveness, OpenCodeLiveness::Idle);
    }

    #[test]
    fn stale_signals_degrade_to_process_evidence() {
        let mut state = OpenCodeSessionState::new("cas-1", "/tmp/project");
        state.apply(OpenCodeSessionEvent {
            at: 10,
            kind: OpenCodeSessionEventKind::RootCreated {
                session_id: "ses_root".to_string(),
                directory: "/tmp/project".to_string(),
            },
        });
        assert_eq!(
            state.liveness_verdict(100, 10, true),
            OpenCodeLivenessVerdict::ProcessAliveFallback
        );
        assert_eq!(
            state.liveness_verdict(100, 10, false),
            OpenCodeLivenessVerdict::NotObserved
        );
    }

    #[test]
    fn state_round_trips_through_atomic_file() {
        let temp = std::env::temp_dir().join(format!("cas-opencode-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let path = opencode_session_state_path(&temp, "cas-1");
        let state = OpenCodeSessionState::new("cas-1", "/tmp/project");
        persist_opencode_session_state(&path, &state).unwrap();
        assert_eq!(load_opencode_session_state(&path).unwrap(), state);
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn session_ids_are_restricted_to_opencode_namespace() {
        assert!(valid_opencode_session_id("ses_123"));
        assert!(!valid_opencode_session_id("uuid-123"));
        assert!(!valid_opencode_session_id("ses_"));
        assert!(!valid_opencode_session_id("ses_bad id"));
    }

    #[allow(dead_code)]
    fn _path_is_pathbuf(_: PathBuf) {}
}
