//! Native Agent Teams integration for factory daemon.
//!
//! Manages Claude Code's native Agent Teams file structure:
//! - `~/.claude/teams/{team-name}/config.json` — team member registry
//! - `~/.claude/teams/{team-name}/inboxes/{agent-name}.json` — per-agent inbox files
//!
//! This replaces the old prompt_queue + mux.inject (PTY stdin injection) transport
//! with native Teams mailbox writes that Claude Code polls internally.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Colors assigned to team members (matches Claude Code's palette).
const AGENT_COLORS: &[&str] = &["green", "blue", "yellow", "cyan", "magenta", "red", "white"];

/// The director agent name registered in the team config.
/// The daemon uses this identity when writing system/auto-prompt messages
/// to agent inboxes so that Claude Code recognizes the sender as a valid
/// team member.
pub const DIRECTOR_AGENT_NAME: &str = "director";

/// The inbox color used for director (automated coordinator) messages.
///
/// Must match the `color` field written to config.json for the director
/// `TeamMember` entry in [`TeamsManager::init_team_config`]. When
/// [`super::delivery::FactoryDaemon::deliver_to_worker`] calls
/// [`TeamsManager::write_to_inbox`] on behalf of the director it passes
/// this constant explicitly so the advertised color matches the config
/// record (cas-405f D-4).
pub const DIRECTOR_AGENT_COLOR: &str = "white";

/// Content-dedupe for identical (from, text) inbox writes (cas-7f57, cas-73c8).
///
/// The daemon can re-fire the same auto-prompt (e.g. "You have been assigned
/// cas-X") via event-detector resets, prompt_queue retries, outbox replays,
/// or SendMessage auto-route + native dual-write. A time-bounded window
/// (previously 10 minutes) still re-delivered handled messages after the
/// window expired with no redelivery marker (cas-73c8).
///
/// Guard: if the target's inbox already contains an identical `from` + `text`
/// entry (any age, still present in the file), skip the append. Intentional
/// redelivery must change the payload or include an explicit redelivery
/// marker so the text is no longer identical. Retention pruning eventually
/// drops old entries and allows a fresh identical send after cleanup.

/// Retention window for messages in the inbox file (task cas-7f57).
///
/// Inbox files are append-only today and `read: false` is never flipped to
/// true (see history comment on the field). Without pruning, every session
/// boot replays the entire accumulated history to Claude Code. On every
/// write we drop messages older than this window so the file stays bounded
/// and stale messages cannot haunt future sessions.
const INBOX_RETENTION: chrono::Duration = chrono::Duration::hours(2);

/// A single message in a Teams inbox file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxMessage {
    pub from: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub timestamp: String,
    pub color: String,
    pub read: bool,
    /// cas-ed6c: worker name a `WorkerIdle`-class alert concerns, if this
    /// row is one. `None` for every other message kind.
    ///
    /// WHY THIS EXISTS: `write_to_inbox` is a plain append to a file Claude
    /// Code only polls at ITS OWN turn boundaries (see `deliver_to_worker`'s
    /// doc comment on `TeamsInbox` delivery) — and `read` is never flipped
    /// to `true` by production code (see the field above), so a written row
    /// sits here, unmodified, until the recipient happens to read it. A
    /// `WorkerIdle` alert is generated and revalidated against LIVE state
    /// at write time (`revalidate_event_for_delivery_with_context`), but
    /// that only proves it was true THEN — if the named worker is
    /// subsequently assigned real work before the recipient's next turn
    /// boundary (which can be minutes away if the recipient is mid-turn),
    /// the already-written row is stale and nothing retracts it. Live
    /// incident: three workers (`interrupt-fixer`/`close-guardrail`/
    /// `activity-clock`) were announced idle/ready in one batch ~7 minutes
    /// after each had a genuine InProgress assignment — the alert content
    /// was true at ~21:22Z (before assignment) and false by the time the
    /// supervisor's client actually surfaced it at ~21:29Z.
    ///
    /// `prune_stale_idle_alerts` uses this tag to retract a queued alert
    /// proactively, before the recipient ever sees it, instead of relying
    /// on generation-time revalidation alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retract_worker: Option<String>,
}

/// Team member entry in config.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMember {
    pub agent_id: String,
    pub name: String,
    pub agent_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_mode_required: Option<bool>,
    pub joined_at: u64,
    pub tmux_pane_id: String,
    pub cwd: String,
    #[serde(default)]
    pub subscriptions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_type: Option<String>,
}

/// Team config.json structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamConfig {
    pub name: String,
    pub description: String,
    pub created_at: u64,
    pub lead_agent_id: String,
    pub lead_session_id: String,
    pub members: Vec<TeamMember>,
}

/// Manages the native Agent Teams file structure for a factory session.
pub struct TeamsManager {
    team_name: String,
    teams_dir: PathBuf,
    inboxes_dir: PathBuf,
}

impl TeamsManager {
    /// Create a new TeamsManager for the given factory session.
    ///
    /// The team name is derived from the session name.
    /// Files are stored at `~/.claude/teams/{team-name}/`.
    pub fn new(session_name: &str) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let teams_dir = home.join(".claude").join("teams").join(session_name);
        let inboxes_dir = teams_dir.join("inboxes");

        Self {
            team_name: session_name.to_string(),
            teams_dir,
            inboxes_dir,
        }
    }

    /// Get the team name.
    pub fn team_name(&self) -> &str {
        &self.team_name
    }

    /// Format an agent ID: `{name}@{team-name}`.
    pub fn agent_id_for(&self, name: &str) -> String {
        format!("{}@{}", name, self.team_name)
    }

    /// Build a teams_configs HashMap for MuxConfig before agents are spawned.
    ///
    /// This is a static method because it's called before TeamsManager is fully
    /// initialized (before `init_team_config`). It constructs the CLI flags map
    /// that Mux::factory() uses when spawning agent PTYs.
    /// Returns `(configs_map, lead_session_id)`.
    pub fn build_configs_for_mux(
        session_name: &str,
        supervisor_name: &str,
        worker_names: &[String],
    ) -> (
        std::collections::HashMap<String, cas_mux::TeamsSpawnConfig>,
        String,
    ) {
        let mut configs = std::collections::HashMap::new();
        let lead_session_id = uuid::Uuid::new_v4().to_string();

        // Supervisor settings path — auto-allow the filesystem tool families
        // so the supervisor's tool calls don't get forwarded to itself via
        // team permission routing (self-leadership deadlock). Workers get
        // the same treatment below via per-worker settings files (cas-e15d)
        // to avoid the symmetric phantom `team-lead` hang.
        //
        // The file is written *here*, eagerly, because the factory spawns the
        // supervisor PTY during `FactoryApp::new` — which runs *before*
        // `init_team_config` in the daemon boot sequence. If we deferred the
        // write, `claude` would be launched with `--settings <path>` pointing
        // at a file that doesn't yet exist and would silently skip our
        // allowlist, recreating the deadlock. Writing here keeps the
        // invariant "path in TeamsSpawnConfig implies file exists on disk".
        let supervisor_settings_path_buf = Self::supervisor_settings_path_for(session_name);
        let supervisor_settings_path = supervisor_settings_path_buf.to_string_lossy().to_string();
        if let Err(e) = Self::write_supervisor_settings_to(&supervisor_settings_path_buf) {
            // Downgrade to a warning so the factory still boots on transient
            // I/O issues; if the write fails the supervisor falls back to
            // the pre-fix (deadlock-prone) behavior but everything else
            // proceeds and the log makes post-hoc diagnosis obvious.
            tracing::warn!(
                "Failed to pre-write supervisor settings at {:?}: {}",
                supervisor_settings_path_buf,
                e
            );
        }

        // Supervisor — keyed by pane name for PTY lookup, but agent_name is
        // always "supervisor" so Claude identifies as "supervisor" in the team.
        configs.insert(
            supervisor_name.to_string(),
            cas_mux::TeamsSpawnConfig {
                team_name: session_name.to_string(),
                agent_id: format!("supervisor@{}", session_name),
                agent_name: "supervisor".to_string(),
                agent_color: "green".to_string(),
                agent_type: "team-lead".to_string(),
                parent_session_id: None,
                lead_session_id: Some(lead_session_id.clone()),
                settings_path: Some(supervisor_settings_path),
            },
        );

        // Workers — each gets its own per-worker settings file so
        // filesystem tool calls auto-approve instead of escalating to the
        // phantom `team-lead` mailbox. Same eager-write invariant as the
        // supervisor: the file must exist on disk *before* the PTY spawns
        // with `--settings <path>` or claude silently falls back to the
        // stock allowlist.
        for (i, name) in worker_names.iter().enumerate() {
            let worker_settings_path_buf = Self::worker_settings_path_for(session_name, name);
            let worker_settings_path =
                worker_settings_path_buf.to_string_lossy().to_string();
            if let Err(e) = Self::write_worker_settings_to(&worker_settings_path_buf) {
                tracing::warn!(
                    "Failed to pre-write worker settings for {} at {:?}: {}",
                    name,
                    worker_settings_path_buf,
                    e
                );
            }

            configs.insert(
                name.clone(),
                cas_mux::TeamsSpawnConfig {
                    team_name: session_name.to_string(),
                    agent_id: format!("{}@{}", name, session_name),
                    agent_name: name.clone(),
                    agent_color: Self::color_for_index(i).to_string(),
                    agent_type: "general-purpose".to_string(),
                    parent_session_id: Some(lead_session_id.clone()),
                    lead_session_id: None,
                    settings_path: Some(worker_settings_path),
                },
            );
        }

        (configs, lead_session_id)
    }

    /// Compute the on-disk path of the supervisor-only settings file for a
    /// given session name. The file lives alongside `config.json` under
    /// `~/.claude/teams/{session}/supervisor-settings.json` and is written by
    /// [`Self::build_configs_for_mux`] (eagerly, before PTY spawn) and
    /// re-written by [`Self::init_team_config`] (idempotent rewrite after the
    /// team directory is fully populated). See [`supervisor_settings_contents`]
    /// for the allowlist shape.
    pub fn supervisor_settings_path_for(session_name: &str) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".claude")
            .join("teams")
            .join(session_name)
            .join("supervisor-settings.json")
    }

    /// Write `supervisor-settings.json` at the given absolute path, creating
    /// the parent directory if needed. Static variant used by
    /// [`Self::build_configs_for_mux`], which runs before any instance of
    /// `TeamsManager` is constructed. Idempotent and safe to call repeatedly.
    pub fn write_supervisor_settings_to(path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(&Self::supervisor_settings_contents())?;
        std::fs::write(path, body)?;
        tracing::info!("Wrote supervisor settings at {:?}", path);
        Ok(())
    }

    /// The JSON body of the supervisor settings file — a Claude Code
    /// `permissions.allow` list that auto-approves the four tool families
    /// whose approvals would otherwise route back to the supervisor itself,
    /// plus a `hooks` block wiring `PreToolUse` and `PermissionRequest` to
    /// `cas hook` so the factory auto-approve handlers actually run.
    ///
    /// Kept tight on purpose: no MCP tools, no network, no shell glob expansion
    /// beyond the base tool name. The deadlock only fires for tools that are
    /// not otherwise auto-allowed, and the factory supervisor already runs
    /// with `--dangerously-skip-permissions`, so this list is the minimum set
    /// observed to produce the routing-deadlock symptom.
    ///
    /// The `hooks` block is the load-bearing belt under Claude Code 2.1.x:
    /// the `permissions.allow` list alone does NOT short-circuit the
    /// team-mode UG9 escalation (see `pre_tool.rs` for the disassembly), so
    /// without these hook entries the supervisor self-deadlocks on every
    /// permission gate that is not otherwise auto-approved.
    pub fn supervisor_settings_contents() -> serde_json::Value {
        let mut body = serde_json::json!({
            "permissions": { "allow": Self::factory_allow_list() },
        });
        body.as_object_mut()
            .expect("object literal")
            .insert("hooks".to_string(), Self::factory_hooks_block());
        body
    }

    /// Compute the on-disk path of a worker's settings file. Lives alongside
    /// `config.json` under `~/.claude/teams/{session}/{worker_name}-settings.json`.
    /// Mirrors [`Self::supervisor_settings_path_for`] — same eager-write
    /// invariant applies.
    pub fn worker_settings_path_for(session_name: &str, worker_name: &str) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".claude")
            .join("teams")
            .join(session_name)
            .join(format!("{worker_name}-settings.json"))
    }

    /// Write a worker settings file at the given absolute path, creating the
    /// parent directory if needed. Idempotent.
    pub fn write_worker_settings_to(path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(&Self::worker_settings_contents())?;
        std::fs::write(path, body)?;
        tracing::info!("Wrote worker settings at {:?}", path);
        Ok(())
    }

    /// JSON body of a worker settings file. Covers the filesystem tool
    /// families whose approvals would otherwise escalate to the phantom
    /// `team-lead` mailbox (agentType misread as name, upstream) and hang.
    ///
    /// Kept to the same shape as [`Self::supervisor_settings_contents`] so
    /// both roles share one surface area review. Any tool added here should
    /// also be added to the supervisor list unless we have a specific reason
    /// to diverge.
    pub fn worker_settings_contents() -> serde_json::Value {
        let mut body = serde_json::json!({
            "permissions": { "allow": Self::factory_allow_list() },
        });
        body.as_object_mut()
            .expect("object literal")
            .insert("hooks".to_string(), Self::factory_hooks_block());
        body
    }

    /// Filesystem tool families whose approval would otherwise route to the
    /// phantom `team-lead` mailbox under Claude Code 2.1.x (see UG9 bug in
    /// `pre_tool.rs`). Used only by the `permissions.allow` list; the
    /// `PreToolUse` hook matcher also includes intercept-only tools that must
    /// not be auto-allowed.
    ///
    /// MUST stay in sync with `FACTORY_AUTO_APPROVE_TOOLS` in
    /// `cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs` — the hook
    /// handler reads that list to decide whether to auto-approve. If they
    /// diverge, ops in this list (but not the hook list) will hang anyway,
    /// and ops in the hook list (but not this matcher) will fire the hook
    /// for nothing.
    fn factory_allow_list() -> &'static [&'static str] {
        &["Read", "Write", "Edit", "Glob", "Grep", "Bash", "NotebookEdit"]
    }

    /// Tools that must reach the `PreToolUse` hook for factory-specific
    /// routing or denial, but must not be listed in `permissions.allow`.
    fn factory_pre_tool_intercept_list() -> &'static [&'static str] {
        &["SendMessage", "AskUserQuestion"]
    }

    /// `hooks` block for per-role settings files. Wires `PreToolUse` (belt
    /// #2) and `PermissionRequest` (belt #3) to `cas hook <event>`, which is
    /// what actually short-circuits the team-mode leader-escalation deadlock
    /// on Claude Code 2.1.x — the `permissions.allow` list alone does not.
    ///
    /// Emits shell-form `"command"` to match `cli/hook/config_gen.rs`
    /// (cas-c17b). The earlier exec-form emit (cas-9a60) was malformed
    /// (`"args": ["cas", ...]` with no `command` field) and tripped
    /// /doctor on every CC version, regardless of #58441 state.
    ///
    /// Defaults mirror `cli/hook/config_gen.rs`: 2000ms timeout for both.
    /// `PreToolUse` matcher covers the filesystem allow list plus
    /// intercept-only tools like `SendMessage` and `AskUserQuestion`; those
    /// intercept-only tools are deliberately omitted from `permissions.allow`.
    fn factory_hooks_block() -> serde_json::Value {
        let matcher = Self::factory_allow_list()
            .iter()
            .chain(Self::factory_pre_tool_intercept_list().iter())
            .copied()
            .collect::<Vec<_>>()
            .join("|");
        serde_json::json!({
            "PreToolUse": [
                {
                    "matcher": matcher,
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook PreToolUse",
                            "timeout": 2000
                        }
                    ]
                }
            ],
            "PermissionRequest": [
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": "cas hook PermissionRequest",
                            "timeout": 2000
                        }
                    ]
                }
            ]
        })
    }

    /// Assign a color to an agent based on its index in the team.
    pub fn color_for_index(index: usize) -> &'static str {
        AGENT_COLORS[index % AGENT_COLORS.len()]
    }

    /// Initialize the team directory and write config.json with supervisor + initial workers.
    ///
    /// `worker_cwds` maps worker names to their actual working directories (worktree paths
    /// when worktrees are enabled). Workers not in the map use `project_cwd` as fallback.
    pub fn init_team_config(
        &self,
        worker_names: &[String],
        project_cwd: &std::path::Path,
        worker_cwds: &std::collections::HashMap<String, std::path::PathBuf>,
        lead_session_id: &str,
    ) -> anyhow::Result<()> {
        // Create directories
        std::fs::create_dir_all(&self.inboxes_dir)?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let project_cwd_str = project_cwd.to_string_lossy().to_string();

        let model = Some("claude-opus-4-7".to_string());

        // Supervisor is the team lead but also a teammate so it polls its inbox.
        // Always registered as "supervisor" regardless of the generated pane name.
        let mut members = vec![TeamMember {
            agent_id: self.agent_id_for("supervisor"),
            name: "supervisor".to_string(),
            agent_type: "team-lead".to_string(),
            model: model.clone(),
            prompt: None,
            color: Some("green".to_string()),
            plan_mode_required: None,
            joined_at: now,
            tmux_pane_id: "tmux".to_string(),
            cwd: project_cwd_str.clone(),
            subscriptions: Vec::new(),
            backend_type: Some("tmux".to_string()),
        }];

        // Director is the daemon's identity for system/auto-prompt messages.
        // Registered as a team member so Claude Code accepts messages from it.
        //
        // `backend_type` is `None` (not "tmux") because the director has no real
        // process or PTY — it is an automated coordinator, not a live peer. Setting
        // "tmux" here (cas-405f D-2) caused CC to render director messages as coming
        // from a live teammate rather than an automated system source. A missing/None
        // backend_type signals to CC that this is a non-interactive sender.
        //
        // `color` must stay "white" and must match `DIRECTOR_AGENT_COLOR` — the two
        // constants are intentionally kept in sync so inbox writes and the config
        // entry advertise the same color (cas-405f D-4).
        members.push(TeamMember {
            agent_id: self.agent_id_for(DIRECTOR_AGENT_NAME),
            name: DIRECTOR_AGENT_NAME.to_string(),
            agent_type: "director".to_string(),
            model: model.clone(),
            prompt: None,
            color: Some(DIRECTOR_AGENT_COLOR.to_string()),
            plan_mode_required: None,
            joined_at: now,
            tmux_pane_id: "tmux".to_string(),
            cwd: project_cwd_str.clone(),
            subscriptions: Vec::new(),
            backend_type: None,
        });

        // Add workers (each may have its own worktree path)
        for (i, worker_name) in worker_names.iter().enumerate() {
            let worker_cwd = worker_cwds
                .get(worker_name)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| project_cwd_str.clone());

            members.push(TeamMember {
                agent_id: self.agent_id_for(worker_name),
                name: worker_name.clone(),
                agent_type: "general-purpose".to_string(),
                model: model.clone(),
                prompt: None,
                color: Some(Self::color_for_index(i).to_string()),
                plan_mode_required: Some(false),
                joined_at: now,
                tmux_pane_id: "tmux".to_string(),
                cwd: worker_cwd,
                subscriptions: Vec::new(),
                backend_type: Some("tmux".to_string()),
            });
        }

        let config = TeamConfig {
            name: self.team_name.clone(),
            description: format!("CAS factory session {}", self.team_name),
            created_at: now,
            lead_agent_id: self.agent_id_for("supervisor"),
            lead_session_id: lead_session_id.to_string(),
            members,
        };

        let config_path = self.teams_dir.join("config.json");
        let json = serde_json::to_string_pretty(&config)?;
        std::fs::write(&config_path, json)?;

        // Re-write the supervisor-only settings file. `build_configs_for_mux`
        // already wrote it eagerly (before `FactoryApp::new` spawned the
        // supervisor PTY, which is when `--settings <path>` needs to resolve).
        // We rewrite it here defensively so that code paths reaching
        // `init_team_config` without going through `build_configs_for_mux`
        // still end up with a valid file on disk. The write is idempotent.
        self.write_supervisor_settings()?;

        // Create empty inbox files for all agents
        self.ensure_inbox("supervisor")?;
        self.ensure_inbox(DIRECTOR_AGENT_NAME)?;
        for worker_name in worker_names {
            self.ensure_inbox(worker_name)?;
        }

        tracing::info!(
            "Initialized Teams config at {:?} with {} members",
            config_path,
            1 + worker_names.len()
        );

        Ok(())
    }

    /// Write `supervisor-settings.json` in the team directory. Safe to call
    /// multiple times; the content is fixed so repeated writes are idempotent.
    /// Delegates to [`Self::write_supervisor_settings_to`] so the eager-write
    /// path and the `init_team_config` rewrite share a single implementation.
    pub fn write_supervisor_settings(&self) -> anyhow::Result<()> {
        Self::write_supervisor_settings_to(&self.teams_dir.join("supervisor-settings.json"))
    }

    /// Add a new member to the team (e.g., when a worker is spawned dynamically).
    pub fn add_member(
        &self,
        name: &str,
        cwd: &std::path::Path,
        color_index: usize,
    ) -> anyhow::Result<()> {
        let config_path = self.teams_dir.join("config.json");
        let json = std::fs::read_to_string(&config_path)?;
        let mut config: TeamConfig = serde_json::from_str(&json)?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        config.members.push(TeamMember {
            agent_id: self.agent_id_for(name),
            name: name.to_string(),
            agent_type: "general-purpose".to_string(),
            model: Some("claude-opus-4-7".to_string()),
            prompt: None,
            color: Some(Self::color_for_index(color_index).to_string()),
            plan_mode_required: Some(false),
            joined_at: now,
            tmux_pane_id: "tmux".to_string(),
            cwd: cwd.to_string_lossy().to_string(),
            subscriptions: Vec::new(),
            backend_type: Some("tmux".to_string()),
        });

        let json = serde_json::to_string_pretty(&config)?;
        std::fs::write(&config_path, json)?;

        self.ensure_inbox(name)?;

        tracing::info!("Added team member '{}' to {}", name, self.team_name);
        Ok(())
    }

    /// Remove a member from the team (e.g., when a worker is shut down).
    pub fn remove_member(&self, name: &str) -> anyhow::Result<()> {
        let config_path = self.teams_dir.join("config.json");
        let json = std::fs::read_to_string(&config_path)?;
        let mut config: TeamConfig = serde_json::from_str(&json)?;

        config.members.retain(|m| m.name != name);

        let json = serde_json::to_string_pretty(&config)?;
        std::fs::write(&config_path, json)?;

        // Remove inbox file
        let inbox_path = self.inboxes_dir.join(format!("{}.json", name));
        let _ = std::fs::remove_file(&inbox_path);

        tracing::info!("Removed team member '{}' from {}", name, self.team_name);
        Ok(())
    }

    /// Write a message to a target agent's inbox file.
    ///
    /// Uses file locking to prevent corruption when multiple writers
    /// (daemon + agents) access the same inbox concurrently.
    pub fn write_to_inbox(
        &self,
        target: &str,
        from: &str,
        message: &str,
        summary: Option<&str>,
        color: Option<&str>,
    ) -> anyhow::Result<()> {
        self.write_to_inbox_impl(target, from, message, summary, color, None)
    }

    /// Like [`Self::write_to_inbox`], but tags the queued row with the
    /// worker name a `WorkerIdle`-class alert concerns (cas-ed6c). Use this
    /// ONLY for prompts generated from `DirectorEvent::WorkerIdle` — the tag
    /// is what lets [`Self::prune_stale_idle_alerts`] retract the row later
    /// if reality changes before the recipient ever reads it.
    pub fn write_to_inbox_for_worker_idle(
        &self,
        target: &str,
        from: &str,
        message: &str,
        summary: Option<&str>,
        color: Option<&str>,
        worker: &str,
    ) -> anyhow::Result<()> {
        self.write_to_inbox_impl(target, from, message, summary, color, Some(worker))
    }

    fn write_to_inbox_impl(
        &self,
        target: &str,
        from: &str,
        message: &str,
        summary: Option<&str>,
        color: Option<&str>,
        retract_worker: Option<&str>,
    ) -> anyhow::Result<()> {
        let inbox_path = self.inboxes_dir.join(format!("{}.json", target));

        // Ensure inbox file exists
        if !inbox_path.exists() {
            std::fs::write(&inbox_path, "[]")?;
        }

        // Use file locking for safe concurrent access
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&inbox_path)?;

        // Acquire exclusive lock
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
        if ret != 0 {
            anyhow::bail!(
                "Failed to lock inbox file {:?}: {}",
                inbox_path,
                std::io::Error::last_os_error()
            );
        }

        // Read existing messages
        let mut messages: Vec<InboxMessage> = {
            let content = std::fs::read_to_string(&inbox_path).unwrap_or_else(|_| "[]".to_string());
            serde_json::from_str(&content).unwrap_or_default()
        };

        let now_utc = chrono::Utc::now();
        let now = now_utc.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let resolved_color = color.unwrap_or("green").to_string();

        // Always set summary — native Claude Code expects it.
        // Fall back to the message text when no explicit summary is provided.
        let resolved_summary = summary.unwrap_or(message).to_string();

        // Prune messages older than INBOX_RETENTION so the file cannot grow
        // unbounded across sessions — see cas-7f57 and the comment on
        // INBOX_RETENTION above.
        //
        // Unread messages (`read: false`) are preserved regardless of age
        // so a supervisor's recovery/unblock prompt to a wedged worker
        // cannot silently evaporate after the 2h window. See
        // memory `feedback_supervisor_stop_message_latency` — stale
        // STOP messages are already a known delivery hazard, and age-only
        // retention would strictly worsen that failure mode.
        let retention_cutoff = now_utc - INBOX_RETENTION;
        let messages_before_retain = messages.len();
        messages.retain(|m| {
            if !m.read {
                return true;
            }
            match chrono::DateTime::parse_from_rfc3339(&m.timestamp) {
                Ok(ts) => ts.with_timezone(&chrono::Utc) >= retention_cutoff,
                // Unparseable timestamp: keep the message rather than
                // silently drop real data; a future migration can clean
                // these up.
                Err(_) => true,
            }
        });
        let retention_pruned = messages.len() != messages_before_retain;

        // Dedup guard (cas-7f57 / cas-73c8): if an identical (from, text)
        // message is still present in the inbox, skip the append — no
        // time window. Prevents director/prompt_queue/outbox replay and
        // post-handle redelivery without an intentional redelivery marker.
        let is_content_duplicate = messages
            .iter()
            .rev()
            .any(|m| m.from == from && m.text == message);

        if is_content_duplicate {
            tracing::debug!(
                target: "cas::coordination",
                stage = "dedup_skip",
                channel = "teams_inbox",
                from = from,
                target_agent = target,
                "inbox write skipped — identical message already present"
            );
            // Only re-serialize+write if the retention sweep actually
            // removed anything; otherwise this is a pure no-op and we
            // avoid a write storm on hot duplicate senders.
            if retention_pruned {
                let json = serde_json::to_string_pretty(&messages)?;
                std::fs::write(&inbox_path, json)?;
            }
            unsafe { libc::flock(fd, libc::LOCK_UN) };
            return Ok(());
        }

        messages.push(InboxMessage {
            from: from.to_string(),
            text: message.to_string(),
            summary: Some(resolved_summary),
            timestamp: now,
            color: resolved_color,
            read: false,
            retract_worker: retract_worker.map(str::to_string),
        });

        // Write back
        let json = serde_json::to_string_pretty(&messages)?;
        std::fs::write(&inbox_path, json)?;

        // Release lock (automatic on drop, but be explicit)
        unsafe { libc::flock(fd, libc::LOCK_UN) };

        tracing::debug!("Wrote message to inbox: {} -> {}", from, target);

        Ok(())
    }

    /// Retract stale `WorkerIdle`-class rows from `target`'s inbox (cas-ed6c).
    ///
    /// A `WorkerIdle` alert is revalidated against live state only at the
    /// moment it's WRITTEN (`revalidate_event_for_delivery_with_context`).
    /// The row then sits in this file, untouched, until Claude Code polls
    /// its inbox at ITS OWN turn boundary — which can be minutes away if the
    /// recipient is mid-turn. If the tagged worker gains a real assignment
    /// in that gap, the write-time revalidation can never catch it because
    /// it already ran. This sweep re-checks every UNREAD row carrying a
    /// `retract_worker` tag against a caller-supplied live predicate and
    /// drops any whose claim is now false — proactively, before the
    /// recipient ever sees it, rather than relying on a one-shot check at
    /// write time. Call once per director tick alongside prompt generation
    /// (see `revalidate_and_prompt_for_delivery`'s caller in `lifecycle.rs`),
    /// reusing the same live snapshot already loaded that tick.
    ///
    /// `worker_now_has_assignment(worker) == true` means the alert is now
    /// stale (the worker is no longer idle) and the row is removed. Rows
    /// without a `retract_worker` tag, or already marked `read`, are left
    /// untouched — this only ever retracts an alert of the specific kind it
    /// was built for, never a message a human might already have seen.
    ///
    /// Returns the number of rows retracted (0 if the inbox doesn't exist,
    /// can't be parsed, or nothing matched — never an error for a missing
    /// file, since "no inbox yet" is not a failure here).
    pub fn prune_stale_idle_alerts(
        &self,
        target: &str,
        worker_now_has_assignment: impl Fn(&str) -> bool,
    ) -> anyhow::Result<usize> {
        let inbox_path = self.inboxes_dir.join(format!("{}.json", target));
        if !inbox_path.exists() {
            return Ok(0);
        }

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&inbox_path)?;
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
        if ret != 0 {
            anyhow::bail!(
                "Failed to lock inbox file {:?}: {}",
                inbox_path,
                std::io::Error::last_os_error()
            );
        }

        let mut messages: Vec<InboxMessage> = {
            let content = std::fs::read_to_string(&inbox_path).unwrap_or_else(|_| "[]".to_string());
            serde_json::from_str(&content).unwrap_or_default()
        };

        let before = messages.len();
        messages.retain(|m| {
            if m.read {
                return true;
            }
            match &m.retract_worker {
                Some(worker) if worker_now_has_assignment(worker) => {
                    tracing::info!(
                        target: "cas::coordination",
                        stage = "retract_stale_idle_alert",
                        channel = "teams_inbox",
                        worker = %worker,
                        target_agent = target,
                        "retracting queued WorkerIdle alert — worker gained a real \
                         assignment before this row was read"
                    );
                    false
                }
                _ => true,
            }
        });
        let removed = before - messages.len();

        if removed > 0 {
            let json = serde_json::to_string_pretty(&messages)?;
            std::fs::write(&inbox_path, json)?;
        }

        unsafe { libc::flock(fd, libc::LOCK_UN) };
        Ok(removed)
    }

    /// Ensure an inbox file exists for the given agent.
    fn ensure_inbox(&self, name: &str) -> anyhow::Result<()> {
        let inbox_path = self.inboxes_dir.join(format!("{}.json", name));
        if !inbox_path.exists() {
            std::fs::write(&inbox_path, "[]")?;
        }
        Ok(())
    }

    /// Clean up the team directory on shutdown.
    pub fn cleanup(&self) {
        if self.teams_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&self.teams_dir) {
                tracing::warn!("Failed to clean up teams dir {:?}: {}", self.teams_dir, e);
            } else {
                tracing::info!("Cleaned up teams dir {:?}", self.teams_dir);
            }
        }
    }

    /// Remove orphaned team directories whose daemon is no longer running.
    ///
    /// Scans `~/.claude/teams/` for directories and checks if the corresponding
    /// factory daemon socket (`~/.cas/factory-{name}.sock`) still exists. If the
    /// socket is gone, the daemon crashed without cleaning up and the team
    /// directory is safe to remove.
    ///
    /// Called once at daemon startup to clean up after previous crashes.
    pub fn cleanup_orphans() {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let teams_root = home.join(".claude").join("teams");

        let entries = match std::fs::read_dir(&teams_root) {
            Ok(entries) => entries,
            Err(_) => return, // No teams directory at all
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };

            // Check if the factory daemon socket still exists
            let sock_path = home.join(".cas").join(format!("factory-{dir_name}.sock"));

            if !sock_path.exists() {
                tracing::info!(
                    "Removing orphaned teams directory {:?} (no daemon socket)",
                    path
                );
                if let Err(e) = std::fs::remove_dir_all(&path) {
                    tracing::warn!("Failed to remove orphaned teams dir {:?}: {}", path, e);
                }
            }
        }
    }

    /// Build a `cas_mux::TeamsSpawnConfig` for spawning a new agent with native teams flags.
    ///
    /// Used for dynamically-added workers (agents added after the initial
    /// `init_team_config` call). Eagerly writes the per-worker settings file
    /// into `self.teams_dir` so the spawned `claude` invocation's
    /// `--settings <path>` resolves at PTY start — same invariant as the
    /// eager-write path in [`Self::build_configs_for_mux`].
    pub fn spawn_config_for(
        &self,
        name: &str,
        agent_type: &str,
        color: &str,
        parent_session_id: Option<&str>,
    ) -> cas_mux::TeamsSpawnConfig {
        let worker_settings_path = self
            .teams_dir
            .join(format!("{name}-settings.json"));
        if let Err(e) = Self::write_worker_settings_to(&worker_settings_path) {
            tracing::warn!(
                "Failed to write worker settings for {} at {:?}: {}",
                name,
                worker_settings_path,
                e
            );
        }

        cas_mux::TeamsSpawnConfig {
            team_name: self.team_name.clone(),
            agent_id: self.agent_id_for(name),
            agent_name: name.to_string(),
            agent_color: color.to_string(),
            agent_type: agent_type.to_string(),
            parent_session_id: parent_session_id.map(|s| s.to_string()),
            lead_session_id: None,
            settings_path: Some(worker_settings_path.to_string_lossy().to_string()),
        }
    }

    // ── Worker pre-commit guard (cas-bea2, LAYER 2) ───────────────────────

    /// Shell-form pre-commit hook content that hard-refuses commits on protected
    /// branches (main/master/staging). Installed into each isolated worker's
    /// git repo by [`Self::install_worker_pre_commit_hook`].
    ///
    /// Shell-form (`#!/bin/sh`) per the cas-7ecd scar — exec-form hooks trip
    /// Claude Code's /doctor validator regardless of Anthropic #58441 state.
    pub const WORKER_PRE_COMMIT_HOOK: &'static str = "#!/bin/sh
# CAS factory worker guard — installed by `cas factory` when spawning isolated workers.
# Workers may ONLY commit on their own factory/<name> branch. All other branches
# (main, master, staging, epic/*, arbitrary branches, and detached HEAD) are denied.
branch=$(git symbolic-ref --short HEAD 2>/dev/null)
case \"$branch\" in
  factory/*)
    exit 0
    ;;
  *)
    if [ -z \"$branch\" ]; then
      echo \"CAS COMMIT GUARD: HEAD is detached — cannot determine branch.\" >&2
    else
      echo \"CAS COMMIT GUARD: Cannot commit on '$branch'.\" >&2
    fi
    echo \"Workers may only commit on their factory/<name> branch.\" >&2
    exit 1
    ;;
esac
";

    /// Marker string that identifies a CAS-installed guard hook (any version).
    const GUARD_MARKER: &'static str = "CAS factory worker guard";

    /// The exact block header [`Self::write_guard_alongside`] appends when
    /// chaining the guard onto a pre-existing project hook. Also used by
    /// [`Self::cleanup_legacy_shared_guard`] to find where a chained CAS
    /// block begins so it can be stripped without touching the project
    /// hook's own content. Keep these two in sync.
    const GUARD_SOURCING_HEADER: &'static str =
        "\n# CAS factory worker guard (sourced by cas factory — do not remove)\n";

    /// Resolve an absolute path from a `git -C <dir> rev-parse --git-path <arg>`
    /// (or `--git-dir`) invocation. Relative results (as returned for plain,
    /// non-worktree repos) are joined against `dir` so callers always get an
    /// absolute path regardless of the process's own CWD.
    fn run_git_path(dir: &std::path::Path, args: &[&str]) -> anyhow::Result<std::path::PathBuf> {
        let dir_str = dir.to_string_lossy();
        let mut full_args: Vec<&str> = vec!["-C", &dir_str];
        full_args.extend_from_slice(args);
        let output = std::process::Command::new("git").args(&full_args).output()?;
        if !output.status.success() {
            anyhow::bail!(
                "git {:?} failed in {:?}: {}",
                args,
                dir,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let p = std::path::Path::new(&raw);
        Ok(if p.is_absolute() {
            std::path::PathBuf::from(p)
        } else {
            dir.join(p)
        })
    }

    /// Run a `git -C <dir> <args...>` command for side effects only (config
    /// writes), bailing with stderr context on non-zero exit.
    fn run_git_ok(dir: &std::path::Path, args: &[&str]) -> anyhow::Result<()> {
        let dir_str = dir.to_string_lossy();
        let mut full_args: Vec<&str> = vec!["-C", &dir_str];
        full_args.extend_from_slice(args);
        let output = std::process::Command::new("git").args(&full_args).output()?;
        if !output.status.success() {
            anyhow::bail!(
                "git {:?} failed in {:?}: {}",
                args,
                dir,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    /// Write the composed guard hook at `hook_path` (inside `hooks_dir`),
    /// preserving `existing_content` (a pre-existing project pre-commit hook)
    /// by appending a sourcing line that dot-sources a sibling
    /// `pre-commit-cas-guard` file — mirrors the historical merge behavior so
    /// project hooks (e.g. a committed `.githooks/pre-commit` lint gate)
    /// still run.
    fn write_guard_alongside(
        hooks_dir: &std::path::Path,
        hook_path: &std::path::Path,
        existing_content: &str,
    ) -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let guard_path = hooks_dir.join("pre-commit-cas-guard");
        std::fs::write(&guard_path, Self::WORKER_PRE_COMMIT_HOOK)?;
        std::fs::set_permissions(&guard_path, std::fs::Permissions::from_mode(0o755))?;

        let sourcing_line = format!(
            "{}_cas_guard=\"$(git rev-parse --git-path hooks 2>/dev/null)/pre-commit-cas-guard\"\n\
             [ -f \"$_cas_guard\" ] && . \"$_cas_guard\"\n",
            Self::GUARD_SOURCING_HEADER,
        );
        let mut updated = existing_content.to_string();
        updated.push_str(&sourcing_line);
        std::fs::write(hook_path, &updated)?;
        std::fs::set_permissions(hook_path, std::fs::Permissions::from_mode(0o755))?;
        Ok(())
    }

    /// Detect and remove a legacy CAS worker guard that a pre-cas-2491 build
    /// wrote directly into the SHARED/common hooks directory.
    ///
    /// Before the worktree-scoping fix, `install_worker_pre_commit_hook`
    /// wrote unconditionally into whatever `git rev-parse --git-path hooks`
    /// reported — which, for the primary checkout, is the repo's real,
    /// permanent hooks directory. Any repo that ever ran an old `cas factory`
    /// session is left with a guard there that permanently blocks the
    /// owner's own commits on `main`/`master`, with no clean-shutdown path
    /// that would have removed it. This is a one-time migration: it runs
    /// every time a worker guard is (re)installed, targets the COMMON dir
    /// (not the worktree-private one), and is a no-op once the shared hook
    /// has been cleaned or never had a CAS guard to begin with.
    ///
    /// - If the shared hook is guard-only (no evidence of a chained project
    ///   hook), the file is deleted outright — that matches exactly what the
    ///   pre-fix installer wrote when no project hook pre-existed.
    /// - If the guard was chained onto a project hook (contains
    ///   [`Self::GUARD_SOURCING_HEADER`]), only the appended CAS block is
    ///   stripped and the sibling `pre-commit-cas-guard` file is removed;
    ///   the original project hook content is left untouched.
    fn cleanup_legacy_shared_guard(worktree_path: &std::path::Path) -> anyhow::Result<()> {
        let common_dir = Self::run_git_path(worktree_path, &["rev-parse", "--git-common-dir"])?;
        let shared_hooks_dir = common_dir.join("hooks");
        let shared_hook_path = shared_hooks_dir.join("pre-commit");

        if !shared_hook_path.exists() {
            return Ok(());
        }
        let content = match std::fs::read_to_string(&shared_hook_path) {
            Ok(c) => c,
            Err(_) => return Ok(()), // unreadable — not ours to touch
        };
        if !content.contains(Self::GUARD_MARKER) {
            return Ok(()); // not a CAS guard (or already cleaned) — leave alone
        }

        if let Some(idx) = content.find(Self::GUARD_SOURCING_HEADER) {
            // Chained onto a project hook: keep everything before our
            // appended block, drop the sibling guard file.
            let stripped = content[..idx].to_string();
            std::fs::write(&shared_hook_path, &stripped)?;
            let sibling = shared_hooks_dir.join("pre-commit-cas-guard");
            if sibling.exists() {
                let _ = std::fs::remove_file(&sibling);
            }
            tracing::info!(
                "cas-2491 migration: stripped legacy CAS guard chained onto project hook at {:?}",
                shared_hook_path
            );
        } else {
            // Guard-only file (the unconditional pre-cas-2491 case) — remove outright.
            std::fs::remove_file(&shared_hook_path)?;
            tracing::info!(
                "cas-2491 migration: removed legacy guard-only pre-commit hook at {:?}",
                shared_hook_path
            );
        }

        Ok(())
    }

    /// Install the CAS worker pre-commit guard, scoped to `worktree_path` alone.
    ///
    /// # Why not just `git rev-parse --git-path hooks` (cas-2491)
    ///
    /// Git worktrees are linked checkouts that share a single COMMON git dir —
    /// `git rev-parse --git-path hooks` resolves to that one shared hooks
    /// directory for the main checkout *and* every linked worktree alike. The
    /// original implementation installed straight into that shared path, so
    /// the guard it wrote for an isolated worker's worktree was, in fact, the
    /// *same file* backing `git commit` in the primary checkout. After the
    /// factory exited (or crashed) the guard stayed behind and silently
    /// blocked the repo owner's own commits on `main`/`master`.
    ///
    /// The fix scopes the guard using git's per-worktree config extension:
    /// each worktree gets a private hooks directory under its own
    /// (non-shared) git-dir — `$GIT_DIR/hooks-cas-guard`, where `$GIT_DIR` for
    /// a linked worktree is `<repo>/.git/worktrees/<name>` — and
    /// `core.hooksPath` is pointed at it via `git config --worktree`, which
    /// requires `extensions.worktreeConfig = true` to store the override in a
    /// worktree-private `config.worktree` file instead of the shared
    /// `.git/config`. This is scoping, not teardown: it holds even if the
    /// factory is killed with `SIGKILL` and shutdown never runs, and the
    /// override disappears automatically when the worktree itself is removed
    /// (`git worktree remove` deletes `$GIT_DIR/worktrees/<name>` outright).
    ///
    /// Idempotent: if the guard marker is already present in the private
    /// hook, no-ops (aside from re-affirming the config, which is itself
    /// idempotent). If a project-level pre-commit hook already existed at the
    /// previously-effective (shared) location, its content is preserved via
    /// [`Self::write_guard_alongside`].
    ///
    /// # Migration for already-leaked guards
    ///
    /// Scoping alone only stops *future* installs from leaking — it does
    /// nothing for a shared-hooks-dir guard a pre-fix build already wrote
    /// (which, on a real machine, silently blocks the owner's commits right
    /// now). Every call therefore starts with
    /// [`Self::cleanup_legacy_shared_guard`], which detects and removes that
    /// specific artifact before anything else runs.
    ///
    /// Non-fatal failures are logged as warnings by callers — LAYER 1
    /// (PreToolUse) and LAYER 3 (SessionStart) are the primary guards.
    pub fn install_worker_pre_commit_hook(
        worktree_path: &std::path::Path,
    ) -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        // Migration (cas-2491): remove any guard a pre-fix build left behind
        // directly in the shared/common hooks dir, BEFORE inspecting that
        // location for a "pre-existing project hook" to preserve below —
        // otherwise a legacy CAS guard would be mistaken for one and chained
        // into the new private hook instead of being cleaned up.
        Self::cleanup_legacy_shared_guard(worktree_path)?;

        // Private git-dir for this worktree: `.git` for a plain checkout, or
        // `.git/worktrees/<name>` for a linked worktree (never shared).
        let git_dir = Self::run_git_path(worktree_path, &["rev-parse", "--git-dir"])?;

        // Whatever hooks dir is *currently* effective (before we scope
        // anything) — on first install this is the shared/common dir; on
        // reinstall it's already our private dir from a prior call.
        let effective_hooks_dir = Self::run_git_path(worktree_path, &["rev-parse", "--git-path", "hooks"])?;

        let private_hooks_dir = git_dir.join("hooks-cas-guard");
        std::fs::create_dir_all(&private_hooks_dir)?;

        let hook_path = private_hooks_dir.join("pre-commit");

        if hook_path.exists()
            && std::fs::read_to_string(&hook_path)
                .unwrap_or_default()
                .contains(Self::GUARD_MARKER)
        {
            tracing::debug!("CAS pre-commit guard already installed at {:?}", hook_path);
        } else {
            let preexisting_project_hook = effective_hooks_dir.join("pre-commit");
            if preexisting_project_hook != hook_path && preexisting_project_hook.exists() {
                let existing = std::fs::read_to_string(&preexisting_project_hook)?;
                Self::write_guard_alongside(&private_hooks_dir, &hook_path, &existing)?;
                tracing::info!(
                    "Installed CAS worker pre-commit guard at {:?} (chained to existing project hook {:?})",
                    hook_path,
                    preexisting_project_hook
                );
            } else {
                std::fs::write(&hook_path, Self::WORKER_PRE_COMMIT_HOOK)?;
                std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755))?;
                tracing::info!("Installed CAS worker pre-commit guard at {:?}", hook_path);
            }
        }

        // Scope core.hooksPath to THIS worktree only, so the guard never
        // leaks into the main checkout or sibling worktrees (cas-2491).
        Self::run_git_ok(worktree_path, &["config", "extensions.worktreeConfig", "true"])?;
        Self::run_git_ok(
            worktree_path,
            &[
                "config",
                "--worktree",
                "core.hooksPath",
                &private_hooks_dir.to_string_lossy(),
            ],
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point the manager at a temp directory instead of `~/.claude/teams/...`
    /// so the test doesn't collide with real factory sessions. We keep the
    /// production constructor in place and just override the internal paths;
    /// that also exercises the real file layout the supervisor CLI sees.
    fn manager_in(tmp: &std::path::Path, name: &str) -> TeamsManager {
        let teams_dir = tmp.join(".claude").join("teams").join(name);
        let inboxes_dir = teams_dir.join("inboxes");
        TeamsManager {
            team_name: name.to_string(),
            teams_dir,
            inboxes_dir,
        }
    }

    /// `supervisor_settings_contents()` must cover every tool family whose
    /// approvals would otherwise hang on the phantom `team-lead` mailbox.
    /// Workers learned the same lesson (Read/Glob/Grep were the original
    /// screenshot's blockers), and the supervisor hits the same ops while
    /// auditing its own `.claude/settings.json`.
    #[test]
    fn supervisor_settings_contents_covers_expected_tools() {
        let body = TeamsManager::supervisor_settings_contents();
        let allow = body
            .get("permissions")
            .and_then(|p| p.get("allow"))
            .and_then(|a| a.as_array())
            .expect("permissions.allow present");
        let names: Vec<&str> = allow.iter().filter_map(|v| v.as_str()).collect();
        for tool in ["Read", "Write", "Edit", "Glob", "Grep", "Bash", "NotebookEdit"] {
            assert!(
                names.contains(&tool),
                "supervisor allowlist must include {tool}, got {names:?}"
            );
        }
    }

    /// Worker allowlist must cover the same filesystem tool families so
    /// every Write/Edit/Read/Glob/Grep/Bash op auto-approves instead of
    /// escalating to the phantom `team-lead`.
    #[test]
    fn worker_settings_contents_covers_expected_tools() {
        let body = TeamsManager::worker_settings_contents();
        let allow = body
            .get("permissions")
            .and_then(|p| p.get("allow"))
            .and_then(|a| a.as_array())
            .expect("permissions.allow present");
        let names: Vec<&str> = allow.iter().filter_map(|v| v.as_str()).collect();
        for tool in ["Read", "Write", "Edit", "Glob", "Grep", "Bash", "NotebookEdit"] {
            assert!(
                names.contains(&tool),
                "worker allowlist must include {tool}, got {names:?}"
            );
        }
        for tool in ["SendMessage", "AskUserQuestion"] {
            assert!(
                !names.contains(&tool),
                "worker allowlist must not auto-allow intercept-only tool {tool}: {names:?}"
            );
        }
    }

    /// Both per-role settings bodies must wire the factory auto-approve
    /// hooks. Without these entries, `cas hook PreToolUse` and
    /// `cas hook PermissionRequest` are never invoked and the team-mode
    /// UG9 escalation self-deadlocks on every permission gate (the bug that
    /// regressed when `715891c` stripped project-level hooks expecting them
    /// to live in per-member settings, but the per-member settings writer
    /// never had them).
    ///
    /// Emitter must produce shell-form `"command": "cas hook <event>"` so
    /// /doctor accepts the entry on every CC version. The earlier exec-form
    /// shape (cas-9a60) put `"cas"` inside `args` with no `command` field,
    /// which is malformed regardless of #58441 state.
    #[test]
    fn settings_contents_wire_factory_auto_approve_hooks() {
        for (role, body) in [
            ("supervisor", TeamsManager::supervisor_settings_contents()),
            ("worker", TeamsManager::worker_settings_contents()),
        ] {
            let hooks = body
                .get("hooks")
                .unwrap_or_else(|| panic!("{role} settings missing `hooks` block: {body}"));

            let pre = hooks
                .get("PreToolUse")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .unwrap_or_else(|| panic!("{role} hooks missing PreToolUse entry: {hooks}"));
            let pre_command = pre
                .get("hooks")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|h| h.get("command"))
                .and_then(|c| c.as_str())
                .unwrap_or_else(|| panic!("{role} PreToolUse missing shell-form command: {pre}"));
            assert_eq!(
                pre_command, "cas hook PreToolUse",
                "{role} PreToolUse must invoke `cas hook PreToolUse` via shell-form command"
            );
            let matcher = pre
                .get("matcher")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("{role} PreToolUse missing matcher: {pre}"));
            for tool in [
                "Read",
                "Write",
                "Edit",
                "Glob",
                "Grep",
                "Bash",
                "NotebookEdit",
                "SendMessage",
                "AskUserQuestion",
            ] {
                assert!(
                    matcher.contains(tool),
                    "{role} PreToolUse matcher must cover {tool}, got {matcher:?}"
                );
            }

            let perm = hooks
                .get("PermissionRequest")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .unwrap_or_else(|| panic!("{role} hooks missing PermissionRequest entry: {hooks}"));
            let perm_command = perm
                .get("hooks")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|h| h.get("command"))
                .and_then(|c| c.as_str())
                .unwrap_or_else(|| {
                    panic!("{role} PermissionRequest missing shell-form command: {perm}")
                });
            assert_eq!(
                perm_command, "cas hook PermissionRequest",
                "{role} PermissionRequest must invoke `cas hook PermissionRequest` via shell-form command"
            );
        }
    }

    /// `build_configs_for_mux` must populate `settings_path` on every worker
    /// entry (not just the supervisor). Before this fix workers got `None` and
    /// every filesystem tool call escalated to `team-lead`, a mailbox that
    /// doesn't exist, and hung forever.
    #[test]
    fn build_configs_for_mux_sets_settings_path_on_every_worker() {
        // Use unique session name so parallel test runs don't race.
        let uniq = format!(
            "worker-allowlist-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let (configs, _lead) = TeamsManager::build_configs_for_mux(
            &uniq,
            "supervisor",
            &["worker-a".to_string(), "worker-b".to_string()],
        );

        for worker in ["worker-a", "worker-b"] {
            let w = configs.get(worker).expect("worker config");
            let path = w
                .settings_path
                .as_ref()
                .unwrap_or_else(|| panic!("worker {worker} must carry settings_path"));
            assert!(
                path.ends_with(&format!("{worker}-settings.json")),
                "worker {worker} settings_path should end with {worker}-settings.json, got {path}"
            );
            assert!(
                path.contains(&uniq),
                "worker {worker} settings_path must live under session dir, got {path}"
            );
        }

        // Cleanup
        let root = TeamsManager::supervisor_settings_path_for(&uniq);
        if let Some(dir) = root.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Worker settings files must be written to disk at the moment
    /// `build_configs_for_mux` returns — before any worker PTY is spawned.
    /// A missing file at spawn time means `claude --settings <path>` silently
    /// falls back to the stock allowlist, recreating the hang.
    #[test]
    fn build_configs_for_mux_writes_worker_settings_files_eagerly() {
        let uniq = format!(
            "worker-eager-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let worker_a_path = TeamsManager::worker_settings_path_for(&uniq, "worker-a");
        let worker_b_path = TeamsManager::worker_settings_path_for(&uniq, "worker-b");
        assert!(!worker_a_path.exists(), "precondition: worker-a settings file absent");
        assert!(!worker_b_path.exists(), "precondition: worker-b settings file absent");

        let _ = TeamsManager::build_configs_for_mux(
            &uniq,
            "supervisor",
            &["worker-a".to_string(), "worker-b".to_string()],
        );

        assert!(
            worker_a_path.exists(),
            "worker-a settings must be written eagerly at {worker_a_path:?}"
        );
        assert!(
            worker_b_path.exists(),
            "worker-b settings must be written eagerly at {worker_b_path:?}"
        );

        // Cleanup
        if let Some(dir) = worker_a_path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Dynamically-spawned workers go through `spawn_config_for` instead of
    /// `build_configs_for_mux`; that path must also write + populate
    /// `settings_path` or the deadlock recurs for runtime-added workers.
    #[test]
    fn spawn_config_for_writes_worker_settings_and_populates_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let uniq = "dynamic-worker-test";
        let tm = manager_in(tmp.path(), uniq);

        let config =
            tm.spawn_config_for("late-joiner", "general-purpose", "blue", Some("lead-xyz"));

        let path = config
            .settings_path
            .as_ref()
            .expect("spawn_config_for must populate settings_path for workers");
        assert!(path.ends_with("late-joiner-settings.json"));

        let on_disk = std::path::PathBuf::from(path);
        assert!(
            on_disk.exists(),
            "spawn_config_for must eagerly write the worker settings file at {on_disk:?}"
        );
    }

    #[test]
    fn init_team_config_writes_supervisor_settings_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let tm = manager_in(tmp.path(), "deadlock-test-team");
        let (_configs, lead_session_id) =
            TeamsManager::build_configs_for_mux("deadlock-test-team", "supervisor", &[]);

        tm.init_team_config(&[], tmp.path(), &std::collections::HashMap::new(), &lead_session_id)
            .expect("init");

        let settings_path = tm.teams_dir.join("supervisor-settings.json");
        assert!(
            settings_path.exists(),
            "supervisor-settings.json should be written next to config.json"
        );

        let body = std::fs::read_to_string(&settings_path).expect("read settings");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        let allow = parsed
            .get("permissions")
            .and_then(|p| p.get("allow"))
            .and_then(|a| a.as_array())
            .expect("permissions.allow array present");

        let names: Vec<&str> = allow.iter().filter_map(|v| v.as_str()).collect();
        // Every filesystem tool family the routing deadlock is observed on.
        // The list was expanded from the original 4 (Write/Edit/Bash/
        // NotebookEdit) after cas-e15d: Read/Glob/Grep were also hanging
        // when the supervisor audited `.claude/settings.json`.
        for tool in ["Read", "Write", "Edit", "Glob", "Grep", "Bash", "NotebookEdit"] {
            assert!(
                names.contains(&tool),
                "supervisor allow must include {tool}, got {names:?}"
            );
        }
    }

    #[test]
    fn build_configs_for_mux_sets_supervisor_settings_path() {
        let uniq = format!(
            "routing-supervisor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let (configs, _lead_session_id) = TeamsManager::build_configs_for_mux(
            &uniq,
            "supervisor",
            &["worker-1".to_string(), "worker-2".to_string()],
        );

        let sup = configs.get("supervisor").expect("supervisor config");
        let path = sup
            .settings_path
            .as_ref()
            .expect("supervisor must carry a settings_path so --settings is emitted");
        assert!(
            path.ends_with("supervisor-settings.json"),
            "settings_path should point at supervisor-settings.json, got {path}"
        );
        assert!(
            path.contains(&uniq),
            "settings_path should live under the session's team dir, got {path}"
        );

        // Cleanup
        let root = TeamsManager::supervisor_settings_path_for(&uniq);
        if let Some(dir) = root.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Core invariant: the supervisor settings file must exist on disk by the
    /// time `build_configs_for_mux` returns, because that's the latest moment
    /// before the factory calls `FactoryApp::new` → `Mux::factory` and spawns
    /// the supervisor PTY with `--settings <path>`. A missing file at spawn
    /// time means `claude` silently falls back to the stock allowlist and the
    /// deadlock recurs.
    #[test]
    fn build_configs_for_mux_writes_settings_file_eagerly() {
        // Use a unique session name so parallel test runs don't race each
        // other on the same path in $HOME/.claude/teams/. The test cleans
        // up after itself at the end.
        let uniq = format!(
            "deadlock-eager-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let expected_path = TeamsManager::supervisor_settings_path_for(&uniq);
        assert!(
            !expected_path.exists(),
            "precondition: settings file must not exist before build_configs_for_mux"
        );

        let (_configs, _lead_id) =
            TeamsManager::build_configs_for_mux(&uniq, "supervisor", &[]);

        assert!(
            expected_path.exists(),
            "build_configs_for_mux must write supervisor-settings.json eagerly; \
             missing at {expected_path:?} would cause --settings to resolve to \
             nothing when the supervisor PTY spawns"
        );

        // Cleanup
        if let Some(dir) = expected_path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Cross-refresh replay: identical (from, text) writes must no-op while
    /// the original remains in the inbox. Core regression guard for
    /// cas-7f57 / cas-73c8 — workers observed "You have been assigned cas-X"
    /// messages replayed for tasks that were already Closed.
    #[test]
    fn write_to_inbox_dedups_identical_messages_within_window() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t1");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();
        mgr.ensure_inbox("swift-fox").unwrap();

        let msg = "You have been assigned cas-7f57\nTask: dup guard";
        mgr.write_to_inbox("swift-fox", DIRECTOR_AGENT_NAME, msg, None, None)
            .unwrap();
        mgr.write_to_inbox("swift-fox", DIRECTOR_AGENT_NAME, msg, None, None)
            .unwrap();
        mgr.write_to_inbox("swift-fox", DIRECTOR_AGENT_NAME, msg, None, None)
            .unwrap();

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(mgr.inboxes_dir.join("swift-fox.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            inbox.len(),
            1,
            "identical writes must dedupe down to one entry; inbox={inbox:?}"
        );

        // A genuinely different payload still gets through.
        mgr.write_to_inbox(
            "swift-fox",
            DIRECTOR_AGENT_NAME,
            "Worker is idle — pick up a task",
            None,
            None,
        )
        .unwrap();
        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(mgr.inboxes_dir.join("swift-fox.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(inbox.len(), 2, "distinct payload must not be deduped");

        // Intentional redelivery marker changes text → allowed through.
        mgr.write_to_inbox(
            "swift-fox",
            DIRECTOR_AGENT_NAME,
            "[redelivery] You have been assigned cas-7f57\nTask: dup guard",
            None,
            None,
        )
        .unwrap();
        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(mgr.inboxes_dir.join("swift-fox.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            inbox.len(),
            3,
            "redelivery-marked payload must not be content-deduped"
        );
    }

    /// Writes from different senders with the same text are independent —
    /// dedup keys on (from, text). Guards against overly aggressive
    /// collapse that would swallow legitimate cross-sender broadcasts.
    #[test]
    fn write_to_inbox_dedup_is_per_sender() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t2");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();
        mgr.ensure_inbox("swift-fox").unwrap();

        mgr.write_to_inbox("swift-fox", DIRECTOR_AGENT_NAME, "ping", None, None)
            .unwrap();
        mgr.write_to_inbox("swift-fox", "supervisor", "ping", None, None)
            .unwrap();

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(mgr.inboxes_dir.join("swift-fox.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            inbox.len(),
            2,
            "same text from different senders must both be retained; inbox={inbox:?}"
        );
    }

    // --- cas-ed6c: write_to_inbox_for_worker_idle + prune_stale_idle_alerts -

    /// A tagged write carries `retract_worker` in the persisted row so a
    /// later sweep can find it.
    #[test]
    fn write_to_inbox_for_worker_idle_tags_the_row() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t_tag");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();

        mgr.write_to_inbox_for_worker_idle(
            "supervisor",
            DIRECTOR_AGENT_NAME,
            "Worker swift-fox is idle with no assigned tasks.",
            None,
            None,
            "swift-fox",
        )
        .unwrap();

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(mgr.inboxes_dir.join("supervisor.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].retract_worker.as_deref(), Some("swift-fox"));
    }

    /// A plain (untagged) `write_to_inbox` write leaves `retract_worker`
    /// `None` — the tag is opt-in, not a default every message gets.
    #[test]
    fn write_to_inbox_plain_write_has_no_retract_worker_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t_notag");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();

        mgr.write_to_inbox("supervisor", DIRECTOR_AGENT_NAME, "hello", None, None)
            .unwrap();

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(mgr.inboxes_dir.join("supervisor.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(inbox[0].retract_worker, None);
    }

    /// The core cas-ed6c fix: a queued, unread WorkerIdle-class alert about
    /// a worker who has since gained a real assignment is retracted by
    /// `prune_stale_idle_alerts` — this is the exact live-incident shape
    /// (three workers announced idle/ready ~7 minutes after each had a
    /// genuine InProgress assignment) reproduced at the inbox layer.
    #[test]
    fn prune_stale_idle_alerts_retracts_row_for_now_assigned_worker() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t_prune");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();

        mgr.write_to_inbox_for_worker_idle(
            "supervisor",
            DIRECTOR_AGENT_NAME,
            "Worker swift-fox is idle with no assigned tasks.",
            None,
            None,
            "swift-fox",
        )
        .unwrap();

        let removed = mgr
            .prune_stale_idle_alerts("supervisor", |worker| worker == "swift-fox")
            .unwrap();
        assert_eq!(removed, 1, "the stale swift-fox alert must be retracted");

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(mgr.inboxes_dir.join("supervisor.json")).unwrap(),
        )
        .unwrap();
        assert!(
            inbox.is_empty(),
            "retracted row must be removed from the inbox file, got {inbox:?}"
        );
    }

    /// Negative control: a tagged alert for a worker who is STILL idle
    /// (predicate returns false) must survive the sweep — this must not
    /// trade false positives for silence (AC#3).
    #[test]
    fn prune_stale_idle_alerts_preserves_row_for_still_idle_worker() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t_prune_keep");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();

        mgr.write_to_inbox_for_worker_idle(
            "supervisor",
            DIRECTOR_AGENT_NAME,
            "Worker swift-fox is idle with no assigned tasks.",
            None,
            None,
            "swift-fox",
        )
        .unwrap();

        let removed = mgr
            .prune_stale_idle_alerts("supervisor", |_worker| false)
            .unwrap();
        assert_eq!(removed, 0, "a genuinely still-idle worker's alert must survive");

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(mgr.inboxes_dir.join("supervisor.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(inbox.len(), 1);
    }

    /// Untagged messages (no `retract_worker`) and messages tagged for a
    /// DIFFERENT worker must never be touched by the sweep — it only ever
    /// retracts the specific kind of row it was built for.
    #[test]
    fn prune_stale_idle_alerts_ignores_untagged_and_other_worker_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t_prune_selective");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();

        mgr.write_to_inbox("supervisor", "supervisor", "an unrelated peer message", None, None)
            .unwrap();
        mgr.write_to_inbox_for_worker_idle(
            "supervisor",
            DIRECTOR_AGENT_NAME,
            "Worker other-worker is idle with no assigned tasks.",
            None,
            None,
            "other-worker",
        )
        .unwrap();
        mgr.write_to_inbox_for_worker_idle(
            "supervisor",
            DIRECTOR_AGENT_NAME,
            "Worker swift-fox is idle with no assigned tasks.",
            None,
            None,
            "swift-fox",
        )
        .unwrap();

        // Only swift-fox is now assigned; other-worker is still idle.
        let removed = mgr
            .prune_stale_idle_alerts("supervisor", |worker| worker == "swift-fox")
            .unwrap();
        assert_eq!(removed, 1);

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(mgr.inboxes_dir.join("supervisor.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(inbox.len(), 2, "unrelated + other-worker rows must survive: {inbox:?}");
        assert!(inbox.iter().any(|m| m.text.contains("unrelated peer message")));
        assert!(inbox.iter().any(|m| m.text.contains("other-worker")));
        assert!(!inbox.iter().any(|m| m.text.contains("swift-fox")));
    }

    /// A `read: true` row must never be retracted even if it matches a
    /// stale predicate — once (hypothetically) marked read, a human may
    /// already have seen it; retracting it after the fact would be a
    /// different, worse bug than the one this fixes.
    #[test]
    fn prune_stale_idle_alerts_never_touches_read_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t_prune_read");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();

        let seeded = vec![InboxMessage {
            from: DIRECTOR_AGENT_NAME.to_string(),
            text: "Worker swift-fox is idle with no assigned tasks.".to_string(),
            summary: None,
            timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            color: "green".to_string(),
            read: true,
            retract_worker: Some("swift-fox".to_string()),
        }];
        let inbox_path = mgr.inboxes_dir.join("supervisor.json");
        std::fs::write(&inbox_path, serde_json::to_string_pretty(&seeded).unwrap()).unwrap();

        let removed = mgr
            .prune_stale_idle_alerts("supervisor", |_worker| true)
            .unwrap();
        assert_eq!(removed, 0, "a read row must never be retracted");
    }

    /// Missing inbox file: `prune_stale_idle_alerts` returns `Ok(0)`, not an
    /// error — "no inbox yet" is not a failure.
    #[test]
    fn prune_stale_idle_alerts_missing_inbox_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t_prune_missing");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();

        let removed = mgr
            .prune_stale_idle_alerts("nobody-home", |_worker| true)
            .unwrap();
        assert_eq!(removed, 0);
    }

    /// cas-73c8: identical (from, text) still present in the inbox is
    /// suppressed regardless of age. A time-bounded window re-delivered
    /// handled messages after 10+ minutes with no redelivery marker.
    #[test]
    fn write_to_inbox_dedups_identical_messages_regardless_of_age() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t_expire");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();

        // Seed inbox with a 15-minute-old identical message (formerly
        // beyond the 10-min window — must still suppress).
        let old_ts = (chrono::Utc::now() - chrono::Duration::minutes(15))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let seeded = vec![InboxMessage {
            from: DIRECTOR_AGENT_NAME.to_string(),
            text: "ping".to_string(),
            summary: Some("ping".to_string()),
            timestamp: old_ts,
            color: "green".to_string(),
            // Marked read so retention is not the reason it stays —
            // dedup alone must suppress the re-write.
            read: true,
            retract_worker: None,
        }];
        let inbox_path = mgr.inboxes_dir.join("swift-fox.json");
        std::fs::write(&inbox_path, serde_json::to_string_pretty(&seeded).unwrap())
            .unwrap();

        mgr.write_to_inbox("swift-fox", DIRECTOR_AGENT_NAME, "ping", None, None)
            .unwrap();

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(&inbox_path).unwrap(),
        )
        .unwrap();
        assert_eq!(
            inbox.len(),
            1,
            "identical content still in inbox must be suppressed even when older than 10 minutes"
        );
    }

    /// Unread messages (`read: false`) survive the retention sweep even
    /// when older than `INBOX_RETENTION`. Guards the cas-7f57 adversarial
    /// P1 finding: a supervisor recovery prompt to a wedged worker must
    /// not silently evaporate after 2h.
    #[test]
    fn write_to_inbox_retention_preserves_unread_messages() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t_unread");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();

        // Seed a 3h-old UNREAD message (beyond 2h retention). Also seed
        // a 3h-old READ message to prove the distinction.
        let stale_ts = (chrono::Utc::now() - chrono::Duration::hours(3))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let seeded = vec![
            InboxMessage {
                from: "supervisor".to_string(),
                text: "unblock yourself".to_string(),
                summary: Some("unblock yourself".to_string()),
                timestamp: stale_ts.clone(),
                color: "green".to_string(),
                read: false,
                retract_worker: None,
            },
            InboxMessage {
                from: DIRECTOR_AGENT_NAME.to_string(),
                text: "already-acked nag".to_string(),
                summary: Some("already-acked nag".to_string()),
                timestamp: stale_ts,
                color: "green".to_string(),
                read: true,
                retract_worker: None,
            },
        ];
        let inbox_path = mgr.inboxes_dir.join("swift-fox.json");
        std::fs::write(&inbox_path, serde_json::to_string_pretty(&seeded).unwrap())
            .unwrap();

        mgr.write_to_inbox("swift-fox", DIRECTOR_AGENT_NAME, "fresh", None, None)
            .unwrap();

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(&inbox_path).unwrap(),
        )
        .unwrap();
        assert_eq!(inbox.len(), 2, "inbox should retain unread + fresh, got {inbox:?}");
        assert!(
            inbox.iter().any(|m| m.text == "unblock yourself" && !m.read),
            "unread supervisor recovery message must survive retention"
        );
        assert!(
            !inbox.iter().any(|m| m.text == "already-acked nag"),
            "stale read message must still be pruned"
        );
        assert!(
            inbox.iter().any(|m| m.text == "fresh"),
            "fresh write should have landed"
        );
    }

    /// Retention sweep: messages older than `INBOX_RETENTION` are dropped
    /// on every write so the inbox file cannot grow unbounded across
    /// sessions. Simulated by seeding a message with an old timestamp and
    /// then writing a fresh one.
    #[test]
    fn write_to_inbox_prunes_messages_older_than_retention() {
        let tmp = tempfile::tempdir().unwrap();
        let mgr = manager_in(tmp.path(), "t3");
        std::fs::create_dir_all(&mgr.inboxes_dir).unwrap();

        // Seed an inbox file with a stale message (3h ago, beyond the 2h
        // retention window).
        let stale_ts = (chrono::Utc::now() - chrono::Duration::hours(3))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let seeded = vec![InboxMessage {
            from: DIRECTOR_AGENT_NAME.to_string(),
            text: "ancient history".to_string(),
            summary: Some("ancient history".to_string()),
            timestamp: stale_ts,
            color: "green".to_string(),
            // Must be marked read — unread messages are preserved
            // regardless of age by design.
            read: true,
            retract_worker: None,
        }];
        let inbox_path = mgr.inboxes_dir.join("swift-fox.json");
        std::fs::write(&inbox_path, serde_json::to_string_pretty(&seeded).unwrap()).unwrap();

        // One fresh write — the stale message should be swept on the same
        // lock pass.
        mgr.write_to_inbox("swift-fox", DIRECTOR_AGENT_NAME, "fresh", None, None)
            .unwrap();

        let inbox: Vec<InboxMessage> = serde_json::from_str(
            &std::fs::read_to_string(&inbox_path).unwrap(),
        )
        .unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].text, "fresh");
    }

    // ── Worker pre-commit hook tests (cas-bea2, LAYER 2) ─────────────────

    fn make_git_repo_for_hook_test() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.email", "t@t.com"],
            vec!["config", "user.name", "T"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(p)
                .output()
                .unwrap();
        }
        std::fs::write(p.join("f.txt"), "x").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-m", "init"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(p)
                .output()
                .unwrap();
        }
        tmp
    }

    /// `WORKER_PRE_COMMIT_HOOK` must contain the guard marker so the idempotent
    /// install check recognises an already-installed hook.
    #[test]
    fn worker_pre_commit_hook_content_has_marker() {
        assert!(
            TeamsManager::WORKER_PRE_COMMIT_HOOK.contains("CAS factory worker guard"),
            "hook content must contain the guard marker for idempotent install"
        );
    }

    /// `WORKER_PRE_COMMIT_HOOK` must start with `#!/bin/sh` (shell-form).
    /// Exec-form hooks (`#!/usr/bin/env cas`) trip /doctor on every CC version.
    #[test]
    fn worker_pre_commit_hook_is_shell_form() {
        assert!(
            TeamsManager::WORKER_PRE_COMMIT_HOOK.starts_with("#!/bin/sh"),
            "pre-commit hook must use shell-form (#!/bin/sh)"
        );
    }

    /// After installation, the pre-commit hook file must exist and be executable.
    #[test]
    fn install_worker_pre_commit_hook_creates_executable_hook() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = make_git_repo_for_hook_test();
        let p = tmp.path();

        TeamsManager::install_worker_pre_commit_hook(p)
            .expect("install should succeed");

        // git rev-parse --git-path hooks may return a relative path (.git/hooks)
        // for plain repos; resolve against p to get the absolute location.
        let output = std::process::Command::new("git")
            .args(["-C", &p.to_string_lossy(), "rev-parse", "--git-path", "hooks"])
            .output()
            .unwrap();
        let hooks_raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let hooks_dir = {
            let rp = std::path::Path::new(&hooks_raw);
            if rp.is_absolute() { rp.to_path_buf() } else { p.join(rp) }
        };
        let hook_path = hooks_dir.join("pre-commit");

        assert!(hook_path.exists(), "pre-commit hook must be created at {hook_path:?}");
        let perms = std::fs::metadata(&hook_path).unwrap().permissions();
        assert!(
            perms.mode() & 0o111 != 0,
            "pre-commit hook must be executable"
        );
    }

    /// Running `git commit` on `main` (non-factory branch) with the hook installed must exit non-zero.
    #[test]
    fn installed_hook_blocks_commit_on_non_factory_branch() {
        let tmp = make_git_repo_for_hook_test();
        let p = tmp.path();

        TeamsManager::install_worker_pre_commit_hook(p)
            .expect("install should succeed");

        // Try to commit something on main
        std::fs::write(p.join("new.txt"), "change").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(p)
            .output()
            .unwrap();

        let result = std::process::Command::new("git")
            .args(["commit", "-m", "should be blocked"])
            .current_dir(p)
            .output()
            .unwrap();

        assert!(
            !result.status.success(),
            "git commit on main should be blocked by the pre-commit hook; exit code: {:?}",
            result.status.code()
        );
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            stderr.contains("CAS COMMIT GUARD") || stderr.contains("protected branch"),
            "hook stderr should mention the guard: {stderr}"
        );
    }

    /// Running `git commit` on a factory branch must succeed (hook allows it).
    #[test]
    fn installed_hook_allows_commit_on_worker_branch() {
        let tmp = make_git_repo_for_hook_test();
        let p = tmp.path();

        // Create and switch to factory/test-worker
        std::process::Command::new("git")
            .args(["checkout", "-b", "factory/test-worker"])
            .current_dir(p)
            .output()
            .unwrap();

        TeamsManager::install_worker_pre_commit_hook(p)
            .expect("install should succeed");

        std::fs::write(p.join("wip.txt"), "work").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(p)
            .output()
            .unwrap();

        let result = std::process::Command::new("git")
            .args(["commit", "-m", "wip on worker branch"])
            .current_dir(p)
            .output()
            .unwrap();

        assert!(
            result.status.success(),
            "git commit on factory/test-worker should be allowed; stderr: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    /// Running `git commit` on an epic branch must exit non-zero (regression guard:
    /// epic/* used to bypass the denylist; the allowlist must close that gap).
    #[test]
    fn installed_hook_blocks_commit_on_epic_branch() {
        let tmp = make_git_repo_for_hook_test();
        let p = tmp.path();

        // Switch to an epic branch before installing hook
        std::process::Command::new("git")
            .args(["checkout", "-b", "epic/cas-073f"])
            .current_dir(p)
            .output()
            .unwrap();

        TeamsManager::install_worker_pre_commit_hook(p)
            .expect("install should succeed");

        std::fs::write(p.join("epic.txt"), "leak").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(p)
            .output()
            .unwrap();

        let result = std::process::Command::new("git")
            .args(["commit", "-m", "should be blocked on epic branch"])
            .current_dir(p)
            .output()
            .unwrap();

        assert!(
            !result.status.success(),
            "git commit on epic/cas-073f should be blocked; exit code: {:?}",
            result.status.code()
        );
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            stderr.contains("CAS COMMIT GUARD"),
            "hook stderr should mention the guard: {stderr}"
        );
    }

    /// Installing the hook twice must be idempotent (second install is a no-op).
    #[test]
    fn install_worker_pre_commit_hook_is_idempotent() {
        let tmp = make_git_repo_for_hook_test();
        let p = tmp.path();

        TeamsManager::install_worker_pre_commit_hook(p).expect("first install");
        TeamsManager::install_worker_pre_commit_hook(p).expect("second install must not fail");

        // Should still have exactly one copy of the guard marker
        let output = std::process::Command::new("git")
            .args(["-C", &p.to_string_lossy(), "rev-parse", "--git-path", "hooks"])
            .output()
            .unwrap();
        let hooks_raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let hooks_dir = {
            let rp = std::path::Path::new(&hooks_raw);
            if rp.is_absolute() { rp.to_path_buf() } else { p.join(rp) }
        };
        let content = std::fs::read_to_string(hooks_dir.join("pre-commit")).unwrap();
        let count = content.matches("CAS factory worker guard").count();
        assert_eq!(
            count, 1,
            "guard marker must appear exactly once after two installs; found {count} occurrences"
        );
    }

    /// When an existing pre-commit hook is present, our guard must be appended
    /// without clobbering the original.
    ///
    /// Note (cas-2491): the guard now lives in a worktree-private hooks dir
    /// scoped via `core.hooksPath`, not the original (pre-scoping) location —
    /// so this test pre-creates the "project's" hook at the location that was
    /// effective *before* install runs, then re-resolves `--git-path hooks`
    /// *after* install to find where the merged hook actually landed. The
    /// original file at its original path is left untouched (that's the
    /// point — it's a separate project hook, not overwritten in place).
    #[test]
    fn install_worker_pre_commit_hook_appends_to_existing_hook() {
        let tmp = make_git_repo_for_hook_test();
        let p = tmp.path();

        // Pre-install a custom hook at the location that's effective before
        // our install call scopes core.hooksPath elsewhere.
        let pre_output = std::process::Command::new("git")
            .args(["-C", &p.to_string_lossy(), "rev-parse", "--git-path", "hooks"])
            .output()
            .unwrap();
        let pre_hooks_raw = String::from_utf8_lossy(&pre_output.stdout).trim().to_string();
        let pre_hooks_dir = {
            let rp = std::path::Path::new(&pre_hooks_raw);
            if rp.is_absolute() { rp.to_path_buf() } else { p.join(rp) }
        };
        std::fs::create_dir_all(&pre_hooks_dir).unwrap();
        let existing_content = "#!/bin/sh\n# My existing hook\nexit 0\n";
        let original_hook_path = pre_hooks_dir.join("pre-commit");
        std::fs::write(&original_hook_path, existing_content).unwrap();

        TeamsManager::install_worker_pre_commit_hook(p).expect("install with existing hook");

        // Re-resolve --git-path hooks now that core.hooksPath is scoped to
        // our private dir; that's where the merged (original + guard) hook
        // must have landed.
        let post_output = std::process::Command::new("git")
            .args(["-C", &p.to_string_lossy(), "rev-parse", "--git-path", "hooks"])
            .output()
            .unwrap();
        let post_hooks_raw = String::from_utf8_lossy(&post_output.stdout).trim().to_string();
        let post_hooks_dir = {
            let rp = std::path::Path::new(&post_hooks_raw);
            if rp.is_absolute() { rp.to_path_buf() } else { p.join(rp) }
        };
        let effective_hook_path = post_hooks_dir.join("pre-commit");
        assert_ne!(
            effective_hook_path, original_hook_path,
            "the guard must scope to a NEW private hooks dir, not the original project one"
        );

        let final_content = std::fs::read_to_string(&effective_hook_path).unwrap();
        assert!(
            final_content.contains("My existing hook"),
            "existing hook content must be preserved in the merged hook"
        );
        assert!(
            final_content.contains("CAS factory worker guard"),
            "guard marker must be appended"
        );
    }

    /// Regression test for cas-2491: "Factory pre-commit guard is left
    /// installed in the MAIN repo and blocks the owner's own commits".
    ///
    /// Before the fix, `install_worker_pre_commit_hook` wrote into whatever
    /// `git rev-parse --git-path hooks` reported — which, for a *linked*
    /// worktree, is the single hooks dir SHARED with the main checkout (and
    /// every other worktree). Installing the guard for an isolated worker's
    /// worktree therefore also installed it for `main`, and nothing ever
    /// uninstalls it (there is no factory-shutdown teardown path for this
    /// hook at all — confirmed by repo search), so it strands the owner's
    /// repo the moment factory exits, cleanly or via `kill -9`.
    ///
    /// This test exercises a REAL linked worktree (not just a plain repo
    /// standing in for one, as the other tests above do) to prove:
    ///   AC1: installing the guard in the worker's worktree does not block a
    ///        commit on `main` in the primary checkout — with no teardown
    ///        call of any kind, simulating both a clean factory exit and an
    ///        abrupt crash (both leave the guard installed; the fix means
    ///        that no longer matters because it's scoped, not torn down).
    ///   AC2: the guard still blocks a worker committing off its
    ///        `factory/<name>` branch inside that same worker worktree.
    #[test]
    fn install_worker_pre_commit_hook_does_not_leak_into_main_checkout() {
        let tmp = make_git_repo_for_hook_test();
        let repo = tmp.path();

        // Create a linked worktree on a factory branch, exactly as the
        // daemon does for isolated workers (see app/mod.rs spawn_prep).
        let wt_path = repo
            .parent()
            .unwrap()
            .join(format!("{}-wt", repo.file_name().unwrap().to_string_lossy()));
        let add_out = std::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                "factory/test-worker",
                &wt_path.to_string_lossy(),
                "main",
            ])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            add_out.status.success(),
            "git worktree add failed: {}",
            String::from_utf8_lossy(&add_out.stderr)
        );

        TeamsManager::install_worker_pre_commit_hook(&wt_path)
            .expect("install into worker worktree should succeed");

        // AC1 — no teardown of any kind runs here (simulating both a clean
        // exit that skips cleanup and an abrupt `kill -9`). The owner's
        // commit on `main` in the PRIMARY checkout must still succeed.
        std::fs::write(repo.join("owner-change.txt"), "owner commit").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(repo)
            .output()
            .unwrap();
        let main_commit = std::process::Command::new("git")
            .args(["commit", "-m", "owner commit on main"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            main_commit.status.success(),
            "commit on main in the primary checkout must succeed after installing the worker \
             guard in a linked worktree (no shutdown/uninstall ran); stderr: {}",
            String::from_utf8_lossy(&main_commit.stderr)
        );

        // AC2 — the guard must still block a worker committing off its
        // factory/<name> branch inside the worker worktree itself.
        std::process::Command::new("git")
            .args(["checkout", "-b", "not-a-factory-branch"])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        std::fs::write(wt_path.join("wip.txt"), "off-branch change").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        let wt_commit = std::process::Command::new("git")
            .args(["commit", "-m", "should be blocked off factory branch"])
            .current_dir(&wt_path)
            .output()
            .unwrap();
        assert!(
            !wt_commit.status.success(),
            "commit off factory/<name> inside the worker worktree must still be blocked"
        );
        let stderr = String::from_utf8_lossy(&wt_commit.stderr);
        assert!(
            stderr.contains("CAS COMMIT GUARD"),
            "hook stderr should mention the guard: {stderr}"
        );

        // Best-effort cleanup so temp dirs don't linger as registered worktrees.
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force", &wt_path.to_string_lossy()])
            .current_dir(repo)
            .output();
    }

    /// Migration regression test for cas-2491 fix round 2: a repo that ran an
    /// OLD (pre-fix) `cas factory` build has a guard-only, unconditional
    /// `pre-commit` sitting in the shared/common hooks dir — exactly the
    /// artifact found on the reporting machine (dated well before this fix).
    /// Scoping alone (fix round 1) only stops *future* leaks; it does nothing
    /// for that already-installed file, so `main` stays blocked forever
    /// unless something removes it. This seeds that exact legacy artifact,
    /// then asserts that installing the guard into a (separate) worker
    /// worktree also cleans up the shared leftover, and a commit on `main`
    /// in the primary checkout succeeds.
    #[test]
    fn install_worker_pre_commit_hook_cleans_up_legacy_guard_only_hook_in_shared_dir() {
        let tmp = make_git_repo_for_hook_test();
        let repo = tmp.path();

        // Seed the exact artifact the pre-fix installer left behind: an
        // unconditional guard written straight into the shared hooks dir,
        // with no chaining (no pre-existing project hook at the time).
        let shared_hooks_dir = repo.join(".git").join("hooks");
        std::fs::create_dir_all(&shared_hooks_dir).unwrap();
        let legacy_hook_path = shared_hooks_dir.join("pre-commit");
        std::fs::write(&legacy_hook_path, TeamsManager::WORKER_PRE_COMMIT_HOOK).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&legacy_hook_path, std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        // Sanity check: before cleanup, main is indeed blocked by the legacy
        // artifact (guards against a fixture bug making this test vacuous).
        std::fs::write(repo.join("pre-fix-change.txt"), "x").unwrap();
        std::process::Command::new("git").args(["add", "."]).current_dir(repo).output().unwrap();
        let blocked = std::process::Command::new("git")
            .args(["commit", "-m", "should still be blocked before cleanup runs"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            !blocked.status.success(),
            "fixture bug: legacy guard should block main before install/cleanup runs"
        );

        // Install into a SEPARATE worker worktree — the cleanup must reach
        // into the shared dir regardless of which worktree triggered it.
        let wt_path = repo
            .parent()
            .unwrap()
            .join(format!("{}-wt", repo.file_name().unwrap().to_string_lossy()));
        let add_out = std::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                "factory/legacy-cleanup-worker",
                &wt_path.to_string_lossy(),
                "main",
            ])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(add_out.status.success(), "git worktree add failed: {}", String::from_utf8_lossy(&add_out.stderr));

        TeamsManager::install_worker_pre_commit_hook(&wt_path)
            .expect("install should succeed and clean up the legacy shared guard");

        assert!(
            !legacy_hook_path.exists(),
            "legacy guard-only hook must be removed from the shared hooks dir"
        );

        // The owner's commit on main in the primary checkout must now succeed.
        std::fs::write(repo.join("post-fix-change.txt"), "y").unwrap();
        std::process::Command::new("git").args(["add", "."]).current_dir(repo).output().unwrap();
        let main_commit = std::process::Command::new("git")
            .args(["commit", "-m", "owner commit after legacy guard cleanup"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            main_commit.status.success(),
            "commit on main must succeed once the legacy shared guard is cleaned up; stderr: {}",
            String::from_utf8_lossy(&main_commit.stderr)
        );

        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force", &wt_path.to_string_lossy()])
            .current_dir(repo)
            .output();
    }

    /// Same migration as above, but the legacy guard was chained onto a
    /// pre-existing project hook (the `write_guard_alongside` shape): the
    /// shared `pre-commit` has the project's own content followed by the CAS
    /// sourcing block, plus a sibling `pre-commit-cas-guard` file. Cleanup
    /// must strip only the CAS-appended portion and remove the sibling,
    /// leaving the project's own hook content intact.
    #[test]
    fn install_worker_pre_commit_hook_cleans_up_legacy_guard_chained_onto_project_hook() {
        let tmp = make_git_repo_for_hook_test();
        let repo = tmp.path();

        let shared_hooks_dir = repo.join(".git").join("hooks");
        std::fs::create_dir_all(&shared_hooks_dir).unwrap();

        let project_hook_content = "#!/bin/sh\n# My existing project hook\necho project-hook-ran\n";
        let sibling_guard_path = shared_hooks_dir.join("pre-commit-cas-guard");
        std::fs::write(&sibling_guard_path, TeamsManager::WORKER_PRE_COMMIT_HOOK).unwrap();

        let chained_content = format!(
            "{project_hook_content}{}_cas_guard=\"$(git rev-parse --git-path hooks 2>/dev/null)/pre-commit-cas-guard\"\n\
             [ -f \"$_cas_guard\" ] && . \"$_cas_guard\"\n",
            TeamsManager::GUARD_SOURCING_HEADER,
        );
        let legacy_hook_path = shared_hooks_dir.join("pre-commit");
        std::fs::write(&legacy_hook_path, &chained_content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&legacy_hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::set_permissions(&sibling_guard_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let wt_path = repo
            .parent()
            .unwrap()
            .join(format!("{}-wt", repo.file_name().unwrap().to_string_lossy()));
        let add_out = std::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                "factory/legacy-cleanup-chained-worker",
                &wt_path.to_string_lossy(),
                "main",
            ])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(add_out.status.success(), "git worktree add failed: {}", String::from_utf8_lossy(&add_out.stderr));

        TeamsManager::install_worker_pre_commit_hook(&wt_path)
            .expect("install should succeed and clean up the chained legacy guard");

        assert!(
            !sibling_guard_path.exists(),
            "sibling pre-commit-cas-guard file must be removed"
        );
        let remaining = std::fs::read_to_string(&legacy_hook_path).unwrap();
        assert!(
            remaining.contains("My existing project hook"),
            "project hook content must survive cleanup: {remaining:?}"
        );
        assert!(
            !remaining.contains("CAS factory worker guard"),
            "CAS guard block must be fully stripped: {remaining:?}"
        );

        // The owner's commit on main must succeed (the remaining project
        // hook here is a no-op `echo`, so nothing else blocks it).
        std::fs::write(repo.join("post-fix-change.txt"), "z").unwrap();
        std::process::Command::new("git").args(["add", "."]).current_dir(repo).output().unwrap();
        let main_commit = std::process::Command::new("git")
            .args(["commit", "-m", "owner commit after chained legacy guard cleanup"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            main_commit.status.success(),
            "commit on main must succeed once the chained legacy guard is stripped; stderr: {}",
            String::from_utf8_lossy(&main_commit.stderr)
        );

        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force", &wt_path.to_string_lossy()])
            .current_dir(repo)
            .output();
    }
}
