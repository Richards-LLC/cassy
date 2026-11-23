//! Shared entry points for Rust publishers, release CLI and workflow callers.
use std::{path::Path, process::Command, time::Duration};
use crate::worktree::{GitOperations, git::train_authority::{ExternalAction, ExternalScope, Snapshot, WorkflowScope}};
use super::provider::GitHubChecks;

const TIMEOUT: Duration = Duration::from_secs(30);

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command.current_dir(repo).args(args).env("GIT_TERMINAL_PROMPT", "0");
    let output = crate::bounded_process::run_command(&mut command, crate::bounded_process::Deadline::after(TIMEOUT), TIMEOUT)
        .map_err(|error| format!("TRAIN_TRANSFER_FENCE_UNAVAILABLE: {error:?}"))?;
    if !output.status.success() { return Err(format!("TRAIN_TRANSFER_FENCE_UNAVAILABLE: {}", String::from_utf8_lossy(&output.stderr).trim())); }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

pub(crate) fn publish_branch(core: Option<&crate::mcp::CasCore>, repo: &Path, branch: &str, head: &str, remote: &str) -> Result<(), String> {
    git(repo, &["check-ref-format", &format!("refs/heads/{branch}")])?;
    if crate::worktree::git::train_authority::fencing(repo, branch)?.is_some() {
        let core = core.ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: authenticated owner store required")?;
        let (snapshot, actor) = super::publication_fence(core, repo, branch)?
            .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: authority disappeared")?;
        if remote != "origin" { return Err("TRAIN_OWNER_PUBLICATION_REQUIRED: train publication requires its enrolled origin".into()); }
        let result = snapshot.publish(&GitOperations::new(repo.to_path_buf()), &actor, head)?;
        if !result.is_published() { return Err(format!("TRAIN_TRANSFER_CONFLICT: {result:?}")); }
        return Ok(());
    }
    crate::worktree::git::train_authority::refuse_unfenced(repo, branch)?;
    if remote.starts_with('-') || remote.is_empty() { return Err("invalid remote".into()); }
    git(repo, &["push", "--quiet", remote, &format!("{head}:refs/heads/{branch}")]).map(drop)
}

fn scope(repo: &Path, source: &str, pr: u64) -> Result<ExternalScope, String> {
    let repository = crate::github_repo::origin_slug(repo, TIMEOUT)?
        .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: canonical GitHub origin unavailable")?;
    let manifest = git(repo, &["show", &format!("{source}:cas-cli/Cargo.toml")])?;
    let parsed: toml::Value = toml::from_str(&manifest).map_err(|error| format!("invalid release manifest: {error}"))?;
    let version = parsed.get("package").and_then(|package| package.get("version")).and_then(toml::Value::as_str)
        .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: release version unavailable")?;
    let workflows = [
        (".github/workflows/release.yml", vec!["Create Release".to_string()]),
        (".github/workflows/install-path-proof.yml", vec![
            "Install path proof / Install path proof — macOS Apple Silicon".into(),
            "Install path proof / Install path proof — clean Linux container".into(),
        ]),
    ].into_iter().map(|(path, jobs)| Ok(WorkflowScope {
        path: path.into(), revision: git(repo, &["rev-parse", &format!("{source}:{path}")])?, jobs,
    })).collect::<Result<Vec<_>, String>>()?;
    Ok(ExternalScope { repository, source: source.into(), pr, tag: format!("v{version}"),
        actions: vec![ExternalAction::ArmPr, ExternalAction::PublishTag, ExternalAction::CreateRelease], workflows })
}

/// The side effect is enclosed by the reservation and its final fresh read;
/// callers cannot accidentally put another provider request after validation.
pub(crate) fn with_pr_arm<T>(core: Option<&crate::mcp::CasCore>, repo: &Path, source: &str, target: &str, pr: u64, head: &str,
    effect: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    crate::worktree::git::train_authority::refuse_unfenced(repo, target)?;
    if crate::worktree::git::train_authority::fencing(repo, source)?.is_none() {
        crate::worktree::git::train_authority::refuse_unfenced(repo, source)?;
        return effect();
    }
    let core = core.ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: authenticated owner store required")?;
    let (snapshot, actor) = super::publication_fence(core, repo, source)?
        .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: authority disappeared")?;
    if snapshot.record.target_sha != head { return Err("TRAIN_TRANSFER_TARGET_DIVERGED: PR head differs from the recorded train tip".into()); }
    let id = uuid::Uuid::new_v4().to_string();
    let reserved = snapshot.reserve_external(repo, &actor, &id, &scope(repo, head, pr)?)?;
    let started = reserved.start_external_call(repo, Some(&actor), &id, ExternalAction::ArmPr, None)?;
    started.validate_external_call(repo, Some(&actor), &id, ExternalAction::ArmPr, None)?;
    let result = effect();
    if result.is_ok() {
        started.complete_external_call(repo, Some(&actor), &id, ExternalAction::ArmPr, None)?;
    }
    // An accepted call still owes actual PR/run outcomes. An error remains
    // uncertain and cannot earn a no-publication receipt from an empty queue.
    result
}

pub(crate) fn arm_pr(core: Option<&crate::mcp::CasCore>, repo: &Path, pr: u64, expected_head: Option<&str>) -> Result<(), String> {
    let output = gh(repo, &["pr", "view", &pr.to_string(), "--json", "headRefName,baseRefName,headRefOid"])?;
    let value: serde_json::Value = serde_json::from_str(&output).map_err(|error| error.to_string())?;
    let field = |name: &str| value[name].as_str().ok_or("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING: PR identity missing".to_string());
    let (source, target, head) = (field("headRefName")?, field("baseRefName")?, field("headRefOid")?);
    if expected_head.is_some_and(|expected| expected != head) { return Err("TRAIN_TRANSFER_CONFLICT: PR head moved".into()); }
    with_pr_arm(core, repo, source, target, pr, head, || {
        gh(repo, &["pr", "merge", &pr.to_string(), "--auto", "--merge", "--match-head-commit", head]).map(drop)
    })
}

fn gh(repo: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(crate::github_repo::gh_binary());
    let slug = crate::github_repo::origin_slug(repo, TIMEOUT)?
        .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: canonical GitHub origin unavailable")?;
    command.current_dir(repo).env("GH_HOST", "github.com").env("GH_REPO", slug).args(args);
    let output = crate::bounded_process::run_command(&mut command, crate::bounded_process::Deadline::after(TIMEOUT), TIMEOUT)
        .map_err(|error| format!("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING: {error:?}"))?;
    if !output.status.success() { return Err(format!("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING: {}", String::from_utf8_lossy(&output.stderr).trim())); }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

/// A workflow checkout has no host registration. Its only continuation comes
/// from a pre-admitted reservation and provider-verified run, never its env.
fn continuation(repo: &Path, source: &str, run: Option<u64>, attempt: Option<u64>) -> Result<Option<(Snapshot, String, super::provider::VerifiedRun)>, String> {
    let snapshots = Snapshot::all(repo)?;
    let trailer = git(repo, &["log", "-1", "--format=%B", "--grep=^Release-Version:", source])?;
    let recorded: Vec<_> = trailer.lines().filter_map(|line| line.strip_prefix("Release-Train: ")).collect();
    if snapshots.is_empty() && recorded.is_empty() { return Ok(None); }
    let checks = GitHubChecks::new(repo)?;
    let (pr, head, branch) = checks.landing_pr(source)?;
    let snapshot = match snapshots.into_iter().find(|snapshot| snapshot.record.target == branch) {
        Some(snapshot) => snapshot,
        None if recorded.is_empty() => return Ok(None),
        None => return Err("TRAIN_TRANSFER_FENCE_UNAVAILABLE: recorded release train authority missing".into()),
    };
    if !recorded.is_empty() && (recorded.len() != 1 || recorded[0] != snapshot.record.train) {
        return Err("TRAIN_TRANSFER_CONFLICT: release train marker disagrees with authority".into());
    }
    let operations: Vec<_> = snapshot.record.external_operations.iter().filter(|operation|
        operation.scope.pr == pr && operation.scope.source == head && operation.terminal.is_none()).collect();
    if operations.len() != 1 { return Err("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING: release reservation missing or ambiguous".into()); }
    let id = operations[0].id.clone();
    let proof = checks.verify_publication_run(operations[0], run.ok_or("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING: actual workflow run required")?,
        attempt.ok_or("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING: actual workflow attempt required")?)?;
    let claimed = snapshot.claim_external(repo, &id, &proof)?;
    Ok(Some((claimed, id, proof)))
}

fn isolated_dry_run(repo: &Path, tag: &str, isolated: bool) -> Result<bool, String> {
    if !isolated { return Ok(false); }
    if !tag.starts_with("dryrun-") || Snapshot::all(repo)?.iter().any(|snapshot|
        snapshot.record.external_operations.iter().any(|operation| operation.scope.tag == tag))
    { return Err("TRAIN_OWNER_PUBLICATION_REQUIRED: isolated dry run cannot mutate an enrolled tag".into()); }
    Ok(true)
}

pub(crate) fn publish_tag(repo: &Path, tag: &str, source: &str, run: Option<u64>, attempt: Option<u64>, isolated: bool) -> Result<(), String> {
    git(repo, &["check-ref-format", &format!("refs/tags/{tag}")])?;
    let continuation = if isolated_dry_run(repo, tag, isolated)? { None } else { continuation(repo, source, run, attempt)? };
    let reference = format!("refs/tags/{tag}");
    let remote = git(repo, &["ls-remote", "--tags", "origin", &reference, &format!("{reference}^{{}}")])?;
    let mut refs = remote.lines().filter_map(|line| line.split_once('\t'));
    let advertised = refs.next().map(|(oid, _)| oid.to_string());
    if advertised.is_some() { git(repo, &["fetch", "--no-tags", "origin", &reference])?; }
    let object = match advertised {
        Some(object) => object,
        None => {
            git(repo, &["-c", "user.name=Cassy", "-c", "user.email=cassy@localhost", "tag", "-a", tag, "-m", tag, source])?;
            git(repo, &["rev-parse", &reference])?
        }
    };
    if git(repo, &["rev-parse", &format!("{object}^{{commit}}")])? != source {
        return Err("TRAIN_TRANSFER_TARGET_DIVERGED: tag peels to another source".into());
    }
    if let Some((snapshot, id, proof)) = continuation {
        if snapshot.external_operation(&id)?.scope.tag != tag { return Err("TRAIN_TRANSFER_CONFLICT: tag is outside the reservation".into()); }
        snapshot.publish_reserved_tag(repo, &id, &proof, &object)?;
    } else {
        // Ordinary releases retain their tag behavior, through this same CLI.
        git(repo, &["push", "origin", &format!("{object}:{reference}")])?;
    }
    Ok(())
}

pub(crate) fn create_release(repo: &Path, tag: &str, source: &str, slug: &str, run: Option<u64>, attempt: Option<u64>, isolated: bool,
    args: &[&str]) -> Result<(), String> {
    if isolated_dry_run(repo, tag, isolated)? {
        if !args.contains(&"--prerelease") { return Err("isolated dry runs require prerelease publication".into()); }
        return gh(repo, args).map(drop);
    }
    if let Some((snapshot, id, proof)) = continuation(repo, source, run, attempt)? {
        let scope = &snapshot.external_operation(&id)?.scope;
        if scope.tag != tag || !scope.repository.eq_ignore_ascii_case(slug) { return Err("TRAIN_TRANSFER_CONFLICT: release tag or repository differs".into()); }
        let started = snapshot.start_external_call(repo, None, &id, ExternalAction::CreateRelease, Some(&proof))?;
        started.validate_external_call(repo, None, &id, ExternalAction::CreateRelease, Some(&proof))?;
        gh(repo, args)?;
        return started.complete_external_call(repo, None, &id, ExternalAction::CreateRelease, Some(&proof)).map(drop);
    }
    gh(repo, args).map(drop)
}

pub(crate) fn reconcile(repo: &Path, train: &str, id: &str, run: u64, attempt: u64) -> Result<(), String> {
    let snapshot = Snapshot::read(repo, train)?;
    let checks = GitHubChecks::new(repo)?;
    let proof = checks.verify_publication_run(snapshot.external_operation(id)?, run, attempt)?;
    snapshot.settle_external(repo, id, &proof).map(drop)
}

pub(crate) fn disarm_pr(core: &crate::mcp::CasCore, repo: &Path, train: &str, id: &str) -> Result<(), String> {
    use crate::worktree::git::train_authority::{ExternalAction, Phase};
    let snapshot = Snapshot::read(repo, train)?;
    let actor = core.resolve_live_supervisor_authority().map_err(|error| format!("TRAIN_OWNER_GENERATION_CHANGED: {error:?}"))?;
    if !crate::cloud::peer_mailbox::owns_recipient(&core.cas_root, &actor, &snapshot.record.identity.owner)
        || super::identity(&actor, &snapshot.record.identity.owner)? != snapshot.record.identity
        || snapshot.record.phase != Phase::Draining
    { return Err("TRAIN_OWNER_DRAINING: disarm requires the recorded draining owner".into()); }
    let operation = snapshot.external_operation(id)?;
    if !operation.accepted.contains(&ExternalAction::ArmPr) || operation.started != [ExternalAction::ArmPr]
        || !operation.claims.is_empty() || operation.tag_object.is_some()
    { return Err("TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING: request completion remains unknown or a release chain exists".into()); }
    // This only removes an admission. Never re-arm an uncertain request.
    gh(repo, &["pr", "merge", &operation.scope.pr.to_string(), "--disable-auto"])?;
    reconcile_unpublished(repo, train, id)
}

pub(crate) fn reconcile_unpublished(repo: &Path, train: &str, id: &str) -> Result<(), String> {
    let snapshot = Snapshot::read(repo, train)?;
    let checks = GitHubChecks::new(repo)?;
    let proof = checks.verify_unpublished(&snapshot, snapshot.external_operation(id)?)?;
    snapshot.settle_unpublished_external(repo, id, &proof).map(drop)
}

pub(crate) fn is_enrolled(repo: &Path, branch: &str, known_train: Option<&str>) -> Result<bool, String> {
    let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
    if let Some(train) = known_train {
        let snapshot = Snapshot::read(repo, train)?;
        if snapshot.record.target != branch {
            return Err("TRAIN_TRANSFER_CONFLICT: recorded train target differs".into());
        }
        return Ok(true);
    }
    crate::worktree::git::train_authority::fencing(repo, branch).map(|snapshot| snapshot.is_some())
}
