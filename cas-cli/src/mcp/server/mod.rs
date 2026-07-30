//! MCP Server implementation for CAS

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use rmcp::ErrorData as McpError;
use rmcp::model::{CallToolResult, Content, ErrorCode};
use rmcp::service::Peer;
use rmcp::service::RoleServer;

use crate::config::Config;
use crate::harness_policy::{verification_required_for_task_type, worker_coordination_tool};
use crate::store::{
    AgentStore, EntityStore, RuleStore, SkillStore, Store, TaskStore, VerificationStore,
    WorktreeStore, open_agent_store, open_entity_store, open_rule_store, open_skill_store,
    open_store, open_task_store, open_verification_store, open_worktree_store,
};
use crate::types::TaskStatus;
use cas_core::SearchIndex;
use cas_core::{SkillSyncer, Syncer};
use tracing::{debug, info, warn};

use crate::mcp::daemon::{ActivityTracker, EmbeddedDaemon, EmbeddedDaemonStatus};

/// Core CAS service - provides store access and helper methods
///
/// Supports two-tier storage architecture:
/// - Global store (~/.config/cas/) - user preferences, general learnings
///
/// CAS requires a project-scoped `.cas/` directory (created via `cas init`).
///
/// Store instances are cached in `OnceLock` fields so each store type is
/// opened exactly once per MCP server lifetime, eliminating repeated
/// connection opens on every tool call.
#[derive(Clone)]
pub struct CasCore {
    /// Project CAS directory (./.cas/)
    pub(crate) cas_root: PathBuf,
    /// Activity tracker for idle detection
    pub(crate) activity: Option<Arc<ActivityTracker>>,
    /// Reference to the embedded daemon (if running)
    pub(crate) daemon: Option<Arc<EmbeddedDaemon>>,
    /// Agent ID for multi-agent coordination (lazily initialized on first tool call)
    pub(crate) agent_id: OnceLock<Option<String>>,
    /// Peer reference for sending MCP notifications (Claude Code 2.1.0+)
    /// Captured on first request, used to notify client of resource changes
    pub(crate) peer: Arc<RwLock<Option<Peer<RoleServer>>>>,
    // Cached store instances (lazily initialized, one per store type)
    pub(crate) cached_store: OnceLock<Arc<dyn Store>>,
    pub(crate) cached_rule_store: OnceLock<Arc<dyn RuleStore>>,
    pub(crate) cached_task_store: OnceLock<Arc<dyn TaskStore>>,
    pub(crate) cached_skill_store: OnceLock<Arc<dyn SkillStore>>,
    pub(crate) cached_entity_store: OnceLock<Arc<dyn EntityStore>>,
    pub(crate) cached_agent_store: OnceLock<Arc<dyn AgentStore>>,
    pub(crate) cached_verification_store: OnceLock<Arc<dyn VerificationStore>>,
    pub(crate) cached_worktree_store: OnceLock<Arc<dyn WorktreeStore>>,
    /// Cached search index (lazily initialized, opened once per server lifetime)
    pub(crate) cached_search_index: OnceLock<SearchIndex>,
    /// Cached config (lazily initialized, loaded once per server lifetime)
    pub(crate) cached_config: OnceLock<Config>,
}

impl CasCore {
    /// Helper: get cached store or initialize it.
    /// Safe for concurrent access — if two threads race, one wins and the other
    /// gets the canonical instance from `get()`.
    fn cached_or_init<T: Clone>(
        cell: &OnceLock<T>,
        init: impl FnOnce() -> Result<T, McpError>,
    ) -> Result<T, McpError> {
        if let Some(val) = cell.get() {
            return Ok(val.clone());
        }
        let val = init()?;
        let _ = cell.set(val);
        Ok(cell.get().unwrap().clone())
    }

    /// Get store (cached — opened once per server lifetime)
    pub(crate) fn open_store(&self) -> Result<Arc<dyn Store>, McpError> {
        Self::cached_or_init(&self.cached_store, || {
            open_store(&self.cas_root).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to open store: {e}")),
                data: None,
            })
        })
    }

    /// Get rule store (cached)
    pub(crate) fn open_rule_store(&self) -> Result<Arc<dyn RuleStore>, McpError> {
        Self::cached_or_init(&self.cached_rule_store, || {
            open_rule_store(&self.cas_root).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to open rule store: {e}")),
                data: None,
            })
        })
    }

    /// Get task store (cached)
    pub(crate) fn open_task_store(&self) -> Result<Arc<dyn TaskStore>, McpError> {
        Self::cached_or_init(&self.cached_task_store, || {
            open_task_store(&self.cas_root).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to open task store: {e}")),
                data: None,
            })
        })
    }

    /// Get skill store (cached)
    pub(crate) fn open_skill_store(&self) -> Result<Arc<dyn SkillStore>, McpError> {
        Self::cached_or_init(&self.cached_skill_store, || {
            open_skill_store(&self.cas_root).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to open skill store: {e}")),
                data: None,
            })
        })
    }

    /// Get entity store (cached)
    pub(crate) fn open_entity_store(&self) -> Result<Arc<dyn EntityStore>, McpError> {
        Self::cached_or_init(&self.cached_entity_store, || {
            open_entity_store(&self.cas_root).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to open entity store: {e}")),
                data: None,
            })
        })
    }

    /// Get agent store (cached)
    pub(crate) fn open_agent_store(&self) -> Result<Arc<dyn AgentStore>, McpError> {
        Self::cached_or_init(&self.cached_agent_store, || {
            open_agent_store(&self.cas_root).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to open agent store: {e}")),
                data: None,
            })
        })
    }

    /// Get verification store (cached)
    pub(crate) fn open_verification_store(&self) -> Result<Arc<dyn VerificationStore>, McpError> {
        Self::cached_or_init(&self.cached_verification_store, || {
            open_verification_store(&self.cas_root).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to open verification store: {e}")),
                data: None,
            })
        })
    }

    /// Get worktree store (cached)
    pub(crate) fn open_worktree_store(&self) -> Result<Arc<dyn WorktreeStore>, McpError> {
        Self::cached_or_init(&self.cached_worktree_store, || {
            open_worktree_store(&self.cas_root).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to open worktree store: {e}")),
                data: None,
            })
        })
    }

    /// Get worktree manager (for workspace lifecycle operations)
    pub(crate) fn worktree_manager(&self) -> Option<crate::worktree::WorktreeManager> {
        use crate::worktree::{WorktreeConfig, WorktreeManager};

        let config = self.load_config();
        let worktrees_config = config.worktrees();

        // Only create manager if worktrees are enabled
        if !worktrees_config.enabled {
            return None;
        }

        let wt_config = WorktreeConfig {
            enabled: worktrees_config.enabled,
            base_path: worktrees_config.base_path,
            branch_prefix: worktrees_config.branch_prefix,
            auto_merge: worktrees_config.auto_merge,
            cleanup_on_close: worktrees_config.cleanup_on_close,
            promote_entries_on_merge: worktrees_config.promote_entries_on_merge,
        };

        // Try to create the manager (will fail if not in a git repo)
        // Note: cas_root is .cas directory, but WorktreeManager needs the project root
        let project_root = self.cas_root.parent().unwrap_or(&self.cas_root);
        WorktreeManager::new(project_root, wt_config).ok()
    }

    /// Detect current worktree branch for scoping entries
    ///
    /// Returns Some(branch) if:
    /// 1. Worktrees are enabled in config
    /// 2. We're currently in a CAS-managed git worktree
    ///
    /// This is used to auto-set the branch field on new entries for virtual isolation.
    pub(crate) fn current_worktree_branch(&self) -> Option<String> {
        use crate::worktree::GitOperations;

        let config = self.load_config();
        let worktrees_config = config.worktrees();

        // Only scope entries if worktrees are enabled
        if !worktrees_config.enabled {
            return None;
        }

        // Get git context from current working directory
        let cwd = std::env::current_dir().ok()?;
        let git_context = GitOperations::get_context(&cwd).ok()?;

        // Only scope if we're in a worktree (not the main checkout)
        if !git_context.is_worktree {
            return None;
        }

        // Return the branch name
        git_context.branch
    }

    /// Get search index (cached — opened once per server lifetime)
    pub(crate) fn open_search_index(&self) -> Result<SearchIndex, McpError> {
        if let Some(idx) = self.cached_search_index.get() {
            return Ok(idx.clone());
        }
        let index_dir = self.cas_root.join("index/tantivy");
        let idx = SearchIndex::open(&index_dir).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to open search index: {e}")),
            data: None,
        })?;
        let _ = self.cached_search_index.set(idx);
        Ok(self.cached_search_index.get().unwrap().clone())
    }

    /// Create success result with text content
    pub(crate) fn success(text: impl Into<String>) -> CallToolResult {
        CallToolResult::success(vec![Content::text(text.into())])
    }

    /// Create tool error result (tool succeeded but operation failed)
    /// This sets is_error: true so Claude knows to handle the failure
    pub(crate) fn tool_error(text: impl Into<String>) -> CallToolResult {
        CallToolResult::error(vec![Content::text(text.into())])
    }

    /// Create error result
    pub(crate) fn error(code: ErrorCode, message: impl Into<String>) -> McpError {
        McpError {
            code,
            message: Cow::from(message.into()),
            data: None,
        }
    }

    /// Record activity (for idle detection)
    pub(crate) fn touch(&self) {
        if let Some(activity) = &self.activity {
            activity.touch();
        }
    }

    /// Notify client that resource list has changed (Claude Code 2.1.0+)
    ///
    /// Call this after any state-modifying operation (create, update, delete)
    /// so Claude Code can refresh its resource list.
    pub(crate) async fn notify_resources_changed(&self) {
        // Clone peer outside of lock to avoid holding guard across await
        let peer = {
            if let Ok(peer_guard) = self.peer.read() {
                peer_guard.clone()
            } else {
                None
            }
        };

        if let Some(peer) = peer {
            // Fire-and-forget - don't block on notification result
            let _ = peer.notify_resource_list_changed().await;
        }
    }

    /// Get daemon status
    pub(crate) async fn daemon_status(&self) -> Option<EmbeddedDaemonStatus> {
        if let Some(daemon) = &self.daemon {
            Some(daemon.status().await)
        } else {
            None
        }
    }

    /// Trigger immediate maintenance
    pub(crate) async fn trigger_maintenance(&self) -> Result<String, McpError> {
        if let Some(daemon) = &self.daemon {
            let result = daemon.trigger_maintenance().await.map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Maintenance failed: {e}")),
                data: None,
            })?;
            Ok(format!(
                "Maintenance completed in {:.2}s:\n- Observations: {}\n- Decay applied: {}",
                result.duration_secs, result.observations_processed, result.decay_applied
            ))
        } else {
            Err(McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from("Daemon not running"),
                data: None,
            })
        }
    }

    /// Load and return config (cached — loaded once per server lifetime)
    pub(crate) fn load_config(&self) -> Config {
        if let Some(cfg) = self.cached_config.get() {
            return cfg.clone();
        }
        let cfg = Config::load(&self.cas_root).unwrap_or_default();
        let _ = self.cached_config.set(cfg);
        self.cached_config.get().unwrap().clone()
    }

    /// Get the registered agent ID, auto-registering if a session file exists
    ///
    /// This method implements lazy auto-registration with auto-revival:
    /// 1. If already registered, check if agent is active and revive if needed
    /// 2. If not registered, try to read session_id from PPID-keyed file (written by SessionStart hook)
    /// 3. If session file missing, try PPID fallback to find existing agent
    /// 4. Auto-register with that session_id
    ///
    /// This ensures agents are always registered and active without requiring explicit registration calls.
    pub(crate) fn get_agent_id(&self) -> Result<String, McpError> {
        // Fast path: already registered - check status and revive if needed
        if let Some(Some(id)) = self.agent_id.get() {
            debug!(agent_id = %id, "Using cached agent id");
            self.ensure_agent_active(id)?;
            return Ok(id.clone());
        }

        // Prefer explicit session id override when present (used by native extensions).
        if let Ok(session_id) = std::env::var("CAS_SESSION_ID") {
            let session_id = session_id.trim().to_string();
            if !session_id.is_empty() {
                let agent_name =
                    std::env::var("CAS_AGENT_NAME").unwrap_or_else(|_| "Primary (env)".to_string());
                info!(
                    session_id = %session_id,
                    agent_name = %agent_name,
                    "Auto-registering agent from CAS_SESSION_ID"
                );
                self.register_agent(session_id.clone(), agent_name, None)?;
                return Ok(session_id);
            }
        }

        // Try to auto-register from PPID-keyed session file
        match crate::agent_id::read_session_for_mcp(&self.cas_root) {
            Ok(session_id) if !session_id.is_empty() => {
                // Auto-register with discovered session_id
                // Use CAS_AGENT_NAME env var if set (from cas start), otherwise default
                let agent_name = std::env::var("CAS_AGENT_NAME")
                    .unwrap_or_else(|_| "Primary (auto)".to_string());
                info!(
                    session_id = %session_id,
                    agent_name = %agent_name,
                    "Auto-registering agent from session mapping"
                );
                self.register_agent(session_id.clone(), agent_name, None)?;
                Ok(session_id)
            }
            Ok(_) => Err(McpError {
                code: ErrorCode::INVALID_REQUEST,
                message: Cow::from(
                    "Session file is empty. SessionStart hook may not have run correctly.",
                ),
                data: None,
            }),
            Err(e) => {
                // Session file missing - try PPID fallback to find existing agent
                let cc_pid = crate::agent_id::get_cc_pid_for_mcp();
                warn!(
                    cc_pid = cc_pid,
                    error = %e,
                    "Session mapping missing for MCP; trying PPID fallback"
                );
                let agent_store = self.open_agent_store()?;

                if let Ok(Some(agent)) = agent_store.get_by_cc_pid(cc_pid) {
                    info!(
                        cc_pid = cc_pid,
                        agent_id = %agent.id,
                        "Found agent by PPID fallback"
                    );
                    let _ = self.agent_id.set(Some(agent.id.clone()));
                    self.ensure_agent_active(&agent.id)?;
                    return Ok(agent.id);
                }

                Err(McpError {
                    code: ErrorCode::INVALID_REQUEST,
                    message: Cow::from(format!(
                        "Agent not registered. The SessionStart hook may not have run yet. \
                         Register manually with: `mcp__cas__agent` action: session_start, session_id: <your-session-id>. \
                         Original error: {e}"
                    )),
                    data: None,
                })
            }
        }
    }

    /// Ensure agent is active, reviving if necessary
    ///
    /// Called from get_agent_id() to auto-revive stale/shutdown agents on MCP tool use.
    pub(crate) fn ensure_agent_active(&self, agent_id: &str) -> Result<(), McpError> {
        let agent_store = self.open_agent_store()?;

        match agent_store.get(agent_id) {
            Ok(agent) if agent.is_alive() => Ok(()), // Already active
            Ok(_agent) => {
                // Agent exists but is stale/shutdown - revive it
                info!(agent_id = %agent_id, "Reviving agent");
                agent_store.revive(agent_id).map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to revive agent: {e}")),
                    data: None,
                })?;

                // Resume heartbeats
                if let Some(ref daemon) = self.daemon {
                    let id_clone = agent_id.to_string();
                    let daemon_clone = Arc::clone(daemon);
                    tokio::spawn(async move {
                        daemon_clone.set_agent_id(id_clone).await;
                    });
                }

                Ok(())
            }
            Err(_) => {
                // Agent doesn't exist - re-register it
                warn!(agent_id = %agent_id, "Re-registering missing agent");
                let mut agent = crate::types::Agent::new(
                    agent_id.to_string(),
                    "Primary (re-registered)".to_string(),
                );
                let our_pid = std::process::id();
                agent.pid = Some(our_pid);
                // PID-reuse fingerprint (cas-ea46): see daemon::stamp_pid_fingerprint.
                crate::mcp::daemon::stamp_pid_fingerprint(&mut agent, our_pid);
                #[cfg(unix)]
                {
                    agent.ppid = Some(std::os::unix::process::parent_id());
                }
                agent.machine_id = Some(crate::types::Agent::get_or_generate_machine_id());

                // Set role from CAS_AGENT_ROLE env var (set by factory mode)
                if let Ok(role_str) = std::env::var("CAS_AGENT_ROLE") {
                    if let Ok(role) = role_str.parse::<crate::types::AgentRole>() {
                        agent.role = role;
                    }
                }
                if agent.role == crate::types::AgentRole::Worker {
                    agent.agent_type = crate::types::AgentType::Worker;
                }

                crate::mcp::daemon::apply_factory_worker_metadata(&mut agent, None);

                agent_store.register(&agent).map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to re-register agent: {e}")),
                    data: None,
                })?;

                // Start heartbeats
                if let Some(ref daemon) = self.daemon {
                    let id_clone = agent_id.to_string();
                    let daemon_clone = Arc::clone(daemon);
                    tokio::spawn(async move {
                        daemon_clone.set_agent_id(id_clone).await;
                    });
                }

                Ok(())
            }
        }
    }

    /// Register an agent with session_id as the canonical identifier
    ///
    /// This must be called before other CAS tools can be used.
    /// The session_id becomes the agent's unique identifier.
    ///
    /// The agent's role is determined from the CAS_AGENT_ROLE environment variable
    /// (set by factory mode when spawning workers/supervisors).
    pub(crate) fn register_agent(
        &self,
        session_id: String,
        name: String,
        parent_id: Option<String>,
    ) -> Result<String, McpError> {
        self.register_agent_with_hints(session_id, name, parent_id, None, None)
    }

    pub(crate) fn register_agent_with_hints(
        &self,
        session_id: String,
        name: String,
        parent_id: Option<String>,
        agent_type_hint: Option<crate::types::AgentType>,
        role_hint: Option<crate::types::AgentRole>,
    ) -> Result<String, McpError> {
        // Set the agent_id in OnceLock (session_id is the canonical ID)
        let _ = self.agent_id.set(Some(session_id.clone()));

        let pid = std::process::id();
        let agent_store = self.open_agent_store()?;

        // Create and register the agent
        let mut agent = if let Some(parent) = parent_id {
            crate::types::Agent::new_sub_agent(session_id.clone(), name, parent)
        } else {
            crate::types::Agent::new(session_id.clone(), name)
        };

        if let Some(agent_type) = agent_type_hint {
            agent.agent_type = agent_type;
        }

        agent.pid = Some(pid);
        // PID-reuse fingerprint (cas-ea46): see daemon::stamp_pid_fingerprint.
        crate::mcp::daemon::stamp_pid_fingerprint(&mut agent, pid);
        #[cfg(unix)]
        {
            agent.ppid = Some(std::os::unix::process::parent_id());
        }
        agent.machine_id = Some(crate::types::Agent::get_or_generate_machine_id());

        // Set role from CAS_AGENT_ROLE env var (set by factory mode)
        if let Some(role) = role_hint {
            agent.role = role;
        } else if let Ok(role_str) = std::env::var("CAS_AGENT_ROLE") {
            if let Ok(role) = role_str.parse::<crate::types::AgentRole>() {
                agent.role = role;
            }
        } else if agent.agent_type == crate::types::AgentType::Worker {
            // Fallback: worker type implies worker role if env is unavailable.
            agent.role = crate::types::AgentRole::Worker;
        }

        // If type hint was not provided, infer agent_type from resolved role.
        if agent_type_hint.is_none() {
            match agent.role {
                crate::types::AgentRole::Worker => {
                    agent.agent_type = crate::types::AgentType::Worker;
                }
                crate::types::AgentRole::Supervisor
                | crate::types::AgentRole::Director
                | crate::types::AgentRole::Standard => {}
            }
        }

        crate::mcp::daemon::apply_factory_worker_metadata(&mut agent, None);

        agent_store.register(&agent).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to register agent: {e}")),
            data: None,
        })?;

        info!(
            agent_id = %session_id,
            agent_name = %agent.name,
            pid = ?agent.pid,
            ppid = ?agent.ppid,
            cc_session_id = ?agent.cc_session_id,
            parent_id = ?agent.parent_id,
            machine_id = ?agent.machine_id,
            role = %agent.role,
            agent_type = %agent.agent_type,
            "Agent registered"
        );

        // Tell the daemon to send heartbeats for this agent
        // This keeps the agent alive and prevents it from being marked as dead
        if let Some(ref daemon) = self.daemon {
            let session_id_clone = session_id.clone();
            let daemon_clone = Arc::clone(daemon);
            tokio::spawn(async move {
                daemon_clone.set_agent_id(session_id_clone).await;
            });
        }

        Ok(session_id)
    }

    /// Check if a tool action is authorized for the current agent.
    ///
    /// Enforces verification jail: when an agent has pending_verification=true
    /// on any leased task, mutating operations are blocked.
    ///
    /// Policy:
    /// - Non-mutating operations: always allowed
    /// - Verification tool: always allowed (escape hatch from jail)
    /// - Agent tool: always allowed (needed for communication/coordination)
    /// - System/factory/team/worktree tools: always allowed (infrastructure)
    /// - Task notes: allowed even when jailed (progress reporting)
    /// - All other mutating operations: blocked when jailed
    ///
    /// Returns Ok(()) if allowed, or Err with VERIFICATION_JAIL_BLOCKED code if blocked.
    ///
    /// `close_task_id`: when `Some(id)`, scopes the verification-jail check to
    /// only the task being closed. Unrelated tasks held by the same agent will
    /// not block this close (cas-a3ca). Pass `None` for all other mutations —
    /// those still enforce the full any-unverified-task jail.
    pub(crate) fn authorize_agent_action(
        &self,
        tool: &str,
        action: &str,
        is_mutating: bool,
        close_task_id: Option<&str>,
    ) -> Result<(), McpError> {
        if !is_mutating {
            return Ok(());
        }

        // Infrastructure/coordination tools are exempt from jail
        match tool {
            "verification" | "agent" | "system" | "factory" | "team" | "worktree" => return Ok(()),
            _ => {}
        }

        // Task notes and release are allowed in jail
        // - notes: progress/blocker reporting
        // - release: escape hatch to release an accidentally claimed task
        if tool == "task" && (action == "notes" || action == "release") {
            return Ok(());
        }

        // Supervisors are exempt from verification jail — their job is coordination
        // (assigning tasks, creating tasks, managing deps) which must not be blocked
        if crate::harness_policy::is_supervisor_from_env() {
            return Ok(());
        }

        // Factory workers are exempt for most mutations — they may have
        // multiple tasks and must continue working while one awaits
        // verification. However, `task.close` itself is NOT exempt: that's
        // the one call where the jail must still fire, because close is
        // what triggers verifier dispatch. Exempting close here was the
        // bba6fbf regression that broke dispatch for factory workers
        // entirely — close_ops.rs emits instructional text but has no
        // forcing function, so the verifier subagent never gets spawned.
        // Narrowing the exemption restores the lever for the one action
        // that needs it while preserving the mutation-cascade fix that
        // bba6fbf correctly addressed.
        let is_factory_worker = std::env::var("CAS_AGENT_ROLE")
            .map(|r| r.eq_ignore_ascii_case("worker"))
            .unwrap_or(false)
            && std::env::var("CAS_FACTORY_MODE").is_ok();
        if is_factory_worker && !(tool == "task" && action == "close") {
            return Ok(());
        }

        // cas-8edb: under `[code_review] owner = "supervisor"` (the default
        // since cas-865b / v2.13.0), the worker's `task.close` is a pure
        // transition operation — it routes the task to
        // `PendingSupervisorReview` and the supervisor owns review at
        // cherry-pick. The legacy verification jail was the lever that
        // forced workers to dispatch `task-verifier` themselves; under the
        // new ownership model that lever has no target to act on. Jailing
        // a worker here just deadlocks every clean close because workers
        // do not submit a `ReviewOutcome` envelope under owner=supervisor,
        // so close_ops's self-cert path cannot fire either.
        //
        // The downstream gate (close_ops::cas_task_close) still enforces
        // every other invariant the close must satisfy — lightweight
        // structural lint, factory-branch merge-state, epic subtask
        // receipts, additive-only enforcement — so this exemption only
        // bypasses the verification-jail lever, not the rest of the close
        // pipeline.
        if is_factory_worker && tool == "task" && action == "close" {
            let cr_config = self.load_config();
            let supervisor_owned = cr_config
                .code_review
                .as_ref()
                .map(|cr| cr.supervisor_owned())
                .unwrap_or_else(|| crate::config::CodeReviewConfig::default().supervisor_owned());
            if supervisor_owned {
                return Ok(());
            }
        }

        // Check if agent is jailed
        let agent_id = match self.get_agent_id() {
            Ok(id) => id,
            Err(_) => return Ok(()), // No agent registered = no jail enforcement
        };

        match self.check_pending_verification(&agent_id, close_task_id)? {
            Some((task_id, task_title)) => {
                // cas-778a: factory workers cannot spawn task-verifier themselves
                // (it's an internal agent). Give them the correct escalation path
                // (forward to supervisor) instead of an impossible instruction.
                // Non-worker callers (supervisors, non-factory contexts) retain
                // the existing Task() spawn suggestion which is correct for them.
                //
                // cas-8aaf: the coordination tool alias differs by harness.
                // Claude workers use mcp__cas__coordination, Codex workers use
                // mcp__cs__coordination. worker_coordination_tool() reads
                // CAS_FACTORY_WORKER_CLI (injected by the PTY spawn) to select
                // the right alias so the suggestion is executable by this worker.
                let guidance = if is_factory_worker {
                    let coord_tool = worker_coordination_tool();
                    format!(
                        "Message supervisor via: \
                         {coord_tool} action=message target=supervisor \
                         summary=\"Ready to close {task_id}\" \
                         message=\"Task {task_id} requires verification before close. \
                         Please run verification (task-verifier for task {task_id}) \
                         and close on my behalf if approved.\""
                    )
                } else {
                    format!(
                        "Use the Task tool to spawn a task-verifier subagent: \
                         Task(subagent_type=\"task-verifier\", prompt=\"Verify task {task_id}\")."
                    )
                };
                Err(McpError {
                    code: ErrorCode::INVALID_REQUEST,
                    message: Cow::from(format!(
                        "VERIFICATION_JAIL_BLOCKED: Mutating operation {tool}.{action} blocked. \
                         Task {task_id} ({task_title}) requires verification before any mutations \
                         are allowed. {guidance}"
                    )),
                    data: None,
                })
            }
            None => Ok(()),
        }
    }

    /// Check if an agent has any tasks with pending verification.
    ///
    /// Returns `Some((task_id, task_title))` if the agent has an in-progress task
    /// that requires verification but hasn't been verified yet.
    /// Returns `None` if the agent can proceed.
    ///
    /// `close_task_id`: when `Some(id)`, only the named task is evaluated —
    /// unrelated tasks leased by the same agent are skipped. This scopes the
    /// jail check for `task.close id=X` to X itself so that a different
    /// in-progress task (unverified) cannot block closing an already-verified
    /// task (cas-a3ca).
    pub(crate) fn check_pending_verification(
        &self,
        agent_id: &str,
        close_task_id: Option<&str>,
    ) -> Result<Option<(String, String)>, McpError> {
        let config = self.load_config();

        if !config.verification_enabled() {
            return Ok(None);
        }

        let agent_store = self.open_agent_store()?;
        let task_store = self.open_task_store()?;

        // Get agent's active leases
        let leases = agent_store
            .list_agent_leases(agent_id)
            .map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to list agent leases: {e}")),
                data: None,
            })?;

        // Check each leased task
        for lease in leases {
            // Only check active leases
            if lease.status != crate::types::LeaseStatus::Active {
                continue;
            }

            // cas-a3ca: when called for a specific task.close, only evaluate
            // the task being closed. An unrelated in-progress task (e.g. task B
            // started after task A was verified) must not block closing task A.
            // The jail scopes to the requested task; other tasks' verification
            // state is irrelevant to whether THIS task can be closed.
            if let Some(target_id) = close_task_id {
                if lease.task_id != target_id {
                    continue;
                }
            }

            // Get the task
            let task = match task_store.get(&lease.task_id) {
                Ok(t) => t,
                Err(_) => continue, // Task may have been deleted
            };

            // Only check in-progress tasks (not open or closed)
            if task.status != TaskStatus::InProgress {
                continue;
            }

            // cas-6a99: a task awaiting WORKTREE MERGE (its close was merge-gated
            // because the branch isn't merged yet) is "work complete, awaiting
            // supervisor merge" — NOT actively-verifying. The worker cannot resolve
            // it (the merge is the supervisor's job), so it must not jail them from
            // starting unrelated/bundled work. This is distinct from the verification
            // jail (`pending_verification`), which still blocks correctly below.
            if task.pending_worktree_merge {
                continue;
            }

            // cas-33eb: a depth=light task never jails the agent — skipping the
            // verification machinery is the whole point of "speed mode" (mirrors
            // the close_ops.rs `depth_light` short-circuit from cas-6538).
            // Without this, a SOLO (non-factory) user who starts then closes a
            // light task still hits VERIFICATION_JAIL_BLOCKED at this dispatch
            // layer, because the factory-worker / supervisor exemptions in
            // `authorize_agent_action` do not apply to them.
            if task.depth == crate::types::TaskDepth::Light {
                continue;
            }

            // Skip task types where current harness policy bypasses verification.
            if !verification_required_for_task_type(task.task_type) {
                continue;
            }

            // Check if task has approved verification
            if let Ok(verification_store) = self.open_verification_store() {
                match verification_store.get_latest_for_task(&task.id) {
                    Ok(Some(v))
                        if v.status == crate::types::VerificationStatus::Approved
                            || v.status == crate::types::VerificationStatus::Skipped =>
                    {
                        // Approved or explicitly skipped (supervisor bypass
                        // during orphaned-task close) — not jailing. See
                        // cas-82d6: close_ops writes a Skipped row when
                        // closing via the assignee-inactive bypass so that
                        // downstream workers aren't trapped by a task that
                        // has no verification record at all.
                        continue;
                    }
                    Ok(_) | Err(_) => {
                        // No verification or not approved - this task is blocking
                        return Ok(Some((task.id.clone(), task.title.clone())));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Auto-claim a task when verification is required/failed
    /// This ensures the Stop hook can block exit for unverified tasks
    pub(crate) fn auto_claim_for_verification(
        &self,
        task_id: &str,
        task_store: &dyn TaskStore,
    ) -> Result<(), McpError> {
        // Get or register agent
        let agent_id = self.get_agent_id()?;
        let agent_store = self.open_agent_store()?;
        let config = self.load_config();
        let lease_duration = (config.lease().default_duration_mins as i64) * 60;

        // Try to claim - ignore if already claimed by us or others
        match agent_store.try_claim(
            task_id,
            &agent_id,
            lease_duration,
            Some("Verification pending"),
        ) {
            Ok(crate::types::ClaimResult::Success(_))
            | Ok(crate::types::ClaimResult::AlreadyClaimed { .. }) => {
                // Claimed successfully or already claimed - ensure task is in_progress
                // so check_pending_verification finds it
                if let Ok(mut task) = task_store.get(task_id) {
                    if task.status == crate::types::TaskStatus::Open {
                        task.status = crate::types::TaskStatus::InProgress;
                        let _ = task_store.update(&task);
                    }
                }
            }
            Ok(_) | Err(_) => {
                // Claim failed for other reasons - log but continue
                // The tool_error response will still signal the issue to Claude
            }
        }

        Ok(())
    }

    /// Sync rules to Claude Code
    pub(crate) fn sync_rules(&self) -> Result<usize, McpError> {
        let config = self.load_config();
        let project_root = self.cas_root.parent().unwrap_or(&self.cas_root);
        let syncer = Syncer::new(
            project_root.join(&config.sync.target),
            config.sync.min_helpful,
        );

        let rule_store = self.open_rule_store()?;
        let rules = rule_store.list().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list rules: {e}"),
            )
        })?;

        let report = syncer.sync_all(&rules).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to sync rules: {e}"),
            )
        })?;

        Ok(report.synced)
    }

    /// Sync skills to Claude Code
    pub(crate) fn sync_skills(&self) -> Result<usize, McpError> {
        let project_root = self.cas_root.parent().unwrap_or(&self.cas_root);
        let syncer = SkillSyncer::with_defaults(project_root);

        let skill_store = self.open_skill_store()?;
        let skills = skill_store.list(None).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list skills: {e}"),
            )
        })?;

        let report = syncer.sync_all(&skills).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to sync skills: {e}"),
            )
        })?;

        Ok(report.synced)
    }

    /// Promote entries from a specific branch to parent scope (clear branch field)
    ///
    /// Used when a worktree is merged - entries created in that worktree
    /// become visible in the parent context.
    pub(crate) fn promote_branch_entries(&self, branch: &str) -> Result<usize, McpError> {
        let store = self.open_store()?;
        let entries = store.list_by_branch(branch).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to list entries for branch: {e}"),
            )
        })?;

        let mut promoted = 0;
        for mut entry in entries {
            entry.branch = None; // Promote to parent scope
            if store.update(&entry).is_ok() {
                promoted += 1;
            }
        }

        Ok(promoted)
    }
}

mod prompts;
mod resources;
mod runtime;

pub use runtime::run_server;
#[cfg(feature = "mcp-proxy")]
pub use runtime::{write_proxy_catalog_cache, write_proxy_health_cache};
