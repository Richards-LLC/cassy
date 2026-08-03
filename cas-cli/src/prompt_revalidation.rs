#[cfg(test)]
mod tests {
    use super::*;
    use cas_types::TaskStatus;
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
        git(repo.path(), &["config", "user.email", "cas-test@example.invalid"]);
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
    fn unstructured_messages_are_not_revalidated() {
        assert_eq!(parse_merge_request_envelope("ordinary free-form message"), None);
        assert_eq!(parse_lifecycle_envelope("ordinary free-form message"), None);
    }
}
