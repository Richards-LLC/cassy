//! Only authenticated provider observations construct a continuation proof.
//! Caller operation/run ids are lookup keys, never capabilities on their own.
use super::*;
use crate::worktree::git::train_authority::{ExternalAction, ExternalOperation, Phase, RunClaim};

pub(crate) struct VerifiedUnpublished {
    operation: ExternalOperation,
    evidence: Value,
}

impl VerifiedUnpublished {
    pub(crate) fn matches(&self, operation: &ExternalOperation) -> bool {
        self.operation.id == operation.id && self.operation.generation == operation.generation
            && self.operation.launch_identity == operation.launch_identity
            && self.operation.expected_target == operation.expected_target && self.operation.scope == operation.scope
            && self.operation.started == operation.started && self.operation.accepted == operation.accepted
    }
    pub(crate) fn evidence(&self) -> &Value { &self.evidence }
}

pub(crate) struct VerifiedRun {
    operation: ExternalOperation,
    claim: RunClaim,
}

impl VerifiedRun {
    pub(crate) fn matches(&self, operation: &ExternalOperation) -> bool {
        self.operation.id == operation.id && self.operation.generation == operation.generation
            && self.operation.launch_identity == operation.launch_identity
            && self.operation.expected_target == operation.expected_target
            && self.operation.scope == operation.scope
    }

    pub(crate) fn claim(&self) -> &RunClaim { &self.claim }
}

fn same_slug(value: &Value, expected: &str) -> bool {
    value.as_str().is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

impl<P: Provider> GitHubChecks<P> {
    /// A completed arm can be disarmed without a landing run. A fresh empty
    /// queue cannot settle an unfinished/uncertain call, which may still arm
    /// after this observation. Never grant this proof for a claimed CI chain.
    pub(crate) fn verify_unpublished(&self, snapshot: &Snapshot, operation: &ExternalOperation) -> Result<VerifiedUnpublished, String> {
        let scope = &operation.scope;
        if snapshot.record.phase != Phase::Draining || operation.generation != snapshot.record.generation
            || !scope.repository.eq_ignore_ascii_case(&self.slug)
            || !operation.accepted.contains(&ExternalAction::ArmPr)
            || operation.started != [ExternalAction::ArmPr] || !operation.claims.is_empty()
            || operation.tag_object.is_some() || operation.terminal.is_some()
        { return Err(pending("request completion is unknown or a release continuation exists")); }
        let clear = self.observe_clear(snapshot)?;
        let observed = clear["prs"].as_array().and_then(|prs| prs.iter().find(|pr| pr["number"].as_u64() == Some(scope.pr)))
            .ok_or_else(|| pending("reserved PR queue observation missing"))?;
        if !matches!(observed["state"].as_str(), Some("OPEN" | "CLOSED"))
            || !observed["autoMergeRequest"].is_null() || !observed["mergeQueueEntry"].is_null()
        { return Err(pending("reserved PR remains armed, queued or merged")); }
        let pr = self.provider.publication(&format!("pulls/{}", scope.pr))?;
        if pr["number"].as_u64() != Some(scope.pr) || pr["merged"].as_bool() != Some(false)
            || !pr.get("merged_at").is_some_and(Value::is_null)
            || pr["head"]["ref"].as_str() != Some(snapshot.record.target.as_str())
            || !same_slug(&pr["head"]["repo"]["full_name"], &self.slug)
            || !same_slug(&pr["base"]["repo"]["full_name"], &self.slug)
        { return Err(pending("unmerged PR identity/outcome unavailable")); }
        let publication = self.provider.release(&scope.tag)?;
        let repository = &publication["data"]["repository"];
        if !same_slug(&repository["nameWithOwner"], &self.slug)
            || !repository.get("release").is_some_and(Value::is_null)
            || !repository.get("ref").is_some_and(Value::is_null)
        { return Err(pending("reserved tag or release exists or its outcome is unknown")); }
        Ok(VerifiedUnpublished { operation: operation.clone(),
            evidence: json!({"disposition":"disarmed_without_landing", "pr":pr,"clear":clear,"publication":publication}) })
    }

    pub(crate) fn landing_pr(&self, sha: &str) -> Result<(u64, String, String), String> {
        let prs = self.provider.publication(&format!("commits/{sha}/pulls?per_page=100"))?;
        let rows = prs.as_array().ok_or_else(|| pending("landing PR association unavailable"))?;
        if rows.len() >= 100 { return Err(pending("landing PR association truncated")); }
        let mut matches = Vec::new();
        for pr in rows {
            if pr["merge_commit_sha"].as_str() != Some(sha) || pr["merged_at"].as_str().is_none() { continue; }
            if !same_slug(&pr["head"]["repo"]["full_name"], &self.slug)
                || !same_slug(&pr["base"]["repo"]["full_name"], &self.slug) { return Err(pending("landing PR repository disagrees")); }
            matches.push((pr["number"].as_u64().ok_or_else(|| pending("PR number missing"))?,
                pr["head"]["sha"].as_str().ok_or_else(|| pending("PR head missing"))?.to_string(),
                pr["head"]["ref"].as_str().ok_or_else(|| pending("PR branch missing"))?.to_string()));
        }
        if matches.len() != 1 { return Err(pending("landing PR association ambiguous or missing")); }
        Ok(matches.remove(0))
    }

    /// Local reusable workflows share the root run, so terminal root status
    /// covers the downstream install proof and publication jobs as well.
    /// Older independently dispatched/unknown chains cannot earn this proof.
    pub(crate) fn verify_publication_run(&self, operation: &ExternalOperation, run: u64, attempt: u64) -> Result<VerifiedRun, String> {
        let scope = &operation.scope;
        if !scope.repository.eq_ignore_ascii_case(&self.slug) || run == 0 || attempt == 0 {
            return Err(pending("run repository or identity disagrees"));
        }
        let workflow = scope.workflows.first().ok_or_else(|| pending("workflow scope missing"))?;
        let pr = self.provider.publication(&format!("pulls/{}", scope.pr))?;
        if pr["number"].as_u64() != Some(scope.pr) || pr["merged"].as_bool() != Some(true)
            || pr.pointer("/head/sha").and_then(Value::as_str) != Some(scope.source.as_str())
            || !same_slug(&pr["head"]["repo"]["full_name"], &self.slug)
            || !same_slug(&pr["base"]["repo"]["full_name"], &self.slug)
        { return Err(pending("merged PR/source linkage unavailable")); }
        let landing = pr["merge_commit_sha"].as_str().filter(|sha| !sha.is_empty())
            .ok_or_else(|| pending("PR landing SHA unavailable"))?;
        let observed = self.provider.publication(&format!("actions/runs/{run}/attempts/{attempt}"))?;
        if observed["id"].as_u64() != Some(run) || observed["run_attempt"].as_u64() != Some(attempt)
            || observed["head_sha"].as_str() != Some(landing)
            || observed["path"].as_str() != Some(workflow.path.as_str())
            || observed["event"] != "push"
            || !same_slug(&observed["repository"]["full_name"], &self.slug)
            || !same_slug(&observed["head_repository"]["full_name"], &self.slug)
        { return Err(pending("run workflow/repository/landing association disagrees")); }
        // A rerun is a distinct attempted publisher. A proof for a completed
        // older attempt cannot settle an active latest attempt.
        let latest = self.provider.publication(&format!("actions/runs/{run}"))?;
        if latest["id"].as_u64() != Some(run) || latest["run_attempt"].as_u64() != Some(attempt)
            || latest["head_sha"].as_str() != Some(landing) || latest["status"] != observed["status"]
        { return Err(pending("a different or unknown run attempt is current")); }
        let mut revisions = Vec::new();
        for (index, expected) in scope.workflows.iter().enumerate() {
            let file = self.provider.publication(&format!("contents/{}?ref={landing}", expected.path))?;
            if file["type"] != "file" || file["sha"].as_str() != Some(expected.revision.as_str()) {
                return Err(pending("workflow revision changed at landing"));
            }
            if index != 0 && observed["status"] == "completed" {
                let prefix = format!("{}/{}@", self.slug, expected.path);
                if observed["referenced_workflows"].as_array().is_none_or(|references| !references.iter().any(|reference| {
                    reference["path"].as_str().is_some_and(|path| path.starts_with(&prefix))
                        && reference["sha"].as_str() == Some(landing)
                })) { return Err(pending("downstream reusable workflow association unavailable")); }
            }
            revisions.push(serde_json::json!({"path": expected.path, "blob": expected.revision}));
        }
        let mut jobs = Vec::new();
        let mut total = None;
        for page in 1..=100 {
            let batch = self.provider.publication(&format!("actions/runs/{run}/attempts/{attempt}/jobs?per_page=100&page={page}"))?;
            let count = batch["total_count"].as_u64().ok_or_else(|| pending("job inventory unknown"))?;
            if total.is_some_and(|prior| prior != count) { return Err(pending("job inventory moved")); }
            total = Some(count);
            let rows = batch["jobs"].as_array().ok_or_else(|| pending("job inventory incomplete"))?;
            if rows.iter().any(|job| job["run_id"].as_u64() != Some(run) || job["run_attempt"].as_u64() != Some(attempt)
                || job["head_sha"].as_str() != Some(landing) || job["id"].as_u64().is_none()
                || job["name"].as_str().is_none()) { return Err(pending("job/run association disagrees")); }
            jobs.extend(rows.iter().cloned());
            if jobs.len() as u64 == count { break; }
            if rows.is_empty() || jobs.len() as u64 > count || page == 100 { return Err(pending("job pagination incomplete")); }
        }
        let mut ids = std::collections::BTreeSet::new();
        if jobs.is_empty() || jobs.iter().any(|job| !ids.insert(job["id"].as_u64().unwrap()))
            || scope.workflows.iter().take(if observed["status"] == "completed" { scope.workflows.len() } else { 1 }).flat_map(|workflow| &workflow.jobs).any(|expected| {
                !jobs.iter().any(|job| job["name"].as_str() == Some(expected.as_str()))
            }) { return Err(pending("expected publication/proof jobs are missing or ambiguous")); }
        let terminal = if observed["status"] == "completed" && latest["status"] == "completed" {
            if observed["conclusion"].as_str().is_none() || observed["conclusion"].is_null()
                || jobs.iter().any(|job| job["status"] != "completed" || job["conclusion"].as_str().is_none()
                    || job["completed_at"].as_str().is_none())
            { return Err(pending("whole workflow/job outcomes are incomplete")); }
            let publication = self.provider.release(&scope.tag)?;
            let repository = &publication["data"]["repository"];
            if !same_slug(&repository["nameWithOwner"], &self.slug)
                || repository.get("release").is_none() || repository.get("ref").is_none()
            { return Err(pending("release/tag observation incomplete")); }
            let tag = &repository["ref"];
            let release = &repository["release"];
            // A cancelled/failed workflow cannot prove an admitted API child
            // stopped before sending. Its uncertain CreateRelease must have
            // an actual reconciled release, even when the run is terminal.
            if operation.started.contains(&ExternalAction::CreateRelease) && release.is_null() {
                return Err(pending("the admitted Release API outcome remains unknown"));
            }
            if (!tag.is_null() && tag.pointer("/target/target/oid").and_then(Value::as_str) != Some(landing))
                || (!tag.is_null() && operation.tag_object.is_none())
                || (!release.is_null() && (release["tagName"] != scope.tag || release["isDraft"] != false
                    || release.pointer("/tagCommit/oid").and_then(Value::as_str) != Some(landing)
                    || release["id"].as_str().is_none()))
                || (observed["conclusion"] == "success" && (tag.is_null() || release.is_null()))
                || operation.tag_object.as_ref().is_some_and(|object| tag.pointer("/target/oid").and_then(Value::as_str) != Some(object.as_str()))
            { return Err(pending("actual release/tag outcome disagrees with the reserved publication")); }
            Some(serde_json::json!({"run": observed, "jobs": jobs, "publication": publication}))
        } else {
            if observed["status"].as_str().is_none_or(|status| !ACTIVE.contains(&status)) {
                return Err(pending("workflow state unknown"));
            }
            if !workflow.jobs.iter().any(|expected| jobs.iter().any(|job|
                job["name"].as_str() == Some(expected.as_str()) && job["status"] == "in_progress")) {
                return Err(pending("the reserved publisher job is not running"));
            }
            None
        };
        Ok(VerifiedRun { operation: operation.clone(), claim: RunClaim {
            workflow: workflow.path.clone(), run, attempt, landing: landing.into(),
            evidence: serde_json::json!({"pr": pr, "run": observed, "workflows": revisions, "jobs": jobs}), terminal,
        } })
    }
}
