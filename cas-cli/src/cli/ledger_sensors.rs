//! Measurements only. GitHub captures are replayable; missing or truncated
//! sources remain unknown. Park cohorts follow the cas-15dd baseline clock.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Measure, unknown};

#[derive(Debug, Serialize)]
pub(super) struct Sensors {
    pub github_captured_at: Option<DateTime<Utc>>,
    pub github_window_end: Option<DateTime<Utc>>,
    pub hosted_runner_losses: Measure<RunnerLosses>,
    pub daily_issues: Measure<IssueDays>,
    pub park_to_train_merge: Measure<ParkLatency>,
    pub cross_machine_parks: Measure<Visibility>,
}
impl Default for Sensors {
    fn default() -> Self {
        Self {
            github_captured_at: None,
            github_window_end: None,
            hosted_runner_losses: unknown("GitHub jobs not read"),
            daily_issues: unknown("project issues not read"),
            park_to_train_merge: unknown("park history not read"),
            cross_machine_parks: unknown("park visibility not read"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Loss {
    job_id: u64,
    run_id: u64,
    job: String,
    runner_labels: Vec<String>,
    evidence: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(super) struct RunnerLosses {
    pub(super) jobs_examined: usize,
    pub(super) losses: Vec<Loss>,
    by_job_and_runner_labels: BTreeMap<String, usize>,
    /// Missing logs/annotations prevent a known zero, but retain confirmed hits.
    pub(super) missing: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(super) struct IssueDay {
    opened: usize,
    closed: usize,
    agent_filed: usize,
    agent_filed_share: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct IssueDays {
    repo: String,
    /// UTC calendar days; only events in the release interval count.
    pub(super) days: BTreeMap<String, IssueDay>,
    agent_evidence: String,
}
#[derive(Debug, Clone, Serialize)]
struct Park {
    task_id: String,
    parked_at: DateTime<Utc>,
    source: String,
    origin_machine: Option<String>,
    owner_machine: Option<String>,
    seen: BTreeMap<String, DateTime<Utc>>,
}
#[derive(Debug, Serialize)]
struct Pair {
    task_id: String,
    parked_at: DateTime<Utc>,
    merged_at: Option<DateTime<Utc>>,
    minutes: Option<f64>,
    source: String,
}
#[derive(Debug, Serialize)]
pub(super) struct ParkLatency {
    pub(super) first_parks: usize,
    pub(super) merged: usize,
    unmatched: usize,
    median_minutes: Option<f64>,
    p90_minutes: Option<f64>,
    captured_at: DateTime<Utc>,
    pairs: Vec<Pair>,
}
#[derive(Debug, Serialize)]
pub(super) struct Visibility {
    threshold_minutes: u32,
    /// Parks inspected, and those whose origin machine was also their merge
    /// owner. A zero cross-machine count is only meaningful beside these.
    pub(super) examined: usize,
    pub(super) same_machine: BTreeMap<String, usize>,
    pub(super) observed_cross_machine: usize,
    seen_within_threshold: usize,
    late: Vec<String>,
    /// Missing observation is censored, never proof a park was lost.
    not_observed_by_capture: Vec<String>,
    pub(super) unmeasurable: Vec<String>,
    captured_at: DateTime<Utc>,
}

fn time(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|t| t.with_timezone(&Utc))
        .or_else(|| {
            NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|t| t.and_utc())
        })
}
fn in_window(at: DateTime<Utc>, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
    from <= at && at < to
}
fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|word| word.strip_prefix(name))
        .map(|s| s.trim_end_matches(']'))
}
fn attribute<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    line.split_once(&format!("{name}=\""))?.1.split('"').next()
}

fn parks(db: &rusqlite::Connection) -> Result<Vec<Park>> {
    let mut found = BTreeMap::<String, Park>::new();
    let mut stmt = db.prepare("SELECT id, notes FROM tasks")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (task, notes) = row?;
        let mut first: Option<(DateTime<Utc>, String, Option<String>, Option<String>)> = None;
        let mut seen = BTreeMap::new();
        for line in notes.lines() {
            if line.get(17..).is_some_and(|tail| {
                tail.starts_with("] Close rejected:")
                    && tail.contains("Task parked as awaiting_merge;")
            }) && let Some(at) = line
                .get(1..17)
                .and_then(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").ok())
                .map(|s| s.and_utc())
            {
                let exact = field(line, "parked_at=")
                    .and_then(time)
                    .filter(|t| *t >= at && *t < at + Duration::minutes(1))
                    .unwrap_or(at);
                if first.as_ref().is_none_or(|(prior, _, _, _)| exact < *prior) {
                    first = Some((
                        exact,
                        "canonical task park note".into(),
                        field(line, "origin_machine=").map(str::to_string),
                        field(line, "merge_owner_machine=").map(str::to_string),
                    ));
                }
            }
            if line.contains("[CAS_SYNC_STATUS]")
                && line.contains("applied_status=awaiting_merge")
                && let (Some(machine), Some(at)) = (
                    field(line, "observer_machine="),
                    field(line, "observed_at=").and_then(time),
                )
            {
                seen.entry(machine.to_string())
                    .and_modify(|old| {
                        if at < *old {
                            *old = at
                        }
                    })
                    .or_insert(at);
            }
        }
        if let Some((parked_at, source, origin_machine, owner_machine)) = first {
            found.insert(
                task.clone(),
                Park {
                    task_id: task,
                    parked_at,
                    source,
                    origin_machine,
                    owner_machine,
                    seen,
                },
            );
        }
    }
    // Same retained relay seam and second-precision refinement as cas-15dd.
    let mut stmt = db.prepare("SELECT id, prompt FROM prompt_queue WHERE prompt LIKE '%<task-lifecycle transition=\"task_awaiting_merge\"%'")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (id, prompt) = row?;
        let Some(tag) = prompt
            .split("<task-lifecycle")
            .nth(1)
            .and_then(|s| s.split('>').next())
        else {
            continue;
        };
        let (Some(task), Some(at)) = (
            attribute(tag, "task_id"),
            attribute(tag, "occurrence").and_then(time),
        ) else {
            continue;
        };
        let row = found.entry(task.into()).or_insert_with(|| Park {
            task_id: task.into(),
            parked_at: at,
            source: format!("task_awaiting_merge relay #{id}"),
            origin_machine: None,
            owner_machine: None,
            seen: BTreeMap::new(),
        });
        if at < row.parked_at
            || (row.source.starts_with("canonical") && at < row.parked_at + Duration::minutes(1))
        {
            row.parked_at = at;
            row.source = format!("task_awaiting_merge relay #{id}");
        }
    }
    Ok(found.into_values().collect())
}

fn latency(
    parks: &[Park],
    merges: &[(String, DateTime<Utc>)],
    captured_at: DateTime<Utc>,
) -> ParkLatency {
    let pairs: Vec<_> = parks
        .iter()
        .map(|p| {
            let merged_at = merges
                .iter()
                .filter(|(task, at)| task == &p.task_id && *at >= p.parked_at && *at <= captured_at)
                .map(|(_, at)| *at)
                .min();
            Pair {
                task_id: p.task_id.clone(),
                parked_at: p.parked_at,
                merged_at,
                minutes: merged_at
                    .map(|at| (at - p.parked_at).num_milliseconds() as f64 / 60_000.0),
                source: p.source.clone(),
            }
        })
        .collect();
    let mut minutes: Vec<_> = pairs.iter().filter_map(|p| p.minutes).collect();
    minutes.sort_by(f64::total_cmp);
    let n = minutes.len();
    ParkLatency {
        first_parks: pairs.len(),
        merged: n,
        unmatched: pairs.len() - n,
        median_minutes: (n > 0).then(|| (minutes[(n - 1) / 2] + minutes[n / 2]) / 2.0),
        p90_minutes: (n > 0).then(|| minutes[(9 * n).div_ceil(10) - 1]),
        captured_at,
        pairs,
    }
}
fn visibility(parks: &[Park], captured_at: DateTime<Utc>, threshold_minutes: u32) -> Visibility {
    let mut out = Visibility {
        threshold_minutes,
        examined: parks.len(),
        same_machine: BTreeMap::new(),
        observed_cross_machine: 0,
        seen_within_threshold: 0,
        late: vec![],
        not_observed_by_capture: vec![],
        unmeasurable: vec![],
        captured_at,
    };
    for p in parks {
        let (Some(origin), Some(owner)) = (&p.origin_machine, &p.owner_machine) else {
            out.unmeasurable.push(format!(
                "{}: origin/merge-owner machine not retained",
                p.task_id
            ));
            continue;
        };
        if origin == owner {
            *out.same_machine.entry(origin.clone()).or_default() += 1;
            continue;
        }
        out.observed_cross_machine += 1;
        let deadline = p.parked_at + Duration::minutes(i64::from(threshold_minutes));
        match p
            .seen
            .get(owner)
            .filter(|seen| **seen >= p.parked_at && **seen <= captured_at)
        {
            Some(seen) if *seen <= deadline => out.seen_within_threshold += 1,
            Some(_) => out.late.push(p.task_id.clone()),
            None if captured_at >= deadline => out.not_observed_by_capture.push(p.task_id.clone()),
            None => out.unmeasurable.push(format!(
                "{}: observation deadline has not elapsed",
                p.task_id
            )),
        }
    }
    out
}

pub(super) fn local(
    root: &Path,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    threshold: u32,
) -> Sensors {
    let mut out = Sensors::default();
    let Some(from) = from else {
        out.park_to_train_merge = unknown("release window has no start");
        out.cross_machine_parks = unknown("release window has no start");
        return out;
    };
    let captured = Utc::now();
    let result = (|| -> Result<_> {
        let db =
            crate::qa_batch::release_close::read_connection(root).map_err(anyhow::Error::msg)?;
        db.execute_batch("BEGIN")?;
        let parks = parks(&db)?
            .into_iter()
            .filter(|p| in_window(p.parked_at, from, to.unwrap_or(captured)))
            .collect::<Vec<_>>();
        anyhow::ensure!(
            !parks.is_empty(),
            "no retained first parks in this release window"
        );
        let mut stmt = db.prepare("SELECT task_id, created_at FROM observed_delivery_merges WHERE target_branch LIKE 'epic/%'")?;
        let merges = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .map(|row| {
                let (task, at) = row?;
                Ok((task, time(&at).context("invalid merge timestamp")?))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((
            latency(&parks, &merges, captured),
            visibility(&parks, captured, threshold),
        ))
    })();
    match result {
        Ok((latency, visibility)) => {
            out.park_to_train_merge = Measure::Known(latency);
            out.cross_machine_parks = Measure::Known(visibility);
        }
        Err(e) => {
            out.park_to_train_merge = unknown(format!("park/merge history: {e}"));
            out.cross_machine_parks = unknown(format!("park visibility: {e}"));
        }
    }
    out
}

fn gh(root: &Path, path: &str, plain: bool) -> Result<Vec<u8>> {
    let program = std::env::var_os(crate::github_issue_attach::GH_BIN_ENV)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "gh".into());
    let _ = root;
    let output = crate::gh_budget::output(
        Command::new(program)
            .args(["api", path])
            .env("GH_PROMPT_DISABLED", "1"),
        std::time::Duration::from_secs(20),
    )?;
    if !output.status.success() {
        bail!("GitHub {path}: {}", output.status)
    }
    if !plain {
        serde_json::from_slice::<Value>(&output.stdout).context("GitHub JSON")?;
    }
    Ok(output.stdout)
}
fn json(root: &Path, path: &str) -> Result<Value> {
    Ok(serde_json::from_slice(&gh(root, path, false)?)?)
}
fn pages(root: &Path, path: &str, key: Option<&str>) -> Result<Vec<Value>> {
    let mut rows = vec![];
    for page in 1..=100 {
        let value = json(
            root,
            &format!(
                "{path}{}per_page=100&page={page}",
                if path.contains('?') { '&' } else { '?' }
            ),
        )?;
        let batch = key
            .map_or(&value, |k| &value[k])
            .as_array()
            .context("GitHub collection missing")?;
        rows.extend(batch.iter().cloned());
        if let Some(total) = value.get("total_count").and_then(Value::as_u64) {
            // GitHub Actions caps filtered run queries at 1000 results.
            if key == Some("workflow_runs") && total >= 1000 {
                bail!("Actions window reaches 1000-run cap; narrow the release capture")
            }
            if rows.len() == total as usize {
                return Ok(rows);
            }
            if rows.len() > total as usize || batch.is_empty() {
                bail!("GitHub pagination changed/truncated")
            }
        } else if batch.len() < 100 {
            return Ok(rows);
        }
    }
    bail!("GitHub pagination exceeds 100 pages")
}
fn loss_evidence(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut evidence = vec![];
    if lower.contains("runner lost communication")
        || lower.contains("runner has lost communication")
    {
        evidence.push("runner_lost_communication".into());
    }
    if lower.contains("shutdown")
        && (lower.contains("exit code 143") || lower.contains("exit code: 143"))
    {
        evidence.push("exit_143_shutdown".into());
    }
    evidence
}
fn runners(
    root: &Path,
    repo: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<RunnerLosses> {
    // Subdivide busy release intervals instead of silently dropping runs at
    // GitHub's 1000-result Actions search limit.
    let mut windows = vec![(from, to)];
    let mut runs = vec![];
    while let Some((start, end)) = windows.pop() {
        let path = format!(
            "repos/{repo}/actions/runs?created={}..{}",
            start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        );
        let first = json(root, &format!("{path}&per_page=100&page=1"))?;
        let total = first["total_count"].as_u64().context("run total missing")?;
        if total >= 1000 {
            anyhow::ensure!(
                (end - start).num_seconds() > 1,
                "Actions search cap within a single second"
            );
            let middle = start + Duration::seconds((end - start).num_seconds() / 2);
            windows.extend([(start, middle), (middle, end)]);
        } else {
            runs.extend(pages(root, &path, Some("workflow_runs"))?);
        }
    }
    let mut out = RunnerLosses::default();
    let mut seen = BTreeSet::new();
    for run in runs {
        let created = run["created_at"]
            .as_str()
            .and_then(time)
            .context("run creation timestamp missing")?;
        if !in_window(created, from, to) {
            continue;
        }
        let id = run["id"].as_u64().context("run ID missing")?;
        let jobs = pages(
            root,
            &format!("repos/{repo}/actions/runs/{id}/jobs?filter=all"),
            Some("jobs"),
        )?;
        for job in jobs {
            let job_id = job["id"].as_u64().context("job ID missing")?;
            if !seen.insert(job_id) {
                continue;
            }
            out.jobs_examined += 1;
            if matches!(job["conclusion"].as_str(), Some("success" | "skipped")) {
                continue;
            }
            if job["status"] != "completed" {
                out.missing.push(format!("job {job_id}: still running"));
                continue;
            }
            let labels: Vec<_> = job["labels"]
                .as_array()
                .context("runner labels missing")?
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            if labels.iter().any(|s| s.eq_ignore_ascii_case("self-hosted")) {
                continue;
            }
            if labels.is_empty() || job["runner_name"].as_str().is_none_or(str::is_empty) {
                out.missing.push(format!(
                    "job {job_id}: hosted runner allocation not retained"
                ));
                continue;
            }
            if !job["runner_name"]
                .as_str()
                .is_some_and(|name| name.starts_with("GitHub Actions"))
                && job["runner_group_name"] != "GitHub Actions"
            {
                out.missing.push(format!(
                    "job {job_id}: hosted runner identity not established"
                ));
                continue;
            }
            let mut evidence = vec![];
            if let Some(url) = job["check_run_url"].as_str() {
                let path = url
                    .strip_prefix("https://api.github.com/")
                    .context("unexpected check URL")?;
                match pages(root, &format!("{path}/annotations"), None) {
                    Ok(rows) => {
                        for annotation in rows {
                            evidence.extend(loss_evidence(
                                annotation["message"].as_str().unwrap_or(""),
                            ));
                        }
                    }
                    Err(e) => out.missing.push(format!("job {job_id} annotations: {e}")),
                }
            }
            if evidence.is_empty() {
                match gh(
                    root,
                    &format!("repos/{repo}/actions/jobs/{job_id}/logs"),
                    true,
                ) {
                    Ok(log) => evidence.extend(loss_evidence(&String::from_utf8_lossy(&log))),
                    Err(e) => out.missing.push(format!("job {job_id} logs: {e}")),
                }
            }
            evidence.sort();
            evidence.dedup();
            if !evidence.is_empty() {
                let mut labels = labels;
                labels.sort();
                let name = job["name"]
                    .as_str()
                    .context("job name missing")?
                    .to_string();
                *out.by_job_and_runner_labels
                    .entry(format!("{name} [{}]", labels.join(", ")))
                    .or_default() += 1;
                out.losses.push(Loss {
                    job_id,
                    run_id: id,
                    job: name,
                    runner_labels: labels,
                    evidence,
                });
            }
        }
    }
    Ok(out)
}
fn issues(root: &Path, repo: &str, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<IssueDays> {
    // Updated >= start covers older issues closed during the window too. The
    // REST endpoint includes PRs, which must never enter the issue denominator.
    let rows = pages(
        root,
        &format!(
            "repos/{repo}/issues?state=all&since={}&sort=created&direction=asc",
            from.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        ),
        None,
    )?;
    let mut days = BTreeMap::<String, IssueDay>::new();
    let mut day = from.date_naive();
    while day.and_hms_opt(0, 0, 0).unwrap().and_utc() < to {
        days.insert(day.to_string(), IssueDay::default());
        day = day.succ_opt().context("date overflow")?;
    }
    for row in rows {
        if row.get("pull_request").is_some() {
            continue;
        }
        if let Some(at) = row["created_at"]
            .as_str()
            .and_then(time)
            .filter(|at| in_window(*at, from, to))
        {
            let day = days.entry(at.date_naive().to_string()).or_default();
            day.opened += 1;
            if row["labels"]
                .as_array()
                .is_some_and(|labels| labels.iter().any(|l| l["name"] == "agent-reported"))
                || row["body"].as_str().is_some_and(|body| {
                    body.contains(
                        "*Reported by agent via `mcp__cas__system action=report_cas_bug`*",
                    )
                })
            {
                day.agent_filed += 1;
            }
        }
        if let Some(at) = row["closed_at"]
            .as_str()
            .and_then(time)
            .filter(|at| in_window(*at, from, to))
        {
            days.entry(at.date_naive().to_string()).or_default().closed += 1;
        }
    }
    for day in days.values_mut() {
        day.agent_filed_share =
            (day.opened > 0).then(|| day.agent_filed as f64 / day.opened as f64);
    }
    Ok(IssueDays {
        repo: repo.into(),
        days,
        agent_evidence: "agent-reported label or Cassy filing signature; share of opens".into(),
    })
}

#[derive(Serialize, Deserialize)]
struct Capture {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    captured_at: DateTime<Utc>,
    actions_repo: Option<String>,
    issues_repo: Option<String>,
    runners: Measure<RunnerLosses>,
    issues: Measure<IssueDays>,
}
pub(super) fn external(
    root: &Path,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    offline: bool,
    out: &mut Sensors,
) {
    out.github_captured_at = None;
    out.github_window_end = None;
    let Some(from) = from else { return };
    let end = to.unwrap_or_else(Utc::now);
    let config = crate::config::Config::load(root).ok();
    let actions_repo = crate::github_issue_attach::default_repo_for(root);
    let issues_repo = config
        .as_ref()
        .and_then(|c| c.issue_repo_registry_for_project(root).project);
    let path = root.join("ledger-sensors").join(format!(
        "{}-{}.json",
        from.timestamp(),
        to.map_or_else(|| "current".into(), |at| at.timestamp().to_string())
    ));
    if offline {
        match std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Capture>(&bytes).ok())
        {
            Some(c)
                if c.from == from
                    && to.is_none_or(|to| c.to == to)
                    && c.actions_repo == actions_repo
                    && c.issues_repo == issues_repo =>
            {
                out.github_captured_at = Some(c.captured_at);
                out.github_window_end = Some(c.to);
                out.hosted_runner_losses = c.runners;
                out.daily_issues = c.issues;
            }
            _ => {
                out.hosted_runner_losses = unknown(format!(
                    "offline: no matching Actions capture at {}",
                    path.display()
                ));
                out.daily_issues = unknown(format!(
                    "offline: no matching issues capture at {}",
                    path.display()
                ));
            }
        }
        return;
    }
    let measured =
        |result: Result<_>| result.map_or_else(|e| unknown(e.to_string()), Measure::Known);
    let runners = actions_repo.as_deref().map_or_else(
        || unknown("Actions repository not configured"),
        |repo| measured(runners(root, repo, from, end)),
    );
    let issues = issues_repo.as_deref().map_or_else(
        || unknown("project issues.repo not configured"),
        |repo| {
            issues(root, repo, from, end).map_or_else(|e| unknown(e.to_string()), Measure::Known)
        },
    );
    let capture = Capture {
        from,
        to: end,
        captured_at: Utc::now(),
        actions_repo,
        issues_repo,
        runners,
        issues,
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&capture) {
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(path.with_extension("tmp"), bytes)?;
            std::fs::rename(path.with_extension("tmp"), &path)
        };
        if let Err(e) = write() {
            tracing::warn!(%e, "ledger sensor capture could not be retained");
        }
    }
    out.github_captured_at = Some(capture.captured_at);
    out.github_window_end = Some(capture.to);
    out.hosted_runner_losses = capture.runners;
    out.daily_issues = capture.issues;
}

pub(super) fn render(out: &mut String, sensors: &Sensors) {
    use std::fmt::Write;
    let _ = writeln!(out, "\nFriction sensors");
    if let (Some(at), Some(end)) = (sensors.github_captured_at, sensors.github_window_end) {
        let _ = writeln!(out, "  GitHub capture: {at}; input window ends {end}");
    }
    let _ = writeln!(
        out,
        "  Hosted runner losses: {}",
        sensors.hosted_runner_losses.render(|r| format!(
            "{} confirmed / {} jobs; {} missing inputs",
            r.losses.len(),
            r.jobs_examined,
            r.missing.len()
        ))
    );
    if let Measure::Known(r) = &sensors.hosted_runner_losses {
        for (key, count) in &r.by_job_and_runner_labels {
            let _ = writeln!(out, "    {key}: {count}");
        }
        for reason in &r.missing {
            let _ = writeln!(out, "    unknown: {reason}");
        }
    }
    let _ = writeln!(
        out,
        "  Daily project issues: {}",
        sensors.daily_issues.render(|r| r.repo.clone())
    );
    if let Measure::Known(r) = &sensors.daily_issues {
        for (date, d) in &r.days {
            let _ = writeln!(
                out,
                "    {date} UTC: {} opened, {} closed, {} agent-filed ({})",
                d.opened,
                d.closed,
                d.agent_filed,
                d.agent_filed_share.map_or_else(
                    || "no opens".into(),
                    |s| format!("{:.1}% of opens", 100.0 * s)
                )
            );
        }
    }
    let _ = writeln!(out, "  First park to train merge: {}", sensors.park_to_train_merge.render(|p|
        format!("{}/{} merged, {} unmatched; median {}, p90 {} min (completed only; first-park cohort; capture {})", p.merged, p.first_parks, p.unmatched,
            p.median_minutes.map_or_else(|| "unknown: no matched merges".into(), |n| format!("{n:.1}")),
            p.p90_minutes.map_or_else(|| "unknown: no matched merges".into(), |n| format!("{n:.1}")), p.captured_at)));
    let _ = writeln!(out, "  Cross-machine parks: {}", sensors.cross_machine_parks.render(|v|
        format!("N={} min; {} cross-machine, {} seen in time, {} late, {} not observed by capture, {} unknown", v.threshold_minutes, v.observed_cross_machine,
            v.seen_within_threshold, v.late.len(), v.not_observed_by_capture.len(), v.unmeasurable.len())));
    if let Measure::Known(v) = &sensors.cross_machine_parks {
        if !v.same_machine.is_empty() {
            let _ = writeln!(out, "    {} park(s) examined; same machine: {}", v.examined,
                v.same_machine.iter().map(|(machine, n)| format!("{machine} {n}")).collect::<Vec<_>>().join(", "));
        }
        for task in &v.late {
            let _ = writeln!(
                out,
                "    {task}: observed after {} min deadline",
                v.threshold_minutes
            );
        }
        for task in &v.not_observed_by_capture {
            let _ = writeln!(
                out,
                "    {task}: no retained merge-owner observation by {}; censored, not proof of loss",
                v.captured_at
            );
        }
        for reason in &v.unmeasurable {
            let _ = writeln!(out, "    unknown: {reason}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn first_park_cohort_replays_baseline_and_censors_visibility() {
        let captured = time("2026-11-18T12:00:00Z").unwrap();
        let mut captured_parks = vec![];
        let mut merges = vec![];
        for line in include_str!("../../tests/fixtures/ledger-park-baseline.txt")
            .lines()
            .filter(|s| !s.starts_with('#'))
        {
            let cols: Vec<_> = line.split_whitespace().collect();
            let at = DateTime::from_timestamp(cols[2].parse().unwrap(), 0).unwrap();
            if cols[0] == "M" {
                merges.push((cols[1].into(), at));
            } else {
                captured_parks.push(Park {
                    task_id: cols[1].into(),
                    parked_at: at,
                    source: "cas-15dd capture art-f72d6618".into(),
                    origin_machine: None,
                    owner_machine: None,
                    seen: BTreeMap::new(),
                });
            }
        }
        // Original independent Python statistics.median / nearest-rank oracle.
        for (start, end, count, merged, median, p90) in [
            (
                "2026-11-03T05:00:00Z",
                "2026-11-10T05:00:00Z",
                415,
                243,
                5.0,
                101.8,
            ),
            (
                "2026-11-10T05:00:00Z",
                "2026-11-17T05:00:00Z",
                399,
                247,
                20.8,
                652.5,
            ),
        ] {
            let cohort: Vec<_> = captured_parks
                .iter()
                .filter(|p| in_window(p.parked_at, time(start).unwrap(), time(end).unwrap()))
                .cloned()
                .collect();
            let measured = latency(&cohort, &merges, captured);
            assert_eq!(
                (measured.first_parks, measured.merged, measured.unmatched),
                (count, merged, count - merged)
            );
            assert!((measured.median_minutes.unwrap() - median).abs() < 0.05);
            assert!((measured.p90_minutes.unwrap() - p90).abs() < 0.05);
        }
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE tasks(id TEXT, notes TEXT); CREATE TABLE prompt_queue(id INTEGER, prompt TEXT);
").unwrap();
        db.execute("INSERT INTO tasks VALUES('cas-x', ?)", [concat!(
            "[2026-11-18 10:00] Close rejected: test. Task parked as awaiting_merge; [CAS_PARK origin_machine=prowl parked_at=2026-11-18T10:00:00Z] merge_owner_machine=soundwave\n",
            "[2026-11-18 10:20] [CAS_SYNC_STATUS] applied_status=awaiting_merge observer_machine=soundwave observed_at=2026-11-18T10:20:00Z\n",
            "[2026-11-18 11:00] Close rejected: repark. Task parked as awaiting_merge;"
        )]).unwrap();
        db.execute("INSERT INTO prompt_queue VALUES(7, ?)", ["<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"cas-x\" occurrence=\"2026-11-18T10:00:25Z\">"]).unwrap();
        let mut actual = parks(&db).unwrap();
        assert_eq!(actual.len(), 1);
        assert_eq!(actual[0].parked_at, time("2026-11-18T10:00:25Z").unwrap());
        let matched = latency(
            &actual,
            &[
                ("cas-x".into(), time("2026-11-18T09:59:59Z").unwrap()),
                ("cas-x".into(), time("2026-11-18T10:05:25Z").unwrap()),
                ("cas-x".into(), captured),
            ],
            captured,
        );
        assert_eq!(matched.median_minutes, Some(5.0));
        let late = visibility(&actual, captured, 10);
        assert_eq!(late.late, vec!["cas-x"]);
        assert_eq!(visibility(&actual, captured, 30).seen_within_threshold, 1);
        actual[0].seen.clear();
        assert_eq!(
            visibility(&actual, captured, 10).not_observed_by_capture,
            vec!["cas-x"]
        );
        actual[0].origin_machine = None;
        assert_eq!(visibility(&actual, captured, 10).unmeasurable.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn release_sensors_collect_evidence_and_replay_offline_without_fabricated_zeroes() {
        use std::os::unix::fs::PermissionsExt;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        env.set("CAS_ROOT", &root);
        let mut config = crate::config::Config::load(&root).unwrap();
        config.set("history.github_repo", "owner/code").unwrap();
        config.set("issues.repo", "owner/issues").unwrap();
        config.save(&root).unwrap();
        let job = |id, labels, conclusion| {
            json!({"id":id,"name":"suite","runner_name":"GitHub Actions 9","labels":labels,
            "status":"completed","conclusion":conclusion,"check_run_url":format!("https://api.github.com/repos/owner/code/check-runs/{id}")})
        };
        let fixtures = json!({
            "repos/owner/code/actions/runs": {"total_count":1,"workflow_runs":[{"id":10,"created_at":"2026-11-18T10:00:00Z"}]},
            "repos/owner/code/actions/runs/10/jobs": {"total_count":5,"jobs":[job(11,json!(["ubuntu-latest"]),"failure"),job(12,json!(["ubuntu-latest"]),"cancelled"),
                job(13,json!(["self-hosted"]),"failure"),job(14,json!(["ubuntu-latest"]),"failure"),job(15,json!(["ubuntu-latest"]),"success")]},
            "repos/owner/code/check-runs/11/annotations":[{"message":"The runner has lost communication with the server."}],
            "repos/owner/code/check-runs/12/annotations":[], "repos/owner/code/actions/jobs/12/logs":"Runner shutdown; process completed with exit code 143",
            "repos/owner/code/check-runs/14/annotations":[], "repos/owner/code/actions/jobs/14/logs":"test assertion 143 failed with exit code 1",
            "repos/owner/issues/issues":[
                {"created_at":"2026-11-18T10:00:00Z","labels":[{"name":"agent-reported"}],"closed_at":null},
                {"created_at":"2026-11-17T10:00:00Z","labels":[],"closed_at":"2026-11-18T10:00:00Z"},
                {"created_at":"2026-11-18T11:00:00Z","labels":[],"closed_at":"2026-11-19T00:00:00Z"},
                {"created_at":"2026-11-18T12:00:00Z","labels":[],"pull_request":{},"closed_at":null}]
        });
        let gh_path = temp.path().join("gh");
        std::fs::write(gh_path.with_extension("json"), fixtures.to_string()).unwrap();
        std::fs::write(
            &gh_path,
            r#"#!/usr/bin/env python3
import json,sys,pathlib
fixtures=json.loads(pathlib.Path(__file__).with_suffix('.json').read_text())
path=sys.argv[2].split('?')[0]
if path not in fixtures: sys.exit(1)
value=fixtures[path]
print(value if isinstance(value,str) else json.dumps(value))
"#,
        )
        .unwrap();
        std::fs::set_permissions(&gh_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        env.set(crate::github_issue_attach::GH_BIN_ENV, &gh_path);
        let from = Some(time("2026-11-18T00:00:00Z").unwrap());
        let to = Some(time("2026-11-19T00:00:00Z").unwrap());
        let mut sensors = Sensors::default();
        external(&root, from, to, false, &mut sensors);
        let json = serde_json::to_value(&sensors).unwrap();
        assert_eq!(
            json["hosted_runner_losses"]["losses"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(json["hosted_runner_losses"]["jobs_examined"], 5);
        assert_eq!(json["hosted_runner_losses"]["missing"], json!([]));
        assert_eq!(json["daily_issues"]["days"]["2026-11-18"]["opened"], 2);
        assert_eq!(json["daily_issues"]["days"]["2026-11-18"]["closed"], 1);
        assert_eq!(
            json["daily_issues"]["days"]["2026-11-18"]["agent_filed_share"],
            0.5
        );
        assert_eq!(json["daily_issues"]["days"].as_object().unwrap().len(), 1);
        std::fs::remove_file(&gh_path).unwrap();
        let mut offline = Sensors::default();
        external(&root, from, to, true, &mut offline);
        assert_eq!(serde_json::to_value(&offline).unwrap(), json);
        external(
            &root,
            from,
            Some(time("2026-11-20T00:00:00Z").unwrap()),
            true,
            &mut offline,
        );
        assert!(matches!(
            offline.hosted_runner_losses,
            Measure::Unknown { .. }
        ));
        assert!(matches!(offline.daily_issues, Measure::Unknown { .. }));
        let mut report = super::super::aggregate("v1.0.0".into(), true, from, to, &[]);
        report.sensors = sensors;
        let mut text = String::new();
        render(&mut text, &report.sensors);
        assert!(!text.is_empty());
        let report_json = serde_json::to_value(report).unwrap();
        assert_eq!(report_json["sensors"], json);
    }
}
