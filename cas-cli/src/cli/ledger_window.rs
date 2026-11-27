use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::Path;

use anyhow::{Result, ensure};
use cas_store::{FrictionSource, QualityPayload};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;

use super::{LedgerReport, Measure, aggregate, health, median, sensors, unknown};

#[derive(Serialize)]
pub(super) struct WindowReport {
    #[serde(flatten)]
    pub(super) ledger: LedgerReport,
    pub(super) efficiency: Efficiency,
}

#[derive(Serialize)]
pub(super) struct Efficiency {
    interval: &'static str,
    refusals: Measure<Refusals>,
    supervisor_wakes_by_harness: Measure<BTreeMap<String, WakeLatency>>,
    issues: Issues,
}

#[derive(Serialize, Default)]
struct Refusals {
    calls: usize,
    facts: usize,
    by_code: BTreeMap<String, usize>,
    by_actor: BTreeMap<String, usize>,
    missing_actor: usize,
    missing_harness: usize,
    legacy_facts_without_call_id: usize,
    observed_merges: usize,
    per_observed_merge: Option<f64>,
}

#[derive(Serialize)]
struct Latency {
    samples: usize,
    median_seconds: Option<f64>,
    p90_seconds: Option<f64>,
}

impl Latency {
    fn from(mut values: Vec<f64>) -> Self {
        values.sort_by(f64::total_cmp);
        Self {
            samples: values.len(),
            median_seconds: median(values.clone()),
            p90_seconds: (!values.is_empty()).then(|| values[(values.len() * 9).div_ceil(10) - 1]),
        }
    }
}

#[derive(Serialize)]
struct WakeLatency {
    wakes: usize,
    created_to_injected: Latency,
    created_to_started: Latency,
    missing_start: usize,
    invalid_timestamps: usize,
}

#[derive(Serialize)]
struct Issues {
    recorded_filed: usize,
    agent_filed: usize,
    unattributed_filed: usize,
    closed_in_window: Measure<usize>,
    closures_in_reconcile_receipts: Measure<usize>,
}

pub(super) fn build(root: &Path, since: DateTime<Utc>, until: DateTime<Utc>, visibility: u32) -> Result<WindowReport> {
    ensure!(since < until, "--since must precede --until");
    let mut events = cas_store::quality_events_between(root, Some(since - Duration::seconds(1)), Some(until))?;
    events.retain(|row| row.occurred_at >= since && row.occurred_at < until);
    let mut ledger = aggregate("window".into(), false, Some(since), Some(until), &events);
    ledger.sensors = sensors::local(root, Some(since), Some(until), visibility);
    ledger.health = health::assess(&ledger);
    let db = Connection::open_with_flags(root.join("cas.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let merges = cas_store::observed_delivery_merges(root).map(|rows| rows.into_iter()
        .filter(|row| row.created_at >= since && row.created_at < until).count());
    let refusals = match merges {
        Ok(merges) => measured(refusals(&db, since, until, merges)),
        Err(error) => unknown(format!("merge record unreadable: {error}")),
    };
    let filed: BTreeMap<_, _> = events.iter().filter_map(|row| match &row.payload {
        QualityPayload::Friction { source: FrictionSource::Issue, reference, .. } => Some((reference.to_lowercase(), row.actor.is_some())),
        _ => None,
    }).fold(BTreeMap::new(), |mut rows, (reference, actor)| {
        rows.entry(reference).and_modify(|known| *known |= actor).or_insert(actor);
        rows
    });
    let agent_filed = filed.values().filter(|known| **known).count();
    let issues = Issues {
        recorded_filed: filed.len(), agent_filed, unattributed_filed: filed.len() - agent_filed,
        closed_in_window: unknown("issue close timestamps are not retained; receipt update time is only an observation"),
        closures_in_reconcile_receipts: measured(closures(&db, since, until)),
    };
    Ok(WindowReport { ledger, efficiency: Efficiency {
        interval: "[since, until)", refusals,
        supervisor_wakes_by_harness: measured(wakes(root, &db, since, until)), issues,
    } })
}

fn measured<T>(result: Result<T>) -> Measure<T> {
    result.map_or_else(|error| unknown(error.to_string()), Measure::Known)
}

fn refusals(db: &Connection, since: DateTime<Utc>, until: DateTime<Utc>, merges: usize) -> Result<Refusals> {
    let mut stmt = db.prepare("SELECT id, entity_id, metadata FROM events WHERE event_type='gate_refused'
        AND julianday(created_at)>=julianday(?1) AND julianday(created_at)<julianday(?2)")?;
    let mut found = Refusals { observed_merges: merges, ..Default::default() };
    let mut calls = BTreeSet::new();
    let mut actor_calls = BTreeMap::<String, BTreeSet<String>>::new();
    for row in stmt.query_map([since.to_rfc3339(), until.to_rfc3339()], |r|
        Ok((r.get::<_, i64>(0)?.to_string(), r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))? {
        let (id, code, metadata) = row?;
        let metadata: Value = metadata.and_then(|m| serde_json::from_str(&m).ok()).unwrap_or_default();
        *found.by_code.entry(code).or_default() += 1;
        found.facts += 1;
        found.missing_actor += usize::from(metadata["actor"].as_str().is_none());
        found.missing_harness += usize::from(metadata["harness"].as_str().is_none());
        found.legacy_facts_without_call_id += usize::from(metadata["fact_id"].as_str().is_none());
        let call = metadata["fact_id"].as_str().unwrap_or(&id).to_owned();
        actor_calls.entry(metadata["actor"].as_str().unwrap_or("unknown").to_owned()).or_default().insert(call.clone());
        calls.insert(call);
    }
    found.by_actor = actor_calls.into_iter().map(|(actor, calls)| (actor, calls.len())).collect();
    found.calls = calls.len();
    found.per_observed_merge = (merges > 0).then(|| found.calls as f64 / merges as f64);
    Ok(found)
}

fn wakes(root: &Path, db: &Connection, since: DateTime<Utc>, until: DateTime<Utc>) -> Result<BTreeMap<String, WakeLatency>> {
    let agents = crate::store::open_agent_store(root)?.list(None)?;
    let mut stmt = db.prepare("SELECT recipient,created_at,injected_at,started_at FROM supervisor_message_wakes
        WHERE julianday(created_at)>=julianday(?1) AND julianday(created_at)<julianday(?2)")?;
    let mut samples = BTreeMap::<String, (usize, Vec<f64>, Vec<f64>, usize, usize)>::new();
    for row in stmt.query_map([since.to_rfc3339(), until.to_rfc3339()], |r| Ok((
        r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?)))? {
        let (recipient, created, injected, started) = row?;
        let created = super::parse_time(&created);
        let injected = super::parse_time(&injected);
        let harness = agents.iter().filter(|a| (a.name == recipient || a.id == recipient)
            && injected.is_some_and(|at| a.registered_at <= at)).max_by_key(|a| a.registered_at)
            .and_then(|a| crate::agent_model::AgentModel::from_agent(a).harness).unwrap_or_else(|| "unknown".into());
        let sample = samples.entry(harness).or_default();
        sample.0 += 1;
        let elapsed = |end: Option<DateTime<Utc>>| created.zip(end).filter(|(a, b)| b >= a)
            .map(|(a, b)| (b - a).num_milliseconds() as f64 / 1000.0);
        match elapsed(injected) { Some(seconds) => sample.1.push(seconds), None => sample.4 += 1 }
        match elapsed(started.as_deref().and_then(super::parse_time)) {
            Some(seconds) => sample.2.push(seconds),
            None if started.is_none() => sample.3 += 1,
            None => sample.4 += 1,
        }
    }
    Ok(samples.into_iter().map(|(harness, (count, injected, started, missing, invalid))| (harness, WakeLatency {
        wakes: count, created_to_injected: Latency::from(injected), created_to_started: Latency::from(started),
        missing_start: missing, invalid_timestamps: invalid,
    })).collect())
}

fn closures(db: &Connection, since: DateTime<Utc>, until: DateTime<Utc>) -> Result<usize> {
    let mut stmt = db.prepare("SELECT receipt_json FROM release_reconcile_operations")?;
    let mut issues = BTreeSet::new();
    for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
        let receipt: Value = serde_json::from_str(&row?)?;
        if receipt["updated_at"].as_str().and_then(super::parse_time).is_some_and(|at| since <= at && at < until) {
            for issue in receipt["issue_outcomes"].as_array().into_iter().flatten() {
                if issue["action"] == "closed" && let Some(url) = issue["issue"].as_str() {
                    issues.insert(url.to_lowercase());
                }
            }
        }
    }
    Ok(issues.len())
}

pub(super) fn render(efficiency: &Efficiency) -> String {
    let mut out = String::new();
    let seconds = |n: Option<f64>| n.map_or_else(|| "unknown".into(), |n| format!("{n:.2}"));
    let _ = writeln!(out, "\nRecorded factory window {} (local rows only)", efficiency.interval);
    let _ = writeln!(out, "  Refusals: {}", efficiency.refusals.render(|r| format!(
        "{} calls / {} observed merges; {} per merge; calls by actor {:?}; {} code facts ({:?}); {} without actor, {} without harness, {} legacy facts without a call id",
        r.calls, r.observed_merges, r.per_observed_merge.map_or_else(|| "unknown".into(), |n| format!("{n:.2}")),
        r.by_actor, r.facts, r.by_code, r.missing_actor, r.missing_harness, r.legacy_facts_without_call_id)));
    let _ = writeln!(out, "  Supervisor wake latency: {}", efficiency.supervisor_wakes_by_harness.render(|rows| {
        rows.iter().map(|(h, r)| format!("{h}: {} wakes; injection median/p90 {}/{} s; start median/p90 {}/{} s ({} starts, {} missing, {} invalid)",
            r.wakes, seconds(r.created_to_injected.median_seconds), seconds(r.created_to_injected.p90_seconds),
            seconds(r.created_to_started.median_seconds), seconds(r.created_to_started.p90_seconds),
            r.created_to_started.samples, r.missing_start, r.invalid_timestamps)).collect::<Vec<_>>().join("; ")
    }));
    let i = &efficiency.issues;
    let _ = writeln!(out, "  Issues: {} recorded filed; {} agent-filed, {} unattributed; closed: {}; closure receipts updated in window: {}",
        i.recorded_filed, i.agent_filed, i.unattributed_filed, i.closed_in_window.render(|n| n.to_string()),
        i.closures_in_reconcile_receipts.render(|n| n.to_string()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::{AgentStore, SqliteAgentStore};
    use cas_store::release_reconcile_store::{ReleaseOperation, ReleaseOperationKey};
    use crate::issue_reconcile::{IssueAction, IssueOutcome};
    use crate::qa_batch::ReleaseScope;
    use crate::release_reconcile::ReleaseReconcileReceipt;
    use cas_types::{Agent, AgentRole};
    use clap::Parser;
    use serde_json::json;

    #[test]
    fn scorecard_window_reads_recorded_rows_and_preserves_missing_measurements() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let since = super::super::parse_time("2026-11-27T04:00:00Z").unwrap();
        let until = since + Duration::hours(1);
        let agents = SqliteAgentStore::open(root).unwrap();
        agents.init().unwrap();
        let mut agent = Agent::new_with_role("actor".into(), "lead".into(), AgentRole::Supervisor);
        agent.registered_at = since - Duration::hours(1);
        agent.metadata.insert("supervisor_cli".into(), "codex".into());
        agents.register(&agent).unwrap();
        let store = crate::store::open_event_store(root).unwrap();
        store.init().unwrap();
        let db = Connection::open(root.join("cas.db")).unwrap();
        for (at, code, metadata) in [
            (since - Duration::seconds(1), "G-61", json!({})),
            (since, "G-61", json!({"fact_id":"call-1", "actor":"actor", "harness":"codex"})),
            (since, "G-68", json!({"fact_id":"call-1", "actor":"actor", "harness":"codex"})),
            (since + Duration::minutes(1), "UNCODED", json!({})),
            (since + Duration::minutes(2), "G-61", json!({"fact_id":"call-2", "actor":"other-actor", "harness":"claude"})),
            (until, "G-61", json!({})),
        ] {
            db.execute("INSERT INTO events(event_type,entity_type,entity_id,summary,metadata,created_at) VALUES ('gate_refused','gate',?1,'',?2,?3)",
                rusqlite::params![code, metadata.to_string(), at.to_rfc3339()]).unwrap();
        }
        cas_store::observed_delivery_merges(root).unwrap();
        for n in 0..2 {
            db.execute("INSERT INTO observed_delivery_merges(task_id,source_branch,target_branch,source_commit_sha,merge_commit_sha,actor_agent_id,created_at)
                VALUES (?1,'factory/test','epic/test',?2,?3,'actor',?4)", rusqlite::params![
                format!("cas-{n}"), format!("source-{n}"), format!("merge-{n}"), since.to_rfc3339()]).unwrap();
        }
        for (n, at, actor) in [(1, since, Some("actor")), (2, since + Duration::minutes(2), None), (3, until, Some("actor"))] {
            assert!(crate::quality_ledger::issue_filed(root, &format!("https://github.com/org/repo/issues/{n}"), "issue", at, actor));
        }
        db.execute_batch("CREATE TABLE supervisor_message_wakes (lead_id INTEGER,recipient TEXT,created_at TEXT,injected_at TEXT,started_at TEXT);").unwrap();
        for (id, created, injected, started, recipient) in [
            (1, since, since + Duration::seconds(10), Some(since + Duration::seconds(20)), "lead"),
            (2, since, since + Duration::seconds(30), None, "lead"),
            (3, until, until, Some(until), "lead"),
            (4, since, since, Some(since), "missing-agent"),
        ] {
            db.execute("INSERT INTO supervisor_message_wakes VALUES (?1,?2,?3,?4,?5)", rusqlite::params![
                id, recipient, created.to_rfc3339(), injected.to_rfc3339(), started.map(|t| t.to_rfc3339())]).unwrap();
        }
        for (n, at) in [since, until].into_iter().enumerate() {
            let key = ReleaseOperationKey {
                project_id: "scorecard-fixture".into(), release_id: format!("v1.0.{n}"), final_release_sha: "a".repeat(40),
            };
            let receipt = ReleaseReconcileReceipt {
                scope: ReleaseScope {
                    project_id: key.project_id.clone(), repository: "org/repo".into(), release_id: key.release_id.clone(),
                    reviewed_sha: key.final_release_sha.clone(), final_release_sha: key.final_release_sha.clone(),
                    promotion_pr: None, release_tag: Some(key.release_id.clone()), constituent_epic_ids: vec![],
                    included_task_ids: vec![], no_code_members: BTreeMap::new(), proof_refs: vec![],
                },
                key: key.clone(), reconciler_version: Some(env!("CARGO_PKG_VERSION").into()), reconciler_stale: false,
                preview: None, triggers: vec![], members: BTreeMap::new(), close_intents: BTreeMap::new(),
                issue_outcomes: vec![
                    IssueOutcome { issue: format!("org/repo/issues/{}", n * 2 + 1), action: IssueAction::Closed,
                        comment_or_error: String::new(), task_ids: vec![] },
                    IssueOutcome { issue: "org/repo/issues/2".into(), action: IssueAction::Partial,
                        comment_or_error: String::new(), task_ids: vec![] },
                ],
                parents: BTreeMap::new(), exceptions: vec![], started_at: at.to_rfc3339(), updated_at: at.to_rfc3339(),
                complete: true, sync_pending: vec![], residuals: vec![], notice_intent: None,
            };
            ReleaseOperation::open(root, key).unwrap().save(&serde_json::to_value(receipt).unwrap()).unwrap();
        }
        let report = build(root, since, until, 10).unwrap();
        assert_eq!(serde_json::to_value(&report).unwrap()["efficiency"]["refusals"]["by_actor"],
            json!({"actor":1,"other-actor":1,"unknown":1}));
        let text = render(&report.efficiency);
        assert!(text.contains("other-actor"));
        let Measure::Known(r) = report.efficiency.refusals else { panic!("refusals unavailable") };
        assert_eq!((r.calls, r.facts, r.observed_merges), (3, 4, 2));
        assert_eq!(r.per_observed_merge, Some(1.5));
        assert_eq!((r.missing_actor, r.missing_harness, r.legacy_facts_without_call_id), (1, 1, 1));
        let Measure::Known(w) = report.efficiency.supervisor_wakes_by_harness else { panic!("wakes unavailable") };
        assert_eq!((w["codex"].wakes, w["codex"].missing_start), (2, 1));
        assert_eq!(w["codex"].created_to_injected.median_seconds, Some(20.0));
        assert_eq!(w["codex"].created_to_injected.p90_seconds, Some(30.0));
        assert_eq!(w["codex"].created_to_started.median_seconds, Some(20.0));
        assert_eq!(w["unknown"].wakes, 1);
        let i = report.efficiency.issues;
        assert_eq!((i.recorded_filed, i.agent_filed, i.unattributed_filed), (2, 1, 1));
        assert_eq!(i.closures_in_reconcile_receipts, Measure::Known(1));
        assert!(matches!(i.closed_in_window, Measure::Unknown { .. }));
        assert_eq!(report.ledger.events, 2);
        assert!(build(root, until, since, 10).is_err());
        db.execute_batch("DROP TABLE supervisor_message_wakes; DROP TABLE release_reconcile_operations; DROP TABLE events;").unwrap();
        let missing = build(root, since, until, 10).unwrap();
        assert!(matches!(missing.efficiency.refusals, Measure::Unknown { .. }));
        assert!(matches!(missing.efficiency.supervisor_wakes_by_harness, Measure::Unknown { .. }));
        assert!(matches!(missing.efficiency.issues.closures_in_reconcile_receipts, Measure::Unknown { .. }));
        for prefix in [vec!["cas", "scorecard"], vec!["cas", "ledger", "report"]] {
            let mut args = prefix.clone();
            args.extend(["--since", "2026-11-27T04:00:00Z", "--until", "2026-11-27T05:00:00Z"]);
            assert!(crate::cli::Cli::try_parse_from(&args).is_ok());
            args.extend(["--release", "v1.0.0"]);
            assert!(crate::cli::Cli::try_parse_from(args).is_err());
            let mut args = prefix;
            args.extend(["--since", "2026-11-27T04:00:00Z"]);
            assert!(crate::cli::Cli::try_parse_from(args).is_err());
        }
    }
}
