use crate::support::*;
use cas::mcp::tools::*;
use rmcp::handler::server::wrapper::Parameters;
use rusqlite::Connection;

#[tokio::test]
async fn test_task_create_basic() {
    let (_temp, service) = setup_cas();

    let req = TaskCreateRequest {
        depth: None,
        title: "Test task".to_string(),
        description: Some("Task description".to_string()),
        priority: 2,
        task_type: "task".to_string(),
        labels: Some("test,task".to_string()),
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    assert!(text.contains("Created task"));
    assert!(text.contains("Test task"));
}

#[tokio::test]
async fn test_task_create_and_start() {
    let (_temp, service) = setup_cas();

    // Create task
    let req = TaskCreateRequest {
        depth: None,
        title: "Auto-start task".to_string(),
        description: None,
        priority: 1,
        task_type: "feature".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    assert!(text.contains("Created"));
    let task_id = extract_task_id(&text).expect("should have task ID");

    // Start the task separately using the start action
    let start_req = IdRequest {
        id: task_id.to_string(),
    };
    let start_result = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("task_start should succeed");

    let start_text = extract_text(start_result);
    // After starting, the output includes claim info (e.g., "claimed until HH:MM")
    assert!(
        start_text.contains("claimed"),
        "Task start should show claimed: {start_text}"
    );
    // Workflow guidance should be included when starting a task
    assert!(
        start_text.contains("Workflow Guidance"),
        "Task start should include workflow guidance: {start_text}"
    );
    assert!(
        start_text.contains("mcp__cas__search"),
        "Workflow guidance should mention CAS search: {start_text}"
    );
}

#[tokio::test]
async fn no_code_create_and_start_warn_that_external_ref_is_required_at_close() {
    let (_temp, service) = setup_cas();
    let created = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: Some("light".to_string()),
            title: "Publish an operational report".to_string(),
            description: None,
            priority: 2,
            task_type: "chore".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: Some("no-code".to_string()),
            epic: None,
        }))
        .await
        .expect("create no-code task");
    let created_text = extract_text(created);
    assert!(created_text.contains("No-code close requirement"));
    assert!(created_text.contains("external_ref"));
    let task_id = extract_task_id(&created_text).unwrap().to_string();

    let started = service
        .cas_task_start(Parameters(IdRequest { id: task_id }))
        .await
        .expect("start no-code task");
    let started_text = extract_text(started);
    assert!(started_text.contains("No-code close requirement"));
    assert!(started_text.contains("external_ref"));
}

/// Test that epic creation creates a branch, not a worktree
///
/// This is a regression test for the bug where supervisors were getting
/// worktrees when creating epics. Epics should only get branches.
#[tokio::test]
async fn test_epic_creates_branch_not_worktree() {
    use std::process::Command;

    let (temp, service) = setup_cas();

    // Initialize git repo (required for branch creation)
    Command::new("git")
        .args(["init"])
        .current_dir(temp.path())
        .output()
        .expect("Failed to init git");

    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(temp.path())
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(temp.path())
        .output()
        .unwrap();

    // Create initial commit (required for branch creation)
    std::fs::write(temp.path().join("README.md"), "# Test").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(temp.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(temp.path())
        .output()
        .unwrap();

    // Create epic task
    let req = TaskCreateRequest {
        depth: None,
        title: "Add User Authentication".to_string(),
        description: Some("Epic for auth feature".to_string()),
        priority: 1,
        task_type: "epic".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let create_text = extract_text(result);
    let epic_id = extract_task_id(&create_text).expect("should have epic ID");
    let expected_branch = format!("epic/add-user-authentication-{epic_id}");

    // Should contain branch info, not worktree info
    assert!(
        create_text.contains("Epic branch created") || create_text.contains("epic/"),
        "Epic should create branch on create: {create_text}"
    );
    assert!(
        !create_text.contains("Worktree created"),
        "Epic should NOT create worktree: {create_text}"
    );

    // Start the epic (which triggers branch creation)
    let start_req = IdRequest {
        id: epic_id.to_string(),
    };
    let start_result = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("task_start should succeed");

    let text = extract_text(start_result);
    println!("Epic start output: {text}");

    // Start should not create a worktree for epics
    assert!(
        !text.contains("Worktree created"),
        "Epic should NOT create worktree: {text}"
    );

    // Verify git branch was created
    let branch_list = Command::new("git")
        .args(["branch", "--list", "epic/*"])
        .current_dir(temp.path())
        .output()
        .expect("Failed to list branches");

    let branches = String::from_utf8_lossy(&branch_list.stdout);
    println!("Git branches: {branches}");
    assert!(
        branches.contains(&expected_branch),
        "Expected {expected_branch} branch, got: {branches}"
    );

    // Verify no worktree directory was created
    let worktree_dir = temp.path().parent().unwrap().join(format!(
        "{}-worktrees",
        temp.path().file_name().unwrap().to_str().unwrap()
    ));
    assert!(
        !worktree_dir.exists(),
        "Worktree directory should not exist"
    );
}

#[tokio::test]
async fn test_task_create_invalid_epic_does_not_persist_task() {
    let (_temp, service) = setup_cas();

    let req = TaskCreateRequest {
        depth: None,
        title: "Should fail atomic create".to_string(),
        description: Some("invalid epic should not leave orphan task".to_string()),
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: Some("cas-does-not-exist".to_string()),
    };

    let result = service.cas_task_create(Parameters(req)).await;
    assert!(result.is_err(), "Create should fail for invalid epic");

    let list_req = TaskListRequest {
        scope: "all".to_string(),
        limit: Some(20),
        status: None,
        task_type: None,
        label: None,
        assignee: None,
        epic: None,
        sort: None,
        sort_order: None,
    };
    let list_result = service
        .cas_task_list(Parameters(list_req))
        .await
        .expect("task_list should succeed");
    let text = extract_text(list_result);
    assert!(
        text.contains("No tasks found matching filters"),
        "Task create should be atomic; unexpected task list output: {text}"
    );
}

#[tokio::test]
async fn test_task_create_surfaces_dependency_write_failure() {
    let (temp, service) = setup_cas();

    let blocker = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Blocking task".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: None,
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await
        .expect("blocker create should succeed");
    let blocker_id = extract_task_id(&extract_text(blocker))
        .expect("blocker id")
        .to_string();

    let db_path = temp.path().join(".cas").join("cas.db");
    let conn = Connection::open(&db_path).expect("open sqlite db");
    conn.execute(
        "CREATE TRIGGER fail_dependency_insert
         BEFORE INSERT ON dependencies
         BEGIN
             SELECT RAISE(FAIL, 'forced dependency insert failure');
         END;",
        [],
    )
    .expect("create insert failure trigger");

    let create_result = service
        .cas_task_create(Parameters(TaskCreateRequest {
            depth: None,
            title: "Should fail dependency write".to_string(),
            description: None,
            priority: 2,
            task_type: "task".to_string(),
            labels: None,
            notes: None,
            blocked_by: Some(blocker_id),
            design: None,
            acceptance_criteria: None,
            external_ref: None,
            assignee: None,
            demo_statement: None,
            execution_note: None,
            epic: None,
        }))
        .await;
    assert!(
        create_result.is_err(),
        "Dependency write failure should be returned to caller"
    );

    let list_text = extract_text(
        service
            .cas_task_list(Parameters(TaskListRequest {
                scope: "all".to_string(),
                limit: Some(20),
                status: None,
                task_type: None,
                label: None,
                assignee: None,
                epic: None,
                sort: None,
                sort_order: None,
            }))
            .await
            .expect("task_list should succeed"),
    );
    assert!(
        !list_text.contains("Should fail dependency write"),
        "create_atomic should roll back task on dependency insert error: {list_text}"
    );
}

// =============================================================================
// cas-0344: per-task depth flag end-to-end coverage (EPIC cas-1255)
// =============================================================================

fn depth_create_req(title: &str, depth: Option<&str>) -> TaskCreateRequest {
    TaskCreateRequest {
        depth: depth.map(|s| s.to_string()),
        title: title.to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    }
}

#[tokio::test]
async fn test_task_create_with_light_depth_shows_light() {
    let (_temp, service) = setup_cas();

    let text = extract_text(
        service
            .cas_task_create(Parameters(depth_create_req("light task", Some("light"))))
            .await
            .expect("create should succeed"),
    );
    let id = extract_task_id(&text).expect("task id").to_string();

    let show = extract_text(
        service
            .cas_task_show(Parameters(TaskShowRequest {
                id,
                with_deps: false,
            }))
            .await
            .expect("show should succeed"),
    );
    assert!(
        show.contains("Depth: light"),
        "expected light depth: {show}"
    );
}

#[tokio::test]
async fn test_task_create_without_depth_defaults_to_deep() {
    let (_temp, service) = setup_cas();

    let text = extract_text(
        service
            .cas_task_create(Parameters(depth_create_req("default task", None)))
            .await
            .expect("create should succeed"),
    );
    let id = extract_task_id(&text).expect("task id").to_string();

    let show = extract_text(
        service
            .cas_task_show(Parameters(TaskShowRequest {
                id,
                with_deps: false,
            }))
            .await
            .expect("show should succeed"),
    );
    assert!(
        show.contains("Depth: deep"),
        "expected deep default: {show}"
    );
}

#[tokio::test]
async fn test_task_create_invalid_depth_is_rejected() {
    let (_temp, service) = setup_cas();

    let result = service
        .cas_task_create(Parameters(depth_create_req("bad depth", Some("medium"))))
        .await;
    assert!(result.is_err(), "invalid depth must be rejected");
    let msg = result.err().unwrap().message.to_string();
    assert!(
        msg.contains("Invalid depth"),
        "error should explain invalid depth: {msg}"
    );
}

#[tokio::test]
async fn test_task_update_depth_to_light() {
    let (_temp, service) = setup_cas();

    let text = extract_text(
        service
            .cas_task_create(Parameters(depth_create_req("upgrade me", None)))
            .await
            .expect("create should succeed"),
    );
    let id = extract_task_id(&text).expect("task id").to_string();

    // Update depth -> light
    let update_text = extract_text(
        service
            .cas_task_update(Parameters(TaskUpdateRequest {
                blocked_by: None,
                id: id.clone(),
                title: None,
                notes: None,
                priority: None,
                labels: None,
                description: None,
                design: None,
                acceptance_criteria: None,
                demo_statement: None,
                execution_note: None,
                external_ref: None,
                assignee: None,
                status: None,
                epic: None,
                origin_project: None,
                epic_verification_owner: None,
                depth: Some("light".to_string()),
            }))
            .await
            .expect("update should succeed"),
    );
    assert!(
        update_text.contains("depth"),
        "update should report depth change: {update_text}"
    );

    let show = extract_text(
        service
            .cas_task_show(Parameters(TaskShowRequest {
                id,
                with_deps: false,
            }))
            .await
            .expect("show should succeed"),
    );
    assert!(
        show.contains("Depth: light"),
        "expected light after update: {show}"
    );
}

// ---------------------------------------------------------------------------
// cas-a85e (GH #99): a follow-on epic created while the checkout is on the
// previous epic branch was cut from main — 36 commits behind in the report —
// so the new epic started empty and a worker could overwrite deliverables.
// ---------------------------------------------------------------------------

fn git_in(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} failed to run: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo_with_commit(repo: &std::path::Path) {
    git_in(repo, &["init", "-q"]);
    git_in(repo, &["config", "user.email", "test@test.com"]);
    git_in(repo, &["config", "user.name", "Test"]);
    std::fs::write(repo.join("README.md"), "# Test").unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-q", "-m", "Initial commit"]);
}

/// Give the task-create fixture a real `origin`, then land a commit there
/// without advancing the supervisor checkout. `cas_task_create` must fetch
/// before it chooses a base, so this models the normal long-running factory
/// session where local trunk is stale while origin/trunk moved remotely.
fn remote_advance_without_local_fetch(repo: &std::path::Path) -> String {
    let origin = repo.join("origin.git");
    git_in(
        repo,
        &[
            "clone",
            "-q",
            "--bare",
            repo.to_str().expect("utf-8 repo path"),
            origin.to_str().expect("utf-8 origin path"),
        ],
    );
    git_in(repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    let trunk = git_in(repo, &["branch", "--show-current"]);
    git_in(repo, &["push", "-q", "-u", "origin", &trunk]);

    let updater = repo.join("updater");
    git_in(
        repo,
        &[
            "clone",
            "-q",
            origin.to_str().expect("utf-8 origin path"),
            updater.to_str().expect("utf-8 updater path"),
        ],
    );
    git_in(&updater, &["config", "user.email", "updater@test.com"]);
    git_in(&updater, &["config", "user.name", "Updater"]);
    std::fs::write(updater.join("remote-only.txt"), "landed remotely").unwrap();
    git_in(&updater, &["add", "."]);
    git_in(&updater, &["commit", "-q", "-m", "remote advance"]);
    git_in(&updater, &["push", "-q", "origin", &trunk]);
    git_in(&updater, &["rev-parse", "HEAD"])
}

fn epic_create_request(title: &str) -> TaskCreateRequest {
    TaskCreateRequest {
        depth: None,
        title: title.to_string(),
        description: None,
        priority: 1,
        task_type: "epic".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    }
}

#[tokio::test]
async fn test_follow_on_epic_continues_active_epic_branch_and_says_so() {
    let (temp, service) = setup_cas();
    let repo = temp.path();
    init_repo_with_commit(repo);

    // A prior epic accumulated work that trunk has never seen.
    git_in(repo, &["checkout", "-q", "-b", "epic/first-cas-aaaa"]);
    std::fs::write(repo.join("deliverable.txt"), "prior epic work").unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-q", "-m", "prior epic work"]);
    std::fs::write(repo.join("report.md"), "prior epic report").unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-q", "-m", "prior epic report"]);
    let prior_tip = git_in(repo, &["rev-parse", "HEAD"]);

    let created = service
        .cas_task_create(Parameters(epic_create_request("Follow On Epic")))
        .await
        .expect("epic create should succeed");
    let text = extract_text(created);
    let epic_id = extract_task_id(&text).expect("should have epic ID");
    let branch = format!("epic/follow-on-epic-{epic_id}");

    assert_eq!(
        git_in(repo, &["rev-parse", &branch]),
        prior_tip,
        "the follow-on epic branch must continue the active epic branch: {text}"
    );
    let files = git_in(repo, &["ls-tree", "-r", "--name-only", &branch]);
    assert!(
        files.contains("deliverable.txt") && files.contains("report.md"),
        "prior epic deliverables must be reachable from the new epic branch: {files}"
    );
    assert!(
        text.contains("Base: 'epic/first-cas-aaaa'"),
        "the creation message must name the base actually used: {text}"
    );
    assert!(
        text.contains("2 commit(s) ahead"),
        "the creation message must state the divergence with a commit count: {text}"
    );
}

#[tokio::test]
async fn test_epic_create_states_the_gap_when_head_is_ahead_but_not_an_epic() {
    let (temp, service) = setup_cas();
    let repo = temp.path();
    init_repo_with_commit(repo);
    let trunk = git_in(repo, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let trunk_sha = git_in(repo, &["rev-parse", &trunk]);

    // An incidental worker/feature HEAD must never seed an epic (cas-dc28) —
    // but the gap it leaves behind must be stated, not swallowed.
    git_in(repo, &["checkout", "-q", "-b", "factory/some-worker"]);
    std::fs::write(repo.join("wip.txt"), "worker wip").unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-q", "-m", "worker wip"]);

    let created = service
        .cas_task_create(Parameters(epic_create_request("Unrelated Epic")))
        .await
        .expect("epic create should succeed");
    let text = extract_text(created);
    let epic_id = extract_task_id(&text).expect("should have epic ID");
    let branch = format!("epic/unrelated-epic-{epic_id}");

    assert_eq!(
        git_in(repo, &["rev-parse", &branch]),
        trunk_sha,
        "a factory branch must not become an epic base: {text}"
    );
    assert!(
        text.contains("factory/some-worker") && text.contains("1 commit(s) ahead"),
        "the excluded commits must be named with a count: {text}"
    );
}

#[tokio::test]
async fn test_epic_create_keeps_trunk_and_warns_when_active_epic_has_diverged() {
    let (temp, service) = setup_cas();
    let repo = temp.path();
    init_repo_with_commit(repo);
    let trunk = git_in(repo, &["rev-parse", "--abbrev-ref", "HEAD"]);

    git_in(repo, &["checkout", "-q", "-b", "epic/first-cas-aaaa"]);
    std::fs::write(repo.join("epic-only.txt"), "epic only").unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-q", "-m", "epic only"]);

    git_in(repo, &["checkout", "-q", &trunk]);
    std::fs::write(repo.join("trunk-only.txt"), "trunk only").unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-q", "-m", "trunk only"]);
    let trunk_sha = git_in(repo, &["rev-parse", &trunk]);
    git_in(repo, &["checkout", "-q", "epic/first-cas-aaaa"]);

    let created = service
        .cas_task_create(Parameters(epic_create_request("Diverged Follow On")))
        .await
        .expect("epic create should succeed");
    let text = extract_text(created);
    let epic_id = extract_task_id(&text).expect("should have epic ID");
    let branch = format!("epic/diverged-follow-on-{epic_id}");

    assert_eq!(
        git_in(repo, &["rev-parse", &branch]),
        trunk_sha,
        "auto-stacking a diverged epic would drop the trunk-only commit: {text}"
    );
    assert!(
        text.contains("DIVERGED") && text.contains("epic/first-cas-aaaa"),
        "divergence must be surfaced for the supervisor to resolve: {text}"
    );
}

#[tokio::test]
async fn test_epic_create_uses_fetched_origin_tip_when_local_trunk_is_stale_cas_201e() {
    let (temp, service) = setup_cas();
    let repo = temp.path();
    init_repo_with_commit(repo);
    let remote_tip = remote_advance_without_local_fetch(repo);
    let trunk = git_in(repo, &["branch", "--show-current"]);
    let stale_local_tip = git_in(repo, &["rev-parse", &trunk]);

    let created = service
        .cas_task_create(Parameters(epic_create_request("Fresh Remote Base")))
        .await
        .expect("epic create should succeed");
    let text = extract_text(created);
    let epic_id = extract_task_id(&text).expect("should have epic ID");
    let branch = format!("epic/fresh-remote-base-{epic_id}");

    assert_ne!(
        stale_local_tip, remote_tip,
        "fixture must leave local trunk stale"
    );
    assert_eq!(
        git_in(repo, &["rev-parse", &branch]),
        remote_tip,
        "epic must be cut from the freshly fetched origin tip: {text}"
    );
    assert!(
        git_in(repo, &["ls-tree", "-r", "--name-only", &branch]).contains("remote-only.txt"),
        "epic must include the commit that existed only on origin: {text}"
    );
    assert!(text.contains(&format!("Base: 'origin/{trunk}'")), "{text}");
    assert!(text.contains("BASE REFRESHED"), "{text}");
}

#[tokio::test]
async fn test_epic_create_keeps_equal_local_and_remote_behavior_quiet_cas_201e() {
    let (temp, service) = setup_cas();
    let repo = temp.path();
    init_repo_with_commit(repo);
    let origin = repo.join("origin.git");
    git_in(
        repo,
        &[
            "clone",
            "-q",
            "--bare",
            repo.to_str().unwrap(),
            origin.to_str().unwrap(),
        ],
    );
    git_in(repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    let trunk = git_in(repo, &["branch", "--show-current"]);
    git_in(repo, &["push", "-q", "-u", "origin", &trunk]);
    let expected_tip = git_in(repo, &["rev-parse", &trunk]);

    let created = service
        .cas_task_create(Parameters(epic_create_request("Equal Remote Base")))
        .await
        .expect("epic create should succeed");
    let text = extract_text(created);
    let epic_id = extract_task_id(&text).expect("should have epic ID");
    let branch = format!("epic/equal-remote-base-{epic_id}");

    assert_eq!(git_in(repo, &["rev-parse", &branch]), expected_tip);
    assert!(
        !text.contains("BASE REFRESHED") && !text.contains("BASE REF DIVERGED"),
        "equal refs must not manufacture a warning: {text}"
    );
}

#[tokio::test]
async fn test_epic_create_reports_divergent_local_and_remote_base_cas_201e() {
    let (temp, service) = setup_cas();
    let repo = temp.path();
    init_repo_with_commit(repo);
    let _remote_tip = remote_advance_without_local_fetch(repo);
    let trunk = git_in(repo, &["branch", "--show-current"]);
    std::fs::write(repo.join("local-only.txt"), "keep local work").unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-q", "-m", "local divergence"]);
    let local_tip = git_in(repo, &["rev-parse", &trunk]);

    let created = service
        .cas_task_create(Parameters(epic_create_request("Divergent Remote Base")))
        .await
        .expect("epic create should succeed");
    let text = extract_text(created);
    let epic_id = extract_task_id(&text).expect("should have epic ID");
    let branch = format!("epic/divergent-remote-base-{epic_id}");

    assert_eq!(
        git_in(repo, &["rev-parse", &branch]),
        local_tip,
        "a genuinely divergent local branch must not be silently overridden: {text}"
    );
    assert!(text.contains("BASE REF DIVERGED"), "{text}");
    assert!(
        text.contains("local-only") && text.contains("remote-only"),
        "{text}"
    );
}
