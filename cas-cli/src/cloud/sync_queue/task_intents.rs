use std::cell::Cell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::Utc;
use fs2::FileExt;
use rusqlite::{OptionalExtension, params};

use crate::cloud::sync_queue::queue_ops::{remove_legacy_team_upsert_row, upsert_queue_row};
use crate::cloud::sync_queue::{EntityType, SyncOperation, SyncQueue};
use crate::error::CasError;

/// Process-wide task-sync lease file. Current binaries hold it SHARED (one
/// refcounted handle per process); binaries before GH #1165 hold it
/// EXCLUSIVE for every mutation and reconcile, so the shared lease is what
/// keeps an older reconciler from consuming a newer mutator's live intent.
const TASK_SYNC_INTENT_LOCK: &str = "task-sync-intents.lock";
/// Per-entity mutation serialization (GH #1165): the intent protocol's state
/// (revisions, receipts, routes, intents) is keyed by entity, so mutations of
/// one task serialize on its stripe while unrelated writers proceed.
const TASK_SYNC_STRIPE_DIR: &str = "task-sync-intents.d";
const TASK_SYNC_STRIPES: u64 = 64;
/// How long a reconcile may wait for the SQLite write lock before it defers.
/// Reconcile runs on task-store opens, which are read paths.
const RECONCILE_BUSY_BOUND: Duration = Duration::from_millis(100);

thread_local! {
    /// Reconcile leases this thread holds. `begin_write` refuses while any is
    /// held: a reconcile takes its lease only after it owns the SQLite write
    /// lock, so it never makes another process wait on the lease while it
    /// waits on SQLite (GH #1165).
    static RECONCILE_LEASES_HELD: Cell<usize> = const { Cell::new(0) };
}

/// Stable across processes and binary versions (FNV-1a), unlike std's hasher.
fn task_sync_stripe(entity_id: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in entity_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash % TASK_SYNC_STRIPES
}

fn open_lock_file(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
}

fn is_lock_contended(error: &std::io::Error) -> bool {
    error.kind() == fs2::lock_contended_error().kind()
        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

/// Try a non-blocking lock; `Ok(false)` means another holder has it.
fn try_lock(file: &File, exclusive: bool) -> std::io::Result<bool> {
    let result = if exclusive {
        file.try_lock_exclusive()
    } else {
        FileExt::try_lock_shared(file)
    };
    match result {
        Ok(()) => Ok(true),
        Err(error) if is_lock_contended(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

/// One shared flock per process and lease file. flock belongs to the open
/// file description, so threads share one descriptor and a holder count
/// instead of contending through separate descriptors.
struct ProcessSharedLock {
    path: PathBuf,
    state: Mutex<(usize, Option<File>)>,
}

fn process_shared_lock(path: PathBuf) -> Arc<ProcessSharedLock> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<ProcessSharedLock>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Arc::clone(locks.entry(path.clone()).or_insert_with(|| {
        Arc::new(ProcessSharedLock {
            path,
            state: Mutex::new((0, None)),
        })
    }))
}

pub(crate) struct SharedTaskSyncLease(Arc<ProcessSharedLock>);

impl SharedTaskSyncLease {
    /// Waits only behind an exclusive holder, a pre-GH #1165 binary; every
    /// current holder takes the lease shared. The wait polls with the
    /// in-process state unlocked, so a sibling thread with a deadline is
    /// never stuck behind this one's wait.
    fn acquire(path: PathBuf, deadline: Option<Instant>) -> std::io::Result<Self> {
        let lock = process_shared_lock(path);
        let mut poll = LOCK_POLL_MIN;
        loop {
            {
                let mut state = lock
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if state.0 > 0 {
                    state.0 += 1;
                    break;
                }
                let file = open_lock_file(&lock.path)?;
                if try_lock(&file, false)? {
                    state.1 = Some(file);
                    state.0 = 1;
                    break;
                }
            }
            poll = wait_for_next_poll(deadline, poll)?;
        }
        Ok(Self(lock))
    }
}

const LOCK_POLL_MIN: Duration = Duration::from_millis(2);
const LOCK_POLL_MAX: Duration = Duration::from_millis(50);

/// Sleep before the next try-lock poll, never past `deadline`; `WouldBlock`
/// once the deadline has passed. Returns the next poll interval.
fn wait_for_next_poll(deadline: Option<Instant>, poll: Duration) -> std::io::Result<Duration> {
    let sleep = match deadline {
        None => poll,
        Some(deadline) => {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "task-sync mutation lease still held when the wait budget ran out",
                ));
            }
            poll.min(remaining)
        }
    };
    std::thread::sleep(sleep);
    Ok((poll * 2).min(LOCK_POLL_MAX))
}

impl Drop for SharedTaskSyncLease {
    fn drop(&mut self) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.0 = state.0.saturating_sub(1);
        if state.0 == 0
            && let Some(file) = state.1.take()
            && let Err(error) = FileExt::unlock(&file)
        {
            tracing::error!(%error, "failed to release task-sync shared lease");
        }
    }
}

/// A live mutation: the process-wide shared lease plus the exclusive stripe
/// of every entity it mutates. The operating system releases both after a
/// crash.
pub(crate) struct TaskSyncMutationGuard {
    stripes: Vec<File>,
    _shared: SharedTaskSyncLease,
}

impl Drop for TaskSyncMutationGuard {
    fn drop(&mut self) {
        for stripe in self.stripes.iter().rev() {
            if let Err(error) = FileExt::unlock(stripe) {
                tracing::error!(%error, "failed to release task-sync mutation stripe");
            }
        }
    }
}

/// A reconcile's claim on one entity, taken non-blocking and only while the
/// reconcile already owns the SQLite write lock.
pub(crate) struct TaskSyncReconcileLease {
    stripe: File,
    global: File,
}

impl Drop for TaskSyncReconcileLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.stripe);
        let _ = FileExt::unlock(&self.global);
        RECONCILE_LEASES_HELD.with(|held| held.set(held.get().saturating_sub(1)));
    }
}

/// Whether a fulfillment runs for a mutator that already holds the entity's
/// mutation lease, or for reconciliation, which must never wait on a lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskSyncFulfillMode {
    Mutation,
    Reconcile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskSyncIntent {
    pub id: i64,
    pub mutation_id: String,
    pub entity_id: String,
    pub operation: String,
    pub previous_updated_at: Option<String>,
    pub previous_revision: i64,
    pub committed_revision: Option<i64>,
    pub team_id: Option<String>,
    pub previous_team_id: Option<String>,
    pub previous_project_id: Option<String>,
    pub global_scope: bool,
}

pub(crate) struct TaskSyncPayload {
    pub payload: String,
    pub current_project_id: Option<String>,
    pub current_team_id: Option<String>,
    pub personal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskSyncFulfillResult {
    Fulfilled,
    ProvenPreCommit,
    Superseded,
    /// Reconcile only: a live mutation owns the entity, or the database was
    /// write-busy past the reconcile bound. Nothing changed; retry later.
    Deferred,
}

impl SyncQueue {
    fn task_sync_lock_path(&self) -> PathBuf {
        self.cas_dir.join(TASK_SYNC_INTENT_LOCK)
    }

    fn task_sync_stripe_path(&self, stripe: u64) -> PathBuf {
        self.cas_dir
            .join(TASK_SYNC_STRIPE_DIR)
            .join(format!("{stripe:02}.lock"))
    }

    /// Serialize mutations of `entity_ids` with each other and exclude
    /// reconciliation of those entities, across processes. Blocks only behind
    /// another live mutation of the same stripe (or a pre-GH #1165 binary);
    /// mutations of unrelated tasks and every reader proceed.
    pub(crate) fn lock_task_sync_mutations<S: AsRef<str>>(
        &self,
        entity_ids: &[S],
    ) -> Result<TaskSyncMutationGuard, CasError> {
        self.lock_task_sync_mutations_within(entity_ids, None)
    }

    /// [`Self::lock_task_sync_mutations`] with a wait budget: `None` waits as
    /// long as it takes; `Some(budget)` fails with an `io::ErrorKind::WouldBlock`
    /// error (as `CasError::Io`) once `budget` has passed without the leases,
    /// so a loop thread can fail fast instead of queueing (GH #1165).
    pub(crate) fn lock_task_sync_mutations_within<S: AsRef<str>>(
        &self,
        entity_ids: &[S],
        budget: Option<Duration>,
    ) -> Result<TaskSyncMutationGuard, CasError> {
        let deadline = budget.map(|budget| Instant::now() + budget);
        let shared = SharedTaskSyncLease::acquire(self.task_sync_lock_path(), deadline)?;
        let mut stripes: Vec<u64> = entity_ids
            .iter()
            .map(|id| task_sync_stripe(id.as_ref()))
            .collect();
        // Ascending order: two multi-entity holders cannot deadlock.
        stripes.sort_unstable();
        stripes.dedup();
        let mut guard = TaskSyncMutationGuard {
            stripes: Vec::with_capacity(stripes.len()),
            _shared: shared,
        };
        for stripe in stripes {
            let file = open_lock_file(&self.task_sync_stripe_path(stripe))?;
            if deadline.is_none() {
                file.lock_exclusive()?;
            } else {
                let mut poll = LOCK_POLL_MIN;
                while !try_lock(&file, true)? {
                    // On WouldBlock the guard drops and releases what it holds.
                    poll = wait_for_next_poll(deadline, poll)?;
                }
            }
            guard.stripes.push(file);
        }
        Ok(guard)
    }

    /// Claim `entity_id` for reconciliation without waiting: `None` when a
    /// live mutation (or a pre-GH #1165 holder) owns it.
    fn try_task_sync_reconcile_lease(
        &self,
        entity_id: &str,
    ) -> Result<Option<TaskSyncReconcileLease>, CasError> {
        let global = open_lock_file(&self.task_sync_lock_path())?;
        if !try_lock(&global, false)? {
            return Ok(None);
        }
        let stripe = open_lock_file(&self.task_sync_stripe_path(task_sync_stripe(entity_id)))?;
        if !try_lock(&stripe, true)? {
            let _ = FileExt::unlock(&global);
            return Ok(None);
        }
        RECONCILE_LEASES_HELD.with(|held| held.set(held.get() + 1));
        Ok(Some(TaskSyncReconcileLease { stripe, global }))
    }

    pub(crate) fn stage_task_sync_intent(
        &self,
        entity_id: &str,
        operation: &str,
        previous_updated_at: Option<&str>,
        team_id: Option<&str>,
        fallback_previous_team_id: Option<&str>,
        fallback_previous_project_id: Option<&str>,
        global_scope: bool,
    ) -> Result<TaskSyncIntent, CasError> {
        let conn = self.conn.lock().unwrap();
        let tx = begin_write(&conn)?;
        let mutation_id = uuid::Uuid::new_v4().to_string();
        let previous_revision = tx
            .query_row(
                "SELECT revision FROM task_mutation_revisions WHERE entity_id = ?1",
                params![entity_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        let previous_route = tx
            .query_row(
                "SELECT team_id, project_id FROM task_sync_routes WHERE entity_id = ?1",
                params![entity_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()?;
        let (previous_team_id, previous_project_id) = previous_route.unwrap_or_else(|| {
            (
                fallback_previous_team_id.map(ToOwned::to_owned),
                fallback_previous_project_id.map(ToOwned::to_owned),
            )
        });
        tx.execute(
            r#"
            INSERT INTO task_sync_intents
                (mutation_id, entity_id, operation, previous_updated_at, previous_revision,
                 team_id, previous_team_id, previous_project_id, global_scope, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                mutation_id,
                entity_id,
                operation,
                previous_updated_at,
                previous_revision,
                team_id,
                previous_team_id,
                previous_project_id,
                global_scope,
                Utc::now().to_rfc3339(),
            ],
        )?;
        let intent = TaskSyncIntent {
            id: tx.last_insert_rowid(),
            mutation_id,
            entity_id: entity_id.to_string(),
            operation: operation.to_string(),
            previous_updated_at: previous_updated_at.map(ToOwned::to_owned),
            previous_revision,
            committed_revision: None,
            team_id: team_id.map(ToOwned::to_owned),
            previous_team_id,
            previous_project_id,
            global_scope,
        };
        tx.commit()?;
        Ok(intent)
    }

    pub(crate) fn cancel_task_sync_intent(&self, intent_id: i64) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        let tx = begin_write(&conn)?;
        tx.execute(
            "DELETE FROM task_mutation_receipts WHERE receipt_id IN
             (SELECT mutation_id FROM task_sync_intents WHERE id = ?1)",
            params![intent_id],
        )?;
        tx.execute(
            "DELETE FROM task_sync_intents WHERE id = ?1",
            params![intent_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn pending_task_sync_intents(&self) -> Result<Vec<TaskSyncIntent>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            r#"
            SELECT i.id, i.mutation_id, i.entity_id, i.operation, i.previous_updated_at,
                   i.previous_revision, r.revision, i.team_id, i.previous_team_id,
                   i.previous_project_id, i.global_scope
            FROM task_sync_intents i
            LEFT JOIN task_mutation_receipts r ON r.receipt_id = i.mutation_id
            ORDER BY i.id ASC
            "#,
        )?;
        let rows = statement.query_map([], |row| {
            Ok(TaskSyncIntent {
                id: row.get(0)?,
                mutation_id: row.get(1)?,
                entity_id: row.get(2)?,
                operation: row.get(3)?,
                previous_updated_at: row.get(4)?,
                previous_revision: row.get(5)?,
                committed_revision: row.get(6)?,
                team_id: row.get(7)?,
                previous_team_id: row.get(8)?,
                previous_project_id: row.get(9)?,
                global_scope: row.get(10)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CasError::from)
    }

    /// Queue only the canonical payload under the current routing policy.
    /// An obsolete proven route receives a payload-free delete.
    ///
    /// `Mutation` callers hold the entity's mutation lease. `Reconcile`
    /// callers hold no lease: this takes the SQLite write lock within
    /// [`RECONCILE_BUSY_BOUND`] and only then claims the entity without
    /// waiting, returning [`TaskSyncFulfillResult::Deferred`] when either is
    /// unavailable (GH #1165).
    pub(crate) fn fulfill_task_sync_intent<F, H>(
        &self,
        intent: &TaskSyncIntent,
        mode: TaskSyncFulfillMode,
        after_validation: H,
        mut load_canonical: F,
    ) -> Result<TaskSyncFulfillResult, CasError>
    where
        F: FnMut() -> Result<TaskSyncPayload, CasError>,
        H: FnOnce(),
    {
        const ATTEMPTS: usize = 3;
        let conn = self.conn.lock().unwrap();
        let mut after_validation = Some(after_validation);
        let mut attempt = 0;
        let (tx, canonical, _reconcile_lease) = loop {
            attempt += 1;
            // cas-0e57: load the canonical task BEFORE taking the write lock.
            // The load runs on the task store's pooled connection, whose
            // in-process mutex another thread can hold while it waits for
            // the SQLite write lock. Loading inside this transaction inverted
            // that order: this thread held the write lock and waited for the
            // mutex while the other held the mutex and waited for the lock,
            // until its retry gave up ("database busy for 31.6s across 6
            // attempts") and every other process queued behind both.
            // Consistency is unchanged: the in-transaction revision must
            // equal the revision read before the load, or the attempt retries.
            let preloaded = match classify_task_sync_intent(&conn, intent)? {
                TaskSyncIntentClass::Proceed { revision } => Some((revision, load_canonical())),
                _ => None,
            };
            // BEGIN IMMEDIATE is load-bearing: revision validation and the
            // outbox rows commit while this transaction excludes every bypass
            // writer. Acquiring it retries with bounded backoff past one
            // busy_timeout window (cas-d5c8). Reconcile instead bounds the
            // wait and defers, and claims the entity only once it owns the
            // write lock, so it never holds a lease across a busy wait.
            let (tx, reconcile_lease) = match mode {
                TaskSyncFulfillMode::Mutation => (begin_write(&conn)?, None),
                TaskSyncFulfillMode::Reconcile => {
                    let Some(tx) = begin_write_bounded(&conn)? else {
                        return Ok(TaskSyncFulfillResult::Deferred);
                    };
                    let Some(lease) = self.try_task_sync_reconcile_lease(&intent.entity_id)? else {
                        return Ok(TaskSyncFulfillResult::Deferred);
                    };
                    (tx, Some(lease))
                }
            };
            let revision = match classify_task_sync_intent(&tx, intent)? {
                TaskSyncIntentClass::Superseded => {
                    retire_task_sync_evidence(&tx, &intent.entity_id)?;
                    tx.commit()?;
                    return Ok(TaskSyncFulfillResult::Superseded);
                }
                TaskSyncIntentClass::ProvenPreCommit => {
                    retire_one_task_sync_intent(&tx, intent)?;
                    tx.commit()?;
                    return Ok(TaskSyncFulfillResult::ProvenPreCommit);
                }
                TaskSyncIntentClass::Blocked(message) => return Err(CasError::Other(message)),
                TaskSyncIntentClass::Proceed { revision } => revision,
            };
            let canonical = match preloaded {
                Some((loaded, result)) if loaded == revision => Some(result?),
                // The task changed between the load and the lock: retry.
                _ if attempt < ATTEMPTS => continue,
                // Reconcile leaves a churning task to its live writers.
                _ if mode == TaskSyncFulfillMode::Reconcile => {
                    return Ok(TaskSyncFulfillResult::Deferred);
                }
                // Sustained churn on one task: fall back to loading under the
                // lock rather than leave the intent unfulfilled.
                _ => None,
            };
            if let Some(hook) = after_validation.take() {
                hook();
            }
            let canonical = match canonical {
                Some(canonical) => canonical,
                None => load_canonical()?,
            };
            break (tx, canonical, reconcile_lease);
        };
        let payload = canonical.payload;
        let local_project = crate::cloud::resolve_canonical_id(&self.cas_dir);
        let project_task = serde_json::from_str::<serde_json::Value>(&payload)
            .ok()
            .is_some_and(|value| value.get("scope").and_then(|v| v.as_str()) != Some("global"));
        let mut authored_here = true;
        if project_task {
            let origin =
                Self::queued_origin(&tx, EntityType::Task, &intent.entity_id, Some(&payload));
            authored_here = local_project.as_deref().is_none_or(|local| {
                origin.as_deref().is_none_or(|origin| {
                    origin != "unknown" && crate::cloud::project_ids_match(origin, local)
                })
            });
        }
        let current_project_id = canonical.current_project_id.as_deref();
        let current_team_id = canonical.current_team_id.as_deref();
        if !authored_here {
            // A move away still owes the old owner a tombstone. Keep that
            // delete across later edits while removing every stale upsert.
            tx.execute(
                "DELETE FROM sync_queue WHERE entity_type = 'task' AND entity_id = ?1 AND operation = 'upsert'",
                params![intent.entity_id],
            )?;
            if let (Some(local), Some(team_id), Some(project_id)) = (
                local_project.as_deref(),
                intent.previous_team_id.as_deref(),
                intent.previous_project_id.as_deref(),
            ) && crate::cloud::project_ids_match(project_id, local)
            {
                upsert_queue_row(
                    &tx,
                    EntityType::Task,
                    &intent.entity_id,
                    SyncOperation::Delete,
                    None,
                    team_id,
                    Some(project_id),
                )?;
            }
            Self::record_unauthored_skip(&tx)?;
        } else if canonical.personal {
            upsert_queue_row(
                &tx,
                EntityType::Task,
                &intent.entity_id,
                SyncOperation::Upsert,
                Some(&payload),
                "",
                None,
            )?;
        } else {
            tx.execute(
                "DELETE FROM sync_queue WHERE entity_type = 'task' AND entity_id = ?1 AND team_id = ''",
                params![intent.entity_id],
            )?;
        }

        let previous_route = intent
            .previous_team_id
            .as_deref()
            .map(|team_id| (team_id, intent.previous_project_id.as_deref()));
        let current_route = current_team_id.map(|team_id| (team_id, current_project_id));
        if authored_here
            && previous_route != current_route
            && let Some((team_id, project_id)) = previous_route
        {
            remove_legacy_team_upsert_row(&tx, EntityType::Task, &intent.entity_id, team_id)?;
            upsert_queue_row(
                &tx,
                EntityType::Task,
                &intent.entity_id,
                SyncOperation::Delete,
                None,
                team_id,
                project_id,
            )?;
        }
        if authored_here && let Some((team_id, project_id)) = current_route {
            upsert_queue_row(
                &tx,
                EntityType::Task,
                &intent.entity_id,
                SyncOperation::Upsert,
                Some(&payload),
                team_id,
                project_id,
            )?;
        }

        tx.execute(
            r#"
            INSERT INTO task_sync_routes (entity_id, team_id, project_id, updated_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(entity_id) DO UPDATE SET
                team_id = excluded.team_id,
                project_id = excluded.project_id,
                updated_at = excluded.updated_at
            "#,
            params![
                intent.entity_id,
                current_team_id,
                current_project_id,
                Utc::now().to_rfc3339(),
            ],
        )?;
        retire_task_sync_evidence(&tx, &intent.entity_id)?;
        tx.commit()?;
        Ok(TaskSyncFulfillResult::Fulfilled)
    }
}

/// Where a staged task sync intent stands against the store's revision and
/// receipt evidence.
enum TaskSyncIntentClass {
    /// The mutation committed and is current at `revision`: load and enqueue.
    Proceed { revision: i64 },
    /// A later mutation superseded this one.
    Superseded,
    /// The mutation never committed.
    ProvenPreCommit,
    /// The evidence does not classify the intent.
    Blocked(String),
}

fn classify_task_sync_intent(
    conn: &rusqlite::Connection,
    intent: &TaskSyncIntent,
) -> Result<TaskSyncIntentClass, CasError> {
    let current_revision = conn
        .query_row(
            "SELECT revision, present FROM task_mutation_revisions WHERE entity_id = ?1",
            params![intent.entity_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? == 1)),
        )
        .optional()?;
    let committed_revision = conn
        .query_row(
            "SELECT revision FROM task_mutation_receipts WHERE receipt_id = ?1 AND entity_id = ?2",
            params![intent.mutation_id, intent.entity_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    Ok(match (committed_revision, current_revision) {
        (Some(committed), Some((current, true))) if committed == current => {
            TaskSyncIntentClass::Proceed { revision: current }
        }
        (Some(committed), Some((current, _))) if current > committed => {
            TaskSyncIntentClass::Superseded
        }
        (None, Some((current, _))) if current == intent.previous_revision => {
            TaskSyncIntentClass::ProvenPreCommit
        }
        (None, None) if intent.previous_revision == 0 => TaskSyncIntentClass::ProvenPreCommit,
        (committed, current) => TaskSyncIntentClass::Blocked(format!(
            "task sync recovery blocked for {}: intent revision is unclassified (previous={}, committed={committed:?}, current={current:?}); durable evidence retained",
            intent.entity_id, intent.previous_revision
        )),
    })
}

/// Open the write transaction every task sync-intent write runs in (cas-d5c8,
/// GH #921): BEGIN IMMEDIATE, so the write lock is taken before the first read
/// and SQLite's busy handler applies, with the bounded jittered retry of
/// `cas_store::shared_db::begin_immediate_with_retry` covering a holder that
/// outlives one busy_timeout window. A DEFERRED transaction that reads first
/// fails the moment it upgrades after another connection committed, which is
/// how fleet task notes/update/create/cancel hit "database is locked" within
/// seconds.
fn begin_write(
    conn: &rusqlite::Connection,
) -> Result<cas_store::shared_db::ImmediateTx<'_>, CasError> {
    refuse_write_wait_under_reconcile_lease()?;
    cas_store::shared_db::begin_immediate_with_retry(conn).map_err(CasError::from)
}

/// GH #1165 structural guard: a thread holding a reconcile lease must not
/// wait for the SQLite write lock, or every process queued on the lease
/// waits out that thread's busy retries (measured up to 42.8 s).
fn refuse_write_wait_under_reconcile_lease() -> Result<(), CasError> {
    if RECONCILE_LEASES_HELD.with(Cell::get) > 0 {
        return Err(CasError::Other(
            "task-sync reconcile lease held while waiting for the SQLite write lock (GH #1165)"
                .to_string(),
        ));
    }
    Ok(())
}

/// One BEGIN IMMEDIATE attempt waiting at most [`RECONCILE_BUSY_BOUND`];
/// `None` when the database stays write-busy. The connection's normal busy
/// timeout is restored before returning.
fn begin_write_bounded(
    conn: &rusqlite::Connection,
) -> Result<Option<cas_store::shared_db::ImmediateTx<'_>>, CasError> {
    refuse_write_wait_under_reconcile_lease()?;
    conn.busy_timeout(RECONCILE_BUSY_BOUND)?;
    let began = cas_store::shared_db::ImmediateTx::new(conn);
    let restored = conn.busy_timeout(cas_store::SQLITE_BUSY_TIMEOUT);
    match began {
        Ok(tx) => {
            restored?;
            Ok(Some(tx))
        }
        Err(error) if cas_store::shared_db::is_busy_error(&error) => {
            restored?;
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

fn retire_one_task_sync_intent(
    conn: &rusqlite::Connection,
    intent: &TaskSyncIntent,
) -> Result<(), CasError> {
    conn.execute(
        "DELETE FROM task_mutation_receipts WHERE receipt_id = ?1",
        params![intent.mutation_id],
    )?;
    conn.execute(
        "DELETE FROM task_sync_intents WHERE id = ?1",
        params![intent.id],
    )?;
    Ok(())
}

fn retire_task_sync_evidence(conn: &rusqlite::Connection, entity_id: &str) -> Result<(), CasError> {
    conn.execute(
        "DELETE FROM task_mutation_receipts WHERE receipt_id IN
         (SELECT mutation_id FROM task_sync_intents WHERE entity_id = ?1)",
        params![entity_id],
    )?;
    conn.execute(
        "DELETE FROM task_sync_intents WHERE entity_id = ?1",
        params![entity_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{SqliteTaskStore, TaskStore};

    #[test]
    fn legacy_intent_migrates_to_recovery_blocked_evidence() {
        let temp = tempfile::TempDir::new().unwrap();
        let tasks = SqliteTaskStore::open(temp.path()).unwrap();
        tasks.init().unwrap();
        let task = crate::types::Task::new("task-legacy-intent".into(), "canonical".into());
        tasks.add(&task).unwrap();
        let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE task_sync_intents (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entity_id TEXT NOT NULL,
                operation TEXT NOT NULL,
                previous_updated_at TEXT,
                team_id TEXT,
                previous_team_id TEXT,
                previous_project_id TEXT,
                global_scope INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            INSERT INTO task_sync_intents
                (entity_id, operation, global_scope, created_at)
            VALUES ('task-legacy-intent', 'update', 0, '2026-09-05T00:00:00Z');
            "#,
        )
        .unwrap();

        let queue = SyncQueue::open(temp.path()).unwrap();
        queue.init().unwrap();
        let intent = queue.pending_task_sync_intents().unwrap().pop().unwrap();
        assert_eq!(intent.previous_revision, -1);
        assert!(intent.mutation_id.starts_with("legacy-unbound-"));

        let error = queue
            .fulfill_task_sync_intent(
                &intent,
                TaskSyncFulfillMode::Reconcile,
                || panic!("unclassified intent must not reach payload loading"),
                || panic!("unclassified intent must not load canonical payload"),
            )
            .unwrap_err();
        assert!(error.to_string().contains("recovery blocked"), "{error}");
        assert_eq!(queue.pending_task_sync_intents().unwrap(), vec![intent]);
    }

    #[test]
    fn concurrent_task_mutations_keep_independent_repair_intents() {
        let temp = tempfile::TempDir::new().unwrap();
        let tasks = SqliteTaskStore::open(temp.path()).unwrap();
        tasks.init().unwrap();
        let queue = SyncQueue::open(temp.path()).unwrap();
        queue.init().unwrap();
        let first = queue
            .stage_task_sync_intent("cas-concurrent", "update", None, None, None, None, false)
            .unwrap();
        let second = queue
            .stage_task_sync_intent("cas-concurrent", "update", None, None, None, None, false)
            .unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(queue.pending_task_sync_intents().unwrap().len(), 2);
        queue.cancel_task_sync_intent(first.id).unwrap();
        assert_eq!(queue.pending_task_sync_intents().unwrap(), vec![second]);
    }

    /// cas-d5c8 (GH #921): every MCP task write stages a sync intent first.
    /// Staging read revision state and then wrote in one DEFERRED transaction,
    /// and SQLite answers that read-to-write upgrade with SQLITE_BUSY as soon
    /// as another connection has committed since the read, without calling
    /// the busy handler. With a fleet writing, task notes/update/create/cancel
    /// failed within seconds with "database is locked". Staging (and the other
    /// intent writes) must wait out a concurrent writer instead.
    #[test]
    fn staging_a_task_mutation_waits_out_a_concurrent_writer() {
        let temp = tempfile::TempDir::new().unwrap();
        let tasks = SqliteTaskStore::open(temp.path()).unwrap();
        tasks.init().unwrap();
        let queue = SyncQueue::open(temp.path()).unwrap();
        queue.init().unwrap();
        let db = temp.path().join("cas.db");
        for round in 0..3 {
            let (held_tx, held_rx) = std::sync::mpsc::channel();
            let writer_db = db.clone();
            let writer = std::thread::spawn(move || {
                let conn = rusqlite::Connection::open(writer_db).unwrap();
                conn.execute_batch("BEGIN IMMEDIATE").unwrap();
                conn.execute(
                    "INSERT INTO task_mutation_revisions (entity_id, revision, present) VALUES (?1, 1, 1)
                     ON CONFLICT(entity_id) DO UPDATE SET revision = revision + 1",
                    params![format!("other-task-{round}")],
                )
                .unwrap();
                held_tx.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(300));
                conn.execute_batch("COMMIT").unwrap();
            });
            held_rx.recv().unwrap();
            let staged = queue.stage_task_sync_intent(
                &format!("cas-contended-{round}"),
                "update",
                None,
                None,
                None,
                None,
                false,
            );
            writer.join().unwrap();
            let intent = staged.unwrap_or_else(|error| {
                panic!("round {round}: staging must wait out the writer: {error}")
            });
            queue.cancel_task_sync_intent(intent.id).unwrap();
        }
    }

    #[test]
    fn task_sync_lock_releases_for_an_independent_handle() {
        let temp = tempfile::TempDir::new().unwrap();
        let queue = SyncQueue::open(temp.path()).unwrap();
        let held = queue.lock_task_sync_mutations(&["cas-held"]).unwrap();
        // A pre-GH #1165 binary takes the process lease exclusive.
        let legacy = open_lock_file(&temp.path().join(TASK_SYNC_INTENT_LOCK)).unwrap();
        assert!(legacy.try_lock_exclusive().is_err());
        let stripe =
            open_lock_file(&queue.task_sync_stripe_path(task_sync_stripe("cas-held"))).unwrap();
        assert!(stripe.try_lock_exclusive().is_err());
        drop(held);
        legacy.try_lock_exclusive().unwrap();
        stripe.try_lock_exclusive().unwrap();
        FileExt::unlock(&legacy).unwrap();
        FileExt::unlock(&stripe).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn task_sync_lock_descriptor_is_close_on_exec() {
        use std::os::fd::AsRawFd;

        let temp = tempfile::TempDir::new().unwrap();
        let queue = SyncQueue::open(temp.path()).unwrap();
        let held = queue.lock_task_sync_mutations(&["cas-cloexec"]).unwrap();
        let shared = held._shared.0.state.lock().unwrap();
        for fd in [
            held.stripes[0].as_raw_fd(),
            shared.1.as_ref().unwrap().as_raw_fd(),
        ] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            assert!(flags >= 0);
            assert_ne!(flags & libc::FD_CLOEXEC, 0);
        }
    }

    /// GH #1165 (4): threads of one process share one lease descriptor and a
    /// holder count. Mutations of unrelated tasks never wait for each other;
    /// the lease is released only when its last in-process holder drops.
    #[test]
    fn unrelated_mutations_share_one_process_lease_without_waiting() {
        let temp = tempfile::TempDir::new().unwrap();
        let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
        let (first_id, second_id) = ("cas-stripe-a", "cas-stripe-b");
        assert_ne!(task_sync_stripe(first_id), task_sync_stripe(second_id));
        let first = queue.lock_task_sync_mutations(&[first_id]).unwrap();
        let contender = Arc::clone(&queue);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let second = contender.lock_task_sync_mutations(&[second_id]).unwrap();
            done_tx.send(()).unwrap();
            drop(second);
        });
        done_rx
            .recv_timeout(std::time::Duration::from_millis(500))
            .expect("an unrelated mutation must not wait for a held one");
        thread.join().unwrap();
        let legacy = open_lock_file(&temp.path().join(TASK_SYNC_INTENT_LOCK)).unwrap();
        assert!(
            legacy.try_lock_exclusive().is_err(),
            "the shared lease outlives a sibling holder's release"
        );
        drop(first);
        legacy.try_lock_exclusive().unwrap();
        FileExt::unlock(&legacy).unwrap();
    }

    /// A loop-thread caller with a wait budget fails fast with WouldBlock
    /// instead of queueing behind a held stripe or a pre-GH #1165 exclusive
    /// holder, and releases whatever it had taken.
    #[test]
    fn bounded_mutation_lock_fails_fast_with_would_block() {
        let temp = tempfile::TempDir::new().unwrap();
        let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
        let budget = Some(std::time::Duration::from_millis(60));
        // Stripes are taken in ascending order: "cas-bounded" (6) is taken,
        // then the wait on the held "cas-other" (9) runs out.
        assert!(task_sync_stripe("cas-bounded") < task_sync_stripe("cas-other"));
        let held = queue.lock_task_sync_mutations(&["cas-other"]).unwrap();
        let contender = Arc::clone(&queue);
        let (started, result) = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let result = contender
                .lock_task_sync_mutations_within(&["cas-other", "cas-bounded"], budget)
                .map(|_| ());
            (started.elapsed(), result)
        })
        .join()
        .unwrap();
        match result {
            Err(CasError::Io(error)) => {
                assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock)
            }
            other => panic!("expected WouldBlock, got {other:?}"),
        }
        assert!(
            started >= std::time::Duration::from_millis(60),
            "{started:?}"
        );
        assert!(
            started < std::time::Duration::from_millis(500),
            "{started:?}"
        );
        // The failed attempt released the stripe it had already taken.
        let other =
            open_lock_file(&queue.task_sync_stripe_path(task_sync_stripe("cas-bounded"))).unwrap();
        other.try_lock_exclusive().unwrap();
        FileExt::unlock(&other).unwrap();
        drop(held);
        drop(
            queue
                .lock_task_sync_mutations_within(&["cas-other"], budget)
                .unwrap(),
        );

        let legacy = open_lock_file(&temp.path().join(TASK_SYNC_INTENT_LOCK)).unwrap();
        legacy.lock_exclusive().unwrap();
        let started = std::time::Instant::now();
        let error = queue
            .lock_task_sync_mutations_within(&["cas-bounded"], budget)
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(&error, CasError::Io(e) if e.kind() == std::io::ErrorKind::WouldBlock));
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
        FileExt::unlock(&legacy).unwrap();
    }

    /// Same-task mutations still serialize: the intent protocol's evidence is
    /// per entity.
    #[test]
    fn same_task_mutations_serialize_on_their_stripe() {
        let temp = tempfile::TempDir::new().unwrap();
        let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
        let held = queue.lock_task_sync_mutations(&["cas-same"]).unwrap();
        let contender = Arc::clone(&queue);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let guard = contender.lock_task_sync_mutations(&["cas-same"]).unwrap();
            done_tx.send(()).unwrap();
            drop(guard);
        });
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_millis(150))
                .is_err(),
            "a second mutation of the same task must wait"
        );
        drop(held);
        done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the waiting mutation proceeds after release");
        thread.join().unwrap();
    }

    /// GH #1165 (3) structural guard: no path can wait for the SQLite write
    /// lock while it holds a reconcile lease.
    #[test]
    fn write_waits_are_refused_while_a_reconcile_lease_is_held() {
        let temp = tempfile::TempDir::new().unwrap();
        let tasks = SqliteTaskStore::open(temp.path()).unwrap();
        tasks.init().unwrap();
        let queue = SyncQueue::open(temp.path()).unwrap();
        queue.init().unwrap();
        let lease = queue
            .try_task_sync_reconcile_lease("cas-guarded")
            .unwrap()
            .expect("an idle entity can be claimed");
        let error = queue
            .stage_task_sync_intent("cas-guarded", "update", None, None, None, None, false)
            .unwrap_err();
        assert!(error.to_string().contains("GH #1165"), "{error}");
        let conn = queue.conn.lock().unwrap();
        assert!(begin_write_bounded(&conn).is_err());
        drop(conn);
        drop(lease);
        let intent = queue
            .stage_task_sync_intent("cas-guarded", "update", None, None, None, None, false)
            .unwrap();
        queue.cancel_task_sync_intent(intent.id).unwrap();
    }

    /// Reconcile claims an entity without waiting: a live mutation of the
    /// entity, or a pre-GH #1165 holder of the process lease, defers it.
    #[test]
    fn reconcile_lease_never_waits() {
        let temp = tempfile::TempDir::new().unwrap();
        let queue = SyncQueue::open(temp.path()).unwrap();
        let mutation = queue.lock_task_sync_mutations(&["cas-live"]).unwrap();
        assert!(
            queue
                .try_task_sync_reconcile_lease("cas-live")
                .unwrap()
                .is_none()
        );
        let other = queue
            .try_task_sync_reconcile_lease("cas-stripe-b")
            .unwrap()
            .expect("another entity's stripe is free");
        drop(other);
        drop(mutation);

        let legacy = open_lock_file(&temp.path().join(TASK_SYNC_INTENT_LOCK)).unwrap();
        legacy.lock_exclusive().unwrap();
        let started = std::time::Instant::now();
        assert!(
            queue
                .try_task_sync_reconcile_lease("cas-live")
                .unwrap()
                .is_none()
        );
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        FileExt::unlock(&legacy).unwrap();
        assert!(
            queue
                .try_task_sync_reconcile_lease("cas-live")
                .unwrap()
                .is_some()
        );
        assert_eq!(RECONCILE_LEASES_HELD.with(Cell::get), 0);
    }
}
