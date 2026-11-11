//! Friction-ledger emitters: one small call from each code path that already
//! observed a check, an intervention, an escape or a feature verdict.
//!
//! Emission fails open. A ledger write must never fail or delay the action it
//! measures, so every error stops at a warning.

pub(crate) mod estimates;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use cas_store::{
    CheckOutcome, CheckStage, FeedbackSource, FeedbackVerdict, FrictionSource, InterventionKind,
    NewQualityEvent, QualityPayload,
};
use chrono::{DateTime, Duration, Utc};

/// Check id of the `worktree_merge` admission gate; its last row for a task tells
/// a tool merge from a hand merge.
pub(crate) const MERGE_GATE_CHECK: &str = "worktree_merge";
/// Check id of a delivery PR's merge-queue result.
pub(crate) const MERGE_QUEUE_CHECK: &str = "merge_queue";

#[derive(serde::Deserialize)]
pub(crate) struct QueueEjection {
    pub task_id: String,
    pub pr_number: u64,
    pub failed_run_id: Option<u64>,
    pub occurrence: String,
    pub failing_job: Option<String>,
}

/// Live relay and historical recovery share one identity, including the old
/// queue-exit/merge-group-run aliases for the same failed run.
pub(crate) fn queue_ejected(root: &Path, ejection: &QueueEjection, at: DateTime<Utc>) -> bool {
    queue_ejected_with_aliases(root, ejection, at, &[queue_ejection_key(ejection, &ejection.occurrence)])
}

fn queue_ejection_key(ejection: &QueueEjection, occurrence: &str) -> String {
    format!("merge_queue|{}|{}|{occurrence}", ejection.task_id, ejection.pr_number)
}

fn queue_ejected_with_aliases(root: &Path, ejection: &QueueEjection, at: DateTime<Utc>, aliases: &[String]) -> bool {
    let occurrence = ejection.failed_run_id.map(|run| format!("merge-group-run:{run}"))
        .unwrap_or_else(|| ejection.occurrence.clone());
    let mut event = NewQualityEvent::new(
        queue_ejection_key(ejection, &occurrence),
        QualityPayload::CheckRun { review: None, check_id: MERGE_QUEUE_CHECK.into(), stage: CheckStage::Queue,
            outcome: CheckOutcome::Fail, sha: None, minutes: None },
    );
    event.occurred_at = at;
    event.task_id = Some(ejection.task_id.clone());
    event.title = ejection.failing_job.clone();
    event.link = ejection.failed_run_id.and_then(|run| crate::github_issue_attach::default_repo_for(root)
        .map(|repo| format!("https://github.com/{repo}/actions/runs/{run}")));
    event.scope = project_scope(root);
    match cas_store::quality_event_recorded(root, &event, aliases) {
        Ok(true) => return false,
        Ok(false) => {},
        Err(error) => {
            tracing::warn!(%error, "cannot resolve historical queue-ejection identity");
            return false;
        }
    }
    emit(root, &event)
}
/// A spawn this soon after a force shutdown is the replacement for that worker.
const RESPAWN_WINDOW_HOURS: i64 = 2;

/// The project every event id is scoped to, resolved once per store. An
/// unresolvable identity only loses the cross-project salt on that row's id.
fn project_scope(cas_root: &Path) -> String {
    static SCOPES: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    let mut scopes = SCOPES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    scopes
        .entry(cas_root.to_path_buf())
        .or_insert_with(|| crate::cloud::resolve_canonical_id_for_sync(cas_root).unwrap_or_default())
        .clone()
}

/// The turn scanner keeps its cursor behind failed writes and retries later.
pub(crate) fn emit_checked(cas_root: &Path, event: &NewQualityEvent) -> cas_store::Result<bool> {
    let mut event = event.clone();
    event.scope = project_scope(cas_root);
    cas_store::record_quality_event(cas_root, &event)
}

/// Append notice detail for an old turn without counting it as another wake.
pub(crate) fn emit_turn_checked(root: &Path, original: &NewQualityEvent) -> cas_store::Result<(bool, bool)> {
    let mut event = original.clone();
    event.scope = project_scope(root);
    if cas_store::record_quality_event(root, &event)? { return Ok((true, true)) }
    if matches!(&event.payload, QualityPayload::Turn { notice_kind: Some(_), .. })
        && let Some(old) = cas_store::quality_event_by_id(root, &event.id())?
        && matches!(old.payload, QualityPayload::Turn { notice_kind: None, .. }) {
        if let QualityPayload::Turn { supersedes, .. } = &mut event.payload { *supersedes = Some(old.id); }
        event.natural_key = format!("turn_detail|{}", event.natural_key);
        return Ok((cas_store::record_quality_event(root, &event)?, false));
    }
    Ok((false, false))
}

pub(crate) fn emit(cas_root: &Path, event: &NewQualityEvent) -> bool {
    match emit_checked(cas_root, event) {
        Ok(inserted) => inserted,
        Err(error) => {
            tracing::warn!(kind = event.payload.kind(), %error, "friction ledger write failed");
            false
        }
    }
}

fn task_title(cas_root: &Path, task_id: &str) -> Option<String> {
    crate::store::open_task_store(cas_root)
        .ok()?
        .get(task_id)
        .ok()
        .map(|task| task.title)
}

pub(crate) struct Check<'a> {
    pub natural_key: String,
    pub check_id: &'a str,
    pub stage: CheckStage,
    pub outcome: CheckOutcome,
    pub task_id: Option<&'a str>,
    pub sha: Option<&'a str>,
    pub minutes: Option<f64>,
    pub link: Option<&'a str>,
    pub actor: Option<&'a str>,
}

pub(crate) fn check_run(cas_root: &Path, check: Check<'_>) {
    let mut event = NewQualityEvent::new(
        check.natural_key,
        QualityPayload::CheckRun { review: None,
            check_id: check.check_id.to_string(),
            stage: check.stage,
            outcome: check.outcome,
            // A ref name or placeholder is not a commit, and must not lose the row.
            sha: check
                .sha
                .filter(|sha| sha.len() >= 7 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .map(str::to_string),
            minutes: check.minutes,
        },
    );
    event.task_id = check.task_id.map(str::to_string);
    event.title = check.task_id.and_then(|id| task_title(cas_root, id));
    event.link = check.link.map(str::to_string);
    event.actor = check.actor.map(str::to_string);
    emit(cas_root, &event);
    if check.outcome == CheckOutcome::Pass && let Some(sha) = check.sha
        && estimates::PIPELINES.contains(&check.check_id) {
        estimates::complete(cas_root, "pipeline", &format!("{}:{sha}", check.check_id), Utc::now(), None, None);
    }
}

/// The same emitter serves live verdicts and historical recovery. An immutable
/// older row gets a companion that supersedes its metrics, never a second check.
pub(crate) fn verification_recorded(root: &Path, verification: &cas_types::Verification) -> bool {
    let Some(outcome) = verifier_outcome(verification.status) else { return false };
    let cycle_id = review_cycle(root, verification);
    let provenance = match verification.provenance {
        cas_types::VerificationProvenance::TaskVerifier | cas_types::VerificationProvenance::PeerAttested => "independent",
        cas_types::VerificationProvenance::SupervisorDirect => "supervisor_direct",
        cas_types::VerificationProvenance::System => "system",
        cas_types::VerificationProvenance::Legacy => "legacy",
    }.to_string();
    let mut event = NewQualityEvent::new(format!("verifier|{}", verification.id),
        QualityPayload::CheckRun {
            check_id: "task_verifier".into(), stage: CheckStage::Verifier, outcome,
            sha: None, minutes: verification.duration_ms.map(|ms| ms as f64 / 60_000.0),
            review: Some(cas_store::ReviewEvidence {
                verification_id: verification.id.clone(), cycle_id, provenance, supersedes: None,
            }),
        });
    event.scope = project_scope(root);
    event.occurred_at = verification.created_at;
    event.task_id = Some(verification.task_id.clone());
    event.title = task_title(root, &verification.task_id);
    event.actor = verification.agent_id.clone();
    let existing = cas_store::quality_events_for_task(root, &verification.task_id).unwrap_or_default();
    if existing.iter().any(|old| matches!(&old.payload,
        QualityPayload::CheckRun { review: Some(review), .. } if review.verification_id == verification.id)) {
        return false;
    }
    if let Some(old) = existing.iter().find(|old| old.id == event.id()) {
        if let QualityPayload::CheckRun { review: Some(review), .. } = &mut event.payload {
            review.supersedes = Some(old.id.clone());
        }
        event.natural_key = format!("verifier_review|{}", verification.id);
    }
    emit(root, &event)
}

fn review_cycle(root: &Path, verification: &cas_types::Verification) -> Option<String> {
    let cycles = cas_store::delivery_cycles(root, &verification.task_id).ok()?;
    let dispatch = verification.dispatch_id.as_deref()
        .and_then(|id| cas_store::get_verification_dispatch(root, id).ok());
    let mut anchors = Vec::new();
    if let Some(dispatch) = &dispatch {
        if let Some(repo) = &dispatch.repository {
            anchors.extend(repo.anchor_commits.iter().cloned());
            anchors.push(repo.head_commit.clone());
        }
        if let Some(receipt) = dispatch.receipt_id.as_deref()
            .and_then(|id| cas_store::get_worker_delivery_by_receipt(root, id).ok().flatten()) {
            anchors.push(receipt.0.commit_sha);
        }
    }
    let candidates: Vec<_> = cycles.iter().filter(|cycle|
        cycle.anchors.iter().any(|anchor| anchors.contains(anchor))).collect();
    match candidates.as_slice() {
        [cycle] => Some(cycle.cycle_id.clone()),
        [] if cycles.len() == 1 => Some(cycles[0].cycle_id.clone()),
        _ => None,
    }
}

/// Explicitly attested external/manual work, retained in ordinary task notes.
/// Its date and release attribution are evidence, not the backfill's run time.
#[derive(serde::Deserialize)]
struct AttestedIntervention {
    occurred_at: DateTime<Utc>, intervention: InterventionKind, key: String,
    actor: String, release: Option<String>, task_id: Option<String>, source: String,
}

fn attested_interventions(root: &Path, notes: &str, since: DateTime<Utc>) -> anyhow::Result<usize> {
    let mut inserted = 0;
    for line in notes.lines() {
        let Some((_, json)) = line.split_once("LEDGER INTERVENTIONS: ") else { continue };
        let rows: Vec<AttestedIntervention> = serde_json::from_str(json)?;
        for row in rows.into_iter().filter(|row| row.occurred_at >= since) {
            anyhow::ensure!(!row.source.trim().is_empty() && !row.key.trim().is_empty(), "attestation needs a source and key");
            let mut event = NewQualityEvent::new(format!("attested|{}|{}", row.intervention.as_str(), row.key),
                QualityPayload::Intervention { intervention: row.intervention, minutes: None });
            event.occurred_at = row.occurred_at;
            event.actor = Some(row.actor);
            event.task_id = row.task_id;
            event.release_version = row.release;
            event.local_note = Some(row.source);
            inserted += usize::from(emit(root, &event));
        }
    }
    Ok(inserted)
}

/// `reason` is free text: it stays on this machine as `local_note`.
pub(crate) fn intervention(
    cas_root: &Path,
    kind: InterventionKind,
    natural_key: String,
    task_id: Option<&str>,
    actor: Option<&str>,
    reason: Option<&str>,
) -> bool {
    let mut event = NewQualityEvent::new(
        natural_key,
        QualityPayload::Intervention {
            intervention: kind,
            minutes: None,
        },
    );
    event.task_id = task_id.map(str::to_string);
    event.title = task_id.and_then(|id| task_title(cas_root, id));
    event.actor = actor.map(str::to_string);
    event.local_note = reason.map(str::to_string);
    emit(cas_root, &event)
}

/// Every successful task close. An override is an intervention, and a close whose
/// last merge row is a refusal or an ejection landed some way other than the tools.
// gate-audit[supervisor_override]: selector Records telemetry about a close already accepted by its policy boundary.
pub(crate) fn task_closed(
    cas_root: &Path,
    task_id: &str,
    actor: Option<&str>,
    supervisor_override: bool,
    reason: Option<&str>,
) {
    if let Err(error) = estimates::task_landed(cas_root, task_id) {
        tracing::warn!(%error, "cannot record task estimate outcome");
    }
    let closed_at = Utc::now().timestamp();
    if supervisor_override {
        intervention(
            cas_root,
            InterventionKind::OverrideClose,
            format!("override_close|{task_id}|{closed_at}"),
            Some(task_id),
            actor,
            reason,
        );
    }
    let last_gate = cas_store::quality_events_for_task(cas_root, task_id)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .find_map(|event| match event.payload {
            QualityPayload::CheckRun { check_id, outcome, .. }
                if check_id == MERGE_GATE_CHECK || check_id == MERGE_QUEUE_CHECK =>
            {
                Some(outcome)
            }
            _ => None,
        });
    if last_gate == Some(CheckOutcome::Fail) {
        intervention(
            cas_root,
            InterventionKind::HandMerge,
            format!("hand_merge|{task_id}|{closed_at}"),
            Some(task_id),
            actor,
            reason,
        );
    }
}

/// An independent QA verdict, plus one check per PASS or FAIL ledger row.
pub(crate) fn qa_recorded(
    cas_root: &Path,
    pass: &cas_types::QaPass,
    verdict: cas_types::QaVerdict,
    ledger_text: &str,
    reviewer: &str,
) {
    check_run(cas_root, Check {
        natural_key: format!("qa|{}", pass.id),
        check_id: "qa_pass",
        stage: CheckStage::Qa,
        outcome: match verdict {
            cas_types::QaVerdict::Approved => CheckOutcome::Pass,
            cas_types::QaVerdict::Rejected => CheckOutcome::Fail,
        },
        task_id: Some(&pass.task_id),
        sha: Some(&pass.bound_head),
        minutes: None,
        link: None,
        actor: Some(reviewer),
    });
    for row in crate::qa_ledger::parse(ledger_text, 0).rows {
        let outcome = match row.verdict {
            crate::qa_ledger::Verdict::Pass => CheckOutcome::Pass,
            crate::qa_ledger::Verdict::Fail => CheckOutcome::Fail,
            _ => continue,
        };
        check_run(cas_root, Check {
            natural_key: format!("journey|{}|{}", pass.id, row.id),
            check_id: &format!("journey:{}", row.id),
            stage: CheckStage::Journey,
            outcome,
            task_id: Some(&pass.task_id),
            sha: Some(&pass.bound_head),
            minutes: None,
            link: None,
            actor: Some(reviewer),
        });
    }
}

/// A spawn pairs with the newest force shutdown it has not already replaced. The
/// natural key is that shutdown, so one shutdown counts at most one respawn.
pub(crate) fn worker_spawned(cas_root: &Path, task_id: Option<&str>, actor: Option<&str>) {
    let since = Utc::now() - Duration::hours(RESPAWN_WINDOW_HOURS);
    let shutdowns = cas_store::quality_interventions_since(
        cas_root,
        InterventionKind::ForceShutdown,
        since,
    )
    .unwrap_or_default();
    for shutdown in shutdowns {
        let task = task_id.or(shutdown.task_id.as_deref());
        if intervention(
            cas_root,
            InterventionKind::Respawn,
            format!("respawn|{}", shutdown.id),
            task,
            actor,
            None,
        ) {
            return;
        }
    }
}

/// `found-in:vX.Y.Z` on a bug is the tag that makes it an escape; optional
/// `missed-by:<stage>` names the stage that should have caught it.
pub(crate) fn bug_labelled(
    cas_root: &Path,
    task_id: &str,
    title: &str,
    task_type: cas_types::TaskType,
    labels: &[String],
    actor: Option<&str>,
) {
    if task_type != cas_types::TaskType::Bug {
        return;
    }
    let missed_stage = labels.iter().find_map(|label| {
        match label.strip_prefix("missed-by:")?.trim() {
            "lane" => Some(CheckStage::Lane),
            "queue" => Some(CheckStage::Queue),
            "verifier" => Some(CheckStage::Verifier),
            "qa" => Some(CheckStage::Qa),
            "journey" => Some(CheckStage::Journey),
            _ => None,
        }
    });
    for label in labels {
        let Some(version) = label
            .strip_prefix("found-in:")
            .and_then(cas_store::normalize_release_version)
        else {
            continue;
        };
        let mut event = NewQualityEvent::new(
            format!("escape|{task_id}|{version}"),
            QualityPayload::Escape {
                found_in: version.clone(),
                missed_stage,
            },
        );
        event.task_id = Some(task_id.to_string());
        event.release_version = Some(version);
        event.title = Some(title.to_string());
        event.actor = actor.map(str::to_string);
        emit(cas_root, &event);
    }
}

/// cas-87a2: the Cassy source checkout, where every bug task is a Cassy defect.
/// In any other project a bug task is that project's bug, not Cassy friction.
pub(crate) fn is_cassy_source(cas_root: &Path) -> bool {
    let root = cas_root.parent().unwrap_or(cas_root);
    std::fs::read_to_string(root.join("cas-cli/Cargo.toml"))
        .is_ok_and(|manifest| manifest.lines().any(|line| line.trim() == "name = \"cas\""))
}

/// The project a problem was found in, as a slug: its checkout directory name.
fn project_slug(cas_root: &Path) -> Option<String> {
    let name = cas_root.parent().unwrap_or(cas_root).file_name()?.to_string_lossy().to_string();
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .take(64)
        .collect();
    (!slug.is_empty()).then_some(slug)
}

/// cas-87a2: filing a bug task in the Cassy repository is the friction write.
/// Nobody has to remember the ledger; the create path records it.
pub(crate) fn bug_filed(
    cas_root: &Path,
    task_id: &str,
    title: &str,
    task_type: cas_types::TaskType,
    priority: i32,
    created_at: DateTime<Utc>,
) -> bool {
    if task_type != cas_types::TaskType::Bug || !is_cassy_source(cas_root) {
        return false;
    }
    let mut event = NewQualityEvent::new(
        format!("friction|task|{task_id}"),
        QualityPayload::Friction {
            source: FrictionSource::Task,
            reference: task_id.to_string(),
            severity: Some(format!("p{priority}")),
            project: project_slug(cas_root),
        },
    );
    event.occurred_at = created_at;
    event.task_id = Some(task_id.to_string());
    event.title = Some(title.to_string());
    emit(cas_root, &event)
}

/// cas-87a2: an issue filed on the Cassy tracker, from any project, is friction.
pub(crate) fn issue_filed(cas_root: &Path, url: &str, title: &str, at: DateTime<Utc>) -> bool {
    let Some(issue) = crate::github_issue_attach::parse_issue_refs(url, None).into_iter().next()
    else {
        return false;
    };
    let reference = format!("{}/{}/issues/{}", issue.owner, issue.repo, issue.number);
    let mut event = NewQualityEvent::new(
        format!("friction|issue|{}", reference.to_ascii_lowercase()),
        QualityPayload::Friction {
            source: FrictionSource::Issue,
            reference,
            severity: None,
            project: project_slug(cas_root),
        },
    );
    event.occurred_at = at;
    event.title = Some(title.to_string());
    event.link = Some(url.to_string());
    emit(cas_root, &event)
}

/// What a friction backfill found.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FrictionBackfill {
    pub tasks: usize,
    pub issues: usize,
    /// Why the Cassy tracker was not read, when it was not.
    pub issues_skipped: Option<String>,
}

/// cas-87a2: record problems filed before the emitters existed. Cassy-source bug
/// tasks come from the task store; Cassy tracker issues come from a bounded
/// `gh issue list`. A missing or failing `gh` skips issues and says why.
pub(crate) fn backfill_friction(
    cas_root: &Path,
    since: DateTime<Utc>,
    gh_program: &Path,
) -> anyhow::Result<FrictionBackfill> {
    let mut found = FrictionBackfill::default();
    for task in crate::store::open_task_store(cas_root)?.list(None)? {
        if task.created_at >= since {
            found.tasks += usize::from(bug_filed(
                cas_root,
                &task.id,
                &task.title,
                task.task_type,
                task.priority.0,
                task.created_at,
            ));
        }
    }
    let repo = crate::config::Config::load(cas_root)
        .ok()
        .and_then(|config| config.get("issues.components.cassy"))
        .filter(|repo| !repo.trim().is_empty());
    let Some(repo) = repo else {
        found.issues_skipped = Some("issues.components.cassy is not set".into());
        return Ok(found);
    };
    match list_cassy_issues(gh_program, &repo, since) {
        Ok(issues) => {
            for (url, title, created_at) in issues {
                found.issues += usize::from(issue_filed(cas_root, &url, &title, created_at));
            }
        }
        Err(reason) => found.issues_skipped = Some(reason),
    }
    Ok(found)
}

/// Issues on `repo` created since `since`, without feature requests.
fn list_cassy_issues(
    gh_program: &Path,
    repo: &str,
    since: DateTime<Utc>,
) -> Result<Vec<(String, String, DateTime<Utc>)>, String> {
    use crate::bounded_process::{Deadline, run_command};
    let budget = std::time::Duration::from_secs(15);
    let mut command = std::process::Command::new(gh_program);
    command
        .args([
            "issue", "list", "--repo", repo, "--state", "all", "--limit", "500",
            "--search", &format!("created:>={}", since.format("%Y-%m-%d")),
            "--json", "url,title,createdAt,labels",
        ])
        .env("GH_PROMPT_DISABLED", "1");
    let output = run_command(&mut command, Deadline::after(budget), budget)
        .map_err(|error| format!("gh issue list --repo {repo} did not complete: {error:?}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "gh issue list --repo {repo} failed: {}",
            stderr.lines().next().unwrap_or("").trim()
        ));
    }
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("gh issue list --repo {repo} returned unreadable JSON: {error}"))?;
    Ok(rows
        .iter()
        .filter(|row| {
            !row["labels"].as_array().is_some_and(|labels| {
                labels.iter().any(|label| {
                    matches!(label["name"].as_str(), Some("enhancement" | "feature"))
                })
            })
        })
        .filter_map(|row| {
            let created = DateTime::parse_from_rfc3339(row["createdAt"].as_str()?).ok()?;
            Some((
                row["url"].as_str()?.to_string(),
                row["title"].as_str()?.to_string(),
                created.with_timezone(&Utc),
            ))
        })
        .filter(|(_, _, created)| *created >= since)
        .collect())
}

/// A verdict from the numbers alone. A supervisor's handoff line can disagree;
/// the report shows both.
pub(crate) fn measured_verdict(injected: u64, used: u64) -> FeedbackVerdict {
    const MIN_SAMPLE: u64 = 20;
    if injected < MIN_SAMPLE {
        return if used > 0 { FeedbackVerdict::Helped } else { FeedbackVerdict::Unused };
    }
    // Under 1% use is noise even when a stray row was used.
    if used * 100 < injected { FeedbackVerdict::Noise } else { FeedbackVerdict::Helped }
}

/// The ledger's feature name for a retrieval family that injects context.
fn retrieval_feature(family: &str) -> Option<&'static str> {
    match family {
        "context_session_start" => Some("session_context"),
        family if family.starts_with("ambient_") => Some("ambient_recall"),
        _ => None,
    }
}

fn measured_feedback(
    natural_key: String,
    feature: &str,
    injected: u64,
    opened: u64,
    used: u64,
    occurred_at: DateTime<Utc>,
    actor: Option<&str>,
) -> NewQualityEvent {
    let mut event = NewQualityEvent::new(
        natural_key,
        QualityPayload::FeatureFeedback {
            feature: feature.to_string(),
            verdict: measured_verdict(injected, used),
            source: FeedbackSource::Measured,
            injected: Some(injected),
            opened: Some(opened),
            used: Some(used),
        },
    );
    event.occurred_at = occurred_at;
    event.actor = actor.map(str::to_string);
    event
}

/// Measured context-injection evidence for one session, summed by ledger feature.
fn feature_totals(cas_root: &Path, session_id: &str) -> Vec<(&'static str, (u64, u64, u64))> {
    let Ok(store) = cas_store::SqliteRetrievalStore::open(cas_root) else {
        return Vec::new();
    };
    let families = match store.evidence_funnel_by_family(Some(session_id)) {
        Ok(families) => families,
        Err(error) => {
            tracing::warn!(%error, "friction ledger: retrieval evidence unavailable");
            return Vec::new();
        }
    };
    let mut totals = std::collections::BTreeMap::<&'static str, (u64, u64, u64)>::new();
    for (family, funnel) in families {
        let Some(feature) = retrieval_feature(&family) else { continue };
        let total = totals.entry(feature).or_default();
        total.0 += funnel.injected;
        total.1 += funnel.opened;
        total.2 += funnel.used;
    }
    totals.into_iter().filter(|(_, (injected, _, _))| *injected > 0).collect()
}

/// A session ended: report what its injected context measured.
pub(crate) fn session_ended(cas_root: &Path, session_id: &str, actor: Option<&str>) {
    for (feature, (injected, opened, used)) in feature_totals(cas_root, session_id) {
        emit(
            cas_root,
            &measured_feedback(
                format!("measured|{session_id}|{feature}"),
                feature,
                injected,
                opened,
                used,
                Utc::now(),
                actor,
            ),
        );
    }
}

const FEEDBACK_LINE: &str = "feature_feedback:";

/// One handoff line per feature: `feature_feedback: <feature> <verdict> [note]`.
/// The note is free text, so it stays local. Returns the verdicts recorded.
pub(crate) fn handoff_saved(
    cas_root: &Path,
    entry_id: &str,
    content: &str,
    actor: Option<&str>,
) -> usize {
    let mut recorded = 0;
    for line in content.lines() {
        let Some(rest) = line.trim().strip_prefix(FEEDBACK_LINE) else { continue };
        let mut words = rest.split_whitespace();
        let (Some(feature), Some(verdict)) = (words.next(), words.next().and_then(FeedbackVerdict::parse))
        else {
            continue;
        };
        let note = words.collect::<Vec<_>>().join(" ");
        let mut event = NewQualityEvent::new(
            format!("supervisor|{entry_id}|{feature}"),
            QualityPayload::FeatureFeedback {
                feature: feature.to_ascii_lowercase(),
                verdict,
                source: FeedbackSource::Supervisor,
                injected: None,
                opened: None,
                used: None,
            },
        );
        event.actor = actor.map(str::to_string);
        event.local_note = (!note.is_empty()).then_some(note);
        recorded += usize::from(emit(cas_root, &event));
    }
    recorded
}

/// History already on disk, as events: nobody types seed data. Natural keys match
/// nothing a live emitter writes twice, and a rerun inserts nothing new.
/// Lifetime retrieval totals have no recoverable time scope; only session end
/// emits measured feature feedback, never a current-dated backfill snapshot.
pub(crate) fn backfill(cas_root: &Path, since: DateTime<Utc>) -> anyhow::Result<usize> {
    use cas_store::SupervisorQueueStore;
    use crate::store::{open_task_store, open_verification_store};
    let mut inserted = 0usize;
    let queue = cas_store::SqliteSupervisorQueueStore::open(cas_root)?;
    queue.init()?;
    // An alias can predate the requested window even when a later relay of
    // the same failed run is inside it. Read retained identities once, but
    // continue to emit only facts in the requested window.
    let history: Vec<_> = queue.event_history_since("merge_queue_ejected", DateTime::<Utc>::UNIX_EPOCH)?
        .into_iter().filter_map(|row| serde_json::from_str::<QueueEjection>(&row.payload).ok()
            .map(|ejection| (row, ejection))).collect();
    let mut aliases = HashMap::<(String, u64, u64), Vec<String>>::new();
    for (_, ejection) in &history {
        if let Some(run) = ejection.failed_run_id {
            aliases.entry((ejection.task_id.clone(), ejection.pr_number, run)).or_default()
                .push(queue_ejection_key(ejection, &ejection.occurrence));
        }
    }
    for (row, ejection) in history.iter().filter(|(row, _)| row.created_at >= since) {
        let keys = ejection.failed_run_id.and_then(|run| aliases.get(&(ejection.task_id.clone(), ejection.pr_number, run)))
            .map(Vec::as_slice).unwrap_or(&[]);
        inserted += usize::from(queue_ejected_with_aliases(cas_root, ejection, row.created_at, keys));
    }
    let tasks = open_task_store(cas_root)?.list(None)?;
    cas_store::backfill_delivery_cycles(cas_root, &project_scope(cas_root))?;
    for task in &tasks {
        inserted += attested_interventions(cas_root, &task.notes, since)?;
        for line in task.notes.lines() {
            let Some(at) = note_time(line).filter(|at| *at >= since) else { continue };
            let kind = if line.contains("DELIVERY BRANCH REBOUND:") {
                InterventionKind::Rebind
            } else if line.contains(" overrode ") && line.contains("before worktree_merge") {
                InterventionKind::OverrideMerge
            } else {
                continue;
            };
            let mut event = NewQualityEvent::new(
                format!("backfill|{}|{}|{}", kind.as_str(), task.id, line_digest(line)),
                QualityPayload::Intervention { intervention: kind, minutes: None },
            );
            event.occurred_at = at;
            event.task_id = Some(task.id.clone());
            event.title = Some(task.title.clone());
            inserted += usize::from(emit(cas_root, &event));
        }
    }
    let verifications = open_verification_store(cas_root)?.list_recent(10_000)?;
    for verification in verifications.iter().filter(|row| row.created_at >= since) {
        // Old close flows wrote descriptive dispatch markers as Error rows.
        // Keep that audit history, but do not invent an executed failed check.
        if matches!(
            verification.provenance,
            cas_types::VerificationProvenance::System | cas_types::VerificationProvenance::Legacy
        )
            && verification.status == cas_types::VerificationStatus::Error
            && verification.summary.starts_with("Dispatch requested")
        {
            continue;
        }
        let title = tasks.iter().find(|task| task.id == verification.task_id).map(|task| task.title.clone());
        let event = if verification.status == cas_types::VerificationStatus::Skipped {
            if !verification.summary.starts_with("Closed via supervisor") {
                continue;
            }
            NewQualityEvent::new(
                format!("backfill|override_close|{}", verification.id),
                QualityPayload::Intervention {
                    intervention: InterventionKind::OverrideClose,
                    minutes: None,
                },
            )
        } else {
            inserted += usize::from(verification_recorded(cas_root, verification));
            continue;
        };
        let mut event = event;
        event.occurred_at = verification.created_at;
        event.task_id = Some(verification.task_id.clone());
        event.title = title;
        inserted += usize::from(emit(cas_root, &event));
    }
    inserted += crate::ui::factory::daemon::runtime::backfill_quality_turns(cas_root, since)?;
    inserted += estimates::backfill_tasks(cas_root, since)?;
    Ok(inserted)
}

/// Non-verdict rows ran no completed check.
pub(crate) fn verifier_outcome(status: cas_types::VerificationStatus) -> Option<CheckOutcome> {
    use cas_types::VerificationStatus as Status;
    match status {
        Status::Approved => Some(CheckOutcome::Pass),
        Status::Rejected => Some(CheckOutcome::Fail),
        Status::Error => Some(CheckOutcome::Error),
        Status::Skipped | Status::Pending | Status::SupervisorCall => None,
    }
}

/// `[2026-10-31 14:25] ...`, the timestamp every task note starts with.
fn note_time(line: &str) -> Option<DateTime<Utc>> {
    let stamp = line.strip_prefix('[')?.split(']').next()?;
    chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M")
        .ok()
        .map(|time| time.and_utc())
}

fn line_digest(line: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    line.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(root: &Path, key: &str, outcome: CheckOutcome) {
        check_run(root, Check {
            natural_key: key.to_string(),
            check_id: MERGE_GATE_CHECK,
            stage: CheckStage::Lane,
            outcome,
            task_id: Some("cas-1"),
            sha: Some("HEAD"),
            minutes: None,
            link: None,
            actor: None,
        });
    }

    fn hand_merges(root: &Path) -> usize {
        cas_store::quality_interventions_since(
            root,
            InterventionKind::HandMerge,
            Utc::now() - Duration::minutes(5),
        )
        .unwrap()
        .len()
    }

    /// A close counts as a hand merge only when the tool's last word was a refusal.
    #[test]
    fn close_after_an_unresolved_refusal_is_a_hand_merge() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        gate(root, "refused", CheckOutcome::Fail);
        gate(root, "merged", CheckOutcome::Pass);
        task_closed(root, "cas-1", None, false, None);
        assert_eq!(hand_merges(root), 0, "refused, then merged by the tool");

        gate(root, "refused again", CheckOutcome::Fail);
        task_closed(root, "cas-1", None, false, None);
        assert_eq!(hand_merges(root), 1);
    }

    /// Real verdicts from one day: 42 errors against 5 rejections. Counting errors
    /// as failed deliveries would credit the verifier with catches it never made.
    #[test]
    fn a_verifier_error_is_not_a_failed_delivery() {
        use cas_types::VerificationStatus as Status;
        assert_eq!(verifier_outcome(Status::Rejected), Some(CheckOutcome::Fail));
        assert_eq!(verifier_outcome(Status::Error), Some(CheckOutcome::Error));
        assert_eq!(verifier_outcome(Status::Skipped), None);
    }

    #[test]
    fn review_backfill_binds_the_reviewed_cycle_without_recounting_legacy_checks() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        let tasks = crate::store::open_task_store(&root).unwrap();
        let mut task = cas_types::Task::new("cas-rounds".into(), "Returned delivery".into());
        task.deliverables.factory_branch_anchor = Some("a".repeat(40));
        tasks.add(&task).unwrap();
        let cycle = cas_store::delivery_cycles(&root, &task.id).unwrap()[0].cycle_id.clone();
        let dispatch = cas_store::create_verification_dispatch_bound(&root, &task.id, "worker", "supervisor",
            &cas_types::VerificationProofBoundary::task_at(cas_types::RepositoryProofBoundary {
                peer_premerge: None, repository_root: "/fixture".into(), worktree_root: "/fixture".into(),
                head_commit: "a".repeat(40), state_digest: "state".into(), target_branch: None,
                anchor_commits: vec!["a".repeat(40)],
            }), Utc::now() + Duration::minutes(10), false).unwrap();
        // The task has already advanced; a backfill must retain cycle 1.
        task = tasks.get(&task.id).unwrap();
        task.deliverables.decline_factory_branch_anchor();
        task.deliverables.delivery_cycle = 2;
        task.deliverables.factory_branch_anchor = Some("b".repeat(40));
        tasks.update(&task).unwrap();
        let mut verdict = cas_types::Verification::new("ver-round".into(), task.id.clone());
        verdict.provenance = cas_types::VerificationProvenance::TaskVerifier;
        verdict.status = cas_types::VerificationStatus::Rejected;
        verdict.dispatch_id = Some(dispatch.id);
        check_run(&root, Check { natural_key: format!("verifier|{}", verdict.id), check_id: "task_verifier",
            stage: CheckStage::Verifier, outcome: CheckOutcome::Fail, task_id: Some(&task.id),
            sha: None, minutes: None, link: None, actor: None });
        assert!(verification_recorded(&root, &verdict));
        assert!(!verification_recorded(&root, &verdict));
        let events = cas_store::quality_events_for_task(&root, &task.id).unwrap();
        assert_eq!(events.len(), 2);
        let QualityPayload::CheckRun { review: Some(review), .. } = &events[1].payload else { panic!("missing review evidence") };
        assert_eq!(review.cycle_id.as_deref(), Some(cycle.as_str()));
        assert_eq!(review.provenance, "independent");
        assert_eq!(review.supersedes.as_deref(), Some(events[0].id.as_str()));
        verdict.id = "ver-direct".into();
        verdict.provenance = cas_types::VerificationProvenance::SupervisorDirect;
        verdict.dispatch_id = None;
        assert!(verification_recorded(&root, &verdict));
        let events = cas_store::quality_events_for_task(&root, &task.id).unwrap();
        let QualityPayload::CheckRun { review: Some(review), .. } = &events[2].payload else { panic!("missing direct verdict") };
        assert_eq!(review.cycle_id, None);
        assert_eq!(review.provenance, "supervisor_direct");
    }

    #[test]
    fn backfill_does_not_count_dispatch_markers_as_failed_checks() {
        use cas_store::{RetrievalHitIdentity, RetrievalStore};
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let project = env.home().join("ledger-project");
        std::fs::create_dir_all(&project).unwrap();
        let root = crate::store::init_cas_dir(&project).unwrap();
        env.set("CAS_ROOT", &root);
        let task = cas_types::Task::new("cas-marker".into(), "Pending review".into());
        crate::store::open_task_store(&root).unwrap().add(&task).unwrap();
        // The old producer persisted an Error before any verifier was run.
        let mut marker = cas_types::Verification::new("ver-marker".into(), task.id.clone());
        marker.provenance = cas_types::VerificationProvenance::System;
        marker.status = cas_types::VerificationStatus::Error;
        marker.summary = "Dispatch requested (vdispatch-marker) — waiting for review".into();
        cas_store::add_system_verification(&root, &marker).unwrap();
        let mut error = cas_types::Verification::new("ver-real-error".into(), task.id);
        error.status = cas_types::VerificationStatus::Error;
        error.summary = "Verifier tool failed to open the evidence".into();
        crate::store::open_verification_store(&root).unwrap().add(&error).unwrap();

        // Lifetime retrieval evidence is real, but its time scope cannot be
        // recovered from the totals. Backfill must not date it as a new sample.
        let retrieval = cas_store::SqliteRetrievalStore::open(&root).unwrap();
        retrieval.record_query(
            "prior-query", "query", "ambient_transition", "current-default-v1",
            Some("prior-session"),
            &[RetrievalHitIdentity { result_id: "mem-1".into(), document_type: "entry".into(), rank: 0 }],
        ).unwrap();

        backfill(&root, Utc::now() - Duration::minutes(1)).unwrap();
        assert_eq!(backfill(&root, Utc::now() - Duration::days(2)).unwrap(), 0);
        let events = cas_store::quality_events_between(&root, None, None).unwrap();
        assert!(events.iter().all(|event| !matches!(event.payload, QualityPayload::FeatureFeedback { .. })),
            "backfill invented a release-window measurement from lifetime evidence: {events:?}");
        let checks: Vec<_> = events.iter().filter_map(|event| match &event.payload {
            QualityPayload::CheckRun { check_id, outcome, .. } => Some((check_id.as_str(), *outcome)),
            _ => None,
        }).collect();
        assert_eq!(checks, [("task_verifier", CheckOutcome::Error)], "only the real failed check counts");
    }

    /// The 10/31 numbers: 283 injected and none used is noise, not a win.
    #[test]
    fn measured_verdict_needs_a_sample_and_real_use() {
        assert_eq!(measured_verdict(283, 0), FeedbackVerdict::Noise);
        assert_eq!(measured_verdict(283, 1), FeedbackVerdict::Noise);
        assert_eq!(measured_verdict(100, 5), FeedbackVerdict::Helped);
        assert_eq!(measured_verdict(5, 0), FeedbackVerdict::Unused);
        assert_eq!(measured_verdict(5, 1), FeedbackVerdict::Helped);
    }
}
