//! Publish and promotion adapters (close-at-release #4). A cas-src tag
//! publication and a staging→main promotion merge each reach one trusted local
//! reconciliation per (project, release, final SHA), whose durable operation
//! record holds the per-member receipt; the adapter sends one named-exception
//! notice. Installed and backfill are recovery triggers, no-ops once complete.
//! GitHub is read as evidence only: no CI job, poller or test rerun is added.
use super::release_automation::{
    ReleaseContext, git, published_tag_sha, published_version, release_roots, tracked_context,
};
use crate::qa_batch::release_close::{ReleasePathTransformation, ReleaseTransformations};
use crate::issue_reconcile::{IssueAction, IssueOutcome};
use crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose;
use crate::qa_batch::{
    ReleaseInventory, ReleaseInventoryException, ReleaseProofKind, ReleaseProofRef, ReleaseScope,
};
use anyhow::{Context, Result, ensure};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

// ---- Seam to close-at-release #3. ----

/// Stand-in for `crate::release_reconcile` (#3), with its exact names and
/// signatures. Swap: delete this module and `use crate::release_reconcile;`.
pub(crate) mod release_reconcile {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub(crate) enum ReconcileTrigger {
        Publish { tag: String },
        Promotion { pr: u64, merge_sha: String },
        Installed,
        Backfill,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct ReleaseOperationKey {
        pub project_id: String,
        pub release_id: String,
        pub final_release_sha: String,
    }

    #[derive(Debug, Default)]
    pub(crate) struct ReleaseReconcileReceipt {
        pub members: BTreeMap<String, CassyMergeClose>,
        pub issue_outcomes: Vec<IssueOutcome>,
        pub parents: BTreeMap<String, ParentOutcome>,
        pub exceptions: Vec<ReleaseInventoryException>,
        /// Every task or epic left non-terminal, with its owner and next action.
        pub residuals: Vec<ReleaseResidual>,
        pub notice_intent: Option<ReleaseNoticeIntent>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct ReleaseResidual {
        pub task_id: String,
        pub gate: String,
        pub owner: String,
        pub next_action: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum ParentOutcome {
        Closed,
        Pending { dispatch_id: String, owner: String },
        Held { gate: String, owner: String, next_action: String },
    }

    /// #3 persists this and never enqueues; `key` is the notice's dedupe identity.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct ReleaseNoticeIntent {
        pub key: String,
        pub recipient: String,
        pub fingerprint: String,
    }

    pub(crate) async fn reconcile_release(
        _root: &Path,
        _scope: &ReleaseScope,
        _inventory: &ReleaseInventory,
        _trigger: ReconcileTrigger,
    ) -> Result<ReleaseReconcileReceipt, String> {
        Err("RELEASE_RECONCILER_PENDING: release reconciliation is not available in this build".into())
    }

    /// None for an absent, partial or held operation.
    pub(crate) fn completed_operation(
        _root: &Path,
        _key: &ReleaseOperationKey,
    ) -> Result<Option<ReleaseReconcileReceipt>, String> {
        Ok(None)
    }
}

use release_reconcile::{
    ParentOutcome, ReconcileTrigger, ReleaseOperationKey, ReleaseReconcileReceipt,
};

/// The two #3 calls the adapters make; tests substitute a recording double.
pub(crate) trait Reconciler {
    fn completed(&self, root: &Path, key: &ReleaseOperationKey) -> Result<Option<ReleaseReconcileReceipt>>;
    fn reconcile(
        &self,
        root: &Path,
        scope: &ReleaseScope,
        inventory: &ReleaseInventory,
        trigger: ReconcileTrigger,
    ) -> Result<ReleaseReconcileReceipt>;
}

struct LocalReconciler;

impl Reconciler for LocalReconciler {
    fn completed(&self, root: &Path, key: &ReleaseOperationKey) -> Result<Option<ReleaseReconcileReceipt>> {
        release_reconcile::completed_operation(root, key).map_err(anyhow::Error::msg)
    }

    fn reconcile(
        &self,
        root: &Path,
        scope: &ReleaseScope,
        inventory: &ReleaseInventory,
        trigger: ReconcileTrigger,
    ) -> Result<ReleaseReconcileReceipt> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        runtime
            .block_on(release_reconcile::reconcile_release(root, scope, inventory, trigger))
            .map_err(anyhow::Error::msg)
    }
}

fn operation_key(project_id: &str, release_id: &str, final_release_sha: &str) -> ReleaseOperationKey {
    ReleaseOperationKey {
        project_id: project_id.into(),
        release_id: release_id.into(),
        final_release_sha: final_release_sha.into(),
    }
}

// ---- One operation per release identity. ----

/// Installed/backfill recovery: `Some(report)` when a primary trigger already
/// completed this tag's operation at its local commit.
pub(crate) fn recorded_closeout_for_tag(root: &Path, tag: &str) -> Option<String> {
    recorded_closeout_with(root, tag, &LocalReconciler)
}

fn recorded_closeout_with(root: &Path, tag: &str, reconciler: &dyn Reconciler) -> Option<String> {
    let project = crate::cloud::resolve_canonical_id(root)?;
    let repo = root.parent()?;
    let sha = git(repo, &["rev-parse", "--verify", "--end-of-options", &format!("refs/tags/{tag}^{{commit}}")]).ok()?;
    reconciler.completed(root, &operation_key(&project, tag, &sha)).ok()?
        .map(|_| format!("{tag}: release operation already completed at {sha}; recovery trigger is a no-op"))
}

/// Run one trigger. The first trigger for a release identity reconciles once;
/// any later trigger reuses #3's completed record. The notice is keyed by #3's
/// notice intent, so a crash between reconcile and notice is repaired by the
/// next trigger without reconciling or notifying twice.
pub(crate) fn reconcile_once(
    root: &Path,
    repo: &Path,
    scope: &ReleaseScope,
    trigger: ReconcileTrigger,
    reconciler: &dyn Reconciler,
) -> Result<String> {
    let key = operation_key(&scope.project_id, &scope.release_id, &scope.final_release_sha);
    let recorded = reconciler.completed(root, &key)?;
    let reused = recorded.is_some();
    let receipt = match recorded {
        Some(receipt) => receipt,
        None => {
            let inventory = crate::qa_batch::resolve_release_inventory(root, repo, scope)
                .map_err(anyhow::Error::msg)?;
            reconciler.reconcile(root, scope, &inventory, trigger)?
        }
    };
    send_notice(root, scope, &receipt)?;
    Ok(format!(
        "{} at {}: {} ({} member(s), {} residual(s))",
        scope.release_id,
        scope.final_release_sha,
        if reused { "operation already completed" } else { "reconciled" },
        receipt.members.len(),
        receipt.residuals.len(),
    ))
}

/// One named-exception notice per release: residuals and held or pending
/// parents with owner and next action, inventory exceptions and unfinished
/// GitHub issues.
fn send_notice(root: &Path, scope: &ReleaseScope, receipt: &ReleaseReconcileReceipt) -> Result<()> {
    let mut detail = format!(
        "{} closeout at {}: {} member(s) reconciled, {} residual(s).",
        scope.release_id,
        scope.final_release_sha,
        receipt.members.len(),
        receipt.residuals.len()
    );
    for r in &receipt.residuals {
        detail.push_str(&format!("\n- {}: {}; owner {}; next: {}", r.task_id, r.gate, r.owner, r.next_action));
    }
    for (id, parent) in &receipt.parents {
        match parent {
            ParentOutcome::Closed => {}
            ParentOutcome::Pending { dispatch_id, owner } => detail.push_str(&format!(
                "\n- {id}: epic verification pending ({dispatch_id}); owner {owner}; next: run that verification"
            )),
            ParentOutcome::Held { gate, owner, next_action } => {
                detail.push_str(&format!("\n- {id}: {gate}; owner {owner}; next: {next_action}"))
            }
        }
    }
    for e in &receipt.exceptions {
        if !receipt.residuals.iter().any(|r| r.task_id == e.task_id) {
            detail.push_str(&format!("\n- {}: inventory exception {:?}", e.task_id, e.code));
        }
    }
    for issue in &receipt.issue_outcomes {
        if matches!(issue.action, IssueAction::Partial | IssueAction::Failed) {
            detail.push_str(&format!(
                "\n- issue {} {:?} ({}): {}",
                issue.issue,
                issue.action,
                issue.task_ids.join(", "),
                issue.comment_or_error
            ));
        }
    }
    let intent = receipt.notice_intent.as_ref().context("release operation has no notice intent")?;
    let agents = crate::store::open_agent_store(root)?.list(None)?;
    let recipient = crate::supervisor_notices::resolve(&agents, Some(&intent.recipient), None);
    crate::supervisor_notices::enqueue_keyed(
        root,
        &recipient,
        "release_closeout",
        &detail,
        &format!("supervisor-notice:release_closeout:{}", intent.key),
        None,
    )?;
    Ok(())
}

// ---- Scope evidence shared by both adapters. ----

/// The newest release review whose candidate the final commit contains, plus
/// every QA/review receipt recorded at that candidate. No review: the final
/// commit with no proof, so closes hold as named residuals.
fn reviewed_proofs(root: &Path, repo: &Path, project: &str, final_sha: &str) -> Result<(String, Vec<ReleaseProofRef>)> {
    let receipts = cas_store::list_qa_batch_receipts(root, project)?;
    let reviewed = receipts
        .iter()
        .filter(|r| r.schema_version == cas_store::RELEASE_VERDICT_SCHEMA)
        .filter(|r| crate::qa_batch::ancestor(repo, &r.staging_sha, final_sha) == Some(true))
        .max_by(|a, b| a.recorded_at.cmp(&b.recorded_at).then_with(|| a.id.cmp(&b.id)))
        .map_or_else(|| final_sha.to_string(), |r| r.staging_sha.clone());
    let mut proofs: Vec<_> = receipts
        .iter()
        .filter(|r| r.staging_sha == reviewed)
        .filter_map(|r| {
            let kind = match r.schema_version {
                cas_store::QA_BATCH_RECEIPT_SCHEMA => ReleaseProofKind::Qa,
                cas_store::RELEASE_VERDICT_SCHEMA => ReleaseProofKind::Review,
                _ => return None, // a published tag is shipping evidence only
            };
            Some(ReleaseProofRef {
                kind,
                artifact_id: r.ledger_artifact_id.clone(),
                sha256: r.ledger_sha256.clone(),
                size_bytes: r.ledger_size_bytes,
                reviewed_sha: r.staging_sha.clone(),
                receipt_id: Some(r.id.clone()),
            })
        })
        .collect();
    proofs.sort_by(|a, b| a.receipt_id.cmp(&b.receipt_id));
    Ok((reviewed, proofs))
}

fn publish_scope(root: &Path, repo: &Path, context: &ReleaseContext, tag: &str, tag_sha: &str) -> Result<ReleaseScope> {
    if let Some(scope) = &context.release_scope {
        ensure!(
            scope.final_release_sha == tag_sha && scope.release_id == tag,
            "tracked release scope names {} at {}, not {tag} at {tag_sha}",
            scope.release_id,
            scope.final_release_sha
        );
        return Ok(scope.clone());
    }
    let project_id = crate::cloud::resolve_canonical_id(root).context("project identity unavailable")?;
    let (reviewed_sha, proof_refs) = reviewed_proofs(root, repo, &project_id, tag_sha)?;
    let included_task_ids = context
        .release_inputs
        .as_ref()
        .and_then(|inputs| inputs["included_task_ids"].as_array())
        .map(|ids| ids.iter().filter_map(|id| id.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    Ok(ReleaseScope {
        project_id,
        repository: context.repository.clone(),
        release_id: tag.to_string(),
        reviewed_sha,
        final_release_sha: tag_sha.to_string(),
        promotion_pr: None,
        release_tag: Some(tag.to_string()),
        constituent_epic_ids: vec![context.train_id.clone()],
        included_task_ids,
        no_code_members: BTreeMap::new(),
        proof_refs,
    })
}

// ---- Publish adapter: the host's existing install-on-publish read. ----

#[derive(Debug, Args)]
pub struct PublishedArgs {
    /// GitHub release JSON already read by install-on-publish
    #[arg(long)]
    receipt: PathBuf,
    #[arg(long)]
    repo: String,
}

pub fn published(args: &PublishedArgs, current_root: Option<&Path>) -> Result<()> {
    let version = published_version(&args.receipt)?;
    let tag = format!("v{version}");
    let mut failures = Vec::new();
    for root in release_roots(current_root)? {
        let Some(context) = tracked_context(&root, &args.repo, &version)? else {
            continue;
        };
        let result = (|| {
            let (repo, _) = crate::mcp::tools::core::task::repo_context::integration_branch(&root)
                .context("project checkout unavailable")?;
            // The tag may be newer than this checkout; fetching it is idempotent.
            let _ = git(&repo, &["fetch", "--quiet", "origin", "tag", &tag]);
            let tag_sha = published_tag_sha(&repo, &tag)?;
            let scope = publish_scope(&root, &repo, &context, &tag, &tag_sha)?;
            let trigger = ReconcileTrigger::Publish { tag: tag.clone() };
            reconcile_once(&root, &repo, &scope, trigger, &LocalReconciler)
        })();
        match result {
            Ok(report) => println!("{report}"),
            Err(error) => {
                eprintln!("[ERROR] {} ({tag}): {error:#}", root.display());
                failures.push(root.display().to_string());
            }
        }
    }
    ensure!(failures.is_empty(), "publish reconciliation pending for {}", failures.join(", "));
    Ok(())
}

// ---- Promotion adapter: one read of the merged staging→main PR. ----

#[derive(Debug, Args)]
pub struct PromotedArgs {
    /// Merged promotion pull request into the default branch
    #[arg(long)]
    pr: u64,
}

pub fn promoted(args: &PromotedArgs, root: &Path) -> Result<()> {
    use crate::mcp::tools::core::task::repo_context::{integration_branch, resolve_default_branch};
    let (repo, _) = integration_branch(root).context("project checkout unavailable")?;
    let slug = crate::github_repo::origin_slug(&repo, std::time::Duration::from_secs(3))
        .map_err(anyhow::Error::msg)?
        .context("promotion needs a GitHub origin")?;
    let output = Command::new("gh")
        .args(["pr", "view", &args.pr.to_string(), "--repo", &slug, "--json", "state,mergeCommit,baseRefName,headRefOid"])
        .output()?;
    ensure!(output.status.success(), "gh pr view {}: {}", args.pr, String::from_utf8_lossy(&output.stderr));
    let pr: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    ensure!(pr["state"] == "MERGED", "PR #{} is not merged", args.pr);
    let merge_sha = pr["mergeCommit"]["oid"].as_str().context("merged PR has no merge commit")?.to_string();
    let head_sha = pr["headRefOid"].as_str().context("PR has no head commit")?;
    let base = resolve_default_branch(&repo).map_err(anyhow::Error::msg)?;
    ensure!(pr["baseRefName"] == base.as_str(), "PR #{} did not merge into {base}", args.pr);
    git(&repo, &["fetch", "--quiet", "origin", &base])?;
    ensure!(
        crate::qa_batch::ancestor(&repo, &merge_sha, &format!("origin/{base}")) == Some(true),
        "merge commit {merge_sha} is not on origin/{base}"
    );
    let project_id = crate::cloud::resolve_canonical_id(root).context("project identity unavailable")?;
    let (mut reviewed_sha, proof_refs) = reviewed_proofs(root, &repo, &project_id, &merge_sha)?;
    if proof_refs.is_empty() {
        reviewed_sha = head_sha.to_string();
    }
    let scope = ReleaseScope {
        project_id,
        repository: slug,
        release_id: format!("pr-{}", args.pr),
        reviewed_sha,
        final_release_sha: merge_sha.clone(),
        promotion_pr: Some(args.pr),
        release_tag: None,
        constituent_epic_ids: Vec::new(),
        included_task_ids: Vec::new(),
        no_code_members: BTreeMap::new(),
        proof_refs,
    };
    let trigger = ReconcileTrigger::Promotion { pr: args.pr, merge_sha };
    println!("{}", reconcile_once(root, &repo, &scope, trigger, &LocalReconciler)?);
    Ok(())
}

// ---- Release review blocks: what the reviewer's published ledger carries. ----

#[derive(Debug, Args)]
pub struct ReviewBlocksArgs {
    /// ReleaseScope JSON naming the reviewed candidate and final release commit
    #[arg(long)]
    scope: PathBuf,
}

#[derive(Debug, Serialize)]
struct ReleaseEpics {
    project_id: String,
    release_id: String,
    reviewed_sha: String,
    final_release_sha: String,
    epics: Vec<ReleaseEpicAcceptance>,
}

#[derive(Debug, Serialize)]
struct ReleaseEpicAcceptance {
    task_id: String,
    generation: String,
    child_ids: Vec<String>,
    acceptance: &'static str,
}

const MAINTAINED_VIEWS: [(&str, &str); 3] = [
    (".claude/CODEMAP.md", "codemap"),
    ("docs/PRODUCT_OVERVIEW.md", "project-overview"),
    ("docs/qa/journeys.yaml", "cas-qa-craft"),
];

pub fn review_blocks(args: &ReviewBlocksArgs, root: &Path) -> Result<()> {
    let scope: ReleaseScope = serde_json::from_slice(&fs::read(&args.scope)?)?;
    let repo = root.parent().context("project checkout unavailable")?;
    let inventory = crate::qa_batch::resolve_release_inventory(root, repo, &scope)
        .map_err(anyhow::Error::msg)?;
    print!("{}", render_review_blocks(root, repo, &inventory)?);
    Ok(())
}

/// Both fenced blocks for the release review ledger. Epics are those every
/// child of which is terminal or ships in this release; the reviewer removes
/// any whose acceptance they did not verify before publishing.
fn render_review_blocks(root: &Path, repo: &Path, inventory: &ReleaseInventory) -> Result<String> {
    let scope = &inventory.scope;
    let blob = |rev: &str, path: &str| git(repo, &["rev-parse", "--verify", "--quiet", &format!("{rev}:{path}")]).ok();
    let mut paths = Vec::new();
    for (path, producer) in MAINTAINED_VIEWS {
        let Some(output_blob) = blob(&scope.final_release_sha, path) else { continue };
        let input_blobs: BTreeSet<_> = inventory
            .deliveries
            .iter()
            .filter_map(|d| d.anchor.as_deref().and_then(|anchor| blob(anchor, path)))
            .filter(|input| *input != output_blob)
            .collect();
        if input_blobs.is_empty() {
            continue;
        }
        let producer_blob = [
            format!("cas-cli/src/builtins/skills/{producer}/SKILL.md"),
            format!(".claude/skills/{producer}/SKILL.md"),
        ]
        .iter()
        .find_map(|skill| blob(&scope.final_release_sha, skill))
        .with_context(|| format!("{path}: producer skill {producer} is absent at the release"))?;
        paths.push(ReleasePathTransformation {
            path: path.to_string(),
            input_blobs: input_blobs.into_iter().collect(),
            output_blob,
            producing_commit: git(repo, &["log", "-1", "--format=%H", &scope.final_release_sha, "--", path])?,
            producer: producer.to_string(),
            producer_blob,
        });
    }
    let transformations = ReleaseTransformations {
        project_id: scope.project_id.clone(),
        release_id: scope.release_id.clone(),
        reviewed_sha: scope.reviewed_sha.clone(),
        final_release_sha: scope.final_release_sha.clone(),
        paths,
    };

    let store = crate::store::open_task_store(root)?;
    let held: BTreeSet<_> = inventory.exceptions.iter().map(|e| e.task_id.as_str()).collect();
    let mut finishing: BTreeSet<String> = inventory
        .deliveries
        .iter()
        .filter(|d| !held.contains(d.task_id.as_str()))
        .map(|d| d.task_id.clone())
        .collect();
    let mut pending: Vec<String> = finishing.iter().cloned().collect();
    let mut epics = BTreeMap::new();
    while let Some(id) = pending.pop() {
        let Some(epic) = store.get_parent_epic(&id)? else { continue };
        if epic.is_terminal() || epics.contains_key(&epic.id) {
            continue;
        }
        let children = store.get_subtasks(&epic.id)?;
        if !children.iter().all(|c| c.is_terminal() || finishing.contains(&c.id)) {
            continue;
        }
        let mut child_ids: Vec<_> = children.into_iter().map(|c| c.id).collect();
        child_ids.sort();
        finishing.insert(epic.id.clone());
        pending.push(epic.id.clone());
        epics.insert(epic.id.clone(), ReleaseEpicAcceptance {
            task_id: epic.id.clone(),
            generation: epic.updated_at.to_rfc3339(),
            child_ids,
            acceptance: "pass",
        });
    }
    let epics = ReleaseEpics {
        project_id: scope.project_id.clone(),
        release_id: scope.release_id.clone(),
        reviewed_sha: scope.reviewed_sha.clone(),
        final_release_sha: scope.final_release_sha.clone(),
        epics: epics.into_values().collect(),
    };
    Ok(format!(
        "```cassy-release-transformations\n{}\n```\n\n```cassy-release-epics\n{}\n```\n",
        serde_json::to_string(&transformations)?,
        serde_json::to_string(&epics)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use release_reconcile::{ReleaseNoticeIntent, ReleaseResidual};
    use std::cell::RefCell;

    /// Records each reconcile; like #3, one completed record per operation key.
    struct Recording {
        recipient: String,
        calls: RefCell<Vec<ReconcileTrigger>>,
        done: RefCell<Vec<ReleaseOperationKey>>,
    }

    impl Recording {
        fn receipt(&self, key: &ReleaseOperationKey) -> ReleaseReconcileReceipt {
            ReleaseReconcileReceipt {
                members: BTreeMap::from([("cas-0001".into(), CassyMergeClose::AlreadyClosed)]),
                residuals: vec![ReleaseResidual {
                    task_id: "cas-0002".into(),
                    gate: "QA_MISSING".into(),
                    owner: "owner".into(),
                    next_action: "record the batch QA receipt".into(),
                }],
                notice_intent: Some(ReleaseNoticeIntent {
                    key: format!("{}:{}", key.release_id, key.final_release_sha),
                    recipient: self.recipient.clone(),
                    fingerprint: "fixture".into(),
                }),
                ..Default::default()
            }
        }
    }

    impl Reconciler for Recording {
        fn completed(&self, _: &Path, key: &ReleaseOperationKey) -> Result<Option<ReleaseReconcileReceipt>> {
            Ok(self.done.borrow().contains(key).then(|| self.receipt(key)))
        }
        fn reconcile(&self, _: &Path, scope: &ReleaseScope, _: &ReleaseInventory, trigger: ReconcileTrigger) -> Result<ReleaseReconcileReceipt> {
            let key = operation_key(&scope.project_id, &scope.release_id, &scope.final_release_sha);
            self.calls.borrow_mut().push(trigger);
            self.done.borrow_mut().push(key.clone());
            Ok(self.receipt(&key))
        }
    }

    #[test]
    fn publish_and_promotion_reconcile_once_and_installed_is_a_no_op() {
        let (temp, root, train, owner, _) = super::super::release_automation::tests::fixture();
        let repo = temp.path();
        let sha = git(repo, &["rev-parse", "HEAD"]).unwrap();
        let scope = |release_id: &str| ReleaseScope {
            project_id: "release-fixture".into(),
            repository: "Example/Repo".into(),
            release_id: release_id.into(),
            reviewed_sha: sha.clone(),
            final_release_sha: sha.clone(),
            promotion_pr: None,
            release_tag: None,
            constituent_epic_ids: vec![train.id.clone()],
            included_task_ids: Vec::new(),
            no_code_members: BTreeMap::new(),
            proof_refs: Vec::new(),
        };
        let reconciler = Recording { recipient: owner.id.clone(), calls: Default::default(), done: Default::default() };
        let publish = ReconcileTrigger::Publish { tag: "v9.99.2".into() };
        let promotion = ReconcileTrigger::Promotion { pr: 4396, merge_sha: sha.clone() };
        for _ in 0..2 {
            reconcile_once(&root, repo, &scope("v9.99.2"), publish.clone(), &reconciler).unwrap();
            reconcile_once(&root, repo, &scope("pr-4396"), promotion.clone(), &reconciler).unwrap();
        }
        reconcile_once(&root, repo, &scope("v9.99.2"), ReconcileTrigger::Installed, &reconciler).unwrap();
        assert_eq!(*reconciler.calls.borrow(), vec![publish, promotion]);

        // The installed host's recovery closer sees Publish's completed record.
        git(repo, &["tag", "v9.99.2", &sha]).unwrap();
        assert!(recorded_closeout_with(&root, "v9.99.2", &reconciler).is_some());
        assert!(recorded_closeout_with(&root, "v9.99.3", &reconciler).is_none());

        // One notice per release, naming each residual with owner and next action.
        let rows = crate::store::open_prompt_queue_store(&root).unwrap().peek_all(10).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.target == owner.name
            && crate::supervisor_notices::is_action_envelope(&row.prompt)
            && row.prompt.contains("cas-0002")
            && row.prompt.contains("record the batch QA receipt")));
    }
}
