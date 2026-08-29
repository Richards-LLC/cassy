//! MCP Server implementation for Cassy

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use rmcp::ErrorData as McpError;
use rmcp::model::{CallToolResult, Content, ErrorCode};
use rmcp::service::Peer;
use rmcp::service::RoleServer;

use crate::config::Config;
use crate::store::{
    AgentStore, EntityStore, RuleStore, SkillStore, Store, TaskStore, VerificationStore,
    WorktreeStore, open_agent_store, open_entity_store, open_rule_store, open_skill_store,
    open_store, open_task_store, open_verification_store, open_worktree_store,
};
use cas_core::{SkillSyncer, Syncer};
use tracing::{debug, info, warn};

use crate::hybrid_search::SearchIndex;

use crate::mcp::daemon::{ActivityTracker, EmbeddedDaemon, EmbeddedDaemonStatus};

/// Provenance of the current in-process agent identity.
///
/// This is deliberately not deserializable and never derived from request or
/// environment fields. Privileged workflow authority may rely on
/// `ServerInternal`, but never on an identity first established by a public
/// registration tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentIdentitySource {
    ServerInternal,
    PublicRegistration,
}

/// Core Cassy service - provides store access and helper methods
///
/// Supports two-tier storage architecture:
/// - Global store (~/.config/cas/) - user preferences, general learnings
///
/// Cassy requires a project-scoped `.cas/` directory (created via `cas init`).
///
/// Store instances are cached in `OnceLock` fields so each store type is
/// opened exactly once per MCP server lifetime, eliminating repeated
/// connection opens on every tool call.
#[derive(Clone)]
pub struct CasCore {
    /// Project Cassy directory (./.cas/)
    pub(crate) cas_root: PathBuf,
    /// Activity tracker for idle detection
    pub(crate) activity: Option<Arc<ActivityTracker>>,
    /// Reference to the embedded daemon (if running)
    pub(crate) daemon: Option<Arc<EmbeddedDaemon>>,
    /// Agent ID for multi-agent coordination (lazily initialized on first tool call)
    pub(crate) agent_id: OnceLock<Option<String>>,
    /// Server-side provenance for `agent_id`; absent is always non-authoritative.
    pub(crate) agent_identity_source: OnceLock<AgentIdentitySource>,
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
    /// Distilled project-knowledge pages (EPIC cas-7d31). Separate from the
    /// entry store: bodies live on disk under `.cas/knowledge/`, only the index
    /// is in SQLite.
    pub(crate) cached_knowledge_store: OnceLock<Arc<dyn cas_store::KnowledgeStore>>,
    /// Cached search index (lazily initialized, opened once per server lifetime)
    pub(crate) cached_search_index: OnceLock<SearchIndex>,
    /// Cached config (lazily initialized, loaded once per server lifetime)
    pub(crate) cached_config: OnceLock<Config>,
}

impl CasCore {
    pub(crate) fn bind_agent_identity(
        &self,
        agent_id: String,
        source: AgentIdentitySource,
    ) -> Result<(), McpError> {
        if let Some(Some(existing)) = self.agent_id.get()
            && existing != &agent_id
        {
            // Registration is also used to announce another agent to an
            // already-authenticated server. Preserve that compatible durable
            // registration path, but never let it replace the server's own
            // immutable identity or provenance.
            return Ok(());
        }
        let _ = self.agent_id.set(Some(agent_id));
        let _ = self.agent_identity_source.set(source);
        Ok(())
    }

    pub(crate) fn has_server_internal_identity(&self, agent_id: &str) -> bool {
        self.agent_id
            .get()
            .and_then(Option::as_deref)
            .is_some_and(|bound| bound == agent_id)
            && self.agent_identity_source.get() == Some(&AgentIdentitySource::ServerInternal)
    }

    pub(crate) fn ensure_public_registration_target(&self, agent_id: &str) -> Result<(), McpError> {
        let agent_store = self.open_agent_store()?;
        if agent_store.get(agent_id).ok().is_some_and(|agent| {
            matches!(
                agent.role,
                crate::types::AgentRole::Supervisor | crate::types::AgentRole::Director
            ) && crate::mcp::daemon::parse_agent_role_hint(
                std::env::var("CAS_AGENT_ROLE").ok().as_deref(),
            ) != Some(agent.role)
        }) && !self.has_server_internal_identity(agent_id)
        {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(
                    "Public registration cannot attach to an existing supervisor or director identity.",
                ),
                data: None,
            });
        }
        Ok(())
    }

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

    /// Get the distilled-knowledge store (cached).
    ///
    /// `SqliteKnowledgeStore::open` also runs `init()`, so a project that has
    /// never run `cas knowledge build` still answers `search`/`list` with an
    /// empty result instead of an error.
    pub(crate) fn open_knowledge_store(
        &self,
    ) -> Result<Arc<dyn cas_store::KnowledgeStore>, McpError> {
        Self::cached_or_init(&self.cached_knowledge_store, || {
            cas_store::SqliteKnowledgeStore::open(&self.cas_root)
                .map(|store| Arc::new(store) as Arc<dyn cas_store::KnowledgeStore>)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!("Failed to open knowledge store: {e}")),
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
    /// 2. We're currently in a Cassy-managed git worktree
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
                "Maintenance completed in {:.2}s:\n- Observations: {}\n- Decay applied: {}\n- Trace archives evicted: {}",
                result.duration_secs,
                result.observations_processed,
                result.decay_applied,
                result.trace_archives_evicted
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
                    self.bind_agent_identity(
                        agent.id.clone(),
                        AgentIdentitySource::ServerInternal,
                    )?;
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

    /// Resolve the authenticated Cassy session without registering or reviving
    /// it. Security-sensitive proof submission uses this read-only variant so
    /// a dead or unknown caller can be rejected before any durable mutation.
    pub(crate) fn get_registered_agent_id_read_only(&self) -> Result<String, McpError> {
        if let Some(Some(id)) = self.agent_id.get() {
            return Ok(id.clone());
        }

        let candidate = std::env::var("CAS_SESSION_ID")
            .ok()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .or_else(|| {
                crate::agent_id::read_session_for_mcp(&self.cas_root)
                    .ok()
                    .filter(|id| !id.is_empty())
            });
        let agent_store = self.open_agent_store()?;
        let id = if let Some(id) = candidate {
            agent_store.get(&id).map_err(|_| McpError {
                code: ErrorCode::INVALID_REQUEST,
                message: Cow::from(
                    "Authenticated Cassy session is not registered; receipt submission cannot auto-register it.",
                ),
                data: None,
            })?;
            id
        } else {
            let cc_pid = crate::agent_id::get_cc_pid_for_mcp();
            agent_store
                .get_by_cc_pid(cc_pid)
                .map_err(|error| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!(
                        "Failed to resolve registered Cassy session read-only: {error}"
                    )),
                    data: None,
                })?
                .map(|agent| agent.id)
                .ok_or_else(|| McpError {
                    code: ErrorCode::INVALID_REQUEST,
                    message: Cow::from(
                        "Receipt submission requires an already registered authenticated Cassy session.",
                    ),
                    data: None,
                })?
        };
        let _ = self.agent_id.set(Some(id.clone()));
        Ok(id)
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

                let configured_role = crate::mcp::daemon::parse_agent_role_hint(
                    std::env::var("CAS_AGENT_ROLE").ok().as_deref(),
                );
                if let Some(role) = configured_role {
                    agent.role = role;
                    agent.agent_type = match role {
                        crate::types::AgentRole::Worker => crate::types::AgentType::Worker,
                        crate::types::AgentRole::Supervisor | crate::types::AgentRole::Director => {
                            crate::types::AgentType::Primary
                        }
                        crate::types::AgentRole::Standard => agent.agent_type,
                    };
                }

                crate::mcp::daemon::apply_factory_worker_metadata(&mut agent, None);

                crate::mcp::daemon::register_with_role_reconciliation(
                    agent_store.as_ref(),
                    &agent,
                    configured_role,
                )
                .map_err(|e| McpError {
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
    /// This must be called before other Cassy tools can be used.
    /// The session_id becomes the agent's unique identifier.
    ///
    /// This server-internal path may bind an already server-created identity.
    /// Environment is accepted only as a non-privileged Worker bootstrap hint.
    pub(crate) fn register_agent(
        &self,
        session_id: String,
        name: String,
        parent_id: Option<String>,
    ) -> Result<String, McpError> {
        self.register_agent_with_hints(
            session_id,
            name,
            parent_id,
            None,
            None,
            AgentIdentitySource::ServerInternal,
        )
    }

    pub(crate) fn register_agent_with_hints(
        &self,
        session_id: String,
        name: String,
        parent_id: Option<String>,
        agent_type_hint: Option<crate::types::AgentType>,
        role_hint: Option<crate::types::AgentRole>,
        source: AgentIdentitySource,
    ) -> Result<String, McpError> {
        let pid = std::process::id();
        let agent_store = self.open_agent_store()?;
        if source == AgentIdentitySource::PublicRegistration {
            self.ensure_public_registration_target(&session_id)?;
        }

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

        let environment_role = crate::mcp::daemon::parse_agent_role_hint(
            std::env::var("CAS_AGENT_ROLE").ok().as_deref(),
        );
        let configured_role = match source {
            AgentIdentitySource::ServerInternal => role_hint.or(environment_role),
            // A typed registration request is explicit caller input. Ambient
            // role is only a bootstrap fallback when no role was requested.
            AgentIdentitySource::PublicRegistration => role_hint.or(environment_role),
        };
        if let Some(role) = configured_role {
            agent.role = role;
            agent.agent_type = match role {
                crate::types::AgentRole::Worker => crate::types::AgentType::Worker,
                crate::types::AgentRole::Supervisor | crate::types::AgentRole::Director => {
                    crate::types::AgentType::Primary
                }
                crate::types::AgentRole::Standard => agent.agent_type,
            };
        } else if agent.agent_type == crate::types::AgentType::Worker {
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

        crate::mcp::daemon::register_with_role_reconciliation(
            agent_store.as_ref(),
            &agent,
            configured_role,
        )
        .map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to register agent: {e}")),
            data: None,
        })?;

        self.bind_agent_identity(session_id.clone(), source)?;

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

    /// Auto-claim the exact task whose close is awaiting verification.
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
                // Claimed successfully or already claimed - keep the task in
                // progress until its own close transition is resolved.
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

pub(crate) mod parent_watchdog;
mod prompts;
mod resources;
mod runtime;

#[cfg(feature = "mcp-proxy")]
pub(crate) use runtime::install_proxy_policy;
pub use runtime::run_server;
#[cfg(feature = "mcp-proxy")]
pub use runtime::{
    ProxySnapshotCache, ProxySnapshotFailure, ProxySnapshotReadError, ProxySnapshotReadErrorKind,
    ProxySnapshotState, proxy_config_fingerprint, read_proxy_catalog_cache,
    read_proxy_health_cache, read_proxy_snapshot_cache, write_empty_proxy_snapshot_cache,
    write_proxy_catalog_cache, write_proxy_health_cache, write_proxy_snapshot_cache,
    write_proxy_snapshot_cache_for_config, write_unavailable_proxy_snapshot_cache,
};

#[cfg(test)]
mod role_registration_tests {
    use super::*;
    use crate::store::{init_cas_dir, open_agent_store};
    use crate::test_support::TestEnvGuard;
    use crate::types::{AgentRole, AgentType};

    #[test]
    fn eager_environment_registration_persists_supervisor_role() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("supervisor")),
            ("CAS_FACTORY_SESSION", Some("factory-eager-supervisor")),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let cas_root = init_cas_dir(temp.path()).unwrap();
        let core = CasCore::with_daemon(cas_root.clone(), None, None);

        core.register_agent(
            "eager-supervisor-session".to_string(),
            "eager-supervisor".to_string(),
            None,
        )
        .unwrap();

        let agent = open_agent_store(&cas_root)
            .unwrap()
            .get("eager-supervisor-session")
            .unwrap();
        assert_eq!(agent.role, AgentRole::Supervisor);
        assert_eq!(agent.agent_type, AgentType::Primary);
        assert_eq!(
            agent.factory_session.as_deref(),
            Some("factory-eager-supervisor")
        );
    }

    #[test]
    fn missing_cached_agent_reregisters_as_supervisor() {
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("supervisor")),
            ("CAS_FACTORY_SESSION", Some("factory-reregister-supervisor")),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let cas_root = init_cas_dir(temp.path()).unwrap();
        let core = CasCore::with_daemon(cas_root.clone(), None, None);
        core.register_agent(
            "cached-supervisor-session".to_string(),
            "original-supervisor".to_string(),
            None,
        )
        .unwrap();
        let store = open_agent_store(&cas_root).unwrap();
        store.unregister("cached-supervisor-session").unwrap();

        core.ensure_agent_active("cached-supervisor-session")
            .unwrap();

        let agent = store.get("cached-supervisor-session").unwrap();
        assert_eq!(agent.name, "Primary (re-registered)");
        assert_eq!(agent.role, AgentRole::Supervisor);
        assert_eq!(agent.agent_type, AgentType::Primary);
    }
}
