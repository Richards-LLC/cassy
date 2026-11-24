//! cas-dcc9, cas-d9d3c: a batch drop keeps the rest of the train, a revert
//! conflict changes nothing and names its files, the second ejection goes to
//! the operator; one cut notice per train state, naming the project's command.
use super::*;
use crate::store::{open_agent_store, open_supervisor_queue_store, open_task_store};
use crate::test_support::TestEnvGuard;
use cas_types::{Agent, AgentRole, Dependency, DependencyType, TaskType};
use std::sync::Mutex;

const EPIC: &str = "cas-tr00";
const TRAIN: &str = "epic/release-train-cas-tr00";

fn run(repo: &Path, args: &[&str]) -> String {
    git(repo, args).unwrap()
}

#[derive(Default)]
struct FakeQueue(Mutex<Vec<u64>>);

impl TrainQueue for FakeQueue {
    fn open_pr(&self, branch: &str) -> Result<Option<u64>> {
        Ok((branch == TRAIN).then_some(42))
    }
    fn enqueue(&self, pr: u64) -> Result<()> {
        self.0.lock().unwrap().push(pr);
        Ok(())
    }
}

/// A train epic with `children` (id, status); returns the Cassy dir.
fn train(repo: &Path, children: &[(&str, TaskStatus)]) -> PathBuf {
    let cas_dir = crate::store::init_cas_dir(repo).unwrap();
    let tasks = open_task_store(&cas_dir).unwrap();
    let agents = open_agent_store(&cas_dir).unwrap();
    let mut supervisor = Agent::new_with_role("sup-id".into(), "supervisor".into(), AgentRole::Supervisor);
    supervisor.factory_session = Some("factory-session".into());
    agents.register(&supervisor).unwrap();
    let mut epic = Task::new(EPIC.into(), "Release train vNEXT".into());
    epic.task_type = TaskType::Epic;
    epic.labels = vec![crate::mcp::tools::core::task::release_train::RELEASE_TRAIN_LABEL.into()];
    epic.branch = Some(TRAIN.into());
    epic.epic_verification_owner = Some("sup-id".into());
    tasks.add(&epic).unwrap();
    for (id, status) in children {
        let worker = format!("worker-{id}");
        agents.register(&Agent::new_with_role(format!("{worker}-id"), worker.clone(), AgentRole::Worker)).unwrap();
        let mut task = Task::new((*id).into(), format!("fix {id}"));
        task.status = *status;
        task.assignee = Some(worker);
        tasks.add(&task).unwrap();
        tasks.add_dependency(&Dependency::new((*id).into(), EPIC.into(), DependencyType::ParentChild)).unwrap();
    }
    cas_dir
}

/// Merge `factory/<id>`, which writes `<id>.txt` and each of `also`, into the
/// train and record it.
fn merge_child(repo: &Path, cas_dir: &Path, id: &str, also: &[&str]) {
    let branch = format!("factory/{id}");
    run(repo, &["checkout", "-q", "-b", &branch, TRAIN]);
    for file in std::iter::once(format!("{id}.txt")).chain(also.iter().map(|f| f.to_string())) {
        std::fs::write(repo.join(file), id).unwrap();
    }
    run(repo, &["add", "."]);
    run(repo, &["commit", "-q", "-m", id]);
    let source = run(repo, &["rev-parse", "HEAD"]);
    run(repo, &["checkout", "-q", TRAIN]);
    run(repo, &["merge", "-q", "--no-ff", "-m", &format!("merge {id}"), &branch]);
    let merge = run(repo, &["rev-parse", "HEAD"]);
    cas_store::record_observed_delivery_merge(cas_dir, id, &branch, TRAIN, &source, &merge, "sup-id").unwrap();
}

fn repo_with_origin() -> (tempfile::TempDir, tempfile::TempDir) {
    let origin = tempfile::tempdir().unwrap();
    run(origin.path(), &["init", "-q", "--bare"]);
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    run(repo, &["init", "-q", "-b", "main"]);
    run(repo, &["config", "user.name", "CAS Test"]);
    run(repo, &["config", "user.email", "cas@example.test"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    run(repo, &["add", "."]);
    run(repo, &["commit", "-q", "-m", "seed"]);
    run(repo, &["remote", "add", "origin", origin.path().to_str().unwrap()]);
    run(repo, &["checkout", "-q", "-b", TRAIN]);
    (dir, origin)
}

#[tokio::test]
async fn batch_drop_reverts_the_bad_children_once_and_a_conflict_changes_nothing_cas_d9d3c() {
    let mut env = TestEnvGuard::temp_home();
    let (dir, _origin) = repo_with_origin();
    let repo = dir.path();
    let children = [("cas-a", TaskStatus::Closed), ("cas-b", TaskStatus::Closed), ("cas-c", TaskStatus::Closed)];
    let cas_dir = train(repo, &children);
    merge_child(repo, &cas_dir, "cas-a", &[]);
    merge_child(repo, &cas_dir, "cas-b", &[]);
    // cas-c also rewrites cas-b's file, so reverting cas-b alone conflicts.
    merge_child(repo, &cas_dir, "cas-c", &["cas-b.txt"]);
    // The supervisor's shared checkout stays on main; the train is a branch.
    run(repo, &["checkout", "-q", "main"]);
    run(repo, &["push", "-q", "origin", TRAIN]);
    let before = run(repo, &["rev-parse", TRAIN]);
    env.set("CAS_ROOT", &cas_dir);
    env.set("CAS_AGENT_ROLE", "supervisor");
    let core = CasCore::with_daemon(cas_dir.clone(), None, None);
    core.set_agent_id_for_testing("sup-id".into());
    let queue = FakeQueue::default();
    let ids = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();

    let error = drop_children(&core, &ids(&["cas-a", "cas-b"]), "merge_group: suite failed", &queue).await.unwrap_err();
    let error = format!("{error:#}");

    assert_eq!(run(repo, &["rev-parse", TRAIN]), before);
    assert!(queue.0.lock().unwrap().is_empty());

    let receipt = drop_children(&core, &ids(&["cas-b", "cas-c", "cas-b"]), "merge_group: suite failed", &queue).await.unwrap();

    let files = run(repo, &["ls-tree", "--name-only", TRAIN]);
    assert!(files.contains("cas-a.txt") && !files.contains("cas-b.txt") && !files.contains("cas-c.txt"), "{files}");
    assert!(is_ancestor(repo, &before, TRAIN), "revert commits, never a history rewrite");
    assert_eq!(receipt.dropped.len(), 2);
    let tip = run(repo, &["rev-parse", TRAIN]);
    assert_eq!(run(repo, &["ls-remote", "origin", TRAIN]).split_whitespace().next(), Some(tip.as_str()));
    let tasks = open_task_store(&cas_dir).unwrap();
    let sq = open_supervisor_queue_store(&cas_dir).unwrap();
    for dropped in &receipt.dropped {
        let task = tasks.get(&dropped.task_id).unwrap();
        let worker = format!("worker-{}", dropped.task_id);
        assert_eq!((task.status, task.assignee.as_deref()), (TaskStatus::Open, Some(worker.as_str())));
        assert!(task.notes.contains(&format!("git revert {}", dropped.revert)), "{}", task.notes);
        assert_eq!(sq.peek(&format!("{worker}-id"), 10).unwrap().len(), 1, "each worker gets the failure");
    }
    assert_eq!(tasks.get("cas-a").unwrap().status, TaskStatus::Closed);
    assert_eq!(*queue.0.lock().unwrap(), vec![42], "one re-enqueue for the batch");

    // The re-enqueue was the train's one repair cycle (rule-176(5)).
    let error = drop_children(&core, &ids(&["cas-a"]), "ejected again", &queue).await.unwrap_err().to_string();
    assert!(error.contains("P0") && error.contains("rule-176(5)"), "{error}");
    assert_eq!(run(repo, &["rev-parse", TRAIN]), tip, "nothing reverted");
    assert_eq!(*queue.0.lock().unwrap(), vec![42]);
    assert_eq!(sq.peek("operator", 10).unwrap().len(), 1);
}

/// The receipt `train_ci::observe` records for the train's current tip.
fn tip_receipt(repo: &Path, cas_dir: &Path, outcome: &str) {
    let dir = crate::train_ci::directory(cas_dir, EPIC).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let state = serde_json::json!({"branch": TRAIN, "head": run(repo, &["rev-parse", TRAIN]), "outcome": outcome});
    std::fs::write(dir.join("state.json"), state.to_string()).unwrap();
}

#[test]
fn production_cut_proposal_requires_pass_for_the_frozen_candidate() {
    let _env = TestEnvGuard::temp_home();
    let (dir, _origin) = repo_with_origin();
    let repo = dir.path();
    let cas_dir = train(repo, &[("cas-a", TaskStatus::AwaitingMerge)]);
    let config = "[project]\ncanonical_id='candidate-fixture'\n[release]\nauto_cut_max_per_day=0\n";
    let configure = |extra: &str| std::fs::write(cas_dir.join("config.toml"), format!("{config}{extra}")).unwrap();
    configure("");
    merge_child(repo, &cas_dir, "cas-a", &["index.html"]);
    let stale = run(repo, &["rev-parse", TRAIN]);
    run(repo, &["commit", "--allow-empty", "-qm", "refresh candidate"]);
    let candidate = run(repo, &["rev-parse", TRAIN]);
    crate::mcp::tools::core::workflow::release_freeze::freeze(&cas_dir, EPIC, "operator", None).unwrap();
    tip_receipt(repo, &cas_dir, "PASS");

    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = tasks.get("cas-a").unwrap();
    task.origin_project = Some("candidate-fixture".into());
    let anchor = run(repo, &["rev-parse", "factory/cas-a"]);
    task.deliverables.factory_branch_anchor = Some(anchor.clone());
    tasks.update(&task).unwrap();
    cas_store::defer_qa_batch_evidence(&cas_dir, &task.id, &anchor).unwrap();
    let mut epic = tasks.get(EPIC).unwrap();
    epic.origin_project = task.origin_project.clone();
    tasks.update(&epic).unwrap();
    let recorder = open_agent_store(&cas_dir).unwrap().get("sup-id").unwrap();
    let artifacts = cas_store::SqliteArtifactStore::open(&cas_dir).unwrap();
    std::fs::write(repo.join("capture.png"), include_bytes!("../../vendor/ghostty/images/icons/icon_16.png")).unwrap();
    let receive = |head: &str, build: &str, passed: bool| {
        let ledger = repo.join("LEDGER.md");
        let (count, row) = if passed {
            ("PASS=1; NOT EXERCISED=0", "| M01 | browser | page visible (index.html:1) | visible: page | PASS | real-build | capture.png | - |")
        } else {
            ("PASS=0; NOT EXERCISED=1", "| M01 | browser | page visible | not run | NOT EXERCISED: browser unavailable | real-build | - | - |")
        };
        std::fs::write(&ledger, format!("schema: 2\nbuild: {build}\ncovers: cas-a\ncells=1; FAIL=0; {count}\nlabels: real-build=1\n{}\n{row}\n", crate::qa_ledger::HEADER)).unwrap();
        let context = crate::artifacts::PublishContext {
            store: &artifacts,
            roots: crate::artifacts::paths::PublishRoots::new(&cas_dir, &repo.join("artifacts"), EPIC),
            task_id: EPIC.into(), cloud: None,
        };
        let artifact = crate::artifacts::publish(&context, &ledger).unwrap().artifact;
        let receipt = crate::qa_batch::prepare_receipt(
            &cas_dir, &recorder, "external-reviewer", &["cas-a".into()], head, &artifact.id,
            &ledger, cas_store::QA_BATCH_RECEIPT_SCHEMA,
        )?;
        cas_store::record_qa_batch_receipt(&cas_dir, &receipt).map_err(|error| error.to_string())
    };
    let propose = || notify_train_cuts(&cas_dir, &BTreeSet::new(), Utc::now() + chrono::Duration::hours(3)).unwrap();

    configure("[qa]\napi_paths=['index.html']\n");
    admit_cut_candidate(&cas_dir, &epic, &candidate).unwrap();
    configure("[factory]\nepic_base_branch='staging'\n");
    admit_cut_candidate(&cas_dir, &epic, &candidate).unwrap();
    configure("");
    assert!(admit_cut_candidate(&cas_dir, &epic, &candidate).unwrap_err().contains(&format!("PRODUCTION_BATCH_QA_REQUIRED candidate_sha={candidate}")));
    assert_eq!(propose(), 0);
    receive(&stale, &stale, true).unwrap();
    assert_eq!(propose(), 0);
    assert!(receive(&candidate, &stale, true).unwrap_err().contains("PRODUCTION_BATCH_QA_BUILD_MISMATCH"));
    assert!(receive(&candidate, &candidate, false).unwrap_err().contains("PRODUCTION_BATCH_QA_BROWSER_PASS_REQUIRED"));
    receive(&candidate, &candidate, true).unwrap();
    assert_eq!(propose(), 1);
    let base = run(repo, &["rev-parse", "main"]);
    let tree = run(repo, &["rev-parse", &format!("{candidate}^{{tree}}")]);
    let candidate_tree = cas_store::ProductionCandidate { base_sha: base.clone(), source_sha: candidate.clone(), tree };
    let synthetic = run(repo, &["commit-tree", &candidate_tree.tree, "-p", &base, "-p", &candidate,
        "-m", &format!("{}fixture", crate::mcp::tools::core::workflow::premerge_verification::CANDIDATE_COMMIT_PREFIX)]);
    receive(&synthetic, &synthetic, true).unwrap();
    crate::qa_batch::production::admit(&cas_dir, repo, "main", &candidate_tree, Some(&synthetic), None).unwrap();
    crate::qa_batch::production::admit(&cas_dir, repo, "main", &candidate_tree, None, None).unwrap();
    let moved = run(repo, &["commit-tree", &format!("{base}^{{tree}}"), "-p", &base, "-m", "advance production"]);
    run(repo, &["update-ref", "refs/heads/main", &moved, &base]);
    assert!(admit_cut_candidate(&cas_dir, &epic, &candidate).unwrap_err().contains(&format!("PRODUCTION_BATCH_QA_REQUIRED candidate_sha={candidate}")));
    let moved_candidate = cas_store::ProductionCandidate { base_sha: moved.clone(), ..candidate_tree.clone() };
    assert!(crate::qa_batch::production::admit(&cas_dir, repo, "main", &moved_candidate, None, None)
        .unwrap_err().contains("PRODUCTION_BATCH_QA_REQUIRED"));
    run(repo, &["update-ref", "refs/heads/main", &base, &moved]);
    run(repo, &["checkout", "-q", "main"]);
    run(repo, &["merge", "--no-ff", "-m", "land the checked tree", TRAIN]);
    let landed = run(repo, &["rev-parse", "HEAD"]);
    crate::qa_batch::production::validate_landing(repo, &candidate_tree, &synthetic, &landed, &base).unwrap();
    assert_eq!(crate::qa_batch::satisfying_deferred_receipt(&cas_dir, &task, repo, "main", &landed)
        .unwrap().unwrap().staging_sha, synthetic);
}

/// GH #1635: no notice asks for a cut without a green verdict at the tip.
/// cas-0fc56: an unfrozen train has no train suite, so its notice asks for
/// the freeze that starts one; once frozen, only green at the current tip notifies.
#[test]
fn frozen_train_cut_notice_waits_for_green_at_its_tip_gh_1635() {
    let _env = TestEnvGuard::temp_home();
    let (dir, _origin) = repo_with_origin();
    let repo = dir.path();
    let cas_dir = train(repo, &[("cas-a", TaskStatus::Closed), ("cas-b", TaskStatus::InProgress)]);
    run(repo, &["branch", "staging", "main"]);
    std::fs::write(cas_dir.join("config.toml"), "[factory]\nepic_base_branch='staging'\n").unwrap();
    merge_child(repo, &cas_dir, "cas-a", &[]);
    let sq = open_supervisor_queue_store(&cas_dir).unwrap();
    let notices = || sq.peek("sup-id", 10).unwrap().len();
    let none = BTreeSet::new();
    let late = Utc::now() + chrono::Duration::hours(3);

    notify_train_cuts(&cas_dir, &none, late).unwrap();
    assert_eq!(notices(), 1, "unfrozen: the max-age notice asks for the freeze");
    crate::mcp::tools::core::workflow::release_freeze::freeze(&cas_dir, EPIC, "operator", None).unwrap();
    notify_train_cuts(&cas_dir, &none, late).unwrap();
    assert_eq!(notices(), 1, "frozen, no verdict yet: no cut notice");
    tip_receipt(repo, &cas_dir, "FAIL");
    notify_train_cuts(&cas_dir, &none, late).unwrap();
    assert_eq!(notices(), 1, "red tip: no cut notice");

    tip_receipt(repo, &cas_dir, "PASS");
    merge_child(repo, &cas_dir, "cas-c", &[]);
    notify_train_cuts(&cas_dir, &none, late).unwrap();
    assert_eq!(notices(), 1, "green at an older tip does not count");

    tip_receipt(repo, &cas_dir, "PASS");
    notify_train_cuts(&cas_dir, &none, late).unwrap();
    assert_eq!(notices(), 2, "green at the current frozen tip notifies");

    // The max-age notice is per tip, not per train: a changed tip that goes green notifies again.
    merge_child(repo, &cas_dir, "cas-d", &[]);
    tip_receipt(repo, &cas_dir, "PASS");
    for _ in 0..2 {
        notify_train_cuts(&cas_dir, &none, late).unwrap();
    }
    assert_eq!(notices(), 3, "a new green tip notifies once more");
}

#[test]
fn cut_notice_is_sent_once_per_state_and_names_the_project_release_command_cas_d9d3c() {
    let _env = TestEnvGuard::temp_home();
    let (dir, _origin) = repo_with_origin();
    let repo = dir.path();
    let cas_dir = train(repo, &[("cas-a", TaskStatus::Closed), ("cas-b", TaskStatus::InProgress)]);
    merge_child(repo, &cas_dir, "cas-a", &[]);
    tip_receipt(repo, &cas_dir, "PASS");
    std::fs::create_dir_all(repo.join("scripts")).unwrap();
    std::fs::write(repo.join("scripts/release.sh"), "#!/bin/sh\n").unwrap();
    let sq = open_supervisor_queue_store(&cas_dir).unwrap();
    let notices = || sq.peek("sup-id", 10).unwrap();
    let none = BTreeSet::new();
    let now = Utc::now();

    notify_train_cuts(&cas_dir, &none, now).unwrap();
    assert!(notices().is_empty(), "a child in progress on a young train waits");
    let later = now + chrono::Duration::hours(3);
    for _ in 0..2 {
        notify_train_cuts(&cas_dir, &none, later).unwrap();
    }
    assert_eq!(notices().len(), 1, "one max-age notice");

    assert!(notices()[0].payload.contains("run `scripts/release.sh`"), "the repository's own script by default");
    let notice = notices()[0].payload.to_lowercase();

    // A project that releases by promotion names its own command.
    let mut config = crate::config::Config::load(&cas_dir).unwrap();
    config.set("factory.release_command", "scripts/promote.sh staging main").unwrap();
    config.save(&cas_dir).unwrap();

    let tasks = open_task_store(&cas_dir).unwrap();
    let mut b = tasks.get("cas-b").unwrap();
    b.status = TaskStatus::Closed;
    tasks.update(&b).unwrap();
    notify_train_cuts(&cas_dir, &BTreeSet::from([TRAIN.to_string()]), now).unwrap();
    assert_eq!(notices().len(), 1, "a train in the merge queue is already being cut");
    for _ in 0..2 {
        notify_train_cuts(&cas_dir, &none, now).unwrap();
    }
    assert_eq!(notices().len(), 2, "one ready notice");
    assert!(notices()[1].payload.contains(&format!("on {TRAIN}, run `scripts/promote.sh staging main`")), "{}", notices()[1].payload);
    let prompts = crate::store::open_prompt_queue_store(&cas_dir).unwrap().peek_all(10).unwrap();
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts.iter().all(|p| p.target == "supervisor" && crate::supervisor_notices::is_action_envelope(&p.prompt)),
        "cut notices wake an idle supervisor: {prompts:?}"
    );
}

/// GH #1826: the cut notice counted only closed children, so six of eighteen
/// merged deliveries. A merged delivery awaiting its verdict counts; a
/// dropped, reopened one does not until it merges again.
#[test]
fn admitted_merges_count_merged_deliveries_awaiting_their_verdict_gh_1826() {
    let _env = TestEnvGuard::temp_home();
    let (dir, _origin) = repo_with_origin();
    let repo = dir.path();
    let children = [("cas-a", TaskStatus::Closed), ("cas-b", TaskStatus::AwaitingMerge), ("cas-c", TaskStatus::Open)];
    let cas_dir = train(repo, &children);
    let tasks = open_task_store(&cas_dir).unwrap();
    for (id, _) in children {
        merge_child(repo, &cas_dir, id, &[]);
    }
    let mut parked = tasks.get("cas-b").unwrap();
    parked.deliverables.factory_branch_anchor = Some(run(repo, &["rev-parse", "factory/cas-b"]));
    tasks.update(&parked).unwrap();

    let ids: Vec<_> = admitted_merges(&cas_dir, tasks.as_ref(), TRAIN).unwrap().into_iter().map(|m| m.task_id).collect();
    assert_eq!(ids, ["cas-a", "cas-b"]);
}
