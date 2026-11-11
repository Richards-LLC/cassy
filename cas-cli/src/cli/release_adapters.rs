//! Publish and promotion adapters (close-at-release #4). A cas-src tag
//! publication and a staging→main promotion merge each reach one trusted local
//! reconciliation per (project, release, final SHA), whose durable operation
//! record holds the per-member receipt; the adapter sends one named-exception
//! notice. Installed and backfill are recovery triggers, no-ops once complete.
//! GitHub is read as evidence only: no CI job, poller or test rerun is added.
use super::release_automation::{
    ReleaseContext, ReleaseInputs, git, published_tag_sha, published_version, receipt_path, release_roots,
    tracked_context,
};
use crate::qa_batch::release_close::{
    EpicAcceptance, EpicManifest, EpicPass, ReleasePathTransformation, ReleaseTransformations,
};
use crate::issue_reconcile::IssueAction;
use crate::release_reconcile::{self, ParentOutcome, ReconcileTrigger, ReleaseReconcileReceipt};
use cas_store::release_reconcile_store::ReleaseOperationKey;
use crate::qa_batch::{
    ReleaseInventory, ReleaseProofKind, ReleaseProofRef, ReleaseScope,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use clap::Args;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

// ---- The one release reconciliation (close-at-release #3). ----

/// The two release-operation calls the adapters make; tests use a recording double.
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

/// Counts describe persisted member outcomes, independently of whether the
/// release operation itself completed. Residuals may also name parent tasks.
#[derive(Debug, Default, Serialize)]
struct ReleaseCloseoutCounts {
    closed: usize,
    already_closed: usize,
    held: usize,
    not_eligible: usize,
    residual: usize,
    complete: bool,
}

impl From<&ReleaseReconcileReceipt> for ReleaseCloseoutCounts {
    fn from(receipt: &ReleaseReconcileReceipt) -> Self {
        use crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose;
        let mut counts = Self {
            residual: receipt.residuals.len(), complete: receipt.complete, ..Self::default()
        };
        for outcome in receipt.members.values() {
            match outcome {
                CassyMergeClose::Closed { .. } => counts.closed += 1,
                CassyMergeClose::AlreadyClosed => counts.already_closed += 1,
                CassyMergeClose::Held(_) => counts.held += 1,
                CassyMergeClose::NotEligible => counts.not_eligible += 1,
            }
        }
        counts
    }
}

impl std::fmt::Display for ReleaseCloseoutCounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} member(s) closed by this operation, {} already closed, {} held, {} not eligible; {} residual(s); complete={}",
            self.closed, self.already_closed, self.held, self.not_eligible, self.residual, self.complete)
    }
}

// ---- One operation per release identity. ----

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
        "{} at {}: {}; {}",
        scope.release_id,
        scope.final_release_sha,
        if reused { "closeout reused" } else { "closeout recorded" },
        ReleaseCloseoutCounts::from(&receipt),
    ))
}

/// One named-exception notice per release: residuals and held or pending
/// parents with owner and next action, inventory exceptions and unfinished
/// GitHub issues.
fn send_notice(root: &Path, scope: &ReleaseScope, receipt: &ReleaseReconcileReceipt) -> Result<()> {
    let counts = ReleaseCloseoutCounts::from(receipt);
    let mut detail = format!(
        "{} closeout at {}: {}.",
        scope.release_id,
        scope.final_release_sha,
        counts,
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
            ParentOutcome::Held { gate, owner, next_action } => detail.push_str(&format!(
                "\n- {id}: {gate}; owner {}; next: {next_action}",
                owner.as_deref().unwrap_or("unassigned")
            )),
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
        Some(serde_json::json!({"closeout_counts": counts})),
    )?;
    Ok(())
}

// ---- Scope evidence shared by both adapters. ----

/// The newest release review whose candidate the final commit contains, plus
/// every QA/review receipt recorded at that candidate. No review: the final
/// commit with no proof, so closes hold as named residuals.
fn reviewed_proofs(root: &Path, repo: &Path, project: &str, final_sha: &str) -> Result<(String, Vec<ReleaseProofRef>)> {
    let receipts = crate::qa_batch::release_close::read_receipts(root, project).map_err(anyhow::Error::msg)?;
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

/// A published tag's scope: the tracked cut receipt's when present, else the
/// train, its tracked inputs and the reviewed candidate the tag contains.
pub(super) fn tag_scope(
    root: &Path,
    repo: &Path,
    context: Option<&ReleaseContext>,
    train_id: &str,
    tag: &str,
    tag_sha: &str,
) -> Result<ReleaseScope> {
    if let Some(scope) = context.and_then(|c| c.release_scope.as_ref()) {
        ensure!(
            scope.final_release_sha == tag_sha && scope.release_id == tag,
            "tracked release scope names {} at {}, not {tag} at {tag_sha}",
            scope.release_id,
            scope.final_release_sha
        );
        return Ok(scope.clone());
    }
    let project_id = crate::cloud::resolve_canonical_id(root).context("project identity unavailable")?;
    let repository = match context {
        Some(context) => context.repository.clone(),
        None => crate::github_repo::origin_slug(repo, std::time::Duration::from_secs(3))
            .map_err(anyhow::Error::msg)?
            .context("release has no GitHub origin")?,
    };
    let (reviewed_sha, proof_refs) = reviewed_proofs(root, repo, &project_id, tag_sha)?;
    let included_task_ids = context
        .and_then(|c| c.release_inputs.as_ref())
        .map(|inputs| inputs.included_task_ids.clone())
        .unwrap_or_default();
    Ok(ReleaseScope {
        project_id,
        repository,
        release_id: tag.to_string(),
        reviewed_sha,
        final_release_sha: tag_sha.to_string(),
        promotion_pr: None,
        release_tag: Some(tag.to_string()),
        constituent_epic_ids: vec![train_id.to_string()],
        included_task_ids,
        no_code_members: context.and_then(|c| c.release_inputs.as_ref())
            .map(|inputs| inputs.no_code_members.clone()).unwrap_or_default(),
        proof_refs,
    })
}

/// Installed/backfill recovery: the same operation as publish, with its own
/// trigger. A release whose operation already completed is a no-op.
pub(crate) fn recover(root: &Path, train_id: &str, tag: &str, trigger: ReconcileTrigger) -> Result<String> {
    recover_with(root, train_id, tag, trigger, &LocalReconciler)
}

fn recover_with(
    root: &Path,
    train_id: &str,
    tag: &str,
    trigger: ReconcileTrigger,
    reconciler: &dyn Reconciler,
) -> Result<String> {
    let (repo, _) = crate::mcp::tools::core::task::repo_context::integration_branch(root)
        .context("project checkout unavailable")?;
    let tag_sha = published_tag_sha(&repo, tag)?;
    let context: Option<ReleaseContext> = match receipt_path(root, tag.trim_start_matches('v')) {
        Ok(path) if path.exists() => Some(serde_json::from_slice(&fs::read(path)?)?),
        _ => None,
    };
    let scope = tag_scope(root, &repo, context.as_ref(), train_id, tag, &tag_sha)?;
    reconcile_once(root, &repo, &scope, trigger, reconciler)
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
            let scope = tag_scope(&root, &repo, Some(&context), &context.train_id, &tag, &tag_sha)?;
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
    /// Audited explicit delivery IDs and portable no-code proof bindings
    #[arg(long)]
    inputs: Option<PathBuf>,
}

pub fn promoted(args: &PromotedArgs, root: &Path) -> Result<()> {
    println!("{}", promoted_with(root, args.pr, args.inputs.as_deref(), &LocalReconciler)?);
    Ok(())
}

fn gh() -> std::ffi::OsString {
    std::env::var_os(crate::qa_pass::github_gate::GH_BIN_ENV)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "gh".into())
}

fn gh_pr_view(repo: &Path, slug: &str, pr: u64) -> Result<serde_json::Value> {
    let output = Command::new(gh())
        .args(["pr", "view", &pr.to_string(), "--repo", slug, "--json", "state,mergeCommit,baseRefName,headRefOid"])
        .current_dir(repo)
        .output()?;
    ensure!(output.status.success(), "gh pr view {pr}: {}", String::from_utf8_lossy(&output.stderr));
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn promoted_with(root: &Path, pr: u64, inputs: Option<&Path>, reconciler: &dyn Reconciler) -> Result<String> {
    use crate::mcp::tools::core::task::repo_context::{integration_branch, resolve_default_branch};
    let (repo, _) = integration_branch(root).context("project checkout unavailable")?;
    let slug = crate::github_repo::origin_slug(&repo, std::time::Duration::from_secs(3))
        .map_err(anyhow::Error::msg)?
        .context("promotion needs a GitHub origin")?;
    let view = gh_pr_view(&repo, &slug, pr)?;
    ensure!(view["state"] == "MERGED", "PR #{pr} is not merged yet; run `cas release promoted --pr {pr}` after it lands");
    let base = resolve_default_branch(&repo).map_err(anyhow::Error::msg)?;
    if view["baseRefName"] != base.as_str() {
        return Ok(format!("PR #{pr} did not merge into {base}: not a promotion, nothing to reconcile"));
    }
    let merge_sha = view["mergeCommit"]["oid"].as_str().context("merged PR has no merge commit")?.to_string();
    let head_sha = view["headRefOid"].as_str().context("PR has no head commit")?;
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
    let inputs: ReleaseInputs = inputs.map(|path| -> Result<_> {
        let inputs: ReleaseInputs = serde_json::from_slice(&fs::read(path)?)?;
        ensure!(inputs.checked_sha == head_sha || inputs.checked_sha == merge_sha,
            "release input audit does not identify this promotion");
        Ok(inputs)
    }).transpose()?.unwrap_or_default();
    let scope = ReleaseScope {
        project_id,
        repository: slug,
        release_id: format!("pr-{pr}"),
        reviewed_sha,
        final_release_sha: merge_sha.clone(),
        promotion_pr: Some(pr),
        release_tag: None,
        constituent_epic_ids: Vec::new(),
        included_task_ids: inputs.included_task_ids,
        no_code_members: inputs.no_code_members,
        proof_refs,
    };
    let report = reconcile_once(root, &repo, &scope, ReconcileTrigger::Promotion { pr, merge_sha }, reconciler)?;
    match fs::remove_file(pending_promotion_path(root, pr)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    Ok(report)
}

/// The integration branch a project promotes to its default branch by PR, if
/// it releases that way. Release trains publish tags instead.
fn promotion_base(root: &Path, repo: &Path) -> Option<String> {
    let factory = crate::config::Config::load(root).ok()?.factory();
    let staging = factory.epic_base_branch.filter(|_| !factory.release_train)?;
    let default = crate::mcp::tools::core::task::repo_context::resolve_default_branch(repo).ok()?;
    (staging != default).then_some(default)
}

/// PostToolUse: a supervisor's successful `gh pr merge` of a staging→main
/// promotion starts the release's one reconciliation in the background. Any
/// other command or role returns at the first check, before any I/O.
pub(crate) fn observe_promotion_merge(cas_root: &Path, cwd: &Path, command: &str, owner: &str) -> Option<String> {
    observe_promotion_merge_with(cas_root, cwd, command, owner, &|pr| spawn_promoted(cas_root, cwd, pr))
}

fn observe_promotion_merge_with(
    cas_root: &Path,
    cwd: &Path,
    command: &str,
    owner: &str,
    launch: &dyn Fn(u64) -> Result<()>,
) -> Option<String> {
    if !command.contains("pr merge") || std::env::var("CAS_AGENT_ROLE").as_deref() != Ok("supervisor") {
        return None;
    }
    let (repo, _) = crate::mcp::tools::core::task::repo_context::integration_branch(cas_root)?;
    let default = promotion_base(cas_root, &repo)?;
    let slug = crate::github_repo::origin_slug(&repo, std::time::Duration::from_secs(3)).ok()??;
    let mut lines = Vec::new();
    for merge in crate::qa_pass::github_gate::github_merges_in(command) {
        let Ok(pr) = crate::qa_pass::github_gate::merge_pr_number(cwd, &merge) else { continue };
        let Ok(view) = gh_pr_view(&repo, merge.repo.as_deref().unwrap_or(&slug), pr) else { continue };
        if view["baseRefName"] != default.as_str() {
            continue;
        }
        let result = if view["state"] == "MERGED" {
            launch(pr)
        } else {
            view["headRefOid"]
                .as_str()
                .context("PR has no head commit")
                .and_then(|head| arm_pending_promotion(cas_root, owner, pr, &default, head))
        };
        if let Err(error) = result {
            lines.push(format!("Promotion closeout for PR #{pr} did not start ({error:#}); run `cas release promoted --pr {pr}` once it lands."));
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

// ---- Pending promotion: the daemon's existing landing observer finishes it. ----

/// Message prefix of the `branch_contained_in` reminder that carries a
/// pending promotion; the daemon runs [`promotion_from_reminder`] before firing it.
const PROMOTION_REMINDER: &str = "PROMOTION CLOSEOUT pr=";

#[derive(Debug, Serialize, Deserialize)]
struct PendingPromotion {
    project_id: String,
    pr: u64,
    base: String,
    head: String,
}

fn pending_promotion_path(cas_root: &Path, pr: u64) -> PathBuf {
    cas_root.join("release-reconcile").join("pending-promotions").join(format!("pr-{pr}.json"))
}

/// A queued or auto-merge promotion: persist its intent and arm the existing
/// `branch_contained_in` landing observation of its head on the default branch.
fn arm_pending_promotion(cas_root: &Path, owner: &str, pr: u64, base: &str, head: &str) -> Result<()> {
    let intent = PendingPromotion {
        project_id: crate::cloud::resolve_canonical_id(cas_root).context("project identity unavailable")?,
        pr,
        base: base.to_string(),
        head: head.to_string(),
    };
    let path = pending_promotion_path(cas_root, pr);
    let dir = path.parent().context("pending promotion directory")?;
    fs::create_dir_all(dir)?;
    let tmp = tempfile::NamedTempFile::new_in(dir)?;
    fs::write(tmp.path(), serde_json::to_vec(&intent)?)?;
    tmp.persist(&path)?;
    let filter = serde_json::json!({"commit": head, "target_branch": base});
    let reminders = crate::store::open_reminder_store(cas_root)?;
    let message = format!("{PROMOTION_REMINDER}{pr}: PR #{pr} landed on {base}; its release closeout started in the background");
    if !reminders.list_all_pending()?.iter().any(|r| r.message == message && r.trigger_filter.as_ref() == Some(&filter)) {
        reminders.create_with_scope(
            owner,
            None,
            &message,
            cas_store::ReminderTriggerType::Event,
            None,
            Some("branch_contained_in"),
            Some(&filter),
            7 * 24 * 3600,
            None,
            None,
            true,
            None,
        )?;
    }
    Ok(())
}

/// The daemon's landing observer calls this before firing a pending
/// promotion's reminder; an error keeps the reminder pending for the next scan.
pub(crate) fn promotion_from_reminder(cas_root: &Path, reminder: &cas_store::Reminder) -> Result<()> {
    let project = cas_root.parent().context("project checkout unavailable")?;
    promotion_from_reminder_with(cas_root, reminder, &|pr| spawn_promoted(cas_root, project, pr))
}

fn promotion_from_reminder_with(
    cas_root: &Path,
    reminder: &cas_store::Reminder,
    launch: &dyn Fn(u64) -> Result<()>,
) -> Result<()> {
    if reminder.trigger_event.as_deref() != Some("branch_contained_in") {
        return Ok(());
    }
    let Some(pr) = reminder
        .message
        .strip_prefix(PROMOTION_REMINDER)
        .and_then(|rest| rest.split(':').next())
        .and_then(|pr| pr.parse::<u64>().ok())
    else {
        return Ok(());
    };
    if !pending_promotion_path(cas_root, pr).exists() {
        return Ok(()); // already reconciled and retired
    }
    launch(pr)
}

/// Detached `cas release promoted`; its output goes to the release-reconcile log.
fn spawn_promoted(cas_root: &Path, cwd: &Path, pr: u64) -> Result<()> {
    let dir = cas_root.join("release-reconcile");
    fs::create_dir_all(&dir)?;
    let log = fs::OpenOptions::new().create(true).append(true).open(dir.join("promotion.log"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["release", "promoted", "--pr", &pr.to_string()])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command.spawn()?;
    Ok(())
}

// ---- Release review blocks: what the reviewer's published ledger carries. ----

const MAINTAINED_VIEWS: [(&str, &str); 3] = [
    (".claude/CODEMAP.md", "codemap"),
    ("docs/PRODUCT_OVERVIEW.md", "project-overview"),
    ("docs/qa/journeys.yaml", "cas-qa-craft"),
];

/// Both blocks for the ReleaseScope JSON at `scope_path`, bound to that scope.
/// `cas qa ledger init --release-scope` writes them into the review ledger,
/// so the published RELEASE_VERDICT bytes carry them.
pub(crate) fn review_blocks_for_scope(root: &Path, scope_path: &Path) -> Result<String> {
    let scope: ReleaseScope = serde_json::from_slice(&fs::read(scope_path)?)?;
    let (repo, _) = crate::mcp::tools::core::task::repo_context::integration_branch(root)
        .context("project checkout unavailable")?;
    let inventory = crate::qa_batch::resolve_release_inventory(root, &repo, &scope)
        .map_err(anyhow::Error::msg)?;
    render_review_blocks(root, &repo, &inventory)
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
        // child_ids is the full recursive subtree (get_subtasks), nested epics included.
        let children = store.get_subtasks(&epic.id)?;
        if !children.iter().all(|c| c.is_terminal() || finishing.contains(&c.id)) {
            continue;
        }
        let mut child_ids: Vec<_> = children.into_iter().map(|c| c.id).collect();
        child_ids.sort();
        finishing.insert(epic.id.clone());
        pending.push(epic.id.clone());
        epics.insert(epic.id.clone(), EpicAcceptance {
            task_id: epic.id.clone(),
            generation: epic.updated_at.to_rfc3339(),
            child_ids,
            acceptance: EpicPass::Pass,
        });
    }
    let epics = EpicManifest {
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
    use crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose;
    use release_reconcile::{ReleaseNoticeIntent, ReleaseResidual};
    use std::cell::RefCell;

    /// Records each reconcile; like #3, one completed record per operation key.
    struct Recording {
        recipient: String,
        calls: RefCell<Vec<ReconcileTrigger>>,
        done: RefCell<Vec<ReleaseReconcileReceipt>>,
    }

    impl Recording {
        fn receipt(&self, key: ReleaseOperationKey, scope: &ReleaseScope, trigger: ReconcileTrigger) -> ReleaseReconcileReceipt {
            ReleaseReconcileReceipt {
                notice_intent: Some(ReleaseNoticeIntent {
                    key: format!("{}:{}", key.release_id, key.final_release_sha),
                    recipient: self.recipient.clone(),
                    fingerprint: "fixture".into(),
                }),
                key,
                scope: scope.clone(),
                triggers: vec![trigger],
                members: BTreeMap::from([("cas-0001".into(), CassyMergeClose::AlreadyClosed)]),
                close_intents: BTreeMap::new(),
                issue_outcomes: Vec::new(),
                parents: BTreeMap::new(),
                exceptions: Vec::new(),
                started_at: String::new(),
                updated_at: String::new(),
                complete: true,
                sync_pending: Vec::new(),
                residuals: vec![ReleaseResidual {
                    task_id: "cas-0002".into(),
                    gate: "QA_MISSING".into(),
                    owner: "owner".into(),
                    next_action: "record the batch QA receipt".into(),
                }],
            }
        }
    }

    impl Reconciler for Recording {
        fn completed(&self, _: &Path, key: &ReleaseOperationKey) -> Result<Option<ReleaseReconcileReceipt>> {
            Ok(self.done.borrow().iter().find(|r| &r.key == key).cloned())
        }
        fn reconcile(&self, _: &Path, scope: &ReleaseScope, _: &ReleaseInventory, trigger: ReconcileTrigger) -> Result<ReleaseReconcileReceipt> {
            let key = operation_key(&scope.project_id, &scope.release_id, &scope.final_release_sha);
            self.calls.borrow_mut().push(trigger.clone());
            let receipt = self.receipt(key, scope, trigger);
            self.done.borrow_mut().push(receipt.clone());
            Ok(receipt)
        }
    }

    #[test]
    fn publish_and_promotion_reconcile_once_and_installed_is_a_no_op() {
        let (temp, root, train, owner, _) = super::super::release_automation::tests::release_fixture();
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

        // One notice per release, naming each residual with owner and next action.
        let rows = crate::store::open_prompt_queue_store(&root).unwrap().peek_all(10).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.target == owner.name
            && crate::supervisor_notices::is_action_envelope(&row.prompt)
            && row.prompt.contains("cas-0002")));

        // Replay the captured all-held incident and a mixed operation through
        // the real notice outbox. Counters are protocol data, not prose pins.
        let all_held = (0..58).map(|i| (format!("cas-{i:04x}"), CassyMergeClose::Held("QA_MISSING".into())))
            .collect::<BTreeMap<_, _>>();
        let mixed = BTreeMap::from([
            ("cas-a001".into(), CassyMergeClose::Closed { notice: String::new() }),
            ("cas-a002".into(), CassyMergeClose::AlreadyClosed),
            ("cas-a003".into(), CassyMergeClose::Held("QA_MISSING".into())),
            ("cas-a004".into(), CassyMergeClose::NotEligible),
        ]);
        for (release, members, residuals, expected) in [
            ("all-held", all_held, 46, serde_json::json!({
                "closed": 0, "already_closed": 0, "held": 58, "not_eligible": 0,
                "residual": 46, "complete": false,
            })),
            ("mixed", mixed, 2, serde_json::json!({
                "closed": 1, "already_closed": 1, "held": 1, "not_eligible": 1,
                "residual": 2, "complete": false,
            })),
            ("complete", BTreeMap::new(), 0, serde_json::json!({
                "closed": 0, "already_closed": 0, "held": 0, "not_eligible": 0,
                "residual": 0, "complete": true,
            })),
        ] {
            let scope = scope(release);
            let mut receipt = reconciler.receipt(
                operation_key(&scope.project_id, release, &sha), &scope, ReconcileTrigger::Installed,
            );
            receipt.members = members;
            receipt.residuals = (0..residuals).map(|i| ReleaseResidual {
                task_id: format!("cas-{i:04x}"), gate: "QA_MISSING".into(),
                owner: owner.id.clone(), next_action: "record proof".into(),
            }).collect();
            receipt.complete = expected["complete"].as_bool().unwrap();
            for _ in 0..2 { send_notice(&root, &scope, &receipt).unwrap(); }
            let key = format!("supervisor-notice:release_closeout:{}", receipt.notice_intent.unwrap().key);
            let row = crate::store::open_supervisor_queue_store(&root).unwrap()
                .get_by_transition_key(&key).unwrap().unwrap();
            let payload: serde_json::Value = serde_json::from_str(&row.payload).unwrap();
            assert_eq!(payload["context"]["closeout_counts"], expected);
        }
        assert_eq!(crate::store::open_prompt_queue_store(&root).unwrap().peek_all(10).unwrap().len(), 5);
    }

    /// The review ledger scaffold publishes both blocks for its scope, and #2's
    /// and #3's parsers read them back; epic child_ids are the full subtree.
    #[test]
    fn release_ledger_blocks_round_trip_with_recursive_epic_subtrees() {
        use cas_types::{Dependency, DependencyType, FactoryBranchStart, Task, TaskStatus, TaskType, WorkTarget};
        let (temp, root, _, _, _) = super::super::release_automation::tests::release_fixture();
        let repo = temp.path();
        let store = crate::store::open_task_store(&root).unwrap();
        let mut base = git(repo, &["rev-parse", "HEAD"]).unwrap();
        for (id, epic) in [("cas-e000", true), ("cas-e001", true), ("cas-d001", false), ("cas-d002", false)] {
            let mut task = Task::new(id.into(), id.into());
            task.origin_project = Some("release-fixture".into());
            if epic {
                task.task_type = TaskType::Epic;
            } else {
                fs::write(repo.join(format!("{id}.txt")), id).unwrap();
                git(repo, &["add", "."]).unwrap();
                git(repo, &["commit", "-m", id]).unwrap();
                let anchor = git(repo, &["rev-parse", "HEAD"]).unwrap();
                task.status = TaskStatus::AwaitingMerge;
                task.deliverables.factory_branch_anchor = Some(anchor.clone());
                task.deliverables.factory_branch_start = Some(FactoryBranchStart {
                    work_target: WorkTarget { repo_selector: "project:release-fixture".into(), target_branch: "main".into() },
                    branch: format!("factory/{id}"),
                    assignee: "worker".into(),
                    base_sha: std::mem::replace(&mut base, anchor),
                });
            }
            store.add(&task).unwrap();
        }
        for (child, parent) in [("cas-e001", "cas-e000"), ("cas-d001", "cas-e001"), ("cas-d002", "cas-e000")] {
            store.add_dependency(&Dependency::new(child.into(), parent.into(), DependencyType::ParentChild)).unwrap();
        }
        let scope = ReleaseScope {
            project_id: "release-fixture".into(),
            repository: "Example/Repo".into(),
            release_id: "v9.99.2".into(),
            reviewed_sha: base.clone(),
            final_release_sha: base.clone(),
            promotion_pr: None,
            release_tag: None,
            constituent_epic_ids: vec!["cas-e000".into()],
            included_task_ids: Vec::new(),
            no_code_members: BTreeMap::new(),
            proof_refs: Vec::new(),
        };
        let scope_path = root.join("scope.json");
        fs::write(&scope_path, serde_json::to_vec(&scope).unwrap()).unwrap();

        let ledger = review_blocks_for_scope(&root, &scope_path).unwrap();
        let transformations = crate::qa_batch::release_close::parse_manifest(&ledger).unwrap().unwrap();
        assert_eq!((transformations.release_id.as_str(), transformations.final_release_sha.as_str()), ("v9.99.2", base.as_str()));
        let json = ledger.split("```cassy-release-epics\n").nth(1).unwrap().split("\n```").next().unwrap();
        let epics: EpicManifest = serde_json::from_str(json).unwrap();
        assert_eq!((epics.release_id.as_str(), epics.final_release_sha.as_str()), ("v9.99.2", base.as_str()));
        let subtrees: BTreeMap<_, _> = epics.epics.into_iter().map(|e| (e.task_id, e.child_ids)).collect();
        assert_eq!(subtrees, BTreeMap::from([
            ("cas-e000".to_string(), vec!["cas-d001".to_string(), "cas-d002".into(), "cas-e001".into()]),
            ("cas-e001".to_string(), vec!["cas-d001".to_string()]),
        ]));
    }

    /// Recovery runs the real shared operation even when a delivery is held:
    /// the durable record, its notice intent and exactly one notice exist.
    #[test]
    fn recovery_with_a_held_delivery_records_the_shared_operation_and_notice() {
        use cas_types::{Dependency, DependencyType, FactoryBranchStart, Task, TaskStatus, WorkTarget};
        let (temp, root, mut train, _, _) = super::super::release_automation::tests::release_fixture();
        let repo = temp.path();
        let base = git(repo, &["rev-parse", "HEAD"]).unwrap();
        fs::write(repo.join("held.txt"), "held\n").unwrap();
        git(repo, &["add", "held.txt"]).unwrap();
        git(repo, &["commit", "-m", "held delivery"]).unwrap();
        let anchor = git(repo, &["rev-parse", "HEAD"]).unwrap();
        git(repo, &["tag", "-a", "v9.99.2", "-m", "release"]).unwrap();
        // Publish the tag to a local origin while keeping the GitHub identity.
        let bare = temp.path().join("origin.git");
        git(repo, &["init", "--bare", bare.to_str().unwrap()]).unwrap();
        git(repo, &["config", &format!("url.{}.insteadOf", bare.display()), "https://github.com/Example/Repo.git"]).unwrap();
        git(repo, &["push", "origin", "v9.99.2"]).unwrap();

        let store = crate::store::open_task_store(&root).unwrap();
        train.origin_project = Some("release-fixture".into());
        store.update(&train).unwrap();
        let mut held = Task::new("cas-b001".into(), "Held delivery".into());
        held.origin_project = Some("release-fixture".into());
        held.status = TaskStatus::AwaitingMerge;
        held.deliverables.factory_branch_anchor = Some(anchor.clone());
        held.deliverables.factory_branch_start = Some(FactoryBranchStart {
            work_target: WorkTarget { repo_selector: "project:release-fixture".into(), target_branch: "main".into() },
            branch: "factory/held".into(),
            assignee: "worker".into(),
            base_sha: base,
        });
        store.add(&held).unwrap();
        store.add_dependency(&Dependency::new(held.id.clone(), train.id.clone(), DependencyType::ParentChild)).unwrap();

        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let _cwd = env.enter_dir(repo);
        for _ in 0..2 {
            recover(&root, &train.id, "v9.99.2", ReconcileTrigger::Installed).unwrap();
        }
        let key = operation_key("release-fixture", "v9.99.2", &anchor);
        let record: ReleaseReconcileReceipt = serde_json::from_value(
            cas_store::release_reconcile_store::load_release_operation(&root, &key).unwrap().unwrap(),
        ).unwrap();
        assert!(!record.complete);
        assert!(matches!(record.members.get("cas-b001"), Some(CassyMergeClose::Held(_))));
        assert_eq!(record.triggers.first(), Some(&ReconcileTrigger::Installed));
        assert!(record.notice_intent.is_some());
        assert_eq!(store.get("cas-b001").unwrap().status, TaskStatus::AwaitingMerge);
        let notices: Vec<_> = crate::store::open_prompt_queue_store(&root).unwrap().peek_all(10).unwrap()
            .into_iter().filter(|row| row.prompt.contains("kind=\"release_closeout\"")).collect();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].prompt.contains("cas-b001"));
    }

    /// A supervisor's queued staging→main promotion PR is reconciled when the
    /// daemon's landing observer sees it on main: one Promotion call, through
    /// the reminder's typed action, and a later merged observation is a no-op.
    #[test]
    fn queued_promotion_reconciles_once_when_its_landing_is_observed() {
        let (temp, root, _, owner, _) = super::super::release_automation::tests::release_fixture();
        let repo = temp.path();
        fs::write(root.join("config.toml"), "[project]\ncanonical_id = 'release-fixture'\n[factory]\nepic_base_branch = 'staging'\n").unwrap();
        git(repo, &["branch", "staging"]).unwrap();
        let merge_sha = git(repo, &["rev-parse", "main"]).unwrap();
        let bare = temp.path().join("origin.git");
        git(repo, &["init", "--bare", bare.to_str().unwrap()]).unwrap();
        git(repo, &["config", &format!("url.{}.insteadOf", bare.display()), "https://github.com/Example/Repo.git"]).unwrap();
        git(repo, &["push", "origin", "main"]).unwrap();
        let view = temp.path().join("pr-view.json");
        let set_state = |state: &str| fs::write(&view, serde_json::json!({
            "state": state, "baseRefName": "main",
            "mergeCommit": {"oid": merge_sha}, "headRefOid": merge_sha,
        }).to_string()).unwrap();
        set_state("OPEN");
        let gh = temp.path().join("gh");
        fs::write(&gh, format!("#!/bin/sh
cat '{}'
", view.display())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set(crate::qa_pass::github_gate::GH_BIN_ENV, &gh);
        let reconciler = Recording { recipient: owner.id.clone(), calls: Default::default(), done: Default::default() };
        let launch = |pr: u64| promoted_with(&root, pr, None, &reconciler).map(drop);
        let command = ["gh", "pr", "merge", "4396", "--auto", "--merge"].join(" ");
        env.set("CAS_AGENT_ROLE", "worker");
        assert!(observe_promotion_merge_with(&root, repo, &command, &owner.id, &launch).is_none());
        assert!(!pending_promotion_path(&root, 4396).exists());

        // Queued: no reconcile yet; a pending intent and its landing watch.
        env.set("CAS_AGENT_ROLE", "supervisor");
        assert!(observe_promotion_merge_with(&root, repo, &command, &owner.id, &launch).is_none());
        assert!(reconciler.calls.borrow().is_empty());
        let reminders = crate::store::open_reminder_store(&root).unwrap();
        let watch: Vec<_> = reminders.list_all_pending().unwrap().into_iter()
            .filter(|r| r.message.starts_with(PROMOTION_REMINDER)).collect();
        assert_eq!(watch.len(), 1);
        assert_eq!(watch[0].trigger_filter, Some(serde_json::json!({"commit": merge_sha, "target_branch": "main"})));

        // The observer sees the landing: the typed action reconciles once and retires the intent.
        set_state("MERGED");
        for _ in 0..2 {
            promotion_from_reminder_with(&root, &watch[0], &launch).unwrap();
        }
        assert!(!pending_promotion_path(&root, 4396).exists());
        let merged = ["gh", "pr", "merge", "4396", "--merge"].join(" ");
        assert!(observe_promotion_merge_with(&root, repo, &merged, &owner.id, &launch).is_none());
        assert_eq!(*reconciler.calls.borrow(), vec![ReconcileTrigger::Promotion { pr: 4396, merge_sha }]);
    }
}
