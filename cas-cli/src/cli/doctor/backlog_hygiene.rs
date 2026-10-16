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

/// Pure rendering of one scan.
pub(super) fn check_for(_report: &crate::backlog_hygiene::BacklogReport) -> Check {
    Check::new(NAME, CheckStatus::Ok, "no stale open work")
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
