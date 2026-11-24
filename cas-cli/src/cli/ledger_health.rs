//! Sensor health: every measurement the report relies on, classified once.
//! OK means it measured something; EMPTY means zero samples in the window (so a
//! zero is not a finding); BROKEN means the input is missing, unattributable or
//! impossible, so the number beside it cannot be trusted.
use std::collections::BTreeMap;
use std::fmt::Write;

use serde::Serialize;

use super::sensors::Sensors;
use super::{LedgerReport, Measure};

/// Longest duration a single sample may report before it is a sensor fault.
const IMPOSSIBLE_MINUTES: f64 = 30.0 * 24.0 * 60.0;
/// Share of examined hosted jobs whose runner could not be read before the sensor is broken.
const MISSING_SHARE: f64 = 0.05;
/// Per-task spend must add up to the actor totals within this share.
const RECONCILE_SHARE: f64 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Health {
    Ok,
    Empty,
    Broken,
}

impl Health {
    fn label(self) -> &'static str {
        match self {
            Health::Ok => "OK",
            Health::Empty => "EMPTY",
            Health::Broken => "BROKEN",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SensorHealth {
    pub sensor: &'static str,
    pub state: Health,
    pub reason: String,
}

fn row(sensor: &'static str, state: Health, reason: impl Into<String>) -> SensorHealth {
    SensorHealth { sensor, state, reason: reason.into() }
}

fn count_by<'a>(items: impl Iterator<Item = &'a str>, classify: impl Fn(&str) -> &'static str) -> String {
    let mut counts = BTreeMap::<&str, usize>::new();
    for item in items {
        *counts.entry(classify(item)).or_default() += 1;
    }
    counts.into_iter().map(|(what, n)| format!("{what} {n}")).collect::<Vec<_>>().join(", ")
}

fn runner_losses(sensors: &Sensors) -> SensorHealth {
    match &sensors.hosted_runner_losses {
        Measure::Unknown { unknown } => row("hosted_runner_losses", Health::Broken, unknown.clone()),
        Measure::Known(r) if r.jobs_examined == 0 => row("hosted_runner_losses", Health::Empty, "no hosted jobs examined in the window"),
        Measure::Known(r) if r.missing.len() as f64 > MISSING_SHARE * r.jobs_examined as f64 => row(
            "hosted_runner_losses",
            Health::Broken,
            format!(
                "{} of {} jobs missing inputs ({})",
                r.missing.len(),
                r.jobs_examined,
                count_by(r.missing.iter().map(String::as_str), |m| if m.contains("allocation not retained") {
                    "runner allocation not retained"
                } else {
                    "job log fetch failed"
                })
            ),
        ),
        Measure::Known(r) => row("hosted_runner_losses", Health::Ok, format!("{} confirmed of {} jobs; {} missing", r.losses.len(), r.jobs_examined, r.missing.len())),
    }
}

fn issues(sensors: &Sensors) -> SensorHealth {
    match &sensors.daily_issues {
        Measure::Unknown { unknown } => row("daily_issues", Health::Broken, unknown.clone()),
        Measure::Known(d) if d.days.is_empty() => row("daily_issues", Health::Empty, "no issues opened or closed in the window"),
        Measure::Known(d) => row("daily_issues", Health::Ok, format!("{} day(s) with issue activity", d.days.len())),
    }
}

fn parks(sensors: &Sensors) -> [SensorHealth; 2] {
    let latency = match &sensors.park_to_train_merge {
        Measure::Unknown { unknown } if unknown.contains("no retained first parks") => row("park_to_train_merge", Health::Empty, unknown.clone()),
        Measure::Unknown { unknown } => row("park_to_train_merge", Health::Broken, unknown.clone()),
        Measure::Known(p) if p.first_parks == 0 => row("park_to_train_merge", Health::Empty, "no first parks in the window"),
        Measure::Known(p) => row("park_to_train_merge", Health::Ok, format!("{} of {} first parks merged", p.merged, p.first_parks)),
    };
    let cross = match &sensors.cross_machine_parks {
        Measure::Unknown { unknown } => row("cross_machine_parks", Health::Broken, unknown.clone()),
        Measure::Known(v) if !v.unmeasurable.is_empty() => row(
            "cross_machine_parks",
            Health::Broken,
            format!("{} park(s) cannot be measured: {}", v.unmeasurable.len(), v.unmeasurable[0]),
        ),
        Measure::Known(v) if v.observed_cross_machine == 0 => row(
            "cross_machine_parks",
            Health::Empty,
            format!(
                "{} park(s) examined, none crossed machines (origin = merge owner: {})",
                v.examined,
                v.same_machine.iter().map(|(machine, n)| format!("{machine} {n}")).collect::<Vec<_>>().join(", ")
            ),
        ),
        Measure::Known(v) => row("cross_machine_parks", Health::Ok, format!("{} of {} parks crossed machines", v.observed_cross_machine, v.examined)),
    };
    [latency, cross]
}

pub(super) fn assess(report: &LedgerReport) -> Vec<SensorHealth> {
    let mut rows = vec![runner_losses(&report.sensors), issues(&report.sensors)];
    rows.extend(parks(&report.sensors));

    let t = &report.turns;
    rows.push(if t.wakes == 0 {
        row("turn_usage", Health::Empty, "no model turns in the window")
    } else if t.missing_tokens > 0 || t.missing_minutes > 0 {
        row("turn_usage", Health::Broken, format!("{} turn(s) without tokens, {} without minutes", t.missing_tokens, t.missing_minutes))
    } else {
        row("turn_usage", Health::Ok, format!("{} turns measured", t.wakes))
    });

    let unknown_wakes: usize = report.wake_classification_missing_inputs.values().sum();
    rows.push(if unknown_wakes == 0 {
        row("wake_classification", Health::Ok, "every wake has a notice kind")
    } else {
        row(
            "wake_classification",
            Health::Broken,
            format!(
                "{unknown_wakes} wake(s) without a notice kind ({})",
                report.wake_classification_missing_inputs.iter().map(|(why, n)| format!("{why} {n}")).collect::<Vec<_>>().join(", ")
            ),
        )
    });

    let c = &report.spend_coverage;
    let summed: u64 = report.spend_by_task.values().map(|t| t.total.input_tokens).sum();
    let off = summed.abs_diff(t.input_tokens) as f64;
    rows.push(if c.turns == 0 {
        row("turn_spend", Health::Empty, "no model turns in the window")
    } else if off > RECONCILE_SHARE * t.input_tokens as f64 {
        row("turn_spend", Health::Broken, format!("per-task input {summed} does not reconcile with actor total {}", t.input_tokens))
    } else if c.with_model == 0 {
        row("turn_spend", Health::Empty, format!("0 of {} turns carry model, effort or harness; turns recorded before the producer stored them cannot be split", c.turns))
    } else {
        row("turn_spend", Health::Ok, format!("{} of {} turns carry model identity, {} carry a task", c.with_model, c.turns, c.with_task))
    });

    for scorer in &report.scorers {
        if let Some(why) = &scorer.unavailable {
            rows.push(row(scorer.name, Health::Broken, why.clone()));
        } else if scorer.scored == 0 {
            rows.push(row(scorer.name, Health::Empty, "0 subjects scored in the window"));
        } else {
            rows.push(row(scorer.name, Health::Ok, format!("{} scored, {} hit(s)", scorer.scored, scorer.hits.len())));
        }
    }

    let e = &report.estimates;
    rows.push(if e.completed == 0 {
        row("estimates", Health::Empty, "0 completed estimates")
    } else {
        row("estimates", Health::Ok, format!("{} completed, {} hit", e.completed, e.hits))
    });

    rows.push(if report.features.is_empty() {
        row("feature_feedback", Health::Empty, format!("no feedback rows with known scope ({} unscoped excluded)", report.excluded_unscoped_measurements))
    } else {
        row("feature_feedback", Health::Ok, format!("{} feature(s) with verdicts", report.features.len()))
    });

    let impossible: Vec<String> = report
        .check_cohorts
        .iter()
        .filter_map(|c| {
            let longest = c.row.max_minutes.filter(|m| *m > IMPOSSIBLE_MINUTES)?;
            Some(format!("{} {}: one sample of {longest:.0} min", c.project, c.check_id))
        })
        .collect();
    rows.push(if impossible.is_empty() {
        row("durations", Health::Ok, "no sample longer than 30 days")
    } else {
        row("durations", Health::Broken, format!("{} (over 30 days)", impossible.join("; ")))
    });
    rows
}

/// Anything that is not OK, then the OK rows, one line each.
pub(super) fn render(out: &mut String, health: &[SensorHealth]) {
    let bad = health.iter().filter(|h| h.state != Health::Ok).count();
    let _ = writeln!(out, "\nSensor health: {bad} of {} need attention", health.len());
    for state in [Health::Broken, Health::Empty, Health::Ok] {
        for h in health.iter().filter(|h| h.state == state) {
            let _ = writeln!(out, "  {:<6} {}: {}", state.label(), h.sensor, h.reason);
        }
    }
}
