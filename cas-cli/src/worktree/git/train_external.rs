//! Durable external publication reservations. An uncertain API call remains
//! pending in shared Git history; neither process exit nor elapsed time clears it.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExternalAction { ArmPr, PublishTag, CreateRelease }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkflowScope {
    pub path: String,
    /// Exact Git blob at the reserved source, compared with provider content
    /// at the actual landing SHA before a CI continuation is admitted.
    pub revision: String,
    pub jobs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExternalScope {
    pub repository: String,
    pub source: String,
    pub pr: u64,
    pub tag: String,
    pub actions: Vec<ExternalAction>,
    /// First is the release run. Remaining entries enumerate its downstream
    /// publication/proof chain; all must have provider-verified terminal runs.
    pub workflows: Vec<WorkflowScope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunClaim {
    pub workflow: String,
    pub run: u64,
    pub attempt: u64,
    pub landing: String,
    pub evidence: serde_json::Value,
    pub terminal: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExternalOperation {
    pub id: String,
    pub generation: String,
    pub launch_identity: Identity,
    pub expected_target: String,
    pub scope: ExternalScope,
    pub claims: Vec<RunClaim>,
    /// A started call is never implicitly replayed. Its outcome may be unknown
    /// even when the launching process reported an error.
    pub started: Vec<ExternalAction>,
    /// Written by the request process only after its provider call succeeds.
    /// Absence includes crashes/timeouts and must never be inferred from an
    /// empty queue: that process could still issue its admitted request later.
    #[serde(default)]
    pub accepted: Vec<ExternalAction>,
    pub tag_object: Option<String>,
    pub terminal: Option<serde_json::Value>,
}

fn pending() -> String {
    "TRAIN_TRANSFER_EXTERNAL_PUBLICATION_PENDING: reserved publication has not reconciled its whole chain".into()
}

fn conflict() -> String {
    "TRAIN_TRANSFER_CONFLICT: external reservation scope or authority changed".into()
}

fn oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl ExternalScope {
    fn validate(&self, repo: &Path) -> Result<(), String> {
        let slug: Vec<_> = self.repository.split('/').collect();
        if slug.len() != 2 || slug.iter().any(|part| part.is_empty()
            || !part.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)))
            || !oid(&self.source) || self.pr == 0 || self.actions.is_empty()
            || self.workflows.is_empty() || self.tag.is_empty()
        { return Err(conflict()); }
        command(repo, &["check-ref-format", &format!("refs/tags/{}", self.tag)], None)?;
        let git = GitOperations::new(repo.to_path_buf());
        if git.resolve_commit(&self.source).as_deref() != Some(self.source.as_str()) {
            return Err(conflict());
        }
        let mut paths = std::collections::BTreeSet::new();
        for workflow in &self.workflows {
            if !workflow.path.starts_with(".github/workflows/") || workflow.path.contains("..")
                || !oid(&workflow.revision) || workflow.jobs.is_empty()
                || workflow.jobs.iter().any(|job| job.trim().is_empty())
                || !paths.insert(&workflow.path)
                || command(repo, &["rev-parse", &format!("{}:{}", self.source, workflow.path)], None)? != workflow.revision
            { return Err(conflict()); }
        }
        if self.actions.iter().enumerate().any(|(i, action)| self.actions[..i].contains(action)) {
            return Err(conflict());
        }
        Ok(())
    }
}

impl Snapshot {
    pub(super) fn external_clear_checked(&self, checks: &impl TransferChecks) -> Result<serde_json::Value, String> {
        self.require_external_settled()?;
        checks.external_clear(self)
    }
    pub(crate) fn external_operation(&self, id: &str) -> Result<&ExternalOperation, String> {
        self.record.external_operations.iter().find(|operation| operation.id == id)
            .ok_or_else(conflict)
    }

    /// The caller authenticates the owner before reaching this Git seam.
    /// Protocol 2 fences older clients before they can erase reservation data.
    pub(crate) fn reserve_external(&self, repo: &Path, actor: &Identity, id: &str, scope: &ExternalScope) -> Result<Self, String> {
        let live = self.coherent(repo)?;
        live.require_actor(actor)?;
        if live.record.phase != Phase::Active {
            return Err(crate::gates::tag("G-113", "TRAIN_OWNER_DRAINING: external reservations are held"));
        }
        scope.validate(repo)?;
        if uuid::Uuid::parse_str(id).is_err() { return Err(conflict()); }
        if let Some(existing) = live.record.external_operations.iter().find(|operation| operation.id == id) {
            return if existing.scope == *scope && existing.generation == live.record.generation
                && existing.launch_identity == *actor && existing.expected_target == live.record.target_sha
            { Ok(live) } else { Err(conflict()) };
        }
        if live.record.external_operations.iter().any(|operation| operation.terminal.is_none()
            && operation.scope.repository == scope.repository && operation.scope.pr == scope.pr)
        { return Err(pending()); }
        let mut next = live.record.clone();
        next.protocol = 2;
        next.external_operations.push(ExternalOperation {
            id: id.into(), generation: next.generation.clone(), launch_identity: actor.clone(),
            expected_target: next.target_sha.clone(), scope: scope.clone(), claims: Vec::new(),
            started: Vec::new(), accepted: Vec::new(), tag_object: None, terminal: None,
        });
        next.operation = format!("reserve-external:{id}");
        live.append(repo, next)
    }

    pub(crate) fn require_external_settled(&self) -> Result<(), String> {
        if self.record.external_operations.iter().any(|operation| operation.terminal.is_none()) {
            return Err(pending());
        }
        Ok(())
    }

    fn continuation(&self, id: &str) -> Result<&ExternalOperation, String> {
        if !matches!(self.record.phase, Phase::Active | Phase::Draining) {
            return Err(conflict());
        }
        let operation = self.external_operation(id)?;
        if operation.generation != self.record.generation || operation.terminal.is_some() {
            return Err(conflict());
        }
        Ok(operation)
    }

    /// Claim only after the provider verifies the actual run, its source and
    /// workflow revision. A second run or attempt cannot borrow an operation.
    pub(crate) fn claim_external(&self, repo: &Path, id: &str, proof: &crate::train_owner::provider::VerifiedRun) -> Result<Self, String> {
        let live = self.coherent(repo)?;
        let operation = live.continuation(id)?;
        if !proof.matches(operation) { return Err(conflict()); }
        let claim = proof.claim();
        if let Some(existing) = operation.claims.iter().find(|existing| existing.workflow == claim.workflow) {
            return if existing.run == claim.run && existing.attempt == claim.attempt && existing.landing == claim.landing {
                Ok(live)
            } else { Err(conflict()) };
        }
        let mut next = live.record.clone();
        let operation = next.external_operations.iter_mut().find(|operation| operation.id == id).unwrap();
        let mut claim = claim.clone();
        // A claim does not by itself settle anything, even if it observed a
        // terminal workflow. Settlement is a separate provider read below.
        claim.terminal = None;
        operation.claims.push(claim);
        next.operation = format!("claim-external:{id}");
        live.append(repo, next)
    }

    fn require_claim(&self, id: &str, proof: &crate::train_owner::provider::VerifiedRun) -> Result<(), String> {
        let operation = self.continuation(id)?;
        let claim = proof.claim();
        if !proof.matches(operation) || claim.terminal.is_some()
            || !operation.claims.iter().any(|existing| existing.workflow == claim.workflow
                && existing.run == claim.run && existing.attempt == claim.attempt && existing.landing == claim.landing)
        { return Err(conflict()); }
        Ok(())
    }

    /// Record an uncertain external call BEFORE invoking it. The caller must
    /// perform `validate_external_call` immediately before the actual API call.
    pub(crate) fn start_external_call(&self, repo: &Path, actor: Option<&Identity>, id: &str, action: ExternalAction,
        proof: Option<&crate::train_owner::provider::VerifiedRun>) -> Result<Self, String> {
        let live = self.coherent(repo)?;
        let operation = live.continuation(id)?;
        if !operation.scope.actions.contains(&action) || operation.started.contains(&action)
            || action == ExternalAction::PublishTag
        { return Err(pending()); }
        match action {
            ExternalAction::ArmPr => {
                let actor = actor.ok_or_else(conflict)?;
                live.require_actor(actor)?;
                if live.record.phase != Phase::Active || operation.launch_identity != *actor
                    || operation.expected_target != live.record.target_sha
                { return Err(conflict()); }
            }
            ExternalAction::CreateRelease => {
                live.require_claim(id, proof.ok_or_else(conflict)?)?;
                if operation.tag_object.is_none() { return Err(pending()); }
            }
            ExternalAction::PublishTag => unreachable!(),
        }
        let mut next = live.record.clone();
        next.external_operations.iter_mut().find(|operation| operation.id == id).unwrap().started.push(action);
        next.operation = format!("start-external:{id}");
        live.append(repo, next)
    }

    pub(crate) fn validate_external_call(&self, repo: &Path, actor: Option<&Identity>, id: &str, action: ExternalAction,
        proof: Option<&crate::train_owner::provider::VerifiedRun>) -> Result<(), String> {
        let live = self.coherent(repo)?;
        let operation = live.continuation(id)?;
        if !operation.started.contains(&action) || !operation.scope.actions.contains(&action) { return Err(conflict()); }
        match action {
            ExternalAction::ArmPr => {
                live.require_actor(actor.ok_or_else(conflict)?)?;
                if live.record.phase != Phase::Active || operation.launch_identity != live.record.identity
                    || operation.expected_target != live.record.target_sha
                { return Err(conflict()); }
            }
            ExternalAction::CreateRelease => {
                live.require_claim(id, proof.ok_or_else(conflict)?)?;
                let tag = format!("refs/tags/{}", operation.scope.tag);
                let remote = command(repo, &["ls-remote", "--refs", "origin", &tag], None)?;
                if remote.split_whitespace().next() != operation.tag_object.as_deref() { return Err(conflict()); }
            }
            ExternalAction::PublishTag => return Err(conflict()),
        }
        Ok(())
    }

    /// Completion may arrive after prepare or a same-owner restart. Preserve
    /// the live phase/incarnation/successor; authenticate the original launch
    /// or claimed run, rather than admitting another request.
    pub(crate) fn complete_external_call(&self, repo: &Path, actor: Option<&Identity>, id: &str, action: ExternalAction,
        proof: Option<&crate::train_owner::provider::VerifiedRun>) -> Result<Self, String> {
        let live = Self::read(repo, &self.record.train)?;
        let live = live.coherent(repo)?;
        let operation = live.continuation(id)?;
        if !operation.started.contains(&action) { return Err(conflict()); }
        match action {
            ExternalAction::ArmPr if actor == Some(&operation.launch_identity) => {},
            ExternalAction::CreateRelease => live.require_claim(id, proof.ok_or_else(conflict)?)?,
            _ => return Err(conflict()),
        }
        if operation.accepted.contains(&action) { return Ok(live); }
        let mut next = live.record.clone();
        next.external_operations.iter_mut().find(|operation| operation.id == id).unwrap().accepted.push(action);
        next.operation = format!("complete-external:{id}");
        live.append(repo, next)
    }

    pub(crate) fn settle_unpublished_external(&self, repo: &Path, id: &str,
        proof: &crate::train_owner::provider::VerifiedUnpublished) -> Result<Self, String> {
        let live = self.coherent(repo)?;
        let operation = live.continuation(id)?;
        if live.record.phase != Phase::Draining || !proof.matches(operation)
            || !operation.accepted.contains(&ExternalAction::ArmPr)
            || operation.started != [ExternalAction::ArmPr]
            || !operation.claims.is_empty() || operation.tag_object.is_some()
        { return Err(pending()); }
        let mut next = live.record.clone();
        next.external_operations.iter_mut().find(|operation| operation.id == id).unwrap().terminal = Some(proof.evidence().clone());
        next.operation = format!("settle-unpublished-external:{id}");
        live.append(repo, next)
    }

    /// The annotated tag and authority advance together, including while
    /// DRAINING. The immutable successor/old generation survive unchanged.
    pub(crate) fn publish_reserved_tag(&self, repo: &Path, id: &str, proof: &crate::train_owner::provider::VerifiedRun,
        object: &str) -> Result<Self, String> {
        let live = self.coherent(repo)?;
        live.require_claim(id, proof)?;
        let operation = live.external_operation(id)?;
        if !operation.scope.actions.contains(&ExternalAction::PublishTag) || !oid(object)
            || command(repo, &["cat-file", "-t", object], None)? != "tag"
            || command(repo, &["rev-parse", &format!("{object}^{{commit}}")], None)? != proof.claim().landing
        { return Err(conflict()); }
        // Require the tag header itself to carry the enumerated tag name,
        // rather than merely accepting another annotated object at that SHA.
        let annotation = command(repo, &["cat-file", "tag", object], None)?;
        if !annotation.lines().take_while(|line| !line.is_empty()).any(|line| line == format!("tag {}", operation.scope.tag)) {
            return Err(conflict());
        }
        let tag = format!("refs/tags/{}", operation.scope.tag);
        let remote = command(repo, &["ls-remote", "--refs", "origin", &tag], None)?;
        let expected = remote.split_whitespace().next().unwrap_or("");
        if !expected.is_empty() && expected != object { return Err(conflict()); }
        if operation.tag_object.as_deref().is_some_and(|prior| prior != object) { return Err(conflict()); }
        if operation.tag_object.as_deref() == Some(object) {
            return if expected == object { Ok(live) } else { Err(conflict()) };
        }
        let mut next = live.record.clone();
        let operation = next.external_operations.iter_mut().find(|operation| operation.id == id).unwrap();
        operation.tag_object = Some(object.into());
        operation.started.push(ExternalAction::PublishTag);
        next.publication_sequence = next.publication_sequence.checked_add(1).ok_or_else(conflict)?;
        next.operation = format!("publish-tag:{id}");
        let tip = record_commit(repo, Some(&live.tip), &next)?;
        let authority_lease = format!("--force-with-lease={}:{}", live.reference, live.tip);
        let tag_lease = format!("--force-with-lease={tag}:{expected}");
        let authority_update = format!("{tip}:{}", live.reference);
        let tag_update = format!("{object}:{tag}");
        let args = ["push", "--atomic", "--porcelain", &authority_lease, &tag_lease, "origin", &authority_update, &tag_update];
        let mut probe = args.to_vec();
        probe.push("--dry-run");
        command(repo, &probe, None)?;
        let result = command(repo, &args, None);
        let confirmed = Self::read(repo, &live.record.train)?;
        if !GitOperations::new(repo.to_path_buf()).is_ancestor(&tip, &confirmed.tip)
            || confirmed.external_operation(id)?.tag_object.as_deref() != Some(object)
        { return Err(result.err().unwrap_or_else(conflict)); }
        let advertised = command(repo, &["ls-remote", "--refs", "origin", &tag], None)?;
        if advertised.split_whitespace().next() != Some(object)
            || confirmed.observed_target(repo)? != confirmed.record.target_sha
        { return Err("TRAIN_TRANSFER_TARGET_DIVERGED: tag publication readback disagrees".into()); }
        Ok(confirmed)
    }

    pub(crate) fn settle_external(&self, repo: &Path, id: &str, proof: &crate::train_owner::provider::VerifiedRun) -> Result<Self, String> {
        let live = self.coherent(repo)?;
        if !matches!(live.record.phase, Phase::Active | Phase::Draining) { return Err(conflict()); }
        let operation = live.external_operation(id)?;
        let claim = proof.claim();
        if operation.generation != live.record.generation || !proof.matches(operation) || claim.terminal.is_none()
            || !operation.claims.iter().any(|existing| existing.workflow == claim.workflow
                && existing.run == claim.run && existing.attempt == claim.attempt && existing.landing == claim.landing)
        { return Err(pending()); }
        if operation.terminal.is_some() { return Ok(live); }
        let mut next = live.record.clone();
        let operation = next.external_operations.iter_mut().find(|operation| operation.id == id).unwrap();
        operation.claims.iter_mut().find(|existing| existing.workflow == claim.workflow).unwrap().terminal = claim.terminal.clone();
        operation.terminal = claim.terminal.clone();
        next.operation = format!("settle-external:{id}");
        live.append(repo, next)
    }

    pub(crate) fn cancel_unstarted_external(&self, repo: &Path, actor: &Identity, id: &str) -> Result<Self, String> {
        let live = self.coherent(repo)?;
        live.require_actor(actor)?;
        let operation = live.continuation(id)?;
        if !operation.started.is_empty() || !operation.claims.is_empty() { return Err(pending()); }
        let mut next = live.record.clone();
        next.external_operations.iter_mut().find(|operation| operation.id == id).unwrap().terminal =
            Some(serde_json::json!({"disposition": "cancelled_before_start", "authority": live.tip}));
        next.operation = format!("cancel-unstarted-external:{id}");
        live.append(repo, next)
    }
}
