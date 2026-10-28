use super::*;
use crate::coordination::binding_intake::ChannelDisposition;
use std::io::{Read, Write};
const PROJECT: &str = "github.com/example/project";

fn project(root: &Path, team: &str) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join("config.toml"),
        format!("[project]\ncanonical_id = {PROJECT:?}\n"),
    )
    .unwrap();
    crate::cloud::CloudConfig {
        team_id: Some(team.into()),
        ..Default::default()
    }
    .save_to_cas_dir(root)
    .unwrap();
}
fn scope() -> IntakeScope {
    IntakeScope {
        team_id: Uuid::new_v4().to_string(),
        canonical_project_id: PROJECT.into(),
        channel_id: "C0123456789".into(),
    }
}
fn runtime(root: &Path, capability: bool) -> std::thread::JoinHandle<()> {
    runtime_rounds(root, capability, 1)
}
fn runtime_rounds(root: &Path, capability: bool, rounds: usize) -> std::thread::JoinHandle<()> {
    use crate::ui::factory::daemon::runtime::violet_intake::*;
    use crate::ui::factory::*;
    let name = "bind-fx";
    let mut metadata = create_metadata(
        name,
        std::process::id(),
        "supervisor",
        &[],
        None,
        None,
        None,
    );
    metadata.project_dir = Some(root.parent().unwrap().to_str().unwrap().into());
    SessionManager::new().save_metadata(&metadata).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(gui_socket_path(name)).unwrap();
    let root = root.to_path_buf();
    std::thread::spawn(move || {
        for _ in 0..rounds {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let welcome = serde_json::json!({ "Welcome": {"session_name":name,"state":{"focused_pane":null,"panes":[],"epic_id":null,"epic_title":null,"cols":80,"rows":24},"scrollback":null,"protocol_version":3,"capabilities": if capability { vec!["binding_intake_control_v1"] } else { vec![] } }});
            stream
                .write_all(&encode_message(&welcome).unwrap())
                .unwrap();
            if !capability {
                // Like a real runtime, stay connected until the client hangs up
                // (here and after the drain reply below). On macOS, setsockopt on
                // a peer-closed socket fails EINVAL before buffered frames are
                // read (cas-cbcc).
                let _ = stream.read(&mut [0; 1]);
                return;
            }
            let mut header = [0; 4];
            stream.read_exact(&mut header).unwrap();
            let mut bytes = vec![0; u32::from_be_bytes(header) as usize];
            stream.read_exact(&mut bytes).unwrap();
            let ClientMessage::BindingIntakePause { request } =
                serde_json::from_slice(&bytes).unwrap()
            else {
                panic!("local pause required")
            };
            let state = IntakeJournal::new(&root).read().unwrap();
            let pause = state
                .pauses
                .iter()
                .find(|pause| pause.operation_id == request.operation_id)
                .unwrap()
                .clone();
            // A busy factory broadcasts pane output to every GUI client, including
            // this control probe, and a newer daemon may send frames this build
            // cannot parse. Neither may exhaust the drain before its reply.
            for index in 0..300 {
                let frame = if index % 50 == 0 {
                    serde_json::json!({ "FutureBroadcast": { "index": index } })
                } else {
                    serde_json::json!({ "OperatorNoticeResolved": { "notification_id": index } })
                };
                stream.write_all(&encode_message(&frame).unwrap()).unwrap();
            }
            let reply = IntakeReply {
                nonce: request.nonce,
                state: Some(state),
                acknowledgement: Some(DrainAcknowledgement {
                    nonce: request.nonce,
                    pause,
                    runtime: RuntimeIdentity::current(&root, name).unwrap(),
                }),
                error: None,
            };
            stream
                .write_all(&encode_message(&DaemonMessage::BindingIntake { reply }).unwrap())
                .unwrap();
            let _ = stream.read(&mut [0; 1]);
        }
    })
}

/// A failed retry must not restart the presence that Cloud is waiting to expire.
#[test]
fn failed_retry_preserves_legacy_quiet_window_cas_40c97() {
    use crate::ui::factory::daemon::runtime::violet_activity::VioletWakeRuntime;
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let root = crate::store::init_cas_dir(&env.home().join("project")).unwrap();
    let scope = scope();
    project(&root, &scope.team_id);
    env.set("CAS_ROOT", &root);
    env.set("CAS_USER_CLOUD_JSON", env.home().join("cloud.json"));
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", server.server_addr());
    crate::cloud::CloudConfig {
        endpoint: endpoint.clone(),
        token: Some("ordinary-fixture-user".into()),
        ..Default::default()
    }
    .save_user()
    .unwrap();
    let gui = runtime_rounds(&root, true, 4);
    let runtime = VioletWakeRuntime::start(&root, "bind-fx", "supervisor").unwrap();
    let target = scope.clone();
    let remote = std::thread::spawn(move || {
        let receive = || {
            server
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .expect("HTTP request")
        };
        let claim = receive();
        assert_eq!(
            claim.url(),
            "/api/violet/activity/claim",
            "ungated tick must actually claim"
        );
        claim
            .respond(tiny_http::Response::from_string(
                r#"{"events":[],"denied":[]}"#,
            ))
            .unwrap();
        let mut claimed_during_hold = false;
        for round in 0..4 {
            let mut get = receive();
            // Respond even on the broken path so the real tick and subsequent
            // retry finish; assert the unwanted claim after joining both peers.
            if round == 3 && get.url() == "/api/violet/activity/claim" {
                claimed_during_hold = true;
                get.respond(tiny_http::Response::from_string(
                    r#"{"events":[],"denied":[]}"#,
                ))
                .unwrap();
                get = receive();
            }
            assert_eq!(get.method().as_str(), "GET");
            assert_eq!(get.url(), format!("/api/teams/{}/projects", target.team_id));
            if round == 0 || round == 2 {
                get.respond(
                    tiny_http::Response::from_string("temporarily unavailable")
                        .with_status_code(503),
                )
                .unwrap();
                continue;
            }
            get.respond(tiny_http::Response::from_string(
                serde_json::json!({"projects":[{"id":"project-id","canonical_id":PROJECT,"name":"fixture","contributor_count":0,"memory_count":0}]}).to_string(),
            )).unwrap();
            let post = receive();
            assert_eq!(post.method().as_str(), "POST");
            assert_eq!(post.url(), RECONCILE_PATH);
            let mut body: serde_json::Value = serde_json::from_str(if round == 1 {
                include_str!(
                    "../../../tests/fixtures/binding_reconciliation/v1/legacy-intake-active.json"
                )
            } else {
                include_str!("../../../tests/fixtures/binding_reconciliation/v1/completed.json")
            })
            .unwrap();
            body["scope"]["team_id"] = target.team_id.clone().into();
            body["binding"]["team_id"] = target.team_id.clone().into();
            post.respond(
                tiny_http::Response::from_string(body.to_string())
                    .with_status_code(if round == 1 { 409 } else { 200 })
                    .with_header(
                        tiny_http::Header::from_bytes("Cache-Control", "no-store").unwrap(),
                    ),
            )
            .unwrap();
        }
        assert!(server.try_recv().unwrap().is_none());
        claimed_during_hold
    });
    assert!(runtime.tick_blocking().0.errors.is_empty());
    let client = OrdinaryBindingClient::new(&endpoint, "ordinary-fixture-user").unwrap();
    // A crash between pause and submit leaves no refusal-backed quiet window.
    let journal = IntakeJournal::new(&root);
    journal.pause(scope.clone(), Uuid::new_v4(), 0).unwrap();
    let orphan_retry = client.repair(&root, &scope).unwrap();
    assert!(matches!(orphan_retry.outcome, RepairOutcome::Blocked));
    assert!(orphan_retry.response.is_none(), "orphan retry must not submit");
    let orphan_released = journal.legacy_allowed().unwrap();
    let refused = client.repair(&root, &scope).unwrap();
    assert_eq!(
        refused.response.unwrap().code,
        ReconciliationCode::LegacyIntakeActive
    );
    assert!(!IntakeJournal::new(&root).legacy_allowed().unwrap());
    let failed = client.repair(&root, &scope).unwrap();
    assert!(
        matches!(failed.outcome, RepairOutcome::Blocked),
        "{failed:?}"
    );
    assert!(failed.response.is_none(), "preflight must not submit");
    assert!(runtime.tick_blocking().0.errors.is_empty());
    let completed = client.repair(&root, &scope).unwrap();
    gui.join().unwrap();
    let claimed_during_hold = remote.join().unwrap();
    assert!(
        matches!(completed.outcome, RepairOutcome::Managed),
        "{completed:?}"
    );
    assert!(
        !claimed_during_hold,
        "failed preflight restarted v1 presence during the quiet window"
    );
    assert!(orphan_released, "preflight failure must release an orphaned pause");
}

#[test]
fn cas_9021_ordinary_http_invocation_is_bound_to_local_drain_and_server_receipt() {
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let scope = scope();
    let root = env.home().join("project/.cas");
    project(&root, &scope.team_id);
    let gui = runtime(&root, true);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", server.server_addr());
    let target = scope.clone();
    let remote = std::thread::spawn(move || {
        let get = server
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .expect("ordinary project verification");
        assert_eq!(get.method().as_str(), "GET");
        assert_eq!(get.url(), format!("/api/teams/{}/projects", target.team_id));
        get.respond(tiny_http::Response::from_string(
            serde_json::json!({"projects":[{"id":"project-id","canonical_id":PROJECT,"name":"fixture","contributor_count":0,"memory_count":0}]})
                .to_string(),
        ))
        .unwrap();
        let Some(mut post) = server.recv_timeout(Duration::from_secs(5)).unwrap() else {
            return false;
        };
        assert_eq!(post.method().as_str(), "POST");
        assert_eq!(post.url(), RECONCILE_PATH);
        assert_eq!(
            post.headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .unwrap()
                .value
                .as_str(),
            "Bearer ordinary-fixture-user"
        );
        let mut bytes = Vec::new();
        post.as_reader().read_to_end(&mut bytes).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::json!({"schema":super::super::binding_reconciliation::SCHEMA,"team_id":target.team_id,"project_id":PROJECT,"channel_id":target.channel_id})
        );
        let mut body: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/binding_reconciliation/v1/completed.json"
        ))
        .unwrap();
        body["scope"]["team_id"] = target.team_id.clone().into();
        body["binding"]["team_id"] = target.team_id.into();
        post.respond(
            tiny_http::Response::from_string(body.to_string())
                .with_header(tiny_http::Header::from_bytes("Cache-Control", "no-store").unwrap()),
        )
        .unwrap();
        true
    });
    let inventory = RuntimeSnapshot::capture(true);
    env.set("CAS_AGENT_ROLE", "supervisor");
    env.set("CAS_FACTORY_SESSION", "bind-fx");
    env.set("CAS_AGENT_NAME", "supervisor");
    crate::ui::factory::record_supervisor_mcp_call();
    for key in ["CAS_AGENT_ROLE", "CAS_FACTORY_SESSION", "CAS_AGENT_NAME"] {
        env.remove(key);
    }
    let mut metadata: crate::ui::factory::SessionMetadata = serde_json::from_slice(
        &std::fs::read(crate::ui::factory::metadata_path("bind-fx")).unwrap(),
    )
    .unwrap();
    assert!(metadata.last_supervisor_mcp_call_at.is_some());
    // The daemon's full metadata writer also runs while update is working.
    for index in 0..4 {
        metadata.epic_id = Some(format!("fixture-epic-{index}"));
        metadata.held_workers = vec![format!("worker-{index}")];
        crate::ui::factory::SessionManager::new()
            .save_metadata(&metadata)
            .unwrap();
    }
    let report = OrdinaryBindingClient::new(&endpoint, "ordinary-fixture-user")
        .unwrap()
        .repair_with_inventory(&root, &scope, &inventory)
        .unwrap();
    assert!(
        matches!(report.outcome, RepairOutcome::Managed),
        "{report:?}"
    );
    gui.join().unwrap();
    let posted = remote.join().unwrap();
    assert!(
        posted,
        "reconciliation must reach the ordinary HTTP path after actual local drain"
    );
    let state = IntakeJournal::new(&root).read().unwrap();
    assert_eq!(state.pauses[0].operation_id, report.operation_id);
    assert!(
        matches!(&state.policies[0].disposition,ChannelDisposition::Managed { operation_id, completion } if *operation_id == report.operation_id && completion.latest_attempt.as_ref().unwrap().id == report.activation_id.as_ref().unwrap().as_str())
    );
}

#[test]
fn dead_runtime_gui_endpoints_are_scoped_cas_0700() {
    use crate::ui::factory::{SessionManager, create_metadata, gui_socket_path};
    use std::os::unix::net::UnixListener;

    let env = crate::test_support::TestEnvGuard::temp_home();
    let root = crate::store::init_cas_dir(&env.home().join("project")).unwrap();
    let other = crate::store::init_cas_dir(&env.home().join("other")).unwrap();
    // A reaped child supplies a real dead PID without assuming a host PID range.
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead_pid = child.id();
    assert!(child.wait().unwrap().success());
    let name = "dead-gui-0700";
    let socket = gui_socket_path(name);
    let mut failures = Vec::new();
    let unresolved = env.home().join("deleted/.cas");
    // The other-project held row proves filtering precedes endpoint refusal.
    // One unresolved-project row covers missing, inactive and held sockets.
    let cases: [(bool, &std::path::Path, &[Option<bool>]); 7] = [
        (true, &other, &[Some(false)]),
        (true, &other, &[Some(true)]),
        (true, &root, &[Some(false)]),
        (true, &root, &[Some(true)]),
        (false, &other, &[Some(false)]),
        (false, &other, &[Some(true)]),
        (true, &unresolved, &[None, Some(false), Some(true)]),
    ];
    for (recorded, project, endpoints) in cases {
        let mut metadata = create_metadata(
            name,
            dead_pid,
            "supervisor",
            &[],
            None,
            project.parent().unwrap().to_str(),
            None,
        );
        if recorded {
            SessionManager::new().save_metadata(&metadata).unwrap();
        } else {
            let _ = std::fs::remove_file(crate::ui::factory::metadata_path(name));
        }
        for endpoint in endpoints {
            let held = *endpoint == Some(true);
            let listener = endpoint.and_then(|held| {
                let listener = UnixListener::bind(&socket).unwrap();
                held.then_some(listener)
            });
            let result = RuntimeInventory::discover(&root);
            match result {
                Err(error) if recorded && held && project != other => {
                    if !error.contains(name) || !error.contains(&format!("cas kill -- {name}")) {
                        failures.push(format!("held endpoint lacks named remedy: {error}"));
                    }
                }
                Err(error) if !recorded && held => {
                    assert!(error.contains("unattributed"), "{error}");
                }
                Err(error) => failures.push(format!(
                    "recorded={recorded}, project={}, endpoint={endpoint:?}: {error}",
                    project.display()
                )),
                Ok(_) if held && (!recorded || project != other) => {
                    failures.push("held endpoint was allowed".into())
                }
                Ok(_) => {}
            }
            assert_eq!(
                socket.exists(),
                endpoint.is_some(),
                "inventory must not change the socket path it probes"
            );
            drop(listener);
            if endpoint.is_some() {
                std::fs::remove_file(&socket).unwrap();
            }
        }
        if project == unresolved {
            metadata.daemon_pid = std::process::id();
            SessionManager::new().save_metadata(&metadata).unwrap();
            let error = RuntimeInventory::discover(&root)
                .err()
                .expect("live unresolved session must refuse");
            assert!(
                error.contains(name) && error.contains(&format!("cas kill -- {name}")),
                "{error}"
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn cas_9021_old_or_unknown_actual_runtime_preflight_sends_zero_post() {
    let env = crate::test_support::TestEnvGuard::temp_home();
    let scope = scope();
    let root = env.home().join("project/.cas");
    project(&root, &scope.team_id);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client = OrdinaryBindingClient::new(
        &format!("http://{}", server.server_addr()),
        "ordinary-fixture-user",
    )
    .unwrap();
    let gui = runtime(&root, false);
    let old = client.repair(&root, &scope).unwrap();
    gui.join().unwrap();
    assert!(matches!(old.outcome, RepairOutcome::Blocked));
    let old_error = old.error.unwrap();
    assert!(old_error.contains("lacks"), "{old_error}");
    let metadata_path = crate::ui::factory::metadata_path("bind-fx");
    let mut metadata: crate::ui::factory::SessionMetadata =
        serde_json::from_slice(&std::fs::read(&metadata_path).unwrap()).unwrap();
    metadata.daemon_build_hash = None;
    crate::ui::factory::SessionManager::new()
        .save_metadata(&metadata)
        .unwrap();
    let unknown = client.repair(&root, &scope).unwrap();
    assert!(matches!(unknown.outcome, RepairOutcome::Blocked));
    assert!(unknown.error.unwrap().contains("build"));
    metadata.daemon_build_hash = Some(option_env!("CAS_GIT_HASH").unwrap_or("unknown").into());
    metadata.daemon_pid_starttime = metadata.daemon_pid_starttime.map(|value| value + 1);
    crate::ui::factory::SessionManager::new()
        .save_metadata(&metadata)
        .unwrap();
    // The earlier runtime thread exited. Keep a real listener held here:
    // a stale PID with a refused endpoint is now intentionally skipped.
    let socket = crate::ui::factory::gui_socket_path("bind-fx");
    std::fs::remove_file(&socket).unwrap();
    let _held = std::os::unix::net::UnixListener::bind(socket).unwrap();
    let stale = client.repair(&root, &scope).unwrap();
    assert!(matches!(stale.outcome, RepairOutcome::Blocked));
    assert!(stale.error.unwrap().contains("cas kill -- bind-fx"));
    std::fs::write(&metadata_path, b"{").unwrap();
    let corrupt = client.repair(&root, &scope).unwrap();
    assert!(matches!(corrupt.outcome, RepairOutcome::Blocked));
    assert!(
        server.try_recv().unwrap().is_none(),
        "no GET/POST may escape unsafe actual-runtime preflight"
    );
    let state = IntakeJournal::new(&root).read().unwrap();
    assert_eq!(state.pauses.len(), 1, "each rerun re-arms the same pause");
    assert!(
        state.legacy_claims_allowed(),
        "a pause that never reached Cloud must not keep project claims shut"
    );
}

/// A transport-uncertain attempt pauses legacy claims; no eligible readers
/// retains pending work until a later definite refusal permits legacy intake.
#[test]
fn cas_9021_uncertain_attempt_is_settled_by_a_later_definite_refusal() {
    let env = crate::test_support::TestEnvGuard::temp_home();
    let scope = scope();
    let root = env.home().join("project/.cas");
    project(&root, &scope.team_id);
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let client = OrdinaryBindingClient::new(
        &format!("http://{}", server.server_addr()),
        "ordinary-fixture-user",
    )
    .unwrap();
    let mut refusal: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/binding_reconciliation/v1/legacy-intake-active.json"
    ))
    .unwrap();
    refusal["scope"]["team_id"] = scope.team_id.clone().into();
    refusal["binding"]["team_id"] = scope.team_id.clone().into();
    for (key, value) in [("code", "no_eligible_readers"), ("reason", "no_eligible_readers"), ("next_action", "retry_later")] {
        refusal[key] = value.into();
    }
    refusal["guidance"] = "Pending activity would reach no supervisor of this project. Start one, then retry; nothing was settled.".into();
    refusal["latest_attempt"]["quiescence"]["blocking_v1_leases"] = 0.into();
    refusal["latest_attempt"]["quiescence"]["blocking_v1_consumers"] = serde_json::json!([]);
    for key in ["blockers", "retry_after_secs"] {
        refusal[key] = serde_json::Value::Null;
    }
    let team = scope.team_id.clone();
    let remote = std::thread::spawn(move || {
        for (status, body, no_store) in [(502, "Bad Gateway".to_string(), false), (409, refusal.to_string(), true)] {
            let get = server.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
            assert_eq!(get.url(), format!("/api/teams/{team}/projects"));
            get.respond(tiny_http::Response::from_string(
                serde_json::json!({"projects":[{"id":"project-id","canonical_id":PROJECT,"name":"fixture","contributor_count":0,"memory_count":0}]}).to_string(),
            ))
            .unwrap();
            let post = server.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
            assert_eq!(post.url(), RECONCILE_PATH);
            let mut response = tiny_http::Response::from_string(body).with_status_code(status);
            if no_store {
                response = response.with_header(tiny_http::Header::from_bytes("Cache-Control", "no-store").unwrap());
            }
            post.respond(response).unwrap();
        }
    });
    let uncertain = client.repair(&root, &scope).unwrap();
    assert!(matches!(uncertain.outcome, RepairOutcome::Unknown), "{uncertain:?}");
    let journal = IntakeJournal::new(&root);
    assert_eq!(journal.read().unwrap().policies[0].disposition, ChannelDisposition::Unknown);
    assert!(!journal.read().unwrap().legacy_claims_allowed());
    let refused = client.repair(&root, &scope).unwrap();
    remote.join().unwrap();
    assert!(matches!(refused.outcome, RepairOutcome::Blocked), "{refused:?}");
    assert_eq!(refused.response.as_ref().unwrap().code, ReconciliationCode::NoEligibleReaders);
    assert_eq!(refused.response.as_ref().unwrap().next_action, super::super::binding_reconciliation::NextAction::RetryLater);
    // pin: Cloud's retryable refusal guidance is the operator-visible contract.
    assert_eq!(refused.error.as_deref(), Some("Pending activity would reach no supervisor of this project. Start one, then retry; nothing was settled."));
    assert!(matches!(journal.read().unwrap().policies[0].disposition, ChannelDisposition::Paused { .. }));
    assert!(!journal.read().unwrap().legacy_claims_allowed(), "pending rows cannot be reopened to legacy claims");
    let failed_retry = client.repair(&root, &scope).unwrap();
    assert!(matches!(failed_retry.outcome, RepairOutcome::Blocked));
    assert!(matches!(journal.read().unwrap().policies[0].disposition, ChannelDisposition::Paused { .. }));
    assert!(!journal.read().unwrap().legacy_claims_allowed(), "failed retry must retain the pending-row pause");
}
