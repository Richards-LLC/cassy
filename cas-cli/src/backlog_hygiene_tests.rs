//! cas-e8d1 (GH #1233): one test per detector, plus the scan over a real
//! git repository and task store.

use super::*;
use crate::types::TaskStatus;
use chrono::Duration;

fn task(id: &str, status: TaskStatus, task_type: TaskType, updated_days_ago: i64) -> Task {
    let mut task = Task::new(id.to_string(), format!("title of {id}"));
    task.status = status;
    task.task_type = task_type;
    task.updated_at = Utc::now() - Duration::days(updated_days_ago);
    task.created_at = task.updated_at;
    task
}

fn commit(sha: &str, subject: &str, days_ago: i64) -> CommitRecord {
    CommitRecord {
        sha: sha.to_string(),
        committed_at: Utc::now() - Duration::days(days_ago),
        subject: subject.to_string(),
    }
}

fn child(child: &str, parent: &str) -> Dependency {
    Dependency::new(child.to_string(), parent.to_string(), DependencyType::ParentChild)
}

/// Kind 1: an open task whose subject-named commit is on the default branch
/// is reported with that commit; a closed one, an epic, a body-only mention
/// and a prefix collision are not.
#[test]
fn delivered_open_names_the_commit_that_delivered_it() {
    let tasks = vec![
        task("cas-a1", TaskStatus::Open, TaskType::Task, 1),
        task("cas-a1b", TaskStatus::InProgress, TaskType::Task, 1),
        task("cas-c3", TaskStatus::Closed, TaskType::Task, 1),
        task("cas-e4", TaskStatus::Open, TaskType::Epic, 1),
        task("cas-f5", TaskStatus::AwaitingMerge, TaskType::Bug, 1),
        task("cas-r1", TaskStatus::Open, TaskType::Task, 1),
        task("cas-r2", TaskStatus::Open, TaskType::Task, 1),
        task("cas-r3", TaskStatus::Open, TaskType::Task, 1),
        task("cas-r4", TaskStatus::Open, TaskType::Task, 1),
    ];
    let commits = vec![
        commit("re-landed", "fix(cas-r4): restore the delivery", 0),
        commit("revert-only", "revert(cas-r1): withdraw the delivery", 0),
        commit("git-revert-only", "Revert \"feat(cas-r2): original delivery\"", 0),
        commit("reverted", "revert(cas-r3): withdraw the delivery", 0),
        commit("older-revert", "revert(cas-r4): withdraw the delivery", 1),
        commit("older-delivery", "feat(cas-r3): original delivery", 1),
        commit("1111111111111111111111111111111111111111", "fix(cas-a1): the actual fix", 2),
        commit("2222222222222222222222222222222222222222", "test(cas-a1): red first", 3),
        commit("3333333333333333333333333333333333333333", "fix(cas-c3): already closed", 2),
        commit("4444444444444444444444444444444444444444", "chore(cas-e4): epic bookkeeping", 2),
        commit("5555555555555555555555555555555555555555", "feat(cas-f5): merged ahead of its close", 2),
        commit("6666666666666666666666666666666666666666", "docs: mention cas-a1bx in passing", 2),
    ];
    let found = delivered_open(&tasks, &commits);
    let ids: Vec<&str> = found.iter().map(|f| f.task_id.as_str()).collect();
    assert_eq!(ids, vec!["cas-a1", "cas-f5", "cas-r4"], "{found:#?}");
    // The newest subject-matching commit is the evidence; the rest count.
    assert_eq!(found[0].commit.sha, "1111111111111111111111111111111111111111");
    assert_eq!(found[0].more_commits, 1);
    assert_eq!(found[1].status, "awaiting_merge");
    assert_eq!(found[2].commit.sha, "re-landed");
}

/// Kind 2: an open epic with children, all ended, is finished; an epic with
/// any open child, a closed epic and an epic with no children are not.
#[test]
fn finished_open_epics_need_children_and_all_of_them_ended() {
    let mut done_child = task("cas-c1", TaskStatus::Closed, TaskType::Task, 3);
    done_child.closed_at = Some(Utc::now() - Duration::days(3));
    let mut later_child = task("cas-c2", TaskStatus::Cancelled, TaskType::Task, 1);
    later_child.closed_at = Some(Utc::now() - Duration::days(1));
    let tasks = vec![
        task("cas-e1", TaskStatus::Open, TaskType::Epic, 5),
        done_child,
        later_child,
        task("cas-e2", TaskStatus::InProgress, TaskType::Epic, 5),
        task("cas-c3", TaskStatus::Closed, TaskType::Task, 1),
        task("cas-c4", TaskStatus::Open, TaskType::Task, 1),
        task("cas-e3", TaskStatus::Closed, TaskType::Epic, 5),
        task("cas-c5", TaskStatus::Closed, TaskType::Task, 1),
        task("cas-e4", TaskStatus::Open, TaskType::Epic, 5),
    ];
    let deps = vec![
        child("cas-c1", "cas-e1"),
        child("cas-c2", "cas-e1"),
        child("cas-c3", "cas-e2"),
        child("cas-c4", "cas-e2"),
        child("cas-c5", "cas-e3"),
        Dependency::new("cas-c4".into(), "cas-e4".into(), DependencyType::Blocks),
    ];
    let found = finished_open_epics(&tasks, &deps);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].epic_id, "cas-e1");
    assert_eq!(found[0].children, 2);
    let last = found[0].last_closed_at.expect("last child close");
    assert!(Utc::now() - last < Duration::days(2), "the latest close is the evidence");
}

/// Kind 3: open non-epic tasks untouched for 14 days or more.
#[test]
fn idle_open_reports_tasks_untouched_for_fourteen_days() {
    let mut assigned = task("cas-i2", TaskStatus::Blocked, TaskType::Task, 30);
    assigned.assignee = Some("old-worker".into());
    let tasks = vec![
        task("cas-i1", TaskStatus::Open, TaskType::Task, IDLE_DAYS),
        assigned,
        task("cas-f1", TaskStatus::Open, TaskType::Task, IDLE_DAYS - 1),
        task("cas-x1", TaskStatus::Closed, TaskType::Task, 90),
        task("cas-e1", TaskStatus::Open, TaskType::Epic, 90),
    ];
    let found = idle_open(&tasks, Utc::now());
    let ids: Vec<&str> = found.iter().map(|f| f.task_id.as_str()).collect();
    assert_eq!(ids, vec!["cas-i2", "cas-i1"], "longest idle first: {found:#?}");
    assert_eq!(found[0].idle_days, 30);
    assert_eq!(found[0].assignee.as_deref(), Some("old-worker"));
    assert_eq!(found[0].status, "blocked");
}

/// The close calls go through the ordinary gates: a commit receipt for a
/// delivered task, a plain close for a finished epic.
#[test]
fn close_calls_name_the_task_and_its_evidence() {
    let delivered = DeliveredOpen {
        task_id: "cas-a1".into(),
        title: "t".into(),
        status: "open".into(),
        commit: commit("1111111111111111111111111111111111111111", "fix(cas-a1): x", 1),
        more_commits: 0,
    };
    let call = delivered_close_call(&delivered, "origin/main");
    assert!(call.starts_with("task action=close id=cas-a1 "), "{call}");
    assert!(call.contains("commit_receipt=1111111111111111111111111111111111111111"), "{call}");
    assert!(call.contains("origin/main"), "{call}");
    let epic = FinishedEpic {
        epic_id: "cas-e1".into(),
        title: "t".into(),
        children: 2,
        last_closed_at: None,
    };
    let call = epic_close_call(&epic);
    assert!(call.starts_with("task action=close id=cas-e1 "), "{call}");
    assert!(call.contains("all 2 children"), "{call}");
}

fn git(repo: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// The scan reads the project's default branch and task store together.
#[test]
fn scan_reads_the_default_branch_and_the_task_store() {
    let temp = tempfile::TempDir::new().unwrap();
    let repo = temp.path();
    git(repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("feature.txt"), "base\n").unwrap();
    git(repo, &["add", "feature.txt"]);
    git(repo, &["commit", "-q", "-m", "initial tree"]);
    std::fs::write(repo.join("feature.txt"), "task delivery\n").unwrap();
    git(repo, &["commit", "-q", "-am", "feat(cas-r1): implemented"]);
    // A rollback need not name the task or use Git's generated revert subject.
    std::fs::write(repo.join("feature.txt"), "base\n").unwrap();
    git(repo, &["commit", "-q", "-am", "restore the earlier feature tree"]);
    // Unrelated work keeps the whole target tree distinct from the old base.
    std::fs::write(repo.join("other.txt"), "normal delivery\n").unwrap();
    git(repo, &["add", "other.txt"]);
    git(repo, &["commit", "-q", "-m", "fix(cas-d1): delivered long ago"]);
    git(repo, &["switch", "-q", "-c", "side"]);
    git(repo, &["commit", "-q", "--allow-empty", "-m", "fix(cas-s1): only on a side branch"]);
    git(repo, &["switch", "-q", "main"]);
    let cas_root = crate::store::init_cas_dir(repo).unwrap();
    let store = crate::store::open_task_store_local(&cas_root).unwrap();
    for (id, task_type) in [
        ("cas-d1", TaskType::Task),
        ("cas-r1", TaskType::Task),
        ("cas-s1", TaskType::Task),
        ("cas-e1", TaskType::Epic),
        ("cas-c1", TaskType::Task),
    ] {
        let mut task = Task::new(id.into(), format!("title of {id}"));
        task.task_type = task_type;
        if id == "cas-c1" {
            task.status = TaskStatus::Closed;
            task.closed_at = Some(Utc::now());
        }
        store.add(&task).unwrap();
    }
    store.add_dependency(&child("cas-c1", "cas-e1")).unwrap();

    let report = scan(&cas_root, Utc::now()).unwrap();
    assert_eq!(report.default_ref.as_deref(), Some("main"));
    assert!(report.delivery_unknown.is_none(), "{:?}", report.delivery_unknown);
    let delivered: Vec<&str> = report.delivered.iter().map(|f| f.task_id.as_str()).collect();
    assert_eq!(delivered, vec!["cas-d1"]);
    assert_eq!(report.finished_epics.len(), 1);
    assert_eq!(report.finished_epics[0].epic_id, "cas-e1");
    assert!(report.idle.is_empty());
    let (text, _) = session_start_block(&report).unwrap();
    assert!(text.contains("delivered then reverted") && text.contains("cas-r1"), "{text}");
    assert!(!text.contains("task action=close id=cas-r1"), "{text}");
}

/// Without a git repository kind 1 is unknown, said so, and kinds 2 and 3
/// still run.
#[test]
fn scan_without_git_says_delivery_is_unknown() {
    let temp = tempfile::TempDir::new().unwrap();
    let cas_root = crate::store::init_cas_dir(temp.path()).unwrap();
    let store = crate::store::open_task_store_local(&cas_root).unwrap();
    let mut old = Task::new("cas-o1".into(), "old".into());
    old.updated_at = Utc::now() - Duration::days(40);
    old.created_at = old.updated_at;
    store.add(&old).unwrap();
    let report = scan(&cas_root, Utc::now()).unwrap();
    assert!(report.default_ref.is_none());
    assert!(report.delivery_unknown.is_some());
    assert_eq!(report.idle.len(), 1, "{:?}", report.idle);
}

fn sample_report() -> BacklogReport {
    BacklogReport {
        default_ref: Some("origin/main".into()),
        delivery_unknown: None,
        delivered: (0..7)
            .map(|n| DeliveredOpen {
                task_id: format!("cas-d{n}"),
                title: format!("delivered task {n}"),
                status: "open".into(),
                commit: commit(&format!("{n}{}", "a".repeat(39)), &format!("fix(cas-d{n}): shipped"), n),
                more_commits: 0,
            })
            .collect(),
        finished_epics: vec![FinishedEpic {
            epic_id: "cas-e1".into(),
            title: "an epic".into(),
            children: 4,
            last_closed_at: Some(Utc::now() - Duration::days(2)),
        }],
        idle: vec![IdleOpen {
            task_id: "cas-i1".into(),
            title: "idle task".into(),
            status: "open".into(),
            assignee: None,
            idle_days: 20,
        }],
        stale_citations: (0..5)
            .map(|n| StaleCitations {
                task_id: format!("cas-s{n}"),
                title: format!("stale task {n}"),
                status: "open".into(),
                gone: vec![GoneCitation {
                    kind: CitationKind::File,
                    cited: format!("cas-cli/src/removed_{n}.rs"),
                    removed_in: "abc123456".into(),
                    removed_on: "2026-10-01".into(),
                }],
            })
            .collect(),
        citations_unchecked: 0,
    }
}

/// Supervisor startup: a bounded block that leads with counts, shows the
/// evidence and the close call for the first entries, and says how many more.
#[test]
fn session_start_block_shows_evidence_and_close_calls_within_its_cap() {
    let (full, compact) = session_start_block(&sample_report()).expect("a block");
    assert!(full.len() <= SESSION_START_CAP_BYTES, "{} bytes", full.len());
    assert!(compact.len() < full.len());
    let first = full.lines().next().unwrap();
    assert!(first.contains("7 delivered but open") && first.contains("1 finished epic") && first.contains("1 idle"), "{first}");
    assert!(full.contains("cas-d0") && full.contains("fix(cas-d0): shipped"), "{full}");
    assert!(full.contains("commit_receipt="), "{full}");
    assert!(full.contains("task action=close id=cas-e1"), "{full}");
    assert!(full.contains("more"), "the rest are counted: {full}");
    assert!(compact.contains("cas doctor"), "{compact}");
    assert!(session_start_block(&BacklogReport::default()).is_none());

    // cas-d9038: sixty heavy kind-4 findings still fit, on their own line.
    let mut report = sample_report();
    report.stale_citations = (0..60)
        .map(|n| StaleCitations {
            task_id: format!("cas-s{n}"),
            title: "x".repeat(200),
            status: "open".into(),
            gone: (0..8)
                .map(|m| GoneCitation {
                    kind: CitationKind::Symbol,
                    cited: format!("crate::very::long::module::path::symbol_{m}_{}", "y".repeat(60)),
                    removed_in: "abc123456".into(),
                    removed_on: "2026-10-01".into(),
                })
                .collect(),
        })
        .collect();
    let (full, compact) = session_start_block(&report).unwrap();
    assert!(full.len() <= SESSION_START_CAP_BYTES, "{} bytes", full.len());
    assert!(compact.len() <= SESSION_START_CAP_BYTES);
    assert!(full.lines().next().unwrap().contains("60 citing removed code"), "{full}");
    let line = stale_citation_line(&report.stale_citations[0]);
    assert!(line.len() <= 400, "one entry is bounded: {} bytes", line.len());
}

/// Supervisor startup only: a worker's SessionStart never carries the block,
/// and a supervisor's does once the backlog holds a finding.
#[test]
fn session_start_block_is_for_supervisors_only() {
    let temp = tempfile::TempDir::new().unwrap();
    let cas_root = crate::store::init_cas_dir(temp.path()).unwrap();
    let store = crate::store::open_task_store_local(&cas_root).unwrap();
    let mut epic = Task::new("cas-e9".into(), "finished epic".into());
    epic.task_type = TaskType::Epic;
    store.add(&epic).unwrap();
    let mut done = Task::new("cas-c9".into(), "done child".into());
    done.status = TaskStatus::Closed;
    done.closed_at = Some(Utc::now());
    store.add(&done).unwrap();
    store.add_dependency(&child("cas-c9", "cas-e9")).unwrap();

    for role in [None, Some("worker")] {
        assert!(session_start_block_for_role(role, &cas_root, Utc::now()).is_none(), "{role:?}");
    }
    let (full, _) = session_start_block_for_role(Some("supervisor"), &cas_root, Utc::now())
        .expect("a supervisor sees the finished epic");
    assert!(full.contains("task action=close id=cas-e9"), "{full}");
}

// ---- cas-d9038: kind 4, tasks whose cited code is all gone ----

/// Paths need a directory and an extension; `:line` drops; URLs, absolute
/// paths and bare words are not citations. Qualified symbols keep their path.
#[test]
fn extract_citations_reads_paths_and_qualified_symbols() {
    let text = "Fix `cas-cli/src/cloud/peers.rs:402` and crates/cas-store/src/lib.rs (see \
                https://github.com/acme/repo/blob/main/src/x.rs and /home/me/repo/src/y.rs). \
                The bug is in close_ops::message_references_task and \
                crate::cloud::peers::render_user_peers; also README.md, Cargo.toml, \
                cas-cli/src/cloud/peers.rs again, and std::fmt.";
    let (files, symbols) = extract_citations(text);
    assert_eq!(files, vec!["cas-cli/src/cloud/peers.rs", "crates/cas-store/src/lib.rs"]);
    assert_eq!(
        symbols,
        vec!["close_ops::message_references_task", "crate::cloud::peers::render_user_peers"],
        "short trailing segments like `fmt` are too common to test"
    );
}

/// A tree with history: one file deleted, one symbol renamed, one kept.
fn repo_with_removals() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::TempDir::new().unwrap();
    let repo = temp.path().to_path_buf();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/old_module.rs"), "pub fn legacy_entry() {}\n").unwrap();
    std::fs::write(repo.join("src/lib.rs"), "pub fn old_helper_name() {}\npub fn kept_helper() {}\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "initial"]);
    git(&repo, &["rm", "-q", "src/old_module.rs"]);
    git(&repo, &["commit", "-q", "-m", "remove the old module"]);
    std::fs::write(repo.join("src/lib.rs"), "pub fn new_helper_name() {}\npub fn kept_helper() {}\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "rename the helper"]);
    (temp, repo)
}

fn open_task(id: &str, description: &str) -> Task {
    let mut task = Task::new(id.into(), format!("title of {id}"));
    task.description = description.into();
    task.created_at = Utc::now() - Duration::days(1);
    task
}

/// Acceptance 1 and 2: a task citing a deleted file and a renamed symbol is
/// reported with each removal dated; a task citing live code, a task with no
/// citations, and a task citing code git never had are not.
#[test]
fn stale_citations_reports_only_tasks_whose_cited_code_is_all_gone() {
    let (_temp, repo) = repo_with_removals();
    let tasks = vec![
        open_task("cas-gone", "Fix src/old_module.rs and crate::lib::old_helper_name."),
        open_task("cas-live", "Touch src/lib.rs and crate::lib::kept_helper."),
        open_task("cas-mixed", "src/old_module.rs moved; see crate::lib::kept_helper."),
        open_task("cas-none", "Make the dashboard friendlier."),
        open_task("cas-planned", "Add src/brand_new_module.rs with crate::lib::future_helper."),
    ];
    let (found, unchecked) = stale_citations(&repo, "main", &tasks, CitationBudget::DOCTOR);
    assert_eq!(unchecked, 0);
    let ids: Vec<&str> = found.iter().map(|f| f.task_id.as_str()).collect();
    assert_eq!(ids, vec!["cas-gone"], "{found:#?}");
    let gone = &found[0].gone;
    assert_eq!(gone.len(), 2, "{gone:#?}");
    assert_eq!((gone[0].kind, gone[0].cited.as_str()), (CitationKind::File, "src/old_module.rs"));
    assert_eq!((gone[1].kind, gone[1].cited.as_str()), (CitationKind::Symbol, "crate::lib::old_helper_name"));
    for citation in gone {
        assert!(!citation.removed_in.is_empty() && citation.removed_on.len() == 10, "{citation:?}");
    }
    assert_ne!(gone[0].removed_in, gone[1].removed_in, "each removal is its own commit");
    let line = stale_citation_line(&found[0]);
    assert!(line.contains("src/old_module.rs") && line.contains(&gone[0].removed_in), "{line}");
    assert!(line.contains("old_helper_name") && line.contains("review"), "{line}");
    assert!(!line.contains("action=close"), "never a close call: {line}");
}

/// Closed tasks and epics are not read; the task cap counts the rest.
#[test]
fn stale_citations_reads_only_open_non_epic_tasks_within_the_cap() {
    let (_temp, repo) = repo_with_removals();
    let mut closed = open_task("cas-closed", "src/old_module.rs");
    closed.status = TaskStatus::Closed;
    let mut epic = open_task("cas-epic", "src/old_module.rs");
    epic.task_type = TaskType::Epic;
    let tasks = vec![closed, epic, open_task("cas-a", "src/old_module.rs"), open_task("cas-b", "src/old_module.rs")];
    let budget = CitationBudget { max_tasks: 1, ..CitationBudget::DOCTOR };
    let (found, unchecked) = stale_citations(&repo, "main", &tasks, budget);
    assert_eq!(found.len(), 1);
    assert_eq!(unchecked, 1);
}
