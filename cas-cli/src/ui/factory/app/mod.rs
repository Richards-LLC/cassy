//! Factory application state and orchestration

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use cas_mux::{Mux, PaneKind};
use chrono::{DateTime, Utc};
use ratatui::layout::Rect;

use super::director::{
    DiffLine, DirectorData, DirectorEvent, DirectorEventDetector, DirectorStores,
    MergeAlertFreshness, PanelAreas, Prompt, SidecarFocus, ViewMode, check_merge_alert_freshness,
    generate_prompt_at, prompt_is_still_deliverable, revalidate_event_for_delivery_with_context,
    revalidate_event_for_delivery_with_focus, supervisor_actionable_state,
};
use crate::store::open_prompt_queue_store;
use crate::types::Worktree;
use crate::ui::factory::buffer_backend::HyperlinkMap;
use crate::ui::factory::input::{InputMode, LayoutSizes};
use crate::ui::factory::layout::{FactoryLayout, PaneGrid};
use crate::ui::factory::notification::Notifier;
use crate::ui::factory::protocol::SessionMetadata;
use crate::ui::factory::session::metadata_path;
use crate::ui::theme::ActiveTheme;
use crate::ui::widgets::TreeItemType;
use crate::worktree::WorktreeManager;

mod branch_visibility;
mod imports;
mod init;
mod panels_and_modes;
pub(crate) mod render_and_ops;
mod sidecar_and_selection;

pub(crate) use branch_visibility::BranchAheadBehind;
pub(crate) use branch_visibility::truncate_branch_middle;
use branch_visibility::{
    BranchVisibilityCache, branch_for_worker_title, format_pane_title_with_branch,
};

/// Stamp the supervisor identity discovered by the trusted factory spawner.
///
/// This is deliberately outside every MCP/public registration path: the
/// factory owns the pane and its generated harness session id, so it can bind
/// that exact id to privileged supervisor authority without trusting a request
/// field or `CAS_AGENT_ROLE`. A later MCP auto-registration is idempotent and
/// preserves the stored role in `SqliteAgentStore::register`.
fn stamp_trusted_factory_supervisor(
    agent_store: &dyn crate::store::AgentStore,
    session_id: &str,
    supervisor_name: &str,
    factory_session: &str,
) -> anyhow::Result<()> {
    use crate::types::{Agent, AgentRole, AgentStatus};

    match agent_store.get(session_id) {
        Ok(mut agent) => {
            agent.name = supervisor_name.to_string();
            agent.role = AgentRole::Supervisor;
            agent.status = AgentStatus::Active;
            agent.factory_session = Some(factory_session.to_string());
            agent_store.update(&agent)?;
        }
        Err(_) => {
            let mut agent = Agent::new(session_id.to_string(), supervisor_name.to_string());
            agent.role = AgentRole::Supervisor;
            agent.factory_session = Some(factory_session.to_string());
            agent_store.register(&agent)?;
        }
    }
    Ok(())
}

// Re-export from cas-factory for backward compatibility
pub use cas_factory::{AutoPromptConfig, EpicState, FactoryConfig};

// Re-export scroll / click dispatch types so callers can use them without
// reaching into the private `sidecar_and_selection` submodule. Constants
// (SCROLL_*_ARROWS / SCROLL_*_SGR / SCROLL_LINES) and
// `alt_screen_wheel_bytes` / `sgr_left_click_bytes` remain reachable via this
// module's children for tests; production daemon code uses
// `FactoryApp::alt_screen_scroll_payload` and `ClickAction` +
// `sgr_left_click_bytes` for cas-7f6f Stop-click forwarding.
pub use sidecar_and_selection::{
    ClickAction, ClientGeometryMode, GrokEscAction, ScrollAction, sgr_left_click_bytes,
};

/// Booting state for a worker that is being spawned (after prepare, before finish)
#[derive(Debug, Clone)]
pub struct PendingWorkerState {
    /// Worker name
    pub name: String,
    /// When this worker entered the pending state
    pub started_at: Instant,
    /// Whether this spawn is using worktree isolation
    pub isolate: bool,
}

/// Worktree preparation data (can be sent to background thread)
pub struct WorktreePrep {
    pub worktree_path: PathBuf,
    pub branch_name: String,
    /// Logical parent of this worker's branch — always a *local* branch name,
    /// because it doubles as the merge-back target.
    pub parent_branch: String,
    /// cas-d897 (GH #146): the commit-ish the worktree is actually cut from,
    /// when it must differ from `parent_branch`. Set when `origin/<parent>` is
    /// strictly ahead of the stale local ref, so the worker starts on the
    /// fetched tip instead of silently building on old history. `None` means
    /// "cut from `parent_branch`", the ordinary case.
    pub base_ref: Option<String>,
    pub repo_root: PathBuf,
    pub cas_dir: PathBuf,
}

/// Data needed to spawn a worker (phase 1 output, can be sent to background thread)
pub struct WorkerSpawnPrep {
    pub worker_name: String,
    pub worktree_info: Option<WorktreePrep>,
    /// cas-ecf7 (GH #118): spawn-time warnings about the resolved base ref
    /// (stale relative to trunk/remote, or not containing the focused epic).
    /// The daemon drains these into the spawn audit trail and the supervisor's
    /// inbox so a worker cut from old history is never a silent event.
    pub warnings: Vec<String>,
    /// cas-7587 (GH #122): one line naming the resolved worktree base and why
    /// it won (task's epic branch / pinned focus / trunk). Informational, not
    /// an error: the daemon writes it to the spawn audit trail so a supervisor
    /// can always see which epic's history a worker was cut from. `None` for
    /// non-isolated spawns (no worktree, no base to resolve).
    pub base_provenance: Option<String>,
}

/// Result of background worktree preparation (phase 2 output)
pub struct WorkerSpawnResult {
    pub worker_name: String,
    pub cwd: PathBuf,
    pub cas_root: Option<PathBuf>,
    pub worktree: Option<Worktree>,
    /// True only when this spawn invocation created the worktree on disk.
    /// Cancellation cleanup must not delete a pre-existing reused worktree.
    pub worktree_created: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct TargetSeedStats {
    files: u64,
    bytes: u64,
}

/// Seed a new worker's private Cargo target from the immutable baseline named
/// by `.cas/build-cache/current`.
///
/// The refresh script builds the named snapshot to completion before
/// atomically publishing the pointer, so this never clones another worker's
/// live target. A temporary sibling also keeps failed seeds from leaving a
/// partial `target/` that Cargo could mistake as valid.
fn seed_worker_target_from_baseline(
    cas_dir: &Path,
    worktree_path: &Path,
) -> anyhow::Result<Option<TargetSeedStats>> {
    let pointer = cas_dir.join("build-cache").join("current");
    let Ok(snapshot_name) = std::fs::read_to_string(&pointer) else {
        return Ok(None);
    };
    let snapshot_name = snapshot_name.trim();
    let mut components = Path::new(snapshot_name).components();
    let safe_name = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    if snapshot_name.is_empty() || !safe_name {
        anyhow::bail!(
            "invalid worker build-cache pointer {}: expected one snapshot directory name",
            pointer.display()
        );
    }

    let source = cas_dir
        .join("build-cache")
        .join("snapshots")
        .join(snapshot_name);
    if !source.is_dir() {
        anyhow::bail!(
            "worker build-cache snapshot '{}' does not exist at {}",
            snapshot_name,
            source.display()
        );
    }

    let target = worktree_path.join("target");
    if target.exists() {
        return Ok(None);
    }
    let staging = worktree_path.join(".target-seed-in-progress");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }

    let mut stats = TargetSeedStats::default();
    if let Err(error) = hardlink_seed_tree(&source, &staging, &target, &mut stats) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    std::fs::rename(&staging, &target)?;
    Ok(Some(stats))
}

fn hardlink_seed_tree(
    source: &Path,
    destination: &Path,
    published_destination: &Path,
    stats: &mut TargetSeedStats,
) -> anyhow::Result<()> {
    hardlink_seed_tree_inner(source, destination, source, published_destination, stats)
}

fn hardlink_seed_tree_inner(
    source: &Path,
    destination: &Path,
    source_root: &Path,
    destination_root: &Path,
    stats: &mut TargetSeedStats,
) -> anyhow::Result<()> {
    std::fs::create_dir(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let destination_entry = destination.join(entry.file_name());
        if file_type.is_dir() {
            hardlink_seed_tree_inner(
                &entry.path(),
                &destination_entry,
                source_root,
                destination_root,
                stats,
            )?;
        } else if file_type.is_file() {
            // Cargo coordinates writes through target/**/.cargo-lock.  A
            // hardlink would make independently seeded worker targets share
            // that inode, serializing their otherwise private builds. Cargo
            // recreates the lock file on first use, so omit it at every depth.
            if entry.file_name() == ".cargo-lock" {
                continue;
            }
            let metadata = entry.metadata()?;
            if entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "d")
            {
                let dep_info = std::fs::read_to_string(entry.path())?;
                let rebased = dep_info.replace(
                    &source_root.to_string_lossy().to_string(),
                    &destination_root.to_string_lossy(),
                );
                std::fs::write(&destination_entry, rebased)?;
            } else {
                std::fs::hard_link(entry.path(), &destination_entry)?;
            }
            stats.files += 1;
            stats.bytes += metadata.len();
        } else if file_type.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(entry.path())?, destination_entry)?;
            #[cfg(not(unix))]
            anyhow::bail!(
                "worker target cache contains a symlink unsupported on this platform: {}",
                entry.path().display()
            );
        }
    }
    Ok(())
}

impl WorkerSpawnPrep {
    /// Phase 2: Run the slow git operations (designed for spawn_blocking).
    pub fn run(self) -> anyhow::Result<WorkerSpawnResult> {
        if let Some(wt) = self.worktree_info {
            use crate::worktree::GitOperations;

            let git = GitOperations::new(wt.repo_root.clone());

            // Check if worktree already exists on disk (reuse from previous session)
            if wt.worktree_path.exists() {
                // STEP 2 (cas-5232): Hard-error — validate that the existing path is a
                // *properly registered* git worktree on the expected branch before reusing
                // it.  Without this check a stale directory (left by a partial failed
                // `git worktree add`, or by a previous non-isolated session that happened
                // to create a same-named subdirectory) would pass the `exists()` test and
                // be returned as the worker's cwd.  Because the directory has no `.git`
                // file, `git` would then traverse upward to the main checkout and commit
                // on whatever branch `HEAD` points at there — typically `main`.
                //
                // The check runs `git rev-parse --abbrev-ref HEAD` in the worktree
                // directory itself (not in wt.repo_root).  In a valid worktree the
                // answer is the worktree's branch; in a plain directory git climbs to
                // the nearest ancestor repo and reports its HEAD instead.
                //
                // cas-30c6: the branch answer alone is forgeable. When the shared
                // checkout is itself parked on `factory/<name>` (the GH #120 /
                // cas-5bef shape), a stale plain directory *matches* the expected
                // branch while every git operation in it resolves to that shared
                // checkout — the exact misbinding of spawn request 1031. Prove the
                // full canonical binding (working-tree root AND branch) first and
                // fail the spawn closed, so no harness is ever started on it.
                crate::factory_isolation::verify_worker_worktree_binding(
                    &self.worker_name,
                    &wt.worktree_path,
                    Some(&wt.branch_name),
                )?;
                let wt_git = GitOperations::new(wt.worktree_path.clone());
                match wt_git.current_branch() {
                    Ok(ref actual_branch) if actual_branch == &wt.branch_name => {
                        // Valid — correct branch confirmed.
                        tracing::info!(
                            worker = %self.worker_name,
                            cwd = %wt.worktree_path.display(),
                            branch = %actual_branch,
                            reused = true,
                            "spawn prep: worktree validated on correct branch (reuse path)"
                        );
                    }
                    Ok(actual_branch) => {
                        anyhow::bail!(
                            "Worker '{}': worktree {:?} exists but git reports branch '{}' \
                             (expected '{}'). Stale directory or branch mismatch — \
                             remove {:?} and retry.",
                            self.worker_name,
                            wt.worktree_path,
                            actual_branch,
                            wt.branch_name,
                            wt.worktree_path,
                        );
                    }
                    Err(e) => {
                        anyhow::bail!(
                            "Worker '{}': path {:?} exists but is not a valid git worktree \
                             (git current_branch failed: {}). Remove {:?} and retry.",
                            self.worker_name,
                            wt.worktree_path,
                            e,
                            wt.worktree_path,
                        );
                    }
                }

                let _ = git.init_submodules(&wt.worktree_path);
                // Ensure gitignored config is available (may be missing from prior run)
                crate::worktree::symlink_project_config(&wt.repo_root, &wt.worktree_path);
                // (Re-)install the pre-commit guard on reuse — the hook may have been
                // removed if the main repo was cloned fresh (cas-bea2 LAYER 2).
                if let Err(e) = crate::ui::factory::daemon::runtime::teams::TeamsManager::install_worker_pre_commit_hook(&wt.worktree_path) {
                    tracing::warn!("Failed to install worker pre-commit guard on reuse: {e}");
                }
                let worktree = Worktree::new(
                    Worktree::generate_id(),
                    wt.branch_name,
                    wt.parent_branch,
                    wt.worktree_path.clone(),
                );
                return Ok(WorkerSpawnResult {
                    worker_name: self.worker_name,
                    cwd: wt.worktree_path,
                    cas_root: Some(wt.cas_dir),
                    worktree: Some(worktree),
                    worktree_created: false,
                });
            }

            // Create parent directory
            if let Some(parent) = wt.worktree_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            // Create git worktree (THE SLOW PART).
            // cas-d897 (GH #146): cut from `base_ref` when spawn resolution
            // found a fresher commit than the local parent branch's ref.
            let checkout_from = wt
                .base_ref
                .clone()
                .unwrap_or_else(|| wt.parent_branch.clone());
            git.create_worktree(&wt.worktree_path, &wt.branch_name, Some(&checkout_from))?;

            if std::env::var("CAS_FACTORY_DISABLE_TARGET_SEED").as_deref() != Ok("1") {
                // A cross-repository worker keeps the session store as
                // `CAS_ROOT`, but its Cargo target must come from the target
                // repository's cache. Seeding the session cache here could
                // copy artifacts built for an unrelated project into the
                // target worktree.
                let build_cache_cas_dir = wt.repo_root.join(".cas");
                match seed_worker_target_from_baseline(&build_cache_cas_dir, &wt.worktree_path) {
                    Ok(Some(stats)) => tracing::info!(
                        worker = %self.worker_name,
                        files = stats.files,
                        bytes = stats.bytes,
                        "spawn prep: seeded private Cargo target from quiescent baseline"
                    ),
                    Ok(None) => tracing::debug!(
                        worker = %self.worker_name,
                        "spawn prep: no worker Cargo target baseline available"
                    ),
                    Err(error) => tracing::warn!(
                        worker = %self.worker_name,
                        error = %error,
                        "spawn prep: worker Cargo target seed skipped"
                    ),
                }
            }

            // STEP 1 (cas-5232): Log the resolved cwd immediately after worktree creation
            // so the daemon trace contains a clear record of which path each worker got.
            tracing::info!(
                worker = %self.worker_name,
                cwd = %wt.worktree_path.display(),
                branch = %wt.branch_name,
                parent = %wt.parent_branch,
                cut_from = %checkout_from,
                reused = false,
                "spawn prep: new worktree created — cwd resolved"
            );

            // Symlink .mcp.json and .claude/ so workers get MCP access
            crate::worktree::symlink_project_config(&wt.repo_root, &wt.worktree_path);

            // Install pre-commit guard (cas-bea2 LAYER 2) — hard backstop that
            // blocks commits on protected branches even via raw `git` invocations
            // that bypass the PreToolUse hook. Non-fatal: LAYER 1 + LAYER 3 cover
            // the model-visible and SessionStart paths.
            if let Err(e) = crate::ui::factory::daemon::runtime::teams::TeamsManager::install_worker_pre_commit_hook(&wt.worktree_path) {
                tracing::warn!("Failed to install worker pre-commit guard: {e}");
            }

            let worktree = Worktree::new(
                Worktree::generate_id(),
                wt.branch_name,
                wt.parent_branch,
                wt.worktree_path.clone(),
            );

            Ok(WorkerSpawnResult {
                worker_name: self.worker_name,
                cwd: wt.worktree_path,
                cas_root: Some(wt.cas_dir),
                worktree: Some(worktree),
                worktree_created: true,
            })
        } else {
            // Non-isolated worker: cwd is wherever the daemon process is running.
            // STEP 1 (cas-5232): Log so this path is distinguishable from the
            // isolated paths in the trace.
            let cwd = std::env::current_dir()?;
            tracing::info!(
                worker = %self.worker_name,
                cwd = %cwd.display(),
                isolated = false,
                "spawn prep: non-isolated worker — sharing process cwd"
            );
            Ok(WorkerSpawnResult {
                worker_name: self.worker_name,
                cwd,
                cas_root: None,
                worktree: None,
                worktree_created: false,
            })
        }
    }
}

/// Post-spawn branch assertion for isolated workers (STEP 3, cas-5232).
///
/// Runs `git rev-parse --abbrev-ref HEAD` in the worker's assigned `cwd` and
/// verifies the result matches `expected_branch`.  If the cwd is the main
/// checkout (e.g., the supervisor's working tree) rather than the worker's
/// dedicated git worktree, git will report the *main checkout's* current branch
/// — not `factory/<name>` — and this function returns an error.  That lets the
/// daemon surface the bug before the worker has a chance to commit to the wrong
/// ref.
///
/// Called from `finish_worker_spawn` right after `mux.add_worker` succeeds so
/// we detect the bad cwd while the spawn is still observable.
pub(crate) fn verify_isolated_worker_branch(
    worker_name: &str,
    cwd: &std::path::Path,
    expected_branch: &str,
) -> anyhow::Result<()> {
    use crate::worktree::GitOperations;
    let git = GitOperations::new(cwd.to_path_buf());
    match git.current_branch() {
        Ok(actual_branch) => {
            if actual_branch == expected_branch {
                tracing::info!(
                    worker = %worker_name,
                    branch = %actual_branch,
                    cwd = %cwd.display(),
                    "post-spawn: worker cwd verified on correct branch"
                );
                Ok(())
            } else {
                // The cwd resolves to the wrong branch.  Most likely causes:
                //   (a) cmd.cwd() was applied to the wrong path,
                //   (b) the worktree directory exists but is not a registered git
                //       worktree (stale dir) and git traversed up to the main repo,
                //   (c) a Teams config.json cwd race caused Claude Code to adopt the
                //       supervisor's project directory.
                // See EPIC cas-073f for the full investigation.
                anyhow::bail!(
                    "ISOLATION BUG: worker '{}' cwd {:?} resolved to branch '{}' \
                     (expected '{}'). Worker will commit to the wrong ref. \
                     See EPIC cas-073f.",
                    worker_name,
                    cwd,
                    actual_branch,
                    expected_branch
                )
            }
        }
        Err(e) => anyhow::bail!(
            "post-spawn: could not verify git branch for worker '{}' in {:?}: {}",
            worker_name,
            cwd,
            e
        ),
    }
}

/// The main factory application
pub struct FactoryApp {
    /// The terminal multiplexer
    pub mux: Mux,
    /// Cassy directory for data loading
    cas_dir: PathBuf,
    /// Cached store handles (avoid re-opening on every 2s refresh)
    director_stores: Option<DirectorStores>,
    /// Director panel data. **Mutated in place** by
    /// `filter_director_agents_to_current_session()` on every `db_changed`
    /// tick — scoped to the currently-tracked epic for TUI display purposes.
    /// Do NOT feed this to `DirectorEventDetector::detect_changes` (see
    /// `unfiltered_director_data`).
    director_data: DirectorData,
    /// Canonical, NEVER-epic-filtered snapshot of the same load that seeds
    /// `director_data`. Refreshed alongside `director_data` on every
    /// `db_changed` tick but never mutated by the epic-scoping filter.
    ///
    /// Exists because `filter_director_agents_to_current_session` mutates
    /// `director_data` in place, and on a subsequent tick where `db_changed`
    /// is false (nothing changed in the DB) but `git_due` is true,
    /// `director_data` is NOT reloaded — it stays as the FILTERED leftover
    /// from the last `db_changed` tick. Feeding that stale-filtered snapshot
    /// into `detect_changes` makes every task belonging to a
    /// currently-untracked epic (a second epic being worked concurrently in
    /// the same session) look like it "disappeared from active sets",
    /// firing a fabricated `TaskCompleted` — the director broadcasts "has
    /// closed task X" for a task that never closed (cas-dbbe). Change
    /// detection and the `TaskCompleted` render-time safety net
    /// (`generate_prompt`, cas-6aaf) must always read the true, unfiltered
    /// task state via this field instead.
    unfiltered_director_data: DirectorData,
    /// Wall-clock instant `director_data` was last actually read from the task
    /// store (cas-ae6d / GH #100).
    ///
    /// `director_data` is only reloaded on a `db_changed` tick, so "now" is not
    /// a safe stand-in for its age. Notices that assert an ABSENCE — "no
    /// dispatchable tasks" — quote this instant so the claim is attributable to
    /// a real read instead of to render time.
    director_data_loaded_at: chrono::DateTime<chrono::Utc>,
    /// Current input mode
    pub input_mode: InputMode,
    /// Buffer for inject mode text input
    pub inject_buffer: String,
    /// Target pane for injection
    pub inject_target: Option<String>,
    /// Show help overlay
    pub show_help: bool,
    /// Show file changes dialog
    pub show_changes_dialog: bool,
    /// Selected file for changes dialog (source_path, file_path, source_name, agent_name)
    pub changes_dialog_file: Option<(PathBuf, String, String, Option<String>)>,
    /// Scroll offset for changes dialog diff
    pub changes_dialog_scroll: u16,
    /// Cached diff lines for changes dialog
    pub changes_dialog_diff: Vec<DiffLine>,
    /// Show task detail dialog
    pub show_task_dialog: bool,
    /// Selected task ID for task dialog
    pub task_dialog_id: Option<String>,
    /// Scroll offset for task dialog
    pub task_dialog_scroll: u16,
    /// Max scroll offset for task dialog (computed during render)
    pub task_dialog_max_scroll: u16,
    /// Show reminder detail dialog
    pub show_reminder_dialog: bool,
    /// Selected reminder index for reminder dialog
    pub reminder_dialog_idx: Option<usize>,
    /// Scroll offset for reminder dialog
    pub reminder_dialog_scroll: u16,
    /// Show terminal dialog (interactive shell)
    pub show_terminal_dialog: bool,
    /// Name of the shell pane in the mux
    pub terminal_pane_name: Option<String>,
    /// Show feedback dialog
    pub show_feedback_dialog: bool,
    /// Current feedback category
    pub feedback_category: super::input::FeedbackCategory,
    /// Feedback text buffer
    pub feedback_buffer: String,
    /// Last Cassy data refresh time
    last_refresh: Instant,
    /// Refresh interval for Cassy data
    refresh_interval: Duration,
    /// Last observed Cassy DB file fingerprint used for cheap change detection
    last_db_fingerprint: Option<CasDbFingerprint>,
    /// Last git refresh time
    last_git_refresh: Instant,
    /// Interval for expensive git refresh operations
    git_refresh_interval: Duration,
    /// Cached branch labels and epic ahead/behind status for render paths.
    branch_visibility: BranchVisibilityCache,
    /// Theme for rendering
    theme: ActiveTheme,
    /// Worker names (for reference)
    worker_names: Vec<String>,
    /// Supervisor name (for reference)
    supervisor_name: String,
    /// Factory session name (for prompt queue isolation)
    factory_session: Option<String>,
    /// Factory session creation timestamp for elapsed-time display.
    session_created_at: Option<DateTime<Utc>>,
    /// Silence threshold before supervisor-owned forward motion becomes a stall.
    supervisor_stall_after_secs: u64,
    /// Supervisor CLI mode (claude/codex)
    supervisor_cli: cas_mux::SupervisorCli,
    /// Worker CLI mode (claude/codex)
    worker_cli: cas_mux::SupervisorCli,
    /// Error message to display (cleared on next key or after timeout)
    pub error_message: Option<String>,
    /// Number of workers currently being spawned (for loading indicator)
    pub spawning_count: usize,
    /// SELECT mode: client has disabled mouse capture so native terminal
    /// text selection works. Set via F10 toggle on the client.
    pub select_mode: bool,
    /// Workers currently in the spawning pipeline (after prepare, before finish).
    /// These appear as booting panes in the layout.
    pub pending_workers: Vec<PendingWorkerState>,
    /// When the error message was set (for auto-dismiss)
    error_set_at: Option<Instant>,
    /// Sidebar collapsed state
    pub sidecar_collapsed: bool,
    /// Worktree manager for worker isolation (None if worktrees disabled)
    worktree_manager: Option<WorktreeManager>,
    /// Index of the currently selected worker tab (0-based, used in tabbed mode)
    pub selected_worker_tab: usize,
    /// Use tabbed worker view instead of side-by-side (config preference)
    tabbed_workers: bool,
    /// Actual tabbed mode active (accounts for auto-switch due to space constraints)
    is_tabbed: bool,
    /// Custom layout percentages (None = use defaults)
    pub layout_sizes: Option<LayoutSizes>,
    /// Spatial grid for pane navigation (rebuilt on layout change)
    pane_grid: PaneGrid,
    /// Currently selected pane in pane select mode
    selected_pane: Option<String>,
    /// Event detector for Cassy state changes
    event_detector: DirectorEventDetector,
    /// Notification manager
    notifier: Notifier,
    /// Current epic state
    epic_state: EpicState,
    /// Explicit current epic ID — set when supervisor creates/starts an epic.
    /// Takes priority over passive scanning in detect_epic_state().
    current_epic_id: Option<String>,
    /// Source of the current epic focus. Renderers stay source-blind; this is
    /// used while resolving focus so only inference-derived epics are gated.
    current_epic_source: Option<EpicFocusSource>,
    /// Sidecar panel focus
    pub sidecar_focus: SidecarFocus,
    /// Sidecar panel scroll/collapse state
    pub panels: super::director::PanelRegistry,
    /// Panel areas for click detection (updated during render)
    panel_areas: PanelAreas,
    /// Current view mode for the sidecar
    pub view_mode: ViewMode,
    /// Scroll offset for detail views
    detail_scroll: u16,
    /// Agent filter (None = show all)
    pub agent_filter: Option<String>,
    /// Cached diff content for FileDiff view (used by search)
    diff_cache: Vec<DiffLine>,
    /// Scroll offset for diff view (legacy, used by search jump)
    diff_scroll: u16,
    /// Parsed diff metadata for DiffWidget rendering
    diff_metadata: Option<cas_diffs::FileDiffMetadata>,
    /// Syntax highlighter for diff rendering
    syntax_highlighter: cas_diffs::highlight::SyntaxHighlighter,
    /// Scroll/hunk navigation state for DiffWidget
    diff_view_state: cas_diffs::widget::DiffViewState,
    /// Diff display style (unified vs split)
    diff_display_style: cas_diffs::iter::DiffStyle,
    /// Inline diff highlighting mode
    diff_inline_mode: cas_diffs::LineDiffType,
    /// Whether to show line numbers in diff view
    diff_show_line_numbers: bool,
    /// Expanded hunk regions (for expanding collapsed context)
    diff_expanded_hunks: std::collections::HashMap<usize, cas_diffs::iter::HunkExpansionRegion>,
    /// Whether all collapsed regions are expanded
    diff_expand_all: bool,
    /// Whether diff search input mode is active
    diff_search_mode: bool,
    /// Current diff search query
    diff_search_query: String,
    /// Line indices that match the search query
    diff_search_matches: Vec<usize>,
    /// Current match index (for n/N navigation)
    diff_search_current: usize,
    /// Collapsed epic IDs (epics whose subtasks are hidden)
    collapsed_epics: HashSet<String>,
    /// Collapsed directory paths in changes panel
    collapsed_dirs: HashSet<String>,
    /// Tree item types for changes panel (for scroll bounds)
    changes_item_types: Vec<TreeItemType>,
    /// Layout areas for click detection (updated during render)
    worker_tab_bar_area: Option<Rect>,
    worker_content_area: Option<Rect>,
    worker_areas: Vec<Rect>,
    supervisor_area: Option<Rect>,
    sidecar_area: Option<Rect>,
    /// Full-mode PTY content rects (bordered inners), updated by `render`.
    /// Kept separate from compact so simultaneous full+compact clients do not
    /// clobber each other (cas-7f6f).
    full_pty_content_areas: HashMap<String, Rect>,
    /// Compact-mode PTY content rects (borderless supervisor content), updated
    /// by `render_compact` only.
    compact_pty_content_areas: HashMap<String, Rect>,
    /// Stored terminal dimensions (for daemon mode where crossterm::terminal::size() doesn't work)
    terminal_cols: u16,
    terminal_rows: u16,
    /// Auto-prompting configuration
    auto_prompt: AutoPromptConfig,
    /// Epic branch name (e.g., "epic/add-user-auth") - workers branch from this
    epic_branch: Option<String>,
    /// Whether recording is enabled for this session
    record_enabled: bool,
    /// Session ID for recordings (only set if record_enabled)
    recording_session_id: Option<String>,
    /// When recording started (for computing event timestamps)
    recording_start: Option<Instant>,
    /// Collected events during this session (for export)
    recorded_events: Vec<(Instant, DirectorEvent)>,
    /// UUID for the team lead's Claude Code session (for Teams config.json)
    lead_session_id: Option<String>,
    /// Project directory (for git operations)
    project_dir: PathBuf,
    /// Session ID to pane name mapping for interaction routing
    session_to_pane: HashMap<String, String>,
    /// Last time Ctrl+C was sent to a pane (debounce rapid repeated presses)
    pub last_interrupt_time: Option<Instant>,
    /// Top-level view mode (Panes vs Mission Control)
    pub factory_view_mode: crate::ui::factory::renderer::FactoryViewMode,
    /// Which panel has focus in Mission Control view
    pub mc_focus: crate::ui::factory::renderer::MissionControlFocus,
    /// Mission Control panel areas for click detection (updated during render)
    mc_workers_area: Rect,
    mc_tasks_area: Rect,
    mc_changes_area: Rect,
    mc_activity_area: Rect,
    /// Per-frame OSC 8 hyperlink metadata for the full terminal pipeline.
    full_pane_hyperlinks: HyperlinkMap,
    /// Per-frame OSC 8 hyperlink metadata for the compact terminal pipeline.
    compact_pane_hyperlinks: HyperlinkMap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CasDbFingerprint {
    db_mtime: Option<SystemTime>,
    wal_mtime: Option<SystemTime>,
}

impl CasDbFingerprint {
    fn from_cas_dir(cas_dir: &std::path::Path) -> Self {
        let db_path = cas_dir.join("cas.db");
        let wal_path = cas_dir.join("cas.db-wal");

        Self {
            db_mtime: file_mtime(&db_path),
            wal_mtime: file_mtime(&wal_path),
        }
    }
}

fn file_mtime(path: &std::path::Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

pub(crate) use crate::mcp::tools::epic_branch_name;

/// Resolve the branch recorded on the active epic task.
///
/// The task store is authoritative because an operator may deliberately move
/// or rename an epic branch after creation (stacked epics are one example).
/// Title-derived naming remains only as a compatibility fallback for legacy
/// epic summaries that predate persisted `branch` metadata.
pub(crate) fn epic_branch_for_state(data: &DirectorData, state: &EpicState) -> Option<String> {
    let EpicState::Active {
        epic_id,
        epic_title,
    } = state
    else {
        return None;
    };

    data.epic_tasks
        .iter()
        .find(|epic| epic.id == *epic_id)
        .and_then(|epic| epic.branch.clone())
        .or_else(|| Some(epic_branch_name(epic_title, epic_id)))
}

/// cas-889d / cas-9eae: determine whether a task belongs to the current
/// factory session for director visibility purposes (i.e. whether it
/// should remain in the `ready_tasks`/`in_progress_tasks` buckets the
/// event detector watches for disappearance).
///
/// A task is visible when either:
///   - its `epic` field matches the currently-tracked `epic_id`, OR
///   - it has no epic link yet (a read race between the task-list and
///     dependency-list queries — a newly created task may not yet have
///     its parent-child dependency visible) AND its assignee is a
///     current-session worker, checked by BOTH display name
///     (`allowed_names`) and session ID (`allowed_session_ids`).
///
/// The dual assignee check exists because `Task.assignee` is always a
/// session ID in the DB, but some construction paths (tests, legacy
/// manual assignment) use the display name. Dropping a genuinely
/// in-progress task from this view — because only one of the two
/// representations was checked — causes the event detector
/// (`DirectorEventDetector::detect_changes_at`) to see it "disappear"
/// from the tracked active sets and fire a fabricated `TaskCompleted`,
/// which the director then broadcasts as "has closed task X" even though
/// the task's real status never left `InProgress`. This is the standalone,
/// directly-testable form of the predicate used by
/// `filter_director_agents_to_current_session` — extracted so the
/// invariant that ultimately gates the "has closed task" broadcast has
/// unit coverage without constructing a full `FactoryApp`.
pub(crate) fn task_belongs_to_current_session(
    task: &cas_factory::TaskSummary,
    epic_id: &str,
    allowed_names: &std::collections::HashSet<String>,
    allowed_session_ids: &std::collections::HashSet<String>,
) -> bool {
    task.epic.as_deref() == Some(epic_id)
        || (task.epic.is_none()
            && task
                .assignee
                .as_ref()
                .is_some_and(|a| allowed_names.contains(a) || allowed_session_ids.contains(a)))
}

impl FactoryApp {
    fn filter_director_agents_to_current_session(&mut self) {
        let mut allowed = std::collections::HashSet::with_capacity(self.worker_names.len() + 1);
        for name in &self.worker_names {
            allowed.insert(name.clone());
        }
        allowed.insert(self.supervisor_name.clone());

        self.director_data
            .agents
            .retain(|agent| allowed.contains(&agent.name));
        self.director_data
            .agent_id_to_name
            .retain(|_, name| allowed.contains(name));

        // Filter tasks to active epic's subtasks only (prevents cross-project task leakage).
        // Also keep tasks assigned to current-session workers that have no epic link yet —
        // there's a read race between task list and dependency list queries where a newly
        // created task may not yet have its parent-child dependency visible, causing its
        // `epic` field to be `None`. Dropping those tasks causes the panel to flash empty.
        //
        // cas-889d: `agent_id_to_name` (already filtered to current-session agents above)
        // maps session-ID → name. Task `assignee` fields are always session IDs (the
        // canonical DB key). Checking `allowed.contains(a)` only accepted display-name
        // assignees (which are never stored in the DB), so tasks with session-ID assignees
        // were dropped even when the worker was actively working, causing the event
        // detector to see them "disappear" from the snapshot and fire fabricated
        // TaskCompleted notices. Collect the current-session session IDs from the already-
        // filtered map and accept them as well.
        if let Some(epic_id) = self.epic_state.epic_id() {
            let epic_id = epic_id.to_string();
            // Collect session IDs of current-session agents (agent_id_to_name is already
            // filtered above to only contain current-session entries).
            let allowed_session_ids: std::collections::HashSet<String> = self
                .director_data
                .agent_id_to_name
                .keys()
                .cloned()
                .collect();
            let belongs_to_session = |t: &cas_factory::TaskSummary| -> bool {
                task_belongs_to_current_session(t, &epic_id, &allowed, &allowed_session_ids)
            };
            self.director_data
                .ready_tasks
                .retain(|t| belongs_to_session(t));
            self.director_data
                .in_progress_tasks
                .retain(|t| belongs_to_session(t));
            self.director_data.epic_tasks.retain(|t| t.id == epic_id);
        }
    }

    /// Check if we should refresh Cassy data
    pub fn should_refresh(&self) -> bool {
        self.last_refresh.elapsed() >= self.refresh_interval
    }

    /// Check if automatic prompting is globally enabled
    pub fn auto_prompt_enabled(&self) -> bool {
        self.auto_prompt.enabled
    }

    /// Get the auto-prompt configuration
    pub fn auto_prompt_config(&self) -> &AutoPromptConfig {
        &self.auto_prompt
    }

    /// Revalidate detected events against a fresh, delivery-time snapshot and
    /// generate prompts from the survivors — in one combined pass.
    ///
    /// cas-627f: `revalidate_events_for_delivery` and
    /// `generate_prompts_for_delivery` used to be two separate public
    /// methods, each independently calling
    /// `load_unfiltered_director_data_for_delivery` — a full
    /// `DirectorData::load_with_stores` (all tasks + parent deps, 50 recent
    /// events, every agent's leases). The daemon tick
    /// (`daemon/runtime/lifecycle.rs`) called both back-to-back on EVERY
    /// `refresh_interval` (2s) tick, even when `events` was empty — a
    /// regression from main, where an idle tick performed zero extra DB
    /// loads. Confirmed P1 (docs/reviews/2026-07-07-cas-b646-epic.md). Now:
    /// short-circuit before touching the DB when there is nothing to
    /// revalidate, and share a single load between revalidation and prompt
    /// generation so the two steps can never observe two different
    /// snapshots.
    ///
    /// cas-ed6c: also returns the authoritative `unfiltered_data` snapshot
    /// this tick loaded (`None` on the empty-events short-circuit or store
    /// load failure) so the caller can
    /// run `TeamsManager::prune_stale_idle_alerts` against the SAME
    /// snapshot instead of triggering a second full `DirectorData` reload
    /// just for the sweep — reusing this tick's data, not adding a new
    /// DB-load path back into the empty-tick case cas-627f eliminated.
    pub fn revalidate_and_prompt_for_delivery(
        &mut self,
        events: &[DirectorEvent],
    ) -> (Vec<DirectorEvent>, Vec<Prompt>, Option<DirectorData>) {
        if events.is_empty() {
            return (Vec::new(), Vec::new(), None);
        }

        let loaded_data = self.try_load_unfiltered_director_data_for_delivery();
        // cas-ae6d (GH #100): the instant the snapshot backing a "no
        // dispatchable tasks" claim was actually read. That claim is counted
        // from `director_data` (session/epic-scoped — see the comment on
        // `dispatchable_ready_count`'s call site), which is reloaded only on a
        // `db_changed` tick, so this is deliberately NOT `Utc::now()`: stamping
        // render time would assert a freshness the data may not have.
        let snapshot_at = self.director_data_loaded_at;
        let delivery_state_is_authoritative = loaded_data.is_ok();
        let unfiltered_data = loaded_data.unwrap_or_else(|_| self.unfiltered_director_data.clone());

        // cas-9fff: pass session-focused epic so EpicAllSubtasksClosed can
        // use session-affinity routing (not just epic_verification_owner).
        let focused_epic_id = self.current_epic_id.as_deref();
        let delivery_events: Vec<DirectorEvent> = events
            .iter()
            .filter_map(|event| {
                // cas-06ca: an unavailable store snapshot is uncertainty,
                // never positive evidence that an epic-completion occurrence
                // is stale. Preserve the event rather than suppressing it
                // against cached data that may itself be out of date.
                if !delivery_state_is_authoritative
                    && matches!(event, DirectorEvent::EpicAllSubtasksClosed { .. })
                {
                    return Some(event.clone());
                }
                if let DirectorEvent::WorkerIdle { worker, .. } = event {
                    revalidate_event_for_delivery_with_context(
                        event,
                        &unfiltered_data,
                        &self.supervisor_name,
                        focused_epic_id,
                        self.event_detector.worker_idle_since(worker),
                    )
                } else if matches!(event, DirectorEvent::SupervisorStalled { .. }) {
                    self.supervisor_stall_event_is_current(event, &unfiltered_data)
                        .then(|| event.clone())
                } else {
                    revalidate_event_for_delivery_with_focus(
                        event,
                        &unfiltered_data,
                        &self.supervisor_name,
                        focused_epic_id,
                    )
                }
            })
            .collect();

        // cas-09d0: tasks that are Open but have an unmet `Blocks` dependency
        // must not be counted as "dispatchable" — see `compute_gated_task_ids`.
        let non_closed_ids = non_closed_task_ids(&unfiltered_data);
        let blocks_deps = self
            .director_stores
            .as_ref()
            .and_then(|s| {
                cas_store::TaskStore::list_dependencies(
                    &s.task_store,
                    Some(cas_types::DependencyType::Blocks),
                )
                .ok()
            })
            .unwrap_or_default();
        let gated_task_ids =
            crate::ui::factory::director::compute_gated_task_ids(&non_closed_ids, &blocks_deps);

        // cas-6883: repo root for the send-time MERGE REQUIRED freshness
        // check — same derivation as `close_project_root` in close_ops.rs /
        // `repo_root` in `director.rs::load_all_git_changes` (`.cas`'s
        // parent is the main checkout all `factory/*` and `epic/*` branches
        // live in).
        let repo_root = self.cas_dir.parent().unwrap_or(&self.cas_dir).to_path_buf();

        let mut prompts: Vec<Prompt> = Vec::with_capacity(delivery_events.len());
        for event in &delivery_events {
            // cas-d4ae: taskless idle and escalated stall events are factory
            // health incidents, not ordinary one-shot director chatter.  The
            // daemon persists and wakes the owning supervisor through its
            // durable attention relay below.  Do not also generate the old
            // direct prompt or one incident would reach the supervisor twice.
            if matches!(
                event,
                DirectorEvent::WorkerIdle {
                    active_task: None,
                    ..
                } | DirectorEvent::WorkerStalled { escalate: true, .. }
                    | DirectorEvent::SupervisorStalled { .. }
            ) {
                continue;
            }
            // cas-6883: re-validate a MERGE REQUIRED / AwaitingMerge idle
            // signal against live git state, and against what was last
            // actually sent, immediately before generating its prompt —
            // as close to true send time as this tick-based pipeline gets.
            // `NotApplicable` (wrong event shape, task no longer
            // AwaitingMerge, or no resolvable epic branch) falls through to
            // `generate_prompt` unaffected, exactly as before this fix.
            let merge_alert_evidence =
                match check_merge_alert_freshness(event, &unfiltered_data, &repo_root) {
                    MergeAlertFreshness::NotApplicable => None,
                    // AC1: branch already carries zero unmerged commits vs
                    // the epic — drop the alert entirely rather than
                    // instruct a merge that already happened.
                    MergeAlertFreshness::Stale => continue,
                    MergeAlertFreshness::Fresh(evidence) => {
                        // AC3: same (task, branch) pair, same evidence as
                        // last time — suppress the repeat.
                        if self.event_detector.merge_alert_should_emit(
                            &evidence.task_id,
                            &evidence.factory_branch,
                            evidence.unmerged_count,
                            &evidence.epic_sha,
                        ) {
                            Some(evidence)
                        } else {
                            continue;
                        }
                    }
                };

            if let Some(prompt) = generate_prompt_at(
                event,
                &self.director_data,
                &unfiltered_data,
                &self.supervisor_name,
                &self.auto_prompt,
                self.supervisor_cli,
                self.worker_cli,
                &gated_task_ids,
                merge_alert_evidence.as_ref(),
                snapshot_at,
            ) {
                prompts.push(prompt);
            }
        }

        let sweep_data = delivery_state_is_authoritative.then_some(unfiltered_data);
        (delivery_events, prompts, sweep_data)
    }

    fn try_load_unfiltered_director_data_for_delivery(&self) -> anyhow::Result<DirectorData> {
        let worktree_root = self.worktree_manager.as_ref().map(|m| m.worktree_root());
        DirectorData::load_with_stores(
            &self.cas_dir,
            worktree_root.as_deref(),
            false,
            self.director_stores.as_ref(),
        )
    }

    fn load_unfiltered_director_data_for_delivery(&self) -> DirectorData {
        self.try_load_unfiltered_director_data_for_delivery()
            .unwrap_or_else(|_| self.unfiltered_director_data.clone())
    }

    /// Re-check a state-bearing prompt at the narrowest point before transport
    /// injection. Batch revalidation happens earlier in the tick; a worker
    /// assignment, epic close, subtask reopen, or — cas-6eab (GH #74) — the
    /// merge this alert is asking for may all land after that snapshot and
    /// before this prompt's turn in the delivery loop. The merge case is not
    /// hypothetical: `handle_epic_change` runs merges inside the very same
    /// tick, between prompt generation and this loop.
    pub(crate) fn prompt_is_still_deliverable(&self, prompt: &Prompt) -> bool {
        if prompt.retract_epic.is_some() || prompt.retract_task.is_some() {
            // Failure to load authoritative state is uncertainty: deliver.
            // Never remove a legitimate notification based on cached data.
            return self
                .try_load_unfiltered_director_data_for_delivery()
                .map(|data| prompt_is_still_deliverable(prompt, &data, &self.delivery_repo_root()))
                .unwrap_or(true);
        }
        if prompt.drop_if_worker_assigned.is_none() {
            return true;
        }
        let data = self.load_unfiltered_director_data_for_delivery();
        prompt_is_still_deliverable(prompt, &data, &self.delivery_repo_root())
    }

    /// The main checkout every `factory/*` and `epic/*` branch lives in — the
    /// same derivation used by the send-time merge freshness check above and
    /// by `close_ops`/`director.rs` (`.cas`'s parent).
    fn delivery_repo_root(&self) -> std::path::PathBuf {
        self.cas_dir.parent().unwrap_or(&self.cas_dir).to_path_buf()
    }

    /// Refresh Cassy data from stores and detect state changes
    ///
    /// Returns the detected events. Prompt generation happens later, at
    /// delivery time, against a fresh snapshot (see
    /// `revalidate_and_prompt_for_delivery`) — this method used to also
    /// build a `Vec<Prompt>` here, but every caller discarded it (the daemon
    /// tick regenerates prompts from the delivery-revalidated events; the
    /// other two callers ignore the whole `Result`), so it was pure wasted
    /// work on every refresh. Removed rather than kept as a second,
    /// drift-prone prompt-generation path (cas-627f).
    pub fn refresh_data(&mut self) -> anyhow::Result<Vec<DirectorEvent>> {
        let next_fingerprint = CasDbFingerprint::from_cas_dir(&self.cas_dir);
        let db_changed = match self.last_db_fingerprint {
            Some(prev) => prev != next_fingerprint,
            None => true,
        };
        let git_due = !self.director_data.git_loaded
            || self.last_git_refresh.elapsed() >= self.git_refresh_interval;

        let worktree_root = self.worktree_manager.as_ref().map(|m| m.worktree_root());
        if db_changed {
            let loaded = DirectorData::load_with_stores(
                &self.cas_dir,
                worktree_root.as_deref(),
                git_due,
                self.director_stores.as_ref(),
            )?;
            self.director_data =
                merge_director_data_preserving_git(&self.director_data, loaded, git_due);
            // cas-dbbe: snapshot the fresh, still-unfiltered load BEFORE
            // `filter_director_agents_to_current_session()` (below) mutates
            // `self.director_data` in place. This canonical copy is what
            // change detection and the TaskCompleted safety net read, so a
            // second epic worked concurrently in this session never looks
            // like it "disappeared" just because it's outside the
            // currently-tracked epic's display scope. Only the fields those
            // two consumers actually read are cloned (see
            // `unfiltered_snapshot_from`) — cloning e.g. `changes` (git diff
            // info per worktree, potentially the largest field) here would
            // be pure waste.
            self.unfiltered_director_data = unfiltered_snapshot_from(&self.director_data);
            // cas-ae6d: the only place `director_data` is genuinely re-read.
            self.director_data_loaded_at = chrono::Utc::now();
            if git_due {
                self.last_git_refresh = Instant::now();
            }
        } else if git_due {
            self.director_data.refresh_git_changes_with_stores(
                &self.cas_dir,
                worktree_root.as_deref(),
                self.director_stores.as_ref(),
            )?;
            self.last_git_refresh = Instant::now();
        } else {
            self.refresh_branch_visibility_cache();
            self.last_refresh = Instant::now();
            // Worker holds live in session metadata rather than cas.db. They
            // must therefore be reconciled even on the common unchanged-DB
            // refresh path; otherwise a just-written hold leaks one more
            // WorkerIdle event before unrelated database activity occurs.
            self.apply_session_metadata_worker_holds();
            return Ok(self.detect_supervisor_stall().into_iter().collect());
        }

        self.refresh_branch_visibility_cache();
        self.last_db_fingerprint = Some(next_fingerprint);
        self.last_refresh = Instant::now();

        // Sync session_id → pane_name mappings from agent store
        self.sync_session_mappings();
        self.apply_session_metadata_focus();
        self.apply_session_metadata_worker_holds();

        // cas-e98e AC3: drop phantom worker panes when registry says the
        // worker is no longer supervision-live (Shutdown, or Stale/dead with
        // no live process). Keeps panes for still-registering names and for
        // process-alive dual-signal workers.
        if db_changed {
            self.reconcile_phantom_worker_panes();
        }

        // Detect state changes against the UNFILTERED snapshot (cas-dbbe) so new
        // epics are visible to the event detector, and so a second epic worked
        // concurrently in this session is never epic-scoped out of the
        // comparison and mistaken for a completed task. This allows EpicStarted
        // to fire and update epic_state, which the filter depends on for
        // subsequent refresh cycles.
        // Pass the currently-tracked epic id so `EpicStarted` is gated on
        // strict improvement: a stray zero-subtask Open-with-branch epic
        // cannot hijack `epic_state` mid-session (see task cas-4181).
        let mut events = self
            .event_detector
            .detect_changes(&self.unfiltered_director_data, self.epic_state.epic_id());
        events.extend(self.detect_supervisor_stall());

        // Now filter to current session (agents + tasks scoped to active epic)
        if db_changed {
            self.filter_director_agents_to_current_session();
        }

        Ok(events)
    }

    fn detect_supervisor_stall(&mut self) -> Option<DirectorEvent> {
        use cas_store::ReminderStatus;

        let session = self.factory_session.as_deref()?;
        let (last_call, mut tracker) = supervisor_progress_from_session_metadata_named(session)?;
        let before = tracker.clone();
        let now = Utc::now();
        let held_workers = worker_holds_from_session_metadata_named(session).unwrap_or_default();
        let repo_root = self.delivery_repo_root();
        let actionable = supervisor_actionable_state(
            &self.unfiltered_director_data,
            self.epic_state.epic_id().or(self.current_epic_id.as_deref()),
            &self.supervisor_name,
            &held_workers,
            now,
            self.supervisor_stall_after_secs,
            |branch| {
                crate::mcp::tools::core::task::lifecycle::close_ops::resolve_branch_sha(
                    &repo_root,
                    branch,
                )
            },
        );
        let supervisor_ids = self
            .unfiltered_director_data
            .agents
            .iter()
            .filter(|agent| agent.name == self.supervisor_name)
            .map(|agent| agent.id.as_str())
            .collect::<HashSet<_>>();
        let window_end = now + chrono::Duration::seconds(self.supervisor_stall_after_secs as i64);
        let covering_reminder = self.unfiltered_director_data.reminders.iter().any(|reminder| {
            reminder.status == ReminderStatus::Pending
                && reminder
                    .session_id
                    .as_deref()
                    .map(|id| id == session)
                    .unwrap_or(true)
                && (supervisor_ids.contains(reminder.target_id.as_str())
                    || reminder.target_id == self.supervisor_name)
                && reminder
                    .trigger_at
                    .is_some_and(|at| at >= now && at <= window_end)
        });
        let observation = tracker.observe(
            actionable,
            last_call.or(self.session_created_at),
            covering_reminder,
            now,
            self.supervisor_stall_after_secs,
        );
        if tracker != before {
            let path = crate::ui::factory::session::metadata_path(session);
            if let Err(error) = persist_session_metadata_supervisor_stall_at(&path, &tracker) {
                tracing::warn!(%error, "failed to persist supervisor stall metric");
            }
        }
        observation.wake.map(|next_step| DirectorEvent::SupervisorStalled {
            next_step,
            occurrence: now.to_rfc3339(),
            actionable_idle_secs: observation.actionable_idle_secs,
        })
    }

    fn supervisor_stall_event_is_current(
        &self,
        event: &DirectorEvent,
        data: &DirectorData,
    ) -> bool {
        let DirectorEvent::SupervisorStalled {
            next_step,
            occurrence,
            ..
        } = event
        else {
            return false;
        };
        let Some(session) = self.factory_session.as_deref() else {
            return false;
        };
        let Some((last_call, _)) = supervisor_progress_from_session_metadata_named(session) else {
            return false;
        };
        let Ok(detected_at) = occurrence.parse::<DateTime<Utc>>() else {
            return false;
        };
        if last_call.is_some_and(|last| last > detected_at) {
            return false;
        }
        let held_workers = worker_holds_from_session_metadata_named(session).unwrap_or_default();
        let repo_root = self.delivery_repo_root();
        supervisor_actionable_state(
            data,
            self.epic_state.epic_id().or(self.current_epic_id.as_deref()),
            &self.supervisor_name,
            &held_workers,
            Utc::now(),
            self.supervisor_stall_after_secs,
            |branch| {
                crate::mcp::tools::core::task::lifecycle::close_ops::resolve_branch_sha(
                    &repo_root,
                    branch,
                )
            },
        )
        .as_ref()
            == Some(next_step)
    }

    /// Route worker panes whose registry rows are all non-live through the normal shutdown path.
    ///
    /// Uses the shared [`crate::mcp::tools::service::agent_liveness`] classifier
    /// so pane grid, worker_status, and agent_list cannot disagree about dead
    /// workers. Names with **no** registry row yet are kept (spawn race).
    fn reconcile_phantom_worker_panes(&mut self) {
        use crate::mcp::tools::service::agent_liveness::should_keep_worker_pane;
        use crate::store::{open_agent_store, open_spawn_queue_store};

        let Ok(agent_store) = open_agent_store(&self.cas_dir) else {
            return;
        };
        let Ok(all_agents) = agent_store.list(None) else {
            return;
        };

        let to_drop: Vec<String> = self
            .worker_names
            .iter()
            .filter(|name| !should_keep_worker_pane(name, all_agents.iter()))
            .cloned()
            .collect();

        if to_drop.is_empty() {
            return;
        }

        let Some(factory_session) = self.factory_session.as_deref() else {
            tracing::warn!(workers = ?to_drop, "stale worker panes retained: no factory session to queue teardown");
            return;
        };
        let Ok(queue) = open_spawn_queue_store(&self.cas_dir) else {
            tracing::warn!(workers = ?to_drop, "stale worker panes retained: shutdown queue unavailable");
            return;
        };
        let queued = queue.peek(512).unwrap_or_default();
        for name in &to_drop {
            if queued.iter().any(|request| {
                request.action == cas_store::SpawnAction::Shutdown
                    && request.factory_session.as_deref() == Some(factory_session)
                    && request.worker_names.iter().any(|target| target == name)
            }) {
                continue;
            }
            match queue.enqueue_shutdown(
                None,
                std::slice::from_ref(name),
                true,
                Some(factory_session),
            ) {
                Ok(request_id) => {
                    tracing::warn!(worker = %name, request_id, "cas-ca69: non-live worker pane retained until forced shutdown reaps its process tree")
                }
                Err(error) => {
                    tracing::error!(worker = %name, %error, "stale worker pane retained because teardown request could not be queued")
                }
            }
        }
    }

    fn set_active_epic(
        &mut self,
        epic_id: &str,
        epic_title: &str,
        source: EpicFocusSource,
    ) -> EpicState {
        let previous = std::mem::replace(
            &mut self.epic_state,
            EpicState::Active {
                epic_id: epic_id.to_string(),
                epic_title: epic_title.to_string(),
            },
        );
        self.current_epic_id = Some(epic_id.to_string());
        self.current_epic_source = Some(source);
        if source == EpicFocusSource::SessionDefault {
            self.persist_current_epic_id();
        }
        previous
    }

    fn source_for_detected_epic_started(&self, epic_id: &str) -> EpicFocusSource {
        let focus = preferred_epic_focus_from_session_metadata();
        if focus.epic_id.as_deref() == Some(epic_id) {
            focus.source.unwrap_or(EpicFocusSource::Inference)
        } else {
            EpicFocusSource::Inference
        }
    }

    fn can_adopt_detected_epic_started(&self, epic_id: &str, source: EpicFocusSource) -> bool {
        match source {
            EpicFocusSource::Pinned | EpicFocusSource::SessionDefault => true,
            EpicFocusSource::Inference => {
                inferred_epic_is_displayable(&self.director_data, epic_id)
            }
        }
    }

    fn persist_current_epic_id(&self) {
        let Some(session_name) = self.factory_session.as_deref() else {
            return;
        };
        let Some(epic_id) = self.current_epic_id.as_deref() else {
            return;
        };
        let path = metadata_path(session_name);
        if let Err(err) = persist_session_metadata_epic_id_at(&path, epic_id) {
            tracing::warn!(
                session = %session_name,
                epic_id,
                error = %err,
                "failed to persist factory session epic focus"
            );
        }
    }

    fn clear_persisted_current_epic_id(&self) {
        let Some(session_name) = self.factory_session.as_deref() else {
            return;
        };
        let path = metadata_path(session_name);
        if let Err(err) = clear_session_metadata_epic_id_at(&path) {
            tracing::warn!(
                session = %session_name,
                error = %err,
                "failed to clear factory session epic focus"
            );
        }
    }

    fn apply_session_metadata_focus(&mut self) {
        let focus = preferred_epic_focus_from_session_metadata();

        // cas-6945: only short-circuit when something is already tracked.
        // The event detector's `EpicStarted` is edge-triggered on the
        // epic's Open+branch/InProgress transition (see
        // events::test_no_duplicate_epic_started_for_existing_open_with_branch).
        // In the normal supervisor sequence the epic branch is auto-created
        // at `task create` time, before any worker exists — so that
        // transition happens, `EpicStarted` fires once, gets rejected by
        // `inferred_epic_is_displayable` (no subtask has an assignee yet),
        // and never refires even after a worker later starts a subtask and
        // picks up an assignee (the `task action=start` fix, also
        // cas-6945). Without retrying resolution on every tick while
        // nothing is tracked, the epic silently never adopts. Once
        // something IS tracked, behavior is unchanged from before.
        let already_synced = self.current_epic_id.is_some()
            && self.current_epic_id.as_deref() == focus.epic_id.as_deref()
            && (focus.epic_id.is_none() || self.current_epic_source == focus.source);
        if already_synced {
            // Branch metadata can change while the focused epic ID stays the
            // same. Keep spawn state live even on this fast path.
            self.epic_branch = epic_branch_for_state(&self.director_data, &self.epic_state);
            return;
        }

        let epic_state = resolve_epic_state_for_focus(&self.director_data, &focus);
        self.current_epic_id = epic_state.epic_id().map(str::to_string);
        self.current_epic_source = epic_state
            .epic_id()
            .map(|_| focus.source.unwrap_or(EpicFocusSource::Inference));
        self.epic_branch = epic_branch_for_state(&self.director_data, &epic_state);
        self.epic_state = epic_state;
    }

    /// Reconcile the durable, session-scoped supervisor hold set into the
    /// director's live idle/stall gate. A transient metadata read failure is
    /// deliberately a no-op: it must not silently release an existing hold.
    fn apply_session_metadata_worker_holds(&mut self) {
        let Some(session_name) = self.factory_session.as_deref() else {
            return;
        };
        let Some(held_workers) = worker_holds_from_session_metadata_named(session_name) else {
            return;
        };

        for worker in self.worker_names.clone() {
            if held_workers.contains(&worker) && !self.event_detector.is_worker_held(&worker) {
                self.event_detector.mark_worker_hold(&worker);
            } else if !held_workers.contains(&worker) && self.event_detector.is_worker_held(&worker)
            {
                self.event_detector.clear_worker_hold(&worker);
            }
        }
    }

    /// Remove a worker from the detector and its durable session hold set.
    /// This prevents a later worker that reuses the same friendly name from
    /// inheriting a hold it never received.
    pub(crate) fn remove_worker_from_event_detector(&mut self, worker_name: &str) {
        self.event_detector.remove_worker(worker_name);
        let Some(session_name) = self.factory_session.as_deref() else {
            return;
        };
        let path = metadata_path(session_name);
        if let Err(error) = persist_session_metadata_worker_hold_at(&path, worker_name, false) {
            tracing::warn!(
                worker = %worker_name,
                factory_session = %session_name,
                error = %error,
                "failed to clear removed worker from durable hold set"
            );
        }
    }

    /// Get the focused pane kind
    pub fn focused_kind(&self) -> Option<&PaneKind> {
        self.mux.focused().map(|p| p.kind())
    }

    /// Check if the supervisor pane is focused
    pub fn focused_is_supervisor(&self) -> bool {
        matches!(self.focused_kind(), Some(PaneKind::Supervisor))
    }

    /// Check if a worker pane is focused
    pub fn focused_is_worker(&self) -> bool {
        matches!(self.focused_kind(), Some(PaneKind::Worker))
    }

    /// Check if the focused pane accepts keyboard input
    ///
    /// Supervisor and worker panes accept keyboard input.
    pub fn focused_accepts_input(&self) -> bool {
        matches!(
            self.focused_kind(),
            Some(PaneKind::Supervisor | PaneKind::Worker | PaneKind::Shell)
        )
    }

    pub(crate) fn focused_pane_branch(&self) -> Option<String> {
        let pane = self.mux.focused()?;
        match pane.kind() {
            PaneKind::Supervisor => self.branch_visibility.branch_for_path(&self.project_dir),
            PaneKind::Worker => {
                let path = self
                    .worktree_manager
                    .as_ref()
                    .map(|manager| manager.worktree_path_for_worker(pane.id()));
                let path = path.as_deref().unwrap_or(&self.project_dir);
                self.branch_visibility.branch_for_path(path)
            }
            PaneKind::Director | PaneKind::Shell => None,
        }
    }

    pub(crate) fn focused_epic_branch_status(&self) -> Option<BranchAheadBehind> {
        let epic_id = self.current_epic_id.as_deref()?;
        self.branch_visibility.epic_ahead_behind(epic_id)
    }

    fn refresh_branch_visibility_cache(&mut self) {
        let worker_paths: Vec<(String, PathBuf)> = self
            .worktree_manager
            .as_ref()
            .map(|manager| manager.worker_cwds().into_iter().collect())
            .unwrap_or_default();
        let epic_branches = self.branch_visible_epics_for_ahead_behind();

        self.branch_visibility.refresh(
            &self.project_dir,
            &worker_paths,
            &epic_branches,
            Instant::now(),
        );
        self.sync_worker_pane_branch_titles();
    }

    fn branch_visible_epics_for_ahead_behind(&self) -> Vec<(String, String)> {
        let mut visible_epic_ids: HashSet<String> = self
            .current_epic_id
            .iter()
            .cloned()
            .chain(
                self.director_data
                    .ready_tasks
                    .iter()
                    .chain(self.director_data.in_progress_tasks.iter())
                    .filter_map(|task| task.epic.clone()),
            )
            .collect();

        self.director_data
            .epic_tasks
            .iter()
            .filter(|epic| visible_epic_ids.remove(&epic.id))
            .filter_map(|epic| {
                epic.branch
                    .as_ref()
                    .map(|branch| (epic.id.clone(), branch.clone()))
            })
            .collect()
    }

    pub(crate) fn sync_worker_pane_branch_titles(&mut self) {
        let worker_branches: HashMap<String, Option<String>> = self
            .worker_names
            .iter()
            .map(|worker| {
                let worktree_path = self
                    .worktree_manager
                    .as_ref()
                    .map(|manager| manager.worktree_path_for_worker(worker));
                let branch = branch_for_worker_title(
                    &self.branch_visibility,
                    worktree_path.as_deref(),
                    &self.project_dir,
                );
                (worker.clone(), branch)
            })
            .collect();

        for pane in self.mux.panes_mut() {
            if *pane.kind() == PaneKind::Worker {
                let branch = worker_branches
                    .get(pane.id())
                    .and_then(|branch| branch.as_deref());
                pane.set_title(format_pane_title_with_branch(pane.id(), branch));
            }
        }
    }

    /// Get all worker names for layout (real + pending booting workers)
    pub fn layout_worker_names(&self) -> Vec<String> {
        let mut names = self.worker_names.clone();
        for pw in &self.pending_workers {
            if !names.contains(&pw.name) {
                names.push(pw.name.clone());
            }
        }
        names
    }

    /// Check if a worker name is pending (still booting)
    pub fn is_pending_worker(&self, name: &str) -> bool {
        self.pending_workers.iter().any(|pw| pw.name == name)
    }

    /// Add a worker to the pending set (called after prepare_worker_spawn succeeds)
    pub fn add_pending_worker(&mut self, name: String, isolate: bool) {
        self.pending_workers.push(PendingWorkerState {
            name,
            started_at: Instant::now(),
            isolate,
        });
        // Rebuild pane grid is NOT needed — pending workers are not navigable
        // But we do need to sync layout sizes so the boot pane gets space
        let _ = self.sync_pane_sizes();
    }

    /// Remove a worker from the pending set (called on spawn success or failure)
    pub fn remove_pending_worker(&mut self, name: &str) {
        self.pending_workers.retain(|pw| pw.name != name);
    }

    /// Get worker names
    pub fn worker_names(&self) -> &[String] {
        &self.worker_names
    }

    /// Get the number of active workers
    pub fn worker_count(&self) -> usize {
        self.worker_names.len()
    }

    /// Select a worker tab by index (0-based)
    ///
    /// Returns true if the selection changed.
    pub fn select_worker_tab(&mut self, index: usize) -> bool {
        let total = self.layout_worker_names().len();
        if index < total && index != self.selected_worker_tab {
            self.selected_worker_tab = index;
            true
        } else {
            false
        }
    }

    /// Select a worker tab by 1-based number (for keyboard shortcuts)
    ///
    /// Returns true if the selection changed.
    pub fn select_worker_by_number(&mut self, number: usize) -> bool {
        let total = self.layout_worker_names().len();
        if number > 0 && number <= total {
            self.select_worker_tab(number - 1)
        } else {
            false
        }
    }

    /// Get the currently selected worker name
    pub fn selected_worker(&self) -> Option<&str> {
        self.worker_names
            .get(self.selected_worker_tab)
            .map(|s| s.as_str())
    }

    /// Ensure selected_worker_tab is valid after workers change
    fn clamp_selected_worker_tab(&mut self) {
        let total = self.layout_worker_names().len();
        if total > 0 && self.selected_worker_tab >= total {
            self.selected_worker_tab = total - 1;
        }
    }

    /// Get supervisor name
    pub fn supervisor_name(&self) -> &str {
        &self.supervisor_name
    }

    /// Resolve a delivery target name to the harness (CLI) it is running.
    ///
    /// This is the source of truth for *recipient-aware* message routing
    /// (cas-b68a): the daemon must decide whether to deliver via the Claude
    /// agent-teams inbox or via a direct PTY write based on the **recipient's**
    /// harness, not the supervisor's mode.
    ///
    /// - The logical name `"supervisor"` and the supervisor's pane name both
    ///   resolve to the supervisor's CLI.
    /// - Any other name is treated as a worker and resolved through the mux's
    ///   per-worker spec registry (`effective_worker_spec`), which falls back to
    ///   the Mux-wide default for unknown names.
    pub fn harness_for(&self, target: &str) -> cas_mux::SupervisorCli {
        if target == "supervisor" || target == self.supervisor_name {
            self.supervisor_cli
        } else {
            self.mux.effective_worker_spec(target, None).cli
        }
    }

    /// Get factory session name (for prompt queue isolation)
    pub fn factory_session(&self) -> Option<&str> {
        self.factory_session.as_deref()
    }

    /// Set factory session name
    pub fn set_factory_session(&mut self, name: String) {
        self.session_created_at = std::fs::read_to_string(metadata_path(&name))
            .ok()
            .and_then(|json| serde_json::from_str::<SessionMetadata>(&json).ok())
            .and_then(|metadata| {
                DateTime::parse_from_rfc3339(&metadata.created_at)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
            });
        if let Some(supervisor_session_id) = self
            .mux
            .get(&self.supervisor_name)
            .and_then(|pane| pane.harness_session_id())
        {
            match crate::store::open_agent_store(&self.cas_dir) {
                Ok(agent_store) => {
                    if let Err(error) = stamp_trusted_factory_supervisor(
                        agent_store.as_ref(),
                        &supervisor_session_id,
                        &self.supervisor_name,
                        &name,
                    ) {
                        tracing::error!(
                            supervisor = %self.supervisor_name,
                            session_id = %supervisor_session_id,
                            factory_session = %name,
                            error = %error,
                            "failed to stamp trusted factory supervisor identity"
                        );
                    }
                }
                Err(error) => tracing::error!(
                    supervisor = %self.supervisor_name,
                    session_id = %supervisor_session_id,
                    factory_session = %name,
                    error = %error,
                    "failed to open agent store for trusted supervisor identity"
                ),
            }
        } else {
            tracing::error!(
                supervisor = %self.supervisor_name,
                factory_session = %name,
                "factory supervisor pane has no harness session id to stamp"
            );
        }

        self.factory_session = Some(name);
        self.apply_session_metadata_worker_holds();
        for worker in self.worker_names.clone() {
            self.track_worker_process_group(&worker);
        }
    }

    /// Persist the process-group boundary created by the worker pane spawn.
    pub(crate) fn track_worker_process_group(&self, worker_name: &str) {
        let Some(factory_session) = self.factory_session.as_deref() else {
            return;
        };
        let Some(pgid) = self.mux.pane_process_group_id(worker_name) else {
            tracing::warn!(
                worker = %worker_name,
                "worker pane has no process group id to track"
            );
            return;
        };
        // cas-99f5 (GH #86): contain first, record second — the durable record
        // must name the scope that already holds the worker, so a teardown by a
        // later process (or after a daemon crash) can find and kill it.
        let cgroup =
            crate::ui::factory::process_groups::contain_worker(worker_name, factory_session, pgid);
        if let Err(error) = crate::ui::factory::process_groups::track_contained(
            self.cas_dir(),
            worker_name,
            factory_session,
            pgid,
            cgroup,
        ) {
            tracing::warn!(
                worker = %worker_name,
                pgid,
                error = %error,
                "failed to persist worker process-group ownership"
            );
        }
    }

    /// Drop a process-group record only after the group is confirmed gone.
    ///
    /// A survivor intentionally remains registered for `gc_report` rather
    /// than being silently forgotten.
    pub(crate) async fn untrack_worker_process_group_if_gone(&self, pgid: u32) {
        let record = crate::ui::factory::process_groups::list(self.cas_dir())
            .unwrap_or_default()
            .into_iter()
            .find(|record| record.pgid == pgid);
        let Some(record) = record else {
            return;
        };
        // cas-99f5 (GH #86): `shutdown_workers` and session end reach this
        // after killpg has already run. killpg cannot touch descendants that
        // left the process group (Node's `detached: true` dev servers), so the
        // cgroup scope is torn down here too — and it reports what it reaped.
        crate::ui::factory::process_groups::reap_cgroup_scope(&record);
        for _ in 0..20 {
            if !crate::ui::factory::process_groups::is_live(&record) {
                if let Err(error) =
                    crate::ui::factory::process_groups::untrack(self.cas_dir(), pgid)
                {
                    tracing::warn!(
                        pgid,
                        error = %error,
                        "failed to remove terminated worker process-group record"
                    );
                }
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        tracing::warn!(
            pgid,
            worker = %record.worker_name,
            "worker process group survived teardown; retaining record for gc_report"
        );
    }

    /// Get the worktree manager (if worktrees are enabled)
    pub fn worktree_manager(&self) -> Option<&WorktreeManager> {
        self.worktree_manager.as_ref()
    }

    /// Get the worktree manager mutably (if worktrees are enabled)
    pub fn worktree_manager_mut(&mut self) -> Option<&mut WorktreeManager> {
        self.worktree_manager.as_mut()
    }

    /// Check if worktree-based isolation is enabled
    pub fn worktrees_enabled(&self) -> bool {
        self.worktree_manager.is_some()
    }

    /// Get the lead session ID (UUID for Teams config.json)
    pub fn lead_session_id(&self) -> Option<&str> {
        self.lead_session_id.as_deref()
    }

    /// Get the director data
    pub fn director_data(&self) -> &DirectorData {
        &self.director_data
    }

    /// Get the Cassy directory path
    pub fn cas_dir(&self) -> &std::path::Path {
        &self.cas_dir
    }

    /// Get the theme
    pub fn theme(&self) -> &ActiveTheme {
        &self.theme
    }

    /// Get the notifier
    pub fn notifier(&self) -> &Notifier {
        &self.notifier
    }

    /// Send notifications for detected events
    pub fn notify_events(&self, events: &[DirectorEvent]) {
        for event in events {
            self.notifier.notify_event(event);
        }
    }

    /// Set an error message (auto-dismisses after 5 seconds)
    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.error_message = Some(msg.into());
        self.error_set_at = Some(Instant::now());
    }

    /// Clear the error message
    pub fn clear_error(&mut self) {
        self.error_message = None;
        self.error_set_at = None;
    }

    /// Check if error should be auto-dismissed (after 5 seconds)
    pub fn check_error_timeout(&mut self) {
        if let Some(set_at) = self.error_set_at {
            if set_at.elapsed() >= Duration::from_secs(5) {
                self.clear_error();
            }
        }
    }

    /// Toggle between Panes and MissionControl factory view modes.
    pub fn toggle_factory_view_mode(&mut self) {
        use crate::ui::factory::renderer::FactoryViewMode;
        self.factory_view_mode = match self.factory_view_mode {
            FactoryViewMode::Panes => FactoryViewMode::MissionControl,
            FactoryViewMode::MissionControl => FactoryViewMode::Panes,
        };
    }

    /// Toggle sidebar collapsed state
    pub fn toggle_sidecar_collapsed(&mut self) {
        self.sidecar_collapsed = !self.sidecar_collapsed;
        // Recalculate PTY dimensions to match new layout
        let _ = self.handle_resize(self.terminal_cols, self.terminal_rows);
    }

    /// Handle resize event
    pub fn handle_resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        // Store terminal dimensions
        self.terminal_cols = cols;
        self.terminal_rows = rows;

        // Include pending workers in layout so boot panes get space
        let all_names = self.layout_worker_names();

        // Calculate actual layout areas and resize panes to match.
        // Must reserve the same identity-header rows as the render path,
        // otherwise PTYs are sized taller than the visible pane area.
        let area = Rect::new(0, 0, cols, rows);
        let layout = FactoryLayout::calculate_from_names_with_header_rows(
            area,
            &all_names,
            self.tabbed_workers,
            self.sidecar_collapsed,
            self.layout_sizes,
            Self::identity_header_rows(area),
        );

        // Resize only REAL worker panes (pending workers have no PTY)
        if layout.is_tabbed {
            // Tabbed mode: all workers share the same viewport size
            if let Some(content_area) = layout.worker_content {
                let inner_height = content_area.height.saturating_sub(2);
                let inner_width = content_area.width.saturating_sub(2);

                for name in &self.worker_names {
                    if let Some(pane) = self.mux.get_mut(name) {
                        let _ = pane.resize(inner_height, inner_width);
                    }
                }
            }
        } else {
            // Side-by-side mode: find each real worker's index in the combined list
            for name in &self.worker_names {
                if let Some(idx) = all_names.iter().position(|n| n == name) {
                    if let Some(worker_area) = layout.worker_areas.get(idx) {
                        let inner_height = worker_area.height.saturating_sub(2);
                        let inner_width = worker_area.width.saturating_sub(2);
                        if let Some(pane) = self.mux.get_mut(name) {
                            let _ = pane.resize(inner_height, inner_width);
                        }
                    }
                }
            }
        }

        // Resize supervisor pane
        if let Some(pane) = self.mux.get_mut(&self.supervisor_name) {
            let inner_height = layout.supervisor_area.height.saturating_sub(2);
            let inner_width = layout.supervisor_area.width.saturating_sub(2);
            let _ = pane.resize(inner_height, inner_width);
        }

        Ok(())
    }

    /// Sync pane sizes with current terminal dimensions
    ///
    /// In daemon mode, crossterm::terminal::size() returns a default (80x24) instead
    /// of failing, so we prefer stored dimensions if they're set to something reasonable.
    pub fn sync_pane_sizes(&mut self) -> anyhow::Result<()> {
        // Use stored dimensions if they're set (indicates daemon mode with client-provided size)
        // Only fall back to crossterm if stored dimensions are at default (120x40)
        let (cols, rows) = if self.terminal_cols > 120 || self.terminal_rows > 40 {
            // We have real dimensions from a client resize event
            (self.terminal_cols, self.terminal_rows)
        } else {
            // Try crossterm, but validate the result
            match crossterm::terminal::size() {
                Ok((c, r)) if c > 80 || r > 24 => (c, r),
                _ => (self.terminal_cols, self.terminal_rows),
            }
        };
        tracing::info!(
            "sync_pane_sizes: using {}x{} (stored: {}x{})",
            cols,
            rows,
            self.terminal_cols,
            self.terminal_rows
        );
        self.handle_resize(cols, rows)
    }
}

/// Task ids from `data` that are NOT in a Closed state — the "still open"
/// universe `compute_gated_task_ids` checks a Blocks-dependency's blocker
/// against (cas-09d0). `ready_tasks`/`in_progress_tasks` only ever contain
/// non-closed statuses already (see `director.rs::load_with_stores`'s
/// bucketing switch), but `epic_tasks` is populated unconditionally — EVERY
/// epic lands there regardless of status, including `Closed` (cas-a91b
/// review finding). Without filtering epic_tasks here, a task blocked by an
/// already-CLOSED epic was incorrectly still counted as "non-closed" and
/// stayed gated forever — over-excluding a task that `TaskStore::list_ready()`
/// (`blocker.status != 'closed'`) would correctly treat as ready. Split out
/// as a standalone function so it's testable against a plain `DirectorData`
/// literal, without needing real stores.
fn non_closed_task_ids(data: &DirectorData) -> HashSet<&str> {
    data.ready_tasks
        .iter()
        .chain(data.in_progress_tasks.iter())
        .chain(data.epic_tasks.iter().filter(|e| {
            !matches!(
                e.status,
                cas_types::TaskStatus::Closed | cas_types::TaskStatus::Cancelled
            )
        }))
        .map(|t| t.id.as_str())
        .collect()
}

fn merge_director_data_preserving_git(
    previous: &DirectorData,
    mut loaded: DirectorData,
    git_due: bool,
) -> DirectorData {
    if !git_due && previous.git_loaded {
        loaded.changes = previous.changes.clone();
        loaded.git_loaded = true;
    }
    loaded
}

/// Build the `unfiltered_director_data` snapshot (cas-dbbe) from a cloned
/// `director_data`, cloning only the fields its two consumers — change
/// detection (`DirectorEventDetector::detect_changes`) and the
/// `TaskCompleted` render-time safety net (`generate_prompt`) — actually
/// read: `ready_tasks`, `in_progress_tasks`, `epic_tasks`, `agents`,
/// `agent_id_to_name`. Neither consumer touches `changes`, `activity`,
/// `reminders`, or `epic_closed_counts` (verified: `rg 'data\.(changes|
/// activity|reminders|epic_closed_counts)' director/events.rs director/
/// prompts.rs` — zero hits), and the epic-scoping filter never mutates
/// those fields either, so they'd always be byte-identical to
/// `director_data`'s copy anyway. `changes` in particular can be the
/// largest field in `DirectorData` (one `FileChangeInfo` per changed file
/// per source, across every active worktree) — deep-cloning it every
/// `db_changed` tick for data nothing ever reads was pure waste.
fn unfiltered_snapshot_from(data: &DirectorData) -> DirectorData {
    DirectorData {
        ready_tasks: data.ready_tasks.clone(),
        in_progress_tasks: data.in_progress_tasks.clone(),
        epic_tasks: data.epic_tasks.clone(),
        agents: data.agents.clone(),
        agent_id_to_name: data.agent_id_to_name.clone(),
        activity: Vec::new(),
        changes: Vec::new(),
        git_loaded: false,
        reminders: Vec::new(),
        epic_closed_counts: HashMap::new(),
    }
}

pub(crate) fn queue_supervisor_intro_prompt(
    cas_dir: &std::path::Path,
    supervisor_name: &str,
    supervisor_cli: cas_mux::SupervisorCli,
    worker_names: &[String],
    session_id: Option<&str>,
    factory_session: Option<&str>,
) {
    let worker_list = if worker_names.is_empty() {
        "(none)".to_string()
    } else {
        worker_names.join(", ")
    };
    let mut prompt = match supervisor_cli {
        cas_mux::SupervisorCli::Codex => format!(
            "Codex supervisor startup:\n\
- Use skills: cas-supervisor, cas-codex-supervisor-checklist, cas-codebase-design\n\
- No hooks: call MCP tools explicitly (tasks/memory/rules/search)\n\
- Do NOT use /cas-start, /cas-context, or /cas-end\n\
- Canonical current workers for this session: {worker_list}\n\
- First steps: mcp__cs__coordination action=whoami; mcp__cs__task action=list task_type=epic; mcp__cs__task action=ready"
        ),
        // cas-0263: Claude has no --rules/developer_instructions launch flag, so
        // the canonical supervisor role contract is delivered as this launch-time
        // intro prompt (the surface a Claude supervisor consumes). Previously
        // Claude got nothing here, leaving it without any launch role contract.
        cas_mux::SupervisorCli::Claude => cas_mux::claude_supervisor_contract(&worker_list),
        // EPIC cas-8888 (cas-9a31, Phase 1): Grok's SessionStart hook fires
        // but its stdout is ignored, so the SessionStart-additionalContext
        // bundle injection Claude relies on does NOT reach a Grok
        // supervisor (see EPIC cas-8888 delta #2) — the real fix (injecting
        // the Cassy context bundle at launch via --agents/--rules/
        // --system-prompt-override) is Phase 2's job (PtyConfig::grok).
        // Until then, queue an explicit startup prompt the same shape as
        // Codex's (no-hooks-context-injection posture) but naming Grok's
        // OWN cas__ tool prefix rather than Codex's mcp__cs__.
        cas_mux::SupervisorCli::Grok => format!(
            "Grok supervisor startup:\n\
- Use skills: cas-supervisor, cas-supervisor-checklist, cas-codebase-design (Grok ignores \
  SessionStart stdout, so the launch intro carries the startup bundle)\n\
- Tools are namespaced cas__<tool> (e.g. cas__task, cas__coordination), \
  not mcp__cas__ or mcp__cs__\n\
- Canonical current workers for this session: {worker_list}\n\
- First steps: cas__coordination action=whoami; cas__task action=list task_type=epic; cas__task action=ready"
        ),
        // OpenCode's generated primary agent owns the durable role contract;
        // this queued intro carries the launch-specific skill names and
        // ambient-recall delivery that arrive before the agent config is
        // inspected.
        cas_mux::SupervisorCli::OpenCode => format!(
            "OpenCode supervisor startup:\n\
- Use skills: cas-supervisor, cas-supervisor-checklist, cas-codebase-design\n\
- Canonical current workers for this session: {worker_list}\n\
- First steps: cas_coordination action=whoami; cas_task action=list task_type=epic; cas_task action=ready"
        ),
    };

    // cas-2085 / GH #290: Claude 2.1.231 can skip SessionStart entirely for
    // a native-team supervisor launched under a non-default config dir, even
    // when both the project settings and the per-team `--settings` file carry
    // `cas hook SessionStart`. The launch intro is a guaranteed delivery seam,
    // so attach the same core context bundle here for that launch shape. Keep
    // direct/default-profile sessions on the ordinary hook path.
    if supervisor_cli == cas_mux::SupervisorCli::Claude {
        if let Some(context) =
            claude_custom_config_context_fallback(cas_dir, session_id.unwrap_or(supervisor_name))
        {
            prompt.push_str("\n\n<cas-session-start-fallback>\n");
            prompt.push_str(&context);
            prompt.push_str("\n</cas-session-start-fallback>");
        }
    }

    // cas-3413 / cas-91df: Codex has no SessionStart hooks and Grok discards
    // SessionStart stdout. Their queued launch intro is therefore the real
    // delivery seam. Reuse one bundle so both receive the normal context and
    // independently bounded ambient-recall packet, with explicit child
    // identity because the parent env is not the supervisor's env yet.
    if matches!(
        supervisor_cli,
        cas_mux::SupervisorCli::Codex
            | cas_mux::SupervisorCli::Grok
            | cas_mux::SupervisorCli::OpenCode
    ) {
        if let Some(bundle) = queued_supervisor_session_start_bundle(
            cas_dir,
            supervisor_name,
            session_id.unwrap_or(supervisor_name),
            factory_session.unwrap_or(supervisor_name),
        ) {
            // Reuse the established supervisor-guidance ceiling for queued
            // delivery. Ambient recall stays protected because it is the
            // purpose of this fallback; ordinary SessionStart context can
            // compact to its retrieval command rather than being truncated.
            use crate::hooks::handlers::session_budget::SessionContextAssembler;

            let mut assembler =
                SessionContextAssembler::new(format!("{prompt}\n\n<cas-queued-session-start>"))
                    .with_budget(crate::builtins::SUPERVISOR_GUIDANCE_SOFT_CAP_BYTES);
            if let Some(context) = bundle.context {
                let tool_prefix = supervisor_cli.backend().capabilities().tool_prefix;
                assembler.append_degradable(
                    context,
                    format!(
                        "[session-start context omitted to fit the queued launch payload — run `{tool_prefix}search action=context`]"
                    ),
                );
            }
            if let Some(packet) = bundle.ambient_recall {
                assembler.append_protected(packet.full);
            }
            assembler.append_protected("</cas-queued-session-start>".to_string());
            prompt = assembler.render();
        }
    }

    if let Ok(queue) = open_prompt_queue_store(cas_dir) {
        if let Some(session) = factory_session {
            let _ = queue.enqueue_with_session("cas", supervisor_name, &prompt, session);
        } else {
            let _ = queue.enqueue("cas", supervisor_name, &prompt);
        }
    }
}

fn claude_custom_config_context_fallback(
    cas_dir: &std::path::Path,
    session_id: &str,
) -> Option<String> {
    let home = dirs::home_dir()?;
    let configured = std::env::var("CLAUDE_CONFIG_DIR").ok();
    let config_dir = crate::ui::factory::daemon::runtime::teams::claude_config_dir_from(
        &home,
        configured.as_deref(),
    );
    if config_dir == home.join(".claude") {
        return None;
    }

    let input = cas_core::hooks::HookInput {
        session_id: session_id.to_string(),
        cwd: cas_dir.parent()?.to_string_lossy().into_owned(),
        hook_event_name: "SessionStart".to_string(),
        source: Some("startup".to_string()),
        agent_role: Some("supervisor".to_string()),
        ..Default::default()
    };
    let limit = crate::config::Config::load(cas_dir)
        .unwrap_or_default()
        .context_limit();
    crate::hooks::build_context(&input, limit, cas_dir)
        .ok()
        .filter(|context| !context.trim().is_empty())
}

struct QueuedSupervisorSessionStartBundle {
    context: Option<String>,
    ambient_recall: Option<crate::ambient_recall::RecallPacket>,
}

fn queued_supervisor_session_start_bundle(
    cas_dir: &std::path::Path,
    supervisor_name: &str,
    session_id: &str,
    factory_session: &str,
) -> Option<QueuedSupervisorSessionStartBundle> {
    let input = cas_core::hooks::HookInput {
        session_id: session_id.to_string(),
        cwd: cas_dir.parent()?.to_string_lossy().into_owned(),
        hook_event_name: "SessionStart".to_string(),
        source: Some("startup".to_string()),
        agent_role: Some("supervisor".to_string()),
        ..Default::default()
    };
    let limit = crate::config::Config::load(cas_dir)
        .unwrap_or_default()
        .context_limit();
    let context = crate::hooks::build_context(&input, limit, cas_dir)
        .ok()
        .filter(|context| !context.trim().is_empty());
    let ambient_recall = crate::ambient_recall::build_ambient_recall_context_for_factory_launch(
        &input,
        cas_dir,
        None,
        supervisor_name,
        factory_session,
    );
    (context.is_some() || ambient_recall.is_some()).then_some(QueuedSupervisorSessionStartBundle {
        context,
        ambient_recall,
    })
}

pub(crate) fn queue_codex_worker_intro_prompt(
    cas_dir: &std::path::Path,
    worker_name: &str,
    worker_cli: cas_mux::SupervisorCli,
) {
    match worker_cli {
        cas_mux::SupervisorCli::Codex => {
            // Codex workers now receive startup workflow as the initial codex prompt arg at spawn time.
            // Avoid queue injection here to prevent duplicate or draft-only startup prompts.
        }
        cas_mux::SupervisorCli::Claude => {
            // cas-0263: deliver the full canonical worker role contract (not just
            // a skill pointer) via the launch-time intro prompt Claude consumes.
            let prompt = cas_mux::claude_worker_contract(worker_name);
            if let Ok(queue) = open_prompt_queue_store(cas_dir) {
                let _ = queue.enqueue("cas", worker_name, &prompt);
            }
        }
        // EPIC cas-8888 (cas-9a31, Phase 1): placeholder arm — the real
        // decision (initial-prompt-at-spawn like Codex, vs. queued like
        // Claude, vs. something new given the passive-SessionStart-hook
        // caveat) belongs with Phase 2's PtyConfig::grok, which controls
        // how Grok is actually launched. Queuing here (Claude-shaped, with
        // Grok's own tool prefix) is a safe default in the meantime: it
        // degrades to "one extra queued prompt" rather than "worker never
        // told its task", not a correctness bug — cli=grok isn't
        // spawnable yet.
        cas_mux::SupervisorCli::Grok => {
            let prompt = format!(
                "You are a Cassy factory worker ({worker_name}).\n\
                 \n\
                 Check your assigned tasks: `cas__task action=mine`\n\
                 \n\
                 See the cas-worker skill for detailed workflow guidance. \
                 Tools are namespaced cas__<tool>, not mcp__cas__/mcp__cs__."
            );
            if let Ok(queue) = open_prompt_queue_store(cas_dir) {
                let _ = queue.enqueue("cas", worker_name, &prompt);
            }
        }
        // cas-a5da owns OpenCode spawn policy; PtyConfig already supplies --prompt.
        cas_mux::SupervisorCli::OpenCode => {}
    }
}

/// A change in epic state
#[derive(Debug, Clone)]
pub enum EpicStateChange {
    /// An epic was started
    Started {
        epic_id: String,
        epic_title: String,
        previous_state: EpicState,
    },
    /// An epic was completed
    Completed { epic_id: String, epic_title: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EpicFocusSource {
    Pinned,
    SessionDefault,
    Inference,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SessionEpicFocus {
    pub(crate) epic_id: Option<String>,
    pub(crate) source: Option<EpicFocusSource>,
}

/// Detect the initial epic state from loaded data.
///
/// If `preferred_epic_id` is set (from session metadata or explicit tracking),
/// look it up directly instead of scanning all epics. Falls back to scanning
/// if the preferred epic is not found or is closed.
pub(crate) fn detect_epic_state(data: &DirectorData, preferred_epic_id: Option<&str>) -> EpicState {
    use cas_types::TaskStatus;

    // If we have an explicit epic ID, try to use it directly (skip scanning)
    if let Some(epic_id) = preferred_epic_id {
        if let Some(epic) = data.epic_tasks.iter().find(|e| e.id == epic_id) {
            if epic.status != TaskStatus::Closed {
                return EpicState::Active {
                    epic_id: epic.id.clone(),
                    epic_title: epic.title.clone(),
                };
            }
        }
    }

    // Find an in-progress epic first (highest priority)
    for epic in &data.epic_tasks {
        if epic.status == TaskStatus::InProgress {
            return EpicState::Active {
                epic_id: epic.id.clone(),
                epic_title: epic.title.clone(),
            };
        }
    }

    // Fall back to open epics that have a branch set (auto-created branch).
    // Prefer epics with active subtasks (in-progress > ready) over stale ones.
    // This prevents stale cross-project epics from shadowing the active epic.
    // The picker is shared with the runtime EpicStarted event detector so the
    // two paths cannot disagree on which Open-with-branch epic is "best" —
    // divergence there caused a mid-session hijack bug (see task cas-4181).
    if let Some(best) = crate::ui::factory::director::pick_best_open_branch_epic(
        &data.epic_tasks,
        &data.in_progress_tasks,
        &data.ready_tasks,
    ) {
        return EpicState::Active {
            epic_id: best.id.clone(),
            epic_title: best.title.clone(),
        };
    }

    // Completing state is transitioned to via handle_epic_events() when EpicCompleted fires
    // Initial state detection only identifies Active epics; Completing is a transient state
    EpicState::Idle
}

pub(crate) fn resolve_epic_state_for_focus(
    data: &DirectorData,
    focus: &SessionEpicFocus,
) -> EpicState {
    match focus.source {
        Some(EpicFocusSource::Pinned | EpicFocusSource::SessionDefault) => {
            detect_epic_state(data, focus.epic_id.as_deref())
        }
        Some(EpicFocusSource::Inference) | None => {
            let state = detect_epic_state(data, None);
            match state.epic_id() {
                Some(epic_id) if inferred_epic_is_displayable(data, epic_id) => state,
                Some(_) => EpicState::Idle,
                None => state,
            }
        }
    }
}

pub(crate) fn inferred_epic_is_displayable(data: &DirectorData, epic_id: &str) -> bool {
    data.in_progress_tasks
        .iter()
        .chain(data.ready_tasks.iter())
        .any(|task| {
            task.epic.as_deref() == Some(epic_id)
                && crate::ui::factory::director::tasks::task_assigned_to_session_agent(task, data)
        })
}

#[cfg(test)]
pub(crate) fn preferred_epic_id_from_session_metadata_named(session_name: &str) -> Option<String> {
    preferred_epic_focus_from_session_metadata_named(session_name).epic_id
}

pub(crate) fn preferred_epic_focus_from_session_metadata() -> SessionEpicFocus {
    let Some(session_name) = std::env::var("CAS_FACTORY_SESSION").ok() else {
        return SessionEpicFocus::default();
    };
    preferred_epic_focus_from_session_metadata_named(&session_name)
}

pub(crate) fn preferred_epic_focus_from_session_metadata_named(
    session_name: &str,
) -> SessionEpicFocus {
    let path = metadata_path(session_name);
    let Some(data) = fs::read_to_string(path).ok() else {
        return SessionEpicFocus::default();
    };
    let Some(metadata) = serde_json::from_str::<SessionMetadata>(&data).ok() else {
        return SessionEpicFocus::default();
    };
    preferred_epic_focus_from_metadata(&metadata)
}

#[cfg(test)]
pub(crate) fn preferred_epic_id_from_metadata(metadata: &SessionMetadata) -> Option<String> {
    preferred_epic_focus_from_metadata(metadata).epic_id
}

pub(crate) fn preferred_epic_focus_from_metadata(metadata: &SessionMetadata) -> SessionEpicFocus {
    if let Some(epic_id) = metadata
        .pinned_epic_id
        .as_ref()
        .filter(|id| !id.trim().is_empty())
    {
        return SessionEpicFocus {
            epic_id: Some(epic_id.clone()),
            source: Some(EpicFocusSource::Pinned),
        };
    }

    if let Some(epic_id) = metadata.epic_id.as_ref().filter(|id| !id.trim().is_empty()) {
        return SessionEpicFocus {
            epic_id: Some(epic_id.clone()),
            source: Some(EpicFocusSource::SessionDefault),
        };
    }

    SessionEpicFocus::default()
}

pub(crate) fn persist_session_metadata_epic_id_at(
    path: &std::path::Path,
    epic_id: &str,
) -> std::io::Result<()> {
    update_session_metadata_at(path, |metadata| {
        metadata.epic_id = Some(epic_id.to_string());
    })
}

pub(crate) fn clear_session_metadata_epic_id_at(path: &std::path::Path) -> std::io::Result<()> {
    update_session_metadata_at(path, |metadata| {
        metadata.epic_id = None;
    })
}

pub(crate) fn persist_session_metadata_pinned_epic_id_at(
    path: &std::path::Path,
    pinned_epic_id: Option<&str>,
) -> std::io::Result<()> {
    update_session_metadata_at(path, |metadata| {
        metadata.pinned_epic_id = pinned_epic_id.map(str::to_string);
    })
}

pub(crate) fn persist_session_metadata_delivery_mode_at(
    path: &std::path::Path,
    delivery_mode: cas_types::DeliveryMode,
) -> std::io::Result<()> {
    update_session_metadata_at(path, |metadata| {
        metadata.delivery_mode = delivery_mode;
    })
}

/// Read the worker hold set for one factory session.
///
/// `None` distinguishes unavailable/malformed metadata from an intentionally
/// empty set, allowing the live detector to preserve holds across a transient
/// read failure rather than releasing them accidentally.
pub(crate) fn worker_holds_from_session_metadata_named(
    session_name: &str,
) -> Option<HashSet<String>> {
    let data = fs::read_to_string(metadata_path(session_name)).ok()?;
    let metadata = serde_json::from_str::<SessionMetadata>(&data).ok()?;
    Some(metadata.held_workers.into_iter().collect())
}

pub(crate) fn persist_session_metadata_worker_hold_at(
    path: &std::path::Path,
    worker_name: &str,
    held: bool,
) -> std::io::Result<()> {
    update_session_metadata_at(path, |metadata| {
        metadata.held_workers.retain(|name| name != worker_name);
        if held {
            metadata.held_workers.push(worker_name.to_string());
        }
        metadata.held_workers.sort();
        metadata.held_workers.dedup();
    })
}

/// Record a supervisor MCP call in the named factory session. Factory worker
/// MCP servers share the same environment shape, so identity is corroborated
/// against the metadata supervisor name before the timestamp is changed.
pub(crate) fn record_supervisor_mcp_call() {
    if std::env::var("CAS_AGENT_ROLE").ok().as_deref() != Some("supervisor") {
        return;
    }
    let Some(session) = std::env::var("CAS_FACTORY_SESSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return;
    };
    let Some(agent_name) = std::env::var("CAS_AGENT_NAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return;
    };
    let path = crate::ui::factory::session::metadata_path(&session);
    let _ = update_session_metadata_at(&path, |metadata| {
        if metadata.supervisor.name == agent_name {
            metadata.last_supervisor_mcp_call_at = Some(chrono::Utc::now());
        }
    });
}

pub(crate) fn supervisor_progress_from_session_metadata_named(
    session: &str,
) -> Option<(
    Option<chrono::DateTime<chrono::Utc>>,
    crate::ui::factory::director::SupervisorStallTracker,
)> {
    let data = fs::read_to_string(crate::ui::factory::session::metadata_path(session)).ok()?;
    let metadata = serde_json::from_str::<SessionMetadata>(&data).ok()?;
    Some((
        metadata.last_supervisor_mcp_call_at,
        metadata.supervisor_stall,
    ))
}

pub(crate) fn persist_session_metadata_supervisor_stall_at(
    path: &std::path::Path,
    tracker: &crate::ui::factory::director::SupervisorStallTracker,
) -> std::io::Result<()> {
    update_session_metadata_at(path, |metadata| {
        metadata.supervisor_stall = tracker.clone();
    })
}

pub(crate) fn update_session_metadata_at(
    path: &std::path::Path,
    mutator: impl FnOnce(&mut SessionMetadata),
) -> std::io::Result<()> {
    use fs2::FileExt;

    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("session metadata path has no parent: {}", path.display()),
        )
    })?;
    fs::create_dir_all(parent)?;
    let lock_path = path.with_extension("json.lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    lock_file.lock_exclusive()?;

    let result = (|| {
        let data = fs::read_to_string(path)?;
        let mut metadata = serde_json::from_str::<SessionMetadata>(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        mutator(&mut metadata);
        let json = serde_json::to_string_pretty(&metadata)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        atomic_write_session_metadata(path, &json)
    })();
    // cas-cef2: release explicitly so a forked child cannot extend this
    // metadata critical section merely by inheriting the descriptor.
    let unlock_result = FileExt::unlock(&lock_file);
    match (result, unlock_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(error),
    }
}

fn atomic_write_session_metadata(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("session metadata path has no parent: {}", path.display()),
        )
    })?;
    let file_name = path.file_name().and_then(|s| s.to_str()).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("non-UTF8 session metadata file name: {}", path.display()),
        )
    })?;

    if let Ok(md) = fs::symlink_metadata(path) {
        if md.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "{} is a symlink; refusing to write through it",
                    path.display()
                ),
            ));
        }
    }

    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let tmp_path = parent.join(format!(
        ".{file_name}.cas-session.{}.{sequence}.tmp",
        std::process::id()
    ));

    atomic_write_session_metadata_via_temp_path(path, contents, &tmp_path)
}

fn atomic_write_session_metadata_via_temp_path(
    path: &std::path::Path,
    contents: &str,
    tmp_path: &std::path::Path,
) -> std::io::Result<()> {
    // A failed create_new means another writer owns the path. Return without
    // cleanup so this invocation can never delete a peer's tempfile.
    let mut f = File::options()
        .create_new(true)
        .write(true)
        .open(tmp_path)?;

    let result = (|| -> std::io::Result<()> {
        f.write_all(contents.as_bytes())?;
        f.flush()?;
        drop(f);
        fs::rename(tmp_path, path)
    })();
    if let Err(err) = result {
        let _ = fs::remove_file(tmp_path);
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::test_support::TestEnvGuard;
    use cas_factory::{EpicState, FileChangeInfo, GitFileStatus, SourceChangesInfo, TaskSummary};
    use cas_types::{Entry, Priority, Task, TaskStatus, TaskType};

    use super::{
        DirectorData, DirectorEvent, merge_director_data_preserving_git, non_closed_task_ids,
        queue_codex_worker_intro_prompt, queue_supervisor_intro_prompt,
        stamp_trusted_factory_supervisor, unfiltered_snapshot_from,
    };

    #[test]
    fn trusted_factory_spawn_stamps_existing_standard_session_as_supervisor() {
        use crate::store::open_agent_store;
        use crate::types::{Agent, AgentRole};

        let temp = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(temp.path()).unwrap();
        let store = open_agent_store(&cas_root).unwrap();
        let standard = Agent::new("spawned-session".to_string(), "wise-viper-85".to_string());
        store.register(&standard).unwrap();

        stamp_trusted_factory_supervisor(
            store.as_ref(),
            "spawned-session",
            "wise-viper-85",
            "factory-live",
        )
        .unwrap();

        let stamped = store.get("spawned-session").unwrap();
        assert_eq!(stamped.role, AgentRole::Supervisor);
        assert_eq!(stamped.factory_session.as_deref(), Some("factory-live"));

        // MCP eager auto-registration later uses register() for the same id;
        // the store must retain the trusted role instead of reverting it to
        // AgentRole::Standard.
        let reregistered = Agent::new("spawned-session".to_string(), "wise-viper-85".to_string());
        store.register(&reregistered).unwrap();
        assert_eq!(
            store.get("spawned-session").unwrap().role,
            AgentRole::Supervisor
        );
    }

    /// cas-0263: the Claude launch surface is the queued intro prompt (Claude has
    /// no --rules/developer_instructions flag). This drives the ACTUAL enqueue
    /// path and proves the queued prompt carries the full canonical role
    /// contract — the Claude supervisor previously got nothing here, the worker
    /// only a skill pointer.
    #[test]
    fn test_claude_intro_prompts_carry_full_role_contract() {
        use crate::store::detect::open_prompt_queue_store;

        let tmp = tempfile::tempdir().unwrap();
        let cas_dir = tmp.path();

        queue_supervisor_intro_prompt(
            cas_dir,
            "sup",
            cas_mux::SupervisorCli::Claude,
            &["worker-a".to_string()],
            None,
            None,
        );
        queue_codex_worker_intro_prompt(cas_dir, "worker-a", cas_mux::SupervisorCli::Claude);

        let queue = open_prompt_queue_store(cas_dir).unwrap();

        let sup = queue.peek_for_targets(&["sup"], None, 10).unwrap();
        assert_eq!(
            sup.len(),
            1,
            "Claude supervisor must receive a launch intro"
        );
        assert!(
            cas_mux::missing_contract_elements(&sup[0].prompt, cas_mux::ContractRole::Supervisor)
                .is_empty(),
            "Claude supervisor intro missing: {:?}",
            cas_mux::missing_contract_elements(&sup[0].prompt, cas_mux::ContractRole::Supervisor)
        );

        let wrk = queue.peek_for_targets(&["worker-a"], None, 10).unwrap();
        assert_eq!(wrk.len(), 1, "Claude worker must receive a launch intro");
        assert!(
            cas_mux::missing_contract_elements(&wrk[0].prompt, cas_mux::ContractRole::Worker)
                .is_empty(),
            "Claude worker intro missing: {:?}",
            cas_mux::missing_contract_elements(&wrk[0].prompt, cas_mux::ContractRole::Worker)
        );
    }

    /// cas-2085 / GH #290: Claude 2.1.231 did not dispatch SessionStart for a
    /// live factory supervisor even though its custom config dir and per-team
    /// `--settings` file both contained the hook. The guaranteed launch-time
    /// intro prompt must therefore carry the context bundle as a fallback.
    #[test]
    fn claude_custom_config_supervisor_intro_carries_context_fallback() {
        use crate::store::{detect::open_prompt_queue_store, init_cas_dir, open_store};

        let project = tempfile::tempdir().unwrap();
        let cas_dir = init_cas_dir(project.path()).unwrap();
        let custom_config = project.path().join(".claude-support@example.test");
        let _guard = TestEnvGuard::with_vars(&[(
            "CLAUDE_CONFIG_DIR",
            custom_config.to_str().expect("UTF-8 config dir"),
        )]);

        open_store(&cas_dir)
            .unwrap()
            .add(&Entry::new(
                "ambient-launch-shape".to_string(),
                "custom profile ambient memory sentinel".to_string(),
            ))
            .unwrap();

        queue_supervisor_intro_prompt(
            &cas_dir,
            "sup",
            cas_mux::SupervisorCli::Claude,
            &[],
            Some("custom-config-factory"),
            Some("custom-config-factory"),
        );

        let queue = open_prompt_queue_store(&cas_dir).unwrap();
        let rows = queue
            .peek_for_targets(&["sup"], Some("custom-config-factory"), 10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0]
                .prompt
                .contains("custom profile ambient memory sentinel"),
            "custom-config supervisor launch must receive ambient context even when Claude skips SessionStart: {}",
            rows[0].prompt
        );
    }

    /// Codex has no SessionStart hook, so its queued startup intro is the
    /// delivery path for both explicit supervisor skills and a real
    /// ambient-recall packet.
    #[test]
    fn codex_supervisor_intro_carries_ambient_recall_and_supervisor_skills() {
        use crate::store::{
            detect::open_prompt_queue_store, init_cas_dir, open_store, open_task_store_local,
        };

        let project = tempfile::tempdir().unwrap();
        let cas_dir = init_cas_dir(project.path()).unwrap();
        let _env =
            TestEnvGuard::with_optional_vars(&[(crate::internal_llm::INTERNAL_LLM_ENV, None)]);
        let mut task = Task::new(
            "cas-codex-launch".to_string(),
            "Coordinate Codex release recovery audit".to_string(),
        );
        task.status = TaskStatus::InProgress;
        open_task_store_local(&cas_dir).unwrap().add(&task).unwrap();
        let mut entry = Entry::new(
            "codex-supervisor-ambient".to_string(),
            "Current Codex release recovery audit preserves operator intent".to_string(),
        );
        entry.title = Some("Current Codex release recovery guidance".to_string());
        entry.tags = vec!["release".to_string(), "recovery".to_string()];
        entry.importance = 0.95;
        open_store(&cas_dir).unwrap().add(&entry).unwrap();

        queue_supervisor_intro_prompt(
            &cas_dir,
            "codex-sup",
            cas_mux::SupervisorCli::Codex,
            &[],
            Some("codex-session-start"),
            Some("codex-factory-session"),
        );

        let queue = open_prompt_queue_store(&cas_dir).unwrap();
        let rows = queue
            .peek_for_targets(&["codex-sup"], Some("codex-factory-session"), 10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        let prompt = &rows[0].prompt;
        assert!(
            prompt.contains("[ambient recall v1 role=supervisor"),
            "Codex's queued startup intro must carry a non-empty ambient packet: {prompt}"
        );
        assert!(
            prompt.contains("codex-supervisor-ambient"),
            "ambient packet must surface the matched high-importance Codex supervisor memory: {prompt}"
        );
        for skill in ["cas-supervisor", "cas-codex-supervisor-checklist"] {
            assert!(
                prompt.contains(skill),
                "Codex's queued startup payload must load {skill}: {prompt}"
            );
        }
        assert!(
            prompt.len() <= 8_000,
            "Codex's measured queued supervisor payload is {} bytes, over the 8KB supervisor-guidance soft ceiling",
            prompt.len()
        );
    }

    /// Grok discards SessionStart hook stdout. Its queued startup intro is the
    /// harness-specific delivery path, so it must carry both the explicit
    /// supervisor skills and a real ambient-recall packet.
    #[test]
    fn grok_supervisor_intro_carries_ambient_recall_and_supervisor_skills() {
        use crate::store::{
            detect::open_prompt_queue_store, init_cas_dir, open_store, open_task_store_local,
        };

        let project = tempfile::tempdir().unwrap();
        let cas_dir = init_cas_dir(project.path()).unwrap();
        let _env =
            TestEnvGuard::with_optional_vars(&[(crate::internal_llm::INTERNAL_LLM_ENV, None)]);
        let mut task = Task::new(
            "cas-grok-launch".to_string(),
            "Coordinate Grok release recovery audit".to_string(),
        );
        task.status = TaskStatus::InProgress;
        open_task_store_local(&cas_dir).unwrap().add(&task).unwrap();
        let mut entry = Entry::new(
            "grok-supervisor-ambient".to_string(),
            "Current Grok release recovery audit preserves operator intent".to_string(),
        );
        entry.title = Some("Current Grok release recovery guidance".to_string());
        entry.tags = vec!["release".to_string(), "recovery".to_string()];
        entry.importance = 0.95;
        open_store(&cas_dir).unwrap().add(&entry).unwrap();

        queue_supervisor_intro_prompt(
            &cas_dir,
            "grok-sup",
            cas_mux::SupervisorCli::Grok,
            &[],
            Some("grok-session-start"),
            Some("grok-factory-session"),
        );

        let queue = open_prompt_queue_store(&cas_dir).unwrap();
        let rows = queue
            .peek_for_targets(&["grok-sup"], Some("grok-factory-session"), 10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        let prompt = &rows[0].prompt;
        assert!(
            prompt.contains("[ambient recall v1 role=supervisor"),
            "Grok's actual SessionStart delivery payload must carry a non-empty ambient packet: {prompt}"
        );
        assert!(
            prompt.contains("grok-supervisor-ambient"),
            "ambient packet must surface the matched high-importance Grok supervisor memory: {prompt}"
        );
        for skill in ["cas-supervisor", "cas-supervisor-checklist"] {
            assert!(
                prompt.contains(skill),
                "Grok's SessionStart payload must load {skill}: {prompt}"
            );
        }
    }

    fn data_with_changes(git_loaded: bool, changes: Vec<SourceChangesInfo>) -> DirectorData {
        DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes,
            git_loaded,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        }
    }

    fn data_with_epics(epics: Vec<TaskSummary>) -> DirectorData {
        DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: epics,
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        }
    }

    /// cas-728b simplify pass: hoisted from two byte-identical local copies
    /// (one per cas_dbbe test) so this fixture builder isn't pasted a third
    /// time by whichever test needs it next.
    fn task(id: &str, epic: &str, assignee: &str) -> TaskSummary {
        TaskSummary {
            id: id.to_string(),
            title: format!("Subtask {id}"),
            status: TaskStatus::InProgress,
            priority: Priority::MEDIUM,
            assignee: Some(assignee.to_string()),
            task_type: TaskType::Task,
            epic: Some(epic.to_string()),
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        }
    }

    /// Epic-kind `TaskSummary` fixture, branch-focused (see `epic_summary`
    /// above for the title/status-focused variant used elsewhere).
    fn epic(id: &str, branch: Option<&str>) -> TaskSummary {
        TaskSummary {
            id: id.to_string(),
            title: format!("Epic {id}"),
            status: TaskStatus::InProgress,
            priority: Priority::HIGH,
            assignee: None,
            task_type: TaskType::Epic,
            epic: None,
            branch: branch.map(str::to_string),
            updated_at: None,
            epic_verification_owner: None,
        }
    }

    fn epic_summary(id: &str, title: &str, status: TaskStatus) -> TaskSummary {
        TaskSummary {
            id: id.to_string(),
            title: title.to_string(),
            status,
            priority: Priority::MEDIUM,
            assignee: None,
            task_type: TaskType::Epic,
            epic: None,
            branch: Some(format!("epic/{id}")),
            updated_at: None,
            epic_verification_owner: None,
        }
    }

    /// cas-a91b review finding: `director.rs::load_with_stores` pushes EVERY
    /// epic into `epic_tasks` regardless of status, including `Closed` —
    /// unlike `ready_tasks`/`in_progress_tasks`, which the bucketing switch
    /// already excludes Closed from. `non_closed_task_ids` must filter
    /// `epic_tasks` itself rather than trusting it's already non-closed, or
    /// a task blocked by an already-closed epic stays gated forever (over-
    /// excluding a task `TaskStore::list_ready()` would correctly show ready).
    #[test]
    fn test_a91b_non_closed_task_ids_excludes_closed_epics() {
        let data = DirectorData {
            ready_tasks: vec![],
            in_progress_tasks: vec![],
            epic_tasks: vec![
                epic_summary("epic-open", "Open Epic", TaskStatus::Open),
                epic_summary("epic-closed", "Closed Epic", TaskStatus::Closed),
            ],
            agents: vec![],
            activity: vec![],
            agent_id_to_name: HashMap::new(),
            changes: vec![],
            git_loaded: true,
            reminders: vec![],
            epic_closed_counts: HashMap::new(),
        };

        let ids = non_closed_task_ids(&data);
        assert!(
            ids.contains("epic-open"),
            "an Open epic must count as non-closed"
        );
        assert!(
            !ids.contains("epic-closed"),
            "a Closed epic must NOT count as non-closed — its blockees should be treated \
             as ready, matching TaskStore::list_ready()'s blocker.status != 'closed' check"
        );
    }

    fn task_summary(
        id: &str,
        title: &str,
        epic: Option<&str>,
        assignee: Option<&str>,
    ) -> TaskSummary {
        TaskSummary {
            id: id.to_string(),
            title: title.to_string(),
            status: TaskStatus::Open,
            priority: Priority::MEDIUM,
            assignee: assignee.map(str::to_string),
            task_type: TaskType::Task,
            epic: epic.map(str::to_string),
            branch: None,
            updated_at: None,
            epic_verification_owner: None,
        }
    }

    // ── task_belongs_to_current_session (cas-889d / cas-9eae) ────────────────
    //
    // Locks in the invariant that ultimately gates the director's "has
    // closed task" broadcast: an in-progress task must never be dropped
    // from the director's visible set (and thus mistaken for a completion)
    // just because its `assignee` uses a different representation
    // (session ID vs display name) than the one being checked.
    mod task_belongs_to_current_session_tests {
        use std::collections::HashSet;

        use cas_factory::TaskSummary;
        use cas_types::{Priority, TaskStatus, TaskType};

        use super::super::task_belongs_to_current_session;

        fn task(epic: Option<&str>, assignee: Option<&str>) -> TaskSummary {
            TaskSummary {
                id: "cas-test1".to_string(),
                title: "Test task".to_string(),
                status: TaskStatus::InProgress,
                priority: Priority::MEDIUM,
                assignee: assignee.map(str::to_string),
                task_type: TaskType::Bug,
                epic: epic.map(str::to_string),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            }
        }

        fn set(items: &[&str]) -> HashSet<String> {
            items.iter().map(|s| s.to_string()).collect()
        }

        /// The bug-report scenario: task is correctly tagged to the active
        /// epic, but its assignee is a session ID that happens not to be in
        /// EITHER lookup set (e.g. a worker that just crashed/reconnected).
        /// Epic membership alone must be sufficient — assignee format must
        /// never exclude an epic-tagged in-progress task.
        #[test]
        fn epic_tagged_task_is_visible_regardless_of_assignee_shape() {
            let t = task(Some("cas-ff98"), Some("sess-id-abc123"));
            assert!(task_belongs_to_current_session(
                &t,
                "cas-ff98",
                &set(&[]),
                &set(&[]),
            ));
        }

        /// cas-889d: task has no epic link yet (read race) and its assignee
        /// is a session ID — must be visible via `allowed_session_ids`, not
        /// just `allowed_names`. Before cas-889d this was dropped, which is
        /// the root cause of the false "has closed task" broadcast in
        /// cas-9eae: the task disappeared from the director's tracked set
        /// while still genuinely `InProgress`.
        #[test]
        fn epic_less_task_visible_via_session_id_assignee() {
            let t = task(None, Some("sess-id-abc123"));
            assert!(task_belongs_to_current_session(
                &t,
                "cas-ff98",
                &set(&["swift-fox"]),      // display names only
                &set(&["sess-id-abc123"]), // session IDs (the fix)
            ));
        }

        /// Same read-race case, but assignee is display-name keyed (legacy
        /// manual assignment path) — must still be visible via
        /// `allowed_names`.
        #[test]
        fn epic_less_task_visible_via_display_name_assignee() {
            let t = task(None, Some("swift-fox"));
            assert!(task_belongs_to_current_session(
                &t,
                "cas-ff98",
                &set(&["swift-fox"]),
                &set(&["sess-id-abc123"]),
            ));
        }

        /// Negative control: no epic link and an assignee absent from both
        /// lookup sets (cross-project leakage / a worker from a different
        /// session) must stay excluded.
        #[test]
        fn epic_less_task_with_unknown_assignee_is_excluded() {
            let t = task(None, Some("sess-id-other-session"));
            assert!(!task_belongs_to_current_session(
                &t,
                "cas-ff98",
                &set(&["swift-fox"]),
                &set(&["sess-id-abc123"]),
            ));
        }

        /// Negative control: task belongs to a DIFFERENT epic than the one
        /// currently tracked — must stay excluded even if the assignee is a
        /// current-session worker (prevents cross-epic task leakage).
        #[test]
        fn task_tagged_to_a_different_epic_is_excluded() {
            let t = task(Some("cas-other-epic"), Some("sess-id-abc123"));
            assert!(!task_belongs_to_current_session(
                &t,
                "cas-ff98",
                &set(&["swift-fox"]),
                &set(&["sess-id-abc123"]),
            ));
        }

        /// Negative control: no epic link and no assignee at all — nothing
        /// to match against, must stay excluded.
        #[test]
        fn epic_less_unassigned_task_is_excluded() {
            let t = task(None, None);
            assert!(!task_belongs_to_current_session(
                &t,
                "cas-ff98",
                &set(&["swift-fox"]),
                &set(&["sess-id-abc123"]),
            ));
        }
    }

    #[test]
    fn epic_state_update_before_filter_retains_new_epic_tasks() {
        use cas_factory::{EpicState, TaskSummary};
        use cas_types::{Priority, TaskStatus, TaskType};

        use super::{DirectorEvent, DirectorEventDetector};

        let old_epic_id = "epic-old";
        let new_epic_id = "epic-new";
        let new_epic_title = "New Feature Epic";

        // Simulate director_data with a new Open-with-branch epic and its subtasks
        let mut data = DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "task-1".to_string(),
                title: "Subtask of new epic".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some(new_epic_id.to_string()),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            }],
            in_progress_tasks: vec![TaskSummary {
                id: "task-2".to_string(),
                title: "In-progress subtask".to_string(),
                status: TaskStatus::InProgress,
                priority: Priority::MEDIUM,
                assignee: Some("worker-1".to_string()),
                task_type: TaskType::Task,
                epic: Some(new_epic_id.to_string()),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            }],
            epic_tasks: vec![TaskSummary {
                id: new_epic_id.to_string(),
                title: new_epic_title.to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Epic,
                epic: None,
                branch: Some("epic/new-feature".to_string()),
                updated_at: None,
                epic_verification_owner: None,
            }],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        // Start with stale epic_state pointing to old epic
        let mut epic_state = EpicState::Active {
            epic_id: old_epic_id.to_string(),
            epic_title: "Old Epic".to_string(),
        };

        // Event detector sees the new epic and fires EpicStarted
        let mut detector =
            DirectorEventDetector::new(vec!["worker-1".to_string()], "supervisor".to_string());
        // Initialize with empty state so detector sees the new epic as new
        detector.initialize(&DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        });

        let events = detector.detect_changes(&data, None);

        // Verify EpicStarted was fired
        let epic_started = events.iter().any(
            |e| matches!(e, DirectorEvent::EpicStarted { epic_id, .. } if epic_id == new_epic_id),
        );
        assert!(epic_started, "EpicStarted event should fire for new epic");

        // THE FIX: Update epic_state from events BEFORE filtering
        for event in &events {
            if let DirectorEvent::EpicStarted {
                epic_id,
                epic_title,
            } = event
            {
                epic_state = EpicState::Active {
                    epic_id: epic_id.clone(),
                    epic_title: epic_title.clone(),
                };
            }
        }

        // Filter tasks to active epic (simulating filter_director_agents_to_current_session)
        if let Some(eid) = epic_state.epic_id() {
            let eid = eid.to_string();
            data.ready_tasks.retain(|t| t.epic.as_deref() == Some(&eid));
            data.in_progress_tasks
                .retain(|t| t.epic.as_deref() == Some(&eid));
            data.epic_tasks.retain(|t| t.id == eid);
        }

        // Tasks should be retained because epic_state now points to new epic
        assert_eq!(
            data.ready_tasks.len(),
            1,
            "ready_tasks should not be empty after filter"
        );
        assert_eq!(
            data.in_progress_tasks.len(),
            1,
            "in_progress_tasks should not be empty after filter"
        );
        assert_eq!(
            data.epic_tasks.len(),
            1,
            "epic_tasks should have the new epic"
        );
    }

    /// cas-dbbe: pins the exact mechanism behind the director's false "has
    /// closed task X" broadcast, observed live when two epics are worked
    /// concurrently in the same factory session (repro task IDs cas-fc44 /
    /// cas-9dc0 on epic cas-6d83, cas-7baa / cas-c0f9 / cas-972c on epic
    /// cas-604d — both epics active in the same session, different workers).
    ///
    /// `filter_director_agents_to_current_session` mutates `director_data`
    /// in place, epic-scoping it to whatever epic the director currently
    /// tracks. On a tick where nothing changed in the DB (`db_changed =
    /// false`) but the git-refresh interval elapsed, `director_data` is NOT
    /// reloaded — it stays as that filtered leftover. Feeding THAT into
    /// `detect_changes` makes every task belonging to the OTHER
    /// (untracked) epic look like it vanished, firing a fabricated
    /// `TaskCompleted` even though the task never closed.
    ///
    /// First half proves the bug: feeding the detector the filtered
    /// snapshot on the second tick fires a false completion for the
    /// other-epic task. Second half proves the fix: feeding it the
    /// preserved unfiltered snapshot (what `FactoryApp.unfiltered_director_data`
    /// now holds) does not.
    #[test]
    fn cas_dbbe_stale_filtered_snapshot_falsely_completes_other_epic_task() {
        use super::{DirectorEvent, DirectorEventDetector};

        let tracked_epic = "epic-604d";
        let other_epic = "epic-6d83";

        // Full, unfiltered snapshot: both epics' in-progress tasks visible —
        // this is what a fresh `DirectorData::load_with_stores` returns, and
        // what `FactoryApp.unfiltered_director_data` is meant to always hold.
        let full_data = DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: vec![
                task("cas-972c", tracked_epic, "jolly-koala-50"),
                task("cas-9dc0", other_epic, "strong-jaguar-64"),
            ],
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: true,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        // Simulate `filter_director_agents_to_current_session` scoping the
        // display copy to `tracked_epic` only — exactly what happens to
        // `FactoryApp.director_data` at the end of a `db_changed` tick.
        let mut filtered_data = full_data.clone();
        filtered_data
            .in_progress_tasks
            .retain(|t| t.epic.as_deref() == Some(tracked_epic));
        assert_eq!(
            filtered_data.in_progress_tasks.len(),
            1,
            "sanity: filter should drop the other-epic task"
        );

        // --- Proves the bug: feeding the STALE FILTERED snapshot on the
        // next tick (simulating db_changed=false, git_due=true leaving
        // director_data un-reloaded) fires a false TaskCompleted for the
        // other-epic task, which never actually closed.
        let mut buggy_detector = DirectorEventDetector::new(
            vec!["jolly-koala-50".to_string(), "strong-jaguar-64".to_string()],
            "supervisor".to_string(),
        );
        buggy_detector.initialize(&full_data);
        let buggy_events = buggy_detector.detect_changes(&filtered_data, Some(tracked_epic));
        assert!(
            buggy_events.iter().any(|e| matches!(
                e,
                DirectorEvent::TaskCompleted { task_id, .. } if task_id == "cas-9dc0"
            )),
            "bug reproduction failed: expected a fabricated TaskCompleted for the \
             other-epic task when fed the stale-filtered snapshot; got {buggy_events:?}"
        );

        // --- Proves the fix: feeding the PRESERVED UNFILTERED snapshot on
        // the next tick (what `unfiltered_director_data` now guarantees)
        // must not report the other-epic task as completed — it's still
        // genuinely InProgress.
        let mut fixed_detector = DirectorEventDetector::new(
            vec!["jolly-koala-50".to_string(), "strong-jaguar-64".to_string()],
            "supervisor".to_string(),
        );
        fixed_detector.initialize(&full_data);
        let fixed_events = fixed_detector.detect_changes(&full_data, Some(tracked_epic));
        assert!(
            !fixed_events.iter().any(|e| matches!(
                e,
                DirectorEvent::TaskCompleted { task_id, .. } if task_id == "cas-9dc0"
            )),
            "false completion survived the fix: other-epic task cas-9dc0 must not be \
             reported completed while still InProgress; got {fixed_events:?}"
        );
    }

    /// cas-dbbe: `filter_director_agents_to_current_session()` must mutate
    /// only `director_data` (the TUI display copy) and must never touch
    /// `unfiltered_director_data` — the canonical snapshot that change
    /// detection and the `TaskCompleted` safety net rely on to see tasks
    /// belonging to a second epic worked concurrently in the same session.
    #[test]
    fn cas_dbbe_filter_leaves_unfiltered_snapshot_untouched() {
        use cas_factory::EpicState;

        let tracked_epic = "epic-604d";
        let other_epic = "epic-6d83";

        let data = DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: vec![
                task("cas-972c", tracked_epic, "jolly-koala-50"),
                task("cas-9dc0", other_epic, "strong-jaguar-64"),
            ],
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: true,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        let mut app = super::FactoryApp::for_test();
        app.worker_names = vec!["jolly-koala-50".to_string(), "strong-jaguar-64".to_string()];
        app.epic_state = EpicState::Active {
            epic_id: tracked_epic.to_string(),
            epic_title: "Isolate concurrent factory sessions".to_string(),
        };
        app.director_data = data.clone();
        app.unfiltered_director_data = data;

        app.filter_director_agents_to_current_session();

        assert_eq!(
            app.director_data.in_progress_tasks.len(),
            1,
            "director_data (display copy) should be scoped to the tracked epic"
        );
        assert_eq!(
            app.unfiltered_director_data.in_progress_tasks.len(),
            2,
            "unfiltered_director_data must retain the other epic's in-progress task \
             untouched by the display filter"
        );
        assert!(
            app.unfiltered_director_data
                .in_progress_tasks
                .iter()
                .any(|t| t.id == "cas-9dc0"),
            "other-epic task must still be present in the unfiltered snapshot"
        );
    }

    /// cas-eb7f (review finding, cas-ebc1 final): `branch_visible_epics_for_ahead_behind`
    /// had zero direct or indirect test coverage. Covers: current epic
    /// included even with no referencing task; epics referenced only via
    /// ready/in-progress tasks are also included; an epic with no branch is
    /// silently dropped; an epic_tasks entry not referenced by anything
    /// (current epic, ready, or in-progress) is excluded.
    #[test]
    fn branch_visible_epics_for_ahead_behind_selects_current_and_referenced_epics_with_branches() {
        use cas_factory::TaskSummary;
        use cas_types::{Priority, TaskStatus, TaskType};

        fn subtask(id: &str, epic_id: &str, status: TaskStatus) -> TaskSummary {
            TaskSummary {
                id: id.to_string(),
                title: format!("Subtask {id}"),
                status,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some(epic_id.to_string()),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            }
        }

        let mut app = super::FactoryApp::for_test();
        app.current_epic_id = Some("epic-current".to_string());
        app.director_data = DirectorData {
            ready_tasks: vec![subtask("cas-1", "epic-ready-ref", TaskStatus::Open)],
            in_progress_tasks: vec![subtask(
                "cas-2",
                "epic-inprogress-ref",
                TaskStatus::InProgress,
            )],
            epic_tasks: vec![
                // Current epic: included even though no task references it.
                epic("epic-current", Some("epic/current")),
                // Referenced only via a ready task's `epic` field.
                epic("epic-ready-ref", Some("epic/ready-ref")),
                // Referenced only via an in-progress task's `epic` field.
                epic("epic-inprogress-ref", Some("epic/inprogress-ref")),
                // Referenced (by current_epic_id would not apply) but has NO
                // branch — must be silently dropped, not (id, "").
                epic("epic-no-branch", None),
                // Not referenced by current_epic_id, ready, or in_progress —
                // must be excluded entirely.
                epic("epic-unrelated", Some("epic/unrelated")),
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };
        // "epic-no-branch" has no task referencing it either, so it must be
        // reachable only via the current_epic_id path to prove the
        // no-branch filter specifically (not just absence from the set).
        app.director_data
            .ready_tasks
            .push(subtask("cas-3", "epic-no-branch", TaskStatus::Open));

        let mut visible = app.branch_visible_epics_for_ahead_behind();
        visible.sort();

        let mut expected = vec![
            ("epic-current".to_string(), "epic/current".to_string()),
            ("epic-ready-ref".to_string(), "epic/ready-ref".to_string()),
            (
                "epic-inprogress-ref".to_string(),
                "epic/inprogress-ref".to_string(),
            ),
        ];
        expected.sort();

        assert_eq!(
            visible, expected,
            "must include current epic + ready/in-progress-referenced epics with a \
             branch, and must exclude branch-less and unrelated epics"
        );
    }

    /// Duplicate references (current_epic_id also referenced by a ready
    /// task's `epic` field) must not produce a duplicate entry — the
    /// underlying HashSet dedupes before the epic_tasks filter runs.
    #[test]
    fn branch_visible_epics_for_ahead_behind_dedupes_current_epic_referenced_by_a_task() {
        use cas_factory::TaskSummary;
        use cas_types::{Priority, TaskStatus, TaskType};

        let mut app = super::FactoryApp::for_test();
        app.current_epic_id = Some("epic-shared".to_string());
        app.director_data = DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "cas-1".to_string(),
                title: "Subtask".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some("epic-shared".to_string()),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            }],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![TaskSummary {
                id: "epic-shared".to_string(),
                title: "Shared epic".to_string(),
                status: TaskStatus::InProgress,
                priority: Priority::HIGH,
                assignee: None,
                task_type: TaskType::Epic,
                epic: None,
                branch: Some("epic/shared".to_string()),
                updated_at: None,
                epic_verification_owner: None,
            }],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        let visible = app.branch_visible_epics_for_ahead_behind();

        assert_eq!(
            visible,
            vec![("epic-shared".to_string(), "epic/shared".to_string())],
            "current epic referenced by both current_epic_id and a task's epic \
             field must yield exactly one entry, not a duplicate"
        );
    }

    /// cas-eb7f (review finding, cas-ebc1 final): `set_factory_session`'s
    /// rfc3339 `created_at` parsing had no test for the missing-file,
    /// malformed-JSON, or non-RFC3339 cases — all of which currently
    /// silently degrade to `None` via `.ok()`. Also proves the happy path
    /// actually populates `session_created_at`, so a future format drift
    /// between the metadata writer and this parser has a failing test.
    ///
    /// Builds `SessionMetadata` via the real struct + `serde_json::to_string`
    /// (not a hand-written JSON literal) so this test can't drift from the
    /// struct's actual required-field shape as fields are added.
    ///
    /// Uses the canonical `TestEnvGuard` to override `HOME`, so parallel tests
    /// cannot interleave process-global environment writes.
    #[test]
    fn set_factory_session_handles_missing_malformed_and_valid_created_at() {
        use crate::ui::factory::protocol::{AgentInfo, SessionMetadata};

        fn sample_metadata(name: &str, created_at: &str) -> SessionMetadata {
            SessionMetadata {
                name: name.to_string(),
                created_at: created_at.to_string(),
                daemon_pid: 1,
                daemon_pid_starttime: None,
                socket_path: "socket".to_string(),
                ws_port: None,
                log_dir: None,
                daemon_log_path: None,
                daemon_trace_log_path: None,
                server_log_path: None,
                server_trace_log_path: None,
                tui_log_path: None,
                panic_log_path: None,
                supervisor: AgentInfo {
                    name: "supervisor".to_string(),
                    pid: None,
                    worktree_path: None,
                },
                workers: Vec::new(),
                epic_id: None,
                pinned_epic_id: None,
                delivery_mode: cas_types::DeliveryMode::PushBranch,
                held_workers: Vec::new(),
                last_supervisor_mcp_call_at: None,
                supervisor_stall: Default::default(),
                project_dir: None,
                team_name: None,
            }
        }

        let home = tempfile::tempdir().unwrap();
        let _guard = TestEnvGuard::with_vars(&[("HOME", home.path().to_str().unwrap())]);
        let sessions_dir = home.path().join(".cas").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let mut app = super::FactoryApp::for_test();

        // Missing metadata file entirely.
        app.set_factory_session("no-such-session".to_string());
        assert_eq!(
            app.session_created_at, None,
            "missing metadata file must degrade to None, not panic"
        );

        // Malformed JSON.
        std::fs::write(sessions_dir.join("bad-json.json"), "{not valid json").unwrap();
        app.set_factory_session("bad-json".to_string());
        assert_eq!(
            app.session_created_at, None,
            "malformed JSON must degrade to None, not panic"
        );

        // Valid JSON (real struct shape), but created_at is not RFC3339.
        let bad_date = sample_metadata("bad-date", "not-a-date");
        std::fs::write(
            sessions_dir.join("bad-date.json"),
            serde_json::to_string(&bad_date).unwrap(),
        )
        .unwrap();
        app.set_factory_session("bad-date".to_string());
        assert_eq!(
            app.session_created_at, None,
            "non-RFC3339 created_at must degrade to None, not panic"
        );

        // Valid JSON with a valid RFC3339 created_at — the happy path.
        let good = sample_metadata("good", "2026-01-01T12:30:00Z");
        std::fs::write(
            sessions_dir.join("good.json"),
            serde_json::to_string(&good).unwrap(),
        )
        .unwrap();
        app.set_factory_session("good".to_string());
        assert_eq!(
            app.session_created_at,
            Some(
                chrono::DateTime::parse_from_rfc3339("2026-01-01T12:30:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc)
            ),
            "valid RFC3339 created_at must populate session_created_at"
        );
    }

    /// cas-60dd: exercise the production bridge between the supervisor's
    /// persisted control state and the detector's live hold gate. Direct
    /// detector tests alone cannot prove the MCP-written state is consumed.
    #[test]
    fn session_worker_holds_reconcile_into_detector_and_clear_on_removal() {
        let home = tempfile::tempdir().unwrap();
        let _guard = TestEnvGuard::with_vars(&[("HOME", home.path().to_str().unwrap())]);
        let session = "worker-hold-reconcile";
        let worker = "lively-crow";
        let path = crate::ui::factory::session::metadata_path(session);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let metadata = crate::ui::factory::session::create_metadata(
            session,
            1,
            "supervisor",
            &[worker.to_string()],
            None,
            None,
            None,
        );
        std::fs::write(&path, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();
        super::persist_session_metadata_worker_hold_at(&path, worker, true).unwrap();

        let mut app = super::FactoryApp::for_test();
        app.worker_names = vec![worker.to_string()];
        app.event_detector.add_worker(worker.to_string());
        app.set_factory_session(session.to_string());
        assert!(
            app.event_detector.is_worker_held(worker),
            "set_factory_session must rehydrate a hold before the first refresh tick"
        );

        super::persist_session_metadata_worker_hold_at(&path, worker, false).unwrap();
        app.apply_session_metadata_worker_holds();
        assert!(
            !app.event_detector.is_worker_held(worker),
            "release state must reconcile into the live gate"
        );

        super::persist_session_metadata_worker_hold_at(&path, worker, true).unwrap();
        app.apply_session_metadata_worker_holds();
        app.remove_worker_from_event_detector(worker);
        assert!(!app.event_detector.is_worker_held(worker));
        let metadata: crate::ui::factory::protocol::SessionMetadata =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            metadata.held_workers.is_empty(),
            "worker removal must clear durable state so a reused name cannot inherit it"
        );
    }

    /// A hold update touches session metadata, not cas.db. The ordinary
    /// unchanged-DB refresh must still synchronize it before the detector can
    /// produce its next idle event.
    #[test]
    fn unchanged_db_refresh_reconciles_worker_holds_before_idle_detection() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let _guard = TestEnvGuard::with_vars(&[
            ("HOME", home.path().to_str().unwrap()),
            ("CAS_FACTORY_SESSION", "worker-hold-fast-path"),
        ]);
        let cas_dir = crate::store::init_cas_dir(project.path()).unwrap();
        let worker = "lively-crow";
        let session = "worker-hold-fast-path";
        let path = crate::ui::factory::session::metadata_path(session);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let metadata = crate::ui::factory::session::create_metadata(
            session,
            1,
            "supervisor",
            &[worker.to_string()],
            None,
            None,
            None,
        );
        std::fs::write(&path, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();
        super::persist_session_metadata_worker_hold_at(&path, worker, true).unwrap();

        let mut app = super::FactoryApp::for_test();
        app.cas_dir = cas_dir.clone();
        app.factory_session = Some(session.to_string());
        app.worker_names = vec![worker.to_string()];
        app.event_detector.add_worker(worker.to_string());
        app.director_data.git_loaded = true;
        app.last_db_fingerprint = Some(super::CasDbFingerprint::from_cas_dir(&cas_dir));
        app.last_git_refresh = std::time::Instant::now();

        assert!(app.refresh_data().unwrap().is_empty());
        assert!(
            app.event_detector.is_worker_held(worker),
            "unchanged-DB refresh must apply the durable hold before idle detection"
        );
    }

    /// cas-728b simplify pass: hoisted from two near-identical PTY scaffolds
    /// (one per test below). `cas_mux::Pane` has no non-PTY Worker
    /// constructor, so these tests spawn a trivial real `cat` process to get
    /// one — mirrors the environment-tolerant pattern in
    /// `crates/cas-pty/src/pty.rs`'s own tests. `pty_id` and `pane_id` are
    /// separate because production panes use a display id distinct from the
    /// underlying PTY's spawn id. Returns `None` on either spawn or
    /// pane-construction failure; callers print their own skip message and
    /// return early (a shared helper can't `return` out of the caller's
    /// test fn).
    fn spawn_cat_worker_pane(pty_id: &str, pane_id: &str) -> Option<cas_mux::Pane> {
        let pty_config = cas_mux::PtyConfig {
            command: "cat".to_string(),
            args: vec![],
            cwd: None,
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        };
        let pty = cas_mux::Pty::spawn(pty_id, pty_config).ok()?;
        cas_mux::Pane::with_pty(
            pane_id,
            cas_mux::PaneKind::Worker,
            pty,
            24,
            80,
            cas_mux::SupervisorCli::Claude,
        )
        .ok()
    }

    /// cas-eb7f (review finding, cas-ebc1 final): `sync_worker_pane_branch_titles`
    /// and its epic_workers.rs call sites had no test proving the
    /// `mux.panes_mut()` filter only touches `cas_mux::PaneKind::Worker` panes. Gives
    /// both a Worker and a Director pane a distinct starting title, runs the
    /// sync, and asserts the Worker pane's title changed while the Director
    /// pane's did not — proving the wiring (not just that *a* title update
    /// happened somewhere).
    #[test]
    fn sync_worker_pane_branch_titles_only_touches_worker_panes() {
        let Some(mut worker_pane) = spawn_cat_worker_pane("test-worker-1", "worker-1") else {
            eprintln!("skipping: `cat`/PTY-backed pane unavailable in this environment");
            return;
        };
        worker_pane.set_title("STALE-WORKER-TITLE");

        let Ok(mut director_pane) = cas_mux::Pane::director("director", 24, 80) else {
            eprintln!("skipping: director pane construction failed");
            return;
        };
        director_pane.set_title("STALE-DIRECTOR-TITLE");

        let mut app = super::FactoryApp::for_test();
        app.worker_names = vec!["worker-1".to_string()];
        app.mux.add_pane(worker_pane);
        app.mux.add_pane(director_pane);

        app.sync_worker_pane_branch_titles();

        let mut saw_worker = false;
        let mut saw_director = false;
        for pane in app.mux.panes_mut() {
            match pane.kind() {
                cas_mux::PaneKind::Worker => {
                    saw_worker = true;
                    assert_ne!(
                        pane.title(),
                        "STALE-WORKER-TITLE",
                        "Worker pane title must be rewritten by sync_worker_pane_branch_titles"
                    );
                }
                cas_mux::PaneKind::Director => {
                    saw_director = true;
                    assert_eq!(
                        pane.title(),
                        "STALE-DIRECTOR-TITLE",
                        "non-Worker panes must not be touched by sync_worker_pane_branch_titles"
                    );
                }
                _ => {}
            }
        }
        assert!(
            saw_worker && saw_director,
            "both fixture panes must still be present"
        );
    }

    /// cas-eb7f (review finding, cas-ebc1 final): the status bar's
    /// branch-label integration point (`app.focused_pane_branch()` feeding
    /// `StatusBar::branch_label_for_width`) only had unit coverage on the
    /// pure formatting helper — no test rendered the status bar against a
    /// real `FactoryApp`/focused pane to prove the wiring itself. Placed
    /// here (not in status_bar.rs) because `FactoryApp`'s `project_dir` and
    /// `branch_visibility` fields are private to this module.
    #[test]
    fn render_status_bar_surfaces_focused_worker_pane_branch_label() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let Some(worker_pane) = spawn_cat_worker_pane("status-bar-worker", "worker-1") else {
            eprintln!("skipping: `cat`/PTY-backed pane unavailable in this environment");
            return;
        };

        let mut app = super::FactoryApp::for_test();
        app.mux.add_pane(worker_pane);
        assert!(app.mux.focus("worker-1"), "fixture pane must be focusable");
        // worktree_manager is None in for_test(), so focused_pane_branch()
        // falls back to project_dir for a focused Worker pane.
        app.branch_visibility
            .insert_path_branch(app.project_dir.clone(), "factory/status-branch");

        let backend = TestBackend::new(120, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                crate::ui::factory::status_bar::StatusBar::render(frame, frame.area(), &app)
            })
            .unwrap();

        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            text.contains("branch factory/status-branch"),
            "status bar must surface the focused pane's branch label; got: {text}"
        );
    }

    /// cas-728b simplify pass on cas-dbbe: `unfiltered_snapshot_from` must
    /// trim `changes`/`activity`/`reminders`/`epic_closed_counts` — neither
    /// `detect_changes` nor the `TaskCompleted` safety net reads them from
    /// `unfiltered_director_data`, so cloning them every `db_changed` tick
    /// (potentially the largest fields in `DirectorData`) was pure waste.
    /// The fields the consumers DO read must survive intact.
    #[test]
    fn unfiltered_snapshot_from_trims_unread_fields_but_keeps_consumed_ones() {
        use cas_types::AgentStatus;

        let mut source = data_with_changes(
            true,
            vec![SourceChangesInfo {
                source_name: "main".to_string(),
                source_path: std::path::PathBuf::from("."),
                agent_name: None,
                changes: vec![FileChangeInfo {
                    file_path: "src/main.rs".to_string(),
                    lines_added: 3,
                    lines_removed: 1,
                    status: GitFileStatus::Modified,
                    staged: false,
                }],
                total_added: 3,
                total_removed: 1,
            }],
        );
        // Populate the fields that ARE supposed to survive with real content
        // so the "preserved" assertions below can't pass trivially on two
        // empty Vecs.
        source.ready_tasks = vec![task("cas-1", "epic-a", "unassigned")];
        source.in_progress_tasks = vec![task("cas-2", "epic-a", "agent-1")];
        source.epic_tasks = vec![epic("epic-a", Some("epic/a"))];
        source.agents = vec![cas_factory::AgentSummary {
            id: "agent-1".to_string(),
            name: "swift-fox".to_string(),
            status: AgentStatus::Active,
            registered_at: chrono::Utc::now(),
            current_task: None,
            latest_activity: None,
            last_heartbeat: None,
            pending_messages: 0,
            pending_supervisor_messages: 0,
            latest_supervisor_message_at: None,
            active_lease: None,
            effort: None,
        }];
        source
            .agent_id_to_name
            .insert("agent-1".to_string(), "swift-fox".to_string());

        let snapshot = unfiltered_snapshot_from(&source);

        // Trimmed: never read by detect_changes or generate_prompt's
        // TaskCompleted safety net.
        assert!(snapshot.changes.is_empty(), "changes must be trimmed");
        assert!(snapshot.activity.is_empty(), "activity must be trimmed");
        assert!(snapshot.reminders.is_empty(), "reminders must be trimmed");
        assert!(
            snapshot.epic_closed_counts.is_empty(),
            "epic_closed_counts must be trimmed"
        );
        assert!(!snapshot.git_loaded, "git_loaded must reset to false");

        // Preserved: what the two consumers actually read.
        assert_eq!(snapshot.ready_tasks.len(), source.ready_tasks.len());
        assert_eq!(
            snapshot.in_progress_tasks.len(),
            source.in_progress_tasks.len()
        );
        assert_eq!(snapshot.epic_tasks.len(), source.epic_tasks.len());
        assert_eq!(snapshot.agents.len(), source.agents.len());
        assert_eq!(snapshot.agent_id_to_name, source.agent_id_to_name);
    }

    #[test]
    fn preserves_previous_changes_when_git_refresh_not_due() {
        let previous = data_with_changes(
            true,
            vec![SourceChangesInfo {
                source_name: "main".to_string(),
                source_path: std::path::PathBuf::from("."),
                agent_name: None,
                changes: vec![FileChangeInfo {
                    file_path: "src/main.rs".to_string(),
                    lines_added: 3,
                    lines_removed: 1,
                    status: GitFileStatus::Modified,
                    staged: false,
                }],
                total_added: 3,
                total_removed: 1,
            }],
        );
        let loaded_without_git = data_with_changes(false, Vec::new());

        let merged = merge_director_data_preserving_git(&previous, loaded_without_git, false);

        assert!(merged.git_loaded);
        assert_eq!(merged.changes.len(), 1);
        assert_eq!(merged.changes[0].source_name, "main");
    }

    #[test]
    fn keeps_loaded_changes_when_git_refresh_is_due() {
        let previous = data_with_changes(true, Vec::new());
        let loaded_with_git = data_with_changes(
            true,
            vec![SourceChangesInfo {
                source_name: "worker-1".to_string(),
                source_path: std::path::PathBuf::from("."),
                agent_name: Some("worker-1".to_string()),
                changes: vec![FileChangeInfo {
                    file_path: "README.md".to_string(),
                    lines_added: 10,
                    lines_removed: 0,
                    status: GitFileStatus::Added,
                    staged: false,
                }],
                total_added: 10,
                total_removed: 0,
            }],
        );

        let merged = merge_director_data_preserving_git(&previous, loaded_with_git, true);

        assert!(merged.git_loaded);
        assert_eq!(merged.changes.len(), 1);
        assert_eq!(merged.changes[0].source_name, "worker-1");
    }

    #[test]
    fn detect_epic_prefers_epic_with_active_subtasks_over_stale() {
        use cas_factory::{EpicState, TaskSummary};
        use cas_types::{Priority, TaskStatus, TaskType};

        let active_epic_id = "cas-active";
        let stale_epic_id = "cas-zzz-stale"; // Higher ID — would win with old heuristic

        let data = DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "task-ready".to_string(),
                title: "Ready subtask".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some(active_epic_id.to_string()),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            }],
            in_progress_tasks: vec![TaskSummary {
                id: "task-ip".to_string(),
                title: "In-progress subtask".to_string(),
                status: TaskStatus::InProgress,
                priority: Priority::MEDIUM,
                assignee: Some("worker-1".to_string()),
                task_type: TaskType::Task,
                epic: Some(active_epic_id.to_string()),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            }],
            epic_tasks: vec![
                TaskSummary {
                    id: stale_epic_id.to_string(),
                    title: "Stale Epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/stale".to_string()),
                    updated_at: None,
                    epic_verification_owner: None,
                },
                TaskSummary {
                    id: active_epic_id.to_string(),
                    title: "Active Epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/active".to_string()),
                    updated_at: None,
                    epic_verification_owner: None,
                },
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        let state = super::detect_epic_state(&data, None);
        match state {
            EpicState::Active { epic_id, .. } => {
                assert_eq!(
                    epic_id, active_epic_id,
                    "Should prefer epic with in-progress subtasks, not stale epic with higher ID"
                );
            }
            other => panic!("Expected Active, got {other:?}"),
        }
    }

    #[test]
    fn detect_epic_falls_back_to_ready_subtasks_when_no_in_progress() {
        use cas_factory::{EpicState, TaskSummary};
        use cas_types::{Priority, TaskStatus, TaskType};

        let active_epic_id = "cas-active";
        let stale_epic_id = "cas-zzz-stale";

        let data = DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "task-ready".to_string(),
                title: "Ready subtask".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some(active_epic_id.to_string()),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            }],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![
                TaskSummary {
                    id: stale_epic_id.to_string(),
                    title: "Stale Epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/stale".to_string()),
                    updated_at: None,
                    epic_verification_owner: None,
                },
                TaskSummary {
                    id: active_epic_id.to_string(),
                    title: "Active Epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/active".to_string()),
                    updated_at: None,
                    epic_verification_owner: None,
                },
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        let state = super::detect_epic_state(&data, None);
        match state {
            EpicState::Active { epic_id, .. } => {
                assert_eq!(
                    epic_id, active_epic_id,
                    "Should prefer epic with ready subtasks over stale epic with no subtasks"
                );
            }
            other => panic!("Expected Active, got {other:?}"),
        }
    }

    #[test]
    fn detect_epic_preferred_id_takes_priority_over_heuristic() {
        use cas_factory::{EpicState, TaskSummary};
        use cas_types::{Priority, TaskStatus, TaskType};

        let preferred_id = "cas-preferred";
        let unrelated_id = "cas-unrelated";

        let data = DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![
                TaskSummary {
                    id: preferred_id.to_string(),
                    title: "Preferred Epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/preferred".to_string()),
                    updated_at: None,
                    epic_verification_owner: None,
                },
                TaskSummary {
                    id: unrelated_id.to_string(),
                    title: "Unrelated InProgress Epic".to_string(),
                    status: TaskStatus::InProgress,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/unrelated".to_string()),
                    updated_at: None,
                    epic_verification_owner: None,
                },
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        // Preferred epic should win even though another epic is explicitly InProgress.
        let state = super::detect_epic_state(&data, Some(preferred_id));
        match state {
            EpicState::Active { epic_id, .. } => {
                assert_eq!(
                    epic_id, preferred_id,
                    "preferred_epic_id should take priority over unrelated InProgress epics"
                );
            }
            other => panic!("Expected Active, got {other:?}"),
        }
    }

    #[test]
    fn detect_epic_preferred_id_takes_priority_over_open_branch_subtask_heuristic() {
        let preferred_id = "cas-preferred";
        let heuristic_id = "cas-heuristic";

        let data = DirectorData {
            ready_tasks: vec![task_summary(
                "cas-ready",
                "Heuristic subtask",
                Some(heuristic_id),
                None,
            )],
            in_progress_tasks: vec![TaskSummary {
                status: TaskStatus::InProgress,
                epic_verification_owner: None,
                ..task_summary(
                    "cas-ip",
                    "Active heuristic subtask",
                    Some(heuristic_id),
                    None,
                )
            }],
            epic_tasks: vec![
                epic_summary(preferred_id, "Preferred Epic", TaskStatus::Open),
                epic_summary(heuristic_id, "Heuristic Epic", TaskStatus::Open),
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        let state = super::detect_epic_state(&data, Some(preferred_id));
        match state {
            EpicState::Active { epic_id, .. } => assert_eq!(
                epic_id, preferred_id,
                "preferred_epic_id should beat the open-branch active-subtask heuristic"
            ),
            other => panic!("Expected Active, got {other:?}"),
        }
    }

    #[test]
    fn persist_session_metadata_epic_id_roundtrips_existing_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let mut metadata = crate::ui::factory::session::create_metadata(
            "test-session",
            12345,
            "supervisor",
            &["worker-1".to_string()],
            Some("cas-old"),
            Some("/tmp/project"),
            Some(4242),
        );
        metadata.team_name = Some("team-a".to_string());
        std::fs::write(&path, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();

        super::persist_session_metadata_epic_id_at(&path, "cas-new").unwrap();

        let updated: crate::ui::factory::protocol::SessionMetadata =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(updated.epic_id, Some("cas-new".to_string()));
        assert_eq!(updated.name, "test-session");
        assert_eq!(updated.ws_port, Some(4242));
        assert_eq!(updated.team_name, Some("team-a".to_string()));
        assert_eq!(updated.project_dir, Some("/tmp/project".to_string()));
    }

    #[test]
    fn preferred_epic_id_from_metadata_uses_pin_before_session_default() {
        let mut metadata = crate::ui::factory::session::create_metadata(
            "test-session",
            12345,
            "supervisor",
            &[],
            Some("cas-session"),
            Some("/tmp/project"),
            None,
        );

        assert_eq!(
            super::preferred_epic_id_from_metadata(&metadata),
            Some("cas-session".to_string())
        );

        metadata.pinned_epic_id = Some("cas-pinned".to_string());
        assert_eq!(
            super::preferred_epic_id_from_metadata(&metadata),
            Some("cas-pinned".to_string())
        );
    }

    #[test]
    fn preferred_epic_focus_from_metadata_tags_sources() {
        let mut metadata = crate::ui::factory::session::create_metadata(
            "test-session",
            12345,
            "supervisor",
            &[],
            Some("cas-session"),
            Some("/tmp/project"),
            None,
        );

        let session_default = super::preferred_epic_focus_from_metadata(&metadata);
        assert_eq!(session_default.epic_id.as_deref(), Some("cas-session"));
        assert_eq!(
            session_default.source,
            Some(super::EpicFocusSource::SessionDefault)
        );

        metadata.pinned_epic_id = Some("cas-pinned".to_string());
        let pinned = super::preferred_epic_focus_from_metadata(&metadata);
        assert_eq!(pinned.epic_id.as_deref(), Some("cas-pinned"));
        assert_eq!(pinned.source, Some(super::EpicFocusSource::Pinned));

        metadata.pinned_epic_id = Some(" ".to_string());
        metadata.epic_id = Some("\t".to_string());
        assert_eq!(
            super::preferred_epic_focus_from_metadata(&metadata),
            super::SessionEpicFocus::default()
        );
    }

    #[test]
    fn preferred_epic_id_from_session_metadata_named_filters_empty_missing_and_malformed() {
        let home = tempfile::tempdir().unwrap();
        let _guard = TestEnvGuard::with_vars(&[("HOME", home.path().to_str().unwrap())]);

        assert_eq!(
            super::preferred_epic_id_from_session_metadata_named("missing-session"),
            None,
            "missing metadata file should resolve no preferred epic"
        );

        let blank_path = crate::ui::factory::session::metadata_path("blank-session");
        std::fs::create_dir_all(blank_path.parent().unwrap()).unwrap();
        let mut blank = crate::ui::factory::session::create_metadata(
            "blank-session",
            12345,
            "supervisor",
            &[],
            Some("   "),
            Some("/tmp/project"),
            None,
        );
        blank.pinned_epic_id = Some("\t".to_string());
        std::fs::write(&blank_path, serde_json::to_string_pretty(&blank).unwrap()).unwrap();
        assert_eq!(
            super::preferred_epic_id_from_session_metadata_named("blank-session"),
            None,
            "empty or whitespace pin/default ids should be filtered"
        );

        let malformed_path = crate::ui::factory::session::metadata_path("malformed-session");
        std::fs::create_dir_all(malformed_path.parent().unwrap()).unwrap();
        std::fs::write(&malformed_path, "{ not json").unwrap();
        assert_eq!(
            super::preferred_epic_id_from_session_metadata_named("malformed-session"),
            None,
            "malformed metadata should resolve no preferred epic"
        );
    }

    #[test]
    fn explicit_focus_sources_render_unassigned_window_but_inference_zero_subtask_is_gated() {
        let focused_id = "cas-focused";
        let zero_id = "cas-zero";
        let data = DirectorData {
            ready_tasks: vec![task_summary(
                "cas-unassigned",
                "Unassigned open subtask",
                Some(focused_id),
                None,
            )],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![
                epic_summary(focused_id, "Focused Epic", TaskStatus::Open),
                epic_summary(zero_id, "Zero Subtask Epic", TaskStatus::InProgress),
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        let pinned = super::resolve_epic_state_for_focus(
            &data,
            &super::SessionEpicFocus {
                epic_id: Some(focused_id.to_string()),
                source: Some(super::EpicFocusSource::Pinned),
            },
        );
        assert_eq!(pinned.epic_id(), Some(focused_id));

        let session_default = super::resolve_epic_state_for_focus(
            &data,
            &super::SessionEpicFocus {
                epic_id: Some(focused_id.to_string()),
                source: Some(super::EpicFocusSource::SessionDefault),
            },
        );
        assert_eq!(session_default.epic_id(), Some(focused_id));

        let zero_only = DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![epic_summary(
                zero_id,
                "Zero Subtask Epic",
                TaskStatus::InProgress,
            )],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };
        assert_eq!(
            super::resolve_epic_state_for_focus(&zero_only, &super::SessionEpicFocus::default())
                .epic_id(),
            None,
            "inference-derived zero-subtask focus should not reach renderers"
        );
    }

    #[test]
    fn inferred_epic_displayability_requires_session_agent_subtask() {
        let epic_id = "cas-focused";
        let mut data = DirectorData {
            ready_tasks: vec![task_summary(
                "cas-foreign",
                "Foreign open subtask",
                Some(epic_id),
                Some("other-agent"),
            )],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![epic_summary(epic_id, "Focused Epic", TaskStatus::Open)],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::from([(
                "session-agent".to_string(),
                "worker-one".to_string(),
            )]),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        assert!(
            !super::inferred_epic_is_displayable(&data, epic_id),
            "foreign-assigned subtasks must not make an inferred epic displayable"
        );

        data.ready_tasks[0].assignee = Some("session-agent".to_string());
        assert!(
            super::inferred_epic_is_displayable(&data, epic_id),
            "session-id assignee should make the inferred epic displayable"
        );

        data.ready_tasks[0].assignee = Some("worker-one".to_string());
        assert!(
            super::inferred_epic_is_displayable(&data, epic_id),
            "session display-name assignee should make the inferred epic displayable"
        );
    }

    #[test]
    fn epic_completed_clears_current_and_session_default_without_clearing_pin() {
        let home = tempfile::tempdir().unwrap();
        let _guard = TestEnvGuard::with_vars(&[
            ("HOME", home.path().to_str().unwrap()),
            ("CAS_FACTORY_SESSION", "session-complete-clear"),
        ]);
        let metadata_path = crate::ui::factory::session::metadata_path("session-complete-clear");
        std::fs::create_dir_all(metadata_path.parent().unwrap()).unwrap();
        let mut metadata = crate::ui::factory::session::create_metadata(
            "session-complete-clear",
            12345,
            "supervisor",
            &[],
            Some("cas-current"),
            Some("/tmp/project"),
            None,
        );
        metadata.pinned_epic_id = Some("cas-pin".to_string());
        std::fs::write(
            &metadata_path,
            serde_json::to_string_pretty(&metadata).unwrap(),
        )
        .unwrap();

        let mut app = super::FactoryApp::for_test();
        app.factory_session = Some("session-complete-clear".to_string());
        app.current_epic_id = Some("cas-current".to_string());
        app.current_epic_source = Some(super::EpicFocusSource::SessionDefault);
        app.epic_state = EpicState::Active {
            epic_id: "cas-current".to_string(),
            epic_title: "Current Epic".to_string(),
        };

        let changes = app.handle_epic_events(&[DirectorEvent::EpicCompleted {
            epic_id: "cas-current".to_string(),
        }]);
        assert_eq!(changes.len(), 1);
        assert_eq!(app.current_epic_id, None);
        assert_eq!(app.current_epic_source, None);

        let updated: crate::ui::factory::protocol::SessionMetadata =
            serde_json::from_str(&std::fs::read_to_string(&metadata_path).unwrap()).unwrap();
        assert_eq!(updated.epic_id, None);
        assert_eq!(updated.pinned_epic_id, Some("cas-pin".to_string()));
        assert_eq!(
            super::preferred_epic_id_from_session_metadata_named("session-complete-clear"),
            Some("cas-pin".to_string()),
            "reload should not resurrect the completed session-default epic"
        );
    }

    #[test]
    fn reset_epic_state_clears_current_and_session_default_without_clearing_pin() {
        let home = tempfile::tempdir().unwrap();
        let _guard = TestEnvGuard::with_vars(&[
            ("HOME", home.path().to_str().unwrap()),
            ("CAS_FACTORY_SESSION", "session-reset-clear"),
        ]);
        let metadata_path = crate::ui::factory::session::metadata_path("session-reset-clear");
        std::fs::create_dir_all(metadata_path.parent().unwrap()).unwrap();
        let mut metadata = crate::ui::factory::session::create_metadata(
            "session-reset-clear",
            12345,
            "supervisor",
            &[],
            Some("cas-current"),
            Some("/tmp/project"),
            None,
        );
        metadata.pinned_epic_id = Some("cas-pin".to_string());
        std::fs::write(
            &metadata_path,
            serde_json::to_string_pretty(&metadata).unwrap(),
        )
        .unwrap();

        let mut app = super::FactoryApp::for_test();
        app.factory_session = Some("session-reset-clear".to_string());
        app.current_epic_id = Some("cas-current".to_string());
        app.current_epic_source = Some(super::EpicFocusSource::SessionDefault);
        app.epic_state = EpicState::Active {
            epic_id: "cas-current".to_string(),
            epic_title: "Current Epic".to_string(),
        };

        app.reset_epic_state();

        assert_eq!(app.current_epic_id, None);
        assert_eq!(app.current_epic_source, None);
        assert!(matches!(app.epic_state, EpicState::Idle));

        let updated: crate::ui::factory::protocol::SessionMetadata =
            serde_json::from_str(&std::fs::read_to_string(&metadata_path).unwrap()).unwrap();
        assert_eq!(updated.epic_id, None);
        assert_eq!(updated.pinned_epic_id, Some("cas-pin".to_string()));
    }

    #[test]
    fn persist_session_metadata_pinned_epic_id_roundtrips_and_clears() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let metadata = crate::ui::factory::session::create_metadata(
            "test-session",
            12345,
            "supervisor",
            &["worker-1".to_string()],
            Some("cas-session"),
            Some("/tmp/project"),
            Some(4242),
        );
        std::fs::write(&path, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();

        super::persist_session_metadata_pinned_epic_id_at(&path, Some("cas-pinned")).unwrap();
        let updated: crate::ui::factory::protocol::SessionMetadata =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(updated.epic_id, Some("cas-session".to_string()));
        assert_eq!(updated.pinned_epic_id, Some("cas-pinned".to_string()));

        super::persist_session_metadata_pinned_epic_id_at(&path, None).unwrap();
        let cleared: crate::ui::factory::protocol::SessionMetadata =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cleared.epic_id, Some("cas-session".to_string()));
        assert_eq!(cleared.pinned_epic_id, None);
        assert_eq!(
            super::preferred_epic_id_from_metadata(&cleared),
            Some("cas-session".to_string())
        );
    }

    #[test]
    fn locked_session_metadata_updates_preserve_concurrent_epic_and_pin_writes() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let metadata = crate::ui::factory::session::create_metadata(
            "test-session",
            12345,
            "supervisor",
            &[],
            Some("cas-epic-initial"),
            Some("/tmp/project"),
            None,
        );
        std::fs::write(&path, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let epic_path = path.clone();
        let epic_barrier = barrier.clone();
        let epic_thread = thread::spawn(move || {
            epic_barrier.wait();
            for i in 0..100 {
                super::persist_session_metadata_epic_id_at(&epic_path, &format!("cas-epic-{i}"))
                    .unwrap();
            }
        });

        let pin_path = path.clone();
        let pin_thread = thread::spawn(move || {
            barrier.wait();
            for i in 0..100 {
                super::persist_session_metadata_pinned_epic_id_at(
                    &pin_path,
                    Some(&format!("cas-pin-{i}")),
                )
                .unwrap();
            }
        });

        epic_thread.join().unwrap();
        pin_thread.join().unwrap();

        let updated: crate::ui::factory::protocol::SessionMetadata =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(updated.epic_id, Some("cas-epic-99".to_string()));
        assert_eq!(updated.pinned_epic_id, Some("cas-pin-99".to_string()));
        assert!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")),
            "atomic write helper must not leave temp files behind"
        );
    }

    #[test]
    fn session_metadata_collision_never_removes_unowned_tempfile() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("session.json");
        let occupied = dir.path().join(".session.json.cas-session.forced.tmp");
        std::fs::write(&occupied, "peer-owned").unwrap();

        let error = super::atomic_write_session_metadata_via_temp_path(&target, "mine", &occupied)
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read_to_string(&occupied).unwrap(),
            "peer-owned",
            "a writer that loses create_new must not delete the winner's tempfile"
        );
    }

    #[test]
    fn session_metadata_atomic_write_supports_concurrent_writers() {
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("session.json");
        let candidates: Vec<String> = (0..16).map(|index| format!("session-{index:03}")).collect();
        let handles: Vec<_> = candidates
            .iter()
            .map(|candidate| {
                let target = target.clone();
                let candidate = candidate.clone();
                thread::spawn(move || {
                    for _ in 0..8 {
                        super::atomic_write_session_metadata(&target, &candidate).unwrap();
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }
        let contents = std::fs::read_to_string(&target).unwrap();
        assert!(
            candidates.contains(&contents),
            "unexpected contents: {contents:?}"
        );
        let stragglers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(".cas-session.")
            })
            .collect();
        assert!(stragglers.is_empty(), "leftover tempfiles: {stragglers:?}");
    }

    #[test]
    fn apply_session_metadata_focus_updates_current_id_and_epic_state() {
        let home = tempfile::tempdir().unwrap();
        let _guard = TestEnvGuard::with_vars(&[
            ("HOME", home.path().to_str().unwrap()),
            ("CAS_FACTORY_SESSION", "session-apply-pin"),
        ]);
        let metadata_path = crate::ui::factory::session::metadata_path("session-apply-pin");
        std::fs::create_dir_all(metadata_path.parent().unwrap()).unwrap();
        let mut metadata = crate::ui::factory::session::create_metadata(
            "session-apply-pin",
            12345,
            "supervisor",
            &[],
            Some("cas-session"),
            Some("/tmp/project"),
            None,
        );
        metadata.pinned_epic_id = Some("cas-pinned".to_string());
        std::fs::write(
            &metadata_path,
            serde_json::to_string_pretty(&metadata).unwrap(),
        )
        .unwrap();

        let mut app = super::FactoryApp::for_test();
        app.current_epic_id = Some("cas-session".to_string());
        app.current_epic_source = Some(super::EpicFocusSource::SessionDefault);
        app.epic_state = EpicState::Active {
            epic_id: "cas-session".to_string(),
            epic_title: "Session Epic".to_string(),
        };
        app.director_data = data_with_epics(vec![
            epic_summary("cas-session", "Session Epic", TaskStatus::Open),
            TaskSummary {
                branch: Some("epic/stacked-base".to_string()),
                ..epic_summary("cas-pinned", "Pinned Epic", TaskStatus::Open)
            },
        ]);

        app.apply_session_metadata_focus();

        assert_eq!(app.current_epic_id, Some("cas-pinned".to_string()));
        assert_eq!(
            app.current_epic_source,
            Some(super::EpicFocusSource::Pinned)
        );
        assert_eq!(
            app.epic_branch.as_deref(),
            Some("epic/stacked-base"),
            "focus_epic must update the spawn branch from task.branch"
        );
        match app.epic_state {
            EpicState::Active {
                ref epic_id,
                ref epic_title,
            } => {
                assert_eq!(epic_id, "cas-pinned");
                assert_eq!(epic_title, "Pinned Epic");
            }
            ref other => panic!("expected Active pinned epic, got {other:?}"),
        }
    }

    #[test]
    fn apply_session_metadata_focus_short_circuits_when_metadata_matches_current() {
        let home = tempfile::tempdir().unwrap();
        let _guard = TestEnvGuard::with_vars(&[
            ("HOME", home.path().to_str().unwrap()),
            ("CAS_FACTORY_SESSION", "session-apply-same"),
        ]);
        let metadata_path = crate::ui::factory::session::metadata_path("session-apply-same");
        std::fs::create_dir_all(metadata_path.parent().unwrap()).unwrap();
        let mut metadata = crate::ui::factory::session::create_metadata(
            "session-apply-same",
            12345,
            "supervisor",
            &[],
            Some("cas-session"),
            Some("/tmp/project"),
            None,
        );
        metadata.pinned_epic_id = Some("cas-pinned".to_string());
        std::fs::write(
            &metadata_path,
            serde_json::to_string_pretty(&metadata).unwrap(),
        )
        .unwrap();

        let mut app = super::FactoryApp::for_test();
        app.current_epic_id = Some("cas-pinned".to_string());
        app.current_epic_source = Some(super::EpicFocusSource::Pinned);
        app.epic_state = EpicState::Active {
            epic_id: "cas-pinned".to_string(),
            epic_title: "Preserved Existing State".to_string(),
        };
        app.director_data = data_with_epics(vec![epic_summary(
            "cas-other",
            "Other InProgress Epic",
            TaskStatus::InProgress,
        )]);

        app.apply_session_metadata_focus();

        assert_eq!(app.current_epic_id, Some("cas-pinned".to_string()));
        match app.epic_state {
            EpicState::Active {
                ref epic_id,
                ref epic_title,
            } => {
                assert_eq!(epic_id, "cas-pinned");
                assert_eq!(epic_title, "Preserved Existing State");
            }
            ref other => panic!("expected preserved Active state, got {other:?}"),
        }
    }

    /// cas-6945 follow-up (raised in review): the event detector's
    /// `EpicStarted` only fires on the epic's state *transition*
    /// (Open→Open-with-branch or →InProgress). In the real supervisor
    /// flow, the branch is auto-created at `task create` time — before any
    /// worker has started a subtask — so `EpicStarted` fires and is
    /// rejected for lack of a displayable subtask on that very first tick,
    /// then never refires (proven by
    /// `events::test_no_duplicate_epic_started_for_existing_open_with_branch`:
    /// an unchanged Open-with-branch epic does not re-emit `EpicStarted`
    /// even when the caller passes `current_epic_id: None`). Without a
    /// periodic retry, AC1 ("worker starts a subtask -> epic becomes
    /// visible, no supervisor action") would silently fail whenever epic
    /// creation predates the first subtask start — the normal sequence.
    /// This test drives `apply_session_metadata_focus` through exactly
    /// that two-tick sequence and asserts the later tick still adopts.
    #[test]
    fn apply_session_metadata_focus_retries_inference_once_assignee_lands_on_a_later_tick() {
        let _guard = TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_SESSION", None)]);
        let epic_id = "cas-late-assignee";
        let mut app = super::FactoryApp::for_test();

        // Tick 1: epic exists (Open, branch already set — as at creation
        // time), but no subtask has an assignee yet. Mirrors the detector
        // having already fired-and-been-rejected for this epic.
        app.director_data =
            data_with_epics(vec![epic_summary(epic_id, "Late Epic", TaskStatus::Open)]);
        app.apply_session_metadata_focus();
        assert_eq!(
            app.current_epic_id, None,
            "epic with no displayable subtask must stay unadopted"
        );

        // Tick 2 (later): a worker starts a subtask and gets an assignee
        // for free (cas-6945 lifecycle.rs fix). The epic's own Open/branch
        // state is unchanged since tick 1, so the event detector would NOT
        // refire `EpicStarted` for it — the periodic retry must still pick
        // it up.
        app.director_data = DirectorData {
            ready_tasks: vec![task_summary(
                "cas-late-assignee-1",
                "Subtask started by worker",
                Some(epic_id),
                Some("worker-one"),
            )],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![epic_summary(epic_id, "Late Epic", TaskStatus::Open)],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::from([("worker-one".to_string(), "worker-one".to_string())]),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };
        app.apply_session_metadata_focus();

        assert_eq!(app.current_epic_id.as_deref(), Some(epic_id));
        assert_eq!(
            app.current_epic_source,
            Some(super::EpicFocusSource::Inference)
        );
    }

    #[test]
    fn detector_driven_adoption_without_metadata_keeps_inference_source() {
        let home = tempfile::tempdir().unwrap();
        let _guard = TestEnvGuard::with_vars(&[
            ("HOME", home.path().to_str().unwrap()),
            ("CAS_FACTORY_SESSION", "session-without-metadata"),
        ]);
        let epic_id = "cas-inferred";
        let mut app = super::FactoryApp::for_test();
        app.director_data = DirectorData {
            ready_tasks: vec![task_summary(
                "cas-session-task",
                "Session-owned task",
                Some(epic_id),
                Some("session-agent"),
            )],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![epic_summary(epic_id, "Inferred Epic", TaskStatus::Open)],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::from([(
                "session-agent".to_string(),
                "worker-one".to_string(),
            )]),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        let changes = app.handle_epic_events(&[DirectorEvent::EpicStarted {
            epic_id: epic_id.to_string(),
            epic_title: "Inferred Epic".to_string(),
        }]);

        assert_eq!(changes.len(), 1);
        assert_eq!(app.current_epic_id.as_deref(), Some(epic_id));
        assert_eq!(
            app.current_epic_source,
            Some(super::EpicFocusSource::Inference),
            "detector adoption absent metadata must not be laundered into SessionDefault"
        );
    }

    #[test]
    fn refresh_then_handle_same_foreign_epic_started_event_does_not_adopt_focus() {
        let _guard = TestEnvGuard::with_optional_vars(&[("CAS_FACTORY_SESSION", None)]);
        let epic_id = "cas-foreign";
        let mut app = super::FactoryApp::for_test();
        app.director_data = DirectorData {
            ready_tasks: vec![task_summary(
                "cas-foreign-task",
                "Foreign task",
                Some(epic_id),
                Some("other-agent"),
            )],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![epic_summary(epic_id, "Foreign Epic", TaskStatus::Open)],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::from([(
                "session-agent".to_string(),
                "worker-one".to_string(),
            )]),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        let events = vec![DirectorEvent::EpicStarted {
            epic_id: epic_id.to_string(),
            epic_title: "Foreign Epic".to_string(),
        }];
        let changes = app.handle_epic_events(&events);

        assert!(changes.is_empty());
        assert_eq!(app.current_epic_id, None);
        assert_eq!(app.current_epic_source, None);
        assert!(matches!(app.epic_state, EpicState::Idle));
    }

    #[test]
    fn session_metadata_without_pinned_epic_id_still_parses() {
        let old_json = r#"{
            "name": "test-session",
            "created_at": "2026-07-06T12:00:00Z",
            "daemon_pid": 12345,
            "socket_path": "/tmp/factory.sock",
            "supervisor": {
                "name": "supervisor",
                "pid": null,
                "worktree_path": null
            },
            "workers": [],
            "epic_id": "cas-session"
        }"#;

        let metadata: crate::ui::factory::protocol::SessionMetadata =
            serde_json::from_str(old_json).unwrap();
        assert_eq!(metadata.epic_id, Some("cas-session".to_string()));
        assert_eq!(metadata.pinned_epic_id, None);
    }

    /// Regression test for cas-4181: factory TUI epic hijack.
    ///
    /// Two Open-with-branch epics exist:
    ///   - `epic-aaa` — lex-earlier, has both in-progress and ready subtasks.
    ///   - `epic-zzz` — lex-later, zero subtasks (the would-be hijacker).
    ///
    /// Before this fix, the runtime `EpicStarted` detector used a
    /// "greatest-lex-ID wins" tiebreak that disagreed with the init path's
    /// subtask-count heuristic. That caused `epic-zzz` to hijack the factory
    /// panel mid-session. After the fix both paths share
    /// `pick_best_open_branch_epic` and the active `epic-aaa` wins init;
    /// the strict-improvement gate then blocks `epic-zzz` from firing
    /// `EpicStarted` at all while `epic-aaa` is already the tracked epic.
    #[test]
    fn runtime_detector_does_not_hijack_active_epic_with_stray_open_branch_epic() {
        use cas_factory::{EpicState, TaskSummary};
        use cas_types::{Priority, TaskStatus, TaskType};

        use super::{DirectorData, DirectorEvent, DirectorEventDetector};

        let active_id = "epic-aaa";
        let hijacker_id = "epic-zzz";

        let data = DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "task-ready".to_string(),
                title: "Ready subtask of active epic".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some(active_id.to_string()),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            }],
            in_progress_tasks: vec![TaskSummary {
                id: "task-ip".to_string(),
                title: "In-progress subtask of active epic".to_string(),
                status: TaskStatus::InProgress,
                priority: Priority::MEDIUM,
                assignee: Some("worker-1".to_string()),
                task_type: TaskType::Task,
                epic: Some(active_id.to_string()),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            }],
            epic_tasks: vec![
                TaskSummary {
                    id: active_id.to_string(),
                    title: "Active epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/active".to_string()),
                    updated_at: None,
                    epic_verification_owner: None,
                },
                TaskSummary {
                    id: hijacker_id.to_string(),
                    title: "Stray zero-subtask epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/hijacker".to_string()),
                    updated_at: None,
                    epic_verification_owner: None,
                },
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        // Init path: detect_epic_state must prefer the active (lex-earlier,
        // has subtasks) epic over the stray lex-later one.
        let state = super::detect_epic_state(&data, None);
        match &state {
            EpicState::Active { epic_id, .. } => {
                assert_eq!(
                    epic_id, active_id,
                    "detect_epic_state must prefer the epic with active subtasks \
                     regardless of lex-ID order (cas-4181 init path)"
                );
            }
            other => panic!("Expected Active, got {other:?}"),
        }

        // Runtime path: event detector sees both epics as new Open-with-branch.
        // With current_epic_id pointing at the active epic, the strict-improvement
        // gate must suppress any EpicStarted for the stray hijacker.
        let mut detector =
            DirectorEventDetector::new(vec!["worker-1".to_string()], "supervisor".to_string());
        detector.initialize(&DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        });

        let events = detector.detect_changes(&data, state.epic_id());

        let hijack_started = events.iter().any(|e| {
            matches!(
                e,
                DirectorEvent::EpicStarted { epic_id, .. } if epic_id == hijacker_id
            )
        });
        assert!(
            !hijack_started,
            "EpicStarted must NOT fire for the zero-subtask hijacker epic \
             while an active epic is already tracked (cas-4181 runtime path)"
        );

        // And the active epic itself should also not re-fire — it's already tracked.
        let active_refired = events.iter().any(|e| {
            matches!(
                e,
                DirectorEvent::EpicStarted { epic_id, .. } if epic_id == active_id
            )
        });
        assert!(
            !active_refired,
            "EpicStarted must not refire for the already-tracked active epic"
        );
    }

    /// If the currently-tracked epic is no longer present in `epic_tasks`
    /// (closed, deleted, or cross-project filter drift) the strict-improvement
    /// gate must treat the slot as vacant so a legitimate new Open-with-branch
    /// epic can take over. Regression guard for the cas-4181 adversarial
    /// "ghost current_epic_id freezes TUI" concern.
    #[test]
    fn runtime_detector_recovers_when_tracked_epic_disappears() {
        use cas_factory::TaskSummary;
        use cas_types::{Priority, TaskStatus, TaskType};

        use super::{DirectorData, DirectorEvent, DirectorEventDetector};

        let new_id = "epic-new";

        let data = DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "task-ready".to_string(),
                title: "Ready subtask of new epic".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some(new_id.to_string()),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            }],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![TaskSummary {
                id: new_id.to_string(),
                title: "New epic".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Epic,
                epic: None,
                branch: Some("epic/new".to_string()),
                updated_at: None,
                epic_verification_owner: None,
            }],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        let mut detector =
            DirectorEventDetector::new(vec!["worker-1".to_string()], "supervisor".to_string());
        detector.initialize(&DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        });

        // current_epic_id points at a ghost epic not in data.epic_tasks.
        let events = detector.detect_changes(&data, Some("epic-ghost"));

        let started_for_new = events.iter().any(|e| {
            matches!(
                e,
                DirectorEvent::EpicStarted { epic_id, .. } if epic_id == new_id
            )
        });
        assert!(
            started_for_new,
            "EpicStarted must fire for the legitimate new epic when the \
             tracked current_epic_id refers to a ghost not in epic_tasks"
        );
    }

    /// Sibling sanity test: when no epic is currently tracked (init-time
    /// detect_changes), the runtime detector must pick the *same* best
    /// Open-with-branch epic as `detect_epic_state` — not the lex-greatest.
    #[test]
    fn runtime_detector_picks_same_epic_as_init_when_no_current_epic() {
        use cas_factory::TaskSummary;
        use cas_types::{Priority, TaskStatus, TaskType};

        use super::{DirectorData, DirectorEvent, DirectorEventDetector};

        let active_id = "epic-aaa";
        let hijacker_id = "epic-zzz";

        let data = DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "task-ready".to_string(),
                title: "Ready subtask".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: None,
                task_type: TaskType::Task,
                epic: Some(active_id.to_string()),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            }],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![
                TaskSummary {
                    id: active_id.to_string(),
                    title: "Active epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/active".to_string()),
                    updated_at: None,
                    epic_verification_owner: None,
                },
                TaskSummary {
                    id: hijacker_id.to_string(),
                    title: "Stray epic".to_string(),
                    status: TaskStatus::Open,
                    priority: Priority::MEDIUM,
                    assignee: None,
                    task_type: TaskType::Epic,
                    epic: None,
                    branch: Some("epic/hijacker".to_string()),
                    updated_at: None,
                    epic_verification_owner: None,
                },
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        let mut detector =
            DirectorEventDetector::new(vec!["worker-1".to_string()], "supervisor".to_string());
        detector.initialize(&DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        });

        let events = detector.detect_changes(&data, None);

        let started_for: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                DirectorEvent::EpicStarted { epic_id, .. } => Some(epic_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            started_for,
            vec![active_id],
            "with no current epic, EpicStarted must fire for the subtask-winning \
             epic, not the lex-greatest one (cas-4181)"
        );
    }
}

/// Spawn-path isolation tests for EPIC cas-073f / task cas-5232.
///
/// These tests follow the test-first mandate from the task execution_note.
/// Two tests were written first (before the implementation):
///   - `reuse_branch_rejects_stale_non_worktree_directory`
///   - `post_spawn_assertion_fails_for_main_checkout`
/// Both failed until the corresponding implementation was added.
#[cfg(test)]
mod spawn_isolation_tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    /// Initialise a bare git repo with one commit on `main`.
    fn init_repo(dir: &std::path::Path) {
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir)
            .output()
            .expect("git init");
        Command::new("git")
            .args(["config", "user.email", "test@cas.test"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Cassy Test"])
            .current_dir(dir)
            .output()
            .unwrap();
        std::fs::write(dir.join("README.md"), "# test").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(dir)
            .output()
            .expect("git commit");
    }

    // -----------------------------------------------------------------
    // cas-30c6: provisioning must bind the worker's OWN worktree, or fail
    // closed before any harness is started.
    // -----------------------------------------------------------------

    /// Spawn request 1031 class: the worker's harness stayed bound to the
    /// dirty SHARED checkout even though a factory worktree existed.
    ///
    /// The reuse validation compares branches by running git *inside* the
    /// worktree path. A stale plain directory is not a worktree, so git
    /// climbs out of it into the enclosing checkout — and when that shared
    /// checkout is itself parked on `factory/<name>` (the GH #120 /
    /// cas-5bef shape), the branch comparison *matches* and provisioning
    /// hands back a cwd whose every git operation resolves to shared main.
    ///
    /// Only the worktree toplevel can distinguish the two, so provisioning
    /// must prove the path is its own worktree root and fail closed
    /// otherwise.
    #[test]
    fn stale_directory_resolving_to_shared_checkout_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        // The shared checkout was left parked on the worker's own branch.
        Command::new("git")
            .args(["switch", "-c", "factory/reuse-worker"])
            .current_dir(&repo)
            .output()
            .expect("park shared HEAD on the worker branch");

        // A stale plain directory sits where the worktree belongs.
        let cas_dir = repo.join(".cas");
        let worktree_path = cas_dir.join("worktrees").join("reuse-worker");
        std::fs::create_dir_all(&worktree_path).unwrap();

        let prep = WorkerSpawnPrep {
            worker_name: "reuse-worker".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worktree_path.clone(),
                branch_name: "factory/reuse-worker".to_string(),
                parent_branch: "main".to_string(),
                base_ref: None,
                repo_root: repo.clone(),
                cas_dir,
            }),
            warnings: Vec::new(),
            base_provenance: None,
        };

        let error = match prep.run() {
            Ok(result) => panic!(
                "a directory that is not the worker's own worktree must fail the spawn, \
                 but provisioning returned cwd {:?}",
                result.cwd
            ),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains("reuse-worker"),
            "the refusal must name the worker whose spawn failed: {error}"
        );
        assert!(
            error.contains(&repo.display().to_string()),
            "the refusal must name the shared checkout the path really resolves to: {error}"
        );
    }

    /// Respawn 1034 class: the path exists as a real worktree but belongs to
    /// a SIBLING worker. Provisioning must refuse rather than hand a worker
    /// another worker's branch.
    #[test]
    fn sibling_owned_worktree_is_rejected_for_a_different_worker() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        let cas_dir = repo.join(".cas");
        let sibling_path = cas_dir.join("worktrees").join("bright-dolphin-92");
        std::fs::create_dir_all(sibling_path.parent().unwrap()).unwrap();
        Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                "factory/bright-dolphin-92",
                sibling_path.to_str().unwrap(),
                "main",
            ])
            .current_dir(&repo)
            .output()
            .expect("git worktree add");

        // fair-pelican-51 is handed the sibling's directory.
        let prep = WorkerSpawnPrep {
            worker_name: "fair-pelican-51".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: sibling_path,
                branch_name: "factory/fair-pelican-51".to_string(),
                parent_branch: "main".to_string(),
                base_ref: None,
                repo_root: repo,
                cas_dir,
            }),
            warnings: Vec::new(),
            base_provenance: None,
        };

        let error = match prep.run() {
            Ok(result) => panic!(
                "a sibling's worktree must never be bound to another worker, \
                 but provisioning returned cwd {:?}",
                result.cwd
            ),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains("fair-pelican-51") && error.contains("bright-dolphin-92"),
            "the refusal must name both the spawning worker and the real owner: {error}"
        );
    }

    /// The ordinary reuse path is unchanged: a real worktree on the worker's
    /// own branch still provisions.
    #[test]
    fn own_worktree_is_still_reused() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        let cas_dir = repo.join(".cas");
        let worktree_path = cas_dir.join("worktrees").join("own-worker");
        std::fs::create_dir_all(worktree_path.parent().unwrap()).unwrap();
        Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                "factory/own-worker",
                worktree_path.to_str().unwrap(),
                "main",
            ])
            .current_dir(&repo)
            .output()
            .expect("git worktree add");

        let prep = WorkerSpawnPrep {
            worker_name: "own-worker".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worktree_path.clone(),
                branch_name: "factory/own-worker".to_string(),
                parent_branch: "main".to_string(),
                base_ref: None,
                repo_root: repo,
                cas_dir,
            }),
            warnings: Vec::new(),
            base_provenance: None,
        };

        let result = prep.run().expect("own worktree must still be reusable");
        assert_eq!(result.cwd, worktree_path);
        assert!(!result.worktree_created, "the existing worktree is reused");
    }

    // -----------------------------------------------------------------
    // verify_isolated_worker_branch tests
    // -----------------------------------------------------------------

    /// Happy path: cwd is a proper worktree on the expected branch → Ok.
    #[test]
    fn post_spawn_assertion_passes_for_correct_worktree() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        let wt_path = repo.join(".cas").join("worktrees").join("test-worker");
        std::fs::create_dir_all(wt_path.parent().unwrap()).unwrap();
        Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                "factory/test-worker",
                wt_path.to_str().unwrap(),
                "main",
            ])
            .current_dir(&repo)
            .output()
            .expect("git worktree add");

        assert!(
            verify_isolated_worker_branch("test-worker", &wt_path, "factory/test-worker").is_ok(),
            "assertion must pass when cwd is the worktree on the correct branch"
        );
    }

    /// Bug-scenario: the worker's cwd is the main checkout (on `main`), not its
    /// worktree.  The assertion must detect the branch mismatch and return Err.
    ///
    /// This test was written BEFORE the implementation (test-first, cas-5232).
    #[test]
    fn post_spawn_assertion_fails_for_main_checkout() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        // Simulate the bug: the worker process started in the MAIN checkout,
        // not in its factory/worker-X worktree.
        let err = verify_isolated_worker_branch("worker-x", &repo, "factory/worker-x").unwrap_err();
        assert!(
            err.to_string().contains("ISOLATION BUG"),
            "error must flag as an isolation bug; got: {err}"
        );
        assert!(
            err.to_string().contains("main"),
            "error must mention the actual branch ('main'); got: {err}"
        );
    }

    // -----------------------------------------------------------------
    // WorkerSpawnPrep::run() tests
    // -----------------------------------------------------------------

    #[test]
    fn new_worker_worktree_with_package_json_gets_branch_local_node_setup_guidance() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);
        std::fs::write(repo.join("package.json"), "{\"name\":\"fixture\"}\n").unwrap();
        std::fs::write(repo.join("package-lock.json"), "{\"lockfileVersion\":3}\n").unwrap();
        Command::new("git")
            .args(["add", "package.json", "package-lock.json"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "node fixture"])
            .current_dir(&repo)
            .output()
            .unwrap();

        let cas_dir = repo.join(".cas");
        let worktree_path = cas_dir.join("worktrees").join("node-worker");
        let prep = WorkerSpawnPrep {
            worker_name: "node-worker".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worktree_path.clone(),
                branch_name: "factory/node-worker".to_string(),
                parent_branch: "main".to_string(),
                base_ref: None,
                repo_root: repo,
                cas_dir,
            }),
            warnings: Vec::new(),
            base_provenance: None,
        };

        let result = prep.run().expect("create Node worker worktree");
        assert!(result.cwd.join("package.json").is_file());
        assert!(
            !result.cwd.join("node_modules").exists(),
            "spawn must not link dependencies whose lockfile/native artifacts may belong to another branch"
        );
        let instruction = crate::worktree::node_modules_setup_instruction(&result.cwd)
            .expect("a Node worktree without dependencies must have setup guidance");
        assert!(instruction.contains("`npm ci`"), "{instruction}");
    }

    #[cfg(unix)]
    #[test]
    fn new_worker_worktree_provisions_gitignored_zig_toolchain() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        let source_zig = repo.join(".context").join("zig");
        std::fs::create_dir_all(&source_zig).unwrap();
        std::fs::write(source_zig.join("zig"), "pinned zig binary").unwrap();

        let cas_dir = repo.join(".cas");
        let worktree_path = cas_dir.join("worktrees").join("zig-worker");
        let prep = WorkerSpawnPrep {
            worker_name: "zig-worker".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worktree_path.clone(),
                branch_name: "factory/zig-worker".to_string(),
                parent_branch: "main".to_string(),
                base_ref: None,
                repo_root: repo,
                cas_dir,
            }),
            warnings: Vec::new(),
            base_provenance: None,
        };

        let result = prep.run().expect("create worker worktree with Zig support");
        let worker_zig = result.cwd.join(".context").join("zig");
        assert!(
            std::fs::symlink_metadata(&worker_zig)
                .unwrap()
                .file_type()
                .is_symlink(),
            "new workers must link the gitignored Zig toolchain before their first Cargo build"
        );
        assert_eq!(
            std::fs::read_to_string(worker_zig.join("zig")).unwrap(),
            "pinned zig binary"
        );
    }

    #[cfg(unix)]
    #[test]
    fn new_worker_target_is_hardlinked_from_published_baseline() {
        use std::os::unix::fs::MetadataExt;

        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        let cas_dir = repo.join(".cas");
        let snapshot = cas_dir
            .join("build-cache")
            .join("snapshots")
            .join("target-main-test");
        let cached_artifact = snapshot.join("debug").join("deps").join("libwarm.rlib");
        std::fs::create_dir_all(cached_artifact.parent().unwrap()).unwrap();
        std::fs::write(&cached_artifact, b"warm dependency artifact").unwrap();
        let cached_dep_info = snapshot.join("debug").join("deps").join("warm.d");
        std::fs::write(
            &cached_dep_info,
            format!("{}: registry-source.rs\n", cached_artifact.display()),
        )
        .unwrap();
        std::fs::write(
            cas_dir.join("build-cache").join("current"),
            "target-main-test\n",
        )
        .unwrap();

        let worktree_path = cas_dir.join("worktrees").join("cache-worker");
        let prep = WorkerSpawnPrep {
            worker_name: "cache-worker".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: worktree_path.clone(),
                branch_name: "factory/cache-worker".to_string(),
                parent_branch: "main".to_string(),
                base_ref: None,
                repo_root: repo,
                cas_dir,
            }),
            warnings: Vec::new(),
            base_provenance: None,
        };

        let result = prep.run().expect("create and seed worker worktree");
        let seeded_artifact = result
            .cwd
            .join("target")
            .join("debug")
            .join("deps")
            .join("libwarm.rlib");
        assert_eq!(
            std::fs::read(&seeded_artifact).unwrap(),
            b"warm dependency artifact"
        );

        let source_metadata = std::fs::metadata(cached_artifact).unwrap();
        let seeded_metadata = std::fs::metadata(seeded_artifact).unwrap();
        assert_eq!(source_metadata.dev(), seeded_metadata.dev());
        assert_eq!(
            source_metadata.ino(),
            seeded_metadata.ino(),
            "seeded artifact must share its inode with the immutable baseline"
        );

        let seeded_dep_info = result.cwd.join("target").join("debug/deps/warm.d");
        let seeded_dep_text = std::fs::read_to_string(&seeded_dep_info).unwrap();
        assert!(seeded_dep_text.contains(&result.cwd.join("target").display().to_string()));
        assert!(!seeded_dep_text.contains(&snapshot.display().to_string()));
        assert_ne!(
            std::fs::metadata(cached_dep_info).unwrap().ino(),
            std::fs::metadata(seeded_dep_info).unwrap().ino(),
            "rebased dep-info must not mutate the immutable baseline hardlink"
        );
    }

    #[cfg(unix)]
    #[test]
    fn target_seed_skips_cargo_lock_so_workers_hold_private_locks() {
        use std::fs::OpenOptions;
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;

        let tmp = TempDir::new().unwrap();
        let snapshot = tmp.path().join("snapshot");
        let source_lock = snapshot.join("debug").join(".cargo-lock");
        let source_artifact = snapshot.join("debug").join("deps").join("libwarm.rlib");
        std::fs::create_dir_all(source_lock.parent().unwrap()).unwrap();
        std::fs::create_dir_all(source_artifact.parent().unwrap()).unwrap();
        std::fs::write(&source_lock, b"Cargo lock").unwrap();
        std::fs::write(&source_artifact, b"warm artifact").unwrap();

        let worker_a = tmp.path().join("worker-a-target");
        let worker_b = tmp.path().join("worker-b-target");
        for worker_target in [&worker_a, &worker_b] {
            let mut stats = TargetSeedStats::default();
            hardlink_seed_tree(&snapshot, worker_target, worker_target, &mut stats).unwrap();
            assert_eq!(stats.files, 1, "only the reusable artifact is seeded");
            assert!(
                !worker_target.join("debug/.cargo-lock").exists(),
                "a seeded target must not inherit the snapshot Cargo lock"
            );
        }

        let worker_a_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(worker_a.join("debug/.cargo-lock"))
            .unwrap();
        let worker_b_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(worker_b.join("debug/.cargo-lock"))
            .unwrap();
        assert_ne!(
            worker_a_lock.metadata().unwrap().ino(),
            worker_b_lock.metadata().unwrap().ino(),
            "Cargo must recreate a private lock inode for each seeded target"
        );
        // Non-blocking exclusive locks succeeding on both files prove the
        // seeded targets cannot serialize each other through a shared inode.
        assert_eq!(
            unsafe { libc::flock(worker_a_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        assert_eq!(
            unsafe { libc::flock(worker_b_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        unsafe {
            libc::flock(worker_a_lock.as_raw_fd(), libc::LOCK_UN);
            libc::flock(worker_b_lock.as_raw_fd(), libc::LOCK_UN);
        }
    }

    /// WorkerSpawnPrep::run() must return the worktree path as cwd for N=4
    /// isolated workers — never the process's current directory.
    ///
    /// This exercises the multi-worker stress-spawn path (AC3 of cas-5232).
    #[test]
    fn spawn_prep_cwd_is_worktree_for_four_workers() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        // Use the fixture repo as the simulated supervisor checkout. Reading
        // the process cwd makes this test depend on unrelated tests that may
        // temporarily chdir into a tempdir.
        let process_cwd = repo.clone();
        let cas_dir = repo.join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();

        for i in 0..4usize {
            let worker_name = format!("w{i}");
            let expected_wt_path = repo.join(".cas").join("worktrees").join(&worker_name);
            let prep = WorkerSpawnPrep {
                worker_name: worker_name.clone(),
                worktree_info: Some(WorktreePrep {
                    worktree_path: expected_wt_path.clone(),
                    branch_name: format!("factory/{worker_name}"),
                    parent_branch: "main".to_string(),
                    base_ref: None,
                    repo_root: repo.clone(),
                    cas_dir: cas_dir.clone(),
                }),
                warnings: Vec::new(),
                base_provenance: None,
            };

            let result = prep
                .run()
                .unwrap_or_else(|e| panic!("worker {i} spawn prep failed: {e}"));

            assert_eq!(
                result.cwd, expected_wt_path,
                "worker {i}: cwd must be the worktree path"
            );
            assert_ne!(
                result.cwd, process_cwd,
                "worker {i}: cwd MUST NOT be the supervisor process cwd"
            );
            assert!(
                result.worktree.is_some(),
                "worker {i}: worktree field must be populated for isolated spawn"
            );
        }
    }

    /// STEP 2 — hard-error regression test (written first, cas-5232):
    /// The REUSE BRANCH must reject a stale directory that exists on disk but is
    /// not a registered git worktree (the directory has no `.git` file, so git
    /// traverses up to the main checkout and reports `main` as current branch).
    ///
    /// Before the hard-error check was added, `run()` would return Ok with the
    /// stale directory as cwd, causing subsequent git operations to commit on
    /// the main branch.
    ///
    /// After the check, `run()` returns Err with a clear message.
    #[test]
    fn reuse_branch_rejects_stale_non_worktree_directory() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_repo(&repo);

        let cas_dir = repo.join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();
        let stale_path = repo.join(".cas").join("worktrees").join("stale");

        // Create the directory WITHOUT running `git worktree add`.
        // exists() == true, but it is NOT a git worktree.
        std::fs::create_dir_all(&stale_path).unwrap();

        let prep = WorkerSpawnPrep {
            worker_name: "stale".to_string(),
            worktree_info: Some(WorktreePrep {
                worktree_path: stale_path.clone(),
                branch_name: "factory/stale".to_string(),
                parent_branch: "main".to_string(),
                base_ref: None,
                repo_root: repo.clone(),
                cas_dir,
            }),
            warnings: Vec::new(),
            base_provenance: None,
        };

        match prep.run() {
            Ok(_) => panic!("reuse branch must reject a stale non-worktree directory; got Ok"),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("stale")
                        || msg.contains("branch")
                        || msg.contains("worktree")
                        || msg.contains("Stale"),
                    "error must describe the rejection; got: {msg}"
                );
            }
        }
    }
}
