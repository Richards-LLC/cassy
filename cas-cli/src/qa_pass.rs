//! Independent QA and polish pass (cas-619f): eligibility and the QA work
//! item. Storage, the no-self-review rule and round accounting live in
//! `cas_store::qa_pass_store`; this module decides *whether* a delivery
//! needs a pass and what the reviewer is told to do.
//!
//! Design: `docs/qa/independent-qa-pass.md`.

use std::path::Path;
use std::process::Command;

use cas_types::{QaPass, Task, TaskType};

use crate::config::QaConfig;

/// Label carried by every Cassy-created QA work item.
pub const QA_PASS_LABEL: &str = "qa-pass";

/// Why a delivery needs the independent pass (empty when it does not).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QaEligibility {
    pub reasons: Vec<String>,
}

impl QaEligibility {
    pub fn is_eligible(&self) -> bool {
        !self.reasons.is_empty()
    }
}

/// Decide whether a parked delivery needs an independent QA pass.
///
/// Epics and QA work items themselves are never eligible (a QA task reviews a
/// delivery; it is not one). Otherwise any of: a demo_statement, a
/// user-facing label, or a changed path matching `qa.user_facing_paths`.
pub fn delivery_eligibility(task: &Task, qa: &QaConfig, changed_paths: &[String]) -> QaEligibility {
    let mut reasons = Vec::new();
    if !qa.independent_pass
        || task.task_type == TaskType::Epic
        || task.labels.iter().any(|label| label == QA_PASS_LABEL)
    {
        return QaEligibility { reasons };
    }
    if !task.demo_statement.trim().is_empty() {
        reasons.push("demo_statement".to_string());
    }
    if let Some(label) = task.labels.iter().find(|label| {
        qa.user_facing_labels
            .iter()
            .any(|configured| configured.eq_ignore_ascii_case(label.trim()))
    }) {
        reasons.push(format!("label:{label}"));
    }
    if let Some((path, glob)) = first_user_facing_path(changed_paths, &qa.user_facing_paths) {
        reasons.push(format!("path:{path} ({glob})"));
    }
    QaEligibility { reasons }
}

/// First changed path matching a configured glob, with the glob it matched.
pub fn first_user_facing_path<'a>(
    changed_paths: &'a [String],
    globs: &'a [String],
) -> Option<(&'a str, &'a str)> {
    let options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    let patterns: Vec<(glob::Pattern, &str)> = globs
        .iter()
        .filter_map(|raw| {
            let raw = raw.trim();
            glob::Pattern::new(raw).ok().map(|pattern| (pattern, raw))
        })
        .collect();
    changed_paths.iter().find_map(|path| {
        patterns.iter().find_map(|(pattern, raw)| {
            // `**/x` must also match `x` at the repository root.
            let root_match = raw
                .strip_prefix("**/")
                .and_then(|rest| glob::Pattern::new(rest).ok())
                .is_some_and(|rest| rest.matches_with(path, options));
            (pattern.matches_with(path, options) || root_match).then_some((path.as_str(), *raw))
        })
    })
}

/// Paths the delivery changes: `merge-base(parent, branch)..branch`.
pub fn changed_paths_for_delivery(
    repo: &Path,
    parent_branch: &str,
    branch: &str,
) -> Result<Vec<String>, String> {
    let base = Command::new("git")
        .args(["merge-base", parent_branch, branch])
        .current_dir(repo)
        .output()
        .map_err(|error| format!("git merge-base failed to start: {error}"))?;
    if !base.status.success() {
        return Err(format!(
            "git merge-base {parent_branch} {branch} failed: {}",
            String::from_utf8_lossy(&base.stderr).trim()
        ));
    }
    let base = String::from_utf8_lossy(&base.stdout).trim().to_string();
    let diff = Command::new("git")
        .args(["diff", "--name-only", &format!("{base}..{branch}")])
        .current_dir(repo)
        .output()
        .map_err(|error| format!("git diff failed to start: {error}"))?;
    if !diff.status.success() {
        return Err(format!(
            "git diff --name-only {base}..{branch} failed: {}",
            String::from_utf8_lossy(&diff.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&diff.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

/// Title of the QA work item for one round.
pub fn qa_task_title(delivery: &Task, pass: &QaPass) -> String {
    let mut title = delivery.title.trim().to_string();
    if title.chars().count() > 80 {
        title = title.chars().take(77).collect::<String>() + "...";
    }
    format!(
        "QA pass (round {}): {title} @{}",
        pass.round,
        pass.head8()
    )
}

/// Ledger directory for one round, under the delivery's artifacts dir.
pub fn round_dir(artifacts_root: &Path, pass: &QaPass) -> std::path::PathBuf {
    artifacts_root
        .join(&pass.task_id)
        .join("qa")
        .join(format!("round-{}", pass.round))
}

/// Brief the reviewer reads when it starts the QA work item.
pub fn qa_task_description(
    delivery: &Task,
    pass: &QaPass,
    reasons: &[String],
    ledger_dir: &Path,
    parent_branch: &str,
) -> String {
    let demo = if delivery.demo_statement.trim().is_empty() {
        "(none — derive the user goal from the delivery title and diff)".to_string()
    } else {
        delivery.demo_statement.trim().to_string()
    };
    format!(
        "Independent QA and polish pass for delivery {task} — round {round}.\n\n\
         You are NOT the implementer ({implementer}); Cassy refuses the implementer here. \
         Review the running product, never edit the delivery. Follow the cas-qa-craft skill, \
         section \"Independent pass\".\n\n\
         - Delivery: {task} — {title}\n\
         - Demo statement: {demo}\n\
         - Branch: {branch} at {head} (review exactly this tip; base: {parent})\n\
         - Why this pass: {reasons}\n\
         - Ledger: {ledger}/LEDGER.md (evidence beside it)\n\
         - Deadline: {deadline} (pass {pass_id})\n\n\
         Steps: build and serve {head}; walk the journeys the diff touches \
         (scripts/journeys-for-diff.py {parent} {head}) plus the demo statement; \
         walk the adjacent paths (empty, loading, error, long content, phone 390px, dark, \
         keyboard-only, reduced motion); run visual-qa.mjs --strict and score the \
         cas-ui-craft rubric with desktop+phone, light+dark screenshots. Every finding \
         cites a trace action and a screenshot.\n\n\
         Record the verdict with: mcp__cas__verification action=qa_record task_id={task} \
         status=approved|rejected summary=\"...\" issues='[...]' ledger_path={ledger}/LEDGER.md \
         — a rejection sends {task} back to its implementer with your ledger.",
        task = delivery.id,
        round = pass.round,
        implementer = pass.implementer_agent_id,
        title = delivery.title.trim(),
        branch = pass.branch,
        head = pass.bound_head,
        parent = parent_branch,
        reasons = reasons.join(", "),
        ledger = ledger_dir.display(),
        deadline = pass.deadline_at.to_rfc3339(),
        pass_id = pass.id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> Task {
        Task::new("cas-ui1".to_string(), "Reply composer".to_string())
    }

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn eligibility_from_demo_label_or_path() {
        let qa = QaConfig::default();
        let mut plain = task();
        assert!(!delivery_eligibility(&plain, &qa, &paths(&["src/lib.rs"])).is_eligible());

        plain.demo_statement = "Type a reply and see it land".to_string();
        assert_eq!(
            delivery_eligibility(&plain, &qa, &[]).reasons,
            vec!["demo_statement".to_string()]
        );

        let mut labelled = task();
        labelled.labels = vec!["Hub".to_string()];
        assert_eq!(
            delivery_eligibility(&labelled, &qa, &[]).reasons,
            vec!["label:Hub".to_string()]
        );

        let pathy = task();
        let reasons = delivery_eligibility(&pathy, &qa, &paths(&["docs/x.md", "hub-web/src/a.css"]))
            .reasons;
        assert_eq!(reasons, vec!["path:hub-web/src/a.css (**/*.css)".to_string()]);
    }

    #[test]
    fn epics_qa_items_and_disabled_config_are_never_eligible() {
        let mut qa = QaConfig::default();
        let mut epic = task();
        epic.task_type = TaskType::Epic;
        epic.demo_statement = "demo".to_string();
        assert!(!delivery_eligibility(&epic, &qa, &[]).is_eligible());

        let mut qa_item = task();
        qa_item.labels = vec![QA_PASS_LABEL.to_string(), "ui".to_string()];
        assert!(!delivery_eligibility(&qa_item, &qa, &[]).is_eligible());

        let mut demo = task();
        demo.demo_statement = "demo".to_string();
        qa.independent_pass = false;
        assert!(!delivery_eligibility(&demo, &qa, &[]).is_eligible());
    }

    #[test]
    fn globs_match_nested_and_root_paths_only() {
        let globs = paths(&["**/*.css", "hub-web/**"]);
        assert_eq!(
            first_user_facing_path(&paths(&["styles.css"]), &globs),
            Some(("styles.css", "**/*.css"))
        );
        assert_eq!(
            first_user_facing_path(&paths(&["hub-web/src/main.ts"]), &globs),
            Some(("hub-web/src/main.ts", "hub-web/**"))
        );
        assert_eq!(first_user_facing_path(&paths(&["cas-cli/src/lib.rs"]), &globs), None);
        assert_eq!(first_user_facing_path(&paths(&["a.css"]), &[]), None);
    }
}
