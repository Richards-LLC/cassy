//! Journey traffic shares from Commander's local usage counts (cas-0b3d).
//!
//! `cas qa journeys --traffic` sums the machine hub's usage ledger over a
//! window, gives each registry journey its share of the counted actions, and
//! with `--write` records those shares and a `traffic_snapshot` header in the
//! registry. Only the traffic blocks, `updated` and the header change; every
//! other line of the hand-curated YAML is kept byte for byte.

use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use chrono::NaiveDate;
use serde::Serialize;

use crate::hub::usage::{UsageLedger, journey_totals};
use crate::qa_registry::Registry;

pub const SOURCE: &str = "Commander usage counter";

#[derive(Debug, Serialize)]
pub struct JourneyShare {
    pub id: String,
    pub name: String,
    pub count: u64,
    pub share_pct: f64,
}

#[derive(Debug, Serialize)]
pub struct TrafficReport {
    pub as_of: String,
    pub window_days: u32,
    /// Counted actions that belong to a registry journey.
    pub actions: u64,
    /// Highest share first; journeys with no counted action have 0.
    pub journeys: Vec<JourneyShare>,
    /// Counted journey ids the registry does not list.
    pub unregistered: BTreeMap<String, u64>,
}

impl TrafficReport {
    pub fn basis(&self, share: &JourneyShare) -> String {
        format!(
            "{} of {} counted Commander actions in the {} days to {}",
            share.count, self.actions, self.window_days, self.as_of
        )
    }

    pub fn snapshot_line(&self) -> String {
        format!(
            "traffic_snapshot: {{as_of: '{}', window_days: {}, actions: {}, source: {SOURCE}}}",
            self.as_of, self.window_days, self.actions
        )
    }
}

pub fn report(registry: &Registry, ledger: &UsageLedger, today: NaiveDate, window_days: u32) -> TrafficReport {
    let mut totals = journey_totals(ledger, today, window_days);
    let live: Vec<_> = registry.journeys.iter().filter(|j| j.status == "live").collect();
    let counts: Vec<u64> = live.iter().map(|j| totals.remove(&j.id).unwrap_or(0)).collect();
    let actions: u64 = counts.iter().sum();
    let mut journeys: Vec<JourneyShare> = live
        .iter()
        .zip(counts)
        .map(|(journey, count)| JourneyShare {
            id: journey.id.clone(),
            name: journey.name.clone().unwrap_or_else(|| journey.id.clone()),
            count,
            share_pct: if actions == 0 {
                0.0
            } else {
                (count as f64 * 1000.0 / actions as f64).round() / 10.0
            },
        })
        .collect();
    journeys.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.id.cmp(&b.id)));
    TrafficReport {
        as_of: today.format("%Y-%m-%d").to_string(),
        window_days: window_days.max(1),
        actions,
        journeys,
        unregistered: totals,
    }
}

fn top_level(line: &str) -> bool {
    !line.is_empty() && !line.starts_with([' ', '#'])
}

/// Rewrite the registry text with the report's shares. Refuses an empty window
/// so measured zeros never replace accepted-debt notes.
pub fn apply(text: &str, report: &TrafficReport) -> Result<String> {
    ensure!(
        report.actions > 0,
        "no counted Commander actions in the {} days to {}; nothing to write",
        report.window_days,
        report.as_of
    );
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    // Header: replace `updated`, then replace or insert `traffic_snapshot`.
    let updated = lines
        .iter()
        .position(|l| l.starts_with("updated:"))
        .context("registry has no top-level updated: line")?;
    lines[updated] = format!("updated: '{}'", report.as_of);
    if let Some(start) = lines.iter().position(|l| l.starts_with("traffic_snapshot:")) {
        let end = (start + 1..lines.len()).find(|&i| top_level(&lines[i])).unwrap_or(lines.len());
        lines.splice(start..end, [report.snapshot_line()]);
    } else {
        lines.insert(updated + 1, report.snapshot_line());
    }
    for share in &report.journeys {
        let start = lines
            .iter()
            .position(|l| l.trim_end() == format!("- id: {}", share.id))
            .with_context(|| format!("journey {} is not written as `- id: {}`", share.id, share.id))?;
        let end = (start + 1..lines.len())
            .find(|&i| lines[i].starts_with("- ") || top_level(&lines[i]))
            .unwrap_or(lines.len());
        let block = [
            "  traffic:".to_owned(),
            format!("    share_pct: {}", share.share_pct),
            format!("    basis: {}", report.basis(share)),
            format!("    source: {SOURCE}"),
        ];
        match (start + 1..end).find(|&i| lines[i].starts_with("  traffic:")) {
            Some(traffic) => {
                let stop = (traffic + 1..end)
                    .find(|&i| !lines[i].starts_with("   "))
                    .unwrap_or(end);
                lines.splice(traffic..stop, block);
            }
            None => {
                lines.splice(end..end, block);
            }
        }
    }
    let mut written = lines.join("\n");
    written.push('\n');
    crate::qa_registry::parse(&written).context("the rewritten registry no longer parses")?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::usage::UsageCount;

    #[test]
    fn shares_rank_journeys_and_write_only_traffic_lines() {
        let text = "version: 1\nupdated: '2026-10-21'\njourneys:\n- id: HUB-J1\n  name: Pair\n  status: live\n  traffic:\n    share_pct: null\n    basis: 'Accepted debt: not instrumented.'\n  spec: a.ts\n- id: HUB-J5\n  name: Reply\n  status: live\n  # kept comment\n  traffic: {share_pct: null}\n- id: OLD\n  status: dead\n  status_reason: gone\n";
        let registry = crate::qa_registry::parse(text).unwrap();
        let mut ledger = UsageLedger::default();
        let day = |pairs: &[(&str, &str, u64)]| {
            [("2026-10-26".to_owned(), pairs.iter().map(|(a, j, c)| ((*a).to_owned(), UsageCount { journey: (*j).into(), count: *c })).collect())].into()
        };
        ledger.devices.insert("a".into(), day(&[("reply.typed", "HUB-J5", 3), ("pair.code", "HUB-J1", 1)]));
        ledger.devices.insert("b".into(), day(&[("reply.typed", "HUB-J5", 4), ("other.thing", "HUB-J99", 2)]));
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let report = report(&registry, &ledger, today, 30);
        assert_eq!(report.actions, 8);
        assert_eq!(report.journeys.iter().map(|j| (j.id.as_str(), j.share_pct)).collect::<Vec<_>>(), vec![("HUB-J5", 87.5), ("HUB-J1", 12.5)]);
        assert_eq!(report.unregistered.get("HUB-J99"), Some(&2));
        let written = apply(text, &report).unwrap();
        assert!(written.contains("updated: '2026-10-26'\ntraffic_snapshot: {as_of: '2026-10-26', window_days: 30, actions: 8, source: Commander usage counter}\n"));
        assert!(written.contains("  traffic:\n    share_pct: 12.5\n    basis: 1 of 8 counted Commander actions in the 30 days to 2026-10-26\n    source: Commander usage counter\n  spec: a.ts\n"));
        assert!(written.contains("  # kept comment\n  traffic:\n    share_pct: 87.5\n"));
        let rewritten = crate::qa_registry::parse(&written).unwrap();
        assert!(rewritten.traffic_snapshot.is_some());
        let empty = super::report(&registry, &UsageLedger::default(), today, 30);
        assert!(apply(text, &empty).is_err());
    }
}
