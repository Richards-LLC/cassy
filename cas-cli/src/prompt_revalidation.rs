use cas_types::{Task, TaskStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::str::FromStr;

const MERGE_ENVELOPE_OPEN: &str = "<cas-merge-request>";
const MERGE_ENVELOPE_CLOSE: &str = "</cas-merge-request>";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MergeRequestEnvelope {
    pub task_id: String,
    pub branch_tip: String,
    pub target_branch: String,
    pub target_branch_tip: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergeRequestDecision {
    Pending { target_tip: String },
    AlreadyIntegrated { target_tip: String },
    Unverifiable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LifecycleEnvelope {
    pub task_id: String,
    pub new_status: TaskStatus,
    pub occurrence: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LifecyclePromptDecision {
    Deliver,
    SuppressStale { task_id: String },
    Unstructured,
}

pub(crate) fn select_unambiguous_merge_task<'a>(
    parked_tasks: &'a [Task],
    worker: &str,
    explicit_task_id: Option<&str>,
) -> Option<&'a Task> {
    let mut matches = parked_tasks.iter().filter(|task| {
        task.status == TaskStatus::AwaitingMerge
            && task
                .assignee
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(worker))
            && explicit_task_id.is_none_or(|id| task.id == id)
    });
    let only = matches.next()?;
    matches.next().is_none().then_some(only)
}

pub(crate) fn revalidate_merge_request(
    repo_path: &Path,
    branch_tip: &str,
    target_branch: &str,
) -> MergeRequestDecision {
    let Some(target_tip) = crate::mcp::tools::core::task::lifecycle::close_ops::resolve_branch_sha(
        repo_path,
        target_branch,
    ) else {
        return MergeRequestDecision::Unverifiable;
    };
    if crate::mcp::tools::core::task::lifecycle::close_ops::git_commit_is_ancestor(
        repo_path,
        branch_tip,
        &target_tip,
    ) {
        MergeRequestDecision::AlreadyIntegrated { target_tip }
    } else {
        MergeRequestDecision::Pending { target_tip }
    }
}

pub(crate) fn attach_merge_request_envelope(
    message: &str,
    envelope: &MergeRequestEnvelope,
) -> String {
    let encoded = serde_json::to_string(envelope)
        .expect("MergeRequestEnvelope contains only JSON-serializable strings");
    format!("{message}\n\n{MERGE_ENVELOPE_OPEN}{encoded}{MERGE_ENVELOPE_CLOSE}")
}

pub(crate) fn merge_landed_guidance(
    task_id: &str,
    branch_tip: &str,
    target_branch: &str,
    target_tip: &str,
) -> String {
    format!(
        "Merge already landed; CAS suppressed your stale merge request.\n\n\
         Task: {task_id}\nBranch tip: {branch_tip}\nTarget: {target_branch} at {target_tip}\n\n\
         Re-run task close for {task_id} now."
    )
}

pub(crate) fn parse_merge_request_envelope(prompt: &str) -> Option<MergeRequestEnvelope> {
    let start = prompt.rfind(MERGE_ENVELOPE_OPEN)? + MERGE_ENVELOPE_OPEN.len();
    let end = prompt[start..].find(MERGE_ENVELOPE_CLOSE)? + start;
    serde_json::from_str(&prompt[start..end]).ok()
}

fn xml_attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

pub(crate) fn parse_lifecycle_envelope(prompt: &str) -> Option<LifecycleEnvelope> {
    let tag_end = prompt.find('>')?;
    let tag = &prompt[..tag_end];
    if !tag.starts_with("<task-lifecycle ") {
        return None;
    }
    let task_id = xml_attribute(tag, "task_id")?.to_string();
    let new_status = TaskStatus::from_str(xml_attribute(tag, "new")?).ok()?;
    let occurrence = DateTime::parse_from_rfc3339(xml_attribute(tag, "occurrence")?)
        .ok()?
        .with_timezone(&Utc);
    Some(LifecycleEnvelope {
        task_id,
        new_status,
        occurrence,
    })
}

pub(crate) fn revalidate_lifecycle_prompt(
    prompt: &str,
    current_status: TaskStatus,
    current_updated_at: DateTime<Utc>,
) -> LifecyclePromptDecision {
    let Some(envelope) = parse_lifecycle_envelope(prompt) else {
        return LifecyclePromptDecision::Unstructured;
    };
    if current_status == envelope.new_status && current_updated_at == envelope.occurrence {
        LifecyclePromptDecision::Deliver
    } else {
        LifecyclePromptDecision::SuppressStale {
            task_id: envelope.task_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::process::Command;

    fn git(repo: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git command");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn merge_then_stale_request_is_suppressed_with_current_target_tip() {
        let repo = tempfile::tempdir().expect("temp repo");
        git(repo.path(), &["init", "-b", "main"]);
        git(
            repo.path(),
            &["config", "user.email", "cas-test@example.invalid"],
        );
        git(repo.path(), &["config", "user.name", "CAS Test"]);
        std::fs::write(repo.path().join("base"), "base\n").expect("base file");
        git(repo.path(), &["add", "base"]);
        git(repo.path(), &["commit", "-m", "base"]);
        git(repo.path(), &["checkout", "-b", "factory/test-worker"]);
        std::fs::write(repo.path().join("work"), "work\n").expect("work file");
        git(repo.path(), &["add", "work"]);
        git(repo.path(), &["commit", "-m", "work"]);
        let worker_tip = git(repo.path(), &["rev-parse", "factory/test-worker"]);

        // Reproduce the production ordering: the supervisor merges first, but
        // the worker's already-composed merge request reaches delivery later.
        git(repo.path(), &["checkout", "main"]);
        git(
            repo.path(),
            &["merge", "--no-ff", "factory/test-worker", "-m", "merge"],
        );
        let target_tip = git(repo.path(), &["rev-parse", "main"]);

        assert_eq!(
            revalidate_merge_request(repo.path(), &worker_tip, "main"),
            MergeRequestDecision::AlreadyIntegrated {
                target_tip: target_tip.clone(),
            }
        );

        let envelope = MergeRequestEnvelope {
            task_id: "cas-test".to_string(),
            branch_tip: worker_tip,
            target_branch: "main".to_string(),
            target_branch_tip: target_tip,
        };
        assert_eq!(
            parse_merge_request_envelope(&attach_merge_request_envelope("please merge", &envelope)),
            Some(envelope)
        );
        let guidance = merge_landed_guidance(
            "cas-test",
            &git(repo.path(), &["rev-parse", "factory/test-worker"]),
            "main",
            &git(repo.path(), &["rev-parse", "main"]),
        );
        assert!(guidance.contains("Merge already landed"));
        assert!(guidance.contains("Re-run task close for cas-test now"));
    }

    #[test]
    fn genuinely_unmerged_request_remains_pending() {
        let repo = tempfile::tempdir().expect("temp repo");
        git(repo.path(), &["init", "-b", "main"]);
        git(
            repo.path(),
            &["config", "user.email", "cas-test@example.invalid"],
        );
        git(repo.path(), &["config", "user.name", "CAS Test"]);
        std::fs::write(repo.path().join("base"), "base\n").expect("base file");
        git(repo.path(), &["add", "base"]);
        git(repo.path(), &["commit", "-m", "base"]);
        let target_tip = git(repo.path(), &["rev-parse", "main"]);
        git(repo.path(), &["checkout", "-b", "factory/test-worker"]);
        std::fs::write(repo.path().join("work"), "work\n").expect("work file");
        git(repo.path(), &["add", "work"]);
        git(repo.path(), &["commit", "-m", "work"]);
        let worker_tip = git(repo.path(), &["rev-parse", "factory/test-worker"]);

        assert_eq!(
            revalidate_merge_request(repo.path(), &worker_tip, "main"),
            MergeRequestDecision::Pending { target_tip }
        );
    }

    #[test]
    fn stale_blocked_occurrence_is_suppressed_after_task_resumes() {
        let occurrence = Utc
            .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
            .single()
            .expect("timestamp");
        let prompt = format!(
            "<task-lifecycle transition=\"task_blocked\" task_id=\"cas-test\" old=\"in_progress\" new=\"blocked\" actor=\"worker\" notification_id=\"1\" occurrence=\"{}\">\nTask blocked\n</task-lifecycle>",
            occurrence.to_rfc3339()
        );
        let current_updated_at = occurrence + chrono::Duration::seconds(1);

        assert_eq!(
            revalidate_lifecycle_prompt(&prompt, TaskStatus::InProgress, current_updated_at),
            LifecyclePromptDecision::SuppressStale {
                task_id: "cas-test".to_string(),
            }
        );
    }

    #[test]
    fn current_blocked_occurrence_is_delivered() {
        let occurrence = Utc
            .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
            .single()
            .expect("timestamp");
        let prompt = format!(
            "<task-lifecycle transition=\"task_blocked\" task_id=\"cas-test\" old=\"in_progress\" new=\"blocked\" actor=\"worker\" notification_id=\"1\" occurrence=\"{}\">\nTask blocked\n</task-lifecycle>",
            occurrence.to_rfc3339()
        );
        assert_eq!(
            revalidate_lifecycle_prompt(&prompt, TaskStatus::Blocked, occurrence),
            LifecyclePromptDecision::Deliver
        );
    }

    #[test]
    fn unstructured_messages_are_not_revalidated() {
        assert_eq!(
            parse_merge_request_envelope("ordinary free-form message"),
            None
        );
        assert_eq!(parse_lifecycle_envelope("ordinary free-form message"), None);
    }

    #[test]
    fn exactly_one_parked_task_is_inferred_but_ambiguity_is_left_unstructured() {
        let mut first = cas_types::Task::new("cas-one".to_string(), "one".to_string());
        first.status = TaskStatus::AwaitingMerge;
        first.assignee = Some("worker-a".to_string());
        let mut second = cas_types::Task::new("cas-two".to_string(), "two".to_string());
        second.status = TaskStatus::AwaitingMerge;
        second.assignee = Some("worker-a".to_string());

        assert_eq!(
            select_unambiguous_merge_task(&[first.clone()], "worker-a", None)
                .map(|task| task.id.as_str()),
            Some("cas-one")
        );
        assert!(
            select_unambiguous_merge_task(&[first.clone(), second], "worker-a", None).is_none(),
            "ambiguous implicit merge context must preserve free-form delivery"
        );
        assert_eq!(
            select_unambiguous_merge_task(&[first], "worker-a", Some("cas-one"))
                .map(|task| task.id.as_str()),
            Some("cas-one")
        );
    }
}
