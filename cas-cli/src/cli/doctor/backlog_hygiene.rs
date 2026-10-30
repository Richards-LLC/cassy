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
        "delivered then reverted (review; no close receipt)",
        report.reverted.iter().map(|f| hygiene::reverted_line(f, reference)).collect(),
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
    section(
        "citing removed code (review; never closed automatically)",
        report.stale_citations.iter().map(hygiene::stale_citation_line).collect(),
        &mut lines,
    );
    if report.citations_unchecked > 0 {
        lines.push(format!(
            "{} open task{} not checked for removed code (scan budget)",
            report.citations_unchecked,
            if report.citations_unchecked == 1 { "" } else { "s" },
        ));
    }
    if let Some(reason) = report.delivery_unknown.as_deref() {
        lines.push(format!("delivered-on-default-branch check unknown: {reason}"));
    }
    let status = if !report.delivered.is_empty() || !report.reverted.is_empty() || !report.finished_epics.is_empty() {
        CheckStatus::Warning
    } else if !report.idle.is_empty()
        || !report.stale_citations.is_empty()
        || report.delivery_unknown.is_some()
    {
        CheckStatus::Info
    } else {
        CheckStatus::Ok
    };
    Check::new(NAME, status, lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backlog_hygiene::{
        BacklogReport, CitationKind, CommitRecord, DeliveredOpen, FinishedEpic, GoneCitation,
        IdleOpen, StaleCitations,
    };

    fn report() -> BacklogReport {
        BacklogReport {
            default_ref: Some("origin/main".into()),
            delivery_unknown: None,
            reverted: vec![],
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
            stale_citations: vec![],
            citations_unchecked: 0,
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

        // cas-d9038: kind 4 informs (review, never close) and stays bounded.
        let stale = |n: usize| StaleCitations {
            task_id: format!("cas-s{n}"),
            title: "z".repeat(120),
            status: "open".into(),
            gone: vec![GoneCitation {
                kind: CitationKind::File,
                cited: format!("cas-cli/src/removed_{n}.rs"),
                removed_in: "abc123456".into(),
                removed_on: "2026-10-01".into(),
            }],
        };
        let only_stale = check_for(&BacklogReport {
            stale_citations: (0..40).map(stale).collect(),
            citations_unchecked: 3,
            ..BacklogReport::default()
        });
        assert!(matches!(only_stale.status, CheckStatus::Info), "{}", only_stale.message);
        assert!(only_stale.message.starts_with("0 delivered but open, 0 finished epics open, 0 idle 14+ days, 40 citing removed code"), "{}", only_stale.message);
        assert!(only_stale.message.contains("cas-cli/src/removed_0.rs") && only_stale.message.contains("+30 more"), "{}", only_stale.message);
        assert!(only_stale.message.contains("3 open tasks not checked"), "{}", only_stale.message);
        assert!(only_stale.message.len() <= 6_000, "doctor row bounded: {} bytes", only_stale.message.len());

        let unknown = check_for(&BacklogReport { delivery_unknown: Some("no git repository".into()), ..BacklogReport::default() });
        assert!(unknown.message.contains("no git repository"), "{}", unknown.message);

        let only_reverted = check_for(&BacklogReport {
            reverted: report().delivered,
            ..BacklogReport::default()
        });
        assert!(matches!(only_reverted.status, CheckStatus::Warning));
        assert!(only_reverted.message.contains("1 delivered then reverted"));
        assert!(!only_reverted.message.contains("commit_receipt=")
            && !only_reverted.message.contains("task action=close"));
    }
}
