//! Measured reference classes and promises in the existing append-only ledger.
use anyhow::{Context, Result, bail};
use cas_store::{
    CheckOutcome, CheckStage, EstimateReference, NewQualityEvent, QualityEvent, QualityPayload,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::path::Path;

const MIN_REFERENCES: usize = 5;
pub(crate) const PIPELINES: &[&str] = &["train_tip_suite", "merge_queue", "release_publish"];

#[derive(Debug, Serialize)]
pub(crate) struct Estimate {
    pub subject_kind: String,
    pub subject: String,
    pub typical_minutes: Option<f64>,
    pub bad_day_minutes: Option<f64>,
    pub evidence_count: usize,
    pub references: Vec<EstimateReference>,
    pub calibration: String,
    pub estimate_id: Option<String>,
    pub reference_details: Vec<TaskReference>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TaskReference {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub task_type: String,
    pub priority: i32,
    pub risk: Vec<cas_types::TaskRisk>,
    pub files_touched: Option<usize>,
    pub worker_model: Option<String>,
    pub lease_count: usize,
    pub distance: u32,
}

impl Estimate {
    pub fn render(&self) -> String {
        match (self.typical_minutes, self.bad_day_minutes) {
            (Some(p50), Some(p90)) => format!(
                "{}: typical {p50:.0} min, bad day {p90:.0} min; {} references ({})",
                self.subject, self.evidence_count, self.calibration
            ),
            _ => format!(
                "{}: uncalibrated: {} references; no measured range",
                self.subject, self.evidence_count
            ),
        }
    }
}

pub(crate) fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1]
}

fn task_reference(root: &Path, task: &cas_types::Task) -> TaskReference {
    let agents = crate::store::open_agent_store(root).ok();
    let history = agents
        .as_ref()
        .and_then(|s| s.get_lease_history(&task.id, None).ok())
        .unwrap_or_default();
    let started = history
        .iter()
        .filter(|h| h.event_type == "claimed")
        .min_by_key(|h| h.timestamp);
    let model = started
        .and_then(|h| agents.as_ref()?.get(&h.agent_id).ok())
        .and_then(|a| a.metadata.get("worker_model").cloned());
    let files = crate::store::open_task_store(root)
        .ok()
        .and_then(|s| s.get_execution_state(&task.id).ok().flatten())
        .and_then(|s| {
            s.get("files_touched")
                .and_then(|v| v.as_array())
                .map(Vec::len)
        });
    TaskReference {
        id: task.id.clone(),
        created_at: task.created_at,
        started_at: started.map(|h| h.timestamp),
        closed_at: task.closed_at,
        task_type: task.task_type.to_string(),
        priority: task.priority.0,
        risk: task.risk.clone(),
        files_touched: files,
        worker_model: model,
        lease_count: history.iter().filter(|h| h.event_type == "claimed").count(),
        distance: 0,
    }
}

fn task_minutes(reference: &TaskReference) -> Option<f64> {
    let seconds =
        (reference.closed_at? - reference.started_at.unwrap_or(reference.created_at)).num_seconds();
    (seconds > 0).then_some(seconds as f64 / 60.0)
}

/// A read is cheap and local. Refreshing remote evidence is an explicit backfill.
pub(crate) fn estimate(
    root: &Path,
    kind: &str,
    subject: &str,
    instance: Option<&str>,
    recipient: &str,
    limit: usize,
    record: bool,
) -> Result<Estimate> {
    if !matches!(kind, "pipeline" | "task" | "epic" | "release") {
        bail!("subject kind must be pipeline, task, epic or release");
    }
    if !matches!(recipient, "operator" | "client") {
        bail!("recipient must be operator or client");
    }
    if kind == "pipeline"
        && record
        && !instance.is_some_and(|sha| {
            sha.len() >= 7 && sha.len() <= 64 && sha.bytes().all(|c| c.is_ascii_hexdigit())
        })
    {
        bail!(
            "recorded pipeline estimates need a hexadecimal head SHA; use preview for an unbound read"
        );
    }
    let limit = limit.clamp(1, 200);
    let mut details = Vec::new();
    let mut samples: Vec<EstimateReference> = if matches!(kind, "task" | "epic") {
        let store = crate::store::open_task_store(root)?;
        let target_task = store.get(subject)?;
        let target = task_reference(root, &target_task);
        for task in store.list(Some(cas_types::TaskStatus::Closed))? {
            if task.id == subject
                || task.task_type.to_string() != target.task_type
                || task.scope != target_task.scope
                || matches!((&task.origin_project,&target_task.origin_project),(Some(a),Some(b)) if a!=b)
            {
                continue;
            }
            let mut reference = task_reference(root, &task);
            if task_minutes(&reference).is_none() {
                continue;
            }
            // Compare like endpoints; a missing lease timestamp is explicitly shown.
            reference.distance =
                8 * u32::from(reference.started_at.is_some() != target.started_at.is_some())
                    + 4 * u32::from(reference.risk != target.risk)
                    + reference.priority.abs_diff(target.priority)
                    + match (reference.files_touched, target.files_touched) {
                        (Some(a), Some(b)) => a.abs_diff(b).min(8) as u32,
                        (None, None) => 0,
                        _ => 2,
                    }
                    + 2 * u32::from(reference.worker_model != target.worker_model)
                    + reference.lease_count.abs_diff(target.lease_count).min(4) as u32;
            details.push(reference);
        }
        details.sort_by(|a, b| {
            a.distance
                .cmp(&b.distance)
                .then_with(|| b.closed_at.cmp(&a.closed_at))
                .then_with(|| a.id.cmp(&b.id))
        });
        details.truncate(limit);
        details
            .iter()
            .filter_map(|r| {
                Some(EstimateReference {
                    id: r.id.clone(),
                    minutes: task_minutes(r)?,
                })
            })
            .collect()
    } else {
        let stage = if kind == "release" {
            "release_total"
        } else {
            subject
        };
        if kind == "pipeline" && !PIPELINES.contains(&stage) {
            bail!("pipeline must be train_tip_suite, merge_queue or release_publish");
        }
        cas_store::quality_check_samples(root, stage, limit)?
            .into_iter()
            .filter(|e| !e.from_cloud)
            .filter_map(|e| match e.payload {
                QualityPayload::CheckRun {
                    check_id,
                    outcome: CheckOutcome::Pass,
                    minutes: Some(minutes),
                    ..
                } if check_id == stage && minutes.is_finite() && minutes > 0.0 => {
                    Some(EstimateReference { id: e.id, minutes })
                }
                _ => None,
            })
            .take(limit)
            .collect()
    };
    let count = samples.len();
    let mut sorted: Vec<_> = samples.iter().map(|r| r.minutes).collect();
    sorted.sort_by(f64::total_cmp);
    let typical = (!sorted.is_empty()).then(|| percentile(&sorted, 0.5));
    let bad_day = (!sorted.is_empty()).then(|| percentile(&sorted, 0.9));
    samples.truncate(5);
    details.truncate(5);
    let bound_subject =
        instance.map_or_else(|| subject.to_string(), |id| format!("{subject}:{id}"));
    let mut result = Estimate {
        subject_kind: kind.into(),
        subject: bound_subject.clone(),
        typical_minutes: typical,
        bad_day_minutes: bad_day,
        evidence_count: count,
        references: samples,
        calibration: if count < MIN_REFERENCES {
            format!("uncalibrated: {count} references")
        } else {
            "measured successful completions; p50/p90 nearest rank".into()
        },
        estimate_id: None,
        reference_details: details,
    };
    if record && let (Some(typical_minutes), Some(bad_day_minutes)) = (typical, bad_day) {
        let mut event = NewQualityEvent::new(
            format!("estimate|{kind}|{bound_subject}|{}", uuid::Uuid::new_v4()),
            QualityPayload::Estimate {
                subject_kind: kind.into(),
                subject: bound_subject,
                typical_minutes,
                bad_day_minutes,
                evidence_count: count as u64,
                references: result.references.clone(),
                recipient: recipient.into(),
            },
        );
        event.scope = super::project_scope(root);
        if matches!(kind, "task" | "epic") {
            event.task_id = Some(subject.into());
        }
        if kind == "release" {
            event.release_version = Some(subject.into());
        }
        event.actor = Some(crate::agent_id::compute_agent_id());
        // Losing telemetry cannot prevent returning a measured estimate.
        if super::emit(root, &event) {
            result.estimate_id = Some(event.id());
        }
    }
    Ok(result)
}

/// The observing path calls this only for a landed subject. Retries share an id.
pub(crate) fn complete(
    root: &Path,
    kind: &str,
    subject: &str,
    at: DateTime<Utc>,
    actual: Option<f64>,
    release: Option<&str>,
) {
    let Ok(events) = cas_store::quality_open_estimates(root) else {
        return;
    };
    for estimate in events
        .iter()
        .filter(|e| !e.from_cloud && e.occurred_at <= at)
    {
        let QualityPayload::Estimate {
            subject_kind,
            subject: bound,
            typical_minutes,
            bad_day_minutes,
            ..
        } = &estimate.payload
        else {
            continue;
        };
        if subject_kind != kind || bound != subject {
            continue;
        }
        let minutes = actual
            .unwrap_or_else(|| (at - estimate.occurred_at).num_milliseconds() as f64 / 60_000.0);
        if !minutes.is_finite() || minutes < 0.0 {
            continue;
        }
        let hit = minutes <= *bad_day_minutes;
        let below = minutes < *typical_minutes;
        let reason = if !hit {
            "above_p90"
        } else if below {
            "below_typical"
        } else {
            "within_range"
        };
        let mut event = NewQualityEvent::new(
            format!("estimate_outcome|{}", estimate.id),
            QualityPayload::EstimateOutcome {
                estimate_id: estimate.id.clone(),
                subject: subject.into(),
                actual_minutes: minutes,
                inside_range: hit,
                below_typical: below,
                reason: reason.into(),
            },
        );
        event.occurred_at = at;
        event.task_id = estimate.task_id.clone();
        event.release_version = release
            .map(str::to_owned)
            .or_else(|| estimate.release_version.clone());
        super::emit(root, &event);
        if !hit {
            let mut friction = NewQualityEvent::new(
                format!("friction|estimate|{}", estimate.id),
                QualityPayload::Friction {
                    source: cas_store::FrictionSource::Estimate,
                    reference: estimate.id.clone(),
                    severity: None,
                    project: super::project_slug(root),
                },
            );
            friction.occurred_at = at;
            friction.task_id = estimate.task_id.clone();
            friction.release_version = event.release_version;
            friction.title = Some(format!("Estimate missed: {subject}"));
            friction.local_note = Some(format!(
                "actual {minutes:.2} minutes exceeded p90 {bad_day_minutes:.2}"
            ));
            super::emit(root, &friction);
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct Calibration {
    pub completed: usize,
    pub hits: usize,
    pub below_typical: usize,
    pub hit_rate: Option<f64>,
    pub misses: Vec<Miss>,
}
#[derive(Debug, Serialize)]
pub(crate) struct Miss {
    pub estimate_id: String,
    pub subject: String,
    pub actual_minutes: f64,
    pub reason: String,
}

pub(crate) fn calibration(events: &[QualityEvent]) -> Calibration {
    let mut result = Calibration::default();
    let mut seen = std::collections::HashSet::new();
    for event in events {
        if let QualityPayload::EstimateOutcome {
            estimate_id,
            subject,
            actual_minutes,
            inside_range,
            below_typical,
            reason,
        } = &event.payload
        {
            if !seen.insert((&event.project_id, estimate_id)) {
                continue;
            }
            result.completed += 1;
            result.hits += usize::from(*inside_range);
            result.below_typical += usize::from(*below_typical);
            if !inside_range {
                result.misses.push(Miss {
                    estimate_id: estimate_id.clone(),
                    subject: subject.clone(),
                    actual_minutes: *actual_minutes,
                    reason: reason.clone(),
                });
            }
        }
    }
    if result.completed > 0 {
        result.hit_rate = Some(result.hits as f64 / result.completed as f64);
    }
    result
}

/// Recover facts, never retroactively invent a promise.
pub(crate) fn backfill_tasks(root: &Path, since: DateTime<Utc>) -> Result<usize> {
    let mut inserted = 0;
    for task in crate::store::open_task_store(root)?.list(Some(cas_types::TaskStatus::Closed))? {
        if task.closed_at.is_some_and(|at| at >= since) {
            inserted += record_task(root, &task)?;
        }
    }
    Ok(inserted)
}

pub(crate) fn task_landed(root: &Path, id: &str) -> Result<usize> {
    record_task(root, &crate::store::open_task_store(root)?.get(id)?)
}
fn record_task(root: &Path, task: &cas_types::Task) -> Result<usize> {
    let reference = task_reference(root, task);
    let Some(at) = task.closed_at else {
        return Ok(0);
    };
    let Some(minutes) = task_minutes(&reference) else {
        return Ok(0);
    };
    let mut event = NewQualityEvent::new(
        format!("task_duration|{}|{}", task.id, at.timestamp()),
        QualityPayload::CheckRun { review: None,
            check_id: format!("task_duration:{}", task.task_type),
            stage: CheckStage::Lane,
            outcome: CheckOutcome::Pass,
            sha: None,
            minutes: Some(minutes),
        },
    );
    event.occurred_at = at;
    event.task_id = Some(task.id.clone());
    event.title = Some(task.title.clone());
    event.local_note = serde_json::to_string(&reference).ok();
    let inserted = usize::from(super::emit(root, &event));
    complete(
        root,
        if task.task_type == cas_types::TaskType::Epic {
            "epic"
        } else {
            "task"
        },
        &task.id,
        at,
        None,
        None,
    );
    Ok(inserted)
}

/// Reusable-workflow jobs are named `<caller job> / <job>`; match the last segment.
pub(crate) fn is_suite_job(name: &str) -> bool {
    let n = name.rsplit(" / ").next().unwrap_or(name);
    n.starts_with("Fast Validation") && n.contains("suite")
}

/// GitHub's created time includes runner acquisition; job completion, unlike a
/// run's updated_at, cannot be moved by a later metadata update.
pub(crate) fn github_history(root: &Path, limit: usize, since: DateTime<Utc>) -> Result<usize> {
    let Some(repo) = crate::github_issue_attach::default_repo_for(root) else {
        return Ok(0);
    };
    let mut inserted = 0;
    for (workflow, event, stage) in [
        ("full-validation.yml", "push", "train_tip_suite"),
        ("full-validation.yml", "merge_group", "merge_queue"),
        ("ci.yml", "push", "train_tip_suite"),
        ("ci.yml", "merge_group", "merge_queue"),
        ("release.yml", "push", "release_publish"),
    ] {
        let path = format!(
            "repos/{repo}/actions/workflows/{workflow}/runs?event={event}&status=completed&per_page={}",
            limit.clamp(1, 100)
        );
        let runs = gh_json(&path)?;
        for run in runs["workflow_runs"].as_array().into_iter().flatten() {
            let Some(start) = time(&run["created_at"]) else {
                continue;
            };
            if start < since || run["conclusion"] != "success" {
                continue;
            }
            if stage == "train_tip_suite"
                && !run["head_branch"]
                    .as_str()
                    .is_some_and(|b| b.starts_with("epic/release-train-"))
            {
                continue;
            }
            let Some(id) = run["id"].as_u64() else {
                continue;
            };
            let jobs = gh_json(&format!(
                "repos/{repo}/actions/runs/{id}/jobs?filter=all&per_page=100"
            ))?;
            let Some(end) = jobs["jobs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|job| time(&job["completed_at"]))
                .max()
            else {
                continue;
            };
            if end < start {
                continue;
            }
            if stage == "train_tip_suite"
                && !jobs["jobs"].as_array().into_iter().flatten().any(|job| {
                    job["name"].as_str().is_some_and(is_suite_job)
                })
            {
                continue;
            }
            let sha = run["head_sha"].as_str().map(str::to_owned);
            inserted += record_pipeline_completion(
                root,
                stage,
                id,
                sha.as_deref(),
                start,
                end,
                run["html_url"].as_str(),
            );
        }
    }
    Ok(inserted)
}
pub(crate) fn pending_pipeline_subjects(root: &Path) -> std::collections::HashSet<String> {
    cas_store::quality_open_estimates(root)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            if let QualityPayload::Estimate {
                subject_kind,
                subject,
                ..
            } = e.payload
            {
                (subject_kind == "pipeline").then_some(subject)
            } else {
                None
            }
        })
        .collect()
}

pub(crate) fn record_pipeline_completion(
    root: &Path,
    stage: &str,
    id: u64,
    sha: Option<&str>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    link: Option<&str>,
) -> usize {
    if end < start {
        return 0;
    }
    let mut row = NewQualityEvent::new(
        format!("github_stage|{stage}|{id}"),
        QualityPayload::CheckRun { review: None,
            check_id: stage.into(),
            stage: if stage == "merge_queue" {
                CheckStage::Queue
            } else {
                CheckStage::Lane
            },
            outcome: CheckOutcome::Pass,
            sha: sha.map(str::to_owned),
            minutes: Some((end - start).num_seconds() as f64 / 60.0),
        },
    );
    row.occurred_at = end;
    row.link = link.map(str::to_owned);
    let inserted = usize::from(super::emit(root, &row));
    if let Some(sha) = sha {
        complete(root, "pipeline", &format!("{stage}:{sha}"), end, None, None);
    }
    inserted
}

fn time(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}
fn gh_json(path: &str) -> Result<serde_json::Value> {
    let program = std::env::var_os(crate::github_issue_attach::GH_BIN_ENV)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "gh".into());
    let output = crate::gh_budget::output(
        std::process::Command::new(program)
            .args(["api", path])
            .env("GH_PROMPT_DISABLED", "1"),
        std::time::Duration::from_secs(20),
    )?;
    if !output.status.success() {
        bail!("GitHub timing history unavailable (exit {})", output.status);
    }
    serde_json::from_slice(&output.stdout).context("decode GitHub timing history")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_validation_qualified_job_names_count_as_suite_jobs() {
        assert!(is_suite_job("Shared train and queue graph / Fast Validation — suite shard 2"));
        assert!(is_suite_job("Fast Validation — suite shard 1"));
        assert!(!is_suite_job("Shared train and queue graph / Full-set gate"));
    }
    fn fixture() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        crate::test_support::TestEnvGuard,
    ) {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        env.set("CAS_ROOT", &root);
        (temp, root, env)
    }
    #[test]
    fn measured_reference_classes_return_quantiles_and_closest_tasks() {
        let (_temp, root, _env) = fixture();
        for (i, minutes) in [8, 12, 16, 20, 31].into_iter().enumerate() {
            let end = Utc::now();
            assert_eq!(
                record_pipeline_completion(
                    &root,
                    "merge_queue",
                    i as u64,
                    Some("abc1234"),
                    end - chrono::Duration::minutes(minutes),
                    end,
                    None
                ),
                1
            );
        }
        let result = estimate(
            &root,
            "pipeline",
            "merge_queue",
            None,
            "operator",
            20,
            false,
        )
        .unwrap();
        assert_eq!(
            (
                result.typical_minutes,
                result.bad_day_minutes,
                result.evidence_count
            ),
            (Some(16.0), Some(31.0), 5)
        );
        assert_eq!(result.references.len(), 5);
        let small = estimate(&root, "pipeline", "merge_queue", None, "operator", 3, false).unwrap();
        assert_eq!(small.calibration, "uncalibrated: 3 references");
        assert_eq!(small.typical_minutes, Some(20.0));
        let missing = estimate(
            &root,
            "pipeline",
            "release_publish",
            Some("abc1234"),
            "client",
            20,
            true,
        )
        .unwrap();
        assert_eq!(missing.typical_minutes, None);
        assert_eq!(
            missing.estimate_id, None,
            "no invented promise without evidence"
        );
        let store = crate::store::open_task_store(&root).unwrap();
        let mut target = cas_types::Task::new("target".into(), "Requested work".into());
        target.task_type = cas_types::TaskType::Feature;
        store.add(&target).unwrap();
        for (i, minutes) in [8, 12, 16, 20, 31].into_iter().enumerate() {
            let mut task = cas_types::Task::new(format!("reference-{i}"), "Comparable work".into());
            task.task_type = target.task_type;
            task.status = cas_types::TaskStatus::Closed;
            task.closed_at = Some(Utc::now());
            task.created_at = task.closed_at.unwrap() - chrono::Duration::minutes(minutes);
            store.add(&task).unwrap();
        }
        let task = estimate(&root, "task", "target", None, "operator", 20, false).unwrap();
        assert_eq!(task.evidence_count, 5);
        assert_eq!(
            (task.typical_minutes, task.bad_day_minutes),
            (Some(16.0), Some(31.0))
        );
        assert_eq!(task.reference_details.len(), 5);
        assert!(
            task.reference_details
                .iter()
                .all(|r| r.files_touched.is_none())
        );
        assert!(
            estimate(
                &root,
                "pipeline",
                "merge_queue",
                Some("not-a-sha"),
                "operator",
                20,
                true
            )
            .is_err()
        );
    }

    #[test]
    fn historical_task_backfill_is_idempotent_and_never_invents_promises() {
        let (_temp, root, _env) = fixture();
        let mut task = cas_types::Task::new("closed".into(), "Historical task".into());
        task.status = cas_types::TaskStatus::Closed;
        task.created_at = Utc::now() - chrono::Duration::minutes(40);
        task.closed_at = Some(Utc::now() - chrono::Duration::minutes(10));
        crate::store::open_task_store(&root)
            .unwrap()
            .add(&task)
            .unwrap();
        let since = Utc::now() - chrono::Duration::days(1);
        assert_eq!(super::super::backfill(&root, since).unwrap(), 1);
        assert_eq!(super::super::backfill(&root, since).unwrap(), 0);
        let events = cas_store::quality_events_between(&root, None, None).unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0].payload,QualityPayload::CheckRun { minutes:Some(m),..} if m==30.0)
        );
    }
}
