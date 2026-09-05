//! Store detection and factory functions
//!
//! Cassy uses project-scoped storage in `./.cas/` directories.
//! Each project requires `cas init` before use.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{
    AgentStore, CodeStore, CommitLinkStore, EntityStore, EventStore, FileChangeStore, LoopStore,
    MarkdownRuleStore, MarkdownStore, NotifyingEntryStore, NotifyingRuleStore, NotifyingSkillStore,
    NotifyingTaskStore, PromptQueueStore, PromptStore, RecordingStore, ReminderStore, RuleStore,
    SkillStore, SpawnQueueStore, SpecStore, SqliteAgentStore, SqliteCodeStore,
    SqliteCommitLinkStore, SqliteEntityStore, SqliteEventStore, SqliteFileChangeStore,
    SqliteLoopStore, SqlitePromptQueueStore, SqlitePromptStore, SqliteRecordingStore,
    SqliteReminderStore, SqliteRuleStore, SqliteSkillStore, SqliteSpawnQueueStore, SqliteSpecStore,
    SqliteStore, SqliteSupervisorQueueStore, SqliteTaskStore, SqliteVerificationStore,
    SqliteWorktreeStore, Store, SupervisorQueueStore, TaskStore, VerificationStore, WorktreeStore,
};
use crate::cloud::{CloudConfig, SyncQueue};
use crate::config::Config;
use crate::error::CasError;
use crate::migration::run_migrations;
use crate::notifications::has_notifier;
use crate::store::{SyncingEntryStore, SyncingRuleStore, SyncingSkillStore, SyncingTaskStore};

/// Result type for detect functions (uses CasError for richer error handling)
type Result<T> = std::result::Result<T, CasError>;

/// Type of storage backend
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreType {
    /// Modern SQLite storage
    Sqlite,
    /// Legacy markdown files
    Markdown,
}

/// Check if a project store exists in the current directory tree
pub fn has_project_cas() -> bool {
    find_cas_root().is_ok()
}

/// Find the .cas directory by searching up the directory tree
///
/// Priority order:
/// 1. CAS_ROOT environment variable (if set and valid)
/// 2. Git worktree detection (uses main repo's .cas)
/// 3. Walk up directory tree from cwd
pub fn find_cas_root() -> Result<PathBuf> {
    // 1. Check CAS_ROOT env var first (highest priority)
    // This enables workers in clones to use the main repo's .cas
    if let Ok(cas_root) = std::env::var("CAS_ROOT") {
        let path = PathBuf::from(&cas_root);
        if path.exists() && path.is_dir() {
            // cas-b69a (GH #157): the override is legitimate but must never be
            // silent — announce it before the caller reads or writes anything.
            if let Ok(cwd) = std::env::current_dir() {
                announce_root_override_once(&path, &cwd);
            }
            return Ok(path);
        }
        // If CAS_ROOT is set but invalid, fall through to other methods
        // (The invalid path will be ignored and we'll try worktree/walk detection)
    }

    // 2. Existing logic: worktree detection, directory walk
    let cwd = std::env::current_dir()?;
    find_cas_root_from(&cwd)
}

/// cas-b69a (GH #157): the one-line notice emitted when `CAS_ROOT` resolves a
/// different store than the caller's location would have.
///
/// ROOT CAUSE this makes visible: `find_cas_root` checks `CAS_ROOT` before the
/// working directory, and nothing said so. During the cas-b129 M3 rehearsal an
/// operator copied the project store to `/tmp`, `cd`'d into the copy and ran the
/// migration — under a factory session `CAS_ROOT` still pointed at the live
/// project, so the "rehearsal" wrote 126 knowledge_pages rows and deleted 11
/// sync_queue rows in production (restored from the tool's own ledger).
///
/// The precedence is deliberately NOT changed: factory workers in clones and
/// worktrees depend on `CAS_ROOT` winning. What changes is that the loser is
/// named out loud.
///
/// Returns `None` when there is nothing to disambiguate: no competing root, or
/// both candidates are the same store (compared through `canonicalize` so a
/// symlinked or `..`-laden spelling of the same directory is not reported as a
/// conflict).
pub(crate) fn root_override_notice(env_root: &Path, cwd_root: &Path) -> Option<String> {
    let same = canonical_or_owned(env_root) == canonical_or_owned(cwd_root);
    if same {
        return None;
    }
    Some(format!(
        "⚠️  CAS_ROOT override: CAS_ROOT={} overrides the working-directory store {} \
         — CAS_ROOT wins; every read and write below targets {}.\n\
         \x20   This precedence is intentional (factory workers in clones rely on it). \
         To act on the working-directory store instead, clear the variable for this \
         command: `env -u CAS_ROOT cas <command>`.",
        env_root.display(),
        cwd_root.display(),
        env_root.display(),
    ))
}

/// `canonicalize` when the path resolves, otherwise the path as given. Used only
/// for equality comparison, never for anything the user sees.
fn canonical_or_owned(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// cas-b69a (GH #157): print [`root_override_notice`] to **stderr**, at most
/// once per process.
///
/// stderr, not stdout, so the notice can never corrupt parseable command output
/// (`--json`, hook payloads, MCP stdio framing). Once per process, because
/// `find_cas_root` is called dozens of times per command and a repeated banner
/// would train everyone to ignore it.
///
/// The comparison itself is also done at most once: resolving the competing root
/// walks the directory tree, and that cost should not ride on every lookup.
fn announce_root_override_once(env_root: &Path, start: &Path) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Ok(cwd_root) = find_cas_root_ignoring_env(start) else {
            // No competing store at all — CAS_ROOT is the only candidate and
            // there is nothing for the operator to be confused about.
            return;
        };
        if let Some(notice) = root_override_notice(env_root, &cwd_root) {
            eprintln!("{notice}");
        }
    });
}

/// Find the .cas directory starting from a specific path
///
/// This function handles git worktrees: if we're in a worktree, it looks
/// for .cas in the main repository first, before falling back to walking
/// up the directory tree.
///
/// Detection priority:
/// 1. CAS_ROOT env var (explicit override)
/// 2. Cassy worktree detection (path contains .cas/worktrees/)
/// 3. Git worktree detection (parse .git file)
/// 4. Directory walk (walk up looking for .cas/)
pub fn find_cas_root_from(start: &Path) -> Result<PathBuf> {
    // Respect CAS_ROOT for explicit overrides (useful for workers in clones and external tooling).
    // This mirrors `find_cas_root()` behavior but applies when callers start from an explicit path.
    if let Ok(cas_root) = std::env::var("CAS_ROOT") {
        let path = PathBuf::from(&cas_root);
        if path.exists() && path.is_dir() {
            // cas-b69a (GH #157): name the root that lost, before any I/O.
            announce_root_override_once(&path, start);
            return Ok(path);
        }
    }

    find_cas_root_ignoring_env(start)
}

/// The root `start` resolves to when `CAS_ROOT` is not consulted at all:
/// Cassy worktree detection, then git-worktree detection, then a directory walk.
///
/// Split out for cas-b69a (GH #157) so the detect layer can answer "what would
/// this location have resolved to on its own?" and name it when `CAS_ROOT`
/// overrides it. The resolution order is byte-for-byte the pre-existing one —
/// only its call site moved.
pub(crate) fn find_cas_root_ignoring_env(start: &Path) -> Result<PathBuf> {
    // Check if we're inside a Cassy worktree (.cas/worktrees/<name>/).
    // This is the most reliable detection for factory workers because it
    // doesn't depend on git state or .git file parsing.
    if let Some(cas_dir) = find_cas_root_from_cas_worktree(start) {
        if cas_dir.exists() && cas_dir.is_dir() {
            return Ok(cas_dir);
        }
    }

    // Check if we're in a git worktree and look for .cas in the main repo.
    // This takes priority because worktrees should share the main repo's .cas.
    if let Some(main_repo) = find_main_repo_from_worktree(start) {
        let cas_dir = main_repo.join(".cas");
        if cas_dir.exists() && cas_dir.is_dir() {
            return Ok(cas_dir);
        }
    }

    // If not in a worktree (or main repo has no .cas), walk up the directory tree
    let mut current = start.to_path_buf();

    loop {
        let cas_dir = current.join(".cas");
        if cas_dir.exists() && cas_dir.is_dir() {
            return Ok(cas_dir);
        }

        if !current.pop() {
            break;
        }
    }

    Err(CasError::NotInitialized)
}

/// Detect if `start` is inside a Cassy factory worktree (.cas/worktrees/<name>/)
/// and return the parent repo's .cas/ directory.
///
/// Cassy factory worktrees are always created under `<project>/.cas/worktrees/<worker>/`.
/// By detecting the `.cas/worktrees/` path component, we can resolve directly to the
/// parent `.cas/` directory without relying on git state.
fn find_cas_root_from_cas_worktree(start: &Path) -> Option<PathBuf> {
    // Convert to string for pattern matching
    let path_str = start.to_string_lossy();

    // Look for .cas/worktrees/ in the path
    if let Some(idx) = path_str.find(".cas/worktrees/") {
        let cas_dir = PathBuf::from(&path_str[..idx + ".cas".len()]);
        if cas_dir.join("cas.db").exists() || cas_dir.is_dir() {
            return Some(cas_dir);
        }
    }

    None
}

/// Check if we're in a git worktree and return the main repository path.
///
/// Git worktrees have a `.git` file (not directory) containing:
/// ```text
/// gitdir: /path/to/main/.git/worktrees/<worktree-name>
/// ```
///
/// We parse this to find the main repository's path.
/// Handles both absolute and relative gitdir paths.
fn find_main_repo_from_worktree(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();

    loop {
        let git_path = current.join(".git");

        // Check if .git is a file (worktree) rather than a directory
        if git_path.is_file() {
            if let Ok(content) = std::fs::read_to_string(&git_path) {
                // Parse "gitdir: /path/to/main/.git/worktrees/<name>"
                if let Some(gitdir) = content.strip_prefix("gitdir: ") {
                    let gitdir = gitdir.trim();
                    let gitdir_path = PathBuf::from(gitdir);

                    // Resolve relative paths against the worktree root (where .git file lives)
                    let gitdir_path = if gitdir_path.is_relative() {
                        current.join(&gitdir_path)
                    } else {
                        gitdir_path
                    };

                    // The gitdir points to .git/worktrees/<name>
                    // We need to go up to .git, then up again to the repo root
                    // e.g., /path/to/main/.git/worktrees/wt1 -> /path/to/main
                    if let Some(git_dir) = gitdir_path.parent() {
                        // .git/worktrees
                        if let Some(git_dir) = git_dir.parent() {
                            // .git
                            if let Some(main_repo) = git_dir.parent() {
                                // main repo — canonicalize to resolve any ../ components
                                let main_repo = main_repo
                                    .canonicalize()
                                    .unwrap_or_else(|_| main_repo.to_path_buf());
                                return Some(main_repo);
                            }
                        }
                    }
                }
            }
        }

        // Also check if this is a regular git repo (has .git directory)
        // If so, we're not in a worktree, stop searching
        if git_path.is_dir() {
            return None;
        }

        if !current.pop() {
            break;
        }
    }

    None
}

/// Detect the storage type for a .cas directory
pub fn detect_store_type(cas_dir: &Path) -> StoreType {
    let db_path = cas_dir.join("cas.db");
    if db_path.exists() {
        return StoreType::Sqlite;
    }

    let entries_dir = cas_dir.join("entries");
    if entries_dir.exists() {
        return StoreType::Markdown;
    }

    // Default to SQLite for new installations
    StoreType::Sqlite
}

/// Open base entry store (sqlite/markdown + optional notifier).
/// Never wraps with [`SyncingEntryStore`] — safe for pull/apply-remote.
fn open_store_base(cas_dir: &Path) -> Result<Arc<dyn Store>> {
    let store_type = detect_store_type(cas_dir);
    let config = Config::load(cas_dir).unwrap_or_default();

    let base_store: Arc<dyn Store> = match store_type {
        StoreType::Sqlite => {
            let store = SqliteStore::open(cas_dir)?;
            store.init()?;
            Arc::new(store)
        }
        StoreType::Markdown => {
            let store = MarkdownStore::open(cas_dir)?;
            store.init()?;
            Arc::new(store)
        }
    };

    // Wrap with notifying store if TUI notifier is active
    if has_notifier() && config.notifications_enabled() {
        Ok(Arc::new(NotifyingEntryStore::new(
            base_store,
            config.notifications(),
        )))
    } else {
        Ok(base_store)
    }
}

/// Open the appropriate store based on what exists.
///
/// When logged in, wraps writes with [`SyncingEntryStore`] so local edits
/// enqueue to the cloud SyncQueue. For pull / apply-remote paths use
/// [`open_store_local`] instead — otherwise every pulled row re-enters the
/// queue and push↔pull never settles (cas-7fbb).
pub fn open_store(cas_dir: &Path) -> Result<Arc<dyn Store>> {
    let base_store = open_store_base(cas_dir)?;

    // Wrap with cloud sync if logged in
    if let Ok(cloud_config) = CloudConfig::load_from_cas_dir(cas_dir) {
        if cloud_config.is_logged_in() {
            if let Ok(queue) = SyncQueue::open(cas_dir) {
                let _ = queue.init();
                return Ok(Arc::new(
                    SyncingEntryStore::new(base_store, Arc::new(queue))
                        .with_cloud_config(Arc::new(cloud_config)),
                ));
            }
        }
    }

    Ok(base_store)
}

/// Open entry store without cloud SyncQueue wrappers.
///
/// Use on pull / team-pull / daemon cloud-sync apply paths so remote rows
/// are written locally without re-enqueueing (cas-7fbb).
pub fn open_store_local(cas_dir: &Path) -> Result<Arc<dyn Store>> {
    open_store_base(cas_dir)
}

/// Open base task store (+ optional notifier). No SyncingTaskStore.
fn open_task_store_base(cas_dir: &Path) -> Result<Arc<dyn TaskStore>> {
    let config = Config::load(cas_dir).unwrap_or_default();

    let origin_project = crate::cloud::resolve_canonical_id(cas_dir);
    let store = SqliteTaskStore::open_with_origin_project(cas_dir, origin_project.as_deref())?;
    store.init()?;
    let mut base_store: Arc<dyn TaskStore> = Arc::new(store);

    // cas-4342 (GH #701): rows quarantined by `cas doctor --fix-cloud-rows`
    // are hidden from every list surface here, at the one seam they all share,
    // rather than at each board caller. Innermost on purpose: the wrappers
    // above must see the same board the operator does. A ledger that cannot be
    // opened simply hides nothing — a suppression must never be able to take
    // the whole task store down with it.
    if let Ok(queue) = crate::cloud::SyncQueue::open(cas_dir) {
        let queue = Arc::new(queue);
        if queue.quarantined_count(crate::cloud::QUARANTINE_TASK).is_ok() {
            base_store = Arc::new(crate::store::QuarantineFilteringTaskStore::new(
                base_store, queue,
            ));
        }
    }

    if has_notifier() && config.notifications_enabled() {
        Ok(Arc::new(NotifyingTaskStore::new(
            base_store,
            config.notifications(),
        )))
    } else {
        Ok(base_store)
    }
}

/// Open the task store (cloud-sync wrap when logged in).
/// Prefer [`open_task_store_local`] for pull/apply-remote paths.
pub fn open_task_store(cas_dir: &Path) -> Result<Arc<dyn TaskStore>> {
    let base_store = open_task_store_base(cas_dir)?;

    // Wrap with cloud sync if logged in
    if let Ok(cloud_config) = CloudConfig::load_from_cas_dir(cas_dir) {
        if cloud_config.is_logged_in() {
            if let Ok(queue) = SyncQueue::open(cas_dir) {
                if queue.init().is_ok() {
                    let store = SyncingTaskStore::new(base_store, Arc::new(queue))
                        .with_cloud_config(Arc::new(cloud_config));
                    // A prior local task write may have committed immediately
                    // before its outbox transaction failed or the process
                    // exited. Repair that durable intent during store reopen;
                    // if SQLite is still unhealthy, re-report degradation
                    // instead of silently returning a store that lost sync.
                    store.reconcile_pending_task_sync()?;
                    return Ok(Arc::new(store));
                }
            }
        }
    }

    Ok(base_store)
}

/// Open task store without cloud SyncQueue wrappers (cas-7fbb).
pub fn open_task_store_local(cas_dir: &Path) -> Result<Arc<dyn TaskStore>> {
    open_task_store_base(cas_dir)
}

/// Open base skill store (+ optional notifier). No SyncingSkillStore.
fn open_skill_store_base(cas_dir: &Path) -> Result<Arc<dyn SkillStore>> {
    let config = Config::load(cas_dir).unwrap_or_default();

    let store = SqliteSkillStore::open(cas_dir)?;
    store.init()?;
    let base_store: Arc<dyn SkillStore> = Arc::new(store);

    if has_notifier() && config.notifications_enabled() {
        Ok(Arc::new(NotifyingSkillStore::new(
            base_store,
            config.notifications(),
        )))
    } else {
        Ok(base_store)
    }
}

/// Open the skill store (cloud-sync wrap when logged in).
/// Prefer [`open_skill_store_local`] for pull/apply-remote paths.
pub fn open_skill_store(cas_dir: &Path) -> Result<Arc<dyn SkillStore>> {
    let base_store = open_skill_store_base(cas_dir)?;

    // Wrap with cloud sync if logged in
    if let Ok(cloud_config) = CloudConfig::load_from_cas_dir(cas_dir) {
        if cloud_config.is_logged_in() {
            if let Ok(queue) = SyncQueue::open(cas_dir) {
                let _ = queue.init();
                return Ok(Arc::new(
                    SyncingSkillStore::new(base_store, Arc::new(queue))
                        .with_cloud_config(Arc::new(cloud_config)),
                ));
            }
        }
    }

    Ok(base_store)
}

/// Open skill store without cloud SyncQueue wrappers (cas-7fbb).
pub fn open_skill_store_local(cas_dir: &Path) -> Result<Arc<dyn SkillStore>> {
    open_skill_store_base(cas_dir)
}

/// Open the entity store for knowledge graph
pub fn open_entity_store(cas_dir: &Path) -> Result<Arc<dyn EntityStore>> {
    let store = SqliteEntityStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the agent store for multi-agent coordination
pub fn open_agent_store(cas_dir: &Path) -> Result<Arc<dyn AgentStore>> {
    let store = SqliteAgentStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the loop store
pub fn open_loop_store(cas_dir: &Path) -> Result<Arc<dyn LoopStore>> {
    let store = SqliteLoopStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the verification store for task quality gates
pub fn open_verification_store(cas_dir: &Path) -> Result<Arc<dyn VerificationStore>> {
    let store = SqliteVerificationStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the worktree store for tracking git worktrees
pub fn open_worktree_store(cas_dir: &Path) -> Result<Arc<dyn WorktreeStore>> {
    let store = SqliteWorktreeStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the recording store for terminal recording metadata
pub fn open_recording_store(cas_dir: &Path) -> Result<Arc<dyn RecordingStore>> {
    let store = SqliteRecordingStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the code store for indexed source code
pub fn open_code_store(cas_dir: &Path) -> Result<Arc<dyn CodeStore>> {
    let store = SqliteCodeStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the supervisor queue store for factory session Director → Supervisor communication
pub fn open_supervisor_queue_store(cas_dir: &Path) -> Result<Arc<dyn SupervisorQueueStore>> {
    let store = SqliteSupervisorQueueStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the prompt queue store (for supervisor → worker communication)
pub fn open_prompt_queue_store(cas_dir: &Path) -> Result<Arc<dyn PromptQueueStore>> {
    let store = SqlitePromptQueueStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the reminder store (for supervisor "Remind Me" feature)
pub fn open_reminder_store(cas_dir: &Path) -> Result<Arc<dyn ReminderStore>> {
    let store = SqliteReminderStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the spawn queue store (for dynamic worker lifecycle management)
pub fn open_spawn_queue_store(cas_dir: &Path) -> Result<Arc<dyn SpawnQueueStore>> {
    let store = SqliteSpawnQueueStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the prompt store (for code attribution / git blame)
pub fn open_prompt_store(cas_dir: &Path) -> Result<Arc<dyn PromptStore>> {
    let store = SqlitePromptStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the file change store (for code attribution / git blame)
pub fn open_file_change_store(cas_dir: &Path) -> Result<Arc<dyn FileChangeStore>> {
    let store = SqliteFileChangeStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the commit link store (for code attribution / git blame)
pub fn open_commit_link_store(cas_dir: &Path) -> Result<Arc<dyn CommitLinkStore>> {
    let store = SqliteCommitLinkStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the event store (for activity tracking)
pub fn open_event_store(cas_dir: &Path) -> Result<Arc<dyn EventStore>> {
    let store = SqliteEventStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open the spec store
pub fn open_spec_store(cas_dir: &Path) -> Result<Arc<dyn SpecStore>> {
    let store = SqliteSpecStore::open(cas_dir)?;
    store.init()?;
    Ok(Arc::new(store))
}

/// Open base rule store (+ optional notifier). No SyncingRuleStore / SyncQueue.
fn open_rule_store_base(cas_dir: &Path) -> Result<Arc<dyn RuleStore>> {
    let store_type = detect_store_type(cas_dir);
    let config = Config::load(cas_dir).unwrap_or_default();

    let base_store: Arc<dyn RuleStore> = match store_type {
        StoreType::Sqlite => {
            let store = SqliteRuleStore::open(cas_dir)?;
            store.init()?;
            Arc::new(store)
        }
        StoreType::Markdown => {
            let store = MarkdownRuleStore::open(cas_dir)?;
            store.init()?;
            Arc::new(store)
        }
    };

    if has_notifier() && config.notifications_enabled() {
        Ok(Arc::new(NotifyingRuleStore::new(
            base_store,
            config.notifications(),
        )))
    } else {
        Ok(base_store)
    }
}

/// Open the appropriate rule store (file + cloud sync wrappers when enabled).
/// Prefer [`open_rule_store_local`] for pull/apply-remote paths.
pub fn open_rule_store(cas_dir: &Path) -> Result<Arc<dyn RuleStore>> {
    let config = Config::load(cas_dir).unwrap_or_default();
    let base_store = open_rule_store_base(cas_dir)?;

    // Wrap with syncing store if sync is enabled
    if config.sync.enabled && !Config::is_sync_disabled() {
        let project_root = cas_dir.parent().unwrap_or(Path::new("."));
        let target_dir = project_root.join(&config.sync.target);

        // Check if cloud sync is also enabled. When it is, thread the
        // CloudConfig through so team auto-promotion is active.
        let cloud_setup: Option<(Arc<SyncQueue>, Arc<CloudConfig>)> =
            if let Ok(cloud_config) = CloudConfig::load_from_cas_dir(cas_dir) {
                if cloud_config.is_logged_in() {
                    SyncQueue::open(cas_dir).ok().map(|q| {
                        let _ = q.init();
                        (Arc::new(q), Arc::new(cloud_config))
                    })
                } else {
                    None
                }
            } else {
                None
            };

        if let Some((queue, cloud_config)) = cloud_setup {
            return Ok(Arc::new(
                SyncingRuleStore::with_cloud_queue(
                    base_store,
                    target_dir,
                    config.sync.min_helpful,
                    queue,
                )
                .with_cloud_config(cloud_config),
            ));
        } else {
            return Ok(Arc::new(SyncingRuleStore::new(
                base_store,
                target_dir,
                config.sync.min_helpful,
            )));
        }
    }

    Ok(base_store)
}

/// Open rule store without SyncingRuleStore / SyncQueue wrappers (cas-7fbb).
///
/// Skips both cloud enqueue and local `.claude/rules` file sync so pull
/// apply does not re-feed the queue. Local rule-file sync still runs on
/// normal edit paths via [`open_rule_store`].
pub fn open_rule_store_local(cas_dir: &Path) -> Result<Arc<dyn RuleStore>> {
    open_rule_store_base(cas_dir)
}

/// Initialize a new .cas directory
pub fn init_cas_dir(path: &Path) -> Result<PathBuf> {
    let cas_dir = path.join(".cas");

    if cas_dir.exists() {
        return Ok(cas_dir);
    }

    std::fs::create_dir_all(&cas_dir)?;

    // Create SQLite store
    let store = SqliteStore::open(&cas_dir)?;
    store.init()?;

    // Create rule store
    let rule_store = SqliteRuleStore::open(&cas_dir)?;
    rule_store.init()?;

    // Create task store
    let task_store = SqliteTaskStore::open(&cas_dir)?;
    task_store.init()?;

    // Create skill store
    let skill_store = SqliteSkillStore::open(&cas_dir)?;
    skill_store.init()?;

    // Create entity store for knowledge graph
    let entity_store = SqliteEntityStore::open(&cas_dir)?;
    entity_store.init()?;

    // Create agent store for multi-agent coordination
    let agent_store = SqliteAgentStore::open(&cas_dir)?;
    agent_store.init()?;

    // Create loop store for iteration loops (auto-inits on open)
    let _loop_store = SqliteLoopStore::open(&cas_dir)?;

    // Create verification store for task quality gates (auto-inits on open)
    let _verification_store = SqliteVerificationStore::open(&cas_dir)?;

    // Create default config
    let config = Config::default();
    config.save(&cas_dir)?;

    // Run migrations to create any additional tables (e.g., worktrees)
    // Fail init if migrations fail to avoid partial/unsafe schema state.
    run_migrations(&cas_dir, false)?;

    Ok(cas_dir)
}

#[cfg(test)]
mod tests {
    use crate::cloud::{SyncOperation, SyncQueue};
    use crate::store::detect::*;
    use crate::store::{SqliteTaskStore, StoreError, TaskStore};
    use crate::test_support::TestEnvGuard;
    use crate::types::Task;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn test_init_cas_dir() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();

        assert!(cas_dir.exists());
        assert!(cas_dir.join("cas.db").exists());
        // Config is now saved as TOML (preferred format)
        assert!(cas_dir.join("config.toml").exists());
    }

    #[test]
    fn logged_in_task_store_open_reconciles_a_committed_task_sync_intent() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        std::fs::write(cas_dir.join("cloud.json"), r#"{"token":"test-token"}"#).unwrap();

        let local = SqliteTaskStore::open(&cas_dir).unwrap();
        local.init().unwrap();
        let task = Task::new("task-restart-repair".to_string(), "repair me".to_string());
        local.add(&task).unwrap();
        let queue = SyncQueue::open(&cas_dir).unwrap();
        queue.init().unwrap();
        queue
            .stage_task_sync_intent(&task.id, "add", None, None, None, false)
            .unwrap();

        open_task_store(&cas_dir).unwrap();

        assert!(queue.pending_task_sync_intents().unwrap().is_empty());
        let pending = queue.pending(10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].entity_id, task.id);
        assert_eq!(pending[0].operation, SyncOperation::Upsert);
    }

    #[test]
    fn logged_in_task_store_open_re_reports_a_still_broken_task_sync_intent() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        std::fs::write(cas_dir.join("cloud.json"), r#"{"token":"test-token"}"#).unwrap();

        let local = SqliteTaskStore::open(&cas_dir).unwrap();
        local.init().unwrap();
        let task = Task::new("task-restart-degraded".to_string(), "repair me".to_string());
        local.add(&task).unwrap();
        let queue = SyncQueue::open(&cas_dir).unwrap();
        queue.init().unwrap();
        queue
            .stage_task_sync_intent(&task.id, "add", None, None, None, false)
            .unwrap();
        let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
        conn.execute_batch(
            r#"
            CREATE TRIGGER fail_task_enqueue_on_reopen
            BEFORE INSERT ON sync_queue
            WHEN NEW.entity_type = 'task' AND NEW.team_id = ''
            BEGIN
                SELECT RAISE(FAIL, 'injected reopen enqueue failure');
            END;
            "#,
        )
        .unwrap();

        let error = match open_task_store(&cas_dir) {
            Ok(_) => panic!("reopen must report the retained intent while enqueue is broken"),
            Err(error) => error,
        };
        match error {
            CasError::StoreErr(StoreError::SyncDegradedAfterCommit {
                entity_id, reason, ..
            }) => {
                assert_eq!(entity_id, task.id);
                assert!(
                    reason.contains("injected reopen enqueue failure"),
                    "{reason}"
                );
            }
            other => panic!("expected structured degraded-sync error, got {other}"),
        }
        assert_eq!(queue.pending_task_sync_intents().unwrap().len(), 1);
        assert!(queue.pending(10, 5).unwrap().is_empty());
    }

    #[test]
    fn test_find_cas_root() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_ROOT", None)]);

        let temp = TempDir::new().unwrap();
        init_cas_dir(temp.path()).unwrap();

        // Create a subdirectory
        let subdir = temp.path().join("subdir/nested");
        std::fs::create_dir_all(&subdir).unwrap();

        // Should find .cas from subdirectory
        let found = find_cas_root_from(&subdir).unwrap();
        assert_eq!(found, temp.path().join(".cas"));
    }

    #[test]
    fn test_detect_store_type() {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();

        // Default should be SQLite
        assert_eq!(detect_store_type(&cas_dir), StoreType::Sqlite);

        // Create entries dir to simulate markdown store
        std::fs::create_dir_all(cas_dir.join("entries")).unwrap();
        assert_eq!(detect_store_type(&cas_dir), StoreType::Markdown);

        // SQLite takes precedence
        std::fs::write(cas_dir.join("cas.db"), "").unwrap();
        assert_eq!(detect_store_type(&cas_dir), StoreType::Sqlite);
    }

    #[test]
    #[ignore] // Uses global state (CWD, env vars) - run with: cargo test -- --ignored
    fn test_has_project_cas() {
        let mut env = TestEnvGuard::with_optional_vars(&[("CAS_ROOT", None)]);

        let temp = TempDir::new().unwrap();
        // Canonicalize to handle macOS /var -> /private/var symlinks
        let temp_path = temp
            .path()
            .canonicalize()
            .expect("Failed to canonicalize temp path");

        env.set_current_dir(&temp_path);

        // In temp dir with no .cas, should return false
        assert!(!has_project_cas(), "Expected no .cas in empty temp dir");

        // After init, should return true
        init_cas_dir(&temp_path).unwrap();
        assert!(has_project_cas(), "Expected .cas to be found after init");

    }

    #[test]
    fn test_find_cas_root_from_worktree() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_ROOT", None)]);

        // Simulate a git worktree structure:
        // /main_repo/.cas/       <- Cassy directory
        // /main_repo/.git/       <- Main git directory
        // /main_repo/.git/worktrees/wt1/  <- Worktree git data
        // /worktrees/wt1/.git    <- File pointing to main repo
        let temp = TempDir::new().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let main_repo = temp_root.join("main_repo");
        let worktree = temp_root.join("worktrees/wt1");

        // Create main repo with .cas
        std::fs::create_dir_all(&main_repo).unwrap();
        init_cas_dir(&main_repo).unwrap();

        // Create main repo's .git directory and worktrees subdir
        let git_dir = main_repo.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let worktree_git_data = git_dir.join("worktrees/wt1");
        std::fs::create_dir_all(&worktree_git_data).unwrap();

        // Create worktree directory with .git file pointing to main repo
        std::fs::create_dir_all(&worktree).unwrap();
        let git_file_content = format!("gitdir: {}", worktree_git_data.display());
        std::fs::write(worktree.join(".git"), git_file_content).unwrap();

        // Should find .cas from worktree by following the git pointer
        let found = find_cas_root_from(&worktree).unwrap();
        assert_eq!(found, main_repo.join(".cas"));

        // Should also work from a subdirectory of the worktree
        let worktree_subdir = worktree.join("src/subdir");
        std::fs::create_dir_all(&worktree_subdir).unwrap();
        let found = find_cas_root_from(&worktree_subdir).unwrap();
        assert_eq!(found, main_repo.join(".cas"));
    }

    #[test]
    fn test_find_main_repo_from_worktree() {
        let temp = TempDir::new().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let main_repo = temp_root.join("main_repo");
        let worktree = temp_root.join("worktrees/wt1");

        // Create main repo's .git directory and worktrees subdir
        let git_dir = main_repo.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let worktree_git_data = git_dir.join("worktrees/wt1");
        std::fs::create_dir_all(&worktree_git_data).unwrap();

        // Create worktree directory with .git file
        std::fs::create_dir_all(&worktree).unwrap();
        let git_file_content = format!("gitdir: {}", worktree_git_data.display());
        std::fs::write(worktree.join(".git"), git_file_content).unwrap();

        // Should find main repo from worktree
        let found = find_main_repo_from_worktree(&worktree);
        assert_eq!(found, Some(main_repo));

        // Should return None for regular git repo
        let regular_repo = temp_root.join("regular_repo");
        std::fs::create_dir_all(regular_repo.join(".git")).unwrap();
        let found = find_main_repo_from_worktree(&regular_repo);
        assert!(found.is_none());

        // Should return None for non-git directory
        let non_git = temp_root.join("non_git");
        std::fs::create_dir_all(&non_git).unwrap();
        let found = find_main_repo_from_worktree(&non_git);
        assert!(found.is_none());
    }

    #[test]
    fn test_find_cas_root_from_cas_worktree() {
        let _env = TestEnvGuard::with_optional_vars(&[("CAS_ROOT", None)]);

        // Simulate a Cassy factory worktree structure:
        // /project/.cas/          <- Cassy directory with cas.db
        // /project/.cas/worktrees/fox/  <- Worker worktree
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        init_cas_dir(&project).unwrap();

        let worktree = project.join(".cas/worktrees/fox");
        std::fs::create_dir_all(&worktree).unwrap();

        // Should find .cas from Cassy worktree via path pattern detection
        let found = find_cas_root_from_cas_worktree(&worktree);
        assert_eq!(found, Some(project.join(".cas")));

        // Should also work from a subdirectory of the worktree
        let subdir = worktree.join("src/deep/nested");
        std::fs::create_dir_all(&subdir).unwrap();
        let found = find_cas_root_from_cas_worktree(&subdir);
        assert_eq!(found, Some(project.join(".cas")));

        // find_cas_root_from should use Cassy worktree detection
        let found = find_cas_root_from(&worktree).unwrap();
        assert_eq!(found, project.join(".cas"));

        // Should return None for non-worktree paths
        let found = find_cas_root_from_cas_worktree(&project);
        assert!(found.is_none());
    }

    #[test]
    fn test_find_main_repo_from_worktree_relative_gitdir() {
        let temp = TempDir::new().unwrap();
        let main_repo = temp.path().join("main_repo");
        let worktree = temp.path().join("worktrees/wt1");

        // Create main repo's .git directory and worktrees subdir
        let git_dir = main_repo.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let worktree_git_data = git_dir.join("worktrees/wt1");
        std::fs::create_dir_all(&worktree_git_data).unwrap();

        // Create worktree with RELATIVE .git path (Git 2.40+)
        std::fs::create_dir_all(&worktree).unwrap();
        let relative_gitdir = "../../main_repo/.git/worktrees/wt1";
        std::fs::write(worktree.join(".git"), format!("gitdir: {relative_gitdir}")).unwrap();

        // Should find main repo even with relative path
        let found = find_main_repo_from_worktree(&worktree);
        assert!(found.is_some());
        // Canonicalize both sides for comparison (resolves symlinks and ../)
        let found_canon = found.unwrap().canonicalize().unwrap();
        let expected_canon = main_repo.canonicalize().unwrap();
        assert_eq!(found_canon, expected_canon);
    }

    /// cas-b69a (GH #157): the notice must name BOTH roots and say which one
    /// won — that is the whole point, an operator reading it has to be able to
    /// tell that the store under their feet was not the one used.
    #[test]
    fn root_override_notice_names_both_roots_and_the_winner_cas_b69a() {
        let notice = root_override_notice(
            Path::new("/live/project/.cas"),
            Path::new("/tmp/casmig/proj/.cas"),
        )
        .expect("differing roots must produce a notice");

        assert!(notice.contains("/live/project/.cas"), "{notice}");
        assert!(notice.contains("/tmp/casmig/proj/.cas"), "{notice}");
        assert!(notice.contains("CAS_ROOT wins"), "{notice}");
        // The first line alone must carry both paths and the verdict: some
        // consoles show only the first stderr line before a wall of output.
        let first_line = notice.lines().next().unwrap();
        assert!(first_line.contains("/live/project/.cas"), "{first_line}");
        assert!(first_line.contains("/tmp/casmig/proj/.cas"), "{first_line}");
        assert!(first_line.contains("CAS_ROOT wins"), "{first_line}");
        // And it must point at the escape hatch, not just complain.
        assert!(notice.contains("env -u CAS_ROOT"), "{notice}");
    }

    /// No conflict, no noise: CAS_ROOT pointing at the very store the working
    /// directory would have found is the overwhelmingly common factory case.
    #[test]
    fn identical_roots_produce_no_notice_cas_b69a() {
        assert!(
            root_override_notice(Path::new("/project/.cas"), Path::new("/project/.cas")).is_none()
        );
    }

    /// Two spellings of the same directory are the same store, not a conflict —
    /// a false alarm here would be worse than silence, because it would teach
    /// people to ignore the real one.
    #[test]
    fn equivalent_paths_spelled_differently_produce_no_notice_cas_b69a() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("project");
        let cas_dir = init_cas_dir(&project).unwrap();
        let round_about = project.join("sub").join("..").join(".cas");
        std::fs::create_dir_all(project.join("sub")).unwrap();

        assert!(
            root_override_notice(&cas_dir, &round_about).is_none(),
            "'{}' and '{}' are the same directory",
            cas_dir.display(),
            round_about.display()
        );
    }

    /// cas-b69a: the split-out helper must resolve exactly what the old inline
    /// code did, and must ignore CAS_ROOT entirely — it is what names the loser.
    #[test]
    fn find_cas_root_ignoring_env_reports_the_cwd_derived_root_cas_b69a() {
        let temp = TempDir::new().unwrap();
        let copy = temp.path().join("copy");
        let live = temp.path().join("live");
        init_cas_dir(&copy).unwrap();
        init_cas_dir(&live).unwrap();

        let _env =
            TestEnvGuard::with_optional_vars(&[("CAS_ROOT", Some(live.join(".cas").to_str().unwrap()))]);

        // Precedence is unchanged: CAS_ROOT still wins the actual resolution...
        assert_eq!(
            find_cas_root_from(&copy).unwrap(),
            live.join(".cas"),
            "CAS_ROOT precedence must NOT change — factory workers depend on it"
        );
        // ...but the losing root is knowable, which is what the notice reports.
        assert_eq!(find_cas_root_ignoring_env(&copy).unwrap(), copy.join(".cas"));
        assert!(
            root_override_notice(&live.join(".cas"), &copy.join(".cas")).is_some(),
            "a cwd store different from CAS_ROOT must be reported"
        );
    }

    #[test]
    #[ignore] // Uses global state (CAS_ROOT env var) - run with: cargo test -- --ignored
    fn test_cas_root_env_var() {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();
        let _env =
            TestEnvGuard::with_optional_vars(&[("CAS_ROOT", Some(cas_dir.to_str().unwrap()))]);

        // find_cas_root should use CAS_ROOT
        let found = find_cas_root().unwrap();
        assert_eq!(found, cas_dir);

    }

    #[test]
    #[ignore] // Uses global state (CAS_ROOT env var, CWD) - run with: cargo test -- --ignored
    fn test_cas_root_env_var_invalid_path() {
        let temp = TempDir::new().unwrap();
        // Canonicalize to handle macOS /var -> /private/var symlinks
        let temp_path = temp
            .path()
            .canonicalize()
            .expect("Failed to canonicalize temp path");

        // Create a real .cas dir to fall back to
        init_cas_dir(&temp_path).unwrap();

        let mut env = TestEnvGuard::with_optional_vars(&[(
            "CAS_ROOT",
            Some("/nonexistent/path/that/does/not/exist"),
        )]);
        env.set_current_dir(&temp_path);

        // Should fall back to directory walk and find the real .cas
        let found = find_cas_root().unwrap();
        assert_eq!(found, temp_path.join(".cas"));

    }
}
