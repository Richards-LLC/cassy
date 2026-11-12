use super::*;
use crate::config::RegistryEnforcement;
use cas_store::TaskStore;
use std::process::Command;

struct Fixture {
    repo: tempfile::TempDir,
    _db: tempfile::TempDir,
    store: cas_store::SqliteTaskStore,
    task: Task,
    base: String,
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "upkeep fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "upkeep fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn write(repo: &Path, path: &str, body: &str) {
    let path = repo.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}
fn commit(repo: &Path) -> String {
    git(repo, &["add", "."]);
    git(
        repo,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "fixture"],
    );
    git(repo, &["rev-parse", "HEAD"])
}
impl Fixture {
    fn new() -> Self {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        write(repo.path(), "src/pages/signup.ts", "signup");
        let base = commit(repo.path());
        let db = tempfile::tempdir().unwrap();
        let store = cas_store::SqliteTaskStore::open(db.path()).unwrap();
        store.init().unwrap();
        let task = Task::new("cas-1234".into(), "Entry change".into());
        store.add(&task).unwrap();
        Self {
            repo,
            _db: db,
            store,
            task,
            base,
        }
    }
}

#[test]
fn registry_upkeep_mismatched_merge_parent_uses_task_only_delta() {
    for reused_lane in [false, true] {
        let mut f = Fixture::new();
        let repo = f.repo.path();
        git(repo, &["checkout", "-qb", "factory/worker"]);
        write(repo, "src/pages/signup.ts", "repaired signup");
        let parked = commit(repo);
        // Preserve unnamed delivery work; a reused lane may also contain a successor.
        let successor = if reused_lane {
            write(repo, "src/pages/foreign.ts", "next task's unowned surface");
            commit(repo)
        } else {
            parked.clone()
        };
        git(repo, &["checkout", "main"]);
        write(repo, "README.md", "target work");
        let target_base = commit(repo);
        git(
            repo,
            &[
                "merge",
                "--no-ff",
                "factory/worker",
                "-m",
                "integrate stacked lane",
            ],
        );
        let integrated = git(repo, &["rev-parse", "HEAD"]);
        assert_eq!(git(repo, &["rev-parse", "HEAD^2"]), successor);
        write(repo, "hub-web/dist/app.js", "generated follow-up");
        let published = commit(repo);
        f.task.status = TaskStatus::AwaitingMerge;
        f.task.deliverables.factory_branch_anchor = Some(parked.clone());
        f.task.deliverables.green_merge_commit = Some(published);
        let window = TaskCommitReceiptWindow {
            supervisor_override_reason: None,
            not_before: chrono::DateTime::UNIX_EPOCH,
            task_floor: chrono::DateTime::UNIX_EPOCH,
            basis: "fixture",
            identity: TaskCommitIdentity {
                superseded_commits: Vec::new(),
                approved_merge_parents: Vec::new(),
                start_base: Some(f.base.clone()),
                task_id: Some(f.task.id.clone()),
                known_commits: vec![parked.clone()],
            },
        };
        let (base, head, paths) =
            delivery_snapshot(&f.task, repo, "main", Some(&window), Some(&parked)).unwrap();
        if reused_lane {
            assert_eq!(head, parked);
            assert_eq!(base, f.base);
            assert_eq!(paths, Some(vec!["src/pages/signup.ts".into()]));
        } else {
            assert_eq!(head, integrated);
            assert_eq!(git(repo, &["rev-parse", &base]), target_base);
            assert_eq!(paths, None);
        }
    }
}

#[test]
fn accepted_finding_subset_survives_merge_without_expanding_authority_cas_5bbe() {
    let mut f = Fixture::new();
    git(f.repo.path(), &["checkout", "-qb", "delivery"]);
    write(f.repo.path(), "src/gates/check.rs", "gate");
    write(
        f.repo.path(),
        "docs/product-overview.d/entry.md",
        "overview",
    );
    let head = commit(f.repo.path());
    let input = "docs/report.html";
    let finding = |detail: &str| Finding {
        gate: "G7",
        token: "report quality waived:",
        title: "REPORT NOT READY FOR THE OPERATOR",
        detail: detail.into(),
    };
    let override_notices = finish(
        &mut CloseNoteTracker::new(&f.store, &f.task),
        &mut f.task,
        f.repo.path(),
        &head,
        RegistryEnforcement::Enforce,
        input,
        vec![finding("one-call finding")],
        &[],
        Some("reviewed for this call"),
    )
    .unwrap();
    assert_eq!(
        crate::gates::override_codes_in(&override_notices.join("\n")),
        vec!["G-51"]
    );
    assert!(
        finish(
            &mut CloseNoteTracker::new(&f.store, &f.task),
            &mut f.task,
            f.repo.path(),
            &head,
            RegistryEnforcement::Enforce,
            input,
            vec![finding("one-call finding")],
            &[],
            None
        )
        .is_err()
    );
    let original = finding("src/gates\ndocs/product-overview.d");
    append_close_decision_note(
        &f.store,
        &mut f.task,
        &format!(
            "✅ DECISION {} reviewed report omissions",
            original.scoped_token(input, &head)
        ),
    );
    finish(
        &mut CloseNoteTracker::new(&f.store, &f.task),
        &mut f.task,
        f.repo.path(),
        &head,
        RegistryEnforcement::Enforce,
        input,
        vec![original],
        &[],
        None,
    )
    .unwrap();
    git(f.repo.path(), &["checkout", "-q", "main"]);
    git(
        f.repo.path(),
        &["merge", "-q", "--no-ff", "delivery", "-m", "land delivery"],
    );
    assert!(
        finish(
            &mut CloseNoteTracker::new(&f.store, &f.task),
            &mut f.task,
            f.repo.path(),
            &head,
            RegistryEnforcement::Enforce,
            input,
            vec![finding("src/gates")],
            &[],
            None
        )
        .is_ok()
    );
    let merge = git(f.repo.path(), &["rev-parse", "HEAD"]);
    for (scope, path, delivery) in [
        ("src/gates\nsrc/other", input, head.as_str()),
        ("src/gates", "docs/OTHER.md", head.as_str()),
        ("src/gates", input, merge.as_str()),
    ] {
        assert!(
            finish(
                &mut CloseNoteTracker::new(&f.store, &f.task),
                &mut f.task,
                f.repo.path(),
                delivery,
                RegistryEnforcement::Enforce,
                path,
                vec![finding(scope)],
                &[],
                None
            )
            .is_err()
        );
    }
}
