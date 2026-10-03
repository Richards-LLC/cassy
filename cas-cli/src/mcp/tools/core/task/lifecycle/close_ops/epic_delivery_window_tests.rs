use super::*;
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "CAS Test")
        .env("GIT_AUTHOR_EMAIL", "cas@example.test")
        .env("GIT_COMMITTER_NAME", "CAS Test")
        .env("GIT_COMMITTER_EMAIL", "cas@example.test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn commit(repo: &Path, paths: &[(&str, &str)], message: &str) -> String {
    for (path, body) in paths {
        std::fs::write(repo.join(path), body).unwrap();
        git(repo, &["add", path]);
    }
    git(repo, &["commit", "-q", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

#[test]
fn epic_window_excludes_unrelated_checkout_paths_and_snapshots_cas_b36b() {
    for remote_only in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        git(repo, &["init", "-q", "-b", "main"]);
        let base = commit(repo, &[("seed.txt", "base\n")], "base");
        git(repo, &["switch", "-q", "-c", "epic/cas-b36b"]);
        let epic_tip = commit(
            repo,
            &[
                ("epic.rs", "pub fn epic() {}\n"),
                ("epic.snap", "epic delivery\n"),
            ],
            "cas-b36b epic delivery",
        );
        git(repo, &["switch", "-q", "-c", "docs/unrelated", "main"]);
        let unrelated_tip = commit(
            repo,
            &[
                ("unrelated.rs", "pub fn unrelated() {}\n"),
                ("unrelated.snap", "unrelated snapshot\n"),
            ],
            "unrelated supervisor work",
        );
        if remote_only {
            git(
                repo,
                &["update-ref", "refs/remotes/origin/epic/cas-b36b", &epic_tip],
            );
            git(repo, &["branch", "-D", "epic/cas-b36b"]);
        }
        let mut task = Task::new("cas-b36b".into(), "epic window".into());
        task.task_type = TaskType::Epic;
        task.branch = Some("epic/cas-b36b".into());
        let window = TaskCommitReceiptWindow {
            supervisor_override_reason: None,
            not_before: chrono::Utc::now() - chrono::Duration::hours(1),
            basis: "test fixture",
            task_floor: chrono::Utc::now() - chrono::Duration::hours(2),
            identity: TaskCommitIdentity {
                task_id: Some(task.id.clone()),
                ..Default::default()
            },
        };
        let tip = close_task_delivery_tip(&task, repo, None, None, false)
            .unwrap()
            .unwrap();
        let paths = task_attribution::paths(repo, "main", &window, Some(&tip)).unwrap();
        assert_eq!(paths, vec!["epic.rs", "epic.snap"]);
        assert_eq!(tip, epic_tip);
        assert_ne!(tip, unrelated_tip);
        assert_eq!(
            close_task_delivery_tip(&task, repo, None, Some(&unrelated_tip), false).unwrap(),
            Some(epic_tip.clone())
        );
        let attributed_base =
            task_attribution::delivery_base(repo, "main", &window, Some(&tip)).unwrap();
        let range = snapshot_gate_range(
            repo,
            "main",
            Some(&attributed_base),
            Some(&tip),
            false,
            true,
        )
        .unwrap();
        assert_eq!(range.base.as_deref(), Some(base.as_str()));
        assert_eq!(range.tip.as_deref(), Some(epic_tip.as_str()));
        // Missing attribution must use the epic tip's merge-base, too.
        let fallback = snapshot_gate_range(repo, "main", None, Some(&tip), false, true).unwrap();
        assert_eq!(fallback, range);
        let refusal = snapshot_approval::rejection(
            repo,
            range.base.as_deref(),
            range.tip.as_deref(),
            &paths,
            "",
            &task.id,
            "",
        )
        .unwrap();
        assert!(refusal.contains("epic.snap"), "{refusal}");
        assert!(!refusal.contains("unrelated.snap"), "{refusal}");
        assert!(
            snapshot_approval::rejection(
                repo,
                range.base.as_deref(),
                range.tip.as_deref(),
                &paths,
                "snapshot-approved: epic.snap — +epic delivery — verified epic delivery",
                &task.id,
                ""
            )
            .is_none()
        );
    }
}

#[test]
fn epic_receipt_wins_and_unresolved_delivery_fails_closed_cas_b36b() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    git(repo, &["init", "-q", "-b", "main"]);
    let base = commit(repo, &[("seed.txt", "base\n")], "base");
    git(repo, &["branch", "epic/old"]);
    let receipt = commit(repo, &[("delivery.rs", "delivery\n")], "cas-b36b delivery");
    let mut task = Task::new("cas-b36b".into(), "epic".into());
    task.task_type = TaskType::Epic;
    task.branch = Some("epic/old".into());
    assert_eq!(
        close_task_delivery_tip(&task, repo, Some(&receipt), Some(&base), false).unwrap(),
        Some(receipt.clone())
    );
    assert!(close_task_delivery_tip(&task, repo, Some("missing-receipt"), None, false).is_err());
    task.branch = Some("epic/missing".into());
    let error = close_task_delivery_tip(&task, repo, None, Some(&receipt), false).unwrap_err();
    assert!(error.contains("EPIC DELIVERY TIP REQUIRED"), "{error}");
    assert!(error.contains("epic/missing"), "{error}");
    task.branch = None;
    assert!(close_task_delivery_tip(&task, repo, None, Some(&receipt), false).is_err());
    // Other task kinds retain their parked-anchor and checkout behavior.
    task.task_type = TaskType::Task;
    assert_eq!(
        close_task_delivery_tip(&task, repo, None, Some(&base), false).unwrap(),
        Some(base)
    );
    assert_eq!(
        close_task_delivery_tip(&task, repo, None, None, false).unwrap(),
        Some(receipt)
    );
}

#[test]
fn branchless_no_code_epic_has_no_delivery_window_cas_04c6() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    git(repo, &["init", "-q", "-b", "main"]);
    commit(
        repo,
        &[("unrelated.snap", "supervisor's unrelated snapshot\n")],
        "unrelated checkout work",
    );
    let mut epic = Task::new("cas-04c6".into(), "Planning epic".into());
    epic.task_type = TaskType::Epic;
    let tip = close_task_delivery_tip(&epic, repo, None, None, false).unwrap();
    assert_eq!(tip, None, "no code epic must never inherit checkout HEAD");
    assert!(
        snapshot_gate_range(repo, "main", None, tip.as_deref(), false, tip.is_some()).is_none()
    );
    let mut child = Task::new("cas-child".into(), "Delivered child".into());
    child.status = TaskStatus::Closed;
    child.deliverables.files_changed = vec!["child.rs".into()];
    let error =
        close_task_delivery_tip(&epic, repo, None, None, has_recorded_code_delivery(&child))
            .unwrap_err();
    assert!(error.contains("EPIC DELIVERY TIP REQUIRED"), "{error}");
    epic.branch = Some("epic/missing".into());
    let error = close_task_delivery_tip(&epic, repo, None, None, false).unwrap_err();
    assert!(error.contains("EPIC DELIVERY TIP REQUIRED"), "{error}");
    assert!(error.contains("epic/missing"), "{error}");
}
