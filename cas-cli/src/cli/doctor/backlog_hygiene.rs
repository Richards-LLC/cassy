//! `cas doctor` row for backlog hygiene (cas-e8d1, GH #1233). The scan lives
//! in [`crate::backlog_hygiene`]; this renders it as one check.

use super::{Check, CheckStatus};
use std::path::Path;

pub(super) const NAME: &str = "backlog hygiene";

pub(super) fn check(cas_dir: &Path) -> Check {
    match crate::backlog_hygiene::scan(cas_dir, chrono::Utc::now()) {
        Ok(report) => check_for(&report),
        Err(error) => Check::new(NAME, CheckStatus::Warning, format!("cannot scan the backlog: {error}")),
    }
}

/// Entries of each kind the row lists before counting the rest.
const LISTED: usize = 10;

/// Pure rendering of one scan: a summary line, then each finding with its
/// evidence, and for delivered tasks and finished epics the close call.
/// Delivered or finished work warns; idle work alone informs.
pub(super) fn check_for(report: &crate::backlog_hygiene::BacklogReport) -> Check {
    use crate::backlog_hygiene as hygiene;
    let reference = report.default_ref.as_deref().unwrap_or("the default branch");
    let mut lines = Vec::new();
    if report.is_clean() {
        lines.push(match report.default_ref.as_deref() {
            Some(reference) => format!("no stale open work (delivery measured on {reference})"),
            None => "no stale open work".to_string(),
        });
    } else {
        lines.push(hygiene::summary_line(report));
    }
    let section = |label: &str, entries: Vec<String>, lines: &mut Vec<String>| {
        if entries.is_empty() {
            return;
        }
        lines.push(format!("{label}:"));
        let more = entries.len().saturating_sub(LISTED);
        lines.extend(entries.into_iter().take(LISTED).map(|line| format!("  {line}")));
        if more > 0 {
            lines.push(format!("  +{more} more"));
        }
    };
    section(
        "delivered but open",
        report.delivered.iter().map(|f| hygiene::delivered_line(f, reference)).collect(),
        &mut lines,
    );
    section(
        "finished epics open",
        report.finished_epics.iter().map(hygiene::epic_line).collect(),
        &mut lines,
    );
    section(
        &format!("idle {}+ days", hygiene::IDLE_DAYS),
        report.idle.iter().map(hygiene::idle_line).collect(),
        &mut lines,
    );
    if let Some(reason) = report.delivery_unknown.as_deref() {
        lines.push(format!("delivered-on-default-branch check unknown: {reason}"));
    }
    let status = if !report.delivered.is_empty() || !report.finished_epics.is_empty() {
        CheckStatus::Warning
    } else if !report.idle.is_empty() || report.delivery_unknown.is_some() {
        CheckStatus::Info
    } else {
        CheckStatus::Ok
    };
    Check::new(NAME, status, lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backlog_hygiene::{BacklogReport, CommitRecord, DeliveredOpen, FinishedEpic, IdleOpen};

    fn report() -> BacklogReport {
        BacklogReport {
            default_ref: Some("origin/main".into()),
            delivery_unknown: None,
            delivered: vec![DeliveredOpen {
                task_id: "cas-d1".into(),
                title: "a delivered task".into(),
                status: "open".into(),
                commit: CommitRecord {
                    sha: "1".repeat(40),
                    committed_at: chrono::Utc::now(),
                    subject: "fix(cas-d1): shipped".into(),
                },
                more_commits: 2,
            }],
            finished_epics: vec![FinishedEpic {
                epic_id: "cas-e1".into(),
                title: "an epic".into(),
                children: 3,
                last_closed_at: None,
            }],
            idle: vec![IdleOpen {
                task_id: "cas-i1".into(),
                title: "an idle task".into(),
                status: "open".into(),
                assignee: Some("w".into()),
                idle_days: 21,
            }],
        }
    }

    /// cas-e8d1: each finding shows its evidence; kinds 1 and 2 show the
    /// close call. Findings warn; a clean backlog passes.
    #[test]
    fn cas_e8d1_doctor_row_lists_each_finding_with_evidence() {
        let row = check_for(&report());
        assert!(matches!(row.status, CheckStatus::Warning), "{}", row.message);
        let text = &row.message;
        assert!(text.starts_with("1 delivered but open, 1 finished epic open, 1 idle 14+ days"), "{text}");
        assert!(text.contains("cas-d1") && text.contains("fix(cas-d1): shipped") && text.contains("+2 more commits"), "{text}");
        assert!(text.contains(&format!("commit_receipt={}", "1".repeat(40))), "{text}");
        assert!(text.contains("cas-e1") && text.contains("all 3 children"), "{text}");
        assert!(text.contains("task action=close id=cas-e1"), "{text}");
        assert!(text.contains("cas-i1") && text.contains("21 days") && text.contains("w"), "{text}");

        let clean = check_for(&BacklogReport { default_ref: Some("main".into()), ..BacklogReport::default() });
        assert!(matches!(clean.status, CheckStatus::Ok), "{}", clean.message);

        let idle_only = check_for(&BacklogReport { finished_epics: vec![], delivered: vec![], ..report() });
        assert!(matches!(idle_only.status, CheckStatus::Info), "idle alone informs: {}", idle_only.message);

        let unknown = check_for(&BacklogReport { delivery_unknown: Some("no git repository".into()), ..BacklogReport::default() });
        assert!(unknown.message.contains("no git repository"), "{}", unknown.message);
    }
}
