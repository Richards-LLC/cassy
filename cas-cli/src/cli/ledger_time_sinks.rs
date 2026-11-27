use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::Path;

use anyhow::{Context, Result};
use cas_store::{AgentStore, CheckStage, QualityEvent, QualityPayload};
use cas_types::AgentRole;
use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use super::{Measure, median, parse_time, sensors, unknown};

type Time = DateTime<Utc>;
type Span = (Time, Time);

#[derive(Serialize, Default)]
struct Distribution {
    samples: usize,
    total: f64,
    median: Option<f64>,
    p90: Option<f64>,
}
impl Distribution {
    fn from(mut values: Vec<f64>) -> Self {
        values.sort_by(f64::total_cmp);
        Self {
            samples: values.len(),
            total: values.iter().sum(),
            median: median(values.clone()),
            p90: (!values.is_empty()).then(|| values[(values.len() * 9).div_ceil(10) - 1]),
        }
    }
}

#[derive(Serialize, Default)]
struct Tokens {
    turns: usize,
    input: u64,
    output: u64,
    missing: usize,
}
impl Tokens {
    fn add(&mut self, event: &QualityEvent) {
        if let QualityPayload::Turn {
            input_tokens,
            output_tokens,
            ..
        } = &event.payload
        {
            self.turns += 1;
            self.input += input_tokens.unwrap_or(0);
            self.output += output_tokens.unwrap_or(0);
            self.missing += usize::from(input_tokens.is_none() || output_tokens.is_none());
        }
    }
}

#[derive(Serialize)]
struct Phase {
    task_id: String,
    phase: &'static str,
    started_at: Time,
    ended_at: Time,
    window_minutes: f64,
    complete: bool,
    tokens: Tokens,
}
#[derive(Serialize)]
struct Waterfall {
    tasks: Vec<Phase>,
    completed_minutes: BTreeMap<String, Distribution>,
    censored_minutes: BTreeMap<String, f64>,
    unattributed_turns: usize,
    tokens_by_phase: BTreeMap<String, Tokens>,
    coverage: &'static str,
}

#[derive(Serialize, Default)]
struct WorkerMinutes {
    observed_alive: f64,
    in_turn: f64,
    leased_out_of_turn: f64,
    blocked_or_waiting: f64,
    unassigned: f64,
}
#[derive(Serialize)]
struct Workers {
    by_worker: BTreeMap<String, WorkerMinutes>,
    fleet: WorkerMinutes,
    missing_turn_duration: usize,
    open_lease_intervals: usize,
    coverage: &'static str,
}

#[derive(Serialize, Default)]
struct Supervisor {
    turns: usize,
    no_op: usize,
    no_op_rate: Option<f64>,
    no_op_minutes: f64,
    by_wake_reason: BTreeMap<String, usize>,
    unknown_actor_turns: usize,
}

#[derive(Serialize)]
struct Recovery {
    calls: usize,
    recovered: usize,
    unresolved: usize,
    minutes: Distribution,
    completed_turn_tokens: Tokens,
    boundary_turns: usize,
    missing_context: usize,
}
#[derive(Serialize)]
struct Reviews {
    rounds_by_task: BTreeMap<String, usize>,
    rounds: Distribution,
    minutes: Distribution,
    missing_duration: usize,
}
#[derive(Serialize)]
pub(super) struct Ci {
    jobs: usize,
    captured_at: Time,
    covered_window_minutes: f64,
    uncovered_window_minutes: f64,
    queue_minutes: Distribution,
    run_minutes: Distribution,
    missing_queue: usize,
    missing_run: usize,
    missing_creation_time: usize,
}
#[derive(Serialize)]
struct Peers {
    cloud_created_to_admitted_minutes: Distribution,
    by_machine: BTreeMap<String, Distribution>,
    invalid_timestamps: usize,
}
#[derive(Serialize)]
struct Shipped {
    shipped_fixes: usize,
    release_membership_known: bool,
    fixes_with_turns: usize,
    tokens: Tokens,
    input_per_fix: Option<f64>,
    output_per_fix: Option<f64>,
    coverage: &'static str,
}
#[derive(Serialize)]
struct Sink {
    category: String,
    minutes: f64,
    basis: &'static str,
}
#[derive(Serialize)]
pub(super) struct TimeSinks {
    phase_waterfall: Measure<Waterfall>,
    worker_minutes: Measure<Workers>,
    refusal_recovery_by_code: Measure<BTreeMap<String, Recovery>>,
    supervisor: Measure<Supervisor>,
    ci: Measure<Ci>,
    reviews: Reviews,
    peer_latency: Measure<Peers>,
    tokens_per_shipped_fix: Measure<Shipped>,
    top_five: Vec<Sink>,
    ranking_scope: &'static str,
}

fn measured<T>(result: Result<T>) -> Measure<T> {
    result.map_or_else(|error| unknown(error.to_string()), Measure::Known)
}
fn minutes((start, end): Span) -> f64 {
    (end - start).as_seconds_f64() / 60.0
}
fn clipped((start, end): Span, (since, until): Span) -> Option<Span> {
    let span = (start.max(since), end.min(until));
    (span.0 < span.1).then_some(span)
}
fn turn_span(event: &QualityEvent) -> Option<Span> {
    let QualityPayload::Turn {
        minutes: Some(n), ..
    } = event.payload
    else {
        return None;
    };
    if !n.is_finite() || !(0.0..=525_600.0).contains(&n) {
        return None;
    }
    Some((
        event.occurred_at - Duration::milliseconds((n * 60_000.0).round() as i64),
        event.occurred_at,
    ))
}
fn effective(events: Vec<QualityEvent>) -> Vec<QualityEvent> {
    let replaced: BTreeSet<_> = events
        .iter()
        .filter_map(|e| match &e.payload {
            QualityPayload::Turn { supersedes, .. } => supersedes
                .as_ref()
                .map(|id| (e.project_id.clone(), id.clone())),
            QualityPayload::CheckRun {
                review: Some(r), ..
            } => r
                .supersedes
                .as_ref()
                .map(|id| (e.project_id.clone(), id.clone())),
            _ => None,
        })
        .collect();
    events
        .into_iter()
        .filter(|e| !replaced.contains(&(e.project_id.clone(), e.id.clone())))
        .collect()
}

pub(super) fn build(root: &Path, db: &Connection, since: Time, until: Time) -> Result<TimeSinks> {
    let canonical = crate::cloud::canonical_id_from_config_toml(root);
    let events = effective(
        cas_store::quality_events_between(root, None, Some(until))?
            .into_iter()
            .filter(|e| {
                e.occurred_at < until
                    && (!e.from_cloud || canonical.as_ref() == Some(&e.project_id))
            })
            .collect(),
    );
    let window: Vec<_> = events
        .iter()
        .filter(|e| e.occurred_at >= since)
        .cloned()
        .collect();
    let agents = cas_store::SqliteAgentStore::open(root)?.list(None)?;
    let mut supervisor = Supervisor::default();
    for event in &window {
        if let QualityPayload::Turn {
            no_op,
            notice_kind,
            woken_by,
            minutes,
            ..
        } = &event.payload
        {
            match agents
                .iter()
                .find(|a| Some(&a.id) == event.actor.as_ref())
                .map(|a| a.role)
            {
                Some(AgentRole::Supervisor) => {
                    supervisor.turns += 1;
                    supervisor.no_op += usize::from(*no_op);
                    if *no_op {
                        supervisor.no_op_minutes += minutes.unwrap_or(0.0);
                    }
                    *supervisor
                        .by_wake_reason
                        .entry(
                            notice_kind
                                .clone()
                                .unwrap_or_else(|| woken_by.as_str().into()),
                        )
                        .or_default() += 1;
                }
                None => supervisor.unknown_actor_turns += 1,
                _ => {}
            }
        }
    }
    supervisor.no_op_rate =
        (supervisor.turns > 0).then(|| supervisor.no_op as f64 / supervisor.turns as f64);
    let mut rounds = BTreeMap::<String, usize>::new();
    let mut review_times = vec![];
    let mut missing_duration = 0;
    for event in &window {
        if let QualityPayload::CheckRun {
            stage: CheckStage::Verifier | CheckStage::Qa,
            minutes,
            ..
        } = &event.payload
        {
            *rounds
                .entry(event.task_id.clone().unwrap_or_else(|| "unknown".into()))
                .or_default() += 1;
            if let Some(n) = minutes {
                review_times.push(*n);
            } else {
                missing_duration += 1;
            }
        }
    }
    let reviews = Reviews {
        rounds: Distribution::from(rounds.values().map(|n| *n as f64).collect()),
        rounds_by_task: rounds,
        minutes: Distribution::from(review_times),
        missing_duration,
    };
    let mut out = TimeSinks {
        phase_waterfall: measured(waterfall(db, &events, (since, until))),
        worker_minutes: measured(workers(root, db, &events, (since, until))),
        refusal_recovery_by_code: measured(recoveries(db, &events, (since, until))),
        supervisor: Measure::Known(supervisor),
        ci: measured(ci(root, (since, until))),
        reviews,
        peer_latency: measured(peers(db, (since, until))),
        tokens_per_shipped_fix: measured(shipped(db, &events, (since, until))),
        top_five: vec![],
        ranking_scope: "Recorded wall/agent/job minutes; categories overlap and are not additive. Missing sensors are excluded.",
    };
    let mut add = |category: String, minutes: f64, basis| {
        if minutes > 0.0 {
            out.top_five.push(Sink {
                category,
                minutes,
                basis,
            });
        }
    };
    if let Measure::Known(w) = &out.worker_minutes {
        add(
            "workers_unassigned".into(),
            w.fleet.unassigned,
            "observed worker minutes",
        );
        add(
            "workers_leased_out_of_turn".into(),
            w.fleet.leased_out_of_turn,
            "observed worker minutes",
        );
        add(
            "workers_blocked_or_waiting".into(),
            w.fleet.blocked_or_waiting,
            "observed worker minutes",
        );
    }
    if let Measure::Known(w) = &out.phase_waterfall {
        let total: f64 = w
            .tasks
            .iter()
            .filter(|p| p.phase == "park_to_train")
            .map(|p| p.window_minutes)
            .sum();
        add(
            "park_to_train".into(),
            total,
            "task wall minutes, including censored waits",
        );
    }
    if let Measure::Known(s) = &out.supervisor {
        add(
            "supervisor_no_op".into(),
            s.no_op_minutes,
            "completed supervisor turn minutes",
        );
    }
    if let Measure::Known(c) = &out.ci {
        add("ci_queue".into(), c.queue_minutes.total, "job minutes");
        add("ci_run".into(), c.run_minutes.total, "job minutes");
    }
    if let Measure::Known(rows) = &out.refusal_recovery_by_code {
        for (code, r) in rows {
            add(
                format!("refusal_{code}"),
                r.minutes.total,
                "per-call recovery minutes, may overlap",
            );
        }
    }
    add(
        "review".into(),
        out.reviews.minutes.total,
        "recorded review execution minutes",
    );
    if let Measure::Known(p) = &out.peer_latency {
        add(
            "peer_delivery".into(),
            p.cloud_created_to_admitted_minutes.total,
            "message minutes",
        );
    }
    out.top_five.sort_by(|a, b| {
        b.minutes
            .total_cmp(&a.minutes)
            .then(a.category.cmp(&b.category))
    });
    out.top_five.truncate(5);
    Ok(out)
}

fn observations(db: &Connection, kind: &str, window: Span) -> Result<Vec<(String, Time, Value)>> {
    let mut stmt = db.prepare(
        "SELECT observation_key,occurred_at,payload FROM efficiency_observations WHERE kind=?1
        AND julianday(occurred_at)>=julianday(?2) AND julianday(occurred_at)<julianday(?3)",
    )?;
    stmt.query_map(
        [kind, &window.0.to_rfc3339(), &window.1.to_rfc3339()],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        },
    )?
    .map(|row| {
        let (key, at, payload) = row?;
        Ok((
            key,
            parse_time(&at).context("invalid observation timestamp")?,
            serde_json::from_str(&payload)?,
        ))
    })
    .collect()
}
pub(super) fn issue_closures(db: &Connection, window: Span) -> Result<usize> {
    Ok(observations(db, "issue_closed", window)?
        .into_iter()
        .filter_map(|(_, _, v)| v["issue"].as_str().map(str::to_lowercase))
        .collect::<BTreeSet<_>>()
        .len())
}

fn peers(db: &Connection, window: Span) -> Result<Peers> {
    let mut by_machine = BTreeMap::<String, Vec<f64>>::new();
    let mut invalid = 0;
    for (_, admitted, payload) in observations(db, "peer_admitted", window)? {
        if let Some(created) = payload["created_at"]
            .as_str()
            .and_then(parse_time)
            .filter(|at| *at <= admitted)
        {
            by_machine
                .entry(payload["machine"].as_str().unwrap_or("unknown").into())
                .or_default()
                .push(minutes((created, admitted)));
        } else {
            invalid += 1;
        }
    }
    Ok(Peers {
        cloud_created_to_admitted_minutes: Distribution::from(
            by_machine.values().flatten().copied().collect(),
        ),
        by_machine: by_machine
            .into_iter()
            .map(|(m, v)| (m, Distribution::from(v)))
            .collect(),
        invalid_timestamps: invalid,
    })
}

pub(super) fn ci(root: &Path, window: Span) -> Result<Ci> {
    let capture = sensors::recorded_ci(root, window.0, window.1)?;
    let mut covered = capture.covered;
    covered.sort();
    let mut covered_window_minutes = 0.0;
    let mut end = window.0;
    for (from, to) in covered {
        if to > end {
            covered_window_minutes += minutes((from.max(end), to));
            end = to;
        }
    }
    let mut queue = vec![];
    let mut run = vec![];
    let mut count = 0;
    let mut missing_creation_time = 0;
    let (mut missing_queue, mut missing_run) = (0, 0);
    for job in capture.jobs {
        missing_creation_time += usize::from(job.run_created_at.is_none());
        if job
            .run_created_at
            .is_none_or(|at| at < window.0 || at >= window.1)
        {
            continue;
        }
        count += 1;
        match job.created_at.zip(job.started_at).filter(|(a, b)| b >= a) {
            Some(span) => queue.push(minutes(span)),
            None => missing_queue += 1,
        }
        match job.started_at.zip(job.completed_at).filter(|(a, b)| b >= a) {
            Some(span) => run.push(minutes(span)),
            None => missing_run += 1,
        }
    }
    Ok(Ci {
        jobs: count,
        captured_at: capture.captured_at,
        covered_window_minutes,
        uncovered_window_minutes: (minutes(window) - covered_window_minutes).max(0.0),
        queue_minutes: Distribution::from(queue),
        run_minutes: Distribution::from(run),
        missing_queue,
        missing_run,
        missing_creation_time,
    })
}

fn recoveries(
    db: &Connection,
    events: &[QualityEvent],
    window: Span,
) -> Result<BTreeMap<String, Recovery>> {
    let success: BTreeMap<_, _> = observations(db, "refusal_recovered", (window.0, window.1))?
        .into_iter()
        .map(|(id, at, _)| (id, at))
        .collect();
    let mut stmt = db.prepare("SELECT id,entity_id,created_at,metadata FROM events WHERE event_type='gate_refused'
        AND julianday(created_at)>=julianday(?1) AND julianday(created_at)<julianday(?2) ORDER BY created_at,id")?;
    let mut groups = BTreeMap::<String, Vec<(Time, Option<Time>, Value)>>::new();
    let mut seen = BTreeSet::new();
    for row in stmt.query_map([window.0.to_rfc3339(), window.1.to_rfc3339()], |r| {
        Ok((
            r.get::<_, i64>(0)?.to_string(),
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })? {
        let (id, code, at, payload) = row?;
        let metadata: Value = payload
            .and_then(|p| serde_json::from_str(&p).ok())
            .unwrap_or_default();
        if !seen.insert((
            code.clone(),
            metadata["fact_id"].as_str().unwrap_or(&id).to_string(),
        )) {
            continue;
        }
        groups.entry(code).or_default().push((
            parse_time(&at).context("invalid refusal timestamp")?,
            success.get(&id).copied(),
            metadata,
        ));
    }
    Ok(groups
        .into_iter()
        .map(|(code, rows)| {
            let mut elapsed = vec![];
            let mut tokens = Tokens::default();
            let mut counted_turns = BTreeSet::new();
            let (mut boundary_turns, mut missing_context) = (0, 0);
            for (start, end, metadata) in &rows {
                missing_context += usize::from(
                    ["actor", "tool", "action"]
                        .iter()
                        .any(|key| metadata[*key].as_str().is_none()),
                );
                let Some(end) = end else { continue };
                elapsed.push(minutes((*start, *end)));
                for event in events.iter().filter(|e| {
                    e.actor.as_deref() == metadata["actor"].as_str()
                        && e.actor.is_some()
                        && (metadata["task_id"].as_str().is_none()
                            || e.task_id.as_deref() == metadata["task_id"].as_str())
                }) {
                    if let Some(span) = turn_span(event)
                        && clipped(span, (*start, *end)).is_some()
                        && counted_turns.insert(event.id.clone())
                    {
                        if span.0 >= *start && span.1 <= *end {
                            tokens.add(event);
                        } else {
                            boundary_turns += 1;
                        }
                    }
                }
            }
            (
                code,
                Recovery {
                    calls: rows.len(),
                    recovered: elapsed.len(),
                    unresolved: rows.len() - elapsed.len(),
                    minutes: Distribution::from(elapsed),
                    completed_turn_tokens: tokens,
                    boundary_turns,
                    missing_context,
                },
            )
        })
        .collect())
}

fn workers(root: &Path, db: &Connection, events: &[QualityEvent], window: Span) -> Result<Workers> {
    let agents = cas_store::SqliteAgentStore::open(root)?.list(None)?;
    let mut stmt = db.prepare("SELECT task_id,agent_id,event_type,timestamp FROM task_lease_history ORDER BY timestamp,id")?;
    let mut leases = BTreeMap::<String, Vec<(String, Span)>>::new();
    let mut active = BTreeMap::<String, (String, Time)>::new();
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })? {
        let (task, agent, kind, at) = row?;
        let at = parse_time(&at).context("invalid lease timestamp")?;
        if at >= window.1 {
            continue;
        }
        if kind == "renewed" {
            continue;
        }
        if let Some((owner, start)) = active.remove(&task) {
            leases
                .entry(owner)
                .or_default()
                .push((task.clone(), (start, at)));
        }
        if kind == "claimed" || kind == "transferred" {
            active.insert(task, (agent, at));
        }
    }
    let open = active.len();
    for (task, (agent, start)) in active {
        leases
            .entry(agent)
            .or_default()
            .push((task, (start, window.1)));
    }
    let mut stmt = db.prepare("SELECT entity_id,event_type,created_at FROM events WHERE event_type IN ('task_started','task_blocked','task_completed','task_created') ORDER BY created_at,id")?;
    let mut states = BTreeMap::<String, Vec<(Time, bool)>>::new();
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (task, kind, at) = row?;
        states.entry(task).or_default().push((
            parse_time(&at).context("invalid state timestamp")?,
            kind == "task_blocked",
        ));
    }
    let mut out = Workers {
        by_worker: BTreeMap::new(),
        fleet: WorkerMinutes::default(),
        missing_turn_duration: 0,
        open_lease_intervals: open,
        coverage: "Alive = registration through last observed heartbeat; completed turns take precedence. TaskBlocked also covers park/cancel in legacy rows. Open leases are censored at window end. Leased silence is not proof of idle work.",
    };
    for agent in agents.into_iter().filter(|a| a.role == AgentRole::Worker) {
        let Some(alive) = clipped((agent.registered_at, agent.last_heartbeat), window) else {
            continue;
        };
        let held = leases.get(&agent.id).cloned().unwrap_or_default();
        let turns: Vec<_> = events
            .iter()
            .filter(|e| e.actor.as_ref() == Some(&agent.id))
            .filter_map(|e| {
                if matches!(e.payload, QualityPayload::Turn { .. }) && turn_span(e).is_none() {
                    out.missing_turn_duration += 1;
                }
                turn_span(e).and_then(|span| clipped(span, alive))
            })
            .collect();
        let mut boundaries = BTreeSet::from([alive.0, alive.1]);
        for span in turns
            .iter()
            .copied()
            .chain(held.iter().map(|(_, span)| *span))
        {
            if let Some((a, b)) = clipped(span, alive) {
                boundaries.extend([a, b]);
            }
        }
        for (task, _) in &held {
            for (at, _) in states
                .get(task)
                .into_iter()
                .flatten()
                .filter(|(at, _)| alive.0 < *at && *at < alive.1)
            {
                boundaries.insert(*at);
            }
        }
        let mut row = WorkerMinutes {
            observed_alive: minutes(alive),
            ..Default::default()
        };
        let boundaries: Vec<_> = boundaries.into_iter().collect();
        for pair in boundaries.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let n = minutes((a, b));
            let held: Vec<_> = held
                .iter()
                .filter(|(_, span)| span.0 <= a && b <= span.1)
                .collect();
            if turns.iter().any(|span| span.0 <= a && b <= span.1) {
                row.in_turn += n;
            } else if held.is_empty() {
                row.unassigned += n;
            } else if held.iter().any(|(task, _)| {
                states
                    .get(task)
                    .and_then(|s| s.iter().rev().find(|(at, _)| *at <= a))
                    .is_some_and(|(_, blocked)| *blocked)
            }) {
                row.blocked_or_waiting += n;
            } else {
                row.leased_out_of_turn += n;
            }
        }
        out.fleet.observed_alive += row.observed_alive;
        out.fleet.in_turn += row.in_turn;
        out.fleet.unassigned += row.unassigned;
        out.fleet.leased_out_of_turn += row.leased_out_of_turn;
        out.fleet.blocked_or_waiting += row.blocked_or_waiting;
        out.by_worker.insert(agent.id, row);
    }
    Ok(out)
}

struct Published {
    main: Option<Time>,
    published: Time,
}

fn release_members(
    db: &Connection,
    events: &[QualityEvent],
) -> Result<(BTreeMap<String, Published>, bool)> {
    let present: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='release_reconcile_operations')", [], |r| r.get(0))?;
    if !present {
        return Ok((BTreeMap::new(), false));
    }
    let mut members = BTreeMap::new();
    let mut stmt = db.prepare("SELECT receipt_json FROM release_reconcile_operations")?;
    for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
        let receipt: crate::release_reconcile::ReleaseReconcileReceipt =
            serde_json::from_str(&row?)?;
        let Some(event) = events
            .iter()
            .filter(|e| {
                matches!(e.payload, QualityPayload::Release { .. })
                    && e.release_version.as_ref() == Some(&receipt.scope.release_id)
            })
            .min_by_key(|e| e.occurred_at)
        else {
            continue;
        };
        let main = match event.payload {
            QualityPayload::Release {
                main_to_published_minutes: Some(n),
                ..
            } if n.is_finite() && (0.0..=525600.0).contains(&n) => {
                Some(event.occurred_at - Duration::milliseconds((n * 60000.0).round() as i64))
            }
            _ => None,
        };
        for task in receipt
            .scope
            .included_task_ids
            .iter()
            .filter(|id| !receipt.scope.no_code_members.contains_key(*id))
        {
            let p = Published {
                main,
                published: event.occurred_at,
            };
            members
                .entry(task.clone())
                .and_modify(|old: &mut Published| {
                    if p.published < old.published {
                        *old = Published {
                            main: p.main,
                            published: p.published,
                        };
                    }
                })
                .or_insert(p);
        }
    }
    Ok((members, true))
}

fn waterfall(db: &Connection, events: &[QualityEvent], window: Span) -> Result<Waterfall> {
    let (published, _) = release_members(db, events)?;
    let parks: BTreeMap<_, _> = sensors::parks(db)?
        .into_iter()
        .map(|p| (p.task_id, p.parked_at))
        .collect();
    let mut started = BTreeMap::<String, Time>::new();
    let mut stmt = db.prepare("SELECT entity_id,created_at FROM events WHERE event_type='task_started' ORDER BY created_at,id")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (task, at) = row?;
        started
            .entry(task)
            .or_insert(parse_time(&at).context("invalid task start")?);
    }
    let mut merges = BTreeMap::<String, Time>::new();
    let mut stmt = db.prepare("SELECT task_id,created_at FROM observed_delivery_merges WHERE target_branch LIKE 'epic/%' ORDER BY created_at")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (task, at) = row?;
        merges
            .entry(task)
            .or_insert(parse_time(&at).context("invalid merge timestamp")?);
    }
    let mut stmt = db.prepare("SELECT id,created_at,closed_at FROM tasks")?;
    let mut out = Waterfall {
        tasks: vec![],
        completed_minutes: BTreeMap::new(),
        censored_minutes: BTreeMap::new(),
        unattributed_turns: 0,
        tokens_by_phase: BTreeMap::new(),
        coverage: "First retained task start/park/train merge; window-clipped spans. Median/p90 exclude censored ends. Only turns fully inside one phase receive tokens. Release membership and recorded main-to-publication duration supply later phases when available.",
    };
    let mut values = BTreeMap::<String, Vec<f64>>::new();
    let mut attributed = BTreeSet::new();
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })? {
        let (task, created, closed) = row?;
        let terminal = closed.as_deref().and_then(parse_time);
        if terminal.is_some_and(|at| at <= window.0)
            && !published
                .get(&task)
                .is_some_and(|p| p.published >= window.0)
            && !merges.get(&task).is_some_and(|at| *at >= window.0)
        {
            continue;
        }
        let created = parse_time(&created).context("invalid task creation")?;
        let start = started.get(&task).copied();
        let review = events
            .iter()
            .filter(|e| e.task_id.as_ref() == Some(&task))
            .filter(|e| {
                matches!(
                    e.payload,
                    QualityPayload::CheckRun {
                        stage: CheckStage::Verifier,
                        ..
                    }
                )
            })
            .map(|e| match e.payload {
                QualityPayload::CheckRun {
                    minutes: Some(n), ..
                } => e.occurred_at - Duration::milliseconds((n * 60000.0) as i64),
                _ => e.occurred_at,
            })
            .min();
        let park = parks.get(&task).copied();
        let review = review.filter(|at| park.is_none_or(|park| *at <= park));
        let merged = merges.get(&task).copied();
        let publication = published.get(&task);
        let main = publication.and_then(|p| p.main);
        let released = publication.map(|p| p.published);
        for (name, from, to) in [
            ("created_to_started", Some(created), start),
            ("implementation", start, review.or(park).or(merged)),
            ("review_to_park", review, park.or(merged)),
            ("park_to_train", park, merged),
            ("train_to_main", merged, main),
            ("main_to_published", main, released),
        ] {
            let Some(from) = from else { continue };
            if name == "train_to_main" && main.is_none() {
                continue;
            }
            if to.is_some_and(|to| to < from) {
                continue;
            }
            let Some(span) = clipped((from, to.or(terminal).unwrap_or(window.1)), window) else {
                continue;
            };
            let complete = to.is_some_and(|at| at <= window.1);
            let mut tokens = Tokens::default();
            for event in events
                .iter()
                .filter(|e| e.occurred_at >= window.0 && e.task_id.as_ref() == Some(&task))
            {
                if let Some(turn) = turn_span(event)
                    && turn.0 >= span.0
                    && turn.1 <= span.1
                    && attributed.insert(event.id.clone())
                {
                    tokens.add(event);
                }
            }
            let n = minutes(span);
            if complete {
                values.entry(name.into()).or_default().push(n);
            } else {
                *out.censored_minutes.entry(name.into()).or_default() += n;
            }
            out.tasks.push(Phase {
                task_id: task.clone(),
                phase: name,
                started_at: span.0,
                ended_at: span.1,
                window_minutes: n,
                complete,
                tokens,
            });
        }
    }
    for p in &out.tasks {
        let t = out.tokens_by_phase.entry(p.phase.into()).or_default();
        t.turns += p.tokens.turns;
        t.input += p.tokens.input;
        t.output += p.tokens.output;
        t.missing += p.tokens.missing;
    }
    out.completed_minutes = values
        .into_iter()
        .map(|(k, v)| (k, Distribution::from(v)))
        .collect();
    out.unattributed_turns = events
        .iter()
        .filter(|e| {
            e.occurred_at >= window.0
                && matches!(e.payload, QualityPayload::Turn { .. })
                && !attributed.contains(&e.id)
        })
        .count();
    Ok(out)
}

fn shipped(db: &Connection, events: &[QualityEvent], window: Span) -> Result<Shipped> {
    let mut fixed = BTreeSet::new();
    let mut stmt = db.prepare("SELECT DISTINCT t.id FROM tasks t JOIN observed_delivery_merges m ON m.task_id=t.id
        WHERE t.task_type='bug' AND m.target_branch IN ('main','master') AND julianday(m.created_at)>=julianday(?1) AND julianday(m.created_at)<julianday(?2)")?;
    for row in stmt.query_map([window.0.to_rfc3339(), window.1.to_rfc3339()], |r| {
        r.get::<_, String>(0)
    })? {
        fixed.insert(row?);
    }
    let (members, release_membership_known) = release_members(db, events)?;
    for (task, p) in members {
        if window.0 <= p.published && p.published < window.1 {
            let bug: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?1 AND task_type='bug')",
                [&task],
                |r| r.get(0),
            )?;
            if bug {
                fixed.insert(task);
            }
        }
    }
    let mut tokens = Tokens::default();
    let mut with_turns = BTreeSet::new();
    for event in events
        .iter()
        .filter(|e| e.task_id.as_ref().is_some_and(|task| fixed.contains(task)))
    {
        if matches!(event.payload, QualityPayload::Turn { .. }) {
            tokens.add(event);
            with_turns.insert(event.task_id.as_ref().unwrap());
        }
    }
    let denominator = (fixed.len() > 0 && with_turns.len() == fixed.len() && tokens.missing == 0)
        .then_some(fixed.len() as f64);
    Ok(Shipped {
        shipped_fixes: fixed.len(),
        release_membership_known,
        fixes_with_turns: with_turns.len(),
        input_per_fix: denominator.map(|n| tokens.input as f64 / n),
        output_per_fix: denominator.map(|n| tokens.output as f64 / n),
        tokens,
        coverage: "Bugs with an observed main/master merge or published release membership in this window; retained attributed turns through window end. Train-only and closed-only tasks do not prove shipping. These attributed costs are a lower bound when turns lack task attribution.",
    })
}

pub(super) fn render(out: &mut String, value: &Measure<TimeSinks>) {
    let Measure::Known(s) = value else {
        let _ = writeln!(out, "  Time sinks: {}", value.render(|_| String::new()));
        return;
    };
    let number = |n: Option<f64>| n.map_or_else(|| "unknown".into(), |n| format!("{n:.2}"));
    let distribution = |d: &Distribution| {
        format!(
            "n={} median/p90 {}/{}, total {:.2}",
            d.samples,
            number(d.median),
            number(d.p90),
            d.total
        )
    };
    let _ = writeln!(
        out,
        "  Phase waterfall (window minutes): {}",
        s.phase_waterfall.render(|w| format!(
            "{} intervals; completed {:?}; censored {:?}; {} turns without a phase",
            w.tasks.len(),
            w.completed_minutes
                .iter()
                .map(|(k, d)| (k, distribution(d)))
                .collect::<BTreeMap<_, _>>(),
            w.censored_minutes,
            w.unattributed_turns
        ))
    );
    if let Measure::Known(w) = &s.phase_waterfall {
        for (phase, t) in &w.tokens_by_phase {
            let _ = writeln!(
                out,
                "    {phase} tokens: {}/{} input/output; {} turns, {} missing",
                t.input, t.output, t.turns, t.missing
            );
        }
        let _ = writeln!(out, "    {}", w.coverage);
        for p in &w.tasks {
            let _ = writeln!(
                out,
                "    {} {}: {:.2} min{}; tokens input/output {}/{} ({} turns, {} missing)",
                p.task_id,
                p.phase,
                p.window_minutes,
                if p.complete { "" } else { " censored" },
                p.tokens.input,
                p.tokens.output,
                p.tokens.turns,
                p.tokens.missing
            );
        }
    }
    let _ = writeln!(out,"  Worker minute buckets: {}",s.worker_minutes.render(|w| {
        let row = |r:&WorkerMinutes| format!("alive {:.2}; in turn {:.2}; leased out of turn {:.2}; blocked/waiting {:.2}; unassigned {:.2}",r.observed_alive,r.in_turn,r.leased_out_of_turn,r.blocked_or_waiting,r.unassigned);
        format!("fleet {}; workers {:?}; {} missing turn durations; {} open leases. {}",row(&w.fleet),w.by_worker.iter().map(|(a,r)| (a,row(r))).collect::<BTreeMap<_,_>>(),w.missing_turn_duration,w.open_lease_intervals,w.coverage)
    }));
    let _ = writeln!(out,"  Refusal recovery: {}",s.refusal_recovery_by_code.render(|r| r.iter().map(|(code,r)| format!("{code}: {}/{} recovered, {} unresolved; minutes {}; completed-turn tokens {}/{} ({} boundary turns excluded; {} missing context)",
        r.recovered,r.calls,r.unresolved,distribution(&r.minutes),r.completed_turn_tokens.input,r.completed_turn_tokens.output,r.boundary_turns,r.missing_context)).collect::<Vec<_>>().join("; ")));
    let _ = writeln!(
        out,
        "  Supervisor: {}",
        s.supervisor.render(|s| format!(
            "{} turns; {} no-op; rate {}; wake reasons {:?}; {} unknown actor turns",
            s.turns,
            s.no_op,
            number(s.no_op_rate),
            s.by_wake_reason,
            s.unknown_actor_turns
        ))
    );
    let _ = writeln!(
        out,
        "  CI queue vs run (job minutes): {}",
        s.ci.render(|c| format!(
            "{} jobs; queue {}; run {}; missing queue/run {}/{}; {} without run creation time; coverage {:.2} min, {:.2} min missing; captured {}",
            c.jobs,
            distribution(&c.queue_minutes),
            distribution(&c.run_minutes),
            c.missing_queue,
            c.missing_run, c.missing_creation_time, c.covered_window_minutes, c.uncovered_window_minutes, c.captured_at
        ))
    );
    let _ = writeln!(
        out,
        "  Review rounds {:?}: {}; execution minutes {}; {} missing durations",
        s.reviews.rounds_by_task,
        distribution(&s.reviews.rounds),
        distribution(&s.reviews.minutes),
        s.reviews.missing_duration
    );
    let _ = writeln!(
        out,
        "  Peer cloud-to-admission minutes: {}",
        s.peer_latency.render(|p| format!(
            "{}; machines {:?}; {} invalid timestamps",
            distribution(&p.cloud_created_to_admitted_minutes),
            p.by_machine
                .iter()
                .map(|(m, d)| (m, distribution(d)))
                .collect::<BTreeMap<_, _>>(),
            p.invalid_timestamps
        ))
    );
    let _ = writeln!(
        out,
        "  Tokens per shipped fix: {}",
        s.tokens_per_shipped_fix.render(|t| format!(
            "{} fixes; {} with turns; input/output {}/{}; release membership retained: {}. {}",
            t.shipped_fixes,
            t.fixes_with_turns,
            number(t.input_per_fix),
            number(t.output_per_fix),
            t.release_membership_known,
            t.coverage
        ))
    );
    let _ = writeln!(
        out,
        "  Top five recorded time sinks (minutes): {}. {}",
        s.top_five
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}. {} {:.2} ({})", i + 1, s.category, s.minutes, s.basis))
            .collect::<Vec<_>>()
            .join("; "),
        s.ranking_scope
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::{NewQualityEvent, TurnSpend, WakeChannel};
    use cas_types::{Agent, Event, EventEntityType, EventType, Task, TaskType};
    use serde_json::json;

    #[test]
    fn time_sinks_replay_producer_contracts_with_censoring_and_unknown_inputs() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let root = crate::store::init_cas_dir(env.home()).unwrap();
        env.set("CAS_ROOT", &root);
        let agents = crate::store::open_agent_store(&root).unwrap();
        let tasks = crate::store::open_task_store(&root).unwrap();
        crate::store::open_prompt_queue_store(&root).unwrap();
        let mut task = Task::new("cas-work".into(), "In flight".into());
        task.created_at = Utc::now() - Duration::hours(1);
        tasks.add(&task).unwrap();
        let mut worker =
            Agent::new_with_role("worker-id".into(), "worker".into(), AgentRole::Worker);
        worker.registered_at = Utc::now() - Duration::hours(1);
        worker.last_heartbeat = Utc::now() + Duration::hours(2);
        agents.register(&worker).unwrap();
        agents.try_claim(&task.id, &worker.id, 7200, None).unwrap();
        let claimed = agents.get_lease_history(&task.id, None).unwrap()[0].timestamp;
        let at = Time::from_timestamp(claimed.timestamp() + 1, 0).unwrap();
        let pre_start_lease = minutes((claimed, at));
        let window = (at - Duration::minutes(10), at + Duration::minutes(60));
        let events = crate::store::open_event_store(&root).unwrap();
        for (kind, offset) in [
            (EventType::TaskStarted, 0),
            (EventType::TaskBlocked, 20),
            (EventType::TaskStarted, 40),
        ] {
            let mut event = Event::new(kind, EventEntityType::Task, &task.id, "fixture");
            event.created_at = at + Duration::minutes(offset);
            events.record(&event).unwrap();
        }
        let mut lead = Agent::new_with_role("lead-id".into(), "lead".into(), AgentRole::Supervisor);
        lead.metadata
            .insert("supervisor_cli".into(), "codex".into());
        agents.register(&lead).unwrap();
        let emit_turn = |key: &str,
                         actor: &str,
                         task: Option<&str>,
                         end: Time,
                         duration: f64,
                         no_op: bool,
                         supersedes: Option<String>| {
            let mut event = NewQualityEvent::new(
                key,
                QualityPayload::Turn {
                    woken_by: WakeChannel::Unknown,
                    notice_kind: Some("task_lifecycle".into()),
                    supersedes,
                    actions: if no_op {
                        vec![]
                    } else {
                        vec![cas_store::TurnAction::Answer]
                    },
                    no_op,
                    input_tokens: Some(6000),
                    output_tokens: Some(60),
                    minutes: Some(duration),
                    spend: Some(TurnSpend::default()),
                },
            );
            event.actor = Some(actor.into());
            event.task_id = task.map(str::to_string);
            event.occurred_at = end;
            event.scope = crate::cloud::resolve_canonical_id_for_sync(&root).unwrap_or_default();
            let id = event.id();
            assert!(crate::quality_ledger::emit(&root, &event));
            id
        };
        let prior = emit_turn(
            "worker-old",
            &worker.id,
            Some(&task.id),
            at + Duration::minutes(15),
            10.0,
            false,
            None,
        );
        emit_turn(
            "worker-detail",
            &worker.id,
            Some(&task.id),
            at + Duration::minutes(15),
            10.0,
            false,
            Some(prior),
        );
        emit_turn(
            "worker-overlap",
            &worker.id,
            Some(&task.id),
            at + Duration::minutes(17),
            5.0,
            false,
            None,
        );
        emit_turn(
            "lead-noop",
            &lead.id,
            None,
            at + Duration::minutes(10),
            2.0,
            true,
            None,
        );
        emit_turn(
            "lead-action",
            &lead.id,
            None,
            at + Duration::minutes(20),
            3.0,
            false,
            None,
        );
        emit_turn("outside", &lead.id, None, window.1, 1.0, true, None);
        let mut verdict = cas_types::Verification::new("ver-time".into(), task.id.clone());
        verdict.status = cas_types::VerificationStatus::Approved;
        verdict.created_at = at + Duration::minutes(20);
        verdict.duration_ms = Some(300_000);
        crate::quality_ledger::verification_recorded(&root, &verdict);
        let args = json!({"action":"close","id":task.id});
        let actor = crate::agent_model::AgentModel::from_agent(&worker);
        crate::gates::facts::observe_tool_result_as(
            &root,
            args.as_object(),
            &Ok(rmcp::model::CallToolResult::error(vec![
                rmcp::model::ContentBlock::text(crate::gates::tag("G-61", "fixture")),
            ])),
            Some(&actor),
            Some("task"),
        );
        let refusal = events
            .list_by_type(EventType::GateRefused, 10)
            .unwrap()
            .pop()
            .unwrap();
        crate::efficiency_facts::tool_succeeded(
            &root,
            "wrong-actor",
            "task",
            "close",
            Some(&task.id),
            refusal.created_at + Duration::minutes(1),
        );
        crate::efficiency_facts::tool_succeeded(
            &root,
            &worker.id,
            "task",
            "show",
            Some(&task.id),
            refusal.created_at + Duration::minutes(1),
        );
        crate::efficiency_facts::tool_succeeded(
            &root,
            &worker.id,
            "task",
            "close",
            Some(&task.id),
            refusal.created_at + Duration::minutes(15),
        );
        crate::efficiency_facts::tool_succeeded(
            &root,
            &worker.id,
            "task",
            "close",
            Some(&task.id),
            refusal.created_at + Duration::minutes(20),
        );
        let message=serde_json::from_value(json!({"id":"peer-1","project_id":"project","recipient_agent_id":lead.id,
            "sender_agent_id":"remote","sender_name":"peer","sender_label":"other-machine","body":"fixture","created_at":at.to_rfc3339()})).unwrap();
        crate::efficiency_facts::peer_admitted(&root, &message, at + Duration::minutes(4));
        crate::efficiency_facts::peer_admitted(&root, &message, at + Duration::minutes(9));
        crate::efficiency_facts::issue_closed(&root, "org/repo#42", at + Duration::minutes(2));
        crate::efficiency_facts::issue_closed(&root, "org/repo#43", window.1);
        let mut fixed = Task::new("cas-shipped".into(), "Shipped bug".into());
        fixed.task_type = TaskType::Bug;
        fixed.created_at = at - Duration::minutes(10);
        fixed.closed_at = Some(at + Duration::seconds(1));
        tasks.add(&fixed).unwrap();
        cas_store::record_observed_delivery_merge(
            &root,
            &fixed.id,
            "factory/worker",
            "main",
            "aaaaaaa",
            "bbbbbbb",
            &lead.id,
        )
        .unwrap();
        emit_turn(
            "shipped-turn",
            &worker.id,
            Some(&fixed.id),
            at - Duration::minutes(1),
            4.0,
            false,
            None,
        );
        let mut member = Task::new("cas-release-member".into(), "Release member".into());
        member.task_type = TaskType::Bug;
        member.created_at = at - Duration::hours(1);
        member.closed_at = Some(at - Duration::minutes(20));
        tasks.add(&member).unwrap();
        cas_store::record_observed_delivery_merge(
            &root,
            &member.id,
            "factory/member",
            "epic/train",
            "ccccccc",
            "ddddddd",
            &lead.id,
        )
        .unwrap();
        emit_turn(
            "member-turn",
            &worker.id,
            Some(&member.id),
            at - Duration::minutes(25),
            2.0,
            false,
            None,
        );
        let key = cas_store::release_reconcile_store::ReleaseOperationKey {
            project_id: "scorecard-fixture".into(),
            release_id: "v1.0.0".into(),
            final_release_sha: "a".repeat(40),
        };
        let receipt = crate::release_reconcile::ReleaseReconcileReceipt {
            scope: crate::qa_batch::ReleaseScope {
                project_id: key.project_id.clone(),
                repository: "org/repo".into(),
                release_id: key.release_id.clone(),
                reviewed_sha: key.final_release_sha.clone(),
                final_release_sha: key.final_release_sha.clone(),
                promotion_pr: None,
                release_tag: Some(key.release_id.clone()),
                constituent_epic_ids: vec![],
                included_task_ids: vec![member.id.clone()],
                no_code_members: BTreeMap::new(),
                proof_refs: vec![],
            },
            key: key.clone(),
            reconciler_version: Some(env!("CARGO_PKG_VERSION").into()),
            reconciler_stale: false,
            preview: None,
            triggers: vec![],
            members: BTreeMap::new(),
            close_intents: BTreeMap::new(),
            issue_outcomes: vec![],
            parents: BTreeMap::new(),
            exceptions: vec![],
            started_at: at.to_rfc3339(),
            updated_at: at.to_rfc3339(),
            complete: true,
            sync_pending: vec![],
            residuals: vec![],
            notice_intent: None,
        };
        cas_store::release_reconcile_store::ReleaseOperation::open(&root, key)
            .unwrap()
            .save(&serde_json::to_value(receipt).unwrap())
            .unwrap();
        let mut release = NewQualityEvent::new(
            "release-fixture",
            QualityPayload::Release {
                commit_to_main_minutes: None,
                main_to_published_minutes: Some(5.0),
                ejections: 0,
                rescues: 0,
            },
        );
        release.release_version = Some("v1.0.0".into());
        release.occurred_at = at + Duration::minutes(50);
        assert!(crate::quality_ledger::emit(&root, &release));
        let db = Connection::open(root.join("cas.db")).unwrap();
        let report = super::super::window::build(&root, window.0, window.1, 10).unwrap();
        let report = serde_json::to_value(report).unwrap();
        let s = &report["efficiency"]["time_sinks"];
        assert!(
            s["phase_waterfall"]["tasks"].is_array(),
            "{}",
            s["phase_waterfall"]
        );
        let w = &s["worker_minutes"]["by_worker"][&worker.id];
        let near = |value: &Value, expected: f64| {
            assert!(
                (value.as_f64().unwrap() - expected).abs() < 1.0 / 60000.0 + 1e-9,
                "{value} != {expected}"
            );
        };
        near(&w["observed_alive"], 70.0);
        near(&w["in_turn"], 16.0);
        near(&w["leased_out_of_turn"], 28.0 + pre_start_lease);
        near(&w["blocked_or_waiting"], 20.0);
        near(&w["unassigned"], 6.0 - pre_start_lease);
        let partition: f64 = [
            "in_turn",
            "leased_out_of_turn",
            "blocked_or_waiting",
            "unassigned",
        ]
        .iter()
        .map(|key| w[key].as_f64().unwrap())
        .sum();
        assert!((partition - w["observed_alive"].as_f64().unwrap()).abs() < 1e-9);
        assert_eq!(
            s["worker_minutes"]["fleet"],
            s["worker_minutes"]["by_worker"][&worker.id]
        );
        assert_eq!(s["supervisor"]["turns"], 2);
        assert_eq!(s["supervisor"]["no_op_rate"], 0.5);
        assert_eq!(s["supervisor"]["by_wake_reason"]["task_lifecycle"], 2);
        assert_eq!(s["reviews"]["rounds_by_task"][&task.id], 1);
        assert_eq!(s["reviews"]["minutes"]["total"], 5.0);
        assert_eq!(
            s["phase_waterfall"]["completed_minutes"]["main_to_published"]["total"],
            5.0
        );
        assert_eq!(
            s["refusal_recovery_by_code"]["G-61"]["minutes"]["median"],
            15.0
        );
        assert_eq!(s["refusal_recovery_by_code"]["G-61"]["recovered"], 1);
        assert_eq!(
            s["peer_latency"]["cloud_created_to_admitted_minutes"]["total"],
            4.0
        );
        assert_eq!(
            s["peer_latency"]["cloud_created_to_admitted_minutes"]["samples"],
            1
        );
        assert_eq!(s["tokens_per_shipped_fix"]["shipped_fixes"], 2);
        assert_eq!(s["tokens_per_shipped_fix"]["input_per_fix"], 6000.0);
        assert_eq!(report["efficiency"]["issues"]["closed_in_window"], 1);
        let implementation = s["phase_waterfall"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["task_id"] == task.id && p["phase"] == "implementation")
            .unwrap();
        near(&implementation["window_minutes"], 15.0);
        assert_eq!(implementation["tokens"]["input"], 6000);
        near(
            &s["phase_waterfall"]["censored_minutes"]["review_to_park"],
            45.0,
        );
        assert!(s["ci"]["unknown"].is_string());
        let ranked = s["top_five"].as_array().unwrap();
        assert!(!ranked.is_empty() && ranked.len() <= 5);
        assert!(
            ranked
                .windows(2)
                .all(|p| p[0]["minutes"].as_f64() >= p[1]["minutes"].as_f64())
        );
        db.execute_batch("DROP TABLE efficiency_observations; DROP TABLE task_lease_history")
            .unwrap();
        let missing = serde_json::to_value(build(&root, &db, window.0, window.1).unwrap()).unwrap();
        assert!(missing["peer_latency"]["unknown"].is_string());
        assert!(missing["worker_minutes"]["unknown"].is_string());
        assert_eq!(missing["supervisor"]["turns"], 2);
    }
}
