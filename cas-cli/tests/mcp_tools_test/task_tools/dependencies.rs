//! cas-ac2e: `dep_add` confirmation + `dep_list`/`task show` rendering must
//! state dependency direction in plain, unambiguous words — no bare
//! `from -> to` arrow. See BUG-dep-add-direction-ambiguous-output-2026-07-08.md.
//!
//! Edge direction/semantics are unchanged by this task (verified by the
//! existing dependency-store tests continuing to pass); these tests cover
//! only the OUTPUT.

use crate::support::*;
use cas::mcp::tools::*;
use cas::store::open_task_store;
use cas::types::TaskStatus;
use rmcp::handler::server::wrapper::Parameters;

async fn create_task(service: &cas::mcp::CasCore, title: &str) -> String {
    let req = TaskCreateRequest {
        depth: None,
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
    };
    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");
    extract_task_id(&extract_text(result))
        .expect("should have task ID")
        .to_string()
}

/// AC: "Test asserting the new dep_add confirmation states 'blocked_by' in
/// plain words (no bare `A -> B` arrow)."
///
/// Reproduces the exact bug-doc scenario: `dep_add id=A to_id=B
/// dep_type=blocks` must NOT read as "A blocks B" — it must plainly say A
/// is blocked_by B (waits on B).
#[tokio::test]
async fn test_dep_add_blocks_confirmation_states_blocked_by_in_plain_words() {
    let (_temp, service) = setup_cas();

    let id_a = create_task(&service, "Task A").await;
    let id_b = create_task(&service, "Task B").await;

    let dep_req = DependencyRequest {
        from_id: id_a.clone(),
        to_id: id_b.clone(),
        dep_type: "blocks".to_string(),
    };
    let text = extract_text(
        service
            .cas_task_dep_add(Parameters(dep_req))
            .await
            .expect("dep_add should succeed"),
    );

    assert!(
        !text.contains("->"),
        "confirmation must not contain a bare arrow: {text}"
    );
    assert!(
        text.contains("blocked_by"),
        "confirmation must state blocked_by in plain words: {text}"
    );
    assert!(
        text.contains(&format!("{id_a} will not start until {id_b} is done")),
        "confirmation must spell out which task waits on which: {text}"
    );
    // Precondition sanity: the edge's actual effect really is A blocked_by B
    // (not the reverse) — confirms the output now matches the real semantics.
    let blocked = extract_text(
        service
            .cas_task_blocked(Parameters(TaskReadyBlockedRequest {
                limit: None,
                scope: "all".to_string(),
                sort: None,
                sort_order: None,
                epic: None,
            }))
            .await
            .expect("blocked should succeed"),
    );
    assert!(
        blocked.contains(&id_a),
        "task A must actually be blocked (by B), confirming the output matches reality: {blocked}"
    );
}

/// AC: "dep_list ... should render dependencies as 'blocked by: […]' and
/// 'blocks: […]' sections rather than raw `A -> B` arrows."
#[tokio::test]
async fn test_dep_list_renders_blocked_by_and_blocks_sections_no_arrows() {
    let (_temp, service) = setup_cas();

    let id_a = create_task(&service, "Task A").await;
    let id_b = create_task(&service, "Task B").await;
    let id_c = create_task(&service, "Task C").await;

    // A is blocked_by B (A -- blocks --> created as from=A to=B).
    service
        .cas_task_dep_add(Parameters(DependencyRequest {
            from_id: id_a.clone(),
            to_id: id_b.clone(),
            dep_type: "blocks".to_string(),
        }))
        .await
        .expect("dep_add A blocked_by B");

    // C is blocked_by A (A blocks C).
    service
        .cas_task_dep_add(Parameters(DependencyRequest {
            from_id: id_c.clone(),
            to_id: id_a.clone(),
            dep_type: "blocks".to_string(),
        }))
        .await
        .expect("dep_add C blocked_by A");

    let text = extract_text(
        service
            .cas_task_dep_list(Parameters(IdRequest { id: id_a.clone() }))
            .await
            .expect("dep_list should succeed"),
    );

    assert!(
        !text.contains("->"),
        "dep_list must not render bare arrows: {text}"
    );
    assert!(
        text.to_lowercase().contains("blocked by"),
        "dep_list must have a plain-worded 'blocked by' section: {text}"
    );
    assert!(
        text.to_lowercase().contains("blocks"),
        "dep_list must have a plain-worded 'blocks' section: {text}"
    );
    // A is blocked BY b (b must go first) and A blocks C (c waits on a).
    assert!(
        text.contains(&id_b),
        "blocked-by section must name the blocker (B): {text}"
    );
    assert!(
        text.contains(&id_c),
        "blocks section must name the task waiting on A (C): {text}"
    );
}

/// AC: "task show" must also render plain-worded sections, not arrows.
/// `cas_task_show` already avoided bare arrows before this task (it used
/// `BlockedBy:`/`Blocks:` labels); this task aligns the wording with
/// dep_list's new "Blocked by:" phrasing for consistency.
#[tokio::test]
async fn test_task_show_with_deps_has_no_arrows_and_names_both_directions() {
    let (_temp, service) = setup_cas();

    let id_a = create_task(&service, "Task A").await;
    let id_b = create_task(&service, "Task B").await;
    let id_c = create_task(&service, "Task C").await;

    service
        .cas_task_dep_add(Parameters(DependencyRequest {
            from_id: id_a.clone(),
            to_id: id_b.clone(),
            dep_type: "blocks".to_string(),
        }))
        .await
        .expect("dep_add A blocked_by B");
    service
        .cas_task_dep_add(Parameters(DependencyRequest {
            from_id: id_c.clone(),
            to_id: id_a.clone(),
            dep_type: "blocks".to_string(),
        }))
        .await
        .expect("dep_add C blocked_by A");

    let text = extract_text(
        service
            .cas_task_show(Parameters(TaskShowRequest {
                id: id_a.clone(),
                with_deps: true,
            }))
            .await
            .expect("task_show should succeed"),
    );

    assert!(
        !text.contains("->"),
        "task show must not render bare arrows: {text}"
    );
    assert!(
        text.to_lowercase().contains("blocked by"),
        "task show must name the blocked-by relationship in plain words: {text}"
    );
    assert!(
        text.contains(&id_b),
        "task show must name the blocker (B): {text}"
    );
    assert!(
        text.contains(&id_c),
        "task show must name the task blocked by A (C): {text}"
    );
}

/// cas-e500: reproduce the live lifecycle shape: a previously closed task is
/// reopened, receives a blocking dependency after reopen, and its assignee
/// explicitly attempts both normal start and manual claim while the blocker is
/// still in progress.
#[tokio::test]
async fn late_blocker_rearms_reopened_task_and_rejects_start_and_claim() {
    let (temp, service) = setup_cas();
    let task_store = open_task_store(&temp.path().join(".cas")).expect("task store");

    let blocker_id = create_task(&service, "In-progress blocker").await;
    let target_id = create_task(&service, "Reopened dependent").await;

    let mut blocker = task_store.get(&blocker_id).expect("blocker");
    blocker.status = TaskStatus::InProgress;
    task_store
        .update(&blocker)
        .expect("mark blocker in progress");

    // Preserve the relevant live-repro history without involving the close
    // verification gate: this task was closed, then reopened before dep_add.
    let mut target = task_store.get(&target_id).expect("target");
    target.status = TaskStatus::Closed;
    task_store.update(&target).expect("close target fixture");
    target.status = TaskStatus::Open;
    target.assignee = Some("test-agent".to_string());
    task_store
        .update(&target)
        .expect("reopen and assign target fixture");

    service
        .cas_task_dep_add(Parameters(DependencyRequest {
            from_id: target_id.clone(),
            to_id: blocker_id.clone(),
            dep_type: "blocks".to_string(),
        }))
        .await
        .expect("late blocking dependency should be added");

    let rearmed = task_store.get(&target_id).expect("rearmed target");
    assert_eq!(
        rearmed.status,
        TaskStatus::Blocked,
        "dep_add must re-arm an open/reopened task"
    );

    // Reproduce the historical stale projection exactly: even if an older DB
    // or concurrent writer leaves the task Open, the lifecycle boundary must
    // fail closed from the live dependency rows rather than trusting status.
    let mut stale_open = rearmed;
    stale_open.status = TaskStatus::Open;
    task_store
        .update(&stale_open)
        .expect("restore stale open status fixture");

    let start_error = service
        .cas_task_start(Parameters(IdRequest {
            id: target_id.clone(),
        }))
        .await
        .expect_err("start must reject an open blocking dependency");
    assert!(
        start_error.message.contains(&blocker_id),
        "start guidance must identify the actionable blocker: {}",
        start_error.message
    );

    let claim_error = service
        .cas_task_claim(Parameters(TaskClaimRequest {
            task_id: target_id,
            duration_secs: 600,
            reason: Some("manual recovery claim".to_string()),
        }))
        .await
        .expect_err("claim must reject an open blocking dependency");
    assert!(
        claim_error.message.contains(&blocker_id),
        "claim guidance must identify the actionable blocker: {}",
        claim_error.message
    );
}

/// The lifecycle gate must consume only `blocks` edges. In particular, a
/// parent-child epic link or a soft related link must never wedge task start.
#[tokio::test]
async fn non_blocking_dependency_types_do_not_rearm_or_reject_start() {
    let (temp, service) = setup_cas();
    let task_store = open_task_store(&temp.path().join(".cas")).expect("task store");

    for dep_type in ["related", "parent"] {
        let prerequisite_id = create_task(&service, &format!("{dep_type} target")).await;
        let dependent_id = create_task(&service, &format!("{dep_type} dependent")).await;

        service
            .cas_task_dep_add(Parameters(DependencyRequest {
                from_id: dependent_id.clone(),
                to_id: prerequisite_id,
                dep_type: dep_type.to_string(),
            }))
            .await
            .expect("non-blocking dependency should be added");

        assert_eq!(
            task_store.get(&dependent_id).expect("dependent").status,
            TaskStatus::Open,
            "{dep_type} must not project Blocked status"
        );
        service
            .cas_task_start(Parameters(IdRequest { id: dependent_id }))
            .await
            .unwrap_or_else(|error| panic!("{dep_type} must not reject start: {error}"));
    }
}

// ---------------------------------------------------------------------------
// cas-d45f (GH #98): `task action=update blocked_by=...` used to be silently
// dropped — no dependency, no error, and "No changes specified" when it was
// the only field. Ordering is correctness-critical, so a supplied blocked_by
// must now either create the Blocks edge or fail loudly.
// ---------------------------------------------------------------------------

fn task_req(value: serde_json::Value) -> cas_mcp::TaskRequest {
    serde_json::from_value(value).expect("TaskRequest should deserialize from test JSON")
}

async fn create_task_via_service(service: &cas::mcp::CasService, title: &str) -> String {
    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": title,
        }))))
        .await
        .expect("task create should succeed");
    extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string()
}

#[tokio::test]
async fn task_update_blocked_by_alone_creates_the_dependency() {
    let (temp, core) = setup_cas();
    let task_store = open_task_store(&temp.path().join(".cas")).expect("task store");
    let service = cas::mcp::CasService::new(core, None);

    let dependent_id = create_task_via_service(&service, "Dependent task").await;
    let blocker_id = create_task_via_service(&service, "Blocker task").await;

    let text = extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "update",
                "id": dependent_id,
                "blocked_by": blocker_id,
            }))))
            .await
            .expect("update with blocked_by should succeed"),
    );
    assert!(
        !text.contains("No changes specified"),
        "blocked_by must never be reported as no change: {text}"
    );
    assert!(
        text.contains("blocked_by"),
        "update confirmation must name the applied field: {text}"
    );

    let deps = extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "dep_list",
                "id": dependent_id,
            }))))
            .await
            .expect("dep_list should succeed"),
    );
    assert!(
        deps.contains(&blocker_id),
        "dep_list must show the blocker created by update: {deps}"
    );

    assert_eq!(
        task_store.get(&dependent_id).expect("dependent").status,
        TaskStatus::Blocked,
        "a late blocking edge must re-arm the open task, same as dep_add"
    );
}

#[tokio::test]
async fn task_update_blocked_by_applies_alongside_another_field() {
    let (temp, core) = setup_cas();
    let task_store = open_task_store(&temp.path().join(".cas")).expect("task store");
    let service = cas::mcp::CasService::new(core, None);

    let dependent_id = create_task_via_service(&service, "Dependent task").await;
    let blocker_id = create_task_via_service(&service, "Blocker task").await;

    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": dependent_id,
            "priority": 0,
            "blocked_by": blocker_id,
        }))))
        .await
        .expect("update with blocked_by + priority should succeed");

    let dependent = task_store.get(&dependent_id).expect("dependent");
    assert_eq!(dependent.priority.0, 0, "priority must still be applied");
    assert!(
        task_store
            .get_dependencies(&dependent_id)
            .expect("deps")
            .iter()
            .any(
                |dep| dep.to_id == blocker_id && dep.dep_type == cas::types::DependencyType::Blocks
            ),
        "blocked_by must not be dropped when combined with a recognised field"
    );
}

#[tokio::test]
async fn task_update_blocked_by_accepts_multiple_ids_and_is_idempotent() {
    let (temp, core) = setup_cas();
    let task_store = open_task_store(&temp.path().join(".cas")).expect("task store");
    let service = cas::mcp::CasService::new(core, None);

    let dependent_id = create_task_via_service(&service, "Dependent task").await;
    let first_blocker = create_task_via_service(&service, "First blocker").await;
    let second_blocker = create_task_via_service(&service, "Second blocker").await;

    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": dependent_id,
            "blocked_by": format!("{first_blocker}, {second_blocker}"),
        }))))
        .await
        .expect("update with two blockers should succeed");

    // Repeating the same call must not duplicate edges nor report no-change.
    let repeat = extract_text(
        service
            .task(Parameters(task_req(serde_json::json!({
                "action": "update",
                "id": dependent_id,
                "blocked_by": first_blocker,
            }))))
            .await
            .expect("repeat update should succeed"),
    );
    assert!(
        !repeat.contains("No changes specified"),
        "repeat blocked_by must report explicitly, not as no change: {repeat}"
    );

    let blocks: Vec<_> = task_store
        .get_dependencies(&dependent_id)
        .expect("deps")
        .into_iter()
        .filter(|dep| dep.dep_type == cas::types::DependencyType::Blocks)
        .collect();
    assert_eq!(blocks.len(), 2, "expected exactly two blockers: {blocks:?}");
}

#[tokio::test]
async fn task_update_blocked_by_rejects_unknown_self_and_empty_values() {
    let (_temp, core) = setup_cas();
    let service = cas::mcp::CasService::new(core, None);

    let dependent_id = create_task_via_service(&service, "Dependent task").await;

    let unknown = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": dependent_id,
            "blocked_by": "cas-nope",
        }))))
        .await
        .expect_err("unknown blocker must be rejected, not dropped");
    assert!(
        unknown.message.contains("cas-nope"),
        "error must name the unknown blocker: {}",
        unknown.message
    );

    let own = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": dependent_id,
            "blocked_by": dependent_id,
        }))))
        .await
        .expect_err("self-blocking must be rejected");
    assert!(
        own.message.contains("cannot block itself"),
        "self-block error must be explicit: {}",
        own.message
    );

    let empty = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": dependent_id,
            "blocked_by": " , ",
        }))))
        .await
        .expect_err("an empty blocked_by list must be rejected, not silently dropped");
    assert!(
        empty.message.contains("blocked_by"),
        "empty-list error must name the field: {}",
        empty.message
    );
}

#[tokio::test]
async fn task_update_blocked_by_rejects_the_parent_epic() {
    let (_temp, core) = setup_cas();
    let service = cas::mcp::CasService::new(core, None);

    let epic = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Epic parent",
            "task_type": "epic",
        }))))
        .await
        .expect("epic create should succeed");
    let epic_id = extract_task_id(&extract_text(epic))
        .expect("epic id")
        .to_string();

    let child_id = create_task_via_service(&service, "Child task").await;
    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": child_id,
            "epic": epic_id,
        }))))
        .await
        .expect("epic association should succeed");

    let error = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": child_id,
            "blocked_by": epic_id,
        }))))
        .await
        .expect_err("a task must not be blocked by its own epic");
    assert!(
        error.message.contains("child of and blocked by"),
        "epic/blocker conflict must be explicit: {}",
        error.message
    );
}
