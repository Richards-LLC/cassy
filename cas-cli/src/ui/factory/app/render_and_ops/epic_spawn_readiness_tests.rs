use super::*;
use std::path::{Path, PathBuf};

pub(super) fn git(repo: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

pub(super) fn commit(repo: &Path, file: &str) {
    std::fs::write(repo.join(file), file).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-m", file]);
}

pub(super) struct Fixture {
    pub repo: PathBuf,
    pub origin: PathBuf,
    pub root: PathBuf,
    pub base: String,
    pub old: String,
    pub app: FactoryApp,
}

impl Fixture {
    pub fn prepare(&mut self) -> anyhow::Result<WorkerSpawnPrep> {
        self.app
            .prepare_worker_spawn(Some("round2-worker"), true, Some("cas-round2-child"))
    }
    pub fn offline(&self) {
        git(
            &self.repo,
            &[
                "remote",
                "set-url",
                "origin",
                self.repo.join("missing.git").to_str().unwrap(),
            ],
        );
    }
    pub fn advance_parent(&mut self) {
        commit(&self.repo, "parent-current");
        self.base = git(&self.repo, &["rev-parse", "main"]);
        git(&self.repo, &["push", "origin", "main"]);
    }
}

pub(super) fn with_fixture(check: impl FnOnce(&mut Fixture)) {
    let temp = tempfile::TempDir::new().unwrap();
    let home = tempfile::TempDir::new().unwrap();
    let _env = crate::test_support::TestEnvGuard::with_vars(&[
        ("HOME", home.path().to_str().unwrap()),
        ("CAS_FACTORY_SESSION", "bfa1-round2"),
        ("CAS_PUSH_EPIC_BRANCH", "0"),
    ]);
    let repo = temp.path().join("repo");
    let origin = temp.path().join("origin.git");
    std::fs::create_dir(&repo).unwrap();
    git(temp.path(), &["init", "--bare", origin.to_str().unwrap()]);
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.name", "Cassy Test"]);
    git(&repo, &["config", "user.email", "test@cas.test"]);
    commit(&repo, "initial");
    let old = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["branch", "epic/round2"]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "origin", "main"]);
    let root = crate::store::init_cas_dir(&repo).unwrap();
    std::fs::write(
        root.join("config.toml"),
        "[project]\ncanonical_id = \"bfa1-round2\"\n",
    )
    .unwrap();
    let tasks = open_task_store_cached(&root).unwrap();
    let mut epic = cas_types::Task::new("cas-round2-epic".into(), "Round 2".into());
    epic.task_type = cas_types::TaskType::Epic;
    epic.branch = Some("epic/round2".into());
    epic.deliverables.work_target = Some(cas_types::WorkTarget {
        repo_selector: "project:bfa1-round2".into(),
        target_branch: "main".into(),
    });
    tasks.add(&epic).unwrap();
    tasks
        .add(&cas_types::Task::new(
            "cas-round2-child".into(),
            "Child".into(),
        ))
        .unwrap();
    tasks
        .add_dependency(&cas_types::Dependency {
            from_id: "cas-round2-child".into(),
            to_id: epic.id.clone(),
            dep_type: cas_types::DependencyType::ParentChild,
            created_at: chrono::Utc::now(),
            created_by: None,
        })
        .unwrap();
    let path = crate::ui::factory::metadata_path("bfa1-round2");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut metadata = crate::ui::factory::create_metadata(
        "bfa1-round2",
        12345,
        "supervisor",
        &[],
        None,
        repo.to_str(),
        None,
    );
    metadata.pinned_epic_id = Some(epic.id);
    std::fs::write(path, serde_json::to_string(&metadata).unwrap()).unwrap();
    let manager = WorktreeManager::new(
        &repo,
        WorktreeConfig {
            enabled: true,
            base_path: repo.join(".cas/worktrees").to_string_lossy().into(),
            branch_prefix: "factory/".into(),
            auto_merge: false,
            cleanup_on_close: false,
            promote_entries_on_merge: false,
        },
    )
    .unwrap();
    let app = FactoryApp::from_init_result(
        root.clone(),
        Mux::new(40, 120),
        Some(manager),
        DirectorData::load_fast(&root).unwrap(),
        "supervisor".into(),
        Vec::new(),
        crate::ui::factory::notification::NotifyConfig::default(),
        false,
        AutoPromptConfig::default(),
        cas_mux::SupervisorCli::Claude,
        cas_mux::SupervisorCli::Claude,
        120,
        40,
        false,
        None,
        None,
        repo.clone(),
    )
    .unwrap();
    check(&mut Fixture {
        repo,
        origin,
        root,
        base: old.clone(),
        old,
        app,
    });
}

#[test]
fn cas_bfa1_spawn_first_use_publishes_and_checks_out_current_base() {
    with_fixture(|f| {
        f.advance_parent();
        let prep = f.prepare().unwrap();
        assert_eq!(git(&f.origin, &["rev-parse", "epic/round2"]), f.base);
        let work = prep.worktree_info.unwrap();
        let checkout = work.base_ref.as_deref().unwrap_or(&work.parent_branch);
        assert_eq!(git(&f.repo, &["rev-parse", checkout]), f.base);
        assert!(prep.warnings.iter().any(|line| line.contains("published")));
    });
}

#[test]
fn cas_bfa1_spawn_offline_refreshes_local_base_and_retains_unverified_notice() {
    with_fixture(|f| {
        f.advance_parent();
        f.offline();
        let prep = f.prepare().unwrap();
        assert_eq!(git(&f.repo, &["rev-parse", "epic/round2"]), f.base);
        let work = prep.worktree_info.unwrap();
        assert_eq!(
            git(
                &f.repo,
                &[
                    "rev-parse",
                    work.base_ref.as_deref().unwrap_or(&work.parent_branch)
                ]
            ),
            f.base
        );
        assert!(
            prep.warnings
                .iter()
                .any(|line| line.contains("UNVERIFIED") || line.contains("UNPUBLISHED"))
        );
        assert!(
            git(
                &f.origin,
                &[
                    "for-each-ref",
                    "--format=%(refname)",
                    "refs/heads/epic/round2"
                ]
            )
            .is_empty()
        );
    });
}

#[test]
fn cas_bfa1_spawn_dirty_epic_refuses_without_cutting_worktree() {
    with_fixture(|f| {
        f.advance_parent();
        git(&f.repo, &["checkout", "epic/round2"]);
        std::fs::write(f.repo.join("initial"), "uncommitted").unwrap();
        let error = f
            .prepare()
            .err()
            .expect("tracked changes must refuse")
            .to_string();
        assert!(
            error.contains("EPIC BASE REFRESH REFUSED") && error.contains("tracked changes"),
            "{error}"
        );
        assert_eq!(git(&f.repo, &["rev-parse", "epic/round2"]), f.old);
        assert_eq!(
            std::fs::read_to_string(f.repo.join("initial")).unwrap(),
            "uncommitted"
        );
        assert!(!f.repo.join(".cas/worktrees/round2-worker").exists());
    });
}

#[test]
fn cas_bfa1_spawn_remote_parent_divergence_refuses_even_offline() {
    with_fixture(|f| {
        git(&f.repo, &["checkout", "epic/round2"]);
        commit(&f.repo, "epic-work");
        let epic = git(&f.repo, &["rev-parse", "HEAD"]);
        git(&f.repo, &["checkout", "main"]);
        f.advance_parent();
        git(&f.repo, &["update-ref", "refs/heads/main", &f.old]);
        f.offline();
        let error = f
            .prepare()
            .err()
            .expect("cached remote-parent divergence must refuse")
            .to_string();
        assert!(error.contains("have diverged"), "{error}");
        assert_eq!(git(&f.repo, &["rev-parse", "epic/round2"]), epic);
    });
}

#[test]
fn cas_bfa1_spawn_closed_or_deleted_epic_is_not_republished() {
    with_fixture(|f| {
        let tasks = open_task_store_cached(&f.root).unwrap();
        let mut epic = tasks.get("cas-round2-epic").unwrap();
        epic.status = cas_types::TaskStatus::Closed;
        tasks.update(&epic).unwrap();
        git(&f.repo, &["branch", "-D", "epic/round2"]);
        let result = f
            .app
            .prepare_worker_spawn(Some("round2-worker"), true, None);
        assert!(result.is_err(), "closed pinned epic must refuse");
        assert!(
            git(
                &f.origin,
                &[
                    "for-each-ref",
                    "--format=%(refname)",
                    "refs/heads/epic/round2"
                ]
            )
            .is_empty()
        );
    });
    with_fixture(|f| {
        git(&f.repo, &["branch", "-D", "epic/round2"]);
        let result = f
            .app
            .prepare_worker_spawn(Some("round2-worker"), true, None);
        assert!(
            result.is_err(),
            "deleted pinned lane must not be recreated from trunk"
        );
        assert!(
            git(
                &f.origin,
                &[
                    "for-each-ref",
                    "--format=%(refname)",
                    "refs/heads/epic/round2"
                ]
            )
            .is_empty()
        );
    });
}

#[test]
fn cas_bfa1_spawn_preserves_unique_epic_work() {
    with_fixture(|f| {
        git(&f.repo, &["checkout", "epic/round2"]);
        commit(&f.repo, "epic-work");
        let tip = git(&f.repo, &["rev-parse", "HEAD"]);
        git(&f.repo, &["checkout", "main"]);
        let prep = f.prepare().unwrap();
        assert_eq!(git(&f.repo, &["rev-parse", "epic/round2"]), tip);
        assert_eq!(git(&f.origin, &["rev-parse", "epic/round2"]), tip);
        assert!(
            prep.warnings
                .iter()
                .any(|line| line.contains("unique") && line.contains("preserved"))
        );
    });
}
