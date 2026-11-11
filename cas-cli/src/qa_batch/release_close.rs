//! Per-member release finalization and scoped parent acceptance evidence.
//! This is internal authority, never a caller-supplied close override.
use super::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseTransformations {
    pub project_id: String,
    pub release_id: String,
    pub reviewed_sha: String,
    pub final_release_sha: String,
    pub paths: Vec<ReleasePathTransformation>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleasePathTransformation {
    pub path: String,
    pub input_blobs: Vec<String>,
    pub output_blob: String,
    pub producing_commit: String,
    /// Maintained view's skill, whose exact revision is producer_blob.
    pub producer: String,
    pub producer_blob: String,
}

#[derive(Clone)]
pub(crate) struct ReleaseClose {
    root: PathBuf,
    repo: PathBuf,
    pub inventory: ReleaseInventory,
    transformations: Vec<(QaBatchReceipt, ReleaseTransformations)>,
    member_errors: BTreeMap<String, String>,
    strict_proofs: bool,
    reviews: Vec<(QaBatchReceipt, String)>,
    parent: Option<ParentReleaseEvidence>,
}

tokio::task_local! {
    static CURRENT: ReleaseClose;
}

impl ReleaseClose {
    /// Resolve membership afresh before granting any lifecycle authority.
    /// The caller keeps per-delivery exceptions; one bad member does not erase
    /// other shipped facts or make the release appear successfully finalized.
    pub(crate) fn prepare(
        root: &Path,
        repo: &Path,
        inventory: &ReleaseInventory,
    ) -> Result<Self, String> {
        // Parent acceptance still consumes explicitly recorded review bytes.
        // Member close never re-runs a whole-release ledger or diff gate.
        let receipts = read_receipts(root, &inventory.scope.project_id)?;
        let mut reviews = Vec::new();
        for proof in inventory.scope.proof_refs.iter().filter(|p| p.kind == ReleaseProofKind::Review) {
            let Some(receipt) = receipts.iter().find(|r| proof_matches(proof, r)) else { continue; };
            let Ok(body) = std::fs::read_to_string(&receipt.ledger_path) else { continue; };
            if body.len() as u64 == receipt.ledger_size_bytes
                && format!("{:x}", Sha256::digest(body.as_bytes())) == receipt.ledger_sha256 {
                reviews.push((receipt.clone(), body));
            }
        }
        Ok(Self {
            root: root.to_path_buf(), repo: repo.to_path_buf(), inventory: inventory.clone(),
            transformations: Vec::new(), member_errors: BTreeMap::new(),
            strict_proofs: true, reviews, parent: None,
        })
    }

    /// Existing receipt callbacks already validate receipt binding. Keep their
    /// fixed reviewed commit even when no inventory adapter called them yet.
    pub(crate) fn for_receipt(
        root: &Path,
        repo: &Path,
        task: &Task,
        receipt: &QaBatchReceipt,
    ) -> Self {
        let proofs = release_proof_refs(
            std::slice::from_ref(receipt),
            &task.id,
            task.deliverables
                .factory_branch_anchor
                .as_deref()
                .unwrap_or(""),
        );
        Self {
            root: root.to_path_buf(),
            repo: repo.to_path_buf(),
            transformations: Vec::new(),
            member_errors: BTreeMap::new(),
            strict_proofs: false,
            reviews: Vec::new(),
            parent: None,
            inventory: ReleaseInventory {
                scope: ReleaseScope {
                    project_id: receipt.project_id.clone(),
                    repository: String::new(),
                    release_id: receipt.id.clone(),
                    reviewed_sha: receipt.staging_sha.clone(),
                    final_release_sha: receipt.staging_sha.clone(),
                    promotion_pr: None,
                    release_tag: receipt.release_tag.clone(),
                    constituent_epic_ids: Vec::new(),
                    included_task_ids: vec![task.id.clone()],
                    no_code_members: BTreeMap::new(),
                    proof_refs: proofs,
                },
                deliveries: vec![ReleaseDelivery {
                    task_id: task.id.clone(),
                    generation: task.updated_at.to_rfc3339(),
                    anchor: delivery_anchor(repo, task),
                    base_sha: task
                        .deliverables
                        .factory_branch_start
                        .as_ref()
                        .map(|s| s.base_sha.clone()),
                    shipped_commit: None,
                    membership: ReleaseMembership::Ancestry,
                    proof_refs: Vec::new(),
                }],
                exceptions: Vec::new(),
            },
        }
    }

    pub(crate) fn member_error(&self, id: &str) -> Option<&str> {
        self.member_errors.get(id).map(String::as_str)
    }

    pub(crate) async fn scope<F: std::future::Future>(self, future: F) -> F::Output {
        CURRENT.scope(self, future).await
    }

    fn delivery(&self, id: &str) -> Option<&ReleaseDelivery> {
        self.inventory.deliveries.iter().find(|d| d.task_id == id)
    }
}

pub(crate) fn parse_manifest(body: &str) -> Result<Option<ReleaseTransformations>, String> {
    let marker = "```cassy-release-transformations\n";
    let mut blocks = body.split(marker);
    blocks.next();
    let Some(block) = blocks.next() else {
        return Ok(None);
    };
    if blocks.next().is_some() {
        return Err("RELEASE_TRANSFORMATION_AMBIGUOUS".into());
    }
    let (json, _) = block
        .split_once("\n```")
        .ok_or("RELEASE_TRANSFORMATION_UNTERMINATED")?;
    serde_json::from_str(json)
        .map(Some)
        .map_err(|e| format!("RELEASE_TRANSFORMATION_INVALID: {e}"))
}

fn proof_matches(proof: &ReleaseProofRef, receipt: &QaBatchReceipt) -> bool {
    let schema = match proof.kind {
        ReleaseProofKind::Qa => cas_store::QA_BATCH_RECEIPT_SCHEMA,
        ReleaseProofKind::Review => cas_store::RELEASE_VERDICT_SCHEMA,
        ReleaseProofKind::Report => return false,
    };
    receipt.schema_version == schema
        && proof.receipt_id.as_deref() == Some(receipt.id.as_str())
        && proof.artifact_id == receipt.ledger_artifact_id
        && proof.sha256 == receipt.ledger_sha256
        && proof.size_bytes == receipt.ledger_size_bytes
        && proof.reviewed_sha == receipt.staging_sha
}

/// Validate before the close's own bookkeeping changes its optimistic token.
pub(crate) fn validate_task(root: &Path, task: &Task) -> Result<(), String> {
    CURRENT
        .try_with(|context| {
            if context.root != root {
                return Err("RELEASE_PROJECT_CHANGED".into());
            }
            if let Some(parent) = context.parent.as_ref().filter(|p| p.task_id == task.id) {
                if parent.generation != task.updated_at.to_rfc3339() {
                    return Err("RELEASE_PARENT_GENERATION_CHANGED".into());
                }
                return parent.validate_children(root);
            }
            let Some(delivery) = context.delivery(&task.id) else {
                return Err("RELEASE_MEMBERSHIP_CHANGED".into());
            };
            if let Some(error) = context.member_error(&task.id) {
                return Err(error.to_string());
            }
            if delivery.generation != task.updated_at.to_rfc3339()
                || delivery.base_sha.as_deref()
                    != task
                        .deliverables
                        .factory_branch_start
                        .as_ref()
                        .map(|s| s.base_sha.as_str())
            {
                return Err("RELEASE_GENERATION_CHANGED".into());
            }
            if task.status == cas_types::TaskStatus::Blocked
                || context
                    .inventory
                    .exceptions
                    .iter()
                    .any(|e| e.task_id == task.id)
            {
                return Err("RELEASE_MEMBER_HELD".into());
            }
            Ok(())
        })
        .unwrap_or(Ok(()))
}

pub(crate) fn target(root: &Path, task_id: &str) -> Option<String> {
    CURRENT
        .try_with(|context| {
            (context.root == root && (context.delivery(task_id).is_some() || context.parent.as_ref().is_some_and(|p| p.task_id == task_id)))
                .then(|| context.inventory.scope.final_release_sha.clone())
        })
        .ok()
        .flatten()
}

pub(crate) fn binding_note(root: &Path, task_id: &str) -> Option<String> {
    CURRENT.try_with(|context| {
        (context.root == root && (context.delivery(task_id).is_some() || context.parent.as_ref().is_some_and(|p| p.task_id == task_id))).then(|| format!(
            "Release {}: close evidence fixed at {}; reviewed candidate {}; proof artifacts: {}.",
            context.inventory.scope.release_id, context.inventory.scope.final_release_sha,
            context.inventory.scope.reviewed_sha,
            context.inventory.scope.proof_refs.iter().map(|p| p.artifact_id.as_str()).collect::<Vec<_>>().join(", "),
        ))
    }).ok().flatten()
}

pub(crate) fn receipt_allowed(root: &Path, task: &Task, receipt: &QaBatchReceipt) -> bool {
    CURRENT
        .try_with(|context| {
            if !context.strict_proofs {
                return true;
            }
            if context.root != root || receipt.project_id != context.inventory.scope.project_id {
                return false;
            }
            let Some(delivery) = context.delivery(&task.id) else {
                return false;
            };
            if receipt.schema_version == cas_store::RELEASE_TAG_VERDICT_SCHEMA {
                // Publication is shipping evidence, never independent review.
                return false;
            }
            receipt.ledger_author_id.is_some()
                && receipt.staging_sha == context.inventory.scope.reviewed_sha
                && context
                    .inventory
                    .scope
                    .proof_refs
                    .iter()
                    .chain(&delivery.proof_refs)
                    .any(|proof| proof_matches(proof, receipt))
        })
        .unwrap_or(true)
}

fn same_repo(repo: &Path, expected: &Path) -> bool {
    let key = |path: &Path| {
        crate::worktree::GitOperations::new(path.to_path_buf())
            .canonical_repo_key()
            .ok()
    };
    matches!((key(repo), key(expected)), (Some(a), Some(b)) if a == b)
}

pub(crate) fn delivery_base(repo: &Path, anchor: &str, target: &str) -> Option<String> {
    CURRENT
        .try_with(|context| {
            (target == context.inventory.scope.final_release_sha && same_repo(repo, &context.repo))
                .then(|| {
                    context
                        .inventory
                        .deliveries
                        .iter()
                        .find(|d| d.anchor.as_deref() == Some(anchor))
                        .and_then(|d| d.base_sha.clone())
                })
                .flatten()
        })
        .ok()
        .flatten()
}

/// Apply only the release review's explicit path receipts. Sources and deletions
/// cannot borrow a generated-view name, an old output, or a tag's authority.
pub(crate) fn transformed_paths(
    repo: &Path,
    anchor: &str,
    target: &str,
    paths: Vec<String>,
    owners: &[&str],
) -> Vec<String> {
    CURRENT
        .try_with(|context| {
            if target != context.inventory.scope.final_release_sha
                || !same_repo(repo, &context.repo)
            {
                return paths.clone();
            }
            paths
                .iter()
                .filter(|path| {
                    !context.transformations.iter().any(|(receipt, manifest)| {
                        let covered = owners.iter().any(|id| {
                            receipt.task_heads.get(*id).is_some_and(|anchor| {
                                Some(anchor) == context.delivery(id).and_then(|d| d.anchor.as_ref())
                            })
                        });
                        covered
                            && manifest.paths.iter().any(|proof| {
                                proof.path == **path
                                    && transformation_matches(repo, anchor, target, proof)
                            })
                    })
                })
                .cloned()
                .collect()
        })
        .unwrap_or(paths)
}

fn git_text(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
        .map(|s| s.trim().to_string())
}

fn transformation_matches(
    repo: &Path,
    anchor: &str,
    target: &str,
    proof: &ReleasePathTransformation,
) -> bool {
    let producer = match proof.path.as_str() {
        ".claude/CODEMAP.md" => "codemap",
        "docs/PRODUCT_OVERVIEW.md" => "project-overview",
        "docs/qa/journeys.yaml" => "cas-qa-craft",
        _ => return false,
    };
    if proof.producer != producer
        || commit(repo, &proof.producing_commit).as_deref() != Some(proof.producing_commit.as_str())
        || ancestor(repo, &proof.producing_commit, target) != Some(true)
    {
        return false;
    }
    let blob = |rev: &str, path: &str| git_text(repo, &["rev-parse", &format!("{rev}:{path}")]);
    let Some(input) = blob(anchor, &proof.path) else {
        return false;
    };
    if !proof.input_blobs.contains(&input)
        || blob(target, &proof.path).as_deref() != Some(proof.output_blob.as_str())
    {
        return false;
    }
    let skill = format!("cas-cli/src/builtins/skills/{producer}/SKILL.md");
    let local_skill = format!(".claude/skills/{producer}/SKILL.md");
    if ![skill, local_skill]
        .iter()
        .any(|path| blob(target, path).as_deref() == Some(proof.producer_blob.as_str()))
    {
        return false;
    }
    // ls-tree binds both output blob and file mode; a later edit, deletion or
    // chmod cannot borrow a producer that no longer describes the release.
    let tree = |rev: &str| git_text(repo, &["ls-tree", rev, "--", &proof.path]);
    let output = tree(&proof.producing_commit);
    let previous = tree(&format!("{}^1", proof.producing_commit));
    output.as_ref().is_some_and(|output| !output.is_empty())
        && output == tree(target)
        && previous.is_some()
        && previous != output
}


/// The release review's `cassy-release-epics` block; the publishing ledger
/// scaffold and this validator share one type.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EpicManifest {
    pub project_id: String,
    pub release_id: String,
    pub reviewed_sha: String,
    pub final_release_sha: String,
    pub epics: Vec<EpicAcceptance>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EpicAcceptance {
    pub task_id: String,
    pub generation: String,
    pub child_ids: Vec<String>,
    pub acceptance: EpicPass,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EpicPass { Pass }

#[derive(Clone)]
struct ParentReleaseEvidence {
    task_id: String,
    generation: String,
    child_versions: BTreeMap<String, String>,
    covered_child_ids: std::collections::BTreeSet<String>,
    review: Option<String>,
    integration: Option<String>,
}

impl ParentReleaseEvidence {
    fn validate_children(&self, root: &Path) -> Result<(), String> {
        let store = crate::store::open_task_store(root).map_err(|e| e.to_string())?;
        let children = store.get_subtasks(&self.task_id).map_err(|e| e.to_string())?;
        let covered: std::collections::BTreeSet<_> = children.iter().map(|c| c.id.clone()).collect();
        let mut pending = children;
        let mut current = BTreeMap::new();
        while let Some(child) = pending.pop() {
            if current.contains_key(&child.id) { continue; }
            if child.id == self.task_id || !child.is_terminal() { return Err("RELEASE_PARENT_CHILDREN_CHANGED".into()); }
            if child.task_type == cas_types::TaskType::Epic {
                pending.extend(store.get_subtasks(&child.id).map_err(|e| e.to_string())?);
            }
            current.insert(child.id.clone(), child.updated_at.to_rfc3339());
        }
        if covered.is_empty() || covered != self.covered_child_ids || current != self.child_versions {
            return Err("RELEASE_PARENT_CHILDREN_CHANGED".into());
        }
        Ok(())
    }
}

impl ReleaseClose {
    /// Extend the already validated release context with explicit parent review.
    /// An absent/mismatched block grants no review authority and queues the
    /// ordinary epic verifier; it never turns prose or shipping into acceptance.
    pub(crate) async fn for_parent(mut self, task: &Task, children: &[Task]) -> Result<Self, String> {
        if children.is_empty() || !children.iter().all(Task::is_terminal) {
            return Err("RELEASE_PARENT_CHILDREN_INCOMPLETE".into());
        }
        let store = crate::store::open_task_store(&self.root).map_err(|e| e.to_string())?;
        let mut descendants = Vec::new();
        let mut pending = children.to_vec();
        let mut seen = std::collections::BTreeSet::new();
        while let Some(child) = pending.pop() {
            if child.id == task.id { return Err("RELEASE_PARENT_CYCLE".into()); }
            if !seen.insert(child.id.clone()) { continue; }
            if child.task_type == cas_types::TaskType::Epic {
                let nested = store.get_subtasks(&child.id).map_err(|e| e.to_string())?;
                if nested.is_empty() { return Err("RELEASE_EMPTY_PARENT".into()); }
                pending.extend(nested);
            }
            descendants.push(child);
        }
        let leaves: Vec<_> = descendants.iter().filter(|c| c.task_type != cas_types::TaskType::Epic && c.status != cas_types::TaskStatus::Cancelled).collect();
        if leaves.is_empty() { return Err("RELEASE_EMPTY_PARENT: no delivered descendants; operator disposition required".into()); }
        for child in &leaves {
            if child.status != cas_types::TaskStatus::Closed || self.delivery(&child.id).is_none() || self.member_error(&child.id).is_some() {
                return Err(format!("RELEASE_PARENT_MEMBER_HELD: {}", child.id));
            }
        }
        let expected: std::collections::BTreeSet<_> = children.iter().map(|c| c.id.as_str()).collect();
        let mut review = None;
        for (receipt, body) in &self.reviews {
            let mut blocks = body.split("```cassy-release-epics\n");
            blocks.next();
            let Some(block) = blocks.next() else { continue; };
            if blocks.next().is_some() { return Err("RELEASE_EPIC_COVERAGE_AMBIGUOUS".into()); }
            let (json, _) = block.split_once("\n```").ok_or("RELEASE_EPIC_COVERAGE_UNTERMINATED")?;
            let manifest: EpicManifest = serde_json::from_str(json).map_err(|e| format!("RELEASE_EPIC_COVERAGE_INVALID: {e}"))?;
            let scope = &self.inventory.scope;
            if manifest.project_id != scope.project_id || manifest.release_id != scope.release_id
                || manifest.reviewed_sha != scope.reviewed_sha || manifest.final_release_sha != scope.final_release_sha {
                return Err("RELEASE_EPIC_COVERAGE_BINDING".into());
            }
            let rows: Vec<_> = manifest.epics.iter().filter(|e| e.task_id == task.id).collect();
            if rows.len() > 1 { return Err("RELEASE_EPIC_COVERAGE_AMBIGUOUS".into()); }
            let Some(row) = rows.first() else { continue; };
            let actual: std::collections::BTreeSet<_> = row.child_ids.iter().map(String::as_str).collect();
            if row.generation != task.updated_at.to_rfc3339() || actual != expected || actual.len() != row.child_ids.len() { continue; }
            let author = receipt.ledger_author_id.as_deref().ok_or("RELEASE_REVIEW_AUTHOR_MISSING")?;
            let agents = crate::store::open_agent_store(&self.root).map_err(|e| e.to_string())?;
            for member in std::iter::once(task).chain(descendants.iter()) {
                if in_lineage(agents.as_ref(), &implementation_lineage(&self.root, member, &self.repo)?, author) {
                    return Err(format!("RELEASE_EPIC_REVIEW_NOT_INDEPENDENT: {}", member.id));
                }
            }
            match row.acceptance { EpicPass::Pass => review = Some(receipt.id.clone()) }
        }
        // Reuse independently recorded QA of every delivered leaf at the same
        // mapped release tree, including their published-byte checks. No new run.
        let integration = self.clone().scope(async {
            let mut proofs = std::collections::BTreeSet::new();
            for child in &leaves {
                let anchor = self.delivery(&child.id).and_then(|d| d.anchor.as_deref());
                let Some(anchor) = anchor else { return Ok::<_, String>(None); };
                let qa = satisfying_receipt(&self.root, child, &self.repo, &self.inventory.scope.final_release_sha, anchor)?;
                let Some(qa) = qa else { return Ok(None); };
                proofs.insert(qa.ledger_artifact_id);
            }
            Ok(Some(format!("Release QA of the mapped tree: {}", proofs.into_iter().collect::<Vec<_>>().join(", "))))
        }).await?;
        self.parent = Some(ParentReleaseEvidence {
            task_id: task.id.clone(), generation: task.updated_at.to_rfc3339(),
            child_versions: descendants.iter().map(|c| (c.id.clone(), c.updated_at.to_rfc3339())).collect(),
            covered_child_ids: children.iter().map(|c| c.id.clone()).collect(), review, integration,
        });
        Ok(self)
    }
}

pub(crate) fn parent_review(root: &Path, id: &str) -> Option<String> {
    CURRENT.try_with(|c| (c.root == root).then(|| c.parent.as_ref().filter(|p| p.task_id == id).and_then(|p| p.review.clone())).flatten()).ok().flatten()
}

pub(crate) fn parent_integration(root: &Path, id: &str) -> Option<String> {
    CURRENT.try_with(|c| (c.root == root).then(|| c.parent.as_ref().filter(|p| p.task_id == id).and_then(|p| p.integration.clone())).flatten()).ok().flatten()
}

pub(crate) fn parent_authority(root: &Path, id: &str) -> Option<String> {
    CURRENT.try_with(|c| {
        (c.root == root && c.parent.as_ref().is_some_and(|p| p.task_id == id)).then(|| {
            let scope = &c.inventory.scope;
            cas_store::release_reconcile_store::ReleaseOperationKey {
                project_id: scope.project_id.clone(), release_id: scope.release_id.clone(), final_release_sha: scope.final_release_sha.clone(),
            }.id().map(|id| format!("release:{id}"))
        }).transpose()
    }).ok().and_then(Result::ok).flatten()
}

pub(crate) fn revalidate_parent_children(root: &Path, id: &str) -> Result<(), String> {
    CURRENT.try_with(|c| {
        if c.root != root { return Ok(()); }
        c.parent.as_ref().filter(|p| p.task_id == id).map(|p| p.validate_children(root)).unwrap_or(Ok(()))
    }).unwrap_or(Ok(()))
}


/// A later independent parent review can satisfy only the exact task-only
/// dispatch at this release SHA. A foreign/newer proof cycle keeps its gate.
pub(crate) fn parent_dispatch_covered(root: &Path, id: &str) -> bool {
    CURRENT.try_with(|c| {
        c.root == root && c.parent.as_ref().is_some_and(|p| p.task_id == id && p.review.is_some())
            && cas_store::get_latest_verification_dispatch(root, id).is_ok_and(|dispatch| dispatch.is_none_or(|d| {
                d.receipt_id.is_none() && d.delivery_transaction_id.is_none()
                    && d.repository.as_ref().is_some_and(|r| r.head_commit == c.inventory.scope.final_release_sha)
            }))
    }).unwrap_or(false)
}

/// Open an existing store without schema initialization, sync repair or cache writes.
pub(crate) fn read_connection(root: &Path) -> Result<rusqlite::Connection, String> {
    rusqlite::Connection::open_with_flags(root.join("cas.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())
}

pub(crate) fn read_tasks(root: &Path) -> Result<cas_store::SqliteTaskStore, String> {
    Ok(cas_store::SqliteTaskStore::with_connection(std::sync::Arc::new(std::sync::Mutex::new(read_connection(root)?))))
}

pub(crate) fn read_receipts(root: &Path, project: &str) -> Result<Vec<QaBatchReceipt>, String> {
    let conn = read_connection(root)?;
    if !conn.table_exists(None, "qa_batch_receipts").map_err(|e| e.to_string())? { return Ok(vec![]); }
    let mut stmt = conn.prepare("SELECT receipt_json FROM qa_batch_receipts WHERE project_id=? ORDER BY recorded_at DESC, id DESC")
        .map_err(|e| e.to_string())?;
    stmt.query_map([project], |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?
        .map(|row| serde_json::from_str(&row.map_err(|e| e.to_string())?).map_err(|e| e.to_string())).collect()
}

/// Read local or synced portable metadata without initializing the artifact
/// ledger or fetching bytes into the cache. Revocation wins over synced notes.
pub(crate) fn read_artifact(root: &Path, id: &str) -> Result<cas_store::PublishedArtifact, String> {
    use cas_store::TaskStore;
    use rusqlite::OptionalExtension;
    let conn = read_connection(root)?;
    if conn.table_exists(None, "artifact_revocations").map_err(|e| e.to_string())?
        && conn.query_row("SELECT EXISTS(SELECT 1 FROM artifact_revocations WHERE artifact_id=?)", [id], |row| row.get::<_, bool>(0)).map_err(|e| e.to_string())? {
        return Err("ARTIFACT_REVOKED".into());
    }
    if conn.table_exists(None, "artifacts").map_err(|e| e.to_string())? {
        let local = conn.query_row(
            "SELECT id, task_id, name, mime, size_bytes, sha256, cloud_artifact_id, status, cloud_url, slack_permalink, created_at FROM artifacts WHERE id=?", [id], |row| {
                Ok(cas_store::PublishedArtifact {
                    id: row.get(0)?, task_id: row.get(1)?, name: row.get(2)?, mime: row.get(3)?,
                    size_bytes: row.get::<_, i64>(4)? as u64, sha256: row.get(5)?, cloud_artifact_id: row.get(6)?,
                    status: row.get(7)?, cloud_url: row.get(8)?, slack_permalink: row.get(9)?,
                    created_at: row.get(10)?, revocation: None,
                })
            }).optional().map_err(|e| e.to_string())?;
        if let Some(local) = local { return Ok(local); }
    }
    let mut found = None;
    for task in read_tasks(root)?.list(None).map_err(|e| e.to_string())? {
        if let Some(proof) = crate::artifacts::proof::from_notes(&task.notes, id, &task.id).map_err(|e| e.to_string())? {
            if found.as_ref().is_some_and(|prior| !crate::artifacts::proof::same_artifact(prior, &proof)) {
                return Err("ARTIFACT_METADATA_CONFLICT".into());
            }
            found = Some(proof);
        }
    }
    found.ok_or_else(|| "ARTIFACT_METADATA_MISSING".into())
}

/// Recorded verdicts already validated their ledger and author. Publication
/// consumes that immutable fact and never grants approval by itself.
fn approved_at(root: &Path, task: &Task, anchor: &str, receipts: &[QaBatchReceipt]) -> Result<bool, String> {
    use rusqlite::OptionalExtension;
    let conn = read_connection(root)?;
    if conn.table_exists(None, "verification_dispatches").map_err(|e| e.to_string())? {
        if let Some(dispatch) = cas_store::get_latest_verification_dispatch_with_conn(&conn, &task.id).map_err(|e| e.to_string())? {
            let head_matches = dispatch.repository.as_ref().is_some_and(|r| r.head_commit == anchor);
            let status: Option<String> = conn.query_row(
                "SELECT status FROM verifications WHERE dispatch_id=? AND task_id=? ORDER BY created_at DESC, id DESC LIMIT 1",
                rusqlite::params![dispatch.id, task.id], |row| row.get(0),
            ).optional().map_err(|e| e.to_string())?;
            if dispatch.state == cas_types::VerificationDispatchState::Resolved && head_matches && status.as_deref() == Some("approved") {
                return Ok(true);
            }
        }
    }
    Ok(receipts.iter().any(|receipt| {
        matches!(receipt.schema_version, cas_store::QA_BATCH_RECEIPT_SCHEMA | cas_store::RELEASE_VERDICT_SCHEMA)
            && receipt.task_heads.get(&task.id).is_some_and(|head| head == anchor)
    }))
}

/// Preview and apply share every decision; only the guarded close write differs.
pub(crate) fn reconcile_members(
    root: &Path, repo: &Path, inventory: &ReleaseInventory, write: bool,
    checkpoint: &mut impl FnMut(&str, crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose) -> Result<(), String>,
) -> Result<BTreeMap<String, crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose>, String> {
    use cas_store::TaskStore;
    use cas_types::TaskStatus;
    use crate::mcp::tools::core::task::lifecycle::{merge_close::CassyMergeClose, review_tier::{self, VerificationTier}};
    let tasks = read_tasks(root)?;
    let receipts = read_receipts(root, &inventory.scope.project_id)?;
    let ids: std::collections::BTreeSet<_> = inventory.deliveries.iter().map(|d| d.task_id.clone())
        .chain(inventory.exceptions.iter().map(|e| e.task_id.clone()))
        .chain(inventory.scope.included_task_ids.iter().cloned())
        .chain(inventory.scope.no_code_members.keys().cloned()).collect();
    let mut outcomes = BTreeMap::new();
    for id in ids {
        let evaluate = || -> Result<CassyMergeClose, String> {
            let task = match tasks.get(&id) { Ok(task) => task, Err(_) => return Ok(CassyMergeClose::NotEligible) };
            if task.status == TaskStatus::Closed { return Ok(CassyMergeClose::AlreadyClosed); }
            if task.is_terminal() { return Ok(CassyMergeClose::NotEligible); }
            if let Some(exception) = inventory.exceptions.iter().find(|e| e.task_id == id) {
                return Err(format!("RELEASE_MEMBER_HELD: {:?}", exception.code));
            }
            let delivery = inventory.deliveries.iter().find(|d| d.task_id == id).ok_or("RELEASE_MEMBERSHIP_MISSING")?;
            if task.updated_at.to_rfc3339() != delivery.generation { return Err("RELEASE_GENERATION_CHANGED".into()); }
            let mut anchors = Vec::new();
            if delivery.membership == ReleaseMembership::ExplicitNoCode {
                crate::release_reconcile::validate_no_code_proofs(root, &inventory.scope, &id, &delivery.proof_refs)?;
                anchors.extend(delivery.proof_refs.iter().map(|proof| proof.reviewed_sha.as_str()));
            } else {
                let anchor = delivery.anchor.as_deref().ok_or("RELEASE_ANCHOR_MISSING")?;
                if task.deliverables.factory_branch_anchor.as_deref().is_some_and(|current| current != anchor) {
                    return Err("RELEASE_GENERATION_CHANGED".into());
                }
                anchors.push(anchor);
            }
            let tier_zero = delivery.anchor.as_deref().is_some_and(|anchor| {
                !task.labels.iter().any(|label| label == review_tier::FORCE_VERIFIER_LABEL)
                    && (delivery.base_sha.as_deref() == Some(anchor)
                        || review_tier::tier_for_delivery(&task, repo).is_some_and(|d| d.tier == VerificationTier::Mechanical))
            });
            let mut approved = !anchors.is_empty();
            for anchor in anchors {
                if ancestor(repo, anchor, &inventory.scope.final_release_sha) != Some(true) {
                    return Err("RELEASE_ANCHOR_NOT_SHIPPED".into());
                }
                approved &= tier_zero || approved_at(root, &task, anchor, &receipts)?;
            }
            if !approved { return Err("REVIEW_MISSING".into()); }
            if write {
                let mut closed = task.clone();
                closed.status = TaskStatus::Closed;
                closed.closed_at = Some(chrono::Utc::now());
                closed.close_reason = Some(task.deliverables.parked_close_reason.clone().unwrap_or_else(|| format!(
                    "Published in {} at {}", inventory.scope.release_id, inventory.scope.final_release_sha)));
                closed.pending_verification = false;
                closed.pending_worktree_merge = false;
                let store = crate::store::open_task_store(root).map_err(|e| e.to_string())?;
                store.update_if_current(&closed, &task).map_err(|error| format!("RELEASE_CLOSE_WRITE: {error}"))?;
            }
            Ok(CassyMergeClose::Closed { notice: "completion included in release receipt".into() })
        };
        let outcome = evaluate().unwrap_or_else(CassyMergeClose::Held);
        checkpoint(&id, outcome.clone())?;
        outcomes.insert(id, outcome);
    }
    Ok(outcomes)
}

#[cfg(test)]
#[path = "release_close_tests.rs"]
mod tests;
