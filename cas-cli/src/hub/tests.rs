use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tower::ServiceExt;

use super::*;
use crate::ui::factory::{
    ClientMessage, DaemonMessage, PROTOCOL_VERSION, PaneInfo, PaneKind, SessionState,
    daemon_capabilities,
};

#[test]
fn h1_origin_01_pre_auth_exposes_health_only_and_rejects_mutations() {
    let auth = PreAuthAuthorizer;

    assert!(auth.authorize(&HubRequest::health()).is_allowed());
    assert!(auth.authorize(&HubRequest::sessions(None)).is_denied());
    assert!(
        auth.authorize(&HubRequest::sessions(Some("https://evil.example")))
            .is_denied()
    );
    assert!(
        auth.authorize(&HubRequest::mutation(Some("http://127.0.0.1:4173")))
            .is_denied()
    );

    let health = HealthResponse::ready();
    let json = serde_json::to_value(health).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["ready"], true);
    assert_eq!(json.as_object().unwrap().len(), 2);
}

#[test]
fn h1_tls_02_plaintext_control_is_loopback_only() {
    let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4173);
    let lan = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 10, 22)), 4173);

    assert!(validate_control_bind(loopback, TransportSecurity::Plaintext).is_ok());
    assert!(validate_control_bind(lan, TransportSecurity::Plaintext).is_err());
    assert!(validate_control_bind(lan, TransportSecurity::Tls13).is_ok());
    assert!(
        validate_control_bind(lan, TransportSecurity::TrustedLoopbackTlsProxy).is_err(),
        "a loopback TLS proxy never authorizes a non-loopback hub listener"
    );
}

#[tokio::test]
async fn h1_mux_03_two_viewers_three_panes_share_one_upstream() {
    let mux = SessionMultiplexer::new(8);
    let mut first = mux.subscribe("factory-a", ["supervisor", "worker-1"]).await;
    let mut second = mux.subscribe("factory-a", ["worker-1", "worker-2"]).await;

    assert_eq!(mux.upstream_start_count("factory-a").await, 1);

    let worker_one = proxy_frame(DaemonMessage::Output {
        pane_id: "worker-1".into(),
        data: b"same bytes".to_vec(),
    });
    let worker_two = proxy_frame(DaemonMessage::Output {
        pane_id: "worker-2".into(),
        data: b"other pane".to_vec(),
    });
    mux.publish("factory-a", worker_one.clone()).await.unwrap();
    mux.publish("factory-a", worker_two.clone()).await.unwrap();

    assert_eq!(first.recv().await.unwrap().bytes, worker_one.bytes);
    assert_eq!(second.recv().await.unwrap().bytes, worker_one.bytes);
    assert_eq!(second.recv().await.unwrap().bytes, worker_two.bytes);
    assert!(
        first.try_recv().is_err(),
        "pane filtering happens in the hub"
    );
}

#[tokio::test]
async fn h1_bp_04_slow_viewer_lags_without_new_upstream_or_harming_fast_viewer() {
    let mux = SessionMultiplexer::new(2);
    let mut slow = mux.subscribe("factory-a", ["worker-1"]).await;
    let mut fast = mux.subscribe("factory-a", ["worker-1"]).await;

    for byte in 0..8 {
        mux.publish(
            "factory-a",
            proxy_frame(DaemonMessage::Output {
                pane_id: "worker-1".into(),
                data: vec![byte],
            }),
        )
        .await
        .unwrap();
        let _ = fast.recv().await.unwrap();
    }

    assert!(matches!(
        slow.recv().await,
        Err(ViewerRecvError::Lagged { .. })
    ));
    assert_eq!(mux.upstream_start_count("factory-a").await, 1);
    assert!(fast.try_recv().is_err());
}

#[test]
fn h1_death_05_reports_clean_signal_sigill_and_unknown_without_invention() {
    assert_eq!(
        diagnose_daemon_death(Some(ProcessExit::Code(0)), true).cause,
        DaemonDeathCause::CleanExit { code: 0 }
    );
    assert_eq!(
        diagnose_daemon_death(Some(ProcessExit::Signal(4)), true).cause,
        DaemonDeathCause::Signal {
            signal: 4,
            name: Some("SIGILL".into()),
            core_dumped: Some(true),
        }
    );
    assert_eq!(
        diagnose_daemon_death(Some(ProcessExit::Signal(15)), false).cause,
        DaemonDeathCause::Signal {
            signal: 15,
            name: Some("SIGTERM".into()),
            core_dumped: Some(false),
        }
    );
    assert_eq!(
        diagnose_daemon_death(None, false).cause,
        DaemonDeathCause::Unknown
    );
}

#[tokio::test]
async fn h1_zero_06_read_paths_never_write_pty_or_create_logical_sessions() {
    let source = RecordingReadModel::with_sessions(vec![fixture_session("factory-a")]);
    let catalog = SessionCatalog::new(source.clone());

    assert_eq!(catalog.list().await.unwrap().len(), 1);
    assert_eq!(catalog.list().await.unwrap().len(), 1);
    assert_eq!(source.read_count(), 2);
    assert_eq!(source.pty_write_count(), 0);
    assert_eq!(source.model_call_count(), 0);
    assert_eq!(source.logical_session_create_count(), 0);
}

#[test]
fn h1_machine_identity_is_stable_on_disk() {
    let temp = tempfile::tempdir().unwrap();
    let state_dir = temp.path().join("hub");
    let store = MachineIdentityStore::new(&state_dir);

    let first = store.load_or_create().unwrap();
    let second = store.load_or_create().unwrap();

    assert_eq!(first, second);
    assert!(!first.id.is_empty());
    assert_eq!(
        std::fs::read_to_string(state_dir.join("machine-id")).unwrap(),
        first.id
    );
}

#[derive(Clone)]
struct ExactOriginReadAuthorizer(&'static str);

impl HubAuthorizer for ExactOriginReadAuthorizer {
    fn authorize(&self, request: &HubRequest) -> AuthorizationDecision {
        if request.action == HubAction::Health
            || (request.action != HubAction::Mutation && request.origin.as_deref() == Some(self.0))
        {
            AuthorizationDecision::Allow
        } else {
            AuthorizationDecision::Deny
        }
    }
}

#[tokio::test]
async fn h1_http_surface_is_real_and_origin_authorized() {
    let source = RecordingReadModel::with_sessions(vec![fixture_session("factory-a")]);
    let events = MachineEventBus::new(16);
    let state = HubState::new(
        SessionCatalog::new(source.clone()),
        Arc::new(ExactOriginReadAuthorizer("http://127.0.0.1:4173")),
        MachineIdentity {
            id: "machine-test".into(),
        },
        DaemonConnector::new(SessionMultiplexer::new(8), events.clone()),
        events,
    );
    let app = router(state);

    let health = app
        .clone()
        .oneshot(Request::get("/v1/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    let health: serde_json::Value =
        serde_json::from_slice(&to_bytes(health.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(
        health,
        serde_json::json!({"schema_version": 1, "ready": true})
    );

    let denied = app
        .clone()
        .oneshot(Request::get("/v1/sessions").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        source.read_count(),
        0,
        "denied reads never touch session state"
    );

    let allowed = app
        .oneshot(
            Request::get("/v1/sessions")
                .header("origin", "http://127.0.0.1:4173")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(allowed.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["sessions"][0]["name"], "factory-a");
    assert_eq!(body["sessions"][0]["liveness"], "live");
    assert_eq!(source.read_count(), 1);
}

#[tokio::test]
async fn h1_real_daemon_connector_preserves_bytes_and_one_upstream_per_session() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let connections = Arc::new(AtomicUsize::new(0));
    let release_output = Arc::new(Notify::new());

    let welcome = DaemonMessage::Welcome {
        session_name: "factory-a".into(),
        state: SessionState {
            focused_pane: Some("worker-1".into()),
            panes: vec![PaneInfo {
                id: "worker-1".into(),
                kind: PaneKind::Worker,
                focused: true,
                title: "Worker 1".into(),
                exited: false,
            }],
            epic_id: Some("cas-epic".into()),
            epic_title: Some("Commander".into()),
            cols: 120,
            rows: 40,
        },
        scrollback: Some(HashMap::from([(
            "worker-1".into(),
            vec![b"scrollback\n".to_vec()],
        )])),
        protocol_version: PROTOCOL_VERSION,
        capabilities: daemon_capabilities(),
    };
    let output = DaemonMessage::Output {
        pane_id: "worker-1".into(),
        data: b"\x1b[32mlive bytes\x1b[0m".to_vec(),
    };
    let welcome_bytes = serde_json::to_vec(&welcome).unwrap();
    let output_bytes = serde_json::to_vec(&output).unwrap();

    let daemon_connections = connections.clone();
    let daemon_release = release_output.clone();
    let daemon_welcome = welcome_bytes.clone();
    let daemon_output = output_bytes.clone();
    let daemon = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            daemon_connections.fetch_add(1, Ordering::SeqCst);
            let release = daemon_release.clone();
            let welcome = daemon_welcome.clone();
            let output = daemon_output.clone();
            tokio::spawn(async move {
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                ws.send(WsMessage::Binary(welcome)).await.unwrap();
                release.notified().await;
                ws.send(WsMessage::Binary(output)).await.unwrap();
                futures_util::future::pending::<()>().await;
            });
        }
    });

    let events = MachineEventBus::new(16);
    let connector = DaemonConnector::new(SessionMultiplexer::new(8), events);
    let mut first = connector
        .attach("factory-a", port, ["worker-1"])
        .await
        .unwrap();
    assert_eq!(first.recv().await.unwrap().bytes, welcome_bytes);

    let mut second = connector
        .attach("factory-a", port, ["worker-1"])
        .await
        .unwrap();
    assert_eq!(
        second.recv().await.unwrap().bytes,
        welcome_bytes,
        "late viewers rehydrate from the byte-identical canonical Welcome"
    );

    release_output.notify_waiters();
    assert_eq!(first.recv().await.unwrap().bytes, output_bytes);
    assert_eq!(second.recv().await.unwrap().bytes, output_bytes);
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    assert_eq!(connections.load(Ordering::SeqCst), 1);
    assert_eq!(connector.upstream_connection_count("factory-a").await, 1);
    daemon.abort();
}

#[tokio::test]
async fn h1_aggregate_events_cover_session_and_pane_lifecycle() {
    let events = MachineEventBus::new(16);
    let mut receiver = events.subscribe();

    events.reconcile_sessions(["factory-a"]).await;
    let added = receiver.recv().await.unwrap();
    assert_eq!(added.kind, MachineEventKind::SessionAdded);
    assert_eq!(added.session.as_deref(), Some("factory-a"));

    events.observe_daemon(
        "factory-a",
        &DaemonMessage::PaneAdded {
            pane: PaneInfo {
                id: "worker-1".into(),
                kind: PaneKind::Worker,
                focused: false,
                title: "Worker 1".into(),
                exited: false,
            },
        },
    );
    let pane = receiver.recv().await.unwrap();
    assert_eq!(pane.kind, MachineEventKind::PaneAdded);
    assert_eq!(pane.pane_id.as_deref(), Some("worker-1"));

    events.reconcile_sessions(std::iter::empty::<&str>()).await;
    let removed = receiver.recv().await.unwrap();
    assert_eq!(removed.kind, MachineEventKind::SessionRemoved);
    assert_eq!(removed.session.as_deref(), Some("factory-a"));
}

#[test]
fn h1_runtime_state_is_single_instance_and_round_trips() {
    let temp = tempfile::tempdir().unwrap();
    let paths = HubRuntimePaths::new(temp.path().join("hub"));
    let first_lock = paths.acquire_instance_lock().unwrap();
    assert!(paths.acquire_instance_lock().is_err());

    let record = HubProcessRecord {
        pid: std::process::id(),
        bind: "127.0.0.1".into(),
        port: 4173,
        version: env!("CARGO_PKG_VERSION").into(),
        started_at: "2026-08-09T00:00:00Z".into(),
    };
    paths.write_process_record(&record).unwrap();
    assert_eq!(paths.read_process_record().unwrap(), record);

    drop(first_lock);
    assert!(paths.acquire_instance_lock().is_ok());
}

#[test]
fn h2_pair_02_pairing_is_bound_persistent_single_use_and_fragment_only() {
    use chrono::{Duration, Utc};

    let temp = tempfile::tempdir().unwrap();
    let state_dir = temp.path().join("hub");
    let now = Utc::now();
    let auth = AuthStore::open(&state_dir, "machine-test").unwrap();
    let invitation = auth
        .mint_pairing(
            "https://controller.example",
            Scope::default_read_only(),
            now,
        )
        .unwrap();
    assert!(invitation.url.contains("#pair="));
    assert!(!invitation.url.contains("?"));

    let exchange = PairingExchange::test_fixture(
        invitation.token.clone(),
        "machine-test",
        "https://controller.example",
        Scope::default_read_only(),
    );
    let credential = auth.exchange_pairing(exchange.clone(), now).unwrap();
    assert!(!credential.credential.is_empty());
    assert!(auth.exchange_pairing(exchange, now).is_err());

    let reopened = AuthStore::open(&state_dir, "machine-test").unwrap();
    assert_eq!(reopened.list_devices().unwrap().len(), 1);

    let expired = reopened
        .mint_pairing(
            "https://controller.example",
            Scope::default_read_only(),
            now,
        )
        .unwrap();
    let expired_exchange = PairingExchange::test_fixture(
        expired.token,
        "machine-test",
        "https://controller.example",
        Scope::default_read_only(),
    );
    assert!(
        reopened
            .exchange_pairing(expired_exchange, now + Duration::minutes(11))
            .is_err()
    );
}

#[tokio::test]
async fn h2_ws_04_ticket_is_five_minute_bound_single_use_under_race() {
    use chrono::{Duration, Utc};

    let temp = tempfile::tempdir().unwrap();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
    let now = Utc::now();
    let (_, context) = paired_context(&auth, now, Scope::default_read_only());
    let ticket = auth
        .issue_ws_ticket(&context, "factory-a", "/v1/sessions/factory-a/attach", now)
        .unwrap();
    assert_eq!(ticket.expires_at, now + Duration::minutes(5));

    let first = auth.clone();
    let second = auth.clone();
    let raw = ticket.ticket.clone();
    let raw_two = raw.clone();
    let (one, two) = tokio::join!(
        tokio::task::spawn_blocking(move || {
            first.consume_ws_ticket(
                &raw,
                "https://controller.example",
                "factory-a",
                "/v1/sessions/factory-a/attach",
                now,
            )
        }),
        tokio::task::spawn_blocking(move || {
            second.consume_ws_ticket(
                &raw_two,
                "https://controller.example",
                "factory-a",
                "/v1/sessions/factory-a/attach",
                now,
            )
        })
    );
    assert_eq!(
        usize::from(one.unwrap().is_ok()) + usize::from(two.unwrap().is_ok()),
        1
    );

    let expired = auth
        .issue_ws_ticket(&context, "factory-a", "/v1/sessions/factory-a/attach", now)
        .unwrap();
    assert!(
        auth.consume_ws_ticket(
            &expired.ticket,
            "https://controller.example",
            "factory-a",
            "/v1/sessions/factory-a/attach",
            now + Duration::minutes(6),
        )
        .is_err()
    );
}

#[test]
fn h2_pair_02_independent_store_instances_reload_and_serialize_mutations() {
    use chrono::Utc;
    use std::sync::Barrier;

    let temp = tempfile::tempdir().unwrap();
    let state_dir = temp.path().join("hub");
    let first = AuthStore::open(&state_dir, "machine-test").unwrap();
    let second = AuthStore::open(&state_dir, "machine-test").unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let now = Utc::now();

    let first_barrier = barrier.clone();
    let first_writer = std::thread::spawn(move || {
        first_barrier.wait();
        first
            .mint_pairing(
                "https://controller.example",
                Scope::default_read_only(),
                now,
            )
            .unwrap()
    });
    let second_writer = std::thread::spawn(move || {
        barrier.wait();
        second
            .mint_pairing(
                "https://controller.example",
                Scope::default_read_only(),
                now,
            )
            .unwrap()
    });
    let invitations = [first_writer.join().unwrap(), second_writer.join().unwrap()];

    let running_hub = AuthStore::open(&state_dir, "machine-test").unwrap();
    for invitation in invitations {
        let exchange = PairingExchange::test_fixture(
            invitation.token,
            "machine-test",
            "https://controller.example",
            Scope::default_read_only(),
        );
        running_hub.exchange_pairing(exchange, now).unwrap();
    }
    assert_eq!(running_hub.list_devices().unwrap().len(), 2);
}

#[test]
fn h2_scope_05_missing_device_context_is_fail_closed() {
    use chrono::Utc;

    let temp = tempfile::tempdir().unwrap();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
    let context = AuthContext::test_fixture(
        "missing-device",
        "https://controller.example",
        Scope::default_read_only(),
    );
    assert!(
        auth.issue_ws_ticket(
            &context,
            "factory-a",
            "/v1/sessions/factory-a/attach",
            Utc::now(),
        )
        .is_err()
    );
}

#[tokio::test]
async fn h2_ws_04_cli_revocation_disconnects_a_running_hub_socket() {
    use chrono::Utc;

    let temp = tempfile::tempdir().unwrap();
    let state_dir = temp.path().join("hub");
    let running_hub_auth = AuthStore::open(&state_dir, "machine-test").unwrap();
    let cli_auth = AuthStore::open(&state_dir, "machine-test").unwrap();
    let now = Utc::now();

    let signing = p256::ecdsa::SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let invitation = cli_auth
        .mint_pairing(
            "https://controller.example",
            Scope::default_read_only(),
            now,
        )
        .unwrap();
    let mut exchange = PairingExchange::test_fixture(
        invitation.token,
        "machine-test",
        "https://controller.example",
        Scope::default_read_only(),
    );
    exchange.public_key_jwk = public_jwk(&signing);
    let credential = running_hub_auth.exchange_pairing(exchange, now).unwrap();
    let proof = sign_dpop(
        &signing,
        &credential.credential,
        "GET",
        "/v1/bootstrap",
        now,
        "running-hub-context",
    );
    let context = running_hub_auth
        .authenticate_dpop(
            &format!("DPoP {}", credential.credential),
            &proof,
            "https://controller.example",
            "GET",
            "/v1/bootstrap",
            now,
        )
        .unwrap();

    let daemon_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let daemon_port = daemon_listener.local_addr().unwrap().port();
    let daemon = tokio::spawn(async move {
        let (stream, _) = daemon_listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let welcome = DaemonMessage::Welcome {
            session_name: "factory-a".into(),
            state: SessionState {
                focused_pane: None,
                panes: vec![],
                epic_id: None,
                epic_title: None,
                cols: 120,
                rows: 40,
            },
            scrollback: None,
            protocol_version: PROTOCOL_VERSION,
            capabilities: daemon_capabilities(),
        };
        socket
            .send(WsMessage::Binary(serde_json::to_vec(&welcome).unwrap()))
            .await
            .unwrap();
        futures_util::future::pending::<()>().await;
    });

    let mut session = fixture_session("factory-a");
    session.ws_port = Some(daemon_port);
    let events = MachineEventBus::new(16);
    let state = HubState::new(
        SessionCatalog::new(RecordingReadModel::with_sessions(vec![session])),
        Arc::new(PreAuthAuthorizer),
        MachineIdentity {
            id: "machine-test".into(),
        },
        DaemonConnector::new(SessionMultiplexer::new(8), events.clone()),
        events,
    )
    .with_auth(running_hub_auth.clone());
    let hub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hub_address = hub_listener.local_addr().unwrap();
    let hub = tokio::spawn(async move {
        axum::serve(hub_listener, router(state)).await.unwrap();
    });

    let endpoint = "/v1/sessions/factory-a/attach";
    let ticket = running_hub_auth
        .issue_ws_ticket(&context, "factory-a", endpoint, now)
        .unwrap();
    let mut request = format!("ws://{hub_address}{endpoint}?ticket={}", ticket.ticket)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", "https://controller.example".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert!(matches!(
        socket.next().await,
        Some(Ok(WsMessage::Binary(_)))
    ));

    cli_auth
        .revoke_device(&credential.device_id, Utc::now())
        .unwrap();
    let disconnected = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match socket.next().await {
                Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert!(
        disconnected.is_ok(),
        "a CLI revocation must close an already-upgraded socket in the running hub"
    );
    daemon.abort();
    hub.abort();
}

#[test]
fn h2_audit_06_independent_process_writers_append_complete_records() {
    use chrono::Utc;
    use std::sync::Barrier;

    let temp = tempfile::tempdir().unwrap();
    let state_dir = temp.path().join("hub");
    let first = AuthStore::open(&state_dir, "machine-test").unwrap();
    let second = AuthStore::open(&state_dir, "machine-test").unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let write = |store: AuthStore, barrier: Arc<Barrier>, action: &'static str| {
        std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..64 {
                store
                    .audit(None, "allowed", action, None, None, Utc::now())
                    .unwrap();
            }
        })
    };
    let one = write(first, barrier.clone(), "first-writer");
    let two = write(second, barrier, "second-writer");
    one.join().unwrap();
    two.join().unwrap();

    let audit = std::fs::read_to_string(state_dir.join("audit.jsonl")).unwrap();
    let records = audit
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 128);
    assert_eq!(
        records
            .iter()
            .filter(|record| record["action"] == "first-writer")
            .count(),
        64
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record["action"] == "second-writer")
            .count(),
        64
    );
}

#[test]
fn h2_scope_05_each_mutation_has_an_exact_scope_and_legacy_interrupt_is_forbidden() {
    use crate::ui::factory::MessageAttribution;

    let input = ClientMessage::Input {
        pane_id: "worker-1".into(),
        data: b"x".to_vec(),
    };
    let targeted = ClientMessage::InterruptPane {
        pane_id: "worker-1".into(),
    };
    let semantic = ClientMessage::SendMessage {
        target: "worker-1".into(),
        text: "status?".into(),
        summary: None,
        urgent: false,
        attribution: MessageAttribution {
            device_id: None,
            credential_id: None,
            device_label: None,
            operator_label: None,
            controller_origin: None,
            request_id: None,
        },
    };

    assert_eq!(required_scope(&input), Some(Scope::PaneInput));
    assert_eq!(required_scope(&targeted), Some(Scope::PaneInterrupt));
    assert_eq!(required_scope(&semantic), Some(Scope::MessageSend));
    assert_eq!(required_scope(&ClientMessage::Interrupt), None);
}

#[test]
fn h2_perm_01_rejects_loose_or_symlinked_machine_auth_state() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let temp = tempfile::tempdir().unwrap();
        let loose = temp.path().join("loose");
        std::fs::create_dir(&loose).unwrap();
        std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(AuthStore::open(&loose, "machine-test").is_err());

        let target = temp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = temp.path().join("link");
        symlink(&target, &link).unwrap();
        assert!(AuthStore::open(&link, "machine-test").is_err());
    }
}

#[test]
fn h2_dpop_03_proof_is_key_method_uri_ath_time_and_replay_bound() {
    use chrono::Utc;
    use p256::ecdsa::SigningKey;
    use p256::elliptic_curve::rand_core::OsRng;

    let temp = tempfile::tempdir().unwrap();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
    let now = Utc::now();
    let signing = SigningKey::random(&mut OsRng);
    let invitation = auth
        .mint_pairing(
            "https://controller.example",
            Scope::default_read_only(),
            now,
        )
        .unwrap();
    let mut exchange = PairingExchange::test_fixture(
        invitation.token,
        "machine-test",
        "https://controller.example",
        Scope::default_read_only(),
    );
    exchange.public_key_jwk = public_jwk(&signing);
    let credential = auth.exchange_pairing(exchange, now).unwrap();
    let authorization = format!("DPoP {}", credential.credential);
    let proof = sign_dpop(
        &signing,
        &credential.credential,
        "GET",
        "/v1/sessions",
        now,
        "jti-1",
    );
    assert!(
        auth.authenticate_dpop(
            &authorization,
            &proof,
            "https://controller.example",
            "GET",
            "/v1/sessions",
            now,
        )
        .is_ok()
    );
    assert!(
        auth.authenticate_dpop(
            &authorization,
            &proof,
            "https://controller.example",
            "GET",
            "/v1/sessions",
            now,
        )
        .is_err(),
        "a DPoP jti is accepted once"
    );
    let wrong_method = sign_dpop(
        &signing,
        &credential.credential,
        "GET",
        "/v1/sessions",
        now,
        "jti-2",
    );
    assert!(
        auth.authenticate_dpop(
            &authorization,
            &wrong_method,
            "https://controller.example",
            "POST",
            "/v1/sessions",
            now,
        )
        .is_err()
    );

    let mut revoked = auth.subscribe_revocations();
    auth.revoke_device(&credential.device_id, now).unwrap();
    assert_eq!(revoked.try_recv().unwrap(), credential.device_id);
    let after_revoke = sign_dpop(
        &signing,
        &credential.credential,
        "GET",
        "/v1/sessions",
        now,
        "jti-3",
    );
    assert!(
        auth.authenticate_dpop(
            &authorization,
            &after_revoke,
            "https://controller.example",
            "GET",
            "/v1/sessions",
            now,
        )
        .is_err(),
        "revocation takes effect on the next request"
    );

    let audit = std::fs::read_to_string(temp.path().join("hub/audit.jsonl")).unwrap();
    assert!(audit.contains("dpop_replay") && audit.contains("device_revoke"));
    assert!(!audit.contains(&credential.credential));
}

fn public_jwk(signing: &p256::ecdsa::SigningKey) -> PublicJwk {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let point = signing.verifying_key().to_encoded_point(false);
    PublicJwk {
        kty: "EC".into(),
        crv: "P-256".into(),
        x: URL_SAFE_NO_PAD.encode(point.x().unwrap()),
        y: URL_SAFE_NO_PAD.encode(point.y().unwrap()),
    }
}

fn paired_context(
    auth: &AuthStore,
    now: chrono::DateTime<chrono::Utc>,
    scopes: std::collections::BTreeSet<Scope>,
) -> (DeviceCredential, AuthContext) {
    use p256::ecdsa::SigningKey;
    use p256::elliptic_curve::rand_core::OsRng;

    let signing = SigningKey::random(&mut OsRng);
    let invitation = auth
        .mint_pairing("https://controller.example", scopes.clone(), now)
        .unwrap();
    let mut exchange = PairingExchange::test_fixture(
        invitation.token,
        "machine-test",
        "https://controller.example",
        scopes,
    );
    exchange.public_key_jwk = public_jwk(&signing);
    let credential = auth.exchange_pairing(exchange, now).unwrap();
    let proof = sign_dpop(
        &signing,
        &credential.credential,
        "GET",
        "/v1/test-context",
        now,
        &uuid::Uuid::new_v4().to_string(),
    );
    let context = auth
        .authenticate_dpop(
            &format!("DPoP {}", credential.credential),
            &proof,
            "https://controller.example",
            "GET",
            "/v1/test-context",
            now,
        )
        .unwrap();
    (credential, context)
}

fn sign_dpop(
    signing: &p256::ecdsa::SigningKey,
    credential: &str,
    method: &str,
    uri: &str,
    now: chrono::DateTime<chrono::Utc>,
    jti: &str,
) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use p256::ecdsa::signature::Signer;
    use sha2::{Digest, Sha256};

    let header = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&serde_json::json!({
            "typ":"dpop+jwt", "alg":"ES256", "jwk":public_jwk(signing)
        }))
        .unwrap(),
    );
    let claims = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&serde_json::json!({
            "htm":method,
            "htu":uri,
            "iat":now.timestamp(),
            "jti":jti,
            "ath":URL_SAFE_NO_PAD.encode(Sha256::digest(credential.as_bytes())),
        }))
        .unwrap(),
    );
    let input = format!("{header}.{claims}");
    let signature: p256::ecdsa::Signature = signing.sign(input.as_bytes());
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
}
