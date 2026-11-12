//! Rubric scorers over the friction ledger (cas-a5eb). Each scorer is a
//! four-label rubric (name, question, labels with value, description and
//! score) shared by the report and any later judge. A hit is a label below the
//! pass line between 0.8 and 0.4, and it names its subject and evidence ids.
//!
//! Measurement only: nothing here gates, refuses or writes. The sources are
//! the rows Cassy already keeps: review rounds in `quality_events`, supervisor
//! notices, release receipts, task events and factory-session hold records.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use cas_store::CheckOutcome;
use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{DeliveryReviews, LedgerReport};

#[derive(Debug, Serialize)]
pub(super) struct Label {
    value: &'static str,
    description: &'static str,
    score: f64,
}

const fn label(value: &'static str, description: &'static str, score: f64) -> Label {
    Label { value, description, score }
}

#[derive(Debug, Serialize)]
pub(super) struct Hit {
    /// The delivery, receipt, train or worker the hit is about.
    pub subject: String,
    pub label: &'static str,
    pub score: f64,
    pub measure: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub evidence: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct Scorer {
    pub name: &'static str,
    pub question: &'static str,
    pub labels: &'static [Label],
    /// Subjects graded, including passes.
    pub scored: usize,
    /// Count per label value over every graded subject.
    pub tally: BTreeMap<&'static str, usize>,
    pub hits: Vec<Hit>,
    /// Subjects seen but not gradable, with why.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unscored: Vec<String>,
    /// Why the source could not be read at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

const PASS_LINE: f64 = 0.6;

impl Scorer {
    fn new(name: &'static str, question: &'static str, labels: &'static [Label]) -> Self {
        Self { name, question, labels, scored: 0, tally: BTreeMap::new(), hits: Vec::new(), unscored: Vec::new(), unavailable: None }
    }

    /// Grade one subject by label index; keep it as a hit below the pass line.
    fn grade(&mut self, index: usize, subject: String, measure: String,
        window: Option<(DateTime<Utc>, DateTime<Utc>)>, evidence: Vec<String>) {
        let label = &self.labels[index];
        self.scored += 1;
        *self.tally.entry(label.value).or_default() += 1;
        if label.score < PASS_LINE {
            self.hits.push(Hit { subject, label: label.value, score: label.score, measure, window, evidence });
        }
    }
}

const REVIEW_ROUNDS: &[Label] = &[
    label("first_pass", "one review round: the delivery passed when first reviewed", 1.0),
    label("one_return", "two rounds: returned once, then passed with no further cost", 0.8),
    label("repeated_returns", "three or four rounds: review returned it repeatedly; an earlier check would have saved rounds", 0.4),
    label("review_loop", "five or more rounds: the same delivery cycled through review", 0.2),
];

const SUPERVISOR_ALONE: &[Label] = &[
    label("independent", "an independent reviewer recorded the approving verdict", 1.0),
    label("independent_then_supervisor", "an independent reviewer graded this delivery; the supervisor recorded the final pass", 0.8),
    label("supervisor_alone", "the supervisor approved it directly and no independent reviewer graded it", 0.4),
    label("supervisor_alone_repeated", "the supervisor graded it alone over several rounds, including its own rejections", 0.2),
];

const RECEIPT_HOLDS: &[Label] = &[
    label("no_holds", "the receipt closed or skipped every member without a hold notice", 1.0),
    label("one_notice", "one grouped hold notice for the whole receipt", 0.8),
    label("notice_burst", "two to five hold notices for one receipt", 0.4),
    label("notice_storm", "more than five hold notices for one receipt: a notice per task", 0.2),
];

const RELEASE_HELD: &[Label] = &[
    label("cut_promptly", "cut within 15 minutes of being green with its verdict recorded", 1.0),
    label("short_wait", "sat ready for up to an hour", 0.8),
    label("held", "sat green and verdict-recorded but uncut for one to four hours", 0.4),
    label("held_long", "sat green and verdict-recorded but uncut for more than four hours", 0.2),
];

const IDLE_READY: &[Label] = &[
    label("brief", "held or idle with ready work for up to 15 minutes", 1.0),
    label("short", "held or idle with ready work for up to 30 minutes", 0.8),
    label("idle", "held or idle for 30 minutes to two hours while its epic had ready, unblocked tasks", 0.4),
    label("idle_long", "held or idle for more than two hours while its epic had ready, unblocked tasks", 0.2),
];

/// Every scorer for one report. Sources that cannot be read leave that scorer
/// `unavailable`, never a silent zero.
pub(super) fn score(cas_root: &Path, repo: &Path, report: &LedgerReport) -> Vec<Scorer> {
    let end = report.to.unwrap_or_else(Utc::now);
    let local = crate::cloud::resolve_canonical_id(cas_root).unwrap_or_default();
    let db = crate::qa_batch::release_close::read_connection(cas_root);
    let receipts = crate::qa_batch::release_close::read_receipts(cas_root, &local)
        .map(|all| all.into_iter().filter(|r| matches!(r.schema_version,
            cas_store::RELEASE_VERDICT_SCHEMA | cas_store::RELEASE_TAG_VERDICT_SCHEMA)).collect::<Vec<_>>());
    let ours = |release: Option<String>| release.unwrap_or_else(|| "unreleased".into()) == report.release;
    let mut scorers = vec![review_rounds(&report.reviews)];
    scorers.push(match &db {
        Ok(db) => supervisor_alone(db, &report.reviews, &local),
        Err(e) => unavailable(Scorer::new("supervisor_alone", Q_SUPERVISOR, SUPERVISOR_ALONE), e),
    });
    let mine: Result<Vec<_>, String> = receipts.map(|all| all.into_iter()
        .filter(|r| ours(crate::issue_reconcile::release_tag_containing(repo, &r.staging_sha))).collect());
    scorers.push(match (&db, &mine) {
        (Ok(db), Ok(mine)) => receipt_holds(db, mine),
        (Err(e), _) | (_, Err(e)) => unavailable(Scorer::new("hold_notices_per_receipt", Q_HOLDS, RECEIPT_HOLDS), e),
    });
    scorers.push(match (&db, &mine) {
        (Ok(db), Ok(mine)) => release_held(db, repo, mine, end),
        (Err(e), _) | (_, Err(e)) => unavailable(Scorer::new("release_held_ready", Q_HELD, RELEASE_HELD), e),
    });
    scorers.push(match &db {
        Ok(db) => idle_with_ready_work(db, cas_root, report.from, end),
        Err(e) => unavailable(Scorer::new("idle_with_ready_work", Q_IDLE, IDLE_READY), e),
    });
    scorers
}

fn unavailable(mut scorer: Scorer, reason: &str) -> Scorer {
    scorer.unavailable = Some(reason.to_string());
    scorer
}

fn review_rounds(reviews: &[DeliveryReviews]) -> Scorer {
    let mut scorer = Scorer::new("review_rounds",
        "Did each delivery pass review within two rounds?", REVIEW_ROUNDS);
    for row in reviews {
        let index = match row.rounds.len() { 0 => continue, 1 => 0, 2 => 1, 3 | 4 => 2, _ => 3 };
        scorer.grade(index, delivery(row), format!("{} rounds", row.rounds.len()), None,
            row.rounds.iter().map(|r| r.verification_id.clone()).collect());
    }
    scorer
}

fn delivery(row: &DeliveryReviews) -> String {
    format!("{} {}", row.task, row.cycle_id.as_deref().unwrap_or("cycle unknown"))
}

const Q_SUPERVISOR: &str = "Was each approved design, spike or deliverable graded by someone other than the supervisor?";

/// Decision-bearing: spikes (designs are spikes) and no-code deliverables.
fn supervisor_alone(db: &rusqlite::Connection, reviews: &[DeliveryReviews], local: &str) -> Scorer {
    let mut scorer = Scorer::new("supervisor_alone", Q_SUPERVISOR, SUPERVISOR_ALONE);
    for row in reviews {
        if row.project != "this project" && row.project != local { continue }
        let Some(last_pass) = row.rounds.iter().rev().find(|r| r.outcome == CheckOutcome::Pass) else { continue };
        let kind: Option<(String, Option<String>)> = db.query_row(
            "SELECT task_type, execution_note FROM tasks WHERE id = ?1", [&row.task],
            |r| Ok((r.get(0)?, r.get(1)?))).ok();
        let Some((task_type, note)) = kind else {
            scorer.unscored.push(format!("{}: task not in this store", row.task));
            continue;
        };
        if task_type != "spike" && note.as_deref() != Some("no-code") { continue }
        let known = |p: &str| p == "independent" || p == "supervisor_direct";
        if !known(&last_pass.provenance) {
            scorer.unscored.push(format!("{}: approving verdict {} has {} provenance", row.task, last_pass.verification_id, last_pass.provenance));
            continue;
        }
        let independent = row.rounds.iter().any(|r| r.provenance == "independent");
        let direct = row.rounds.iter().filter(|r| r.provenance == "supervisor_direct").count();
        let index = match (last_pass.provenance.as_str(), independent) {
            ("independent", _) => 0,
            (_, true) => 1,
            _ if direct > 1 => 3,
            _ => 2,
        };
        let kind = if task_type == "spike" { "spike" } else { "no-code deliverable" };
        scorer.grade(index, delivery(row), format!("{kind}; {direct} supervisor-direct verdict(s)"), None,
            row.rounds.iter().map(|r| format!("{} {}", r.verification_id, r.provenance)).collect());
    }
    scorer
}

const Q_HOLDS: &str = "Did each release receipt raise at most one grouped hold notice?";

struct Notice { id: i64, at: DateTime<Utc>, key: Option<String>, payload: serde_json::Value }

fn notices(db: &rusqlite::Connection, kinds: &[&str]) -> rusqlite::Result<Vec<Notice>> {
    let mut stmt = db.prepare("SELECT id, created_at, transition_key, payload FROM supervisor_queue
        WHERE event_type = 'supervisor_notice' OR event_type IN (SELECT value FROM json_each(?1)) ORDER BY id")?;
    let kinds_json = serde_json::to_string(kinds).unwrap_or_default();
    let rows = stmt.query_map([&kinds_json], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?,
        r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, at, key, payload) = row?;
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap_or_default();
        if !payload["kind"].as_str().is_some_and(|k| kinds.contains(&k)) { continue }
        let Some(at) = sqlite_time(&at) else { continue };
        out.push(Notice { id, at, key, payload });
    }
    Ok(out)
}

pub(super) fn sqlite_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value).map(|t| t.with_timezone(&Utc)).ok().or_else(||
        chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f").ok().map(|t| t.and_utc()))
}

/// The receipt a hold notice belongs to: its context names it; per-task
/// notices from before grouping name it in their first line.
fn notice_receipt(notice: &Notice) -> Option<String> {
    notice.payload["context"]["receipt_id"].as_str().map(str::to_string).or_else(|| {
        let detail = notice.payload["detail"].as_str()?;
        let id = detail.strip_prefix("Receipt ")?.split_whitespace().next()?;
        id.starts_with("qab-").then(|| id.to_string())
    })
}

fn receipt_holds(db: &rusqlite::Connection, receipts: &[cas_store::QaBatchReceipt]) -> Scorer {
    let mut scorer = Scorer::new("hold_notices_per_receipt", Q_HOLDS, RECEIPT_HOLDS);
    let holds = match notices(db, &["batch_close_hold"]) {
        Ok(holds) => holds,
        Err(e) => return unavailable(scorer, &e.to_string()),
    };
    let mut by_receipt = BTreeMap::<String, Vec<&Notice>>::new();
    for notice in &holds {
        if let Some(receipt) = notice_receipt(notice) { by_receipt.entry(receipt).or_default().push(notice); }
    }
    for receipt in receipts {
        let found = by_receipt.get(&receipt.id).map_or(&[][..], Vec::as_slice);
        let index = match found.len() { 0 => 0, 1 => 1, 2..=5 => 2, _ => 3 };
        let tasks: BTreeSet<_> = found.iter().filter_map(|n| n.key.as_deref()
            .and_then(|k| k.strip_prefix("supervisor-notice:batch_close_hold:"))
            .and_then(|rest| rest.split(':').next()).filter(|id| !id.starts_with("qab-")).map(str::to_string))
            .chain(found.iter().filter_map(|n| n.payload["context"]["held_by_kind"].as_object())
                .flat_map(|kinds| kinds.values().filter_map(|v| v.as_array()).flatten()
                    .filter_map(|m| m["task_id"].as_str().map(str::to_string)).collect::<Vec<_>>()))
            .collect();
        let window = found.first().zip(found.last()).map(|(a, b)| (a.at, b.at));
        let mut evidence = vec![receipt.id.clone()];
        evidence.extend(found.iter().map(|n| format!("notice #{}", n.id)));
        evidence.extend(tasks.iter().cloned());
        scorer.grade(index, receipt.id.clone(),
            format!("{} hold notice(s) over {} held task(s); {} member(s)", found.len(), tasks.len(), receipt.task_heads.len()),
            window, evidence);
    }
    scorer
}

const Q_HELD: &str = "Was each green release train with its verdict recorded cut promptly?";

/// Minutes outside `busy` in `[from, to)`, as segments of at least `min` minutes.
fn free_segments(from: DateTime<Utc>, to: DateTime<Utc>, busy: &[(DateTime<Utc>, DateTime<Utc>)], min: i64)
    -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let mut busy: Vec<_> = busy.iter().copied().filter(|(a, b)| *b > from && *a < to).collect();
    busy.sort();
    let mut out = Vec::new();
    let mut cursor = from;
    for (a, b) in busy.into_iter().chain(std::iter::once((to, to))) {
        if a > cursor && (a.min(to) - cursor).num_minutes() >= min { out.push((cursor, a.min(to))); }
        cursor = cursor.max(b);
        if cursor >= to { break }
    }
    out
}

/// In-progress spans of these tasks, from task events: a start ends at the
/// next block, completion or deletion.
fn work_spans(db: &rusqlite::Connection, tasks: &[String], now: DateTime<Utc>)
    -> rusqlite::Result<Vec<(String, DateTime<Utc>, DateTime<Utc>)>> {
    let mut out = Vec::new();
    for (task, events) in task_events(db, tasks)? {
        let mut started = None;
        for (at, kind) in events {
            match kind.as_str() {
                "task_started" => { started.get_or_insert(at); }
                "task_blocked" | "task_completed" | "task_deleted" => {
                    if let Some(start) = started.take() { out.push((task.clone(), start, at)); }
                }
                _ => {}
            }
        }
        if let Some(start) = started { out.push((task.clone(), start, now)); }
    }
    Ok(out)
}

fn task_events(db: &rusqlite::Connection, tasks: &[String])
    -> rusqlite::Result<BTreeMap<String, Vec<(DateTime<Utc>, String)>>> {
    let mut stmt = db.prepare("SELECT created_at, event_type FROM events WHERE entity_type = 'task' AND entity_id = ?1
        AND event_type IN ('task_created', 'task_started', 'task_blocked', 'task_completed', 'task_deleted') ORDER BY created_at, id")?;
    let mut out = BTreeMap::new();
    for task in tasks {
        let rows = stmt.query_map([task], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut events = Vec::new();
        for row in rows { let (at, kind) = row?; if let Some(at) = sqlite_time(&at) { events.push((at, kind)); } }
        out.insert(task.clone(), events);
    }
    Ok(out)
}

fn first_sha(key: &str) -> Option<&str> {
    key.split(':').find(|part| part.len() == 40 && part.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A train is held ready from the later of its green cut-ready notice and its
/// release verdict at the same tip, until it is cut or its tip moves. Time a
/// task aimed at the train is in progress is repair, not waiting; gaps under
/// 15 minutes between repairs are handoffs.
fn release_held(db: &rusqlite::Connection, repo: &Path, receipts: &[cas_store::QaBatchReceipt], now: DateTime<Utc>) -> Scorer {
    let mut scorer = Scorer::new("release_held_ready", Q_HELD, RELEASE_HELD);
    let ready = match notices(db, &["release_cut_ready"]) {
        Ok(ready) => ready,
        Err(e) => return unavailable(scorer, &e.to_string()),
    };
    for receipt in receipts {
        let Some((train, green)) = ready.iter().filter_map(|n| {
            let key = n.key.as_deref()?.strip_prefix("supervisor-notice:release_cut_ready:")?;
            (first_sha(key) == Some(receipt.staging_sha.as_str())).then(|| (key.split(':').next().unwrap_or_default().to_string(), n))
        }).min_by_key(|(_, n)| n.at) else { continue };
        let Some(verdict) = sqlite_time(&receipt.recorded_at) else { continue };
        let from = verdict.max(green.at);
        let train_tasks: Vec<String> = db.prepare("SELECT id FROM tasks WHERE
            json_extract(deliverables, '$.work_target.target_branch') LIKE 'epic/%-' || ?1")
            .and_then(|mut s| s.query_map([&train], |r| r.get(0))?.collect()).unwrap_or_default();
        let cut = crate::issue_reconcile::release_tag_containing(repo, &receipt.staging_sha)
            .and_then(|tag| super::tag_time(repo, &tag).map(|at| (tag, at)));
        let moved = train_merge_after(db, &train_tasks, from);
        let (to, end) = match (cut.as_ref(), moved.as_ref()) {
            (Some((_, at)), Some((task, m))) if m < at => (*m, format!("tip moved by {task}'s merge")),
            (Some((tag, at)), _) => (*at, format!("cut as {tag}")),
            (None, Some((task, m))) => (*m, format!("tip moved by {task}'s merge")),
            (None, None) => (now, "still uncut".into()),
        };
        if to <= from { continue }
        let spans = work_spans(db, &train_tasks, now).unwrap_or_default();
        let busy: Vec<_> = spans.iter().map(|(_, a, b)| (*a, *b)).collect();
        let segments = free_segments(from, to, &busy, 15);
        let minutes: i64 = segments.iter().map(|(a, b)| (*b - *a).num_minutes()).sum();
        let index = match minutes { ..=15 => 0, 16..=60 => 1, 61..=240 => 2, _ => 3 };
        let mut evidence = vec![format!("cut-ready notice #{}", green.id), receipt.id.clone()];
        evidence.extend(spans.iter().filter(|(_, a, b)| *b > from && *a < to)
            .map(|(task, a, b)| format!("{task} in progress {}–{}", super::when(*a), super::when(*b))));
        evidence.push(end);
        let window = segments.first().zip(segments.last()).map(|((a, _), (_, b))| (*a, *b));
        scorer.grade(index, format!("{train} at {}", &receipt.staging_sha[..12.min(receipt.staging_sha.len())]),
            format!("{minutes} min held over {} span(s)", segments.len()), window, evidence);
    }
    scorer
}

fn train_merge_after(db: &rusqlite::Connection, tasks: &[String], after: DateTime<Utc>) -> Option<(String, DateTime<Utc>)> {
    let mut stmt = db.prepare("SELECT task_id, occurred_at FROM quality_events WHERE kind = 'check_run'
        AND json_extract(payload, '$.check_id') = 'worktree_merge' AND json_extract(payload, '$.outcome') = 'pass'
        AND task_id IN (SELECT value FROM json_each(?1)) ORDER BY occurred_at").ok()?;
    let tasks = serde_json::to_string(tasks).ok()?;
    stmt.query_map([tasks], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).ok()?
        .filter_map(Result::ok)
        .filter_map(|(task, at)| sqlite_time(&at).map(|at| (task, at)))
        .find(|(_, at)| *at > after)
}

const Q_IDLE: &str = "Did workers sit held or idle while an epic they work had ready, unblocked tasks?";

/// Factory-session records: holds, task starts and shutdowns per worker.
#[derive(Default)]
struct WorkerLog { holds: Vec<(String, String, DateTime<Utc>, bool)>, starts: Vec<(String, DateTime<Utc>)>, stops: Vec<(String, DateTime<Utc>)> }

fn worker_log(cas_root: &Path) -> WorkerLog {
    let mut log = WorkerLog::default();
    let Ok(dir) = std::fs::read_dir(cas_root.join("logs")) else { return log };
    let mut files: Vec<_> = dir.filter_map(Result::ok).map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("factory-session-") && n.ends_with(".log")))
        .collect();
    files.sort();
    for line in files.iter().filter_map(|p| std::fs::read_to_string(p).ok()).flat_map(|t| t.lines().map(str::to_string).collect::<Vec<_>>()) {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        let Some(at) = record["timestamp"].as_str().and_then(sqlite_time) else { continue };
        let text = |k: &str| record[k].as_str().unwrap_or_default().to_string();
        match record["event"].as_str() {
            Some(e @ ("worker_hold_armed" | "worker_hold_released")) =>
                log.holds.push((text("factory_session"), text("worker"), at, e == "worker_hold_armed")),
            Some("task_started") => log.starts.push((text("assignee"), at)),
            Some("workers_shutdown_queued") => for w in text("workers").split(',') { log.stops.push((w.trim().to_string(), at)); },
            _ => {}
        }
    }
    log
}

fn idle_with_ready_work(db: &rusqlite::Connection, cas_root: &Path, from: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Scorer {
    let mut scorer = Scorer::new("idle_with_ready_work", Q_IDLE, IDLE_READY);
    let log = worker_log(cas_root);
    // (session, worker) -> waiting spans with their source record.
    let mut waits = BTreeMap::<(String, String), Vec<(DateTime<Utc>, DateTime<Utc>, String)>>::new();
    let mut armed = BTreeMap::<(String, String), DateTime<Utc>>::new();
    for (session, worker, at, hold) in &log.holds {
        let key = (session.clone(), worker.clone());
        if *hold { armed.entry(key).or_insert(*at); }
        else if let Some(start) = armed.remove(&key) {
            waits.entry(key).or_default().push((start, *at, format!("hold {}–{}", super::when(start), super::when(*at))));
        }
    }
    for (key, start) in armed {
        waits.entry(key).or_default().push((start, now, format!("hold since {}", super::when(start))));
    }
    match notices(db, &["worker_idle"]) {
        Ok(idle) => for n in idle {
            let (Some(worker), Some(session)) = (n.payload["worker"].as_str(), n.payload["factory_session"].as_str()) else { continue };
            let next = log.starts.iter().chain(&log.stops).filter(|(w, at)| w == worker && *at > n.at).map(|(_, at)| *at)
                .chain(log.holds.iter().filter(|(s, w, at, _)| s == session && w == worker && *at > n.at).map(|(_, _, at, _)| *at))
                .min();
            let Some(end) = next else {
                scorer.unscored.push(format!("{worker}: idle notice #{} has no recorded end", n.id));
                continue;
            };
            waits.entry((session.to_string(), worker.to_string())).or_default()
                .push((n.at, end, format!("idle notice #{} until {}", n.id, super::when(end))));
        },
        Err(e) => scorer.unscored.push(format!("idle notices unreadable: {e}")),
    }
    for ((session, worker), spans) in waits {
        let spans: Vec<_> = spans.into_iter().filter(|(_, b, _)| from.is_none_or(|f| *b > f) ).filter(|(a, _, _)| *a < now).collect();
        if spans.is_empty() { continue }
        let epics: Vec<String> = db.prepare("SELECT DISTINCT w.epic_id FROM working_epics w JOIN agents a ON a.id = w.agent_id
            WHERE a.name = ?1 AND a.factory_session = ?2").and_then(|mut s| s.query_map([&worker, &session], |r| r.get(0))?.collect())
            .unwrap_or_default();
        let ready = ready_spans(db, &epics, now).unwrap_or_default();
        for (start, end, source) in spans {
            let (start, end) = (from.map_or(start, |f| start.max(f)), end.min(now));
            let overlapping: Vec<_> = ready.iter().filter(|(_, _, a, b)| *a < end && *b > start).collect();
            if overlapping.is_empty() { continue }
            let ready_now: Vec<_> = overlapping.iter().map(|(_, _, a, b)| (*a, *b)).collect();
            // The wait's spans covered by at least one ready task.
            let covered = free_segments(start, end, &free_segments(start, end, &ready_now, 0), 0);
            let minutes: i64 = covered.iter().map(|(a, b)| (*b - *a).num_minutes()).sum();
            if covered.is_empty() { continue }
            let index = match minutes { ..=15 => 0, 16..=30 => 1, 31..=120 => 2, _ => 3 };
            let tasks: BTreeSet<_> = overlapping.iter().map(|(task, epic, _, _)| format!("{task} ready in {epic}")).collect();
            let mut evidence = vec![source];
            evidence.extend(tasks.iter().take(5).cloned());
            if tasks.len() > 5 { evidence.push(format!("{} more ready task(s)", tasks.len() - 5)); }
            let window = covered.first().zip(covered.last()).map(|((a, _), (_, b))| (*a, *b));
            scorer.grade(index, worker.clone(), format!("{minutes} min with {} ready task(s)", tasks.len()), window, evidence);
        }
    }
    scorer
}

/// Spans each child of these epics was open with every blocker closed: a
/// create or reopen starts one; a start, block, completion or deletion ends it.
fn ready_spans(db: &rusqlite::Connection, epics: &[String], now: DateTime<Utc>)
    -> rusqlite::Result<Vec<(String, String, DateTime<Utc>, DateTime<Utc>)>> {
    let mut out = Vec::new();
    let mut children = db.prepare("SELECT t.id, t.status FROM dependencies d JOIN tasks t ON t.id = d.from_id
        WHERE d.to_id = ?1 AND d.dep_type = 'parent-child' AND t.task_type != 'epic'")?;
    let mut blockers = db.prepare("SELECT b.closed_at FROM dependencies d JOIN tasks b ON b.id = d.to_id
        WHERE d.from_id = ?1 AND d.dep_type = 'blocks'")?;
    for epic in epics {
        let kids: Vec<(String, String)> = children.query_map([epic], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        let ids: Vec<String> = kids.iter().map(|(id, _)| id.clone()).collect();
        let events = task_events(db, &ids)?;
        for (task, status) in kids {
            let closed: Vec<Option<String>> = blockers.query_map([&task], |r| r.get(0))?.collect::<Result<_, _>>()?;
            if closed.iter().any(Option::is_none) { continue }
            let unblocked = closed.iter().flatten().filter_map(|t| sqlite_time(t)).max();
            let mut spans = Vec::new();
            let mut open = None;
            for (at, kind) in events.get(&task).into_iter().flatten() {
                match kind.as_str() {
                    "task_created" => { open.get_or_insert(*at); }
                    _ => if let Some(start) = open.take() { spans.push((start, *at)); }
                }
            }
            if let Some(start) = open.filter(|_| status == "open") { spans.push((start, now)); }
            out.extend(spans.into_iter()
                .map(|(a, b)| (unblocked.map_or(a, |u| a.max(u)), b))
                .filter(|(a, b)| a < b)
                .map(|(a, b)| (task.clone(), epic.clone(), a, b)));
        }
    }
    Ok(out)
}

pub(super) fn render(out: &mut String, scorers: &[Scorer]) {
    use std::fmt::Write;
    let _ = writeln!(out, "\nScorers (pass line between 0.8 and 0.4; hits score 0.4 or below)");
    for scorer in scorers {
        let _ = writeln!(out, "  {}: {}", scorer.name, scorer.question);
        let labels = scorer.labels.iter().map(|l| format!("{} {}", l.value, l.score)).collect::<Vec<_>>().join(", ");
        let _ = writeln!(out, "    labels: {labels}");
        if let Some(reason) = &scorer.unavailable {
            let _ = writeln!(out, "    not measured: {reason}");
            continue;
        }
        let tally = scorer.tally.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ");
        let _ = writeln!(out, "    {} scored, {} hit(s){}", scorer.scored, scorer.hits.len(),
            if tally.is_empty() { String::new() } else { format!(" ({tally})") });
        for hit in &scorer.hits {
            let window = hit.window.map_or_else(String::new, |(a, b)| format!(", {}–{}", super::when(a), super::when(b)));
            let _ = writeln!(out, "    hit {}: {} ({}){window}, {}", hit.subject, hit.label, hit.score, hit.measure);
            let _ = writeln!(out, "      evidence: {}", hit.evidence.join("; "));
        }
        for note in &scorer.unscored {
            let _ = writeln!(out, "    unscored {note}");
        }
    }
}

#[cfg(test)]
mod tests {
    //! Each test replays a Nov 11-12 incident through the report's real path.
    use super::*;
    use cas_types::{DependencyType, Task, TaskType};

    fn at(text: &str) -> DateTime<Utc> { DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc) }

    fn store() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        use cas_store::SupervisorQueueStore;
        cas_store::SqliteSupervisorQueueStore::open(&root).unwrap().init().unwrap();
        crate::store::open_agent_store(&root).unwrap();
        let db = rusqlite::Connection::open(root.join("cas.db")).unwrap();
        for statement in cas_store::QA_BATCH_SCHEMA_STATEMENTS { db.execute_batch(statement).unwrap(); }
        (dir, root)
    }

    fn sql(root: &Path, statement: &str, params: &[&dyn rusqlite::ToSql]) {
        rusqlite::Connection::open(root.join("cas.db")).unwrap().execute(statement, params).unwrap();
    }

    fn task(root: &Path, id: &str, kind: TaskType, note: Option<&str>, train: Option<&str>, epic: Option<&str>) {
        let mut task = Task::new(id.into(), id.into());
        task.task_type = kind;
        task.execution_note = note.map(str::to_string);
        task.deliverables.work_target = train.map(|branch| cas_types::WorkTarget {
            repo_selector: "project:cas-src".into(), target_branch: branch.into() });
        let store = crate::store::open_task_store(root).unwrap();
        store.add(&task).unwrap();
        if let Some(epic) = epic {
            store.add_dependency(&cas_types::Dependency::new(id.into(), epic.into(), DependencyType::ParentChild)).unwrap();
        }
    }

    fn event(root: &Path, task: &str, kind: &str, when: &str) {
        sql(root, "INSERT INTO events (event_type, entity_type, entity_id, summary, created_at) VALUES (?1, 'task', ?2, ?1, ?3)",
            &[&kind, &task, &when]);
    }

    fn notice(root: &Path, id: i64, when: &str, kind: &str, key: &str, payload: serde_json::Value) {
        let mut payload = payload;
        payload["kind"] = kind.into();
        let event_type = if kind == "worker_idle" { kind } else { "supervisor_notice" };
        sql(root, "INSERT INTO supervisor_queue (id, supervisor_id, event_type, payload, created_at, transition_key) VALUES (?1, 'sup', ?2, ?3, ?4, ?5)",
            &[&id, &event_type, &payload.to_string(), &when, &key]);
    }

    fn receipt(root: &Path, id: &str, sha: &str, when: &str, members: &[&str]) {
        let project = crate::cloud::resolve_canonical_id(root).unwrap_or_default();
        let heads: BTreeMap<_, _> = members.iter().map(|m| (m.to_string(), sha.to_string())).collect();
        let json = serde_json::json!({"schema_version": 2, "id": id, "project_id": project, "ledger_artifact_id": "art-1",
            "ledger_path": "ledger.md", "ledger_sha256": "0", "ledger_size_bytes": 1, "staging_sha": sha,
            "task_heads": heads, "recorder_agent_id": "sup", "recorded_at": when});
        sql(root, "INSERT INTO qa_batch_receipts (id, project_id, receipt_json, recorded_at) VALUES (?1, ?2, ?3, ?4)",
            &[&id, &project, &json.to_string(), &when]);
    }

    fn review(root: &Path, task: &str, id: &str, outcome: &str, provenance: &str) {
        let payload = serde_json::from_value(serde_json::json!({"kind": "check_run", "check_id": "task_verifier",
            "stage": "verifier", "outcome": outcome,
            "review": {"verification_id": id, "cycle_id": format!("dc-{task}"), "provenance": provenance}})).unwrap();
        let mut event = cas_store::NewQualityEvent::new(id, payload);
        event.task_id = Some(task.into());
        cas_store::record_quality_event(root, &event).unwrap();
    }

    /// The report as `cas ledger report` builds it for work since v4.18.0.
    fn scored(root: &Path, name: &str) -> Scorer {
        let events = cas_store::quality_events_between(root, None, None).unwrap();
        let mut report = super::super::aggregate("unreleased".into(), false,
            Some(at("2026-11-12T00:42:12Z")), Some(at("2026-11-12T13:00:00Z")), &events);
        report.scorers = score(root, root.parent().unwrap(), &report);
        report.scorers.into_iter().find(|s| s.name == name).unwrap()
    }

    #[test]
    fn review_rounds_flags_a_delivery_above_two_rounds() {
        let (_dir, root) = store();
        for (id, outcome) in [("ver-1", "fail"), ("ver-2", "fail"), ("ver-3", "pass")] { review(&root, "cas-b5cd", id, outcome, "independent"); }
        for (id, outcome) in [("ver-4", "fail"), ("ver-5", "pass")] { review(&root, "cas-ok", id, outcome, "independent"); }
        let scorer = scored(&root, "review_rounds");
        assert_eq!(scorer.scored, 2);
        assert_eq!(scorer.hits.iter().map(|h| (h.subject.as_str(), h.label, h.evidence.clone())).collect::<Vec<_>>(),
            vec![("cas-b5cd dc-cas-b5cd", "repeated_returns", vec!["ver-1".into(), "ver-2".into(), "ver-3".into()])]);
    }

    #[test]
    fn supervisor_alone_flags_decision_verdicts_without_an_independent_reviewer() {
        let (_dir, root) = store();
        task(&root, "cas-3500", TaskType::Spike, Some("no-code"), None, None);
        task(&root, "cas-b021", TaskType::Spike, None, None, None);
        task(&root, "cas-c6ff", TaskType::Task, Some("no-code"), None, None);
        task(&root, "cas-821e", TaskType::Bug, None, None, None);
        review(&root, "cas-3500", "ver-a", "fail", "supervisor_direct");
        review(&root, "cas-3500", "ver-b", "pass", "supervisor_direct");
        review(&root, "cas-b021", "ver-c", "pass", "supervisor_direct");
        review(&root, "cas-c6ff", "ver-d", "pass", "independent");
        review(&root, "cas-821e", "ver-e", "pass", "supervisor_direct");
        let scorer = scored(&root, "supervisor_alone");
        assert_eq!(scorer.scored, 3);
        assert_eq!(scorer.hits.iter().map(|h| (h.subject.as_str(), h.label)).collect::<Vec<_>>(),
            vec![("cas-3500 dc-cas-3500", "supervisor_alone_repeated"), ("cas-b021 dc-cas-b021", "supervisor_alone")]);
        assert!(scorer.hits[1].evidence.iter().any(|e| e.starts_with("ver-c")));
    }

    #[test]
    fn hold_notices_flag_a_per_task_storm_on_one_receipt() {
        let (_dir, root) = store();
        receipt(&root, "qab-ee7c", "8c0dbddce6007e9a74f4573dfb6c310ec1762fb8", "2026-11-12T01:00:00Z", &["cas-253c", "cas-2b13b", "cas-3dd7"]);
        receipt(&root, "qab-7840", "c249a21b5c629f16e7ebf75e15f7463d726949a9", "2026-11-12T05:01:21Z", &["cas-2b"]);
        for (id, member) in [(102742, "cas-253c"), (102743, "cas-2b13b"), (102747, "cas-3dd7")] {
            notice(&root, id, "2026-11-12T01:01:00Z", "batch_close_hold",
                &format!("supervisor-notice:batch_close_hold:{member}:sha:sup"),
                serde_json::json!({"detail": format!("Receipt qab-ee7c covers {member} at sha. Ordinary finalization is held")}));
        }
        notice(&root, 103201, "2026-11-12T05:03:16Z", "batch_close_hold", "supervisor-notice:batch_close_hold:qab-7840",
            serde_json::json!({"detail": "Receipt qab-7840 at c249: 0 closed, 0 already closed, 1 held.",
                "context": {"receipt_id": "qab-7840", "held_by_kind": {"TASK CLOSE REJECTED": [{"task_id": "cas-2b"}]}}}));
        let scorer = scored(&root, "hold_notices_per_receipt");
        assert_eq!(scorer.scored, 2);
        assert_eq!(scorer.tally.get("one_notice"), Some(&1));
        let [hit] = scorer.hits.as_slice() else { panic!("{:?}", scorer.hits) };
        assert_eq!((hit.subject.as_str(), hit.label), ("qab-ee7c", "notice_burst"));
        for cited in ["notice #102742", "notice #102747", "cas-2b13b"] { assert!(hit.evidence.iter().any(|e| e == cited), "{cited}"); }
    }

    /// v4.19.0, Nov 12: green and verdict-recorded at c249a21b just after
    /// midnight ET, repair stopped at 12:35 AM, and the train sat until a
    /// repair merge moved its tip at 7:30 AM.
    #[test]
    fn release_held_ready_reproduces_the_overnight_v4_19_window() {
        let (_dir, root) = store();
        let train = "epic/release-train-vnext-cas-2fdd";
        task(&root, "cas-2fdd", TaskType::Epic, None, None, None);
        for id in ["cas-828e", "cas-dc95d", "cas-08c8"] { task(&root, id, TaskType::Bug, None, Some(train), None); }
        let sha = "c249a21b5c629f16e7ebf75e15f7463d726949a9";
        notice(&root, 103176, "2026-11-12T04:47:56Z", "release_cut_ready",
            &format!("supervisor-notice:release_cut_ready:cas-2fdd:{sha}:sup"), serde_json::json!({}));
        receipt(&root, "qab-7840", sha, "2026-11-12T05:01:21Z", &["cas-e41bb"]);
        for (id, kind, when) in [("cas-828e", "task_started", "2026-11-12T05:10:14Z"), ("cas-828e", "task_blocked", "2026-11-12T05:11:28Z"),
            ("cas-828e", "task_started", "2026-11-12T05:11:49Z"), ("cas-828e", "task_blocked", "2026-11-12T05:30:49Z"),
            ("cas-dc95d", "task_started", "2026-11-12T05:32:23Z"), ("cas-dc95d", "task_blocked", "2026-11-12T05:35:09Z"),
            ("cas-08c8", "task_started", "2026-11-12T11:37:41Z"), ("cas-08c8", "task_blocked", "2026-11-12T11:45:36Z")] {
            event(&root, id, kind, when);
        }
        let mut merged = cas_store::NewQualityEvent::new("merge-08c8", cas_store::QualityPayload::CheckRun { review: None,
            check_id: "worktree_merge".into(), stage: cas_store::CheckStage::Lane, outcome: CheckOutcome::Pass,
            sha: Some("5dbeb6b11235259b01d44dd70d6c81dbd53181a6".into()), minutes: None });
        merged.task_id = Some("cas-08c8".into());
        merged.occurred_at = at("2026-11-12T12:30:31Z");
        cas_store::record_quality_event(&root, &merged).unwrap();
        let scorer = scored(&root, "release_held_ready");
        let [hit] = scorer.hits.as_slice() else { panic!("{:?}", scorer.hits) };
        assert_eq!((hit.subject.as_str(), hit.label), ("cas-2fdd at c249a21b5c62", "held_long"));
        assert_eq!(hit.window, Some((at("2026-11-12T05:35:09Z"), at("2026-11-12T12:30:31Z"))));
        assert!(hit.measure.starts_with("406 min"), "{}", hit.measure);
        assert!(hit.evidence.iter().any(|e| e == "qab-7840") && hit.evidence.iter().any(|e| e == "cut-ready notice #103176"));
    }

    /// Nov 12, 5:15-6:37 AM ET: fast-cobra-16 was held while its other epic
    /// had a ready task; quiet-crow-7's only epic had none.
    #[test]
    fn idle_with_ready_work_reproduces_the_morning_hold_window() {
        let (_dir, root) = store();
        std::fs::create_dir_all(root.join("logs")).unwrap();
        let log = [("quiet-crow-7", true, "2026-11-12T10:15:21Z"), ("fast-cobra-16", true, "2026-11-12T10:15:30Z"),
            ("fast-cobra-16", false, "2026-11-12T11:37:27Z"), ("quiet-crow-7", false, "2026-11-12T12:42:46Z")]
            .map(|(worker, hold, when)| serde_json::json!({"event": if hold { "worker_hold_armed" } else { "worker_hold_released" },
                "factory_session": "zen-hawk-76", "worker": worker, "timestamp": when}).to_string()).join("\n");
        std::fs::write(root.join("logs/factory-session-2026-11-12.log"), log).unwrap();
        for (id, name, epic) in [("agent-fc", "fast-cobra-16", "cas-41bd"), ("agent-qc", "quiet-crow-7", "cas-cf795")] {
            sql(&root, "INSERT INTO agents (id, name, registered_at, last_heartbeat, factory_session) VALUES (?1, ?2, ?3, ?3, 'zen-hawk-76')",
                &[&id, &name, &"2026-11-12T08:00:00Z"]);
            sql(&root, "INSERT INTO working_epics (agent_id, epic_id, started_at) VALUES (?1, ?2, '2026-11-12T08:00:00Z')", &[&id, &epic]);
        }
        for epic in ["cas-41bd", "cas-cf795"] { task(&root, epic, TaskType::Epic, None, None, None); }
        task(&root, "cas-74d3", TaskType::Bug, None, None, Some("cas-41bd"));
        event(&root, "cas-74d3", "task_created", "2026-11-12T08:12:25Z");
        task(&root, "cas-03bc", TaskType::Task, None, None, Some("cas-cf795"));
        for (kind, when) in [("task_created", "2026-11-12T06:22:31Z"), ("task_started", "2026-11-12T06:22:37Z"), ("task_blocked", "2026-11-12T09:27:00Z")] {
            event(&root, "cas-03bc", kind, when);
        }
        let scorer = scored(&root, "idle_with_ready_work");
        let [hit] = scorer.hits.as_slice() else { panic!("{:?}", scorer.hits) };
        assert_eq!((hit.subject.as_str(), hit.label), ("fast-cobra-16", "idle"));
        assert_eq!(hit.window, Some((at("2026-11-12T10:15:30Z"), at("2026-11-12T11:37:27Z"))));
        assert!(hit.evidence.iter().any(|e| e == "cas-74d3 ready in cas-41bd"));
    }
}
