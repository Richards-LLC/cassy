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
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
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
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, DIST, "(()=>base())();\n", "seed bundle");
    git(repo, &["checkout", "-qb", "factory/worker"]);
    commit(repo, DIST, "(()=>draft())();\n", "build(cas-test1): draft bundle");
    commit(repo, DIST, "(()=>finalBuild())();\n", "build(cas-test1): final bundle");
    if revert_on_lane {
        commit(repo, DIST, "(()=>base())();\n", "revert: abandon cas-test1 bundle");
    }
    git(repo, &["checkout", "-q", "main"]);
    commit(repo, "docs/sibling.md", "sibling\n", "docs(cas-aaaa): sibling delivery");
    git(repo, &["checkout", "-q", "factory/worker"]);
    git(repo, &["merge", "--no-ff", "main", "-m", "sync epic (cas-test1)"]);
    let anchor = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["checkout", "-q", "main"]);
    if drop_on_merge {
        git(repo, &["merge", "--no-ff", "-s", "ours", "factory/worker", "-m", "drop worker bundle"]);
    } else {
        git(repo, &["merge", "--no-ff", "factory/worker", "-m", "integrate worker"]);
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
    run_factory_branch_merge_gate(task, &TaskCloseRequest {
        id: task.id.clone(), reason: None, supervisor_override: None,
        legacy_bypass_code_review: None, search_manifest: None,
        commit_receipt: None, stranded_branch_override: None,
    }, "main", repo)
}

fn epic_row(repo: &Path, task: &Task) -> EpicChildBranchStatus {
    collect_epic_branch_statuses(std::slice::from_ref(task), "main", repo).remove(0)
}

#[test]
fn identical_minified_snapshot_close_and_epic_proof_cas_5f0b() {
    let (dir, task, _) = fixture(false, false);
    let repo = dir.path();
    let anchor = task.deliverables.factory_branch_anchor.as_deref().unwrap();
    assert_eq!(git(repo, &["rev-parse", &format!("{anchor}:{DIST}")]),
        git(repo, &["rev-parse", &format!("main:{DIST}")]));
    assert_eq!(git(repo, &["diff", "--name-only", &format!("{anchor}^1"), anchor]), "docs/sibling.md");
    match close_gate(repo, &task) {
        MergeStateGateOutcome::ProceedWithNote(note) => {
            assert!(note.contains("byte-identical"), "{note}");
            assert!(!note.contains("regenerated build artifact"), "{note}");
        }
        other => panic!("final snapshot requires exact-blob proof: {other:?}"),
    }
    let row = epic_row(repo, &task);
    assert!(!row.blocks_epic_close(), "{row:?}");
    assert!(row.content_evolution_note.as_deref().is_some_and(|note| note.contains("byte-identical")), "{row:?}");
}

#[test]
fn genuine_minified_drop_close_and_epic_reject_cas_5f0b() {
    let (dir, task, _) = fixture(true, false);
    match close_gate(dir.path(), &task) {
        MergeStateGateOutcome::Reject(message) => assert!(message.contains("DELIVERY CONTENT DROPPED") && message.contains(DIST), "{message}"),
        other => panic!("dropping integration must reject: {other:?}"),
    }
    let row = epic_row(dir.path(), &task);
    assert!(row.blocks_epic_close() && row.dropped_paths.contains(&DIST.into()), "{row:?}");
}

#[test]
fn identical_reverted_minified_snapshot_rejects_cas_5f0b() {
    let (dir, task, _) = fixture(false, true);
    assert!(matches!(close_gate(dir.path(), &task), MergeStateGateOutcome::Reject(_)));
    let row = epic_row(dir.path(), &task);
    assert!(row.blocks_epic_close() && row.dropped_paths.contains(&DIST.into()), "{row:?}");
}

#[test]
fn minified_drop_requires_audited_superseding_commit_cas_5f0b() {
    let (dir, task, integration) = fixture(true, false);
    let repo = dir.path();
    let anchor = task.deliverables.factory_branch_anchor.as_deref().unwrap();
    let paths = vec![DIST.into()];
    assert!(validated_delivery_drop_review(repo, anchor, "main", &paths,
        Some(&format!("reviewed-drop: {integration} -- merge threw away bundle"))).is_err());
    let replacement = commit(repo, DIST, "(()=>reviewedBuild())();\n", "build: reviewed replacement");
    let review = validated_delivery_drop_review(repo, anchor, "main", &paths,
        Some(&format!("reviewed-drop: {replacement} -- rebuilt reviewed bundle"))).unwrap().unwrap();
    assert!(review.contains(anchor) && review.contains(&replacement) && review.contains(DIST), "{review}");
    assert!(matches!(close_gate(repo, &task), MergeStateGateOutcome::Reject(_)));
    assert!(epic_row(repo, &task).blocks_epic_close());
}
