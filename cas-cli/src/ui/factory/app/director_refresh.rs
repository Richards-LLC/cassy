//! The read half of the director refresh, separable from the loop (cas-ee9ab).
//!
//! [`FactoryApp::refresh_data`] used to read the store and apply the result in
//! one call on the daemon loop. The read (every task, agent and recent event,
//! plus git changes when due) is the part that costs 100 ms or more on a large
//! store. A [`DirectorRefreshRequest`] captures what the read needs, runs on any
//! thread, and returns a [`DirectorRefreshLoad`] the app applies on the loop.
//! The delivery stage's own fresh read is split the same way
//! ([`DeliveryRequest`] and [`DeliveryInputs`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use cas_factory::SourceChangesInfo;

use super::{
    CasDbFingerprint, FactoryApp, merge_director_data_preserving_git, unfiltered_snapshot_from,
};
use crate::ui::factory::director::{DirectorData, DirectorEvent, DirectorStores};

/// Everything a director refresh read needs, captured on the loop.
#[derive(Debug, Clone)]
pub(crate) struct DirectorRefreshRequest {
    cas_dir: PathBuf,
    worktree_root: Option<PathBuf>,
    git_due: bool,
    previous_fingerprint: Option<CasDbFingerprint>,
    agent_id_to_name: HashMap<String, String>,
    project_id: Option<String>,
    /// The app's refresh clock when the request was made. A result whose
    /// clock no longer matches is older than a refresh applied since, and is
    /// discarded rather than rolling panel data back.
    requested_after: Instant,
}

/// The result of a director refresh read.
pub(crate) struct DirectorRefreshLoad {
    fingerprint: CasDbFingerprint,
    git_due: bool,
    director: DirectorLoad,
    requested_after: Instant,
}

enum DirectorLoad {
    /// The database is unchanged and git is not due: nothing was read.
    Unchanged,
    Loaded(anyhow::Result<DirectorData>),
    GitOnly(anyhow::Result<Vec<SourceChangesInfo>>),
}

impl DirectorRefreshRequest {
    /// Read the store (and git when due). Runs on any thread.
    pub(crate) fn read(&self, stores: Option<&DirectorStores>) -> DirectorRefreshLoad {
        let fingerprint = CasDbFingerprint::from_cas_dir(&self.cas_dir);
        let db_changed = self.previous_fingerprint != Some(fingerprint);
        let director = if db_changed {
            DirectorLoad::Loaded(DirectorData::load_with_stores_for_project(
                &self.cas_dir,
                self.worktree_root.as_deref(),
                self.git_due,
                stores,
                self.project_id.as_deref(),
            ))
        } else if self.git_due {
            DirectorLoad::GitOnly(DirectorData::load_git_changes_with_stores(
                &self.cas_dir,
                self.worktree_root.as_deref(),
                &self.agent_id_to_name,
                stores,
            ))
        } else {
            DirectorLoad::Unchanged
        };
        DirectorRefreshLoad {
            fingerprint,
            git_due: self.git_due,
            director,
            requested_after: self.requested_after,
        }
    }
}

/// What the delivery stage's fresh read needs.
#[derive(Debug, Clone)]
pub(crate) struct DeliveryRequest {
    cas_dir: PathBuf,
    worktree_root: Option<PathBuf>,
    project_id: Option<String>,
}

/// The delivery stage's fresh read: the unfiltered snapshot events are
/// revalidated against, and the `Blocks` dependencies that gate dispatch.
pub(crate) struct DeliveryInputs {
    pub(super) data: anyhow::Result<DirectorData>,
    pub(super) blocks_deps: Vec<cas_types::Dependency>,
}

impl DeliveryRequest {
    /// Read the delivery snapshot. Runs on any thread.
    pub(crate) fn read(&self, stores: Option<&DirectorStores>) -> DeliveryInputs {
        let data = DirectorData::load_with_stores_for_project(
            &self.cas_dir,
            self.worktree_root.as_deref(),
            false,
            stores,
            self.project_id.as_deref(),
        );
        let blocks_deps = stores
            .and_then(|stores| {
                cas_store::TaskStore::list_dependencies(
                    &stores.task_store,
                    Some(cas_types::DependencyType::Blocks),
                )
                .ok()
            })
            .unwrap_or_default();
        DeliveryInputs { data, blocks_deps }
    }
}

/// Run one step of applying a refresh on the loop, and name it when it is
/// slow: a pass budget is 100 ms, and the step that spends it must be
/// findable from the log alone (cas-ee9ab).
pub(crate) fn timed<T>(step: &'static str, run: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let value = run();
    let elapsed = started.elapsed();
    if elapsed >= std::time::Duration::from_millis(50) {
        tracing::warn!(
            target: "cas::refresh_profile",
            step,
            elapsed_ms = elapsed.as_millis() as u64,
            "slow director refresh step on the daemon loop"
        );
    }
    value
}

impl FactoryApp {
    /// The project the director's loads are scoped to (cas-0c98).
    pub(crate) fn project_scope(&self) -> Option<String> {
        self.director_stores
            .as_ref()
            .and_then(|stores| stores.project_id.clone())
    }

    /// Capture what the next director refresh read needs.
    pub(crate) fn director_refresh_request(&self) -> DirectorRefreshRequest {
        DirectorRefreshRequest {
            cas_dir: self.cas_dir.clone(),
            worktree_root: self.worktree_manager.as_ref().map(|m| m.worktree_root()),
            git_due: !self.director_data.git_loaded
                || self.last_git_refresh.elapsed() >= self.git_refresh_interval,
            previous_fingerprint: self.last_db_fingerprint,
            agent_id_to_name: self.director_data.agent_id_to_name.clone(),
            project_id: self.project_scope(),
            requested_after: self.last_refresh,
        }
    }

    /// Capture what the delivery stage's fresh read needs.
    pub(crate) fn delivery_request(&self) -> DeliveryRequest {
        DeliveryRequest {
            cas_dir: self.cas_dir.clone(),
            worktree_root: self.worktree_manager.as_ref().map(|m| m.worktree_root()),
            project_id: self.project_scope(),
        }
    }

    /// Apply a finished director read: merge it into panel data and detect
    /// state changes. In-memory work plus the session metadata files.
    pub(crate) fn apply_director_refresh(
        &mut self,
        load: DirectorRefreshLoad,
    ) -> anyhow::Result<Vec<DirectorEvent>> {
        if load.requested_after != self.last_refresh {
            // A newer refresh landed while this read ran (a synchronous
            // refresh on an operator action, say). Applying the older read
            // would roll panel data and change detection back.
            tracing::debug!("discarding a director refresh read older than the applied one");
            return Ok(Vec::new());
        }
        let git_due = load.git_due;
        let db_changed = match load.director {
            DirectorLoad::Loaded(loaded) => {
                self.director_data =
                    merge_director_data_preserving_git(&self.director_data, loaded?, git_due);
                // cas-dbbe: snapshot the fresh, still-unfiltered load BEFORE
                // `filter_director_agents_to_current_session()` mutates
                // `self.director_data` in place. Change detection and the
                // TaskCompleted safety net read this canonical copy.
                self.unfiltered_director_data = unfiltered_snapshot_from(&self.director_data);
                // cas-ae6d: the only place `director_data` is genuinely re-read.
                self.director_data_loaded_at = chrono::Utc::now();
                if git_due {
                    self.last_git_refresh = Instant::now();
                }
                true
            }
            DirectorLoad::GitOnly(changes) => {
                self.director_data.changes = changes?;
                self.director_data.git_loaded = true;
                self.last_git_refresh = Instant::now();
                false
            }
            DirectorLoad::Unchanged => {
                timed("branch visibility", || self.refresh_branch_visibility_cache());
                self.last_refresh = Instant::now();
                // Worker holds live in session metadata rather than cas.db.
                // They must be reconciled even on the unchanged-DB path;
                // otherwise a just-written hold leaks one more WorkerIdle
                // event before unrelated database activity occurs.
                timed("worker holds", || self.apply_session_metadata_worker_holds());
                return Ok(timed("supervisor stall", || self.detect_supervisor_stall())
                    .into_iter()
                    .collect());
            }
        };

        timed("branch visibility", || self.refresh_branch_visibility_cache());
        self.last_db_fingerprint = Some(load.fingerprint);
        self.last_refresh = Instant::now();

        // Sync session_id → pane_name mappings from agent store
        timed("session mappings", || self.sync_session_mappings());
        timed("epic focus", || self.apply_session_metadata_focus());
        timed("worker holds", || self.apply_session_metadata_worker_holds());

        // cas-e98e AC3: drop phantom worker panes when registry says the
        // worker is no longer supervision-live.
        if db_changed {
            timed("phantom panes", || self.reconcile_phantom_worker_panes());
        }

        // Detect state changes against the UNFILTERED snapshot (cas-dbbe), and
        // gate `EpicStarted` on the tracked epic (cas-4181).
        let mut events = timed("change detection", || {
            self.event_detector
                .detect_changes(&self.unfiltered_director_data, self.epic_state.epic_id())
        });
        events.extend(timed("supervisor stall", || self.detect_supervisor_stall()));

        // Now filter to current session (agents + tasks scoped to active epic)
        if db_changed {
            self.filter_director_agents_to_current_session();
        }

        Ok(events)
    }
}
