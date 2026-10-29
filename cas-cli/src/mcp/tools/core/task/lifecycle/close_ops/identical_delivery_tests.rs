//! Final minified delivery snapshots and their preceding history (cas-5f0b).
use super::*;
use std::path::Path;
use std::process::Command;

const DIST: &str = "hub-web/dist/app.js";

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit(repo: &Path, path: &str, contents: &str, subject: &str) -> String {
    let file = repo.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, contents).unwrap();
    git(repo, &["add", path]);
    git(repo, &["commit", "-qm", subject]);
    git(repo, &["rev-parse", "HEAD"])
}

// The sync merge imports sibling docs; its first-parent effect is not dist.
fn fixture(drop_on_merge: bool, revert_on_lane: bool) -> (tempfile::TempDir, Task, String) {
    fixture_with_return(
        drop_on_merge,
        revert_on_lane.then_some(("(()=>base())();\n", "revert: abandon cas-test1 bundle")),
    )
}

fn fixture_with_return(
    drop_on_merge: bool,
    return_to: Option<(&str, &str)>,
) -> (tempfile::TempDir, Task, String) {
    fixture_with_subjects(drop_on_merge, return_to, true, DIST, false)
}

fn fixture_with_subjects(
    drop_on_merge: bool,
    return_to: Option<(&str, &str)>,
    named: bool,
    delivery_path: &str,
    rebuild_on_sync: bool,
) -> (tempfile::TempDir, Task, String) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, delivery_path, "(()=>base())();\n", "seed bundle");
    git(repo, &["checkout", "-qb", "factory/worker"]);
    commit(
        repo,
        delivery_path,
        "(()=>draft())();\n",
        if named { "build(cas-test1): draft bundle" } else { "build: draft bundle" },
    );
    git(repo, &["checkout", "-q", "main"]);
    commit(
        repo,
        delivery_path,
        "(()=>epicBuild())();\n",
        "build(cas-aaaa): epic bundle",
    );
    git(repo, &["checkout", "-q", "factory/worker"]);
    // Recovery discards the obsolete draft in favor of the imported epic
    // bundle. The later reviewed rebuild is the task's final delivery.
    git(
        repo,
        &["merge", "--no-ff", "--no-commit", "-s", "ours", "main"],
    );
    commit(
        repo,
        delivery_path,
        "(()=>epicBuild())();\n",
        if named { "merge(cas-test1): recover onto epic" } else { "merge: recover onto epic" },
    );
    commit(
        repo,
        delivery_path,
        "(()=>finalBuild())();\n",
        if named { "build(cas-test1): final bundle" } else { "build: final bundle" },
    );
    if let Some((contents, subject)) = return_to {
        commit(repo, delivery_path, contents, subject);
    }
    git(repo, &["checkout", "-q", "main"]);
    commit(
        repo,
        "docs/sibling.md",
        "sibling\n",
        "docs(cas-aaaa): sibling delivery",
    );
    git(repo, &["checkout", "-q", "factory/worker"]);
    git(
        repo,
        &["merge", "--no-ff", "main", "-m", if named { "sync epic (cas-test1)" } else { "sync epic" }],
    );
    if rebuild_on_sync {
        // A generated merge driver can produce output unlike either parent.
        std::fs::write(repo.join(delivery_path), "(()=>syncBuild())();\n").unwrap();
        git(repo, &["add", delivery_path]);
        git(repo, &["commit", "--amend", "--no-edit", "-q"]);
    }
    let anchor = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["checkout", "-q", "main"]);
    if drop_on_merge {
        git(
            repo,
            &[
                "merge",
                "--no-ff",
                "-s",
                "ours",
                "factory/worker",
                "-m",
                "drop worker bundle",
            ],
        );
    } else {
        git(
            repo,
            &[
                "merge",
                "--no-ff",
                "factory/worker",
                "-m",
                "integrate worker",
            ],
        );
    }
    let integration = git(repo, &["rev-parse", "HEAD"]);
    let mut task = Task {
        id: "cas-test1".into(),
        title: "minified bundle delivery".into(),
        status: TaskStatus::AwaitingMerge,
        assignee: Some("worker".into()),
        created_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        ..Default::default()
    };
    task.deliverables.parked_branch = Some("factory/worker".into());
    task.deliverables.factory_branch_anchor = Some(anchor);
    (dir, task, integration)
}

fn close_gate(repo: &Path, task: &Task) -> MergeStateGateOutcome {
    run_factory_branch_merge_gate(
        task,
        &TaskCloseRequest {
            no_delivery: None,
            id: task.id.clone(),
            reason: None,
            supervisor_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: None,
            stranded_branch_override: None,
        },
        "main",
        repo,
    )
}

fn epic_row(repo: &Path, task: &Task) -> EpicChildBranchStatus {
    collect_epic_branch_statuses(std::slice::from_ref(task), "main", repo).remove(0)
}

#[test]
fn identical_minified_snapshot_close_and_epic_proof_cas_5f0b() {
    let (dir, task, _) = fixture(false, false);
    let repo = dir.path();
    let anchor = task.deliverables.factory_branch_anchor.as_deref().unwrap();
    assert_eq!(
        git(repo, &["rev-parse", &format!("{anchor}:{DIST}")]),
        git(repo, &["rev-parse", &format!("main:{DIST}")])
    );
    assert_eq!(
        git(
            repo,
            &["diff", "--name-only", &format!("{anchor}^1"), anchor]
        ),
        "docs/sibling.md"
    );
    match close_gate(repo, &task) {
        MergeStateGateOutcome::ProceedWithNote(note) => {
            assert!(note.contains("byte-identical"), "{note}");
            assert!(!note.contains("regenerated build artifact"), "{note}");
        }
        other => panic!("final snapshot requires exact-blob proof: {other:?}"),
    }
    let row = epic_row(repo, &task);
    assert!(!row.blocks_epic_close(), "{row:?}");
    assert!(
        row.content_evolution_note
            .as_deref()
            .is_some_and(|note| note.contains("byte-identical")),
        "{row:?}"
    );
}

#[test]
fn genuine_minified_drop_close_and_epic_reject_cas_5f0b() {
    let (dir, task, _) = fixture(true, false);
    match close_gate(dir.path(), &task) {
        MergeStateGateOutcome::Reject(message) => assert!(
            message.contains("DELIVERY CONTENT DROPPED") && message.contains(DIST),
            "{message}"
        ),
        other => panic!("dropping integration must reject: {other:?}"),
    }
    let row = epic_row(dir.path(), &task);
    assert!(
        row.blocks_epic_close() && row.dropped_paths.contains(&DIST.into()),
        "{row:?}"
    );
}

#[test]
fn identical_reverted_minified_snapshot_rejects_cas_5f0b() {
    let (dir, task, _) = fixture(false, true);
    assert!(matches!(
        close_gate(dir.path(), &task),
        MergeStateGateOutcome::Reject(_)
    ));
    let row = epic_row(dir.path(), &task);
    assert!(
        row.blocks_epic_close() && row.dropped_paths.contains(&DIST.into()),
        "{row:?}"
    );
}

#[test]
fn minified_drop_requires_audited_superseding_commit_cas_5f0b() {
    let (dir, task, integration) = fixture(true, false);
    let repo = dir.path();
    let anchor = task.deliverables.factory_branch_anchor.as_deref().unwrap();
    let paths = vec![DIST.into()];
    assert!(
        validated_delivery_drop_review(
            repo,
            anchor,
            "main",
            &paths,
            Some(&format!(
                "reviewed-drop: {integration} -- merge threw away bundle"
            ))
        )
        .is_err()
    );
    let replacement = commit(
        repo,
        DIST,
        "(()=>reviewedBuild())();\n",
        "build: reviewed replacement",
    );
    let review = validated_delivery_drop_review(
        repo,
        anchor,
        "main",
        &paths,
        Some(&format!(
            "reviewed-drop: {replacement} -- rebuilt reviewed bundle"
        )),
    )
    .unwrap()
    .unwrap();
    assert!(
        review.contains(anchor) && review.contains(&replacement) && review.contains(DIST),
        "{review}"
    );
    let identity = task_commit_identity(&task, None);
    let reason = format!("reviewed-drop: {replacement} -- rebuilt reviewed bundle");
    match anchored_delivery_content_gate(
        &task,
        repo,
        anchor,
        "main",
        None,
        &identity,
        None,
        Some(&reason),
    ) {
        Some(MergeStateGateOutcome::ProceedWithNote(note)) => {
            assert!(
                note.contains(anchor) && note.contains(&replacement) && note.contains(DIST),
                "{note}"
            );
        }
        other => panic!("a valid supersession must keep an audited decision: {other:?}"),
    }

    assert!(matches!(
        close_gate(repo, &task),
        MergeStateGateOutcome::Reject(_)
    ));
    assert!(epic_row(repo, &task).blocks_epic_close());
}

#[test]
fn later_lane_task_cannot_supply_old_minified_snapshot_cas_5f0b() {
    let (dir, task, _) = fixture(true, false);
    let repo = dir.path();
    git(repo, &["checkout", "-q", "factory/worker"]);
    commit(
        repo,
        DIST,
        "(()=>nextTask())();\n",
        "build(cas-aaaa): later lane task",
    );
    git(repo, &["checkout", "-q", "main"]);
    // The side carries a different bundle; resolve to that final later task.
    git(
        repo,
        &[
            "merge",
            "--no-ff",
            "--no-commit",
            "-s",
            "ours",
            "factory/worker",
        ],
    );
    commit(
        repo,
        DIST,
        "(()=>nextTask())();\n",
        "integrate later lane task",
    );
    let row = epic_row(repo, &task);
    assert!(
        row.blocks_epic_close() && row.dropped_paths.contains(&DIST.into()),
        "{row:?}"
    );
}

#[test]
fn imported_baseline_inverse_close_and_epic_reject_cas_5f0b() {
    // Both explicit and unlabeled inverses remove the task's final bundle.
    // The imported epic baseline differs from the task's earliest baseline,
    // so final-target equality and earliest-base inequality are insufficient.
    for subject in [
        "revert: abandon cas-test1 bundle",
        "build(cas-test1): restore imported bundle",
    ] {
        let (dir, task, _) = fixture_with_return(false, Some(("(()=>epicBuild())();\n", subject)));
        let repo = dir.path();
        let anchor = task.deliverables.factory_branch_anchor.as_deref().unwrap();
        let root = git(repo, &["rev-list", "--max-parents=0", "HEAD"]);
        let target_blob = git(repo, &["rev-parse", &format!("main:{DIST}")]);
        assert_eq!(
            target_blob,
            git(repo, &["rev-parse", &format!("{anchor}:{DIST}")])
        );
        assert_ne!(
            target_blob,
            git(repo, &["rev-parse", &format!("{root}:{DIST}")])
        );
        match close_gate(repo, &task) {
            MergeStateGateOutcome::Reject(message) => assert!(
                message.contains("DELIVERY CONTENT DROPPED") && message.contains(DIST),
                "{subject}: {message}"
            ),
            other => panic!(
                "an imported baseline is not this task's final delivery: {subject}: {other:?}"
            ),
        }
        let row = epic_row(repo, &task);
        assert!(
            row.blocks_epic_close() && row.dropped_paths.contains(&DIST.into()),
            "{subject}: {row:?}"
        );
    }
}

#[test]
fn attributed_shared_helper_survival_close_and_epic_cas_5f0b() {
    for remove_all in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        commit(repo, "shared.rs", "original();\n", "seed shared helper");
        git(repo, &["checkout", "-qb", "factory/worker"]);
        commit(
            repo,
            "worker.rs",
            "task();\n",
            "feat(cas-test1): worker delivery",
        );
        git(repo, &["checkout", "-qb", "factory/other", "main"]);
        commit(
            repo,
            "shared.rs",
            "imported();\n",
            "chore(cas-aaaa): newer baseline",
        );
        commit(
            repo,
            "shared.rs",
            "imported();\nadded();\nremoved();\n",
            "chore: shared helper\n\nUnblocks cas-test1.",
        );
        commit(
            repo,
            "shared.rs",
            if remove_all {
                "imported();\n"
            } else {
                "imported();\nadded();\n"
            },
            "chore: trim helper",
        );
        git(repo, &["checkout", "-q", "main"]);
        git(
            repo,
            &[
                "merge",
                "--no-ff",
                "factory/other",
                "-m",
                "integrate shared helper",
            ],
        );
        git(repo, &["checkout", "-q", "factory/worker"]);
        git(
            repo,
            &["merge", "--no-ff", "main", "-m", "sync epic (cas-test1)"],
        );
        let anchor = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "main"]);
        git(
            repo,
            &[
                "merge",
                "--no-ff",
                "factory/worker",
                "-m",
                "integrate worker",
            ],
        );
        let mut task = Task {
            id: "cas-test1".into(),
            title: "shared helper delivery".into(),
            status: TaskStatus::AwaitingMerge,
            assignee: Some("worker".into()),
            created_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
            ..Default::default()
        };
        task.deliverables.parked_branch = Some("factory/worker".into());
        task.deliverables.factory_branch_anchor = Some(anchor);
        match (remove_all, close_gate(repo, &task)) {
            (false, MergeStateGateOutcome::Proceed | MergeStateGateOutcome::ProceedWithNote(_)) => {
            }
            (true, MergeStateGateOutcome::Reject(message)) => assert!(
                message.contains("DELIVERY CONTENT DROPPED") && message.contains("shared.rs"),
                "{message}"
            ),
            (_, other) => {
                panic!("side-parent attributed effect remove_all={remove_all}: {other:?}")
            }
        }
        let row = epic_row(repo, &task);
        assert_eq!(row.blocks_epic_close(), remove_all, "{row:?}");
    }
}

#[test]
fn missing_attributed_base_keeps_identical_delivery_but_rejects_drops_cas_3474() {
    for mode in ["present", "source present", "restarted", "rebuilt", "dropped", "missing", "reverted", "imported baseline"] {
        let return_to = match mode {
            "reverted" => Some(("(()=>base())();\n", "revert: abandon bundle")),
            "imported baseline" => Some(("(()=>epicBuild())();\n", "build: restore imported bundle")),
            _ => None,
        };
        let path = if mode == "source present" { "docs/design/brief.md" } else { DIST };
        let (dir, task, _) = fixture_with_subjects(mode == "dropped", return_to, false, path, mode == "rebuilt");
        let repo = dir.path();
        if mode == "missing" {
            git(repo, &["rm", path]);
            git(repo, &["commit", "-qm", "remove integrated bundle"]);
        }
        let anchor = task.deliverables.factory_branch_anchor.as_deref().unwrap();
        let window = TaskCommitReceiptWindow {
            supervisor_override_reason: None,
            not_before: if mode == "restarted" {
                chrono::Utc::now() + chrono::Duration::hours(1)
            } else {
                chrono::DateTime::from_timestamp(0, 0).unwrap()
            },
            task_floor: chrono::DateTime::from_timestamp(0, 0).unwrap(),
            basis: "unnamed delivery before integrated sync anchor",
            identity: task_commit_identity(&task, None),
        };
        assert!(task_attribution::delivery_base(repo, "main", &window, Some(anchor)).is_none(),
            "fixture must reproduce unavailable attributed delivery base");
        assert_eq!(
            tree_path_blob(repo, anchor, path).unwrap() == tree_path_blob(repo, "main", path).unwrap(),
            !matches!(mode, "dropped" | "missing"),
        );
        let result = run_factory_branch_merge_gate_with_attribution(
            &task,
            &TaskCloseRequest { id: task.id.clone(), no_delivery: None, reason: None,
                supervisor_override: None, legacy_bypass_code_review: None,
                search_manifest: None, commit_receipt: None, stranded_branch_override: None },
            "main", repo, TaskCommitAttribution { receipt: None, window: Some(&window) },
        );
        if matches!(mode, "dropped" | "missing" | "reverted" | "imported baseline") {
            assert!(matches!(result, MergeStateGateOutcome::Reject(_)), "{mode}: {result:?}");
        } else {
            assert!(matches!(result, MergeStateGateOutcome::Proceed | MergeStateGateOutcome::ProceedWithNote(_)),
                "{mode}: identical final blobs survive missing attribution base: {result:?}");
        }
    }
}
