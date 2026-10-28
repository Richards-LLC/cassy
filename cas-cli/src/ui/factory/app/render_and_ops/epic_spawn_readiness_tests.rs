use super::*;
use std::path::{Path, PathBuf};

pub(super) fn run_bash_remedy(repo: &Path, command: &str, preamble: &str) -> std::process::Output {
    use std::io::Write;
    let mut shell = std::process::Command::new("bash")
        .args(["--noprofile", "--norc", "-i"])
        .env("HISTFILE", "/dev/null")
        .env("PS1", "")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(repo).spawn().unwrap();
    writeln!(shell.stdin.take().unwrap(),
        "set -H\nhistory -s 'echo history-expanded'\n{preamble}\n{command}\nexit $?").unwrap();
    shell.wait_with_output().unwrap()
}

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
        ("XDG_CONFIG_HOME", home.path().join(".config").to_str().unwrap()),
        ("CAS_FACTORY_SESSION", "bfa1-round2"),
        ("CAS_PUSH_EPIC_BRANCH", "0"),
        (
            "GIT_TRACE2_EVENT",
            temp.path().join("git-trace.json").to_str().unwrap(),
        ),
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
    epic.deliverables.epic_branch_origin =
        crate::mcp::tools::core::task::lifecycle::epic_lane::branch_origin("epic/round2");
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
    let app = make_app(&repo, &root);
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
fn cas_5dc9_unpublished_peer_epic_reports_creator_host_and_publish_remedy() {
    // Two repositories share task JSON and a real bare origin, but not refs.
    // Cover both creation routes; neither may lose the originating host in sync.
    let mut failures = Vec::new();
    for adopted_branch in [
        None,
        Some("epic/$(touch${IFS}SUBSTITUTED)"),
        Some("epic/{a,b}"),
        Some("epic/a!e"),
        Some("epic/a'quote"),
        Some("epic/café"),
        Some("epic/control\u{85}label"),
        Some("epic/invisible\u{202e}label"),
        Some("epic/invisible\u{200b}label"),
    ] {
        let adopted = adopted_branch.is_some();
        with_fixture(|f| {
            crate::cloud::device::DeviceConfig {
                device_id: "source-device".into(), name: None, registered_at: "now".into(),
            }.save().unwrap();
            let core = crate::mcp::CasCore::with_daemon(f.root.clone(), None, None);
            crate::store::known_repos::ensure_host_schema().unwrap();
            core.register_agent("peer-lane-creator".into(), "peer-lane-creator".into(), None)
                .unwrap();
            let request = serde_json::from_value(serde_json::json!({
                "title": "Peer-only epic", "task_type": "epic", "risk": "none"
            }))
            .unwrap();
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(core.cas_task_create_with_target(
                    request,
                    if adopted { f.repo.to_str() } else { None },
                    adopted_branch,
                    true,
                ))
                .unwrap();
            let epic = core
                .open_task_store()
                .unwrap()
                .list(None)
                .unwrap()
                .into_iter()
                .find(|task| task.title == "Peer-only epic")
                .unwrap();
            let branch = epic.branch.as_deref().unwrap();
            let origin_json = serde_json::to_value(&epic).unwrap();
            if origin_json["deliverables"]["epic_branch_origin"]["device_id"] != "source-device" {
                failures.push("creation did not retain device identity".into());
            }
            git(&f.repo, &["checkout", branch]);
            commit(&f.repo, "unpublished-work");
            git(&f.repo, &["checkout", "main"]);
            let tip = git(&f.repo, &["rev-parse", branch]);
            assert!(
                git(
                    &f.origin,
                    &[
                        "for-each-ref",
                        "--format=%(refname)",
                        &format!("refs/heads/{branch}")
                    ]
                )
                .is_empty()
            );
            let peer = f.repo.parent().unwrap().join("peer clone");
            git(
                f.repo.parent().unwrap(),
                &[
                    "clone",
                    "--branch",
                    "main",
                    f.origin.to_str().unwrap(),
                    peer.to_str().unwrap(),
                ],
            );
            let peer_root = crate::store::init_cas_dir(&peer).unwrap();
            std::fs::copy(f.root.join("config.toml"), peer_root.join("config.toml")).unwrap();
            let peer_tasks = open_task_store_cached(&peer_root).unwrap();
            // A different host owns the peer clone in production. Retain the
            // real create assertion, then inject the synced display label.
            let current_host = crate::cloud::device::DeviceConfig::hostname().unwrap();
            assert_eq!(
                epic.deliverables.epic_branch_origin.as_ref().unwrap().host,
                current_host
            );
            let host = format!("source-{current_host}");
            let mut shared_epic = epic.clone();
            shared_epic
                .deliverables
                .epic_branch_origin
                .as_mut()
                .unwrap()
                .host = format!("\u{1b}{host}\r\n\t\u{85}");
            let wire = serde_json::to_vec(&shared_epic).unwrap();
            let shared: cas_types::Task = serde_json::from_slice(&wire).unwrap();
            peer_tasks.add(&shared).unwrap();
            peer_tasks
                .add(&cas_types::Task::new(
                    "cas-peer-child".into(),
                    "Peer child".into(),
                ))
                .unwrap();
            peer_tasks
                .add_dependency(&cas_types::Dependency {
                    from_id: "cas-peer-child".into(),
                    to_id: epic.id.clone(),
                    dep_type: cas_types::DependencyType::ParentChild,
                    created_at: chrono::Utc::now(),
                    created_by: None,
                })
                .unwrap();
            crate::cloud::device::DeviceConfig {
                device_id: "peer-device".into(), name: None, registered_at: "now".into(),
            }.save().unwrap();
            let error = prepare_existing_task(&peer, &peer_root, "cas-peer-child")
                .err()
                .expect("unpublished peer lane must not be recreated")
                .to_string();
            let invisible_label = match branch {
                "epic/invisible\u{202e}label" => Some("epic/invisible\\u{202e}label"),
                "epic/invisible\u{200b}label" => Some("epic/invisible\\u{200b}label"),
                _ => None,
            };
            let control_ref = branch.chars().any(char::is_control) || invisible_label.is_some();
            if let Some(escaped) = invisible_label {
                assert!(error.contains(escaped), "invisible ref must be visible: {error:?}");
                assert!(!error.contains(branch), "invisible ref printed literally: {error:?}");
                assert!(!error.lines().any(|line| line.starts_with("git ")), "invisible ref must not produce executable advice: {error:?}");
                if branch.contains('\u{202e}') {
                    // pin: invisible-ref refusal layout is the terminal-QA fixture.
                    assert_eq!(error, include_str!("../../../../../../docs/design/cli/epic-invisible-ref.capture.txt").trim_end());
                }
            } else if control_ref {
                // pin: control-bearing refs require visible identity and manual action, never a command.
                assert_eq!(error, include_str!("../../../../../../docs/design/cli/epic-control-ref.capture.txt").trim_end());
                if !error.contains("contains control characters") || !error.contains("\\u{85}")
                    || error.lines().any(|line| line.starts_with("git "))
                    || error.chars().any(|c| c.is_control() && c != '\n')
                {
                    failures.push(format!("control-bearing ref lacked safe refusal: {error:?}"));
                }
            } else if !error.contains(&format!("epic branch not published by {host}"))
                || error.lines().count() != 3
                || error.chars().any(|c| c.is_control() && c != '\n')
            {
                failures.push(format!("synced host was not sanitized: {error:?}"));
            }
            if error.contains("spawn refused:") {
                failures.push(format!("shared readiness diagnostic names spawn: {error}"));
            }
            assert!(!error.contains("deleted epic lane"), "{error}");
            for repo in [&peer, &f.origin] {
                assert!(
                    git(
                        repo,
                        &[
                            "for-each-ref",
                            "--format=%(refname)",
                            &format!("refs/heads/{branch}")
                        ]
                    )
                    .is_empty(),
                    "refusal changed a ref"
                );
            }
            // pin: the terminal fixture is the actual spawn diagnostic after
            // normalizing host/ref identity, not a separately authored renderer.
            let normalized = format!("{}\n", error.lines().map(|line| {
                if line.starts_with("git push ") { "git push origin epic/peer".to_string() }
                else { line.replace(&host, "soundwave").replace(branch, "epic/peer")
                    .replace(&branch.chars().filter(|c| !c.is_control()).collect::<String>(), "epic/peer") }
            }).collect::<Vec<_>>().join("\n"));
            let expected =
                include_str!("../../../../../../docs/design/cli/epic-peer-publication.capture.txt")
                    .to_string();
            if !control_ref && normalized != expected {
                failures.push(format!("diagnostic capture mismatch: {normalized:?}"));
            }
            if branch == "epic/{a,b}" {
                // pin: the displayed quoted command comes from actual peer preparation.
                assert_eq!(error.replace(&host, "soundwave"), include_str!("../../../../../../docs/design/cli/epic-brace-ref.capture.txt").trim_end());
            }
            if control_ref {
                // The source operator may push by hand; the refusal prints no command.
                git(&f.repo, &["push", "origin", branch]);
            } else if let Some(command) = error.lines().find(|line| line.starts_with("git push ")) {
                // Paste the entire displayed line, without stripping any markup.
                let published = run_bash_remedy(&f.repo, command, "");
                if !published.status.success() {
                    failures.push(format!("bash rejected remedy for {branch:?}: {}", String::from_utf8_lossy(&published.stderr)));
                    git(&f.repo, &["push", "origin", branch]);
                }
            } else {
                failures.push(format!("remedy is not a standalone shell line: {error}"));
                // Continue fixture setup so RED also checks published adoption.
                // The aggregated failure below remains authoritative.
                git(&f.repo, &["push", "origin", branch]);
            }
            assert!(
                !f.repo.join("SUBSTITUTED").exists(),
                "paste executed a branch token"
            );
            assert_eq!(git(&f.origin, &["rev-parse", branch]), tip);
            let prep = prepare_existing_task(&peer, &peer_root, "cas-peer-child")
                .expect("published remedy must make peer spawn ready");
            let work = prep.worktree_info.unwrap();
            let checkout = work.base_ref.as_deref().unwrap_or(&work.parent_branch);
            assert_eq!(git(&peer, &["rev-parse", checkout]), tip);
            assert_eq!(git(&f.repo, &["rev-parse", branch]), tip);
            if !adopted {
                git(
                    &f.repo,
                    &["push", "origin", "main:refs/heads/epic/published"],
                );
                let request = serde_json::from_value(serde_json::json!({
                    "title": "Adopt existing published lane", "task_type": "epic", "risk": "none"
                }))
                .unwrap();
                tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(core.cas_task_create_with_target(
                        request,
                        f.repo.to_str(),
                        Some("epic/published"),
                        true,
                    ))
                    .unwrap();
                let published = core
                    .open_task_store()
                    .unwrap()
                    .list(None)
                    .unwrap()
                    .into_iter()
                    .find(|task| task.title == "Adopt existing published lane")
                    .unwrap();
                if published.deliverables.epic_branch_origin.is_some() {
                    failures.push("adopter was stamped as the published lane's origin host".into());
                }
            }
        });
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn cas_bfa1_spawn_first_use_publishes_and_checks_out_current_base() {
    // Equal, behind and ahead lanes need no delivery inventory. An unrelated
    // malformed backlog row must not prevent preparing their current base.
    for relation in ["equal", "behind", "ahead"] {
        with_fixture(|f| {
            match relation {
                "behind" => f.advance_parent(),
                "ahead" => {
                    git(&f.repo, &["checkout", "epic/round2"]);
                    commit(&f.repo, "epic-work");
                    f.base = git(&f.repo, &["rev-parse", "HEAD"]);
                    git(&f.repo, &["checkout", "main"]);
                }
                _ => {}
            }
            let tasks = open_task_store_cached(&f.root).unwrap();
            tasks
                .add(&cas_types::Task::new(
                    "cas-unrelated".into(),
                    "Unrelated".into(),
                ))
                .unwrap();
            let db = rusqlite::Connection::open(f.root.join("cas.db")).unwrap();
            db.execute(
                "UPDATE tasks SET priority = 'invalid' WHERE id = 'cas-unrelated'",
                [],
            )
            .unwrap();
            assert!(
                tasks.list(None).is_err(),
                "delivery inventory is unavailable"
            );
            let prep = f
                .prepare()
                .expect("aligned lanes must not read the delivery inventory");
            assert_eq!(git(&f.origin, &["rev-parse", "epic/round2"]), f.base);
            let work = prep.worktree_info.unwrap();
            let checkout = work.base_ref.as_deref().unwrap_or(&work.parent_branch);
            assert_eq!(git(&f.repo, &["rev-parse", checkout]), f.base);
            assert!(prep.warnings.iter().any(|line| line.contains("published")));
        });
    }
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
    for diverged in [false, true] {
        with_fixture(|f| {
            if diverged {
                git(&f.repo, &["checkout", "epic/round2"]);
                commit(&f.repo, "planning-only-epic-work");
                f.old = git(&f.repo, &["rev-parse", "HEAD"]);
                git(&f.repo, &["checkout", "main"]);
            }
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
}

#[test]
fn spawn_unpublished_diverged_epic_resets_only_without_unique_delivery_cas_dab9() {
    for (in_trunk, delivered, online) in [
        (true, true, true),
        (false, false, true),
        (false, true, true),
        (false, false, false),
    ] {
        with_fixture(|f| {
            git(&f.repo, &["branch", "staging", "main"]);
            let tasks = open_task_store_cached(&f.root).unwrap();
            let mut epic = tasks.get("cas-round2-epic").unwrap();
            epic.deliverables
                .work_target
                .as_mut()
                .unwrap()
                .target_branch = "staging".into();
            tasks.update(&epic).unwrap();
            git(&f.repo, &["checkout", "epic/round2"]);
            commit(&f.repo, "epic-work");
            let tip = git(&f.repo, &["rev-parse", "HEAD"]);
            if delivered {
                let mut child = tasks.get("cas-round2-child").unwrap();
                child.deliverables.commit_hash = Some(tip.clone());
                tasks.update(&child).unwrap();
            }
            if delivered && !in_trunk {
                // An archived branch named master is not the authoritative
                // trunk. Its copy of this work must not authorize a reset.
                git(
                    &f.repo,
                    &[
                        "symbolic-ref",
                        "refs/remotes/origin/HEAD",
                        "refs/remotes/origin/main",
                    ],
                );
                git(&f.repo, &["branch", "master", &tip]);
                git(&f.repo, &["push", "origin", "master"]);
            }
            git(&f.repo, &["checkout", "main"]);
            if in_trunk {
                git(&f.repo, &["merge", "--ff-only", "epic/round2"]);
                git(&f.repo, &["push", "origin", "main"]);
            }
            git(&f.repo, &["checkout", "staging"]);
            commit(&f.repo, "parent-current");
            git(&f.repo, &["push", "origin", "staging"]);
            let parent = git(&f.repo, &["rev-parse", "HEAD"]);
            if !online {
                f.offline();
            }
            if online && !delivered {
                git(&f.repo, &["checkout", "epic/round2"]);
            }
            let result = f.prepare();
            if online && (in_trunk || !delivered) {
                let prep = result.expect("an unpublished stale plan must reset, not refuse");
                let notice = prep.warnings.join("\n");
                let reason = if delivered {
                    "TRUNK-COVERED WORK"
                } else {
                    "NO TASK DELIVERIES"
                };
                assert!(
                    notice.contains(&format!("EPIC BASE RESET (UNPUBLISHED, {reason})")),
                    "{notice}"
                );
                assert_eq!(
                    git(
                        &f.repo,
                        &["rev-parse", &format!("refs/cas/epic-base-resets/{tip}")]
                    ),
                    tip,
                    "the previous graph remains recoverable"
                );
                assert_eq!(git(&f.repo, &["rev-parse", "epic/round2"]), parent);
                assert_eq!(git(&f.origin, &["rev-parse", "epic/round2"]), parent);
                if !delivered {
                    assert!(
                        f.repo.join("parent-current").is_file(),
                        "the checked-out epic receives the parent tree"
                    );
                    assert!(
                        !f.repo.join("epic-work").exists(),
                        "old-only content remains in the recovery ref, not the reset checkout"
                    );
                    git(&f.repo, &["diff", "--exit-code", "HEAD"]);
                }
                let work = prep.worktree_info.unwrap();
                assert_eq!(
                    git(
                        &f.repo,
                        &[
                            "rev-parse",
                            work.base_ref.as_deref().unwrap_or(&work.parent_branch)
                        ]
                    ),
                    parent
                );
            } else {
                let error = result
                    .err()
                    .expect("unique deliveries or unverified publication must refuse")
                    .to_string();
                assert!(
                    error.contains("EPIC BASE REFRESH REFUSED") && error.contains("have diverged"),
                    "{error}"
                );
                assert!(
                    error.contains("git -C") && error.contains("merge origin/staging"),
                    "{error}"
                );
                assert_eq!(git(&f.repo, &["rev-parse", "epic/round2"]), tip);
                assert!(!f.repo.join(".cas/worktrees/round2-worker").exists());
            }
        });
    }
}

#[test]
fn spawn_reconcile_command_uses_existing_epic_checkout_cas_6e67() {
    with_fixture(|f| {
        let linked = f.repo.parent().unwrap().join("epic checkout");
        git(
            &f.repo,
            &["worktree", "add", linked.to_str().unwrap(), "epic/round2"],
        );
        commit(&linked, "epic-work");
        // Publishing prevents a planning-only reset and requires reconciliation.
        git(&f.repo, &["push", "origin", "epic/round2"]);
        f.advance_parent();
        let error = f
            .prepare()
            .err()
            .expect("divergence must refuse")
            .to_string();
        let command = error
            .split("Reconcile: `")
            .nth(1)
            .unwrap()
            .split('`')
            .next()
            .unwrap();
        let output = std::process::Command::new("sh")
            .args(["-c", command])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "printed reconciliation must work with a linked checkout: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        git(&linked, &["merge-base", "--is-ancestor", &f.base, "HEAD"]);
        assert_eq!(git(&f.repo, &["branch", "--show-current"]), "main");
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
        let error = result
            .err()
            .expect("closed pinned epic must refuse")
            .to_string();
        assert!(
            error.contains("pinned epic cas-round2-epic is closed")
                && error.contains("no lane was republished"),
            "{error}"
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
    with_fixture(|f| {
        git(&f.repo, &["checkout", "epic/round2"]);
        commit(&f.repo, "landed-epic-work");
        git(&f.repo, &["push", "origin", "epic/round2"]);
        git(&f.repo, &["checkout", "main"]);
        git(&f.repo, &["merge", "--ff-only", "epic/round2"]);
        git(&f.repo, &["push", "origin", "main"]);
        git(&f.origin, &["update-ref", "-d", "refs/heads/epic/round2"]);
        git(
            &f.repo,
            &["update-ref", "-d", "refs/remotes/origin/epic/round2"],
        );
        git(&f.repo, &["branch", "-D", "epic/round2"]);
        let result = f
            .app
            .prepare_worker_spawn(Some("round2-worker"), true, None);
        let error = result
            .err()
            .expect("deleted pinned lane must refuse")
            .to_string();
        assert!(
            error.contains("epic branch 'epic/round2' doesn't resolve")
                && error.contains("no lane was recreated"),
            "{error}"
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

#[test]
fn cas_bfa1_spawn_network_observation_is_reused() {
    for published in [false, true] {
        with_fixture(|f| {
            if published {
                git(&f.repo, &["push", "origin", "epic/round2"]);
            }
            f.advance_parent();
            let trace = f.repo.parent().unwrap().join("git-trace.json");
            std::fs::write(&trace, "").unwrap();
            f.prepare().unwrap();
            let entries: Vec<serde_json::Value> = std::fs::read_to_string(trace)
                .unwrap()
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            let count = |command: &str, branch: Option<&str>| {
                entries
                    .iter()
                    .filter(|entry| {
                        let Some(args) = entry.get("argv").and_then(|args| args.as_array()) else {
                            return false;
                        };
                        entry["event"] == "start"
                            && args.get(1).and_then(|value| value.as_str()) == Some(command)
                            && branch.is_none_or(|branch| {
                                args.iter().any(|value| value.as_str() == Some(branch))
                            })
                    })
                    .count()
            };
            assert_eq!(count("ls-remote", None), 1);
            assert_eq!(count("fetch", Some("main")), 1);
            assert_eq!(count("fetch", Some("epic/round2")), usize::from(published));
        });
    }
}

#[test]
fn cas_bfa1_real_spawn_notices_deduplicate_within_session() {
    for kind in ["UNVERIFIED", "PRESERVED", "UNPUBLISHED", "LOCAL-ONLY"] {
        with_fixture(|f| {
            if kind == "LOCAL-ONLY" {
                git(&f.repo, &["remote", "remove", "origin"]);
            }
            if kind == "UNVERIFIED" {
                f.offline();
            }
            if kind == "PRESERVED" {
                git(&f.repo, &["push", "origin", "epic/round2"]);
                git(&f.repo, &["checkout", "epic/round2"]);
                commit(&f.repo, "unique");
                git(&f.repo, &["checkout", "main"]);
            }
            if kind == "UNPUBLISHED" {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let hook = f.origin.join("hooks/pre-receive");
                    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
                    std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o755)).unwrap();
                }
                #[cfg(not(unix))]
                {
                    return;
                }
            }
            let mut original = None;
            for request in 1..=8 {
                let prep = f.prepare().unwrap();
                let notice = prep
                    .warnings
                    .iter()
                    .find(|line| line.starts_with(&format!("EPIC BRANCH {kind}")))
                    .expect("real notice");
                let id = crate::ui::factory::daemon::runtime::queue_and_events::enqueue_spawn_warning_notice(
                    &f.root, "supervisor", "session-a", Some(request), &format!("worker-{request}"), notice,
                ).unwrap();
                assert_eq!(*original.get_or_insert(id), id);
                if request == 8 {
                    let other = crate::ui::factory::daemon::runtime::queue_and_events::enqueue_spawn_warning_notice(
                        &f.root, "supervisor", "session-b", Some(request), "other-worker", notice,
                    ).unwrap();
                    assert_ne!(other, id, "each factory session gets its own notice");
                }
                assert!(!notice.contains("UNPUBLISHED: ;"));
            }
            assert_eq!(
                crate::store::open_prompt_queue_store(&f.root)
                    .unwrap()
                    .peek_all(100)
                    .unwrap()
                    .into_iter()
                    .filter(|row| matches!(
                        row.factory_session.as_deref(),
                        Some("session-a" | "session-b")
                    ))
                    .count(),
                2
            );
        });
    }
}

#[test]
fn cas_bfa1_assigned_epic_ignores_closed_unrelated_focus() {
    with_fixture(|f| {
        let tasks = open_task_store_cached(&f.root).unwrap();
        let mut other = cas_types::Task::new("cas-old-focus".into(), "Old focus".into());
        other.task_type = cas_types::TaskType::Epic;
        other.status = cas_types::TaskStatus::Closed;
        other.branch = Some("epic/old-focus".into());
        tasks.add(&other).unwrap();
        let path = crate::ui::factory::metadata_path("bfa1-round2");
        let mut metadata: crate::ui::factory::SessionMetadata =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        metadata.pinned_epic_id = Some(other.id);
        std::fs::write(path, serde_json::to_string(&metadata).unwrap()).unwrap();
        let prep = f.prepare().unwrap();
        assert_eq!(prep.worktree_info.unwrap().parent_branch, "epic/round2");
        assert_eq!(git(&f.origin, &["rev-parse", "epic/round2"]), f.base);
        assert!(
            git(
                &f.origin,
                &[
                    "for-each-ref",
                    "--format=%(refname)",
                    "refs/heads/epic/old-focus"
                ]
            )
            .is_empty()
        );
    });
}

#[test]
fn cas_bfa1_round3_adopted_and_targetless_lanes_keep_diverged_history() {
    for self_target in [true, false] {
        for published in [true, false] {
            with_fixture(|f| {
                let tasks = open_task_store_cached(&f.root).unwrap();
                let mut epic = tasks.get("cas-round2-epic").unwrap();
                epic.deliverables.work_target = self_target.then(|| cas_types::WorkTarget {
                    repo_selector: "project:bfa1-round2".into(),
                    target_branch: "epic/round2".into(),
                });
                tasks.update(&epic).unwrap();
                git(&f.repo, &["checkout", "epic/round2"]);
                commit(&f.repo, "adopted-work");
                let tip = git(&f.repo, &["rev-parse", "HEAD"]);
                if published {
                    git(&f.repo, &["push", "origin", "epic/round2"]);
                }
                git(&f.repo, &["checkout", "main"]);
                f.advance_parent();
                let prep = f
                    .prepare()
                    .expect("self-target and target-less lanes have no parent refresh");
                let work = prep.worktree_info.unwrap();
                assert_eq!(work.parent_branch, "epic/round2");
                assert_eq!(
                    git(
                        &f.repo,
                        &[
                            "rev-parse",
                            work.base_ref.as_deref().unwrap_or(&work.parent_branch)
                        ]
                    ),
                    tip
                );
                assert_eq!(git(&f.origin, &["rev-parse", "epic/round2"]), tip);
                assert_eq!(git(&f.repo, &["rev-parse", "main"]), f.base);
            });
        }
    }
}

#[test]
fn cas_bfa1_round3_remote_only_recorded_parent_is_usable() {
    with_fixture(|f| {
        f.advance_parent();
        git(&f.repo, &["branch", "release-parent", "main"]);
        git(&f.repo, &["push", "origin", "release-parent"]);
        git(&f.repo, &["branch", "-D", "release-parent"]);
        let tasks = open_task_store_cached(&f.root).unwrap();
        let mut epic = tasks.get("cas-round2-epic").unwrap();
        epic.deliverables
            .work_target
            .as_mut()
            .unwrap()
            .target_branch = "release-parent".into();
        tasks.update(&epic).unwrap();
        let prep = f.prepare().expect("origin-only parent is authoritative");
        assert_eq!(prep.worktree_info.unwrap().parent_branch, "epic/round2");
        assert_eq!(git(&f.repo, &["rev-parse", "epic/round2"]), f.base);
        assert_eq!(git(&f.origin, &["rev-parse", "epic/round2"]), f.base);
    });
}

#[test]
fn cas_bfa1_round3_cancelled_epic_is_terminal() {
    with_fixture(|f| {
        let tasks = open_task_store_cached(&f.root).unwrap();
        let mut epic = tasks.get("cas-round2-epic").unwrap();
        epic.status = cas_types::TaskStatus::Cancelled;
        tasks.update(&epic).unwrap();
        for task in [None, Some("cas-round2-child")] {
            let error = f
                .app
                .prepare_worker_spawn(Some("round2-worker"), true, task)
                .err()
                .expect("cancelled epic must refuse")
                .to_string();
            assert!(
                error.contains("cancelled") && error.contains("no lane was republished"),
                "{error}"
            );
        }
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
fn cas_bfa1_round3_live_epic_preferred_over_terminal_duplicate_lane() {
    with_fixture(|f| {
        let tasks = open_task_store_cached(&f.root).unwrap();
        let mut old = tasks.get("cas-round2-epic").unwrap();
        old.id = "cas-old-lane".into();
        old.status = cas_types::TaskStatus::Closed;
        old.priority = cas_types::Priority::CRITICAL;
        tasks.add(&old).unwrap();
        f.advance_parent();
        let prep = f
            .prepare()
            .expect("live lane owner must beat a terminal record");
        assert_eq!(prep.worktree_info.unwrap().parent_branch, "epic/round2");
        assert_eq!(git(&f.origin, &["rev-parse", "epic/round2"]), f.base);
    });
}

// These adapters only construct persisted inputs and invoke the real spawn
// preparation path; ancestry and parent selection remain production code.
fn make_app(repo: &Path, root: &Path) -> FactoryApp {
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
    FactoryApp::from_init_result(
        root.to_path_buf(),
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
        repo.to_path_buf(),
    )
    .unwrap()
}

pub(super) fn prepare_existing_task(
    repo: &Path,
    root: &Path,
    task: &str,
) -> anyhow::Result<WorkerSpawnPrep> {
    make_app(repo, root).prepare_worker_spawn(Some("legacy-path-worker"), true, Some(task))
}

pub(super) fn prepare_recorded_lane(
    repo: &Path,
    branch: &str,
    parent: &str,
) -> Result<Option<String>, String> {
    let root = crate::store::init_cas_dir(repo).map_err(|e| e.to_string())?;
    std::fs::write(
        root.join("config.toml"),
        "[project]\ncanonical_id = \"legacy-spawn-test\"\n",
    )
    .unwrap();
    let tasks = open_task_store_cached(&root).map_err(|e| e.to_string())?;
    let mut epic = cas_types::Task::new("cas-legacy-lane".into(), "Recorded lane".into());
    epic.task_type = cas_types::TaskType::Epic;
    epic.branch = Some(branch.into());
    // This legacy lane fixture represents retained task work. Planning-only
    // lanes are exercised by with_fixture and the GH #1317 regression.
    epic.deliverables.commit_hash = Some(git(repo, &["rev-parse", branch]));
    epic.deliverables.work_target = Some(cas_types::WorkTarget {
        repo_selector: "project:legacy-spawn-test".into(),
        target_branch: parent.into(),
    });
    tasks.add(&epic).map_err(|e| e.to_string())?;
    let mut child = cas_types::Task::new("cas-legacy-child".into(), "Targeted child".into());
    child.deliverables.work_target = Some(cas_types::WorkTarget {
        repo_selector: "project:legacy-spawn-test".into(),
        target_branch: branch.into(),
    });
    tasks.add(&child).map_err(|e| e.to_string())?;
    let prep = prepare_existing_task(repo, &root, &child.id).map_err(|e| e.to_string())?;
    Ok((!prep.warnings.is_empty()).then(|| prep.warnings.join("\n")))
}

#[cfg(unix)]
#[test]
fn cas_bfa1_round3_seen_remote_with_failed_fetch_is_unverified_not_deleted() {
    use std::os::unix::fs::PermissionsExt;
    with_fixture(|f| {
        git(&f.repo, &["push", "origin", "epic/round2"]);
        git(&f.repo, &["branch", "-D", "epic/round2"]);
        let script = f.repo.parent().unwrap().join("upload-pack.sh");
        let counter = f.repo.parent().unwrap().join("upload-pack-count");
        // Real local Git transport: ls-remote sees the lane, its subsequent
        // fetch fails, and no fallback may label that uncertainty a deletion.
        std::fs::write(&script, format!(
            "#!/bin/sh\nn=0\n[ ! -f '{0}' ] || n=$(cat '{0}')\nn=$((n + 1))\necho $n > '{0}'\n[ $n -ne 2 ] || exit 1\nexec git-upload-pack \"$@\"\n", counter.display()
        )).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        git(
            &f.repo,
            &[
                "config",
                "remote.origin.uploadpack",
                script.to_str().unwrap(),
            ],
        );
        let error = f
            .app
            .prepare_worker_spawn(Some("round2-worker"), true, None)
            .err()
            .expect("missing local lane with unverified origin must refuse")
            .to_string();
        assert!(
            error.contains("EPIC BRANCH UNVERIFIED") && error.contains("no lane was recreated"),
            "{error}"
        );
        assert!(!error.contains("deleted epic lane"), "{error}");
        assert_eq!(git(&f.origin, &["rev-parse", "epic/round2"]), f.old);
        assert!(
            git(
                &f.repo,
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
fn cas_bfa1_round4_no_parent_publishes_exact_tip_even_without_unique_work() {
    for self_target in [true, false] {
        with_fixture(|f| {
            let tasks = open_task_store_cached(&f.root).unwrap();
            let mut epic = tasks.get("cas-round2-epic").unwrap();
            epic.deliverables.work_target = self_target.then(|| cas_types::WorkTarget {
                repo_selector: "project:bfa1-round2".into(),
                target_branch: "epic/round2".into(),
            });
            tasks.update(&epic).unwrap();
            git(&f.repo, &["tag", "hotfix-base", &f.old]);
            f.advance_parent();
            let prep = f
                .prepare()
                .expect("an adopted tag lane stays at its recorded tip");
            assert_eq!(git(&f.repo, &["rev-parse", "epic/round2"]), f.old);
            assert_eq!(git(&f.origin, &["rev-parse", "epic/round2"]), f.old);
            let work = prep.worktree_info.unwrap();
            assert_eq!(
                git(
                    &f.repo,
                    &[
                        "rev-parse",
                        work.base_ref.as_deref().unwrap_or(&work.parent_branch)
                    ]
                ),
                f.old
            );
            assert!(
                !prep
                    .warnings
                    .iter()
                    .any(|line| line.contains("FAST-FORWARDED"))
            );
        });
    }
}

#[test]
fn cas_bfa1_round4_missing_recorded_parent_skips_refresh_with_notice() {
    for published in [false, true] {
        with_fixture(|f| {
            if published {
                git(&f.repo, &["push", "origin", "epic/round2"]);
            }
            let tasks = open_task_store_cached(&f.root).unwrap();
            let mut epic = tasks.get("cas-round2-epic").unwrap();
            epic.deliverables
                .work_target
                .as_mut()
                .unwrap()
                .target_branch = "epic/deleted-outer".into();
            tasks.update(&epic).unwrap();
            let prep = f
                .prepare()
                .expect("missing recorded parent must not refuse an existing lane");
            assert!(
                prep.warnings
                    .iter()
                    .any(|line| line.contains("EPIC BASE REFRESH SKIPPED")
                        && line.contains("epic/deleted-outer")),
                "{:?}",
                prep.warnings
            );
            assert_eq!(git(&f.repo, &["rev-parse", "epic/round2"]), f.old);
            if published {
                assert_eq!(git(&f.origin, &["rev-parse", "epic/round2"]), f.old);
            } else {
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
            }
        });
    }
}

#[test]
fn cas_bfa1_round4_quarantined_epic_cannot_authorize_publication() {
    with_fixture(|f| {
        f.advance_parent();
        let queue = crate::cloud::SyncQueue::open(&f.root).unwrap();
        queue.init().unwrap();
        queue
            .quarantine_row(
                crate::cloud::QUARANTINE_TASK,
                "cas-round2-epic",
                "fixture hidden authority",
            )
            .unwrap();
        assert!(
            open_task_store_cached(&f.root)
                .unwrap()
                .list(None)
                .unwrap()
                .iter()
                .all(|task| task.id != "cas-round2-epic")
        );
        // Hidden authority leaves an unmanaged base. Since cas-02ae an
        // unmanaged base must be verified on origin, so spawn refuses with the
        // cas-cd99 publication guidance instead of pushing it.
        let error = f
            .prepare()
            .err()
            .expect("an unpublished unmanaged base must refuse")
            .to_string();
        assert!(
            error.contains("SPAWN BASE NOT PUBLISHED")
                && error.contains("git push origin epic/round2"),
            "{error}"
        );
        assert!(!f.repo.join(".cas/worktrees/round2-worker").exists());
        assert_eq!(git(&f.repo, &["rev-parse", "epic/round2"]), f.old);
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
