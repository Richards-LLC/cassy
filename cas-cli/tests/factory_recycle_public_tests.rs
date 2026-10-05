//! Exercise the published MCP dispatch, including its destructive-field check.
use super::*;

#[tokio::test]
async fn cas_3e3a_public_recycle_and_clear_keep_clean_held_work_without_force() {
    for action in ["recycle_worker", "clear_context"] {
        let _guard = EnvGuard::set(&[("CAS_FACTORY_SESSION", "session-3e3a")]);
        let env = FactoryTestEnv::new();
        let path = init_pushed_worker_repo(&env, "checkpoint-worker");
        let worker = register_codex_worker_at(&env, "checkpoint-worker", "session-3e3a", &path);
        let handoff = env.cas_root.join("HANDOFF.md");
        std::fs::write(&handoff, "Continue from this durable checkpoint.\n").unwrap();
        let mut before = Vec::new();
        for (id, status) in [
            ("cas-3111", TaskStatus::InProgress),
            ("cas-3222", TaskStatus::AwaitingMerge),
            ("cas-3333", TaskStatus::Open),
        ] {
            let mut task = Task::new(id.into(), format!("checkpoint {id}"));
            task.status = status;
            task.assignee = Some("checkpoint-worker".into());
            task.notes = format!("HANDOFF: {}", handoff.display());
            env.task_store().add(&task).unwrap();
            before.push((
                id,
                serde_json::to_value(env.task_store().get(id).unwrap()).unwrap(),
            ));
        }
        assert!(
            env.agent_store()
                .try_claim("cas-3111", &worker, 600, Some("resume"))
                .unwrap()
                .is_success()
        );
        let lease = serde_json::to_value(env.agent_store().get_lease("cas-3111").unwrap()).unwrap();
        let agent = serde_json::to_value(env.agent_store().get(&worker).unwrap()).unwrap();
        let checkout = std::fs::read(path.join("README")).unwrap();

        let mut request = coord_req(action);
        request.target = Some("checkpoint-worker".into());
        let result = env
            .service
            .factory(Parameters(request))
            .await
            .unwrap_or_else(|error| panic!("{action} without force: {}", error.message));
        let text = get_text(&result);
        assert!(
            text.contains("Queued recycle") && text.contains("checkpoint-worker"),
            "{text}"
        );
        let entries = env.spawn_queue().peek(10).unwrap();
        assert_eq!(entries.len(), 1, "one same-worker lifecycle request");
        assert_eq!(entries[0].action, cas_store::SpawnAction::Recycle);
        assert_eq!(entries[0].worker_names, vec!["checkpoint-worker"]);
        let spec: cas_mux::WorkerSpec =
            serde_json::from_str(entries[0].worker_spec.as_deref().unwrap()).unwrap();
        assert_eq!(spec.name.as_deref(), Some("checkpoint-worker"));
        assert_eq!(spec.cli, SupervisorCli::Codex);
        for (id, value) in before {
            assert_eq!(
                serde_json::to_value(env.task_store().get(id).unwrap()).unwrap(),
                value,
                "{action} keeps status, assignment and checkpoint notes for {id}"
            );
        }
        assert_eq!(
            serde_json::to_value(env.agent_store().get_lease("cas-3111").unwrap()).unwrap(),
            lease
        );
        assert_eq!(
            serde_json::to_value(env.agent_store().get(&worker).unwrap()).unwrap(),
            agent
        );
        assert_eq!(std::fs::read(path.join("README")).unwrap(), checkout);
        assert!(handoff.is_file(), "the durable handoff survives");
        assert!(
            env.prompt_queue().peek_all(10).unwrap().is_empty(),
            "Codex reset uses recycle, not an unsupported /clear"
        );
    }
}

#[tokio::test]
async fn cas_3e3a_public_refusals_offer_a_supported_remedy_and_preserve_work() {
    let _guard = EnvGuard::set(&[("CAS_FACTORY_SESSION", "session-3e3a-unsafe")]);
    let env = FactoryTestEnv::new();
    let path = init_pushed_worker_repo(&env, "unsafe-worker");
    let worker = register_codex_worker_at(&env, "unsafe-worker", "session-3e3a-unsafe", &path);
    let mut task = Task::new("cas-3444".into(), "Keep assigned work".into());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("unsafe-worker".into());
    env.task_store().add(&task).unwrap();
    let task_before = serde_json::to_value(env.task_store().get(&task.id).unwrap()).unwrap();
    let worker_before = serde_json::to_value(env.agent_store().get(&worker).unwrap()).unwrap();
    let dirty = path.join("dirty.txt");
    std::fs::write(&dirty, "Do not destroy this work.\n").unwrap();

    for action in ["recycle_worker", "clear_context"] {
        let mut request = coord_req(action);
        request.target = Some("unsafe-worker".into());
        let error = env.service.factory(Parameters(request)).await.unwrap_err();
        assert!(error.message.starts_with(action), "{}", error.message);
        assert!(
            error.message.contains("commit and push"),
            "{}",
            error.message
        );
        assert!(
            !error.message.contains("force=true"),
            "no unusable force remedy: {}",
            error.message
        );
    }
    // The public surface rejects force before touching state. It must never
    // first tell the caller that this unsupported flag is required.
    let mut request = coord_req("recycle_worker");
    request.target = Some("unsafe-worker".into());
    request.force = Some(true);
    let error = env.service.factory(Parameters(request)).await.unwrap_err();
    assert!(
        error.message.contains("Unsupported parameter(s)") && error.message.contains("force"),
        "{}",
        error.message
    );
    assert!(
        !error.message.contains("requires force=true"),
        "{}",
        error.message
    );
    assert!(env.spawn_queue().peek(10).unwrap().is_empty());
    assert!(env.prompt_queue().peek_all(10).unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(dirty).unwrap(),
        "Do not destroy this work.\n"
    );
    assert_eq!(
        serde_json::to_value(env.task_store().get(&task.id).unwrap()).unwrap(),
        task_before
    );
    assert_eq!(
        serde_json::to_value(env.agent_store().get(&worker).unwrap()).unwrap(),
        worker_before
    );
}
