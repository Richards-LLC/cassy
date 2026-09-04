use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::cloud::syncer::{
    CloudSyncer, ConflictAction, ConflictResolution, PullResponse, SyncResult,
    TaskStatusTransition, TeamPullResponse, UpsertResult,
};
use crate::cloud::{
    EntityType, SyncOperation, SyncQueue, get_project_canonical_id,
    project_ids_match as canonical_project_ids_match,
};
use crate::error::CasError;
use crate::store::{
    CommitLinkStore, EventStore, FileChangeStore, PromptStore, RuleStore, SkillStore, SpecStore,
    Store, TaskStore,
};
use crate::types::{
    CommitLink, Dependency, DependencyType, Entry, Event, FileChange, Prompt, Rule, Session, Skill,
    Spec, Task, TaskStatus,
};

/// Path of the cloud sync pull endpoint.
///
/// Single source of truth: this is the only place in shipped source where the
/// pull path literal is written. Every production caller must build its URL
/// through [`build_scoped_pull_url`], which is what keeps the pull scoped to
/// the current project (cas-2eb3 / cas-ed15).
pub(crate) const PULL_PATH: &str = "/api/sync/pull";

/// One deduplicated project-scoping warning observed during a pull or doctor
/// contamination scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncWarningSummary {
    pub entity_kind: String,
    pub project: String,
    pub count: usize,
}

thread_local! {
    static WARNING_SINK: RefCell<Option<BTreeMap<(String, String), usize>>> = const { RefCell::new(None) };
}

/// Collect project-scope warnings for a bounded operation. The normal cloud
/// pull path has no collector and therefore logs the original warning through
/// tracing without writing directly to stderr.
pub(crate) fn collect_sync_warnings<T>(
    operation: impl FnOnce() -> T,
) -> (T, Vec<SyncWarningSummary>) {
    WARNING_SINK.with(|sink| {
        let previous = sink.replace(Some(BTreeMap::new()));
        let result = operation();
        let collected = sink.replace(previous).unwrap_or_default();
        let warnings = collected
            .into_iter()
            .map(|((entity_kind, project), count)| SyncWarningSummary {
                entity_kind,
                project,
                count,
            })
            .collect();
        (result, warnings)
    })
}

fn record_project_warning(entity_kind: &str, project: &str, message: &str) {
    let collected = WARNING_SINK.with(|sink| {
        sink.borrow_mut()
            .as_mut()
            .map(|warnings| {
                *warnings
                    .entry((entity_kind.to_string(), project.to_string()))
                    .or_default() += 1;
            })
            .is_some()
    });
    if collected {
        return;
    }

    tracing::debug!("[Cassy sync] WARNING: {message}");
}

/// Deserialize one raw pull entity while retaining its wire identifier in any
/// error. Pull payloads are raw JSON specifically so one malformed row does
/// not abort the rest of a sync; without this boundary the resulting warning
/// was un-actionable because it named neither the row nor its entity type.
fn deserialize_pulled_entity<T: DeserializeOwned>(
    raw: serde_json::Value,
    entity_type: &str,
) -> Result<T, String> {
    let id = raw
        .get("id")
        .or_else(|| raw.get("entity_id"))
        .or_else(|| raw.get("commit_hash"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<missing-id>")
        .to_owned();
    serde_json::from_value(raw)
        .map_err(|error| format!("{entity_type} deserialize error (id={id}): {error}"))
}

#[derive(Debug, Deserialize)]
struct TaskDependencyRecord {
    from_id: String,
    to_id: String,
    dep_type: DependencyType,
    /// Older cloud tombstones omit this field. It is unused for deletes, but
    /// defaulting it lets those records still protect the local edge.
    #[serde(default = "default_dependency_created_at")]
    created_at: DateTime<Utc>,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    deleted: bool,
    /// Delete time carried by a cloud tombstone. The cloud updates the live row
    /// in place, so a tombstone also overwrites `updated_at` with the same
    /// instant; either field dates the deletion.
    #[serde(default)]
    deleted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
}

fn default_dependency_created_at() -> DateTime<Utc> {
    Utc::now()
}

impl TaskDependencyRecord {
    fn is_delete(&self) -> bool {
        self.deleted
            || self
                .operation
                .as_deref()
                .is_some_and(|operation| operation.eq_ignore_ascii_case("delete"))
    }

    /// When this delete happened, for ordering against a local edge.
    ///
    /// A tombstone that dates itself only by its absent fields cannot be
    /// ordered, so it is treated as "just now" — the conservative reading that
    /// suppresses an equally undated local edge rather than resurrecting it.
    fn deleted_at(&self) -> DateTime<Utc> {
        self.deleted_at.or(self.updated_at).unwrap_or_else(Utc::now)
    }

    fn dependency(self) -> Dependency {
        Dependency {
            from_id: self.from_id,
            to_id: self.to_id,
            dep_type: self.dep_type,
            created_at: self.created_at,
            created_by: None,
        }
    }
}

/// The row's own statement of which project it belongs to.
///
/// `origin_project` is written by the client that created the row and travels
/// with it. It is the only field on a pulled row that survives replication
/// intact — see [`row_attribution`].
fn row_origin_project(raw: &serde_json::Value) -> Option<&str> {
    raw.get("origin_project")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
}

/// Which project a pulled row actually belongs to (GH #701).
///
/// # Why `origin_project` outranks the scope stamp
///
/// The cloud stamps every row it returns with `project_id = <the scope you
/// asked for>`, so that field says nothing about ownership — it is an echo of
/// the request. Measured on this account 2026-09-03:
/// `GET /api/sync/pull?project_id=richards-llc-accounting` returns **1** task
/// and **3,002** task-dependency rows, and all 3,002 are stamped
/// `project_id: "richards-llc-accounting"` while carrying
/// `origin_project: "cas-src"`. Reading the stamp first — which is what this
/// client did — admits every one of them, which is the inflow behind the
/// foreign-row growth in GH #701 (1,772 → 1,862 rows across 9 → 12 projects,
/// three of them ephemeral probes).
///
/// So attribution reads `origin_project` **first**, and falls back to the scope
/// stamp only when the row does not carry one. That fallback is load-bearing,
/// not laziness: rows written before `origin_project` existed have no origin,
/// and rejecting those would silently drop real history instead of fixing the
/// leak.
///
/// Comparison runs through `project_ids_match`, so a legacy spelling folded by
/// the cloud's alias record (GH #669) is still recognized as this project's own.
fn row_attribution<'a>(raw: &'a serde_json::Value) -> Option<(&'a str, &'static str)> {
    if let Some(origin) = row_origin_project(raw) {
        return Some((origin, "origin_project"));
    }
    raw.get("project_canonical_id")
        .or_else(|| raw.get("project_id"))
        .and_then(serde_json::Value::as_str)
        .map(|scope| (scope, "project_id"))
}

pub(crate) fn task_dependency_matches_project(
    raw: &serde_json::Value,
    current_project_id: &str,
) -> bool {
    let edge_id = raw
        .get("id")
        .or_else(|| raw.get("entity_id"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<unknown>");
    match row_attribution(raw) {
        Some((project, _)) if project_ids_match(project, current_project_id) => true,
        Some((project, field)) => {
            tracing::debug!(
                "[Cassy sync] WARNING: skipping task dependency '{edge_id}' from foreign project '{project}' (by {field}; expected '{current_project_id}')"
            );
            false
        }
        None => {
            tracing::debug!(
                "[Cassy sync] WARNING: parking task dependency '{edge_id}' — no project identity (expected '{current_project_id}')"
            );
            false
        }
    }
}

fn dependency_entity_id(dependency: &Dependency) -> String {
    format!(
        "{}:{}:{}",
        dependency.from_id, dependency.to_id, dependency.dep_type
    )
}

#[derive(Debug, Default)]
struct RemoteDependencyState {
    live: BTreeMap<String, Dependency>,
    /// Tombstoned edges with the instant the cloud recorded the delete. The
    /// timestamp is what lets a genuinely recreated local edge win.
    deleted: BTreeMap<String, DateTime<Utc>>,
}

fn remote_dependency_state(
    raw_dependencies: &[serde_json::Value],
    current_project_id: &str,
) -> RemoteDependencyState {
    let mut state = RemoteDependencyState::default();
    for raw in raw_dependencies {
        // Same origin-first attribution as the ingest guard (GH #701): the
        // healer must not resurrect an edge the guard just refused.
        let project_matches =
            row_attribution(raw).is_some_and(|(project, _)| project == current_project_id);
        if !project_matches {
            continue;
        }
        let Ok(record) = serde_json::from_value::<TaskDependencyRecord>(raw.clone()) else {
            continue;
        };
        let is_delete = record.is_delete();
        let deleted_at = record.deleted_at();
        let dependency = record.dependency();
        let entity_id = dependency_entity_id(&dependency);
        if is_delete {
            state.live.remove(&entity_id);
            state.deleted.insert(entity_id, deleted_at);
        } else if !state.deleted.contains_key(&entity_id) {
            state.live.insert(entity_id, dependency);
        }
    }
    state
}

fn local_dependency_map(
    task_store: &dyn TaskStore,
) -> Result<BTreeMap<String, Dependency>, CasError> {
    Ok(task_store
        .list_dependencies(None)?
        .into_iter()
        .map(|dependency| (dependency_entity_id(&dependency), dependency))
        .collect())
}

fn apply_task_dependencies(
    raw_dependencies: &[serde_json::Value],
    task_store: &dyn TaskStore,
    current_project_id: &str,
    result: &mut SyncResult,
    deleted_dependency_ids: &BTreeMap<String, DateTime<Utc>>,
    queue: &SyncQueue,
) {
    for raw in raw_dependencies {
        if !task_dependency_matches_project(raw, current_project_id) {
            continue;
        }
        let edge_id = raw
            .get("id")
            .or_else(|| raw.get("entity_id"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<unknown>")
            .to_owned();
        let record: TaskDependencyRecord = match serde_json::from_value(raw.clone()) {
            Ok(record) => record,
            Err(error) => {
                result.errors.push(format!(
                    "task dependency deserialize error (id={edge_id}): {error}"
                ));
                continue;
            }
        };
        let is_delete = record.is_delete();
        let deleted_at = record.deleted_at();
        let dependency = record.dependency();
        let dependency_id = dependency_entity_id(&dependency);
        if is_delete {
            apply_dependency_tombstone(
                &dependency,
                &dependency_id,
                deleted_at,
                task_store,
                queue,
                result,
            );
            continue;
        }

        // A delete tombstone wins for the whole response, regardless of wire
        // ordering. This prevents a stale upsert in the same envelope from
        // recreating an edge just removed by the user.
        if deleted_dependency_ids.contains_key(&dependency_id) {
            continue;
        }

        let from_exists = task_store.get(&dependency.from_id).is_ok();
        let to_exists = task_store.get(&dependency.to_id).is_ok();
        if !from_exists || !to_exists {
            tracing::warn!(
                from_id = %dependency.from_id,
                to_id = %dependency.to_id,
                dep_type = %dependency.dep_type,
                "Parking dangling task dependency from cloud pull because one or both tasks are absent locally"
            );
            continue;
        }

        if let Err(error) = task_store.add_dependency(&dependency) {
            result.errors.push(format!(
                "Task dependency upsert error ({}:{}): {error}",
                dependency.from_id, dependency.to_id
            ));
        } else {
            result.pulled_task_dependencies += 1;
        }
    }
}

/// Apply one cloud deletion tombstone: drop the local edge, remember the
/// delete, and retire any queued upsert for it.
///
/// The ledger write is the durable half. An incremental pull delivers a
/// tombstone once, so without a local record the next reconciliation would see
/// a local-only edge and push it straight back — the resurrection loop GH #640
/// exists to close.
fn apply_dependency_tombstone(
    dependency: &Dependency,
    dependency_id: &str,
    deleted_at: DateTime<Utc>,
    task_store: &dyn TaskStore,
    queue: &SyncQueue,
    result: &mut SyncResult,
) {
    let existed = task_store
        .get_dependencies(&dependency.from_id)
        .map(|dependencies| {
            dependencies.iter().any(|existing| {
                existing.to_id == dependency.to_id && existing.dep_type == dependency.dep_type
            })
        })
        .unwrap_or(false);
    if let Err(error) = task_store.remove_dependency_of_type(
        &dependency.from_id,
        &dependency.to_id,
        dependency.dep_type,
    ) {
        result.errors.push(format!(
            "Task dependency delete error ({}:{}): {error}",
            dependency.from_id, dependency.to_id
        ));
        return;
    }
    if existed {
        result.deleted_task_dependencies += 1;
    }
    if let Err(error) = queue.record_dependency_tombstone(
        dependency_id,
        &dependency.from_id,
        &dependency.to_id,
        &dependency.dep_type.to_string(),
        deleted_at,
    ) {
        result
            .errors
            .push(format!("Task dependency tombstone error ({dependency_id}): {error}"));
    }
    // The server refuses a stale resurrection anyway; dropping the queued row
    // stops every future push from retrying a request that cannot succeed.
    let _ = queue.drop_queued_dependency_upsert(dependency_id);
}

#[derive(Debug, Default)]
struct DependencyHealReport {
    to_cloud: usize,
    from_cloud: usize,
    skipped_by_tombstone: usize,
}

/// How often a watermarked pull spends one extra request on a full dependency
/// snapshot. Reconciliation is a repair path, not a per-pull duty: the ordinary
/// edge lifecycle is already queued by `dep_add`/`dep_remove`.
const DEPENDENCY_RECONCILE_INTERVAL_SECS: i64 = 6 * 60 * 60;

impl CloudSyncer {
    /// Apply the endpoint's dependency envelope, then reconcile the complete
    /// local edge set against it. An omitted field is handled by the caller as
    /// an older endpoint response and intentionally skips healing.
    fn apply_and_heal_task_dependencies(
        &self,
        raw_dependencies: Vec<serde_json::Value>,
        task_store: &dyn TaskStore,
        current_project_id: &str,
        team_id: Option<&str>,
        incremental: bool,
        result: &mut SyncResult,
    ) -> Result<DependencyHealReport, CasError> {
        let local_before = local_dependency_map(task_store)?;
        let remote = remote_dependency_state(&raw_dependencies, current_project_id);

        apply_task_dependencies(
            &raw_dependencies,
            task_store,
            current_project_id,
            result,
            &remote.deleted,
            &self.queue,
        );

        let local_after = local_dependency_map(task_store)?;

        // A `since=`-filtered envelope says which edges CHANGED, not which
        // edges the cloud holds. Diffing the full local set against it made
        // every untouched local edge look cloud-missing and re-queued it on
        // every pull (cas-cf1f; 1,371 rows from one pull on the reporting
        // host). Reconciliation therefore needs a complete snapshot: the
        // envelope itself when no watermark was sent, otherwise one extra
        // `types=task_dependencies` request, taken on an interval.
        let snapshot = if incremental {
            match self.due_dependency_reconcile(team_id, current_project_id)? {
                false => None,
                true => match self.fetch_dependency_snapshot(team_id, current_project_id) {
                    Ok(raw_snapshot) => {
                        let snapshot_state =
                            remote_dependency_state(&raw_snapshot, current_project_id);
                        // Apply only the snapshot's deltas: a full re-application
                        // of thousands of unchanged edges is pure write churn.
                        let pending_records = snapshot_records_needing_apply(
                            raw_snapshot,
                            current_project_id,
                            &local_after,
                        );
                        apply_task_dependencies(
                            &pending_records,
                            task_store,
                            current_project_id,
                            result,
                            &snapshot_state.deleted,
                            &self.queue,
                        );
                        Some(snapshot_state)
                    }
                    Err(error) => {
                        // A repair pass that cannot see the cloud's full edge
                        // set must do nothing rather than guess.
                        tracing::debug!(
                            "[Cassy sync] dependency reconciliation skipped: {error}"
                        );
                        None
                    }
                },
            }
        } else {
            Some(remote)
        };

        let Some(snapshot) = snapshot else {
            return Ok(DependencyHealReport {
                to_cloud: 0,
                from_cloud: materialized_from_cloud(&local_before, &local_after),
                skipped_by_tombstone: 0,
            });
        };

        let local_after = local_dependency_map(task_store)?;
        let from_cloud = materialized_from_cloud(&local_before, &local_after);

        // Existing queue state is authoritative for idempotency: a pending
        // local delete must never be overwritten by this repair pass.
        let pending = match team_id {
            Some(team_id) => self.queue.pending_for_team(team_id, usize::MAX, i32::MAX)?,
            None => self.queue.pending_for_entity_type(
                Some(EntityType::TaskDependency),
                usize::MAX,
                i32::MAX,
            )?,
        };
        let pending_operations = pending
            .into_iter()
            .map(|item| (item.entity_id, item.operation))
            .collect::<BTreeMap<_, _>>();
        let tombstones = self.queue.dependency_tombstones()?;

        let mut to_cloud = 0;
        let mut skipped_by_tombstone = 0;
        for (entity_id, dependency) in local_after {
            if snapshot.live.contains_key(&entity_id) {
                continue;
            }
            if pending_operations.contains_key(&entity_id) {
                continue;
            }

            // Ordering, not arrival, decides: a tombstone suppresses an edge
            // created at or before the delete, while an edge recreated after it
            // is newer state that must reach the cloud (and retires the
            // tombstone so it cannot suppress the edge again).
            let tombstoned_at = snapshot
                .deleted
                .get(&entity_id)
                .copied()
                .into_iter()
                .chain(tombstones.get(&entity_id).copied())
                .max();
            if let Some(tombstoned_at) = tombstoned_at {
                if dependency.created_at <= tombstoned_at {
                    skipped_by_tombstone += 1;
                    if let Err(error) = task_store.remove_dependency_of_type(
                        &dependency.from_id,
                        &dependency.to_id,
                        dependency.dep_type,
                    ) {
                        result.errors.push(format!(
                            "Task dependency delete error ({}:{}): {error}",
                            dependency.from_id, dependency.to_id
                        ));
                    }
                    let _ = self.queue.record_dependency_tombstone(
                        &entity_id,
                        &dependency.from_id,
                        &dependency.to_id,
                        &dependency.dep_type.to_string(),
                        tombstoned_at,
                    );
                    let _ = self.queue.drop_queued_dependency_upsert(&entity_id);
                    continue;
                }
                self.queue.clear_dependency_tombstone(&entity_id)?;
            }

            let payload = serde_json::json!({
                "from_id": dependency.from_id,
                "to_id": dependency.to_id,
                "dep_type": dependency.dep_type.to_string(),
                "created_at": dependency.created_at,
                "origin_project": current_project_id,
            });
            let payload = serde_json::to_string(&payload).map_err(|error| {
                CasError::Other(format!(
                    "Could not serialize healed task dependency: {error}"
                ))
            })?;
            match team_id {
                Some(team_id) => self.queue.enqueue_for_team(
                    EntityType::TaskDependency,
                    &entity_id,
                    SyncOperation::Upsert,
                    Some(&payload),
                    team_id,
                )?,
                None => self.queue.enqueue(
                    EntityType::TaskDependency,
                    &entity_id,
                    SyncOperation::Upsert,
                    Some(&payload),
                )?,
            }
            to_cloud += 1;
        }

        self.queue.set_metadata(
            &dependency_reconcile_key(team_id, current_project_id),
            &Utc::now().to_rfc3339(),
        )?;
        // Follow the cloud's 90-day tombstone horizon so the ledger cannot
        // outlive the server's own memory of the delete.
        let _ = self.queue.prune_dependency_tombstones(Utc::now());

        Ok(DependencyHealReport {
            to_cloud,
            from_cloud,
            skipped_by_tombstone,
        })
    }

    /// Whether a watermarked pull should spend one request on a full snapshot.
    fn due_dependency_reconcile(
        &self,
        team_id: Option<&str>,
        project_id: &str,
    ) -> Result<bool, CasError> {
        let key = dependency_reconcile_key(team_id, project_id);
        let Some(previous) = self.queue.get_metadata(&key)? else {
            return Ok(true);
        };
        let Ok(previous) = DateTime::parse_from_rfc3339(&previous) else {
            return Ok(true);
        };
        Ok((Utc::now() - previous.with_timezone(&Utc)).num_seconds()
            >= DEPENDENCY_RECONCILE_INTERVAL_SECS)
    }

    /// Fetch the cloud's complete dependency-edge set for this project.
    ///
    /// `types=task_dependencies` (the plural wire key) with no `since` is the
    /// only supported full-snapshot request; the cloud has no `full=1` flag and
    /// no per-entity count/hash endpoint to diff against instead.
    fn fetch_dependency_snapshot(
        &self,
        team_id: Option<&str>,
        project_id: &str,
    ) -> Result<Vec<serde_json::Value>, CasError> {
        let body = match team_id {
            Some(team_id) => {
                let token = self
                    .cloud_config
                    .token
                    .as_ref()
                    .ok_or_else(|| CasError::Other("Not logged in".to_string()))?;
                let url = format!(
                    "{}/api/teams/{}/sync/pull?project_id={}&types=task_dependencies",
                    self.cloud_config.endpoint,
                    team_id,
                    project_id.replace('/', "%2F")
                );
                match ureq::get(&url)
                    .timeout(self.config.timeout)
                    .set("Authorization", &format!("Bearer {token}"))
                    .call()
                {
                    Ok(response) => response.into_json::<serde_json::Value>().map_err(|error| {
                        CasError::Other(format!("Failed to parse dependency snapshot: {error}"))
                    })?,
                    Err(ureq::Error::Status(code, response)) => {
                        let body = response.into_string().unwrap_or_default();
                        return Err(CasError::Other(format!(
                            "Dependency snapshot failed with status {code}: {body}"
                        )));
                    }
                    Err(ureq::Error::Transport(error)) => {
                        return Err(CasError::Other(format!("Network error: {error}")));
                    }
                }
            }
            None => {
                self.fetch_pull_json(
                    Some(project_id),
                    &["types=task_dependencies".to_string()],
                )?
                .0
            }
        };
        Ok(body
            .get("task_dependencies")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default())
    }

    fn record_dependency_heal(result: &mut SyncResult, report: DependencyHealReport) {
        result.healed_task_dependencies_to_cloud += report.to_cloud;
        result.healed_task_dependencies_from_cloud += report.from_cloud;
        result.skipped_task_dependencies_by_tombstone += report.skipped_by_tombstone;
        if let Some(summary) = result.dependency_heal_summary() {
            tracing::debug!("[Cassy sync] {summary}");
        }
    }
}

/// Edges present locally only because this pull materialized them.
fn materialized_from_cloud(
    before: &BTreeMap<String, Dependency>,
    after: &BTreeMap<String, Dependency>,
) -> usize {
    after
        .keys()
        .filter(|entity_id| !before.contains_key(*entity_id))
        .count()
}

fn dependency_reconcile_key(team_id: Option<&str>, project_id: &str) -> String {
    match team_id {
        Some(team_id) => format!("last_dependency_reconcile_at_{team_id}_{project_id}"),
        None => format!("last_dependency_reconcile_at_personal_{project_id}"),
    }
}

/// Narrow a full snapshot to the records that change local state: tombstones
/// for edges still held locally, and live edges the local database is missing.
fn snapshot_records_needing_apply(
    raw_snapshot: Vec<serde_json::Value>,
    current_project_id: &str,
    local: &BTreeMap<String, Dependency>,
) -> Vec<serde_json::Value> {
    raw_snapshot
        .into_iter()
        .filter(|raw| {
            let Ok(record) = serde_json::from_value::<TaskDependencyRecord>(raw.clone()) else {
                return false;
            };
            let is_delete = record.is_delete();
            let entity_id = dependency_entity_id(&record.dependency());
            if is_delete {
                local.contains_key(&entity_id)
            } else {
                !local.contains_key(&entity_id)
            }
        })
        .filter(|raw| task_dependency_matches_project(raw, current_project_id))
        .collect()
}

/// Read the cloud mutation timestamp carried by a pulled entry.
///
/// The local `Entry` model predates the wire-level `updated_at` field, so
/// retain the timestamp as pull metadata instead of changing that public
/// model. Older responses fall back to `last_accessed`/`created` below.
fn pulled_entry_updated_at(raw: &serde_json::Value) -> Option<DateTime<Utc>> {
    raw.get("updated_at")
        .or_else(|| raw.get("updatedAt"))
        .and_then(serde_json::Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

const PROPOSAL_PROVENANCE_BEGIN: &str = "--- BEGIN SERVER-ATTESTED PROPOSAL PROVENANCE ---";
const PROPOSAL_PROVENANCE_END: &str = "--- END CLIENT-ASSERTED PROPOSAL PROVENANCE ---";

fn render_provenance_value(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"(unrenderable)\"".to_string())
}

fn unique_complete_provenance_block(notes: &str) -> Option<(usize, usize)> {
    let begins = notes
        .match_indices(PROPOSAL_PROVENANCE_BEGIN)
        .collect::<Vec<_>>();
    let ends = notes
        .match_indices(PROPOSAL_PROVENANCE_END)
        .collect::<Vec<_>>();
    if begins.len() != 1 || ends.len() != 1 {
        return None;
    }
    let start = begins[0].0;
    let end = ends[0].0 + PROPOSAL_PROVENANCE_END.len();
    let marker_is_line = |offset: usize, marker_len: usize| {
        (offset == 0 || notes.as_bytes().get(offset.wrapping_sub(1)) == Some(&b'\n'))
            && (offset + marker_len == notes.len()
                || notes.as_bytes().get(offset + marker_len) == Some(&b'\n'))
    };
    (start < ends[0].0
        && marker_is_line(start, PROPOSAL_PROVENANCE_BEGIN.len())
        && marker_is_line(ends[0].0, PROPOSAL_PROVENANCE_END.len()))
    .then_some((start, end))
}

/// Persist proposal provenance visibly in the ordinary local task record.
/// The cloud proposal row remains authoritative; this rendering is explicitly
/// labeled and is refreshed whenever the materialized task is pulled.
fn render_task_proposal_provenance(raw: &mut serde_json::Value) {
    let Some(raw_provenance) = raw.get("proposal_provenance").cloned() else {
        return;
    };
    let Ok(provenance) =
        serde_json::from_value::<crate::cloud::task_proposals::ProposalProvenance>(raw_provenance)
    else {
        return;
    };
    let server = &provenance.server_attested;
    let client = &provenance.client_asserted;
    let rendered = format!(
        "{PROPOSAL_PROVENANCE_BEGIN}\n  proposal_id: {}\n  target_task_id: {}\n  creator_user_id: {}\n  team_id: {}\n  origin_project_canonical_id: {}\n  target_project_canonical_id: {}\n  received_at: {}\n  client_request_id: {}\n--- END SERVER-ATTESTED PROPOSAL PROVENANCE ---\n\n--- BEGIN CLIENT-ASSERTED PROPOSAL PROVENANCE ---\n  origin_session_id: {}\n  origin_agent_id: {}\n  origin_agent_name: {}\n  origin_agent_role: {}\n  client_version: {}\n  client_build: {}\n{PROPOSAL_PROVENANCE_END}",
        render_provenance_value(&server.proposal_id),
        render_provenance_value(&server.target_task_id),
        render_provenance_value(&server.creator_user_id),
        render_provenance_value(&server.team_id),
        render_provenance_value(&server.origin_project_canonical_id),
        render_provenance_value(&server.target_project_canonical_id),
        render_provenance_value(&server.received_at),
        render_provenance_value(&server.client_request_id),
        client
            .origin_session_id
            .as_deref()
            .map(render_provenance_value)
            .unwrap_or_else(|| "(not asserted)".to_string()),
        client
            .origin_agent_id
            .as_deref()
            .map(render_provenance_value)
            .unwrap_or_else(|| "(not asserted)".to_string()),
        client
            .origin_agent_name
            .as_deref()
            .map(render_provenance_value)
            .unwrap_or_else(|| "(not asserted)".to_string()),
        client
            .origin_agent_role
            .as_deref()
            .map(render_provenance_value)
            .unwrap_or_else(|| "(not asserted)".to_string()),
        client
            .client_version
            .as_deref()
            .map(render_provenance_value)
            .unwrap_or_else(|| "(not asserted)".to_string()),
        client
            .client_build
            .as_deref()
            .map(render_provenance_value)
            .unwrap_or_else(|| "(not asserted)".to_string()),
    );
    let notes = raw
        .get("notes")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let updated = if let Some((start, end)) = unique_complete_provenance_block(notes) {
        format!("{}{}{}", &notes[..start], rendered, &notes[end..])
    } else if notes.is_empty() {
        rendered
    } else {
        format!("{}\n\n{}", notes.trim_end(), rendered)
    };
    raw["notes"] = serde_json::Value::String(updated);
}

/// Build a project-scoped `/api/sync/pull` URL, **failing closed** when the
/// project scope cannot be resolved.
///
/// `extra_params` are appended verbatim (already `key=value` encoded); the
/// `project_id=` parameter is always appended by this function. If
/// `get_project_canonical_id()` returns `None` — i.e. the caller is not inside
/// a CAS project directory, which is the realistic daemon / cwd-independent
/// case — no URL is produced at all and the pull aborts. Omitting the scope
/// would ask the server for *every* project's rows, which is exactly the
/// cross-project contamination this path exists to prevent.
///
/// Returns `(url, resolved_project_id)`; callers that also filter the response
/// client-side reuse the resolved id rather than resolving it a second time.
pub(crate) fn build_scoped_pull_url(
    endpoint: &str,
    extra_params: &[String],
) -> Result<(String, String), CasError> {
    build_scoped_pull_url_with(endpoint, extra_params, get_project_canonical_id)
}

/// [`build_scoped_pull_url`] with an injectable project-scope resolver.
///
/// The resolver is a parameter purely so tests can exercise the unresolvable
/// (`None`) branch without depending on the process-wide cache inside
/// `get_project_canonical_id` or on the test's working directory.
pub(crate) fn build_scoped_pull_url_with(
    endpoint: &str,
    extra_params: &[String],
    resolve_project_id: impl FnOnce() -> Option<String>,
) -> Result<(String, String), CasError> {
    let project_id = resolve_project_id().ok_or_else(|| {
        CasError::Other("Cannot pull: not inside a CAS project directory".to_string())
    })?;
    let mut params: Vec<String> = extra_params.to_vec();
    params.push(format!("project_id={}", project_id.replace('/', "%2F")));
    let url = format!("{endpoint}{PULL_PATH}?{}", params.join("&"));
    Ok((url, project_id))
}

/// Check whether a raw JSON entity belongs to the current project.
///
/// Returns `true` if the entity should be accepted, `false` if it should be skipped.
///
/// An entity is accepted only when its `project_canonical_id` or `project_id` field
/// is a string that exactly matches `current_project_id`. All other cases — missing
/// field, null field, wrong project, or unexpected type — are rejected. The legacy
/// "no field = accept" and "null = accept" paths have been removed now that the cloud
/// always echoes `project_id` in every pull-response row (cas-6479).
/// The per-row ingest guard. Shared with the knowledge pull (`knowledge.rs`)
/// so both directions of the sync client apply *one* definition of "is this
/// row mine", rather than a second implementation that can drift from this one.
///
/// Project identities are compared through the canonical cloud normalizer so
/// a legacy remote-shaped row can match an explicit bare-slug pin. Values that
/// normalize to different host/org/repository identities remain foreign.
pub(crate) fn entity_matches_project(
    raw: &serde_json::Value,
    current_project_id: &str,
    entity_kind: &str,
) -> bool {
    let entity_id = raw
        .get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("<unknown>");

    // GH #701: the row's own `origin_project` outranks the server's scope
    // stamp. See `row_attribution` for the measurement that forced this — the
    // stamp is an echo of the requested scope, so reading it first admits
    // every replicated row as native.
    if let Some(origin) = row_origin_project(raw) {
        if project_ids_match(origin, current_project_id) {
            return true;
        }
        record_project_warning(
            entity_kind,
            origin,
            &format!(
                "skipping {entity_kind} '{entity_id}' — origin project '{origin}' is not \
                 '{current_project_id}' (the row was replicated into this scope)"
            ),
        );
        return false;
    }

    // Check both field names the server might use
    let project_field = raw
        .get("project_canonical_id")
        .or_else(|| raw.get("project_id"));

    match project_field {
        None => {
            // Missing field — cloud now always includes project_id; treat as unscoped/foreign.
            record_project_warning(
                entity_kind,
                "<missing>",
                &format!(
                    "skipping {entity_kind} '{entity_id}' — no project_id field (expected '{current_project_id}')"
                ),
            );
            false
        }
        Some(serde_json::Value::Null) => {
            // Explicitly null — no longer accepted; cloud must scope all entities.
            record_project_warning(
                entity_kind,
                "<null>",
                &format!(
                    "skipping {entity_kind} '{entity_id}' — null project_id (expected '{current_project_id}')"
                ),
            );
            false
        }
        Some(serde_json::Value::String(s)) => {
            if project_ids_match(s, current_project_id) {
                true
            } else {
                record_project_warning(
                    entity_kind,
                    s,
                    &format!(
                        "skipping {entity_kind} '{entity_id}' from foreign project '{s}' (expected '{current_project_id}')"
                    ),
                );
                false
            }
        }
        Some(_) => {
            // Unexpected type — reject; unexpected field shapes shouldn't be silently accepted.
            record_project_warning(
                entity_kind,
                "<invalid>",
                &format!(
                    "skipping {entity_kind} '{entity_id}' — unexpected project_id type (expected string '{current_project_id}')"
                ),
            );
            false
        }
    }
}

fn project_ids_match(candidate: &str, current: &str) -> bool {
    canonical_project_ids_match(candidate, current)
}

fn task_wire_id(raw: &serde_json::Value) -> Option<&str> {
    raw.get("id").and_then(serde_json::Value::as_str)
}

fn task_wire_origin_project(raw: &serde_json::Value) -> Option<&str> {
    raw.get("origin_project")
        .and_then(serde_json::Value::as_str)
}

/// Return the server-attested project key for a task row, when one exists.
///
/// The requested scope is deliberately not a fallback: on a team pull the
/// response may contain a replica from another project, and stamping that row
/// with the request would make it look native to doctor. A legacy row with no
/// origin and no server project is parked by the caller.
fn task_wire_cloud_project(raw: &serde_json::Value) -> Option<&str> {
    ["project_canonical_id", "project_id"]
        .into_iter()
        .find_map(|field| {
            raw.get(field)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|project| !project.is_empty())
        })
}

fn task_wire_is_owner(raw: &serde_json::Value) -> bool {
    task_wire_origin_project(raw).is_some_and(|origin| {
        task_wire_cloud_project(raw)
            .is_some_and(|cloud_project| project_ids_match(origin, cloud_project))
    })
}

fn task_wire_updated_at(raw: &serde_json::Value) -> Option<DateTime<Utc>> {
    raw.get("updated_at")
        .or_else(|| raw.get("updatedAt"))
        .and_then(serde_json::Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

/// Collapse duplicate task IDs in a team envelope before deserialization and
/// upsert. A row whose origin matches its cloud project key is the owner row;
/// owner status wins even when a foreign replica row has a newer timestamp.
/// Rows without an owner signal retain timestamp ordering, but are still
/// reduced to one row so wire order cannot make the result nondeterministic.
fn select_owner_task_rows(
    raw_tasks: Vec<serde_json::Value>,
) -> (Vec<serde_json::Value>, Vec<(String, serde_json::Value)>) {
    let mut grouped = BTreeMap::<String, Vec<serde_json::Value>>::new();
    let mut unkeyed = Vec::new();
    for raw in raw_tasks {
        let Some(id) = task_wire_id(&raw) else {
            unkeyed.push(raw);
            continue;
        };
        grouped.entry(id.to_owned()).or_default().push(raw);
    }

    let mut selected = unkeyed;
    let mut discarded = Vec::new();
    for (id, mut rows) in grouped {
        if rows.len() == 1 {
            selected.push(rows.pop().expect("one-row task group is non-empty"));
            continue;
        }

        let winner_index = rows
            .iter()
            .enumerate()
            .max_by(|(left_index, left), (right_index, right)| {
                task_wire_is_owner(left).cmp(&task_wire_is_owner(right))
                    .then_with(|| task_wire_updated_at(left).cmp(&task_wire_updated_at(right)))
                    .then_with(|| left_index.cmp(right_index))
            })
            .map(|(index, _)| index)
            .expect("multi-row task group is non-empty");
        let winner = rows.swap_remove(winner_index);
        discarded.extend(rows.into_iter().map(|row| (id.clone(), row)));
        selected.push(winner);
    }
    (selected, discarded)
}

/// cas-fc52: detect a teammate's web-initiated close (cloud contract §4).
///
/// The cloud server records a web close as a soft tombstone merged into the
/// task's `data`: `closed_via == "web"` (plus `status="closed"`,
/// `close_reason`, `closed_at`) with a bumped `updated_at`. The marker arrives
/// at the top level of the pulled task JSON (the same level the strongly-typed
/// `Task` deserializes from). `closed_via == "web"` is the discriminator: the
/// client's OWN pushed closes never carry it, so this never reconciles a
/// self-close as web-initiated (no loop, no double-close).
fn is_web_close_tombstone(raw: &serde_json::Value) -> bool {
    raw.get("closed_via").and_then(|v| v.as_str()) == Some("web")
}

/// cas-fc52: apply a teammate's web-initiated close as the authoritative local
/// close (cloud contract §4). Unlike the timestamp-gated [`upsert_task`], this
/// forces the closed status even when the local row looks newer — a web close
/// is an explicit instruction, not a data-merge race.
///
/// Side effects mirror a real close at the task-store level: status becomes
/// `Closed` (carried on the tombstone `Task`, along with `close_reason` /
/// `closed_at`), and the task's `assignee` is cleared so no stale ownership
/// lingers on the closed task. The separate agent-lease row (if any) is left to
/// the existing lease GC — a closed task is not claimable, so a lingering lease
/// is inert.
///
/// Idempotent: a task already locally `Closed` is a no-op.
fn reconcile_web_close(
    store: &dyn TaskStore,
    task: Task,
    sync_id: &str,
    source: &str,
) -> Result<UpsertResult, CasError> {
    match store.get(&task.id) {
        Ok(local) => {
            if local.status == TaskStatus::Closed {
                // Already closed locally — nothing to do.
                return Ok(UpsertResult::Skipped);
            }
            // Merge ONLY the close signal onto the local row. A full overwrite
            // with the remote tombstone would clobber locally-authored,
            // not-yet-pushed content (notes / description / acceptance_criteria):
            // the web close is authoritative about the CLOSE, not the body.
            let mut merged = local.clone();
            merged.status = TaskStatus::Closed;
            merged.close_reason = task.close_reason;
            merged.closed_at = task.closed_at.or(merged.closed_at);
            merged.updated_at = task.updated_at;
            merged.assignee = None;
            append_sync_status_provenance(&mut merged, &local, sync_id, source);
            store.update(&merged)?;
            tracing::info!(
                task_id = %task.id,
                "cas-fc52: reconciled web-initiated close from cloud pull"
            );
            Ok(UpsertResult::Updated)
        }
        Err(cas_store::StoreError::TaskNotFound(_)) => {
            // Never had this task locally — record the closed tombstone as-is
            // (no local content to preserve).
            let mut tombstone = task;
            tombstone.assignee = None;
            store.add(&tombstone)?;
            Ok(UpsertResult::Created)
        }
        Err(e) => Err(e.into()),
    }
}

/// Merge append-only task-note blocks without allowing a whole-row conflict
/// resolution to discard one machine's timeline.
fn merge_task_notes(local: &str, remote: &str) -> String {
    let mut blocks = Vec::<(Option<chrono::NaiveDateTime>, usize, String)>::new();
    for notes in [local, remote] {
        for block in notes.split("\n\n") {
            let block = block.trim();
            if block.is_empty() || blocks.iter().any(|(_, _, existing)| existing == block) {
                continue;
            }
            let timestamp = block
                .strip_prefix('[')
                .and_then(|value| value.split_once(']'))
                .and_then(|(value, _)| {
                    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M").ok()
                });
            let order = blocks.len();
            blocks.push((timestamp, order, block.to_string()));
        }
    }
    blocks.sort_by_key(|(timestamp, order, _)| (timestamp.is_none(), *timestamp, *order));
    blocks
        .into_iter()
        .map(|(_, _, note)| note)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A terminal row is a durable outcome, not merely the latest version of a
/// mutable task body. An incoming active row may replace it only when the
/// remote task carries the audit event written by Cassy's authorised `reopen`
/// action after the local terminal timestamp.
fn rejects_terminal_regression(local: &Task, remote: &Task) -> bool {
    if !local.is_terminal() || remote.is_terminal() {
        return false;
    }

    !has_explicit_remote_reopen(remote, local.closed_at)
}

/// Cassy's `task reopen` action writes `[YYYY-mm-dd HH:MM] Reopened:
/// actor=<agent> reason=<reason>` into the task's replicated note timeline.
/// Treat that timestamped, attributed record as the reopening event; a bare
/// active task row, even one with a newer `updated_at`, is not authorization to
/// undo a close/cancellation.
fn has_explicit_remote_reopen(
    remote: &Task,
    local_closed_at: Option<chrono::DateTime<chrono::Utc>>,
) -> bool {
    let Some(local_closed_at) = local_closed_at else {
        return false;
    };
    remote.notes.split("\n\n").any(|note| {
        let Some((timestamp, event)) = note.strip_prefix('[').and_then(|note| note.split_once(']'))
        else {
            return false;
        };
        let Some(audit) = event.trim_start().strip_prefix("Reopened:") else {
            return false;
        };
        let Some((actor, reason)) = audit
            .trim()
            .strip_prefix("actor=")
            .and_then(|audit| audit.split_once(" reason="))
        else {
            return false;
        };
        if actor.trim().is_empty() || reason.trim().is_empty() {
            return false;
        }
        let Ok(timestamp) = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M")
        else {
            return false;
        };
        let timestamp = timestamp.and_utc();
        timestamp >= local_closed_at - chrono::Duration::minutes(1)
    })
}

fn append_sync_status_provenance(merged: &mut Task, local: &Task, sync_id: &str, source: &str) {
    if local.status == merged.status {
        return;
    }

    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
    let prior_close_reason = local
        .close_reason
        .as_deref()
        .unwrap_or("<none>")
        .replace(['\n', '\r'], " ");
    let note = format!(
        "[{timestamp}] [CAS_SYNC_STATUS] sync_id={sync_id} source={source} prior_status={} prior_close_reason={prior_close_reason} applied_status={}",
        local.status, merged.status,
    );
    if merged.notes.is_empty() {
        merged.notes = note;
    } else {
        merged.notes.push_str("\n\n");
        merged.notes.push_str(&note);
    }
}

impl CloudSyncer {
    fn record_owner_conflict_value(
        &self,
        task_id: &str,
        discarded_row: &serde_json::Value,
    ) -> Result<(), CasError> {
        let discarded_row_json = serde_json::to_string(&serde_json::json!({
            "rejected_remote": discarded_row,
            "reason": "owner_wins",
        }))
        .map_err(|error| {
            CasError::Other(format!("Could not serialize owner sync conflict: {error}"))
        })?;
        self.queue.record_conflict(
            EntityType::Task.as_str(),
            task_id,
            &discarded_row_json,
            "owner",
            "owner_wins",
            None,
            None,
        )?;
        Ok(())
    }

    fn upsert_owner_task(
        &self,
        store: &dyn TaskStore,
        task: Task,
        sync_id: &str,
        source: &str,
    ) -> Result<UpsertResult, CasError> {
        match store.get(&task.id) {
            Ok(local) => {
                if rejects_terminal_regression(&local, &task) {
                    self.record_terminal_regression_conflict(&local, &task)?;
                    return Ok(UpsertResult::Skipped);
                }
                let notes_differ = local.notes != task.notes;
                self.journal_local_overwrite(
                    EntityType::Task,
                    &task.id,
                    &local,
                    "owner",
                    "owner_wins",
                )?;
                let mut merged = task;
                if notes_differ {
                    merged.notes = merge_task_notes(&local.notes, &merged.notes);
                }
                append_sync_status_provenance(&mut merged, &local, sync_id, source);
                store.update(&merged)?;
                Ok(UpsertResult::Updated)
            }
            Err(cas_store::StoreError::TaskNotFound(_)) => {
                store.add(&task)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Apply owner identity before the ordinary timestamp/strategy resolver.
    /// A local row carrying a different owner is a replica and cannot replace
    /// the owner row. Conversely, an owner row replaces a foreign replica even
    /// when its timestamp is older. Terminal regressions still use the
    /// existing explicit-reopen guard.
    fn upsert_task_with_owner_preference(
        &self,
        store: &dyn TaskStore,
        task: Task,
        incoming_is_owner: bool,
        strategy: Option<ConflictResolution>,
        sync_id: &str,
        source: &str,
    ) -> Result<UpsertResult, CasError> {
        if let Ok(local) = store.get(&task.id) {
            let origins_differ = match (
                local.origin_project.as_deref(),
                task.origin_project.as_deref(),
            ) {
                (Some(local), Some(incoming)) => !project_ids_match(local, incoming),
                (None, None) => false,
                _ => true,
            };
            if incoming_is_owner && origins_differ {
                return self.upsert_owner_task(store, task, sync_id, source);
            }
            if !incoming_is_owner && origins_differ && local.origin_project.is_some() {
                // Preserve the existing terminal guard's conflict strategy and
                // audit shape for an active row attempting to reopen a close.
                if local.is_terminal() && !task.is_terminal() {
                    return match strategy {
                        Some(strategy) => {
                            self.upsert_task_with_strategy(store, task, strategy, sync_id, source)
                        }
                        None => self.upsert_task(store, task, sync_id, source),
                    };
                }
                let discarded = serde_json::to_value(&task).map_err(|error| {
                    CasError::Other(format!("Could not serialize owner sync conflict: {error}"))
                })?;
                self.record_owner_conflict_value(&task.id, &discarded)?;
                return Ok(UpsertResult::Skipped);
            }
        }

        match strategy {
            Some(strategy) => {
                self.upsert_task_with_strategy(store, task, strategy, sync_id, source)
            }
            None => self.upsert_task(store, task, sync_id, source),
        }
    }

    fn record_terminal_regression_conflict(
        &self,
        local: &Task,
        remote: &Task,
    ) -> Result<(), CasError> {
        let discarded_row_json = serde_json::to_string(&serde_json::json!({
            "local": local,
            "rejected_remote": remote,
            "reason": "terminal_status_guard",
        }))
        .map_err(|error| {
            CasError::Other(format!(
                "Could not serialize terminal sync conflict: {error}"
            ))
        })?;
        self.queue.record_conflict(
            EntityType::Task.as_str(),
            &local.id,
            &discarded_row_json,
            "local",
            "terminal_status_guard",
            None,
            None,
        )?;
        tracing::warn!(
            task_id = %local.id,
            local_status = %local.status,
            remote_status = %remote.status,
            "cloud pull rejected an unproven terminal task regression"
        );
        Ok(())
    }

    fn journal_local_overwrite<T: serde::Serialize>(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        local: &T,
        winner_side: &str,
        strategy: &str,
    ) -> Result<(), CasError> {
        self.journal_local_overwrite_with_revisions(
            entity_type,
            entity_id,
            local,
            winner_side,
            strategy,
            None,
        )
    }

    /// Journal a discarded local row together with the revisions that settled
    /// the conflict.
    ///
    /// The revisions are read back from the conflict log this pull just wrote,
    /// so the journal row and the logged decision cannot disagree. A conflict
    /// resolved on the timestamp path records `NULL` revisions, which is how an
    /// operator tells the two regimes apart when auditing.
    fn journal_local_overwrite_with_revisions<T: serde::Serialize>(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        local: &T,
        winner_side: &str,
        strategy: &str,
        revisions: Option<(Option<i64>, Option<i64>)>,
    ) -> Result<(), CasError> {
        if self
            .queue
            .has_pending_entity_change(entity_type, entity_id)?
        {
            let json = serde_json::to_string(local).map_err(|error| {
                CasError::Other(format!("Could not serialize sync conflict: {error}"))
            })?;
            let (local_revision, remote_revision) = revisions
                .unwrap_or_else(|| self.logged_revisions(entity_type.as_str(), entity_id));
            self.queue.record_conflict(
                entity_type.as_str(),
                entity_id,
                &json,
                winner_side,
                strategy,
                local_revision,
                remote_revision,
            )?;
        }
        Ok(())
    }

    /// The revisions recorded by the most recent decision for this row.
    fn logged_revisions(&self, entity_type: &str, entity_id: &str) -> (Option<i64>, Option<i64>) {
        self.conflict_log
            .lock()
            .ok()
            .and_then(|conflicts| {
                conflicts
                    .iter()
                    .rev()
                    .find(|conflict| {
                        conflict.entity_type == entity_type && conflict.entity_id == entity_id
                    })
                    .map(|conflict| (conflict.local_revision, conflict.remote_revision))
            })
            .unwrap_or((None, None))
    }

    /// Fetch a project-scoped pull envelope without applying it to local
    /// stores or advancing a pull watermark.
    ///
    /// This is the read-only counterpart used by commands that need to inspect
    /// remote ownership before making a separate mutation, such as
    /// `cas cloud unlink --purge-remote`. The request still goes through the
    /// same scoped URL builder as [`Self::pull`], so adding a query filter
    /// cannot accidentally create an unscoped production pull path.
    pub(crate) fn pull_raw(
        &self,
        project_id: &str,
        entity_types: &[&str],
        team_id: Option<&str>,
    ) -> Result<serde_json::Value, CasError> {
        let mut params = Vec::new();
        if !entity_types.is_empty() {
            params.push(format!("types={}", entity_types.join(",")));
        }
        if let Some(team_id) = team_id {
            params.push(format!("team_id={}", urlencoding::encode(team_id)));
        }
        let (body, _) = self.fetch_pull_json(Some(project_id), &params)?;
        Ok(body)
    }

    fn fetch_pull_json(
        &self,
        project_id: Option<&str>,
        params: &[String],
    ) -> Result<(serde_json::Value, String), CasError> {
        let (pull_url, project_id) = match project_id {
            Some(project_id) => {
                build_scoped_pull_url_with(&self.cloud_config.endpoint, params, || {
                    Some(project_id.to_owned())
                })?
            }
            None => build_scoped_pull_url(&self.cloud_config.endpoint, params)?,
        };
        let token = self
            .cloud_config
            .token
            .as_ref()
            .ok_or_else(|| CasError::Other("Not logged in".to_string()))?;
        let response = ureq::get(&pull_url)
            .timeout(self.config.timeout)
            .set("Authorization", &format!("Bearer {token}"))
            .call();
        let body = match response {
            Ok(resp) => resp
                .into_json()
                .map_err(|e| CasError::Other(format!("Failed to parse response: {e}")))?,
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                return Err(CasError::Other(format!(
                    "Pull failed with status {code}: {body}"
                )));
            }
            Err(ureq::Error::Transport(e)) => {
                return Err(CasError::Other(format!("Network error: {e}")));
            }
        };
        Ok((body, project_id))
    }

    /// Mirror the cloud's per-project `aliases` record into
    /// `.cas/config.toml` and drop the cached alias class so the ingest guard
    /// picks it up on this same pull (GH #669).
    ///
    /// Deliberately infallible: identity refresh is an optimization of
    /// *attribution*, never a precondition for syncing rows.
    fn refresh_project_alias_record(&self) {
        let Ok(cas_root) = crate::store::find_cas_root() else {
            return;
        };
        let Some(token) = self.cloud_config.token.as_deref() else {
            return;
        };
        let Some(project_id) = self
            .push_project_canonical_id
            .clone()
            .or_else(crate::cloud::get_project_canonical_id)
        else {
            return;
        };
        match crate::cloud::refresh_project_alias_record(
            &cas_root,
            &self.cloud_config.endpoint,
            token,
            &project_id,
            self.config.timeout,
        ) {
            Ok(aliases) if !aliases.is_empty() => {
                crate::cloud::invalidate_cached_project_alias_class();
                tracing::debug!(
                    "[Cassy sync] project `{project_id}` has {} registered alias(es): {}",
                    aliases.len(),
                    aliases.join(", ")
                );
            }
            Ok(_) => crate::cloud::invalidate_cached_project_alias_class(),
            Err(e) => tracing::warn!(
                "[Cassy sync] could not refresh the project alias record for \
                 `{project_id}` ({e}); keeping the cached record"
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn pull(
        &self,
        store: &dyn Store,
        task_store: &dyn TaskStore,
        rule_store: &dyn RuleStore,
        skill_store: &dyn SkillStore,
        spec_store: &dyn SpecStore,
        event_store: &dyn EventStore,
        prompt_store: &dyn PromptStore,
        file_change_store: &dyn FileChangeStore,
        commit_link_store: &dyn CommitLinkStore,
    ) -> Result<SyncResult, CasError> {
        self.clear_conflict_log();
        self.clear_incoming_revisions();
        let mut result = SyncResult::default();
        let start = Instant::now();

        if !self.is_available() {
            return Ok(result);
        }

        // GH #669: refresh the per-project `aliases` record before the ingest
        // guard runs, so rows the server folded into this project's canonical
        // bucket are attributed here instead of skipped as foreign. Failure is
        // logged and the previously cached record is kept — an unreachable
        // identity endpoint must not fail a pull.
        self.refresh_project_alias_record();

        // Get last pull timestamp
        let since = self.queue.get_metadata("last_pull_at")?;
        let had_prior_watermark = since.is_some();

        let mut params = Vec::new();
        if let Some(since) = &since {
            params.push(format!("since={since}"));
        }
        let (raw_body, project_id) = self.fetch_pull_json(None, &params)?;
        let body: PullResponse = serde_json::from_value(raw_body)
            .map_err(|e| CasError::Other(format!("Failed to parse response: {e}")))?;

        // Use the already-resolved project ID for client-side entity validation
        let current_project_id = &project_id;

        // Process entries
        for raw_entry in body.entries.unwrap_or_default() {
            if !entity_matches_project(&raw_entry, &current_project_id, "entry") {
                continue;
            }
            let remote_updated_at = pulled_entry_updated_at(&raw_entry);
            let entry_revision = crate::cloud::wire_revision(&raw_entry);
            let entry_revision_id = raw_entry
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            if let Some(id) = &entry_revision_id {
                self.note_incoming_revision(EntityType::Entry, id, &raw_entry);
            }
            let remote_entry: Entry = match deserialize_pulled_entity(raw_entry, "entry") {
                Ok(e) => e,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            let remote_updated_at = remote_updated_at
                .unwrap_or_else(|| remote_entry.last_accessed.unwrap_or(remote_entry.created));
            match self.upsert_entry_lww(store, remote_entry, remote_updated_at) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_entries += 1;
                    if let (Some(id), Some(revision)) = (&entry_revision_id, entry_revision) {
                        let _ = self.queue.record_revision(EntityType::Entry, id, revision);
                    }
                }
                Ok(UpsertResult::Skipped) => {
                    result.record_local_conflict();
                }
                Err(e) => {
                    result.errors.push(format!("Entry error: {e}"));
                }
            }
        }

        // Process tasks
        let task_sync_id = uuid::Uuid::new_v4().to_string();
        for mut raw_task in body.tasks.unwrap_or_default() {
            if !entity_matches_project(&raw_task, &current_project_id, "task") {
                continue;
            }
            // cas-fc52: a teammate's web-initiated close arrives as a soft
            // tombstone (closed_via == "web"). The CLI owns the real local
            // close — apply it authoritatively rather than via the
            // timestamp-gated upsert.
            let web_close = is_web_close_tombstone(&raw_task);
            let task_revision = crate::cloud::wire_revision(&raw_task);
            if let Some(id) = raw_task.get("id").and_then(serde_json::Value::as_str) {
                self.note_incoming_revision(EntityType::Task, id, &raw_task);
            }
            render_task_proposal_provenance(&mut raw_task);
            let server_owner = task_wire_cloud_project(&raw_task).map(str::to_owned);
            let mut remote_task: Task = match deserialize_pulled_entity(raw_task, "task") {
                Ok(t) => t,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            // The scoped response proves which project supplied a legacy row;
            // preserve an explicit origin or use only the server-attested
            // project identity. The request scope is not an ownership stamp.
            if remote_task.origin_project.is_none() {
                let Some(server_owner) = server_owner else {
                    record_project_warning(
                        "task",
                        "<missing>",
                        &format!(
                            "parking task '{}' — no origin_project or server project identity",
                            remote_task.id
                        ),
                    );
                    continue;
                };
                remote_task.origin_project = Some(server_owner);
            }
            let incoming_is_owner = remote_task
                .origin_project
                .as_deref()
                .is_some_and(|origin| project_ids_match(origin, &current_project_id));
            let previous_status = task_store.get(&remote_task.id).ok().map(|task| task.status);
            let task_outcome = if web_close {
                reconcile_web_close(
                    task_store,
                    remote_task.clone(),
                    &task_sync_id,
                    "personal_pull",
                )
            } else {
                self.upsert_task_with_owner_preference(
                    task_store,
                    remote_task.clone(),
                    incoming_is_owner,
                    None,
                    &task_sync_id,
                    "personal_pull",
                )
            };
            match task_outcome {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_tasks += 1;
                    if let Some(revision) = task_revision {
                        let _ = self
                            .queue
                            .record_revision(EntityType::Task, &remote_task.id, revision);
                    }
                    if let Some(from) = previous_status.filter(|from| *from != remote_task.status) {
                        result.task_status_transitions.push(TaskStatusTransition {
                            task_id: remote_task.id,
                            project_id: current_project_id.to_string(),
                            source: "personal_pull".to_string(),
                            from,
                            to: remote_task.status,
                        });
                    }
                }
                Ok(UpsertResult::Skipped) => {
                    result.record_local_conflict();
                }
                Err(e) => {
                    result.errors.push(format!("Task error: {e}"));
                }
            }
        }

        // Dependencies are applied after tasks so a fresh pull can materialize
        // an edge whose endpoints arrived in the same envelope. A response
        // without this optional field predates dependency healing, so it is
        // left untouched until the endpoint supports the collection.
        if let Some(raw_dependencies) = body.task_dependencies {
            let report = self.apply_and_heal_task_dependencies(
                raw_dependencies,
                task_store,
                current_project_id,
                None,
                had_prior_watermark,
                &mut result,
            )?;
            Self::record_dependency_heal(&mut result, report);
        }

        // Process rules
        for raw_rule in body.rules.unwrap_or_default() {
            if !entity_matches_project(&raw_rule, &current_project_id, "rule") {
                continue;
            }
            let rule_revision = crate::cloud::wire_revision(&raw_rule);
            if let Some(id) = raw_rule.get("id").and_then(serde_json::Value::as_str) {
                self.note_incoming_revision(EntityType::Rule, id, &raw_rule);
            }
            let remote_rule: Rule = match deserialize_pulled_entity(raw_rule, "rule") {
                Ok(r) => r,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            let remote_rule_id = remote_rule.id.clone();
            match self.upsert_rule(rule_store, remote_rule) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_rules += 1;
                    if let Some(revision) = rule_revision {
                        let _ = self
                            .queue
                            .record_revision(EntityType::Rule, &remote_rule_id, revision);
                    }
                }
                Ok(UpsertResult::Skipped) => {
                    result.record_local_conflict();
                }
                Err(e) => {
                    result.errors.push(format!("Rule error: {e}"));
                }
            }
        }

        // Process skills
        for raw_skill in body.skills.unwrap_or_default() {
            if !entity_matches_project(&raw_skill, &current_project_id, "skill") {
                continue;
            }
            let skill_revision = crate::cloud::wire_revision(&raw_skill);
            if let Some(id) = raw_skill.get("id").and_then(serde_json::Value::as_str) {
                self.note_incoming_revision(EntityType::Skill, id, &raw_skill);
            }
            let remote_skill: Skill = match deserialize_pulled_entity(raw_skill, "skill") {
                Ok(s) => s,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            let remote_skill_id = remote_skill.id.clone();
            match self.upsert_skill(skill_store, remote_skill) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_skills += 1;
                    if let Some(revision) = skill_revision {
                        let _ = self
                            .queue
                            .record_revision(EntityType::Skill, &remote_skill_id, revision);
                    }
                }
                Ok(UpsertResult::Skipped) => {
                    result.record_local_conflict();
                }
                Err(e) => {
                    result.errors.push(format!("Skill error: {e}"));
                }
            }
        }

        // cas-bba4: process the 5 entity kinds the inline `cas cloud pull`
        // path used to import unscoped (cas-ed15 dropped them when collapsing
        // through CloudSyncer::pull). Each block mirrors the entries/tasks
        // shape: filter via `entity_matches_project` so foreign rows are
        // skipped, then delegate to a per-kind upsert helper. `specs` arrives
        // empty until cloud ships the matching pull-endpoint extension
        // (docs/requests/FEATURE-cloud-sync-pull-return-specs.md).

        // Process specs
        for raw_spec in body.specs.unwrap_or_default() {
            if !entity_matches_project(&raw_spec, &current_project_id, "spec") {
                continue;
            }
            let remote_spec: Spec = match deserialize_pulled_entity(raw_spec, "spec") {
                Ok(s) => s,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            match self.upsert_spec(spec_store, remote_spec) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_specs += 1;
                }
                Ok(UpsertResult::Skipped) => {
                    result.record_local_conflict();
                }
                Err(e) => {
                    result.errors.push(format!("Spec error: {e}"));
                }
            }
        }

        // Process events. EventStore is append-only (`record()` is straight
        // INSERT, no dedup); matches the pre-cas-ed15 inline path behavior.
        // The `since=` watermark on the request limits volume on incremental
        // pulls. `--full` re-imports duplicates — same as the prior path,
        // not a regression.
        for raw_event in body.events.unwrap_or_default() {
            if !entity_matches_project(&raw_event, &current_project_id, "event") {
                continue;
            }
            let remote_event: Event = match deserialize_pulled_entity(raw_event, "event") {
                Ok(e) => e,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            match event_store.record(&remote_event) {
                Ok(_) => result.pulled_events += 1,
                Err(e) => result.errors.push(format!("Event error: {e}")),
            }
        }

        // Process prompts. PromptStore exposes `add()` keyed by id; we
        // dedup-skip on existing rows (the previous inline path used
        // `let _ = prompt_store.add(&prompt)` and silently double-counted
        // on duplicate-key error — we instead surface real errors).
        for raw_prompt in body.prompts.unwrap_or_default() {
            if !entity_matches_project(&raw_prompt, &current_project_id, "prompt") {
                continue;
            }
            let remote_prompt: Prompt = match deserialize_pulled_entity(raw_prompt, "prompt") {
                Ok(p) => p,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            match prompt_store.get(&remote_prompt.id) {
                Ok(Some(_)) => {
                    result.record_local_conflict();
                }
                Ok(None) => match prompt_store.add(&remote_prompt) {
                    Ok(_) => result.pulled_prompts += 1,
                    Err(e) => result.errors.push(format!("Prompt error: {e}")),
                },
                Err(e) => result.errors.push(format!("Prompt lookup error: {e}")),
            }
        }

        // Process file changes (append-only, same shape as prompts).
        for raw_fc in body.file_changes.unwrap_or_default() {
            if !entity_matches_project(&raw_fc, &current_project_id, "file_change") {
                continue;
            }
            let remote_fc: FileChange = match deserialize_pulled_entity(raw_fc, "file_change") {
                Ok(f) => f,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            match file_change_store.get(&remote_fc.id) {
                Ok(Some(_)) => {
                    result.record_local_conflict();
                }
                Ok(None) => match file_change_store.add(&remote_fc) {
                    Ok(_) => result.pulled_file_changes += 1,
                    Err(e) => result.errors.push(format!("FileChange error: {e}")),
                },
                Err(e) => result.errors.push(format!("FileChange lookup error: {e}")),
            }
        }

        // Process commit links (keyed by `commit_hash`).
        for raw_cl in body.commit_links.unwrap_or_default() {
            if !entity_matches_project(&raw_cl, &current_project_id, "commit_link") {
                continue;
            }
            let remote_cl: CommitLink = match deserialize_pulled_entity(raw_cl, "commit_link") {
                Ok(c) => c,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            match commit_link_store.get(&remote_cl.commit_hash) {
                Ok(Some(_)) => {
                    result.record_local_conflict();
                }
                Ok(None) => match commit_link_store.add(&remote_cl) {
                    Ok(_) => result.pulled_commit_links += 1,
                    Err(e) => result.errors.push(format!("CommitLink error: {e}")),
                },
                Err(e) => result.errors.push(format!("CommitLink lookup error: {e}")),
            }
        }

        for conflict in self.take_conflict_log() {
            result.record_conflict_detail(conflict);
        }

        // An empty first pull can mean this new machine resolved the wrong
        // canonical project id. Stamping that response's watermark would make
        // a later corrected-id pull incremental and permanently skip the
        // historical backfill (GH #192). Once a project has established a
        // watermark, retain the existing behavior: healthy empty incremental
        // pulls advance to the server clock.
        // A locally-retained conflict (including a terminal-row rejection) is
        // still a successfully consumed, project-scoped server row. Advance
        // past it so an unattributed reopen is journaled once rather than
        // fetched and rejected forever. Foreign/malformed rows never reach
        // `conflicts_resolved`, so the GH #192 empty/wrong-bucket safeguard
        // remains intact.
        if (had_prior_watermark
            || result.total_pulled() > 0
            || result.conflicts_resolved > 0
            || result.healed_task_dependencies_to_cloud > 0)
            && let Some(pulled_at) = body.pulled_at
        {
            let _ = self.queue.set_metadata("last_pull_at", &pulled_at);
        }

        self.reassert_quarantine();

        result.duration_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }

    /// Re-assert the local quarantine after a pull has written rows
    /// (cas-4342 / GH #701).
    ///
    /// Hiding is computed from the ledger at read time, so a re-pulled row
    /// cannot resurface on the board no matter how often its content is
    /// rewritten — that part needs no work and is why the quarantine count
    /// stays flat across pulls. What *can* drift is the push side: any code
    /// path that enqueues on write would hand a quarantined row to the next
    /// push, and a quarantine decision must never leave this machine. So the
    /// invariant is enforced here rather than assumed, and what it removed is
    /// logged instead of being silently swallowed.
    ///
    /// Deliberately infallible: a pull that succeeded must not be reported as
    /// failed because a local suppression ledger could not be read.
    fn reassert_quarantine(&self) {
        let ids = match self.queue.quarantined_ids(crate::cloud::QUARANTINE_TASK) {
            Ok(ids) if !ids.is_empty() => ids,
            Ok(_) => return,
            Err(error) => {
                tracing::warn!(
                    "[Cassy sync] could not read the local quarantine ledger after pull ({error}); \
                     quarantined rows stay hidden but their push suppression was not re-checked"
                );
                return;
            }
        };

        let mut dropped = 0usize;
        for id in &ids {
            match self
                .queue
                .drop_queued_pushes_for(crate::cloud::QUARANTINE_TASK, id)
            {
                Ok(count) => dropped += count,
                Err(error) => tracing::warn!(
                    "[Cassy sync] could not clear queued pushes for quarantined row {id}: {error}"
                ),
            }
        }
        if dropped > 0 {
            tracing::info!(
                "[Cassy sync] dropped {dropped} queued push(es) for {} quarantined row(s) after pull",
                ids.len()
            );
        }
    }

    fn upsert_task(
        &self,
        store: &dyn TaskStore,
        task: Task,
        sync_id: &str,
        source: &str,
    ) -> Result<UpsertResult, CasError> {
        match store.get(&task.id) {
            Ok(local) => {
                if rejects_terminal_regression(&local, &task) {
                    self.record_terminal_regression_conflict(&local, &task)?;
                    return Ok(UpsertResult::Skipped);
                }
                if self.remote_supersedes_local(
                    EntityType::Task,
                    &task.id,
                    local.updated_at,
                    task.updated_at,
                ) {
                    let notes_differ = local.notes != task.notes;
                    self.journal_local_overwrite(
                        EntityType::Task,
                        &task.id,
                        &local,
                        if notes_differ { "merged" } else { "remote" },
                        if notes_differ {
                            "notes_union"
                        } else {
                            "timestamp_lww"
                        },
                    )?;
                    let mut merged = task;
                    if notes_differ {
                        merged.notes = merge_task_notes(&local.notes, &merged.notes);
                    }
                    append_sync_status_provenance(&mut merged, &local, sync_id, source);
                    store.update(&merged)?;
                    Ok(UpsertResult::Updated)
                } else {
                    Ok(UpsertResult::Skipped)
                }
            }
            Err(cas_store::StoreError::TaskNotFound(_)) => {
                store.add(&task)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    fn upsert_rule(&self, store: &dyn RuleStore, rule: Rule) -> Result<UpsertResult, CasError> {
        match store.get(&rule.id) {
            Ok(local) => {
                // Compare by last_accessed or created
                let local_time = local.last_accessed.unwrap_or(local.created);
                let remote_time = rule.last_accessed.unwrap_or(rule.created);

                if remote_time > local_time {
                    self.journal_local_overwrite(
                        EntityType::Rule,
                        &rule.id,
                        &local,
                        "remote",
                        "timestamp_lww",
                    )?;
                    store.update(&rule)?;
                    Ok(UpsertResult::Updated)
                } else {
                    Ok(UpsertResult::Skipped)
                }
            }
            Err(cas_store::StoreError::RuleNotFound(_)) => {
                store.add(&rule)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    fn upsert_skill(&self, store: &dyn SkillStore, skill: Skill) -> Result<UpsertResult, CasError> {
        match store.get(&skill.id) {
            Ok(local) => {
                if self.remote_supersedes_local(
                    EntityType::Skill,
                    &skill.id,
                    local.updated_at,
                    skill.updated_at,
                ) {
                    self.journal_local_overwrite(
                        EntityType::Skill,
                        &skill.id,
                        &local,
                        "remote",
                        "timestamp_lww",
                    )?;
                    store.update(&skill)?;
                    Ok(UpsertResult::Updated)
                } else {
                    Ok(UpsertResult::Skipped)
                }
            }
            // SqliteSkillStore::get reports a missing row as the generic
            // `NotFound`, while some store implementations use the older
            // skill-specific variant. Both mean a team member has not pulled
            // this shared skill yet and must take the insert path.
            Err(cas_store::StoreError::SkillNotFound(_) | cas_store::StoreError::NotFound(_)) => {
                store.add(&skill)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    fn upsert_spec(&self, store: &dyn SpecStore, spec: Spec) -> Result<UpsertResult, CasError> {
        // SpecStore::get returns `Result<Spec>` (not Option), with
        // `StoreError::NotFound` when absent — mirrors the task/skill shape.
        match store.get(&spec.id) {
            Ok(local) => {
                if spec.updated_at > local.updated_at {
                    store.update(&spec)?;
                    Ok(UpsertResult::Updated)
                } else {
                    Ok(UpsertResult::Skipped)
                }
            }
            Err(cas_store::StoreError::NotFound(_)) => {
                store.add(&spec)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Upsert a team entry by id using timestamp last-writer-wins.
    ///
    /// Team pulls intentionally use LWW for entries even though the other
    /// team entity kinds retain their configured conflict strategy. An entry
    /// can legitimately exist in both personal and team scope under the same
    /// id (GH #633); RemoteWins would overwrite a newer personal copy with an
    /// older team snapshot and would rewrite equal snapshots on every pull.
    fn upsert_entry_lww(
        &self,
        store: &dyn Store,
        entry: Entry,
        remote_updated_at: DateTime<Utc>,
    ) -> Result<UpsertResult, CasError> {
        let local = match store.get(&entry.id) {
            Ok(local) => Some(local),
            Err(cas_store::StoreError::EntryNotFound(_)) => match store.get_archived(&entry.id) {
                Ok(local) => Some(local),
                Err(cas_store::StoreError::EntryNotFound(_)) => None,
                Err(error) => return Err(error.into()),
            },
            Err(error) => return Err(error.into()),
        };

        let Some(local) = local else {
            return match store.add(&entry) {
                Ok(()) => Ok(UpsertResult::Created),
                // A concurrent local write can win the get→add race. Resolve
                // that row through the same LWW path instead of surfacing a
                // duplicate-key warning for an otherwise healthy pull.
                Err(cas_store::StoreError::EntryExists(_)) => {
                    let local = match store.get(&entry.id) {
                        Ok(local) => local,
                        Err(cas_store::StoreError::EntryNotFound(_)) => {
                            match store.get_archived(&entry.id) {
                                Ok(local) => local,
                                Err(error) => return Err(error.into()),
                            }
                        }
                        Err(error) => return Err(error.into()),
                    };
                    self.merge_entry_lww(store, local, entry, remote_updated_at)
                }
                Err(error) => Err(error.into()),
            };
        };

        self.merge_entry_lww(store, local, entry, remote_updated_at)
    }

    fn merge_entry_lww(
        &self,
        store: &dyn Store,
        local: Entry,
        remote: Entry,
        remote_updated_at: DateTime<Utc>,
    ) -> Result<UpsertResult, CasError> {
        let local_time = store.recent_timestamp(&local)?;
        let action = self.resolve_conflict(
            "entry",
            &remote.id,
            local_time,
            remote_updated_at,
            ConflictResolution::KeepRecent,
        );

        match action {
            ConflictAction::UseRemote => {
                self.journal_local_overwrite(
                    EntityType::Entry,
                    &remote.id,
                    &local,
                    "remote",
                    ConflictResolution::KeepRecent.as_str(),
                )?;
                store.update(&remote)?;
                Ok(UpsertResult::Updated)
            }
            ConflictAction::UseLocal | ConflictAction::Skip => Ok(UpsertResult::Skipped),
        }
    }

    /// Upsert task with configurable conflict resolution for team sync
    fn upsert_task_with_strategy(
        &self,
        store: &dyn TaskStore,
        task: Task,
        strategy: ConflictResolution,
        sync_id: &str,
        source: &str,
    ) -> Result<UpsertResult, CasError> {
        match store.get(&task.id) {
            Ok(local) => {
                if rejects_terminal_regression(&local, &task) {
                    self.record_terminal_regression_conflict(&local, &task)?;
                    return Ok(UpsertResult::Skipped);
                }
                let action = self.resolve_conflict(
                    "task",
                    &task.id,
                    local.updated_at,
                    task.updated_at,
                    strategy,
                );

                match action {
                    ConflictAction::UseRemote => {
                        let notes_differ = local.notes != task.notes;
                        self.journal_local_overwrite(
                            EntityType::Task,
                            &task.id,
                            &local,
                            if notes_differ { "merged" } else { "remote" },
                            if notes_differ {
                                "notes_union"
                            } else {
                                strategy.as_str()
                            },
                        )?;
                        let mut merged = task;
                        if notes_differ {
                            merged.notes = merge_task_notes(&local.notes, &merged.notes);
                        }
                        append_sync_status_provenance(&mut merged, &local, sync_id, source);
                        store.update(&merged)?;
                        Ok(UpsertResult::Updated)
                    }
                    ConflictAction::UseLocal | ConflictAction::Skip => Ok(UpsertResult::Skipped),
                }
            }
            Err(cas_store::StoreError::TaskNotFound(_)) => {
                store.add(&task)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Upsert rule with configurable conflict resolution for team sync
    fn upsert_rule_with_strategy(
        &self,
        store: &dyn RuleStore,
        rule: Rule,
        strategy: ConflictResolution,
    ) -> Result<UpsertResult, CasError> {
        match store.get(&rule.id) {
            Ok(local) => {
                let local_time = local.last_accessed.unwrap_or(local.created);
                let remote_time = rule.last_accessed.unwrap_or(rule.created);

                let action =
                    self.resolve_conflict("rule", &rule.id, local_time, remote_time, strategy);

                match action {
                    ConflictAction::UseRemote => {
                        self.journal_local_overwrite(
                            EntityType::Rule,
                            &rule.id,
                            &local,
                            "remote",
                            strategy.as_str(),
                        )?;
                        store.update(&rule)?;
                        Ok(UpsertResult::Updated)
                    }
                    ConflictAction::UseLocal | ConflictAction::Skip => Ok(UpsertResult::Skipped),
                }
            }
            Err(cas_store::StoreError::RuleNotFound(_)) => {
                store.add(&rule)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Upsert skill with configurable conflict resolution for team sync
    fn upsert_skill_with_strategy(
        &self,
        store: &dyn SkillStore,
        skill: Skill,
        strategy: ConflictResolution,
    ) -> Result<UpsertResult, CasError> {
        match store.get(&skill.id) {
            Ok(local) => {
                let action = self.resolve_conflict(
                    "skill",
                    &skill.id,
                    local.updated_at,
                    skill.updated_at,
                    strategy,
                );

                match action {
                    ConflictAction::UseRemote => {
                        self.journal_local_overwrite(
                            EntityType::Skill,
                            &skill.id,
                            &local,
                            "remote",
                            strategy.as_str(),
                        )?;
                        store.update(&skill)?;
                        Ok(UpsertResult::Updated)
                    }
                    ConflictAction::UseLocal | ConflictAction::Skip => Ok(UpsertResult::Skipped),
                }
            }
            // Keep team and personal pulls aligned: a fresh local store may
            // represent an absent skill with either missing-row variant.
            Err(cas_store::StoreError::SkillNotFound(_) | cas_store::StoreError::NotFound(_)) => {
                store.add(&skill)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Full sync: push then pull
    #[allow(clippy::too_many_arguments)]
    pub fn sync(
        &self,
        store: &dyn Store,
        task_store: &dyn TaskStore,
        rule_store: &dyn RuleStore,
        skill_store: &dyn SkillStore,
        spec_store: &dyn SpecStore,
        event_store: &dyn EventStore,
        prompt_store: &dyn PromptStore,
        file_change_store: &dyn FileChangeStore,
        commit_link_store: &dyn CommitLinkStore,
    ) -> Result<SyncResult, CasError> {
        self.sync_with_sessions(
            store,
            task_store,
            rule_store,
            skill_store,
            spec_store,
            event_store,
            prompt_store,
            file_change_store,
            commit_link_store,
            &[],
        )
    }

    /// Full sync with sessions: push (including sessions) then pull
    #[allow(clippy::too_many_arguments)]
    pub fn sync_with_sessions(
        &self,
        store: &dyn Store,
        task_store: &dyn TaskStore,
        rule_store: &dyn RuleStore,
        skill_store: &dyn SkillStore,
        spec_store: &dyn SpecStore,
        event_store: &dyn EventStore,
        prompt_store: &dyn PromptStore,
        file_change_store: &dyn FileChangeStore,
        commit_link_store: &dyn CommitLinkStore,
        sessions: &[Session],
    ) -> Result<SyncResult, CasError> {
        let start = Instant::now();

        // Push personal changes first (with sessions).
        let push_result = self.push_with_sessions(sessions)?;

        // Team-scoped writes are queued separately from personal writes. The
        // automatic daemon enters through this method, so omitting this drain
        // leaves every team row untouched (retry_count=0, last_error=NULL)
        // until somebody happens to run the manual CLI sync path.
        //
        // Keep team failure isolated from pull, matching `cas cloud sync`:
        // push_team records per-row HTTP failures, while setup/config errors
        // are surfaced in the aggregate result without suppressing pull.
        let team_push_result = match self.cloud_config.active_team_id() {
            Some(team_id) => self.push_team(&team_id).unwrap_or_else(|error| SyncResult {
                errors: vec![format!("Team push failed: {error}")],
                ..SyncResult::default()
            }),
            None => SyncResult::default(),
        };

        // Then pull
        let pull_result = self.pull(
            store,
            task_store,
            rule_store,
            skill_store,
            spec_store,
            event_store,
            prompt_store,
            file_change_store,
            commit_link_store,
        )?;

        // Combine results
        Ok(SyncResult {
            // Knowledge pages are synced by the dedicated
            // `push_knowledge_pages` / `pull_knowledge_pages` pair (they need
            // a KnowledgeStore this entry point does not take), so this
            // combined result reports zero for them rather than guessing.
            pushed_knowledge_pages: 0,
            pulled_knowledge_pages: 0,
            skipped_lww_acked: push_result.skipped_lww_acked + team_push_result.skipped_lww_acked,
            requeued_after_upgrade: push_result.requeued_after_upgrade
                + team_push_result.requeued_after_upgrade,
            pushed_entries: push_result.pushed_entries + team_push_result.pushed_entries,
            pushed_tasks: push_result.pushed_tasks + team_push_result.pushed_tasks,
            pushed_rules: push_result.pushed_rules + team_push_result.pushed_rules,
            pushed_skills: push_result.pushed_skills + team_push_result.pushed_skills,
            pushed_sessions: push_result.pushed_sessions + team_push_result.pushed_sessions,
            pushed_verifications: push_result.pushed_verifications
                + team_push_result.pushed_verifications,
            pushed_events: push_result.pushed_events + team_push_result.pushed_events,
            pushed_prompts: push_result.pushed_prompts + team_push_result.pushed_prompts,
            pushed_file_changes: push_result.pushed_file_changes
                + team_push_result.pushed_file_changes,
            pushed_commit_links: push_result.pushed_commit_links
                + team_push_result.pushed_commit_links,
            pushed_agents: push_result.pushed_agents + team_push_result.pushed_agents,
            pushed_worktrees: push_result.pushed_worktrees + team_push_result.pushed_worktrees,
            pushed_task_dependencies: push_result.pushed_task_dependencies
                + team_push_result.pushed_task_dependencies,
            pulled_entries: pull_result.pulled_entries,
            pulled_tasks: pull_result.pulled_tasks,
            pulled_rules: pull_result.pulled_rules,
            pulled_skills: pull_result.pulled_skills,
            pulled_specs: pull_result.pulled_specs,
            pulled_events: pull_result.pulled_events,
            pulled_prompts: pull_result.pulled_prompts,
            pulled_file_changes: pull_result.pulled_file_changes,
            pulled_commit_links: pull_result.pulled_commit_links,
            pulled_task_dependencies: pull_result.pulled_task_dependencies,
            deleted_task_dependencies: pull_result.deleted_task_dependencies,
            skipped_task_dependencies_by_tombstone: pull_result
                .skipped_task_dependencies_by_tombstone,
            healed_task_dependencies_to_cloud: pull_result.healed_task_dependencies_to_cloud,
            healed_task_dependencies_from_cloud: pull_result.healed_task_dependencies_from_cloud,
            task_status_transitions: pull_result.task_status_transitions,
            conflicts_resolved: pull_result.conflicts_resolved,
            conflicts_resolved_local: pull_result.conflicts_resolved_local,
            conflicts_resolved_remote: pull_result.conflicts_resolved_remote,
            conflicts: pull_result.conflicts,
            errors: [
                push_result.errors,
                team_push_result.errors,
                pull_result.errors,
            ]
            .concat(),
            batches_run: push_result.batches_run + team_push_result.batches_run,
            remaining_backlog: push_result.remaining_backlog,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Pull team data from cloud and merge into local store.
    ///
    /// `project_id` is the canonical project ID for the current scope
    /// (typically `cas::cloud::get_project_canonical_id()` at the caller
    /// site). Taking it as a parameter (rather than resolving inside the
    /// function) keeps the watermark scope explicit AND avoids the
    /// process-wide cache in `get_project_canonical_id` which would make
    /// it impossible to exercise the cross-project watermark behavior
    /// in a single test process. The value is used for:
    /// - The `last_team_pull_at_{team_id}_{project_id}` metadata key
    ///   (cas-53d5 — per-(team, project) watermark scoping, fixes the
    ///   "second project sees stale `since=` from the first" regression
    ///   that surfaced as hypothesis #2 of the cas-ffc4 bug doc).
    /// - The `project_id=` URL query param.
    /// - The client-side `entity_matches_project` filter for non-task rows.
    ///   Task rows are accepted only when their explicit origin belongs to the
    ///   requested project. A null origin may use a server-attested project
    ///   field, but never the requesting scope as an implicit owner.
    pub fn pull_team(
        &self,
        team_id: &str,
        project_id: &str,
        store: &dyn Store,
        task_store: &dyn TaskStore,
        rule_store: &dyn RuleStore,
        skill_store: &dyn SkillStore,
    ) -> Result<SyncResult, CasError> {
        self.clear_conflict_log();
        self.clear_incoming_revisions();
        let mut result = SyncResult::default();
        let start = Instant::now();

        if !self.is_available() {
            return Ok(result);
        }

        let token = self
            .cloud_config
            .token
            .as_ref()
            .ok_or_else(|| CasError::Other("Not logged in".to_string()))?;

        // Get last pull timestamp for this (team_id, project_id) scope.
        // cas-53d5: re-keyed from the old `last_team_pull_at_{team_id}`.
        // Absence of the new-format key is treated as "first sync into
        // this scope" — we send no `since=`, triggering a full backfill.
        // This is the bug fix: previously the global-per-team watermark
        // leaked across projects, causing the second project to skip its
        // historical backfill.
        let since_key = format!("last_team_pull_at_{team_id}_{project_id}");
        let since = self.queue.get_metadata(&since_key)?;

        let mut pull_url = format!(
            "{}/api/teams/{}/sync/pull",
            self.cloud_config.endpoint, team_id
        );
        let mut params = Vec::new();
        if let Some(since) = &since {
            params.push(format!("since={since}"));
        }
        params.push(format!("project_id={}", project_id.replace('/', "%2F")));
        if !params.is_empty() {
            pull_url = format!("{pull_url}?{}", params.join("&"));
        }

        let response = ureq::get(&pull_url)
            .timeout(self.config.timeout)
            .set("Authorization", &format!("Bearer {token}"))
            .call();

        let body: TeamPullResponse = match response {
            Ok(resp) => resp
                .into_json()
                .map_err(|e| CasError::Other(format!("Failed to parse team pull response: {e}")))?,
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                return Err(CasError::Other(format!(
                    "Team pull failed with status {code}: {body}"
                )));
            }
            Err(ureq::Error::Transport(e)) => {
                return Err(CasError::Other(format!("Network error: {e}")));
            }
        };

        // Use configured conflict resolution strategy for team sync
        let strategy = self.config.team_conflict_resolution;
        tracing::debug!("[Cassy sync] Starting team pull: team={team_id} strategy={strategy:?}");

        // Use the caller-supplied project ID for client-side validation.
        // (cas-53d5: previously resolved internally via
        // `get_project_canonical_id`; now passed in as a function
        // parameter so the watermark key, URL param, and entity-filter
        // all agree on a single explicit scope.)
        let current_project_id = project_id;

        // Process entries
        for raw_entry in body.entries.unwrap_or_default() {
            if !entity_matches_project(&raw_entry, &current_project_id, "entry") {
                continue;
            }
            let remote_updated_at = pulled_entry_updated_at(&raw_entry);
            let entry_revision = crate::cloud::wire_revision(&raw_entry);
            let entry_revision_id = raw_entry
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            if let Some(id) = &entry_revision_id {
                self.note_incoming_revision(EntityType::Entry, id, &raw_entry);
            }
            let remote_entry: Entry = match deserialize_pulled_entity(raw_entry, "entry") {
                Ok(e) => e,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            let remote_updated_at = remote_updated_at
                .unwrap_or_else(|| remote_entry.last_accessed.unwrap_or(remote_entry.created));
            match self.upsert_entry_lww(store, remote_entry, remote_updated_at) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_entries += 1;
                    if let (Some(id), Some(revision)) = (&entry_revision_id, entry_revision) {
                        let _ = self.queue.record_revision(EntityType::Entry, id, revision);
                    }
                }
                Ok(UpsertResult::Skipped) => {
                    result.record_local_conflict();
                }
                Err(e) => {
                    result.errors.push(format!("Entry error: {e}"));
                }
            }
        }

        // Process tasks
        let task_sync_id = uuid::Uuid::new_v4().to_string();
        // Group duplicate IDs before filtering so an owner row can win over a
        // stale foreign-keyed replica, then apply the same origin-first
        // ownership rule doctor uses. Foreign rows are parked at ingest and
        // never become local contamination.
        let (raw_tasks, discarded_task_rows) =
            select_owner_task_rows(body.tasks.unwrap_or_default());
        for (task_id, discarded_row) in discarded_task_rows {
            self.record_owner_conflict_value(&task_id, &discarded_row)?;
            result.record_local_conflict();
        }
        for raw_task in raw_tasks {
            if !entity_matches_project(&raw_task, current_project_id, "task") {
                continue;
            }
            let server_owner = task_wire_cloud_project(&raw_task).map(str::to_owned);
            let mut raw_task = raw_task;
            render_task_proposal_provenance(&mut raw_task);
            let wire_is_owner = task_wire_is_owner(&raw_task);
            let team_task_revision = crate::cloud::wire_revision(&raw_task);
            if let Some(id) = raw_task.get("id").and_then(serde_json::Value::as_str) {
                self.note_incoming_revision(EntityType::Task, id, &raw_task);
            }
            let mut remote_task: Task = match deserialize_pulled_entity(raw_task, "task") {
                Ok(t) => t,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            // Preserve explicit origin. For legacy rows without an origin,
            // only a server-attested project is safe to persist; an unscoped
            // row was already rejected by entity_matches_project above.
            if remote_task.origin_project.is_none() {
                let Some(server_owner) = server_owner else {
                    record_project_warning(
                        "task",
                        "<missing>",
                        &format!(
                            "parking task '{}' — no origin_project or server project identity",
                            remote_task.id
                        ),
                    );
                    continue;
                };
                remote_task.origin_project = Some(server_owner);
            }
            let incoming_is_owner = wire_is_owner
                || remote_task
                    .origin_project
                    .as_deref()
                    .is_some_and(|origin| project_ids_match(origin, current_project_id));
            let previous_status = task_store.get(&remote_task.id).ok().map(|task| task.status);
            match self.upsert_task_with_owner_preference(
                task_store,
                remote_task.clone(),
                incoming_is_owner,
                Some(strategy),
                &task_sync_id,
                "team_pull",
            ) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_tasks += 1;
                    if let Some(revision) = team_task_revision {
                        let _ = self
                            .queue
                            .record_revision(EntityType::Task, &remote_task.id, revision);
                    }
                    if let Some(from) = previous_status.filter(|from| *from != remote_task.status) {
                        result.task_status_transitions.push(TaskStatusTransition {
                            task_id: remote_task.id,
                            project_id: current_project_id.to_string(),
                            source: "team_pull".to_string(),
                            from,
                            to: remote_task.status,
                        });
                    }
                }
                Ok(UpsertResult::Skipped) => {
                    result.record_local_conflict();
                }
                Err(e) => {
                    result.errors.push(format!("Task error: {e}"));
                }
            }
        }

        // Dependencies are applied after tasks; missing endpoints are parked
        // with a warning instead of creating dangling local rows. A response
        // without this optional field predates dependency healing, so it is
        // left untouched until the endpoint supports the collection.
        if let Some(raw_dependencies) = body.task_dependencies {
            let report = self.apply_and_heal_task_dependencies(
                raw_dependencies,
                task_store,
                current_project_id,
                Some(team_id),
                since.is_some(),
                &mut result,
            )?;
            Self::record_dependency_heal(&mut result, report);
        }

        // Process rules
        for raw_rule in body.rules.unwrap_or_default() {
            if !entity_matches_project(&raw_rule, &current_project_id, "rule") {
                continue;
            }
            let rule_revision = crate::cloud::wire_revision(&raw_rule);
            if let Some(id) = raw_rule.get("id").and_then(serde_json::Value::as_str) {
                self.note_incoming_revision(EntityType::Rule, id, &raw_rule);
            }
            let remote_rule: Rule = match deserialize_pulled_entity(raw_rule, "rule") {
                Ok(r) => r,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            let remote_rule_id = remote_rule.id.clone();
            match self.upsert_rule_with_strategy(rule_store, remote_rule, strategy) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_rules += 1;
                    if let Some(revision) = rule_revision {
                        let _ = self
                            .queue
                            .record_revision(EntityType::Rule, &remote_rule_id, revision);
                    }
                }
                Ok(UpsertResult::Skipped) => {
                    result.record_local_conflict();
                }
                Err(e) => {
                    result.errors.push(format!("Rule error: {e}"));
                }
            }
        }

        // Process skills
        for raw_skill in body.skills.unwrap_or_default() {
            if !entity_matches_project(&raw_skill, &current_project_id, "skill") {
                continue;
            }
            let skill_revision = crate::cloud::wire_revision(&raw_skill);
            if let Some(id) = raw_skill.get("id").and_then(serde_json::Value::as_str) {
                self.note_incoming_revision(EntityType::Skill, id, &raw_skill);
            }
            let remote_skill: Skill = match deserialize_pulled_entity(raw_skill, "skill") {
                Ok(s) => s,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            let remote_skill_id = remote_skill.id.clone();
            match self.upsert_skill_with_strategy(skill_store, remote_skill, strategy) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_skills += 1;
                    if let Some(revision) = skill_revision {
                        let _ = self
                            .queue
                            .record_revision(EntityType::Skill, &remote_skill_id, revision);
                    }
                }
                Ok(UpsertResult::Skipped) => {
                    result.record_local_conflict();
                }
                Err(e) => {
                    result.errors.push(format!("Skill error: {e}"));
                }
            }
        }

        // Update team pull timestamp under the new per-(team, project)
        // key. On successful write, best-effort retire the legacy
        // `last_team_pull_at_{team_id}` global-per-team key — once the
        // new-format key exists for any project under this team, the
        // legacy key is dead metadata that would otherwise sit forever.
        // Best-effort: a delete failure here cannot regress the pull
        // result, so we swallow the error.
        if let Some(pulled_at) = body.pulled_at {
            let _ = self.queue.set_metadata(&since_key, &pulled_at);
            let legacy_key = format!("last_team_pull_at_{team_id}");
            let _ = self.queue.delete_metadata(&legacy_key);
        }

        for conflict in self.take_conflict_log() {
            result.record_conflict_detail(conflict);
        }
        result.duration_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PROPOSAL_PROVENANCE_BEGIN, PROPOSAL_PROVENANCE_END, PULL_PATH, SyncWarningSummary,
        build_scoped_pull_url_with, collect_sync_warnings, deserialize_pulled_entity,
        entity_matches_project, remote_dependency_state, render_task_proposal_provenance,
        task_dependency_matches_project,
    };
    use crate::types::{Entry, Task};
    use serde_json::json;

    #[test]
    fn deserialize_error_names_the_wire_entity_type_and_id() {
        let error = deserialize_pulled_entity::<Entry>(
            json!({
                "id": "p-malformed-created",
                "project_id": "cas-src",
                "content": "legacy entry without its creation timestamp",
            }),
            "entry",
        )
        .unwrap_err();

        assert!(error.contains("entry deserialize error"), "{error}");
        assert!(error.contains("id=p-malformed-created"), "{error}");
        assert!(error.contains("missing field `created`"), "{error}");
    }

    #[test]
    fn deserialize_migrates_stringified_task_deliverables() {
        let mut raw = serde_json::to_value(Task::new(
            "cas-legacy-deliverables".to_string(),
            "Moved task".to_string(),
        ))
        .unwrap();
        raw["project_id"] = json!("cas-src");
        raw["deliverables"] =
            json!("{\"files_changed\":[\"src/lib.rs\"],\"factory_branch_anchor\":\"deadbeef\"}");

        let task = deserialize_pulled_entity::<Task>(raw, "task")
            .expect("legacy task payload should be readable");
        assert_eq!(task.deliverables.files_changed, ["src/lib.rs"]);
        assert_eq!(
            task.deliverables.factory_branch_anchor.as_deref(),
            Some("deadbeef")
        );
    }

    #[test]
    fn materialized_task_renders_attested_and_asserted_provenance_visibly() {
        let mut raw = json!({
            "notes": "Receiver notes",
            "proposal_provenance": {
                "server_attested": {
                    "proposal_id": "proposal-1",
                    "target_task_id": "cas-0123456789abcdef",
                    "creator_user_id": "user-1",
                    "team_id": "team-1",
                    "origin_project_canonical_id": "origin",
                    "target_project_canonical_id": "target",
                    "received_at": "2026-08-13T12:00:00Z",
                    "client_request_id": "request-1"
                },
                "client_asserted": {
                    "origin_session_id": "session-1",
                    "origin_agent_id": "agent-1",
                    "origin_agent_name": "supervisor",
                    "origin_agent_role": "supervisor",
                    "client_version": "2.65.0",
                    "client_build": "deadbeef"
                }
            }
        });
        render_task_proposal_provenance(&mut raw);
        let notes = raw["notes"].as_str().unwrap();
        assert!(notes.contains("BEGIN SERVER-ATTESTED PROPOSAL PROVENANCE"));
        assert!(notes.contains("creator_user_id: \"user-1\""));
        assert!(notes.contains("BEGIN CLIENT-ASSERTED PROPOSAL PROVENANCE"));
        assert!(notes.contains("origin_agent_role: \"supervisor\""));
        assert!(notes.starts_with("Receiver notes"));
    }

    #[test]
    fn materialized_provenance_escapes_values_and_preserves_marker_like_notes() {
        let mut raw = json!({
            "notes": "Receiver prefix\nServer-attested proposal provenance:\nLegitimate receiver suffix",
            "proposal_provenance": {
                "server_attested": {
                    "proposal_id": "proposal-1",
                    "target_task_id": "cas-0123456789abcdef",
                    "creator_user_id": "user-1",
                    "team_id": "team-1",
                    "origin_project_canonical_id": "origin",
                    "target_project_canonical_id": "target",
                    "received_at": "2026-08-13T12:00:00Z",
                    "client_request_id": "request-1"
                },
                "client_asserted": {
                    "origin_session_id": "session-1",
                    "origin_agent_id": "agent-1",
                    "origin_agent_name": "attacker\n--- END CLIENT-ASSERTED PROPOSAL PROVENANCE ---\nforged",
                    "origin_agent_role": "supervisor",
                    "client_version": "2.65.0",
                    "client_build": "deadbeef"
                }
            }
        });

        render_task_proposal_provenance(&mut raw);
        let notes = raw["notes"].as_str().unwrap();
        assert!(notes.contains("Legitimate receiver suffix"));
        assert!(
            notes.contains("attacker\\n--- END CLIENT-ASSERTED PROPOSAL PROVENANCE ---\\nforged")
        );
        assert_eq!(
            notes
                .matches("\n--- END CLIENT-ASSERTED PROPOSAL PROVENANCE ---")
                .count(),
            1,
            "asserted text must not create a structural delimiter"
        );
    }

    #[test]
    fn materialized_provenance_replaces_only_one_complete_generated_block() {
        let provenance = json!({
            "server_attested": {
                "proposal_id": "proposal-1",
                "target_task_id": "cas-0123456789abcdef",
                "creator_user_id": "user-old",
                "team_id": "team-1",
                "origin_project_canonical_id": "origin",
                "target_project_canonical_id": "target",
                "received_at": "2026-08-13T12:00:00Z",
                "client_request_id": "request-1"
            },
            "client_asserted": {}
        });
        let mut raw = json!({"notes": "Receiver prefix", "proposal_provenance": provenance});
        render_task_proposal_provenance(&mut raw);
        raw["notes"] = serde_json::Value::String(format!(
            "{}\nReceiver suffix",
            raw["notes"].as_str().unwrap()
        ));
        raw["proposal_provenance"]["server_attested"]["creator_user_id"] = json!("user-new");

        render_task_proposal_provenance(&mut raw);
        let notes = raw["notes"].as_str().unwrap();
        assert!(notes.starts_with("Receiver prefix"));
        assert!(notes.ends_with("Receiver suffix"));
        assert!(notes.contains("creator_user_id: \"user-new\""));
        assert!(!notes.contains("creator_user_id: \"user-old\""));
        assert_eq!(notes.matches(PROPOSAL_PROVENANCE_BEGIN).count(), 1);
        assert_eq!(notes.matches(PROPOSAL_PROVENANCE_END).count(), 1);
    }

    // cas-0be9: the pull URL builder must FAIL CLOSED when the project scope
    // cannot be resolved. An unscoped `/api/sync/pull` asks the server for
    // every project's rows — the cas-2eb3 contamination vector.
    #[test]
    fn pull_url_aborts_when_project_scope_is_unresolvable() {
        let err = build_scoped_pull_url_with("https://cloud.example", &[], || None)
            .expect_err("an unresolvable project scope must abort the pull, not drop the scope");
        let message = err.to_string();
        assert!(
            message.contains("not inside a CAS project directory"),
            "unexpected error message: {message}"
        );
    }

    #[test]
    fn pull_url_is_scoped_and_carries_extra_params() {
        let (url, project_id) = build_scoped_pull_url_with(
            "https://cloud.example",
            &["since=2026-01-01T00:00:00Z".to_string()],
            || Some("github.com/owner/repo".to_string()),
        )
        .expect("a resolvable project scope builds a URL");
        assert_eq!(project_id, "github.com/owner/repo");
        assert_eq!(
            url,
            format!(
                "https://cloud.example{PULL_PATH}?since=2026-01-01T00:00:00Z\
                 &project_id=github.com%2Fowner%2Frepo"
            )
        );
    }

    /// GH #701, the measured shape. The cloud stamps `project_id` with the
    /// scope you asked for, so a row replicated into this project's bucket
    /// looks native by that field alone. Its `origin_project` still names the
    /// project that wrote it, and that is what attribution must read.
    #[test]
    fn a_replicated_row_is_refused_on_its_origin_despite_a_native_scope_stamp() {
        // Verbatim shape of a row from
        // GET /api/sync/pull?project_id=richards-llc-accounting (2026-09-03):
        // 3,002 of these came back, every one stamped with the requested scope.
        let edge = json!({
            "id": "cas-0074:cas-3648:parent-child",
            "from_id": "cas-0074",
            "to_id": "cas-3648",
            "dep_type": "parent-child",
            "origin_project": "cas-src",
            "project_id": "richards-llc-accounting",
            "team_id": null
        });

        assert!(
            !task_dependency_matches_project(&edge, "richards-llc-accounting"),
            "a cas-src edge replicated into the accounting scope must not be ingested"
        );
        assert!(
            task_dependency_matches_project(&edge, "cas-src"),
            "the same edge is native when cas-src pulls it"
        );

        let task = json!({
            "id": "cas-1234",
            "title": "a cas-src task",
            "origin_project": "cas-src",
            "project_id": "gabber-studio"
        });
        assert!(!entity_matches_project(&task, "gabber-studio", "task"));
        assert!(entity_matches_project(&task, "cas-src", "task"));
    }

    /// The fallback is load-bearing: rows written before `origin_project`
    /// existed carry no origin, and rejecting those would drop real history
    /// rather than fix the leak.
    #[test]
    fn a_row_without_an_origin_still_falls_back_to_the_scope_stamp() {
        let legacy = json!({ "id": "e-001", "project_id": "gabber-studio" });
        assert!(entity_matches_project(&legacy, "gabber-studio", "entry"));

        let legacy_edge = json!({
            "id": "a:b:parent-child",
            "project_id": "gabber-studio"
        });
        assert!(task_dependency_matches_project(
            &legacy_edge,
            "gabber-studio"
        ));

        // An empty or whitespace origin is not an assertion of ownership.
        let blank = json!({
            "id": "e-002",
            "origin_project": "   ",
            "project_id": "gabber-studio"
        });
        assert!(entity_matches_project(&blank, "gabber-studio", "entry"));
    }

    /// Attribution runs through the same alias-aware predicate as everything
    /// else (GH #669), so a legacy spelling of *this* project is still native.
    #[test]
    fn an_origin_spelled_as_a_known_alias_of_this_project_is_still_native() {
        let row = json!({
            "id": "t-1",
            "origin_project": "git@GitHub.com:Richards-LLC/gabber-studio.git",
            "project_id": "gabber-studio"
        });
        assert!(entity_matches_project(&row, "gabber-studio", "task"));
    }

    /// The healer reads the same attribution as the guard, or it would
    /// resurrect edges the guard just refused.
    #[test]
    fn the_dependency_healer_ignores_rows_the_ingest_guard_refuses() {
        let raw = vec![
            json!({
                "id": "a:b:parent-child", "from_id": "a", "to_id": "b",
                "dep_type": "parent-child", "created_at": "2026-01-01T00:00:00Z",
                "origin_project": "cas-src", "project_id": "gabber-studio"
            }),
            json!({
                "id": "c:d:parent-child", "from_id": "c", "to_id": "d",
                "dep_type": "parent-child", "created_at": "2026-01-01T00:00:00Z",
                "origin_project": "gabber-studio", "project_id": "gabber-studio"
            }),
        ];

        let state = remote_dependency_state(&raw, "gabber-studio");

        assert_eq!(state.live.len(), 1, "only the native edge survives");
        assert!(state.live.contains_key("c:d:parent-child"));
    }

    #[test]
    fn test_entity_matches_project_no_field() {
        // No project field — rejected now that cloud always echoes project_id (cas-6479)
        let entity = json!({ "id": "e-001", "content": "hello" });
        assert!(!entity_matches_project(
            &entity,
            "github.com/owner/repo",
            "entry"
        ));
    }

    #[test]
    fn test_entity_matches_project_null_field() {
        // Null project_canonical_id — rejected; cloud must scope all entities (cas-6479)
        let entity = json!({ "id": "e-001", "project_canonical_id": null });
        assert!(!entity_matches_project(
            &entity,
            "github.com/owner/repo",
            "entry"
        ));
    }

    #[test]
    fn test_entity_matches_project_matching() {
        // Matching project — accepted
        let entity = json!({ "id": "e-001", "project_canonical_id": "github.com/owner/repo" });
        assert!(entity_matches_project(
            &entity,
            "github.com/owner/repo",
            "entry"
        ));
    }

    #[test]
    fn test_entity_matches_project_foreign() {
        // Different project — rejected (returns false)
        let entity = json!({ "id": "e-001", "project_canonical_id": "github.com/other/repo" });
        assert!(!entity_matches_project(
            &entity,
            "github.com/owner/repo",
            "entry"
        ));
    }

    #[test]
    fn test_entity_matches_project_id_field_alias() {
        // Also checks `project_id` field as an alias
        let entity = json!({ "id": "t-abc", "project_id": "github.com/owner/repo" });
        assert!(entity_matches_project(
            &entity,
            "github.com/owner/repo",
            "task"
        ));
    }

    #[test]
    fn test_entity_matches_project_id_field_foreign() {
        let entity = json!({ "id": "t-abc", "project_id": "github.com/other/repo" });
        assert!(!entity_matches_project(
            &entity,
            "github.com/owner/repo",
            "task"
        ));
    }

    #[test]
    fn collect_sync_warnings_groups_rejected_rows() {
        let (matched, warnings) = collect_sync_warnings(|| {
            vec![
                entity_matches_project(
                    &json!({ "project_id": "other" }),
                    "current",
                    "knowledge_page",
                ),
                entity_matches_project(
                    &json!({ "project_id": "other" }),
                    "current",
                    "knowledge_page",
                ),
                entity_matches_project(&json!({ "project_id": "other" }), "current", "task"),
            ]
        });

        assert_eq!(matched, vec![false, false, false]);
        assert_eq!(
            warnings,
            vec![
                SyncWarningSummary {
                    entity_kind: "knowledge_page".to_string(),
                    project: "other".to_string(),
                    count: 2,
                },
                SyncWarningSummary {
                    entity_kind: "task".to_string(),
                    project: "other".to_string(),
                    count: 1,
                },
            ]
        );
    }

    #[test]
    fn test_entity_matches_project_null_project_id() {
        // Null project_id — rejected; cloud must scope all entities (cas-6479)
        let entity = json!({ "id": "t-abc", "project_id": null });
        assert!(!entity_matches_project(
            &entity,
            "github.com/owner/repo",
            "task"
        ));
    }

    #[test]
    fn test_entity_matches_local_project() {
        // local: prefix IDs work the same way
        let entity = json!({ "id": "p-001", "project_canonical_id": "local:abcd1234ef567890" });
        assert!(entity_matches_project(
            &entity,
            "local:abcd1234ef567890",
            "entry"
        ));
        assert!(!entity_matches_project(
            &entity,
            "local:0000000000000000",
            "entry"
        ));
    }

    #[test]
    fn alias_project_row_is_owned_after_normalization() {
        let entity = json!({
            "id": "alias-row",
            "project_canonical_id": "git@GitHub.com:Richards-LLC/gabber-studio.git"
        });

        assert!(entity_matches_project(&entity, "gabber-studio", "task"));
    }

    #[test]
    fn canonical_identity_pull_accepts_remote_alias_spellings() {
        let entity = json!({ "id": "t-1", "project_canonical_id": "github.com/Acme/Ledger" });

        assert!(entity_matches_project(
            &entity,
            "github.com/Acme/Ledger",
            "task"
        ));
        for variant in [
            "github.com/acme/ledger",
            "github.com/ACME/LEDGER",
            "github.com/Acme/ledger",
            " https://github.com/Acme/Ledger.git/ ",
        ] {
            assert!(
                entity_matches_project(&entity, variant, "task"),
                "alias '{variant}' must match the canonical project"
            );
        }
        assert!(!entity_matches_project(
            &entity,
            "github.com/other/Ledger",
            "task"
        ));
    }

    #[test]
    fn canonical_identity_pull_accepts_bare_alias_for_remote_scope_dependency() {
        let dependency = json!({
            "id": "alias-edge",
            "project_id": "gabber-studio",
        });

        assert!(task_dependency_matches_project(
            &dependency,
            "github.com/richards-llc/gabber-studio"
        ));
    }
}

// cas-fc52: web-initiated close reconcile (cloud contract §4)
#[cfg(test)]
mod web_close_tests {
    use super::{CloudSyncer, is_web_close_tombstone, merge_task_notes, reconcile_web_close};
    use crate::cloud::syncer::{CloudSyncerConfig, UpsertResult};
    use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
    use crate::store::{init_cas_dir, open_task_store};
    use crate::types::{Task, TaskStatus};
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn closed_tombstone(id: &str) -> Task {
        let mut t = Task::new(id.to_string(), format!("title {id}"));
        t.status = TaskStatus::Closed;
        t.close_reason = Some("closed from web by teammate".to_string());
        t.closed_at = Some(chrono::Utc::now());
        t
    }

    #[test]
    fn detects_web_close_marker() {
        assert!(is_web_close_tombstone(
            &json!({ "id": "t1", "closed_via": "web" })
        ));
        // Our own pushed closes never carry closed_via == "web".
        assert!(!is_web_close_tombstone(
            &json!({ "id": "t1", "status": "closed" })
        ));
        assert!(!is_web_close_tombstone(
            &json!({ "id": "t1", "closed_via": "cli" })
        ));
        assert!(!is_web_close_tombstone(
            &json!({ "id": "t1", "closed_via": null })
        ));
    }

    #[test]
    fn reconcile_forces_close_on_open_task_even_if_local_is_newer() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();

        // Local in-progress task, assigned, with a NEWER updated_at than the
        // tombstone — a timestamp-gated upsert would skip it; the web close
        // must apply regardless.
        let mut local = Task::new("t-web".to_string(), "title".to_string());
        local.assignee = Some("agent-x".to_string());
        local.status = TaskStatus::InProgress;
        local.updated_at = chrono::Utc::now() + chrono::Duration::hours(1);
        store.add(&local).unwrap();

        let outcome = reconcile_web_close(
            &*store,
            closed_tombstone("t-web"),
            "sync-web",
            "personal_pull",
        )
        .unwrap();
        assert!(matches!(outcome, UpsertResult::Updated));

        let got = store.get("t-web").unwrap();
        assert_eq!(got.status, TaskStatus::Closed);
        assert_eq!(
            got.close_reason.as_deref(),
            Some("closed from web by teammate")
        );
        assert!(
            got.assignee.is_none(),
            "assignee must be cleared on web close"
        );
    }

    #[test]
    fn reconcile_preserves_locally_authored_content() {
        // P1 (cas-71f7 review): a web close must NOT clobber local-only,
        // not-yet-pushed body content. Only the close signal is applied.
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();

        let mut local = Task::new("t-content".to_string(), "title".to_string());
        local.status = TaskStatus::InProgress;
        local.description = "local description not yet pushed".to_string();
        local.notes = "local working notes".to_string();
        local.updated_at = chrono::Utc::now() + chrono::Duration::hours(1);
        store.add(&local).unwrap();

        // Tombstone carries empty body fields (server snapshot) + the close.
        let outcome = reconcile_web_close(
            &*store,
            closed_tombstone("t-content"),
            "sync-web",
            "personal_pull",
        )
        .unwrap();
        assert!(matches!(outcome, UpsertResult::Updated));

        let got = store.get("t-content").unwrap();
        assert_eq!(got.status, TaskStatus::Closed);
        assert_eq!(
            got.close_reason.as_deref(),
            Some("closed from web by teammate")
        );
        // Local-authored content survives the close.
        assert_eq!(got.description, "local description not yet pushed");
        assert!(got.notes.starts_with("local working notes"));
        assert!(got.notes.contains("[CAS_SYNC_STATUS]"));
        assert!(got.assignee.is_none());
    }

    #[test]
    fn reconcile_is_idempotent_on_already_closed() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();

        let mut local = Task::new("t-done".to_string(), "title".to_string());
        local.status = TaskStatus::Closed;
        local.close_reason = Some("already closed locally".to_string());
        store.add(&local).unwrap();

        let outcome = reconcile_web_close(
            &*store,
            closed_tombstone("t-done"),
            "sync-web",
            "personal_pull",
        )
        .unwrap();
        assert!(matches!(outcome, UpsertResult::Skipped));
        // The no-op must not clobber the pre-existing local close_reason.
        let got = store.get("t-done").unwrap();
        assert_eq!(got.close_reason.as_deref(), Some("already closed locally"));
    }

    #[test]
    fn reconcile_adds_unknown_closed_task() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();

        let outcome = reconcile_web_close(
            &*store,
            closed_tombstone("t-new"),
            "sync-web",
            "personal_pull",
        )
        .unwrap();
        assert!(matches!(outcome, UpsertResult::Created));
        assert_eq!(store.get("t-new").unwrap().status, TaskStatus::Closed);
    }

    #[test]
    fn task_note_union_is_timestamp_ordered_and_deduplicated() {
        let local = "[2026-08-09 10:30] 📝 PROGRESS local follow-up\n\n[2026-08-09 11:30] 📝 PROGRESS shared";
        let remote =
            "[2026-08-09 09:30] 📝 PROGRESS remote first\n\n[2026-08-09 11:30] 📝 PROGRESS shared";

        assert_eq!(
            merge_task_notes(local, remote),
            "[2026-08-09 09:30] 📝 PROGRESS remote first\n\n[2026-08-09 10:30] 📝 PROGRESS local follow-up\n\n[2026-08-09 11:30] 📝 PROGRESS shared"
        );
    }

    #[test]
    fn local_task_conflict_unions_notes_and_journals_the_losing_row() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();
        let queue = Arc::new(SyncQueue::open(&cas_dir).unwrap());
        queue.init().unwrap();
        let syncer = CloudSyncer::new(
            queue.clone(),
            CloudConfig::default(),
            CloudSyncerConfig::default(),
        );

        let mut local = Task::new("cas-conflict".to_string(), "local title".to_string());
        local.notes = "[2026-08-09 10:30] 📝 PROGRESS local note".to_string();
        store.add(&local).unwrap();
        queue
            .enqueue(EntityType::Task, &local.id, SyncOperation::Upsert, None)
            .unwrap();

        let mut remote = local.clone();
        remote.title = "remote title".to_string();
        remote.notes = "[2026-08-09 09:30] 📝 PROGRESS remote note".to_string();
        remote.updated_at += chrono::Duration::minutes(1);
        assert!(matches!(
            syncer.upsert_task(&*store, remote, "sync-test", "personal_pull"),
            Ok(UpsertResult::Updated)
        ));

        let merged = store.get("cas-conflict").unwrap();
        assert_eq!(merged.title, "remote title");
        assert_eq!(
            merged.notes,
            "[2026-08-09 09:30] 📝 PROGRESS remote note\n\n[2026-08-09 10:30] 📝 PROGRESS local note"
        );
        let conflicts = queue.list_conflicts(1).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].winner_side, "merged");
        assert!(conflicts[0].discarded_row_json.contains("local note"));
    }

    #[test]
    fn stale_active_remote_row_cannot_resurrect_closed_task_and_is_retained_as_conflict() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();
        let queue = Arc::new(SyncQueue::open(&cas_dir).unwrap());
        queue.init().unwrap();
        let syncer = CloudSyncer::new(
            queue.clone(),
            CloudConfig::default(),
            CloudSyncerConfig::default(),
        );

        let mut local = Task::new("cas-gh451".to_string(), "closed incident".to_string());
        local.status = TaskStatus::Closed;
        local.closed_at = Some(chrono::Utc::now() - chrono::Duration::hours(2));
        local.close_reason = Some("production incident remediated".to_string());
        store.add(&local).unwrap();

        // Mirrors GH #451: a cloud row is stamped newer, but contains only an
        // active status and no authorised reopen record.
        let mut stale_remote = local.clone();
        stale_remote.status = TaskStatus::Open;
        stale_remote.closed_at = None;
        stale_remote.close_reason = None;
        stale_remote.updated_at = chrono::Utc::now();
        assert!(matches!(
            syncer.upsert_task(&*store, stale_remote, "sync-gh451", "personal_pull"),
            Ok(UpsertResult::Skipped)
        ));

        let retained = store.get("cas-gh451").unwrap();
        assert_eq!(retained.status, TaskStatus::Closed);
        assert_eq!(
            retained.close_reason.as_deref(),
            Some("production incident remediated")
        );
        let conflicts = queue.list_conflicts(1).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].winner_side, "local");
        assert_eq!(conflicts[0].strategy, "terminal_status_guard");
        assert!(conflicts[0].discarded_row_json.contains("rejected_remote"));
    }

    #[test]
    fn team_remote_wins_cannot_bypass_terminal_status_guard() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();
        let queue = Arc::new(SyncQueue::open(&cas_dir).unwrap());
        queue.init().unwrap();
        let syncer = CloudSyncer::new(queue, CloudConfig::default(), CloudSyncerConfig::default());

        let mut local = Task::new("cas-team-terminal".to_string(), "closed task".to_string());
        local.status = TaskStatus::Cancelled;
        local.closed_at = Some(chrono::Utc::now() - chrono::Duration::hours(1));
        store.add(&local).unwrap();

        let mut remote = local.clone();
        remote.status = TaskStatus::InProgress;
        remote.updated_at = chrono::Utc::now();
        assert!(matches!(
            syncer.upsert_task_with_strategy(
                &*store,
                remote,
                crate::cloud::syncer::ConflictResolution::RemoteWins,
                "sync-team",
                "team_pull",
            ),
            Ok(UpsertResult::Skipped)
        ));
        assert_eq!(
            store.get("cas-team-terminal").unwrap().status,
            TaskStatus::Cancelled
        );
    }

    #[test]
    fn explicit_remote_reopen_applies_and_has_machine_provenance() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();
        let queue = Arc::new(SyncQueue::open(&cas_dir).unwrap());
        queue.init().unwrap();
        let syncer = CloudSyncer::new(queue, CloudConfig::default(), CloudSyncerConfig::default());

        let mut local = Task::new("cas-reopen".to_string(), "closed task".to_string());
        local.status = TaskStatus::Closed;
        local.closed_at = Some(chrono::Utc::now() - chrono::Duration::hours(1));
        local.close_reason = Some("original delivery merged".to_string());
        store.add(&local).unwrap();

        let mut remote = local.clone();
        remote.status = TaskStatus::Open;
        remote.closed_at = None;
        remote.close_reason = None;
        remote.updated_at = chrono::Utc::now() + chrono::Duration::minutes(1);
        remote.notes = format!(
            "[{}] Reopened: actor=remote-supervisor reason=regression found after deployment",
            chrono::Utc::now().format("%Y-%m-%d %H:%M")
        );
        assert!(matches!(
            syncer.upsert_task(&*store, remote, "sync-reopen", "personal_pull"),
            Ok(UpsertResult::Updated)
        ));

        let reopened = store.get("cas-reopen").unwrap();
        assert_eq!(reopened.status, TaskStatus::Open);
        assert!(reopened.notes.contains("[CAS_SYNC_STATUS]"));
        assert!(reopened.notes.contains("sync_id=sync-reopen"));
        assert!(reopened.notes.contains("source=personal_pull"));
        assert!(reopened.notes.contains("prior_status=closed"));
        assert!(
            reopened
                .notes
                .contains("prior_close_reason=original delivery merged")
        );
    }

    #[test]
    fn terminal_sync_rejects_unattributed_reopen_note() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();
        let queue = Arc::new(SyncQueue::open(&cas_dir).unwrap());
        queue.init().unwrap();
        let syncer = CloudSyncer::new(queue, CloudConfig::default(), CloudSyncerConfig::default());

        let mut local = Task::new("cas-unattributed-reopen".into(), "closed task".into());
        local.status = TaskStatus::Closed;
        local.closed_at = Some(chrono::Utc::now() - chrono::Duration::hours(1));
        store.add(&local).unwrap();

        let mut remote = local.clone();
        remote.status = TaskStatus::Open;
        remote.closed_at = None;
        remote.updated_at = chrono::Utc::now() + chrono::Duration::minutes(1);
        remote.notes = format!(
            "[{}] Reopened: regression found after deployment",
            chrono::Utc::now().format("%Y-%m-%d %H:%M")
        );

        assert!(matches!(
            syncer.upsert_task(&*store, remote, "sync-unattributed", "personal_pull"),
            Ok(UpsertResult::Skipped)
        ));
        assert_eq!(
            store.get("cas-unattributed-reopen").unwrap().status,
            TaskStatus::Closed,
            "sync must not accept a terminal exit lacking actor and reason"
        );
    }
}
