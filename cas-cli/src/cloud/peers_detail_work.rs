//! Inventory fields for the existing peer projection. No network or registry writes.
use super::*;
use serde::Deserialize;
use std::collections::BTreeMap;

/// A section's evidence age is independent of the supervisor's heartbeat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationStatus {
    pub state: String,
    pub observed_at: Option<DateTime<Utc>>,
    pub reason: Option<String>,
}

impl ObservationStatus {
    pub(super) fn at(state: &str, at: Option<DateTime<Utc>>, reason: Option<&str>) -> Self {
        Self {
            state: state.into(),
            observed_at: at,
            reason: reason.map(str::to_string),
        }
    }
    fn observed(at: DateTime<Utc>, now: DateTime<Utc>) -> Self {
        let state = if at > now + chrono::Duration::seconds(30) {
            "unavailable"
        } else if (now - at).num_seconds() > peers::PEER_LIVE_SECS {
            "stale"
        } else {
            "ready"
        };
        Self::at(state, Some(at), None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerPr {
    pub url: Option<String>,
    pub head_sha: String,
    pub branch: String,
    pub number: u64,
    pub state: String,
    pub in_merge_queue: bool,
}

/// The existing CI poll supplies these records. The timestamp never advances
/// merely because presence heartbeats succeeded. No task locks are published.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PrSnapshot {
    pub observed_at: DateTime<Utc>,
    pub truncated: bool,
    pub prs: Vec<PeerPr>,
}

pub(crate) const PR_META: &str = "peer_prs_v1";
const MAX_SNAPSHOT_BYTES: u64 = 32 * 1024;
const MAX_PRS: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerSplit {
    pub id: String,
    pub epic_id: String,
    pub peer_agent_id: String,
    pub state: String,
    pub mine: Vec<String>,
    pub theirs: Vec<String>,
    pub expires_at: DateTime<Utc>,
    pub target_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SplitSnapshot {
    observed_at: DateTime<Utc>,
    splits: Vec<PeerSplit>,
}

fn read_snapshot(root: &Path) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(root.join("peer-pr-observations.json")).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_SNAPSHOT_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_SNAPSHOT_BYTES)
        .then(|| String::from_utf8(bytes).ok())
        .flatten()
}

/// Called only from the CI watcher's existing blocking job. Retain actual PR
/// URLs/head/queue evidence, without adding any remote request to the poll.
pub(crate) fn save_pr_snapshot(root: &Path, mut prs: Vec<PeerPr>, now: DateTime<Utc>) {
    let mut truncated = prs.len() >= MAX_PRS; // existing GitHub query has first:100
    prs.truncate(MAX_PRS);
    let bytes = loop {
        let snapshot = PrSnapshot {
            observed_at: now,
            truncated,
            prs: prs.clone(),
        };
        let Ok(bytes) = serde_json::to_vec(&snapshot) else {
            return;
        };
        if bytes.len() as u64 <= MAX_SNAPSHOT_BYTES {
            break bytes;
        }
        truncated = true;
        if prs.pop().is_none() {
            return;
        }
    };
    let path = root.join("peer-pr-observations.json");
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    if std::fs::write(&tmp, bytes)
        .and_then(|_| std::fs::rename(&tmp, &path))
        .is_err()
    {
        let _ = std::fs::remove_file(tmp);
        tracing::debug!("could not persist peer PR observations");
    }
}

/// Presence already executes on its coalesced blocking worker. Only local
/// config and the existing CI observation file are read here. Unknown roots
/// remain unknown; a shared scratch base is explicitly labelled as shared.
pub(crate) fn presence_agent(root: &Path, agent: &crate::types::Agent) -> crate::types::Agent {
    let mut agent = agent.clone();
    let worktree = agent
        .metadata
        .get("clone_path")
        .cloned()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| root.parent().map(|p| p.to_string_lossy().into_owned()));
    if let Some(path) = worktree {
        agent.metadata.insert("clone_path".into(), path);
    }
    let scratch = crate::config::Config::load(root)
        .ok()
        .and_then(|c| c.staging)
        .and_then(|s| s.scratch_root);
    agent.metadata.insert(
        "scratch_scope".into(),
        if scratch.is_some() {
            "configured"
        } else {
            "shared base"
        }
        .into(),
    );
    agent.metadata.insert(
        "scratch_root".into(),
        scratch.unwrap_or_else(|| {
            crate::temp_hygiene::scratch_root_base()
                .to_string_lossy()
                .into_owned()
        }),
    );
    // Absence clears a previously published value on the next heartbeat.
    if agent.role != crate::types::AgentRole::Supervisor {
        agent.metadata.insert(PR_META.into(), String::new());
        agent
            .metadata
            .insert("peer_splits_v1".into(), String::new());
        return agent;
    }
    agent
        .metadata
        .insert(PR_META.into(), read_snapshot(root).unwrap_or_default());
    // GH #1319: a supervisor whose harness is logged out still heartbeats,
    // so peers would read it as live and their mail as delivered. Publish the
    // harness's own refusal; its next answered turn clears it.
    let auth_failure =
        match crate::mcp::tools::service::factory_ops::supervisor_auth_failure_evidence(
            root, &agent,
        ) {
            crate::factory_auth_health::AuthFailureEvidence::Failed { message, .. } => message,
            _ => String::new(),
        };
    agent.metadata.insert(
        crate::cloud::peers::META_HARNESS_AUTH_FAILED.into(),
        auth_failure,
    );
    // Split records are existing repository collaboration records, never
    // private task lock payloads or titles. An unreadable inventory clears
    // old metadata rather than publishing a fresh empty section.
    let tasks = crate::store::open_task_store_local(root).ok();
    let splits = cas_store::list_epic_splits(root)
        .ok()
        .map(|rows| SplitSnapshot {
            observed_at: Utc::now(),
            splits: rows
                .into_iter()
                .filter(|s| s.local_agent_id == agent.id)
                .map(|s| PeerSplit {
                    target_branch: tasks
                        .as_ref()
                        .and_then(|tasks| tasks.get(&s.epic_id).ok())
                        .and_then(|t| {
                            t.deliverables
                                .work_target
                                .map(|w| w.target_branch)
                                .or(t.branch)
                        }),
                    state: s.display_state(Utc::now()).into(),
                    id: s.id,
                    epic_id: s.epic_id,
                    peer_agent_id: s.peer_agent_id,
                    mine: s.mine,
                    theirs: s.theirs,
                    expires_at: s.expires_at,
                })
                .collect(),
        })
        .and_then(|s| serde_json::to_string(&s).ok())
        .filter(|s| s.len() as u64 <= MAX_SNAPSHOT_BYTES);
    agent
        .metadata
        .insert("peer_splits_v1".into(), splits.unwrap_or_default());
    agent
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerTask {
    pub status: String,
    /// Only an active cloud lease establishes the owner. Assignment/activity
    /// are observations, kept separate even for parked deliveries.
    pub owner: Option<String>,
    pub task_id: String,
    pub assignee: Option<String>,
    pub title: Option<String>,
    pub target_branch: Option<String>,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    pub claim: Option<PeerClaim>,
    pub observed_at: Option<DateTime<Utc>>,
    pub pr: Option<PeerPr>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WaitingMerge {
    pub kind: String,
    pub owner: Option<String>,
    pub task_id: Option<String>,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    pub pr_url: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerWork {
    pub inventory_scope: String,
    pub worktree_root: Option<String>,
    pub scratch_root: Option<String>,
    pub scratch_scope: Option<String>,
    pub observations_status: ObservationStatus,
    pub claims_status: ObservationStatus,
    pub tasks_status: ObservationStatus,
    pub prs_status: ObservationStatus,
    pub splits_status: ObservationStatus,
    pub splits: Vec<PeerSplit>,
    pub epics: Vec<PeerTask>,
    pub tasks: Vec<PeerTask>,
    pub prs: Vec<PeerPr>,
    pub waiting_merges: Vec<WaitingMerge>,
}

impl PeerWork {
    pub(super) fn new(
        peer: &Peer,
        owner: Option<&AgentInfo>,
        visibility: ClaimsVisibility,
        now: DateTime<Utc>,
    ) -> Self {
        let observed = ObservationStatus::observed(peer.last_heartbeat, now);
        Self {
            inventory_scope: "claims, epic children and unambiguous parked assignments".into(),
            worktree_root: owner.and_then(|a| meta(a, "clone_path").map(str::to_string)),
            scratch_root: owner.and_then(|a| meta(a, "scratch_root").map(str::to_string)),
            scratch_scope: owner.and_then(|a| meta(a, "scratch_scope").map(str::to_string)),
            observations_status: if !peer.live {
                ObservationStatus::at("stale", Some(peer.last_heartbeat), None)
            } else {
                observed
            },
            claims_status: ObservationStatus::at(
                match visibility {
                    ClaimsVisibility::SameAccount | ClaimsVisibility::TeamProject if !peer.live => {
                        "stale"
                    }
                    ClaimsVisibility::SameAccount | ClaimsVisibility::TeamProject => "ready",
                    ClaimsVisibility::DifferentAccount => "private",
                    ClaimsVisibility::Unknown => "unavailable",
                },
                Some(now),
                None,
            ),
            tasks_status: ObservationStatus::at(
                "unavailable",
                Some(now),
                Some("task records not observed"),
            ),
            prs_status: ObservationStatus::at(
                "unavailable",
                Some(now),
                Some("no PR observation published; details unknown"),
            ),
            splits_status: ObservationStatus::at(
                "unavailable",
                Some(now),
                Some("no split observation published"),
            ),
            splits: Vec::new(),
            epics: Vec::new(),
            tasks: Vec::new(),
            prs: Vec::new(),
            waiting_merges: Vec::new(),
        }
    }
}

pub(super) fn inventory(
    detail: &mut PeerDetail,
    owner: Option<&AgentInfo>,
    store: &dyn TaskStore,
    agents: &[AgentInfo],
    aliases: &[String],
    now: DateTime<Utc>,
) {
    let work = &mut detail.work;
    // Private claims never become an apparently empty task/merge inventory.
    if !detail.claims_visibility.visible() {
        work.tasks_status = work.claims_status.clone();
        work.prs_status = work.claims_status.clone();
        work.splits_status = work.claims_status.clone();
        return;
    }
    if let Some(snapshot) = owner
        .and_then(|a| meta(a, "peer_splits_v1"))
        .filter(|s| s.len() as u64 <= MAX_SNAPSHOT_BYTES)
        .and_then(|s| serde_json::from_str::<SplitSnapshot>(s).ok())
    {
        work.splits_status = ObservationStatus::observed(snapshot.observed_at, now);
        work.splits = snapshot.splits;
    }
    let mut records = BTreeMap::new();
    let mut missing = false;
    for claim in &detail.claims {
        match store.get(&claim.task_id) {
            Ok(task) => {
                if task.task_type == crate::types::TaskType::Epic {
                    match store.get_subtasks(&task.id) {
                        Ok(children) => {
                            for child in children {
                                records.insert(child.id.clone(), child);
                            }
                        }
                        Err(_) => missing = true,
                    }
                }
                records.insert(task.id.clone(), task);
            }
            Err(_) => missing = true,
        }
    }
    // Parking releases a lease. Preserve these existing delivery records as
    // observations without promoting an assignment/name to ownership. A name
    // shared by agents on different machines cannot associate a delivery.
    for status in [
        crate::types::TaskStatus::AwaitingMerge,
        crate::types::TaskStatus::Open,
        crate::types::TaskStatus::InProgress,
        crate::types::TaskStatus::Blocked,
    ] {
        let candidates = match store.list(Some(status)) {
            Ok(rows) => rows,
            Err(_) => {
                missing = true;
                continue;
            }
        };
        for task in candidates.into_iter().filter(|t| {
            t.status == crate::types::TaskStatus::AwaitingMerge
                || t.deliverables.parked_branch.is_some()
        }) {
            let Some(assignee) = task.assignee.as_deref() else {
                continue;
            };
            let member = agents.iter().find(|a| a.id == assignee).or_else(|| {
                let mut matches = agents.iter().filter(|a| a.name == assignee);
                let first = matches.next()?;
                matches.next().is_none().then_some(first)
            });
            if member.is_some_and(|a| {
                a.id == detail.supervisor.agent_id
                    || belongs_to_peer(a, &detail.supervisor, owner, aliases)
            }) {
                records.insert(task.id.clone(), task);
            }
        }
    }
    work.tasks_status = ObservationStatus::at(
        if missing { "unavailable" } else { "ready" },
        Some(now),
        missing.then_some("task inventory unavailable or claimed records not synced"),
    );
    let snapshot = owner
        .and_then(|a| meta(a, PR_META))
        .filter(|s| s.len() as u64 <= MAX_SNAPSHOT_BYTES)
        .and_then(|s| serde_json::from_str::<PrSnapshot>(s).ok())
        .filter(|s| s.prs.len() <= MAX_PRS);
    if let Some(snapshot) = snapshot {
        work.prs_status = ObservationStatus::observed(snapshot.observed_at, now);
        if snapshot.truncated {
            if work.prs_status.state == "ready" {
                work.prs_status.state = "partial".into();
            }
            work.prs_status.reason =
                Some("PR inventory capped; missing detail stays unknown".into());
        }
        let branches: Vec<_> = records
            .values()
            .filter_map(|t| {
                t.deliverables
                    .parked_branch
                    .as_deref()
                    .or(t.branch.as_deref())
            })
            .chain(detail.workers.iter().filter_map(|w| w.branch.as_deref()))
            .chain(detail.branch.as_deref())
            .collect();
        work.prs = snapshot
            .prs
            .into_iter()
            .filter(|p| branches.contains(&p.branch.as_str()))
            .collect();
    }
    for claim in &detail.claims {
        if !records.contains_key(&claim.task_id) {
            work.tasks.push(PeerTask {
                status: claim.status.clone().unwrap_or_else(|| "unknown".into()),
                owner: claim.agent_name.clone(),
                task_id: claim.task_id.clone(),
                assignee: None,
                title: claim.title.clone(),
                target_branch: None,
                branch: None,
                head_sha: None,
                claim: Some(claim.clone()),
                observed_at: None,
                pr: None,
            });
        }
    }
    for task in records.into_values().filter(|t| !t.is_terminal()) {
        let claim = detail.claims.iter().find(|c| c.task_id == task.id).cloned();
        // cas-47a2: before its first commit, a claimed task's branch is the
        // one its live claimant has checked out.
        let branch = task
            .deliverables
            .parked_branch
            .clone()
            .or_else(|| task.branch.clone())
            .or_else(|| {
                let claim = claim.as_ref()?;
                detail
                    .workers
                    .iter()
                    .find(|worker| worker.agent_id == claim.agent_id)?
                    .branch
                    .clone()
            });
        let pr = branch
            .as_ref()
            .and_then(|b| work.prs.iter().find(|p| &p.branch == b))
            .cloned();
        let row = PeerTask {
            status: task.status.to_string(),
            owner: claim.as_ref().and_then(|c| c.agent_name.clone()),
            task_id: task.id.clone(),
            assignee: task.assignee,
            title: Some(task.title),
            target_branch: task
                .deliverables
                .work_target
                .map(|w| w.target_branch)
                .or_else(|| {
                    (task.task_type == crate::types::TaskType::Epic)
                        .then(|| task.branch.clone())
                        .flatten()
                }),
            branch,
            head_sha: task.deliverables.factory_branch_anchor.clone(),
            claim,
            observed_at: Some(task.updated_at),
            pr,
        };
        let mut waiting = Vec::new();
        if task.status == crate::types::TaskStatus::AwaitingMerge {
            waiting.push("awaiting_merge");
        }
        if task.deliverables.parked_branch.is_some() {
            waiting.push("parked_delivery");
        }
        if row.pr.as_ref().is_some_and(|p| p.state == "OPEN")
            && task.task_type == crate::types::TaskType::Epic
        {
            waiting.push("open_epic_pr");
        }
        if row.pr.as_ref().is_some_and(|p| p.in_merge_queue) {
            waiting.push("merge_queue");
        }
        for kind in waiting {
            let pr_observation = matches!(kind, "merge_queue" | "open_epic_pr");
            work.waiting_merges.push(WaitingMerge {
                kind: kind.into(),
                owner: row.owner.clone(),
                task_id: Some(row.task_id.clone()),
                branch: row.branch.clone(),
                // Local delivery anchors and remote PR heads have independent
                // evidence times. Never attach an old PR head to a newer
                // task timestamp and make that observation appear fresh.
                head_sha: if pr_observation {
                    row.pr.as_ref().map(|p| p.head_sha.clone())
                } else {
                    row.head_sha.clone()
                },
                pr_url: row.pr.as_ref().and_then(|p| p.url.clone()),
                observed_at: if pr_observation {
                    work.prs_status.observed_at
                } else {
                    row.observed_at
                },
            });
        }
        if task.task_type == crate::types::TaskType::Epic {
            work.epics.push(row);
        } else {
            work.tasks.push(row);
        }
    }
    for pr in work.prs.iter().filter(|p| p.in_merge_queue) {
        if !work
            .waiting_merges
            .iter()
            .any(|w| w.kind == "merge_queue" && w.branch.as_deref() == Some(&pr.branch))
        {
            work.waiting_merges.push(WaitingMerge {
                kind: "merge_queue".into(),
                owner: None,
                task_id: None,
                branch: Some(pr.branch.clone()),
                head_sha: Some(pr.head_sha.clone()),
                pr_url: pr.url.clone(),
                observed_at: work.prs_status.observed_at,
            });
        }
    }
    if !detail.supervisor.live && work.tasks_status.state == "ready" {
        work.tasks_status.state = "stale".into();
        work.tasks_status.observed_at = Some(detail.supervisor.last_heartbeat);
    }
}

pub(super) fn render_work(work: &PeerWork, line: &mut impl FnMut(String)) {
    let time = |at: Option<DateTime<Utc>>| {
        at.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_else(|| "unknown".into())
    };
    line(format!(
        "  Observations {} at {}",
        work.observations_status.state,
        time(work.observations_status.observed_at)
    ));
    line(format!("  Scope {}", work.inventory_scope));
    line(format!(
        "  Worktree {}",
        work.worktree_root.as_deref().unwrap_or("unknown")
    ));
    line(format!(
        "  Scratch {}",
        work.scratch_root.as_deref().unwrap_or("unknown")
    ));
    line(format!(
        "    Scope {}",
        work.scratch_scope.as_deref().unwrap_or("unknown")
    ));
    for (name, status) in [
        ("Claims", &work.claims_status),
        ("Tasks", &work.tasks_status),
        ("PRs", &work.prs_status),
        ("Splits", &work.splits_status),
    ] {
        line(format!(
            "  {name} {} at {}",
            status.state,
            time(status.observed_at)
        ));
        if let Some(reason) = &status.reason {
            line(format!("    {reason}"));
        }
    }
    for split in &work.splits {
        line(format!("  Split {} [{}]", split.id, split.state));
        line(format!(
            "    Epic {} | peer {}",
            split.epic_id, split.peer_agent_id
        ));
        line(format!(
            "    Target {}",
            split.target_branch.as_deref().unwrap_or("unknown")
        ));
        line(format!("    Mine {}", split.mine.join(", ")));
        line(format!("    Theirs {}", split.theirs.join(", ")));
        line(format!("    Expires {}", time(Some(split.expires_at))));
    }
    for (label, rows) in [("Epic", &work.epics), ("Task", &work.tasks)] {
        for row in rows {
            line(format!(
                "  {label} [{}] Owner {}",
                row.status,
                row.owner.as_deref().unwrap_or("unknown (no active claim)")
            ));
            line(format!(
                "    {} | {}",
                row.task_id,
                row.title.as_deref().unwrap_or("title not synced")
            ));
            line(format!(
                "    Target {}",
                row.target_branch.as_deref().unwrap_or("unknown")
            ));
            line(format!(
                "    Branch {}",
                row.branch.as_deref().unwrap_or("unknown")
            ));
            line(format!(
                "    Head {}",
                row.head_sha.as_deref().unwrap_or("unknown")
            ));
            line(format!(
                "    Assignment {}",
                row.assignee.as_deref().unwrap_or("unknown")
            ));
            line(format!("    Task observed {}", time(row.observed_at)));
            line(format!(
                "    Claim {}",
                if row.claim.is_some() {
                    "active"
                } else {
                    "none observed"
                }
            ));
            if row.pr.is_none() {
                line("    PR unknown (no matching observation)".into());
            }
        }
    }
    for pr in &work.prs {
        line(format!(
            "  PR #{} [{}] | queue {}",
            pr.number, pr.state, pr.in_merge_queue
        ));
        line(format!(
            "    {}",
            pr.url.as_deref().unwrap_or("URL unknown")
        ));
        line(format!("    Branch {}", pr.branch));
        line(format!("    Head {}", pr.head_sha));
    }
    if work.claims_status.state != "ready" {
        line(format!(
            "  Waiting merges {} (claims {})",
            if work.claims_status.state == "private" {
                "private work"
            } else {
                "unknown"
            },
            work.claims_status.state
        ));
    } else {
        line(format!(
            "  Waiting merges {} recorded | PRs {}",
            work.waiting_merges.len(),
            work.prs_status.state
        ));
    }
    for waiting in &work.waiting_merges {
        line(format!(
            "    [{}] Owner {}",
            waiting.kind,
            waiting
                .owner
                .as_deref()
                .unwrap_or("unknown (no active claim)")
        ));
        line(format!(
            "      Task {}",
            waiting.task_id.as_deref().unwrap_or("unknown")
        ));
        line(format!(
            "      Branch {}",
            waiting.branch.as_deref().unwrap_or("unknown")
        ));
        line(format!(
            "      Head {}",
            waiting.head_sha.as_deref().unwrap_or("unknown")
        ));
        line(format!(
            "      PR {}",
            waiting.pr_url.as_deref().unwrap_or("unknown")
        ));
        line(format!("      Observed {}", time(waiting.observed_at)));
    }
}
