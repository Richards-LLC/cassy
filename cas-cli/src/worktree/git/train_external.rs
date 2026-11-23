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
            started: Vec::new(), tag_object: None, terminal: None,
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
}
