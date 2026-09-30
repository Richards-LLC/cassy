//! Advisory review rounds are deliberately outside the gate-consumed
//! VerificationStore. An immutable dispatch supplies proof, not verdict authority.
mod git;
mod persistence;
#[cfg(test)]
mod tests;

use super::super::imports::*;
use crate::mcp::tools::core::task::lifecycle::repository_proof::{
    capture_repository_proof_with_anchors, verify_repository_proof,
};
use cas_types::{
    Agent, AgentRole, AgentType, RepositoryProofBoundary, Verification, VerificationStatus,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

type ReviewResult<T> = Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Axis {
    Spec,
    Standards,
}
impl Axis {
    fn name(self) -> &'static str {
        match self {
            Self::Spec => "spec",
            Self::Standards => "standards",
        }
    }
    fn other(self) -> Self {
        match self {
            Self::Spec => Self::Standards,
            Self::Standards => Self::Spec,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Start {
        task_id: String,
        dispatch_id: String,
        base_ref: String,
        spec_agent_id: String,
        standards_agent_id: String,
    },
    Context {
        round_id: String,
        #[serde(default)]
        cross_check: bool,
    },
    Report {
        round_id: String,
        report: AxisReport,
    },
    CrossCheck {
        round_id: String,
        commit: String,
        decision: Decision,
        reason: String,
    },
    Show {
        round_id: String,
    },
    Apply {
        round_id: String,
        opt_in: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CriterionVerdict {
    /// Exact criterion text, including a list marker when present.
    criterion: String,
    status: VerificationStatus,
    evidence: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Finding {
    id: String,
    rank: u32,
    source: String,
    evidence: String,
    uncertain: bool,
    #[serde(default)]
    judgement: bool,
    #[serde(default)]
    commit: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AxisReport {
    axis: Axis,
    status: VerificationStatus,
    summary: String,
    #[serde(default)]
    criteria: Vec<CriterionVerdict>,
    #[serde(default)]
    scope_creep: Vec<Finding>,
    #[serde(default)]
    findings: Vec<Finding>,
}
impl AxisReport {
    fn findings(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().chain(&self.scope_creep)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Decision {
    Accept,
    Revert,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CrossCheck {
    state: CrossCheckState,
    axis: Axis,
    agent_id: String,
    commit: String,
    decision: Decision,
    reason: String,
    revert_commit: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RuleSnapshot {
    id: String,
    paths: String,
    content: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct AxisRecord {
    axis: Axis,
    agent_id: String,
    side_ref: String,
    worktree: PathBuf,
    report: Option<AxisReport>,
    reported_tip: Option<String>,
    /// Changes only when the other authenticated reviewer requests a revert.
    checked_tip: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Application {
    repository: Option<RepositoryProofBoundary>,
    before: String,
    after: Option<String>,
    commits: Vec<String>,
    landed_commits: Vec<String>,
    supervisor_agent_id: String,
    /// Persist intent before touching delivery Git, allowing safe recovery.
    state: ApplicationState,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CrossCheckState {
    Intent,
    Complete,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ApplicationState {
    Intent,
    Applied,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Round {
    id: String,
    task_id: String,
    dispatch_id: String,
    supervisor_agent_id: String,
    proof: RepositoryProofBoundary,
    delivery_ref: String,
    base_commit: String,
    task_description: String,
    criteria: Vec<String>,
    coding_standards: Option<String>,
    rules: Vec<RuleSnapshot>,
    spec: AxisRecord,
    standards: AxisRecord,
    cross_checks: Vec<CrossCheck>,
    application: Option<Application>,
}
impl Round {
    fn axis(&self, axis: Axis) -> &AxisRecord {
        match axis {
            Axis::Spec => &self.spec,
            Axis::Standards => &self.standards,
        }
    }
    fn axis_mut(&mut self, axis: Axis) -> &mut AxisRecord {
        match axis {
            Axis::Spec => &mut self.spec,
            Axis::Standards => &mut self.standards,
        }
    }
    fn caller_axis(&self, caller: &Agent) -> ReviewResult<Axis> {
        require_child(caller, &self.supervisor_agent_id)?;
        if caller.id == self.spec.agent_id {
            Ok(Axis::Spec)
        } else if caller.id == self.standards.agent_id {
            Ok(Axis::Standards)
        } else {
            Err("shadow round is sealed to its two registered children".into())
        }
    }
    fn require_supervisor(&self, caller: &Agent, internal: bool) -> ReviewResult<()> {
        require_supervisor(caller, internal)?;
        if caller.id != self.supervisor_agent_id {
            return Err("shadow round belongs to another supervisor".into());
        }
        Ok(())
    }
}

fn require_supervisor(caller: &Agent, internal: bool) -> ReviewResult<()> {
    if caller.role != AgentRole::Supervisor || !internal || !caller.is_alive() {
        return Err("shadow start/show/apply requires an active registered supervisor with server-internal identity".into());
    }
    Ok(())
}
fn require_child(child: &Agent, parent: &str) -> ReviewResult<()> {
    if !child.is_alive()
        || child.role != AgentRole::Standard
        || child.agent_type != AgentType::SubAgent
        || child.parent_id.as_deref() != Some(parent)
        || child.id == parent
    {
        return Err("shadow axes require distinct active registered Standard SubAgent children of the supervisor".into());
    }
    Ok(())
}
fn safe_id(id: &str) -> ReviewResult<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("invalid shadow task/round/finding identifier".into());
    }
    Ok(())
}
fn sanitize_text(text: String) -> String {
    let mut payload = Verification::new(String::new(), String::new());
    payload.summary = text;
    payload.sanitize_verifier_authored_content();
    payload.summary
}
fn bounded_text(text: &str) -> ReviewResult<()> {
    if text.trim().is_empty() || text.len() > 16 * 1024 {
        return Err("review evidence/reason must be nonempty and at most 16 KiB".into());
    }
    Ok(())
}
fn require_proof(round: &Round) -> ReviewResult<()> {
    verify_repository_proof(&round.proof)
        .map_err(|e| format!("shadow dispatch proof changed; request a fresh round: {e}"))
}

impl CasCore {
    pub(crate) async fn shadow_review(
        &self,
        payload: Option<&str>,
    ) -> Result<CallToolResult, McpError> {
        let result = self.shadow_review_inner(payload);
        result
            .map(|value| CallToolResult::success(vec![Content::text(value.to_string())]))
            .map_err(|message| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::Owned(message),
                data: None,
            })
    }

    pub(crate) fn shadow_review_inner(
        &self,
        payload: Option<&str>,
    ) -> ReviewResult<serde_json::Value> {
        let payload = payload
            .filter(|s| s.len() <= 512 * 1024)
            .ok_or("shadow requires review JSON of at most 512 KiB")?;
        // Unknown fields and malformed reports fail before locks or mutations.
        let request: Request = serde_json::from_str(payload).map_err(|_| {
            "invalid typed shadow review operation; see cas-shadow-review".to_string()
        })?;
        let caller_id = self.get_agent_id().map_err(|e| e.message.to_string())?;
        let agents = self.open_agent_store().map_err(|e| e.message.to_string())?;
        let caller = agents.get(&caller_id).map_err(|e| e.to_string())?;
        if !caller.is_alive() {
            return Err("shadow caller is not an active registered session".into());
        }
        let internal = self.has_server_internal_identity(&caller_id);
        let store = persistence::Store::lock(&self.cas_root)?;
        if let Request::Start {
            task_id,
            dispatch_id,
            base_ref,
            spec_agent_id,
            standards_agent_id,
        } = request
        {
            require_supervisor(&caller, internal)?;
            safe_id(&task_id)?;
            safe_id(&dispatch_id)?;
            if spec_agent_id == standards_agent_id {
                return Err("Spec and Standards need separate registered contexts".into());
            }
            for id in [&spec_agent_id, &standards_agent_id] {
                if id == &caller_id {
                    return Err("a reviewer cannot be its supervisor".into());
                }
                let child = agents.get(id).map_err(|e| e.to_string())?;
                require_child(&child, &caller_id)?;
            }
            let dispatch = cas_store::get_verification_dispatch(&self.cas_root, &dispatch_id)
                .map_err(|e| e.to_string())?;
            if dispatch.task_id != task_id {
                return Err("shadow task does not match its exact dispatch".into());
            }
            if [&spec_agent_id, &standards_agent_id].contains(&&dispatch.owner_agent_id)
                || [&spec_agent_id, &standards_agent_id].contains(&&dispatch.requester_agent_id)
                || dispatch
                    .verifier_agent_id
                    .as_ref()
                    .is_some_and(|id| [&spec_agent_id, &standards_agent_id].contains(&id))
            {
                return Err(
                    "shadow reviewers must be distinct from the implementer and legacy verifier"
                        .into(),
                );
            }
            let proof = dispatch
                .repository
                .ok_or("shadow review needs the legacy dispatch's exact repository proof")?;
            let root = Path::new(&proof.worktree_root);
            verify_repository_proof(&proof).map_err(|e| e.to_string())?;
            git::clean(root)?;
            if git::head(root)? != proof.head_commit {
                return Err(
                    "shadow review starts only at the exact delivered dispatch HEAD".into(),
                );
            }
            let base_commit = git::resolve(root, &base_ref)?;
            let delivery_ref = git::branch(root)?;
            git::ancestor(root, &base_commit, &proof.head_commit)?;
            if base_commit == proof.head_commit {
                return Err("shadow review requires a nonempty fixed delivery diff".into());
            }
            let id = format!("shadow-{dispatch_id}");
            if store.exists(&id) {
                let round = store.load(&id)?;
                round.require_supervisor(&caller, internal)?;
                if round.spec.agent_id == spec_agent_id
                    && round.standards.agent_id == standards_agent_id
                    && round.base_commit == base_commit
                {
                    return comparison(&self.cas_root, &round);
                }
                return Err(
                    "dispatch already has a sealed shadow round with different participants/base"
                        .into(),
                );
            }
            let task = self
                .open_task_store()
                .map_err(|e| e.message.to_string())?
                .get(&task_id)
                .map_err(|e| e.to_string())?;
            let criteria: Vec<String> = task
                .acceptance_criteria
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if criteria.is_empty() {
                return Err("shadow Spec review requires stored acceptance criteria".into());
            }
            let coding_standards = git::file_at(root, &proof.head_commit, "CODING_STANDARDS.md")?;
            let rules = self
                .open_rule_store()
                .map_err(|e| e.message.to_string())?
                .list_proven()
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|r| RuleSnapshot {
                    id: r.id,
                    paths: r.paths,
                    content: r.content,
                })
                .collect();
            let spec =
                git::create_axis(&self.cas_root, &task_id, Axis::Spec, spec_agent_id, &proof)?;
            let standards = match git::create_axis(
                &self.cas_root,
                &task_id,
                Axis::Standards,
                standards_agent_id,
                &proof,
            ) {
                Ok(axis) => axis,
                Err(e) => {
                    git::remove_axis(&proof, &spec);
                    return Err(e);
                }
            };
            let round = Round {
                id,
                task_id,
                dispatch_id,
                supervisor_agent_id: caller_id,
                proof,
                delivery_ref,
                base_commit,
                task_description: task.description,
                criteria,
                coding_standards,
                rules,
                spec,
                standards,
                cross_checks: vec![],
                application: None,
            };
            if let Err(e) = store.save(&round) {
                git::remove_axis(&round.proof, &round.spec);
                git::remove_axis(&round.proof, &round.standards);
                return Err(e);
            }
            return comparison(&self.cas_root, &round);
        }
        let id = match &request {
            Request::Context { round_id, .. }
            | Request::Report { round_id, .. }
            | Request::CrossCheck { round_id, .. }
            | Request::Show { round_id }
            | Request::Apply { round_id, .. } => round_id,
            Request::Start { .. } => unreachable!(),
        };
        let mut round = store.load(id)?;
        match request {
            Request::Context { cross_check, .. } => {
                let axis = round.caller_axis(&caller)?;
                require_proof(&round)?;
                if cross_check {
                    if round.spec.report.is_none() || round.standards.report.is_none() {
                        return Err("cross-check context opens only after both independent reports are sealed".into());
                    }
                    let other = round.axis(axis.other());
                    Ok(
                        serde_json::json!({"round_id":round.id, "axis":axis, "review_other":other, "proof":round.proof}),
                    )
                } else {
                    let own = round.axis(axis);
                    let sources = match axis {
                        Axis::Spec => {
                            serde_json::json!({"task_description":round.task_description,"criteria":round.criteria})
                        }
                        Axis::Standards => {
                            serde_json::json!({"coding_standards":round.coding_standards,"promoted_rules":round.rules,"baseline":"Read the smell baseline in CODING_STANDARDS.md when present. Those findings are judgement calls; repository standards and promoted rules govern this review."})
                        }
                    };
                    Ok(
                        serde_json::json!({"round_id":round.id,"axis":axis,"side_ref":own.side_ref,"worktree":own.worktree,"base_commit":round.base_commit,"head_commit":round.proof.head_commit,"sources":sources}),
                    )
                }
            }
            Request::Report { mut report, .. } => {
                let axis = round.caller_axis(&caller)?;
                require_proof(&round)?;
                report.summary = sanitize_text(report.summary);
                for criterion in &mut report.criteria {
                    criterion.evidence = sanitize_text(std::mem::take(&mut criterion.evidence));
                }
                for finding in report.findings.iter_mut().chain(&mut report.scope_creep) {
                    finding.evidence = sanitize_text(std::mem::take(&mut finding.evidence));
                }
                if let Some(previous) = &round.axis(axis).report {
                    if previous != &report {
                        return Err(
                            "axis report is immutable; start a fresh dispatch to revise it".into(),
                        );
                    }
                    return serde_json::to_value(round.axis(axis)).map_err(|e| e.to_string());
                }
                validate_report(&round, axis, &report)?;
                let record = round.axis_mut(axis);
                let tip = git::head(&record.worktree)?;
                record.report = Some(report);
                record.reported_tip = Some(tip.clone());
                record.checked_tip = Some(tip);
                store.save(&round)?;
                // A child receives its own report; no other axis's ranking leaks.
                Ok(serde_json::to_value(round.axis(axis)).map_err(|e| e.to_string())?)
            }
            Request::CrossCheck {
                commit,
                decision,
                reason,
                ..
            } => {
                let axis = round.caller_axis(&caller)?;
                require_proof(&round)?;
                cross_check(
                    &store,
                    &mut round,
                    axis,
                    &caller.id,
                    &commit,
                    decision,
                    sanitize_text(reason),
                )?;
                store.save(&round)?;
                Ok(serde_json::to_value(&round.cross_checks).map_err(|e| e.to_string())?)
            }
            Request::Show { .. } => {
                round.require_supervisor(&caller, internal)?;
                comparison(&self.cas_root, &round)
            }
            Request::Apply { opt_in, .. } => {
                round.require_supervisor(&caller, internal)?;
                if !opt_in {
                    return Err(
                        "shadow fixes stay on side refs; apply requires explicit opt_in=true"
                            .into(),
                    );
                }
                apply(&store, &mut round, &caller.id)?;
                comparison(&self.cas_root, &round)
            }
            Request::Start { .. } => unreachable!(),
        }
    }
}

fn validate_report(round: &Round, axis: Axis, report: &AxisReport) -> ReviewResult<()> {
    if report.axis != axis {
        return Err("report axis differs from sealed caller axis".into());
    }
    bounded_text(&report.summary)?;
    match axis {
        Axis::Spec => {
            let actual: Vec<&str> = report
                .criteria
                .iter()
                .map(|c| c.criterion.as_str())
                .collect();
            let expected: Vec<&str> = round.criteria.iter().map(String::as_str).collect();
            if actual != expected {
                return Err(
                    "Spec must quote every stored criterion exactly once in source order".into(),
                );
            }
            if report.status == VerificationStatus::Approved
                && report
                    .criteria
                    .iter()
                    .any(|c| c.status != VerificationStatus::Approved)
            {
                return Err(
                    "an approved Spec report requires all criterion verdicts approved".into(),
                );
            }
            for criterion in &report.criteria {
                bounded_text(&criterion.evidence)?;
            }
        }
        Axis::Standards => {
            if !report.criteria.is_empty() || !report.scope_creep.is_empty() {
                return Err("Standards reports cannot own Spec criterion/scope verdicts".into());
            }
        }
    }
    let mut ids = std::collections::HashSet::new();
    for finding in report.findings() {
        safe_id(&finding.id)?;
        if !ids.insert(&finding.id) {
            return Err("finding identifiers must be unique per axis".into());
        }
        bounded_text(&finding.source)?;
        bounded_text(&finding.evidence)?;
        if finding.uncertain && finding.commit.is_some() {
            return Err("uncertain findings are reported without fix commits".into());
        }
        if axis == Axis::Spec && !round.criteria.iter().any(|c| c == &finding.source) {
            return Err(
                "Spec findings and scope-creep checks must quote their source criterion".into(),
            );
        }
        if axis == Axis::Standards {
            if finding.source == "CODING_STANDARDS.md" {
                if round.coding_standards.is_none() {
                    return Err("this delivery has no CODING_STANDARDS.md source".into());
                }
            } else if !round.rules.iter().any(|r| r.id == finding.source) {
                return Err("Standards finding must cite CODING_STANDARDS.md, or a snapshotted promoted rule id".into());
            }
        }
    }
    git::validate_fixes(round, axis, report)
}

fn cross_check(
    store: &persistence::Store,
    round: &mut Round,
    axis: Axis,
    caller_id: &str,
    commit: &str,
    decision: Decision,
    reason: String,
) -> ReviewResult<()> {
    bounded_text(&reason)?;
    if round.spec.report.is_none() || round.standards.report.is_none() {
        return Err("both reports must be sealed before cross-checking".into());
    }
    let other = round.axis(axis.other());
    let finding = other
        .report
        .as_ref()
        .unwrap()
        .findings()
        .find(|f| f.commit.as_deref() == Some(commit))
        .ok_or("cross-check must name a fix from the other axis's sealed report")?;
    if let Some(old) = round.cross_checks.iter().find(|c| c.commit == commit) {
        if old.state != CrossCheckState::Complete {
            return Err("cross-check revert has an unresolved intent; inspect the side-ref Git operation before recovery".into());
        }
        if old.axis == axis && old.decision == decision && old.reason == reason {
            return Ok(());
        }
        return Err("fix already has an immutable cross-check receipt".into());
    }
    git::unchanged_axis(other)?;
    let other = other.clone();
    let finding_id = finding.id.clone();
    round.cross_checks.push(CrossCheck {
        state: CrossCheckState::Intent,
        axis,
        agent_id: caller_id.to_string(),
        commit: commit.to_string(),
        decision,
        reason: reason.clone(),
        revert_commit: None,
    });
    store.save(round)?;
    let revert_commit = if decision == Decision::Revert {
        Some(git::revert(
            &other,
            axis,
            &finding_id,
            commit,
            &reason,
            caller_id,
        )?)
    } else {
        None
    };
    if let Some(tip) = &revert_commit {
        round.axis_mut(axis.other()).checked_tip = Some(tip.clone());
    }
    let receipt = round.cross_checks.last_mut().unwrap();
    receipt.revert_commit = revert_commit;
    receipt.state = CrossCheckState::Complete;
    Ok(())
}

fn apply(store: &persistence::Store, round: &mut Round, caller_id: &str) -> ReviewResult<()> {
    if let Some(application) = &round.application {
        if application.state == ApplicationState::Applied {
            if let Some(proof) = &application.repository {
                verify_repository_proof(proof).map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        return Err("shadow application has an unresolved intent; inspect delivery Git and recover before retrying".into());
    }
    require_proof(round)?;
    let root = Path::new(&round.proof.worktree_root);
    git::clean(root)?;
    if git::head(root)? != round.proof.head_commit {
        return Err("opt-in requires the original clean delivery HEAD; regenerate the round after delivery changes".into());
    }
    if git::branch(root)? != round.delivery_ref {
        return Err("delivery worktree switched branches after dispatch; restore its sealed branch before applying".into());
    }
    let mut commits = vec![];
    for axis in [Axis::Spec, Axis::Standards] {
        let record = round.axis(axis);
        let report = record
            .report
            .as_ref()
            .ok_or("both reports must be sealed before applying")?;
        git::unchanged_axis(record)?;
        for commit in git::commits(
            &record.worktree,
            &round.proof.head_commit,
            record.reported_tip.as_deref().unwrap(),
        )? {
            if !report
                .findings()
                .any(|f| f.commit.as_ref() == Some(&commit))
            {
                return Err("side ref contains an unreported fix".into());
            }
            let receipt = round
                .cross_checks
                .iter()
                .find(|c| c.commit == commit)
                .ok_or("every fix needs the other axis's cross-check receipt before opt-in")?;
            if receipt.state != CrossCheckState::Complete {
                return Err("cross-check intent is unresolved; no fixes can be applied".into());
            }
            if receipt.decision == Decision::Accept {
                commits.push(commit);
            }
        }
    }
    round.application = Some(Application {
        repository: None,
        before: round.proof.head_commit.clone(),
        after: None,
        commits: commits.clone(),
        landed_commits: Vec::new(),
        supervisor_agent_id: caller_id.to_string(),
        state: ApplicationState::Intent,
    });
    store.save(round)?;
    match git::apply(root, &commits) {
        Ok(after) => {
            // The existing proof model keeps patch-id survival on the applied
            // anchors. A new legacy dispatch is still required for merge gates.
            let landed = git::commits(root, &round.proof.head_commit, &after)?;
            if landed.len() != commits.len() {
                return Err(
                    "application intent retained: landed commit count differs from accepted fixes"
                        .into(),
                );
            }
            let proof = capture_repository_proof_with_anchors(
                Path::new(&round.proof.repository_root),
                root,
                landed.clone(),
            )?;
            let application = round.application.as_mut().unwrap();
            application.repository = Some(proof);
            application.landed_commits = landed;
            application.after = Some(after);
            application.state = ApplicationState::Applied;
            store.save(round)
        }
        Err(e) => {
            // git::apply aborts conflicts, retaining the original clean tip.
            round.application.as_mut().unwrap().state = ApplicationState::Failed;
            store.save(round)?;
            Err(e)
        }
    }
}

fn comparison(cas_root: &Path, round: &Round) -> ReviewResult<serde_json::Value> {
    let legacy: Option<Verification> =
        cas_store::get_verification_for_dispatch(cas_root, &round.dispatch_id)
            .map_err(|e| e.to_string())?;
    Ok(
        serde_json::json!({"mode":"shadow", "merge_gate_authority":false, "legacy_verdict":legacy, "round":round,
        "next": if round.application.as_ref().is_some_and(|a| a.state == ApplicationState::Applied) { "Request a fresh legacy verification dispatch for the changed delivery." } else { "Complete both independent reports and cross-check every fix; apply only with explicit supervisor opt-in." }}),
    )
}
