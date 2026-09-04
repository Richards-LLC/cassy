//! Read-only detection of cross-project ("foreign") task and knowledge-page rows.
//!
//! # Why this exists
//!
//! The `cas-ed15` pull leak (fixed in v2.15.0) made `cas cloud pull` return
//! every `team_id IS NULL` row of a user's *entire* account, not just the rows
//! belonging to the project being pulled. Machines that synced several projects
//! through one personal token before that fix still carry the residue: hundreds
//! to thousands of another project's task rows sit in each project database,
//! and nothing in cas ever reported them. Frozen replicas of months-finished
//! work show up as `open`, so `task ready` lists another project's backlog as
//! this project's outstanding work.
//!
//! # Identity: `(id, title)`, never `id` alone
//!
//! Task ids are 4 hex characters (~65k space) and **collide**. Measured across
//! 39 project databases on the reporting machine: 5,824 distinct ids appeared,
//! 2,265 of them in more than one database — 2,149 genuine replicas but **73
//! pure collisions** (same id, two entirely different real tasks) and 43 mixed.
//! `created_at` is not an identity key either (the same title was observed with
//! different `created_at` values across databases). Any id-keyed detection or
//! cleanup therefore destroys real work. Everything below keys on
//! `(id, trimmed title)`, and rows that share an id but *not* a title are
//! reported separately as collisions — precisely so a human reading the report
//! is warned off `DELETE ... WHERE id IN (...)`.
//!
//! # Attribution
//!
//! Local task rows carry no project column, so "foreign" cannot be read off a
//! single database. Instead each project database on the host (from the
//! `known_repos` registry) is compared, and attribution uses **local-activity
//! evidence**: lease history, leases, verifications, worker receipts, spawn
//! queue entries — tables that are never synced through the cloud and so only
//! exist in the database where the work actually happened. Evidence is
//! asymmetric proof: its presence proves a row is native, its absence proves
//! nothing on its own. So a row is only called foreign when *this* database has
//! no evidence for it and some *other* project database does. Replicas with no
//! evidence anywhere are reported honestly as unattributed rather than being
//! guessed at.
//!
//! This module never writes: every database (including the local one) is opened
//! `SQLITE_OPEN_READ_ONLY`.
//!
//! Knowledge pages do not need the peer/evidence heuristic: m226 gives them a
//! durable local-vs-cloud-pull origin and the exact project id accepted from the
//! wire. Their audit reuses the cloud ingest predicate so pre-write refusal and
//! post-write detection cannot drift on project identity.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Tables whose `task_id` column proves the task was actually worked in the
/// database that holds the row. All of these are local-only: none is part of
/// the cloud sync payload (`entries`, `tasks`, `rules`, `skills`, `specs`,
/// `events`, `prompts`, `file_changes`, `commit_links`), so a replica pulled
/// from another project arrives without any of them.
const LOCAL_ACTIVITY_TABLES: &[&str] = &[
    "task_leases",
    "task_lease_history",
    "verifications",
    "verification_dispatches",
    "worker_completion_receipts",
    "worktrees",
    "spawn_queue",
    "loops",
    "worker_delivery_transactions",
];

/// One task row as seen in some project database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    /// Persisted owner identity, when the local schema has it. Legacy rows
    /// without this field are not safe purge candidates even with peer proof.
    pub origin_project: Option<String>,
    /// `status == "closed"`. Split out because closed replicas are noise while
    /// non-closed replicas actively lie in ready queues (AC3).
    pub closed: bool,
}

impl TaskRow {
    /// Identity component used for matching. Trimmed so that a stray trailing
    /// newline in one database does not masquerade as an id collision.
    fn key_title(&self) -> &str {
        self.title.trim()
    }
}

/// A project database reduced to what the scan needs.
#[derive(Debug, Clone, Default)]
pub struct DbSnapshot {
    /// Human label for the project (directory name, or the path if ambiguous).
    pub project: String,
    /// Absolute path of the database the snapshot was read from.
    pub db_path: PathBuf,
    pub tasks: Vec<TaskRow>,
    /// Task ids with local-activity evidence in this database.
    pub worked_task_ids: BTreeSet<String>,
    /// Knowledge pages carry direct provenance, unlike legacy task rows.
    pub knowledge_pages: Vec<KnowledgePageAttributionRow>,
}

/// Durable attribution columns read from one knowledge-page row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgePageAttributionRow {
    pub id: String,
    pub title: String,
    pub rel_path: String,
    pub origin: String,
    pub origin_project_id: Option<String>,
}

/// A cloud-pulled page whose asserted project fails the sync ingest predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKnowledgePage {
    pub id: String,
    pub title: String,
    pub rel_path: String,
    pub origin_project_id: Option<String>,
}

/// A page whose provenance cannot be audited (legacy or malformed schema data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnattributedKnowledgePage {
    pub id: String,
    pub title: String,
    pub rel_path: String,
    pub reason: String,
}

/// A local row attributed to another project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignRow {
    pub id: String,
    pub title: String,
    pub closed: bool,
    /// The local row's persisted owner identity. A backfilled current-project
    /// value is the signal that peer evidence must override.
    pub origin_project: Option<String>,
    /// Project whose database carries local-activity evidence for this row.
    pub home_project: String,
    /// Every other project database holding the same `(id, title)`.
    pub also_present_in: Vec<String>,
}

/// A local row replicated elsewhere with no activity evidence anywhere, so its
/// home cannot be established. Reported, never accused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnattributedRow {
    pub id: String,
    pub title: String,
    pub closed: bool,
    pub present_in: Vec<String>,
}

/// Same id, different task. The reason id-keyed cleanup is unsafe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdCollision {
    pub id: String,
    pub local_title: String,
    pub other_project: String,
    pub other_title: String,
}

/// A peer database that exists but could not be read. Named, never silently
/// skipped: a scan that quietly ignored half the host would under-report
/// contamination and read as a clean bill of health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadablePeer {
    pub project: String,
    pub db_path: PathBuf,
    pub error: String,
}

/// Result of a read-only contamination scan of one project database.
#[derive(Debug, Clone, Default)]
pub struct ForeignRowReport {
    pub local_project: String,
    pub local_task_count: usize,
    pub peers_compared: Vec<String>,
    pub peers_unreadable: Vec<UnreadablePeer>,
    pub foreign: Vec<ForeignRow>,
    pub unattributed: Vec<UnattributedRow>,
    pub collisions: Vec<IdCollision>,
    /// Rows also present in a peer database but worked *here* — native, listed
    /// only so the totals add up.
    pub locally_worked_replicas: usize,
    pub local_knowledge_page_count: usize,
    pub foreign_knowledge_pages: Vec<ForeignKnowledgePage>,
    pub unattributed_knowledge_pages: Vec<UnattributedKnowledgePage>,
}

impl ForeignRowReport {
    pub fn foreign_open(&self) -> usize {
        self.foreign.iter().filter(|r| !r.closed).count()
    }

    pub fn foreign_closed(&self) -> usize {
        self.foreign.iter().filter(|r| r.closed).count()
    }

    pub fn unattributed_open(&self) -> usize {
        self.unattributed.iter().filter(|r| !r.closed).count()
    }

    pub fn unattributed_closed(&self) -> usize {
        self.unattributed.iter().filter(|r| r.closed).count()
    }

    pub fn is_clean(&self) -> bool {
        self.foreign.is_empty()
            && self.unattributed.is_empty()
            && self.foreign_knowledge_pages.is_empty()
            && self.unattributed_knowledge_pages.is_empty()
    }

    /// Distinct projects the foreign rows are attributed to, sorted.
    pub fn home_projects(&self) -> Vec<String> {
        let mut homes: Vec<String> = self
            .foreign
            .iter()
            .map(|r| r.home_project.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        homes.sort();
        homes
    }

    /// One-line summary used as the `cas doctor` check message.
    pub fn summary(&self) -> String {
        if self.peers_compared.is_empty() {
            // Still not a bare "clean": nothing was compared, and if databases
            // existed but could not be opened, that is the reason — not health.
            let mut summary = format!(
                "0 project DB(s) compared ({} local task row(s), {} DB(s) unreadable) — task replication was not checked",
                self.local_task_count,
                self.peers_unreadable.len(),
            );
            if self.foreign_knowledge_pages.is_empty()
                && self.unattributed_knowledge_pages.is_empty()
            {
                summary.push_str(&format!(
                    "; 0 foreign knowledge page(s): {} attributed page(s) checked",
                    self.local_knowledge_page_count
                ));
            } else {
                if !self.foreign_knowledge_pages.is_empty() {
                    summary.push_str(&format!(
                        "; {} foreign cloud-pulled knowledge page(s)",
                        self.foreign_knowledge_pages.len()
                    ));
                }
                if !self.unattributed_knowledge_pages.is_empty() {
                    summary.push_str(&format!(
                        "; {} knowledge page(s) whose provenance cannot be audited",
                        self.unattributed_knowledge_pages.len()
                    ));
                }
            }
            return summary;
        }
        if self.is_clean() {
            // The honest zero: a bare "clean" is indistinguishable from a scan
            // that compared nothing. Always say what was actually covered —
            // rows scanned, peers compared, peers that could not be read.
            return format!(
                "0 foreign task row(s): {} local row(s) compared against {} project DB(s) on (id,title), {} DB(s) unreadable; 0 foreign knowledge page(s): {} attributed page(s) checked",
                self.local_task_count,
                self.peers_compared.len(),
                self.peers_unreadable.len(),
                self.local_knowledge_page_count,
            );
        }
        let mut parts = Vec::new();
        if !self.foreign.is_empty() {
            let homes = self.home_projects();
            parts.push(format!(
                "{} foreign task row(s) from {} other project(s) ({}), {} of them not closed",
                self.foreign.len(),
                homes.len(),
                homes.join(", "),
                self.foreign_open(),
            ));
        }
        if !self.unattributed.is_empty() {
            parts.push(format!(
                "{} replicated row(s) whose home project cannot be established ({} not closed)",
                self.unattributed.len(),
                self.unattributed_open(),
            ));
        }
        if !self.foreign_knowledge_pages.is_empty() {
            parts.push(format!(
                "{} foreign cloud-pulled knowledge page(s)",
                self.foreign_knowledge_pages.len()
            ));
        }
        if !self.unattributed_knowledge_pages.is_empty() {
            parts.push(format!(
                "{} knowledge page(s) whose provenance cannot be audited",
                self.unattributed_knowledge_pages.len()
            ));
        }
        parts.join("; ")
    }

    /// Safe remediation path (AC1) — always names the `(id,title)` constraint.
    pub fn remediation(&self) -> String {
        let mut text = String::from(
            "Review the full list with `cas doctor --foreign-rows` (read-only), then remediate with \
`cas cloud purge-foreign --dry-run` and re-run without `--dry-run` once the preview looks right; \
it deletes cloud-backed content rows and re-pulls this project's own scope.",
        );
        if !self.collisions.is_empty() {
            text.push_str(&format!(
                " Any hand-written cleanup must match on (id, title): {} id(s) in this scan name a \
different real task in another project, so deleting by id alone destroys live work.",
                self.collisions.len()
            ));
        } else {
            text.push_str(
                " Any hand-written cleanup must match on (id, title) — 4-hex task ids collide \
across projects, so deleting by id alone destroys live work.",
            );
        }
        if !self.foreign_knowledge_pages.is_empty() || !self.unattributed_knowledge_pages.is_empty()
        {
            text.push_str(
                " Knowledge pages are attributed independently: inspect the named rel_path and \
origin_project_id before removing a page, then re-run a scoped pull and this audit.",
            );
        }
        text
    }

    pub fn to_json(&self) -> serde_json::Value {
        let foreign: Vec<_> = self
            .foreign
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "title": r.title,
                    "closed": r.closed,
                    "origin_project": r.origin_project,
                    "home_project": r.home_project,
                    "also_present_in": r.also_present_in,
                })
            })
            .collect();
        let unattributed: Vec<_> = self
            .unattributed
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "title": r.title,
                    "closed": r.closed,
                    "present_in": r.present_in,
                })
            })
            .collect();
        let collisions: Vec<_> = self
            .collisions
            .iter()
            .map(|c| {
                serde_json::json!({
                    "id": c.id,
                    "local_title": c.local_title,
                    "other_project": c.other_project,
                    "other_title": c.other_title,
                })
            })
            .collect();
        let unreadable: Vec<_> = self
            .peers_unreadable
            .iter()
            .map(|p| {
                serde_json::json!({
                    "project": p.project,
                    "db_path": p.db_path.to_string_lossy(),
                    "error": p.error,
                })
            })
            .collect();
        serde_json::json!({
            "local_project": self.local_project,
            "local_task_count": self.local_task_count,
            "peers_compared": self.peers_compared,
            "peers_unreadable": unreadable,
            "identity_key": "(id, title)",
            "foreign": {
                "total": self.foreign.len(),
                "not_closed": self.foreign_open(),
                "closed": self.foreign_closed(),
                "rows": foreign,
            },
            "unattributed": {
                "total": self.unattributed.len(),
                "not_closed": self.unattributed_open(),
                "closed": self.unattributed_closed(),
                "rows": unattributed,
            },
            "id_collisions": {
                "total": self.collisions.len(),
                "rows": collisions,
            },
            "locally_worked_replicas": self.locally_worked_replicas,
            "knowledge_pages": {
                "local_count": self.local_knowledge_page_count,
                "foreign_total": self.foreign_knowledge_pages.len(),
                "foreign_rows": self.foreign_knowledge_pages.iter().map(|page| serde_json::json!({
                    "id": page.id,
                    "title": page.title,
                    "rel_path": page.rel_path,
                    "origin": "cloud_pull",
                    "origin_project_id": page.origin_project_id,
                })).collect::<Vec<_>>(),
                "unattributed_total": self.unattributed_knowledge_pages.len(),
                "unattributed_rows": self.unattributed_knowledge_pages.iter().map(|page| serde_json::json!({
                    "id": page.id,
                    "title": page.title,
                    "rel_path": page.rel_path,
                    "reason": page.reason,
                })).collect::<Vec<_>>(),
                "identity_predicate": "byte-exact project_canonical_id/project_id match (shared with cloud pull ingest)",
            },
            "clean": self.is_clean(),
            "remediation": self.remediation(),
        })
    }
}

/// Classify every local row against the peer snapshots. Pure: no I/O, so the
/// collision and attribution rules are testable without databases.
pub fn classify(local: &DbSnapshot, peers: &[DbSnapshot]) -> ForeignRowReport {
    // id -> [(peer index, row)] across all peers, so a single pass over local
    // rows can find both same-title replicas and same-id collisions.
    let mut by_id: HashMap<&str, Vec<(usize, &TaskRow)>> = HashMap::new();
    for (peer_idx, peer) in peers.iter().enumerate() {
        for row in &peer.tasks {
            by_id
                .entry(row.id.as_str())
                .or_default()
                .push((peer_idx, row));
        }
    }

    let mut report = ForeignRowReport {
        local_project: local.project.clone(),
        local_task_count: local.tasks.len(),
        peers_compared: peers.iter().map(|p| p.project.clone()).collect(),
        local_knowledge_page_count: local.knowledge_pages.len(),
        ..Default::default()
    };
    // Deduplicate collisions: the same (id, other project, other title) pair can
    // be reached from several local rows only if the local DB itself has
    // duplicate ids (it cannot — id is the primary key), but a peer may hold
    // several differing titles for one id.
    let mut collisions: BTreeMap<(String, String, String), IdCollision> = BTreeMap::new();

    for row in &local.tasks {
        let Some(candidates) = by_id.get(row.id.as_str()) else {
            continue; // unique id across the host — nothing to say about it
        };

        let mut same_title_peers: Vec<usize> = Vec::new();
        for (peer_idx, peer_row) in candidates {
            if peer_row.key_title() == row.key_title() {
                same_title_peers.push(*peer_idx);
            } else {
                let peer = &peers[*peer_idx];
                let collision = IdCollision {
                    id: row.id.clone(),
                    local_title: row.title.clone(),
                    other_project: peer.project.clone(),
                    other_title: peer_row.title.clone(),
                };
                collisions.insert(
                    (
                        collision.id.clone(),
                        collision.other_project.clone(),
                        collision.other_title.clone(),
                    ),
                    collision,
                );
            }
        }

        if same_title_peers.is_empty() {
            continue;
        }
        same_title_peers.sort_unstable();
        same_title_peers.dedup();

        // Worked here => native, whatever any peer holds.
        if local.worked_task_ids.contains(&row.id) {
            report.locally_worked_replicas += 1;
            continue;
        }

        let home = same_title_peers
            .iter()
            .copied()
            .find(|idx| peers[*idx].worked_task_ids.contains(&row.id));

        let present_in: Vec<String> = same_title_peers
            .iter()
            .map(|idx| peers[*idx].project.clone())
            .collect();

        match home {
            Some(home_idx) => report.foreign.push(ForeignRow {
                id: row.id.clone(),
                title: row.title.clone(),
                closed: row.closed,
                origin_project: row.origin_project.clone(),
                home_project: peers[home_idx].project.clone(),
                also_present_in: present_in
                    .iter()
                    .filter(|p| **p != peers[home_idx].project)
                    .cloned()
                    .collect(),
            }),
            None => report.unattributed.push(UnattributedRow {
                id: row.id.clone(),
                title: row.title.clone(),
                closed: row.closed,
                present_in,
            }),
        }
    }

    report.collisions = collisions.into_values().collect();
    report
}

/// Add direct knowledge-page attribution to a task-replica report.
///
/// `entity_matches_project` is intentionally the same byte-exact, fail-closed
/// predicate used before cloud-pull ingest. Doctor must never grow a parallel
/// interpretation of project identity that disagrees with the write guard.
pub fn classify_knowledge_pages(
    report: &mut ForeignRowReport,
    pages: &[KnowledgePageAttributionRow],
    current_project_id: Option<&str>,
) {
    for page in pages {
        match page.origin.as_str() {
            "local" => {}
            "cloud_pull" => {
                let Some(current_project_id) = current_project_id else {
                    report
                        .unattributed_knowledge_pages
                        .push(UnattributedKnowledgePage {
                            id: page.id.clone(),
                            title: page.title.clone(),
                            rel_path: page.rel_path.clone(),
                            reason: "current project canonical id could not be resolved"
                                .to_string(),
                        });
                    continue;
                };
                let raw = serde_json::json!({
                    "id": page.id,
                    "project_canonical_id": page.origin_project_id,
                });
                if !crate::cloud::entity_matches_project(
                    &raw,
                    current_project_id,
                    "knowledge page attribution",
                ) {
                    report.foreign_knowledge_pages.push(ForeignKnowledgePage {
                        id: page.id.clone(),
                        title: page.title.clone(),
                        rel_path: page.rel_path.clone(),
                        origin_project_id: page.origin_project_id.clone(),
                    });
                }
            }
            other => report
                .unattributed_knowledge_pages
                .push(UnattributedKnowledgePage {
                    id: page.id.clone(),
                    title: page.title.clone(),
                    rel_path: page.rel_path.clone(),
                    reason: format!("unknown origin '{other}'"),
                }),
        }
    }
}

/// Open a project database read-only and reduce it to a [`DbSnapshot`].
///
/// Read-only by construction: `SQLITE_OPEN_READ_ONLY` without `CREATE`, so a
/// missing or unreadable file is an error rather than a freshly created empty
/// database that would report "no contamination".
pub fn read_snapshot(db_path: &Path, project: &str) -> anyhow::Result<DbSnapshot> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_millis(250))?;

    let task_columns = conn
        .prepare("PRAGMA table_info(tasks)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    let origin_column = task_columns.iter().any(|column| column == "origin_project");
    let task_sql = if origin_column {
        "SELECT id, title, status, origin_project FROM tasks"
    } else {
        "SELECT id, title, status, NULL FROM tasks"
    };
    let mut stmt = conn.prepare(task_sql)?;
    let mut tasks = Vec::new();
    let rows = stmt.query_map([], |row| {
        Ok(TaskRow {
            id: row.get::<_, String>(0)?,
            title: row.get::<_, String>(1)?,
            origin_project: row.get::<_, Option<String>>(3)?,
            closed: row.get::<_, String>(2)?.eq_ignore_ascii_case("closed"),
        })
    })?;
    for row in rows {
        // No `.flatten()`: a row that fails to decode must not vanish into a
        // shorter list that reads as less contamination than there is.
        tasks.push(row?);
    }
    drop(stmt);

    let mut worked_task_ids = BTreeSet::new();
    for table in LOCAL_ACTIVITY_TABLES {
        let sql = format!("SELECT DISTINCT task_id FROM {table} WHERE task_id IS NOT NULL");
        let mut stmt = match conn.prepare(&sql) {
            Ok(stmt) => stmt,
            // Table absent in an older database — that kind of evidence simply
            // does not exist here. Any other prepare failure is real.
            Err(e) if is_missing_object(&e) => continue,
            Err(e) => return Err(e.into()),
        };
        let ids = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for id in ids {
            worked_task_ids.insert(id?);
        }
    }

    let has_knowledge_pages: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='knowledge_pages')",
        [],
        |row| row.get(0),
    )?;
    let mut knowledge_pages = Vec::new();
    if has_knowledge_pages {
        let has_attribution: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('knowledge_pages') WHERE name='origin')
             AND EXISTS(SELECT 1 FROM pragma_table_info('knowledge_pages') WHERE name='origin_project_id')",
            [],
            |row| row.get(0),
        )?;
        let sql = if has_attribution {
            "SELECT id, title, rel_path, origin, origin_project_id FROM knowledge_pages"
        } else {
            "SELECT id, title, rel_path, 'legacy_unattributed', NULL FROM knowledge_pages"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |row| {
            Ok(KnowledgePageAttributionRow {
                id: row.get(0)?,
                title: row.get(1)?,
                rel_path: row.get(2)?,
                origin: row.get(3)?,
                origin_project_id: row.get(4)?,
            })
        })?;
        for row in rows {
            knowledge_pages.push(row?);
        }
    }

    Ok(DbSnapshot {
        project: project.to_string(),
        db_path: db_path.to_path_buf(),
        tasks,
        worked_task_ids,
        knowledge_pages,
    })
}

fn is_missing_object(err: &rusqlite::Error) -> bool {
    let text = err.to_string();
    text.contains("no such table") || text.contains("no such column")
}

/// Human label for a project root: the directory name, falling back to the
/// full path when the name is empty.
fn project_label(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| root.to_string_lossy().to_string())
}

/// Label each root, falling back to the full path where a bare directory name
/// would be ambiguous.
///
/// Two checkouts can share a directory name (`.../client-a/Accounting` and
/// `.../archive/Accounting`). A report that blames "Accounting" when two
/// different Accountings exist cannot be acted on.
fn disambiguated_labels(roots: &[PathBuf]) -> Vec<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for root in roots {
        *counts.entry(project_label(root)).or_default() += 1;
    }
    roots
        .iter()
        .map(|root| {
            let short = project_label(root);
            if counts.get(&short).copied().unwrap_or(0) > 1 {
                root.to_string_lossy().to_string()
            } else {
                short
            }
        })
        .collect()
}

/// Peer project roots to compare against: the host `known_repos` registry,
/// minus this project and minus roots without a database on disk.
fn peer_roots(local_db: &Path) -> Vec<PathBuf> {
    let local_canonical = local_db
        .canonicalize()
        .unwrap_or_else(|_| local_db.to_path_buf());
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();

    use crate::store::KnownRepoStore as _;

    let registry = match crate::store::known_repos::open_host_known_repo_store() {
        Ok(store) => store.list().unwrap_or_default(),
        // A host that never ran `cas init` has no registry: nothing to compare.
        Err(_) => return out,
    };

    for repo in registry {
        let db = repo.path.join(".cas").join("cas.db");
        if !db.is_file() {
            continue; // repo deleted, moved, or never initialized
        }
        let canonical = db.canonicalize().unwrap_or_else(|_| db.clone());
        if canonical == local_canonical {
            continue; // this project
        }
        if seen.insert(canonical) {
            out.push(repo.path);
        }
    }
    out
}

/// Run the full read-only scan for the project rooted at `cas_root`
/// (the `.cas` directory).
pub fn scan(cas_root: &Path) -> anyhow::Result<ForeignRowReport> {
    let local_db = cas_root.join("cas.db");
    let local_project = cas_root
        .parent()
        .map(project_label)
        .unwrap_or_else(|| cas_root.to_string_lossy().to_string());
    let local = read_snapshot(&local_db, &local_project)?;

    let roots = peer_roots(&local_db);
    let labels = disambiguated_labels(&roots);

    let mut peers = Vec::new();
    let mut unreadable = Vec::new();
    for (root, label) in roots.into_iter().zip(labels) {
        let db = root.join(".cas").join("cas.db");
        match read_snapshot(&db, &label) {
            Ok(snapshot) => peers.push(snapshot),
            Err(e) => unreadable.push(UnreadablePeer {
                project: label,
                db_path: db,
                error: e.to_string(),
            }),
        }
    }
    peers.sort_by(|a, b| a.project.cmp(&b.project));

    let mut report = classify(&local, &peers);
    let project_id = crate::cloud::resolve_canonical_id(cas_root);
    classify_knowledge_pages(&mut report, &local.knowledge_pages, project_id.as_deref());
    report.peers_unreadable = unreadable;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attributed_fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("cas.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT NOT NULL, status TEXT NOT NULL);
             CREATE TABLE knowledge_pages (
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL,
                 rel_path TEXT NOT NULL,
                 origin TEXT NOT NULL,
                 origin_project_id TEXT
             )",
        )
        .unwrap();
        drop(conn);
        (temp, db)
    }

    fn task(id: &str, title: &str, closed: bool) -> TaskRow {
        TaskRow {
            id: id.to_string(),
            title: title.to_string(),
            origin_project: None,
            closed,
        }
    }

    fn snapshot(project: &str, tasks: Vec<TaskRow>, worked: &[&str]) -> DbSnapshot {
        DbSnapshot {
            project: project.to_string(),
            db_path: PathBuf::from(format!("/tmp/{project}/.cas/cas.db")),
            tasks,
            worked_task_ids: worked.iter().map(|s| s.to_string()).collect(),
            knowledge_pages: Vec::new(),
        }
    }

    #[test]
    fn seeded_foreign_cloud_page_is_detected_post_hoc_with_shared_identity_predicate() {
        let (_temp, db) = attributed_fixture();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "INSERT INTO knowledge_pages (id, title, rel_path, origin, origin_project_id)
             VALUES (?1, ?2, ?3, 'cloud_pull', ?4)",
            rusqlite::params![
                "cas-kn999",
                "Foreign architecture",
                "architecture/foreign.md",
                "github.com/other/project"
            ],
        )
        .unwrap();
        drop(conn);

        let local = read_snapshot(&db, "local").unwrap();
        let mut report = classify(&local, &[]);
        classify_knowledge_pages(
            &mut report,
            &local.knowledge_pages,
            Some("github.com/local/project"),
        );

        assert_eq!(report.local_knowledge_page_count, 1);
        assert_eq!(report.foreign_knowledge_pages.len(), 1);
        assert_eq!(report.foreign_knowledge_pages[0].id, "cas-kn999");
        assert!(!report.is_clean());
        assert!(
            report
                .summary()
                .contains("foreign cloud-pulled knowledge page")
        );
    }

    #[test]
    fn live_shaped_local_backfill_reports_zero_foreign_pages() {
        let (_temp, db) = attributed_fixture();
        let mut conn = rusqlite::Connection::open(&db).unwrap();
        let tx = conn.transaction().unwrap();
        for index in 1..=107 {
            tx.execute(
                "INSERT INTO knowledge_pages (id, title, rel_path, origin, origin_project_id)
                 VALUES (?1, ?2, ?3, 'local', NULL)",
                rusqlite::params![
                    format!("cas-kn{index:03}"),
                    format!("Local page {index}"),
                    format!("guide/local-page-{index}.md")
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        drop(conn);

        let local = read_snapshot(&db, "local").unwrap();
        let mut report = classify(&local, &[]);
        classify_knowledge_pages(
            &mut report,
            &local.knowledge_pages,
            Some("github.com/local/project"),
        );

        assert_eq!(report.local_knowledge_page_count, 107);
        assert!(report.foreign_knowledge_pages.is_empty());
        assert!(report.unattributed_knowledge_pages.is_empty());
        assert!(report.is_clean());
        assert!(report.summary().contains("107 attributed page(s) checked"));
    }

    /// AC2: the measured failure mode — two real, different tasks sharing a
    /// 4-hex id. Detection must never call either one a replica of the other.
    #[test]
    fn id_collision_is_never_reported_as_a_foreign_row_cas_fc6fa() {
        let local = snapshot(
            "cas-src",
            vec![task("cas-1234", "Fix the pull scoping regression", false)],
            &[],
        );
        // Same id, entirely different task, and the peer even has activity
        // evidence for it — the tempting-but-wrong "id + peer worked it" rule
        // would delete live local work here.
        let peers = vec![snapshot(
            "accounting",
            vec![task("cas-1234", "Reconcile Q3 payroll ledger", true)],
            &["cas-1234"],
        )];

        let report = classify(&local, &peers);

        assert!(report.foreign.is_empty(), "collision must not be foreign");
        assert!(report.unattributed.is_empty());
        assert_eq!(report.collisions.len(), 1);
        assert_eq!(report.collisions[0].id, "cas-1234");
        assert_eq!(report.collisions[0].other_project, "accounting");
        assert!(report.remediation().contains("(id, title)"));
    }

    /// A row that is a genuine replica *and* collides with a third project's
    /// unrelated task must be both attributed and flagged.
    #[test]
    fn mixed_id_is_attributed_on_title_and_still_reports_the_collision_cas_fc6fa() {
        let local = snapshot(
            "cas-src",
            vec![task("cas-abcd", "Ship the invoice export", false)],
            &[],
        );
        let peers = vec![
            snapshot(
                "accounting",
                vec![task("cas-abcd", "Ship the invoice export", false)],
                &["cas-abcd"],
            ),
            snapshot(
                "ozer",
                vec![task("cas-abcd", "Rotate the API keys", true)],
                &[],
            ),
        ];

        let report = classify(&local, &peers);

        assert_eq!(report.foreign.len(), 1);
        assert_eq!(report.foreign[0].home_project, "accounting");
        assert_eq!(report.collisions.len(), 1);
        assert_eq!(report.collisions[0].other_project, "ozer");
    }

    /// AC3: closed replicas are noise; non-closed replicas are the rows that
    /// lie in ready queues, so the counts must be reported separately.
    #[test]
    fn closed_and_non_closed_foreign_rows_are_counted_separately_cas_fc6fa() {
        let local = snapshot(
            "cas-src",
            vec![
                task("cas-0001", "Frozen replica of finished work", true),
                task("cas-0002", "Foreign row lying in the ready queue", false),
                task("cas-0003", "Native cas-src task", false),
            ],
            &["cas-0003"],
        );
        let peers = vec![snapshot(
            "accounting",
            vec![
                task("cas-0001", "Frozen replica of finished work", true),
                task("cas-0002", "Foreign row lying in the ready queue", false),
            ],
            &["cas-0001", "cas-0002"],
        )];

        let report = classify(&local, &peers);

        assert_eq!(report.foreign.len(), 2);
        assert_eq!(report.foreign_open(), 1);
        assert_eq!(report.foreign_closed(), 1);
        assert!(report.summary().contains("1 of them not closed"));
        assert!(!report.is_clean());
    }

    /// Evidence is asymmetric: a row worked *here* is native even when a peer
    /// also holds it (this project's own rows leaked outward too).
    #[test]
    fn locally_worked_rows_are_never_foreign_cas_fc6fa() {
        let local = snapshot(
            "cas-src",
            vec![task("cas-aaaa", "Local work", false)],
            &["cas-aaaa"],
        );
        let peers = vec![snapshot(
            "accounting",
            vec![task("cas-aaaa", "Local work", false)],
            &["cas-aaaa"],
        )];

        let report = classify(&local, &peers);

        assert!(report.foreign.is_empty());
        assert!(report.unattributed.is_empty());
        assert_eq!(report.locally_worked_replicas, 1);
        assert!(report.is_clean());
    }

    /// No evidence anywhere: report the replication honestly instead of
    /// guessing a home project.
    #[test]
    fn replicas_without_evidence_anywhere_are_unattributed_not_foreign_cas_fc6fa() {
        let local = snapshot(
            "cas-src",
            vec![task("cas-bbbb", "Never leased anywhere", false)],
            &[],
        );
        let peers = vec![
            snapshot(
                "accounting",
                vec![task("cas-bbbb", "Never leased anywhere", false)],
                &[],
            ),
            snapshot(
                "ozer",
                vec![task("cas-bbbb", "Never leased anywhere", false)],
                &[],
            ),
        ];

        let report = classify(&local, &peers);

        assert!(report.foreign.is_empty());
        assert_eq!(report.unattributed.len(), 1);
        assert_eq!(
            report.unattributed[0].present_in,
            vec!["accounting", "ozer"]
        );
        assert_eq!(report.unattributed_open(), 1);
        assert!(report.summary().contains("cannot be established"));
    }

    /// Titles differing only in surrounding whitespace are the same task, not
    /// an id collision.
    #[test]
    fn titles_match_after_trimming_cas_fc6fa() {
        let local = snapshot("cas-src", vec![task("cas-cccc", "Same task\n", false)], &[]);
        let peers = vec![snapshot(
            "accounting",
            vec![task("cas-cccc", "Same task", false)],
            &["cas-cccc"],
        )];

        let report = classify(&local, &peers);

        assert_eq!(report.foreign.len(), 1);
        assert!(report.collisions.is_empty());
    }

    /// The honest zero. A bare "clean" is indistinguishable from a scan that
    /// compared nothing, so the zero case must state its own coverage: rows
    /// scanned, peer DBs compared, peer DBs that could not be read.
    #[test]
    fn a_clean_scan_states_what_it_actually_covered_cas_fc6fa() {
        let local = snapshot("cas-src", vec![task("cas-dddd", "Only here", false)], &[]);
        let peers = vec![
            snapshot(
                "accounting",
                vec![task("cas-eeee", "Elsewhere", false)],
                &[],
            ),
            snapshot("ozer", vec![task("cas-ffff", "Also elsewhere", false)], &[]),
        ];

        let mut report = classify(&local, &peers);
        report.peers_unreadable = vec![UnreadablePeer {
            project: "pantheon".to_string(),
            db_path: PathBuf::from("/home/u/pantheon/.cas/cas.db"),
            error: "file is not a database".to_string(),
        }];

        assert!(report.is_clean());
        let summary = report.summary();
        assert!(summary.contains("0 foreign task row(s)"), "{summary}");
        assert!(summary.contains("1 local row(s)"), "{summary}");
        assert!(summary.contains("2 project DB(s)"), "{summary}");
        assert!(summary.contains("1 DB(s) unreadable"), "{summary}");
        assert_eq!(report.to_json()["identity_key"], "(id, title)");
    }

    /// A machine with a single project must not read as "checked and clean".
    #[test]
    fn a_scan_with_no_peers_distinguishes_task_and_page_coverage_cas_fc6fa() {
        let local = snapshot("cas-src", vec![task("cas-dddd", "Only here", false)], &[]);

        let report = classify(&local, &[]);

        assert!(report.is_clean());
        let summary = report.summary();
        assert!(summary.contains("0 project DB(s) compared"), "{summary}");
        assert!(summary.contains("task replication was not checked"), "{summary}");
        assert!(summary.contains("0 attributed page(s) checked"), "{summary}");
    }

    /// Same folder name in two places must not collapse into one accusation.
    #[test]
    fn peers_sharing_a_directory_name_are_labelled_by_full_path_cas_fc6fa() {
        let roots = vec![
            PathBuf::from("/home/u/client-a/Accounting"),
            PathBuf::from("/mnt/archive/Accounting"),
            PathBuf::from("/home/u/cas-src"),
        ];

        let labels = disambiguated_labels(&roots);

        assert_eq!(
            labels,
            vec![
                "/home/u/client-a/Accounting".to_string(),
                "/mnt/archive/Accounting".to_string(),
                "cas-src".to_string(),
            ]
        );
    }

    /// End-to-end over real SQLite files, including a database that predates
    /// the activity tables entirely.
    #[test]
    fn read_snapshot_reads_tasks_and_activity_evidence_cas_fc6fa() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cas.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT NOT NULL, status TEXT NOT NULL);
                 INSERT INTO tasks VALUES ('cas-1111', 'Worked here', 'open');
                 INSERT INTO tasks VALUES ('cas-2222', 'Merely resident', 'closed');
                 CREATE TABLE task_lease_history (task_id TEXT NOT NULL);
                 INSERT INTO task_lease_history VALUES ('cas-1111');",
            )
            .unwrap();
        }

        let snap = read_snapshot(&db, "fixture").unwrap();

        assert_eq!(snap.tasks.len(), 2);
        assert!(snap.tasks.iter().any(|t| t.id == "cas-2222" && t.closed));
        assert!(snap.worked_task_ids.contains("cas-1111"));
        assert!(!snap.worked_task_ids.contains("cas-2222"));
        // Absent activity tables are tolerated, not fatal.
        assert_eq!(snap.worked_task_ids.len(), 1);
    }

    /// cas-647c: `~/.cas/artifacts/cas-1bfb/fresh-proxy` is a copied CAS root
    /// used as a proxy-health fixture — 10 tables, no `tasks`. It is not a
    /// project store, so there is nothing to compare and nothing is wrong with
    /// it. Peer enumeration must say "skipped", not "could NOT be read".
    #[test]
    fn a_peer_database_without_a_tasks_table_is_skipped_not_unreadable_cas_647c() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cas.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE proxy_health (id TEXT PRIMARY KEY, checked_at TEXT NOT NULL);",
            )
            .unwrap();
        }

        assert!(
            read_peer_snapshot(&db, "fresh-proxy").unwrap().is_none(),
            "a database with no tasks table is not a peer project store"
        );
        // A genuinely broken file is still an error, not a silent skip.
        let broken = dir.path().join("broken.db");
        std::fs::write(&broken, b"this is not a database").unwrap();
        assert!(read_peer_snapshot(&broken, "broken").is_err());
    }

    /// A real project store still reads through the peer entry point.
    #[test]
    fn read_peer_snapshot_reads_a_real_project_store_cas_647c() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cas.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT NOT NULL, status TEXT NOT NULL);
                 INSERT INTO tasks VALUES ('cas-1111', 'Real work', 'open');",
            )
            .unwrap();
        }

        let snapshot = read_peer_snapshot(&db, "accounting").unwrap().unwrap();
        assert_eq!(snapshot.tasks.len(), 1);
        assert_eq!(snapshot.project, "accounting");
    }

    /// The skipped set is coverage bookkeeping, never a defect: it must appear
    /// in the summary and the JSON, and must not make the report unclean.
    #[test]
    fn skipped_peers_are_informational_and_never_the_warn_driver_cas_647c() {
        let local = snapshot("cas-src", vec![task("cas-dddd", "Only here", false)], &[]);
        let peers = vec![snapshot(
            "accounting",
            vec![task("cas-eeee", "Elsewhere", false)],
            &[],
        )];

        let mut report = classify(&local, &peers);
        report.peers_skipped = vec![SkippedPeer {
            project: "fresh-proxy".to_string(),
            db_path: PathBuf::from("/home/u/.cas/artifacts/cas-1bfb/fresh-proxy/.cas/cas.db"),
            reason: NOT_A_PROJECT_STORE.to_string(),
        }];

        assert!(report.is_clean(), "a skipped non-store is not contamination");
        assert!(report.peers_unreadable.is_empty());
        let summary = report.summary();
        assert!(
            summary.contains("1 registry root(s) skipped (not a project store): fresh-proxy"),
            "{summary}"
        );
        assert!(!summary.contains("could NOT be read"), "{summary}");
        assert_eq!(
            report.to_json()["peers_skipped"][0]["project"],
            "fresh-proxy"
        );
    }

    /// The zero-peer branch of the summary must carry the skipped clause too —
    /// a host whose only other registry row is a scratch copy would otherwise
    /// report "0 project DB(s) compared" with no explanation of why.
    #[test]
    fn a_scan_with_only_skipped_roots_says_why_nothing_was_compared_cas_647c() {
        let local = snapshot("cas-src", vec![task("cas-dddd", "Only here", false)], &[]);
        let mut report = classify(&local, &[]);
        report.peers_skipped = vec![SkippedPeer {
            project: "fresh-proxy".to_string(),
            db_path: PathBuf::from("/home/u/.cas/artifacts/cas-1bfb/fresh-proxy/.cas/cas.db"),
            reason: NOT_A_PROJECT_STORE.to_string(),
        }];

        let summary = report.summary();
        assert!(summary.contains("0 project DB(s) compared"), "{summary}");
        assert!(
            summary.contains("1 registry root(s) skipped (not a project store)"),
            "{summary}"
        );
    }

    /// The scan must never create or modify a database it inspects.
    #[test]
    fn read_snapshot_refuses_a_missing_database_instead_of_creating_one_cas_fc6fa() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cas.db");

        assert!(read_snapshot(&db, "fixture").is_err());
        assert!(!db.exists(), "read-only scan must not create the database");
    }
}
