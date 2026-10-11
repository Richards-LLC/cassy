use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::{Command, Stdio};
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
    ClientMessage, DaemonMessage, PROTOCOL_VERSION, PaneBootstrap, PaneInfo, PaneKind,
    SessionState, daemon_capabilities,
};

/// cas-5e53: an already-open SSE grant must end after another auth-store
/// process revokes its device, even with no event traffic or browser heartbeat.
#[tokio::test]
async fn installation_revoke_ends_open_sse_and_refuses_inventory_credential() {
    installation_revoke_sse(false).await;
}

#[tokio::test]
async fn installation_revoke_ends_idle_live_sse_after_replay_complete_cas_2b3a5() {
    installation_revoke_sse(true).await;
}

async fn installation_revoke_sse(drain_replay: bool) {
    use chrono::Utc;
    use p256::ecdsa::SigningKey;
    use p256::elliptic_curve::rand_core::OsRng;
    let temp = private_tempdir();
    let root = temp.path().join("hub");
    let auth = AuthStore::open(&root, "machine-test").unwrap();
    let signing = SigningKey::random(&mut OsRng);
    let now = Utc::now();
    let invitation = auth.mint_pairing("https://controller.example", Scope::default_read_only(), now).unwrap();
    let mut exchange = PairingExchange::test_fixture(invitation.token, "machine-test", "https://controller.example", Scope::default_read_only());
    exchange.public_key_jwk = public_jwk(&signing);
    let credential = auth.exchange_pairing(exchange, now).unwrap();
    let events = MachineEventBus::new(16);
    let app = router(HubState::new(
        SessionCatalog::new(RecordingReadModel::with_sessions(vec![fixture_session("factory-a")])),
        Arc::new(PreAuthAuthorizer), MachineIdentity { id: "machine-test".into() },
        DaemonConnector::new(SessionMultiplexer::new(8), events.clone()), events,
    ).with_auth(auth));
    let request = |path: &str| Request::get(path)
        .header("origin", "https://controller.example")
        .header("authorization", format!("DPoP {}", credential.credential))
        .header("dpop", sign_dpop(&signing, &credential.credential, "GET", path, Utc::now(), &uuid::Uuid::new_v4().to_string()))
        .body(Body::empty()).unwrap();
    let inventory = app.clone().oneshot(request("/v1/auth/devices")).await.unwrap();
    assert_eq!(inventory.status(), StatusCode::OK);
    let devices: serde_json::Value = serde_json::from_slice(&to_bytes(inventory.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(devices[0]["device_id"], credential.device_id);
    assert_eq!(devices[0]["account_enrollment"]["state"], "unenrolled");
    let opened = app.clone().oneshot(request("/v1/events")).await.unwrap();
    assert_eq!(opened.status(), StatusCode::OK);
    let mut stream = opened.into_body().into_data_stream();
    if drain_replay {
        let mut received = String::new();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !received.contains("event: replay_complete") {
                let frame = stream.next().await.expect("authorized replay stays open").unwrap();
                received.push_str(std::str::from_utf8(&frame).unwrap());
            }
        }).await.expect("metadata and complete marker arrive before revocation");
        assert!(received.contains("event: stream_metadata"));
    }
    let other_process = AuthStore::open(&root, "machine-test").unwrap();
    other_process.revoke_device(&credential.device_id, Utc::now()).unwrap();
    let ended = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next()).await.expect("SSE must terminate without a heartbeat");
    assert!(ended.is_none());
    assert_eq!(app.oneshot(request("/v1/auth/devices")).await.unwrap().status(), StatusCode::UNAUTHORIZED);
}

/// cas-4634: the account challenge is issued to an authenticated device,
/// and an enrollment assertion on a hub with no operator-inbox principal is
/// refused with a closed code, leaving the installation unenrolled.
#[tokio::test]
async fn account_enrollment_routes_need_a_session_and_an_enrolled_hub() {
    use chrono::Utc;
    use p256::ecdsa::SigningKey;
    use p256::elliptic_curve::rand_core::OsRng;
    let temp = private_tempdir();
    let root = temp.path().join("hub");
    let auth = AuthStore::open(&root, "machine-test").unwrap();
    let signing = SigningKey::random(&mut OsRng);
    let now = Utc::now();
    let invitation = auth.mint_pairing("https://controller.example", Scope::default_read_only(), now).unwrap();
    let mut exchange = PairingExchange::test_fixture(invitation.token, "machine-test", "https://controller.example", Scope::default_read_only());
    exchange.public_key_jwk = public_jwk(&signing);
    let credential = auth.exchange_pairing(exchange, now).unwrap();
    let events = MachineEventBus::new(16);
    let app = router(HubState::new(
        SessionCatalog::new(RecordingReadModel::with_sessions(vec![fixture_session("factory-a")])),
        Arc::new(PreAuthAuthorizer), MachineIdentity { id: "machine-test".into() },
        DaemonConnector::new(SessionMultiplexer::new(8), events.clone()), events,
    ).with_auth(auth));
    let post = |path: &str, body: &str, signed: bool| {
        let mut request = Request::post(path)
            .header("origin", "https://controller.example")
            .header("content-type", "application/json");
        if signed {
            request = request
                .header("authorization", format!("DPoP {}", credential.credential))
                .header("dpop", sign_dpop(&signing, &credential.credential, "POST", path, Utc::now(), &uuid::Uuid::new_v4().to_string()));
        }
        request.body(Body::from(body.to_owned())).unwrap()
    };
    let refused = app.clone().oneshot(post("/v1/auth/account/challenge", "{}", false)).await.unwrap();
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    let issued = app.clone().oneshot(post("/v1/auth/account/challenge", "{}", true)).await.unwrap();
    assert_eq!(issued.status(), StatusCode::OK);
    let issued: serde_json::Value = serde_json::from_slice(&to_bytes(issued.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(issued["hub_id"], "machine-test");
    assert!(issued["hub_challenge"].as_str().is_some_and(|challenge| challenge.len() >= 32));
    let enrollment = app.clone().oneshot(post("/v1/auth/account/enrollment", r#"{"assertion":"a.b.c"}"#, true)).await.unwrap();
    assert_eq!(enrollment.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_slice(&to_bytes(enrollment.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(body["error"], "hub_not_enrolled");
    let inventory = app.oneshot(Request::get("/v1/auth/devices")
        .header("origin", "https://controller.example")
        .header("authorization", format!("DPoP {}", credential.credential))
        .header("dpop", sign_dpop(&signing, &credential.credential, "GET", "/v1/auth/devices", Utc::now(), &uuid::Uuid::new_v4().to_string()))
        .body(Body::empty()).unwrap()).await.unwrap();
    let devices: serde_json::Value = serde_json::from_slice(&to_bytes(inventory.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(devices[0]["account_enrollment"]["state"], "unenrolled");
}

#[test]
fn operator_reply_relay_reaches_another_authenticated_device() {
    let temp = private_tempdir();
    let auth = AuthStore::open(&temp.path().join("hub"), "machine-test").unwrap();
    let viewer = AuthContext {
        device_id: "computer".into(),
        credential_id: "computer-credential".into(),
        device_label: "Desktop".into(),
        operator_label: "Daniel".into(),
        controller_origin: "https://controller.example".into(),
        scopes: [Scope::PaneRead].into_iter().collect(),
        request_id: "request-shared".into(),
    };
    let frame = serde_json::to_vec(&DaemonMessage::OperatorReply {
        notification_id: 42,
        reply_to: Some(41),
        message: "From supervisor".into(),
        summary: "reply".into(),
        device_id: "phone".into(),
        operator_label: None,
        kind: crate::ui::factory::OperatorTurnKind::Answer,
        attachments: Vec::new(),
        notice: None,
        reply_to_session: None,
    }).unwrap();
    assert!(super::server::operator_reply_allowed(&Some((auth, viewer)), &frame));
    assert!(!super::server::operator_reply_allowed(&None, &frame));
}

/// Hub state initialization intentionally refuses to traverse symlinked path
/// components. macOS exposes its temporary directory through `/var`, which is
/// a symlink to `/private/var`, so fixtures must start at the canonical root.
fn private_tempdir() -> tempfile::TempDir {
    let parent = std::env::temp_dir()
        .canonicalize()
        .expect("temporary directory must be canonicalizable");
    tempfile::tempdir_in(parent).expect("canonical temporary fixture directory")
}

// This fixture launches a second test binary, then waits for both its Tokio
// runtime and the parent-side reaper/connector chain to receive CPU time.
//
// cas-e207: do NOT "fix" a timeout here by raising this again. The slow step
// was never scheduling: with a pipe `core_pattern` (systemd-coredump, apport)
// and a non-zero `core_pipe_limit`, the kernel keeps the SIGILL'd child — and
// the socket whose close is the disconnect this test waits for — alive until
// the helper finishes symbolizing the ~1 GB debug test binary: 6.5 s idle on
// the factory host, past 15 s under load. The fixture child now zeroes its
// soft RLIMIT_CORE under a pipe pattern (see
// `h1_death_05_fixture_process_entry`), which keeps WCOREDUMP but lets the
// helper return at once. If this budget is ever hit again, find what the
// child is waiting on instead of lengthening the wait. 30 s is headroom for a
// saturated host, many times the post-fix idle runtime.
const H1_DEATH_REAL_PROCESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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
    let keyframe = proxy_frame(DaemonMessage::PaneKeyframe {
        pane_id: "worker-1".into(),
        epoch: 7,
        seq: 99,
        cols: 80,
        rows: 24,
        ansi: b"fresh screen".to_vec(),
    });
    mux.publish("factory-a", keyframe.clone()).await.unwrap();
    assert_eq!(
        slow.recv().await.unwrap().bytes,
        keyframe.bytes,
        "lag recovery skips stale deltas and accepts an authoritative keyframe"
    );
    assert_eq!(fast.recv().await.unwrap().bytes, keyframe.bytes);
    assert_eq!(mux.upstream_start_count("factory-a").await, 1);
    assert!(fast.try_recv().is_err());
}

#[test]
fn h1_death_05_reports_clean_signal_sigill_and_unknown_without_invention() {
    assert_eq!(
        diagnose_daemon_death(Some(ProcessExit::Code(0)), Some(true)).cause,
        DaemonDeathCause::CleanExit { code: 0 }
    );
    assert_eq!(
        diagnose_daemon_death(Some(ProcessExit::Signal(4)), Some(true)).cause,
        DaemonDeathCause::Signal {
            signal: 4,
            name: Some("SIGILL".into()),
            core_dumped: Some(true),
        }
    );
    assert_eq!(
        diagnose_daemon_death(Some(ProcessExit::Signal(15)), Some(false)).cause,
        DaemonDeathCause::Signal {
            signal: 15,
            name: Some("SIGTERM".into()),
            core_dumped: Some(false),
        }
    );
    assert_eq!(
        diagnose_daemon_death(Some(ProcessExit::Signal(9)), None).cause,
        DaemonDeathCause::Signal {
            signal: 9,
            name: Some("SIGKILL".into()),
            core_dumped: None,
        }
    );
    assert_eq!(
        diagnose_daemon_death(Some(ProcessExit::Code(7)), None).cause,
        DaemonDeathCause::ExitCode { code: 7 }
    );
    assert_eq!(
        diagnose_daemon_death(None, None).cause,
        DaemonDeathCause::Unknown
    );
    assert!(
        diagnose_daemon_death(Some(ProcessExit::Signal(4)), Some(false))
            .next_action
            .contains("portable release artifact")
    );
}

#[test]
fn h1_death_05_fixture_process_entry() {
    let Ok(port_file) = std::env::var("CAS_H1_DEATH_FIXTURE_PORT_FILE") else {
        return;
    };
    // cas-e207: under a pipe core_pattern the kernel holds this process (and
    // its socket) open until the coredump helper finishes, and symbolizing
    // this debug test binary takes seconds. A zero soft RLIMIT_CORE makes the
    // helper skip the work while the kernel still reports WCOREDUMP, so the
    // parent's `core_dumped: Some(true)` assertion holds. A file pattern (or
    // macOS) is left unchanged: there a zero limit would suppress the dump
    // and the core flag with it.
    #[cfg(target_os = "linux")]
    {
        let pipe_pattern = std::fs::read_to_string("/proc/sys/kernel/core_pattern")
            .is_ok_and(|pattern| pattern.trim_start().starts_with('|'));
        if pipe_pattern {
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            // SAFETY: plain getrlimit/setrlimit on this fixture process with a
            // valid, initialised `rlimit`; only the soft core limit changes.
            unsafe {
                if libc::getrlimit(libc::RLIMIT_CORE, &mut limit) == 0 {
                    limit.rlim_cur = 0;
                    libc::setrlimit(libc::RLIMIT_CORE, &limit);
                }
            }
        }
    }
    std::fs::write(
        std::path::Path::new(&port_file).with_extension("started"),
        "hub::tests::h1_death_05_fixture_process_entry",
    ).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        std::fs::write(port_file, listener.local_addr().unwrap().port().to_string()).unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let welcome = DaemonMessage::Welcome {
            session_name: "death-fixture".into(),
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
            pane_bootstrap: Vec::new(),
        };
        socket
            .send(WsMessage::Binary(serde_json::to_vec(&welcome).unwrap()))
            .await
            .unwrap();
        futures_util::future::pending::<()>().await;
    });
}

#[cfg(unix)]
#[tokio::test]
async fn h1_death_05_real_sigill_fixture_preserves_exact_diagnostic_without_multiplication() {
    let temp = private_tempdir();
    let port_file = temp.path().join("port");
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "hub::tests::h1_death_05_fixture_process_entry",
            "--nocapture",
        ])
        .env("CAS_H1_DEATH_FIXTURE_PORT_FILE", &port_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let store = DaemonExitEvidenceStore::new(temp.path().join("daemon-exits"));
    let (identity, reaper) = supervise_spawned_daemon("death-fixture", child, store.clone())
        .expect("Linux fixture has a process-start fingerprint");

    let port = tokio::time::timeout(H1_DEATH_REAL_PROCESS_TIMEOUT, async {
        loop {
            if let Ok(value) = std::fs::read_to_string(&port_file) {
                break value.parse::<u16>().unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    // This child intentionally dies from SIGILL before libtest can emit a
    // passing summary. The entry receipt proves its test body actually ran.
    assert_eq!(
        std::fs::read_to_string(port_file.with_extension("started")).unwrap(),
        "hub::tests::h1_death_05_fixture_process_entry",
    );

    let source = RecordingReadModel::with_sessions(vec![fixture_session("death-fixture")]);
    let catalog = SessionCatalog::new(source.clone());
    assert_eq!(catalog.list().await.unwrap().len(), 1);
    let events = MachineEventBus::new(8);
    let mut event_rx = events.subscribe();
    let connector =
        DaemonConnector::new(SessionMultiplexer::new(8), events).with_exit_evidence_store(store);
    let mut viewer = connector
        .attach(
            "death-fixture",
            port,
            std::iter::empty::<String>(),
            Some(identity.clone()),
        )
        .await
        .unwrap();
    let welcome = viewer.recv().await.unwrap();
    assert!(matches!(
        serde_json::from_slice::<DaemonMessage>(&welcome.bytes).unwrap(),
        DaemonMessage::Welcome { .. }
    ));
    assert_eq!(
        connector.upstream_connection_count("death-fixture").await,
        1
    );

    // SAFETY: exact child pid is fingerprinted above and owned by this test.
    assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGILL) }, 0);
    let disconnected = tokio::time::timeout(H1_DEATH_REAL_PROCESS_TIMEOUT, async {
        loop {
            let event = event_rx.recv().await.unwrap();
            if event.kind == MachineEventKind::DaemonDisconnected {
                break event;
            }
        }
    })
    .await
    .unwrap();
    let diagnostic = disconnected.diagnostic.unwrap();
    assert_eq!(
        diagnostic.cause,
        DaemonDeathCause::Signal {
            signal: libc::SIGILL,
            name: Some("SIGILL".into()),
            core_dumped: Some(cfg!(target_os = "linux")),
        }
    );
    assert!(diagnostic.next_action.contains("portable release artifact"));
    reaper.join().unwrap();

    assert_eq!(source.model_call_count(), 0);
    assert_eq!(source.logical_session_create_count(), 0);
    assert_eq!(catalog.list().await.unwrap().len(), 1);
    assert_eq!(
        connector.upstream_connection_count("death-fixture").await,
        1
    );
}

#[tokio::test]
async fn h1_death_05_receipts_distinguish_exit_and_signal_and_reject_stale_epoch() {
    let temp = private_tempdir();
    let store = DaemonExitEvidenceStore::new(temp.path());
    let identity = DaemonIdentity {
        session: "factory-a".into(),
        pid: 100,
        pid_starttime: 200,
    };
    store
        .write(&DaemonExitReceipt {
            identity: identity.clone(),
            exit: ProcessExit::Code(0),
            core_dumped: None,
            observed_at: "2026-08-09T00:00:00Z".into(),
        })
        .unwrap();
    assert_eq!(
        super::death::diagnose_disconnect(Some(&identity), Some(&store))
            .await
            .cause,
        DaemonDeathCause::CleanExit { code: 0 }
    );

    store
        .write(&DaemonExitReceipt {
            identity: identity.clone(),
            exit: ProcessExit::Signal(15),
            core_dumped: Some(false),
            observed_at: "2026-08-09T00:00:01Z".into(),
        })
        .unwrap();
    assert_eq!(
        super::death::diagnose_disconnect(Some(&identity), Some(&store))
            .await
            .cause,
        DaemonDeathCause::Signal {
            signal: 15,
            name: Some("SIGTERM".into()),
            core_dumped: Some(false),
        }
    );

    let replacement_epoch = DaemonIdentity {
        pid_starttime: identity.pid_starttime + 1,
        ..identity
    };
    assert!(store.read_matching(&replacement_epoch).is_none());
    assert_eq!(
        super::death::diagnose_disconnect(Some(&replacement_epoch), Some(&store))
            .await
            .cause,
        DaemonDeathCause::Unknown
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn h1_death_05_live_fingerprinted_daemon_is_transport_loss_not_a_signal() {
    let temp = private_tempdir();
    let identity = DaemonIdentity {
        session: "factory-a".into(),
        pid: std::process::id(),
        pid_starttime: crate::mcp::daemon::read_pid_starttime(std::process::id()).unwrap(),
    };
    let diagnostic = super::death::diagnose_disconnect(
        Some(&identity),
        Some(&DaemonExitEvidenceStore::new(temp.path())),
    )
    .await;
    assert_eq!(diagnostic.cause, DaemonDeathCause::TransportLost);
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
    let temp = private_tempdir();
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
    assert!(!health.headers().contains_key("access-control-allow-origin"));
    let health: serde_json::Value =
        serde_json::from_slice(&to_bytes(health.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(
        health,
        serde_json::json!({"schema_version": 1, "ready": true})
    );

    let favicon = app
        .clone()
        .oneshot(
            Request::get("/commander/favicon.svg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(favicon.status(), StatusCode::OK);
    assert_eq!(favicon.headers()["content-type"], "image/svg+xml");
    assert!(
        to_bytes(favicon.into_body(), usize::MAX)
            .await
            .unwrap()
            .starts_with(b"<svg")
    );

    // cas-8951: every icon and the manifest index.html links is served.
    for (uri, content_type, magic) in [
        ("/commander/favicon-16.png", "image/png", &b"\x89PNG"[..]),
        ("/commander/favicon-32.png", "image/png", &b"\x89PNG"[..]),
        ("/commander/apple-touch-icon.png", "image/png", &b"\x89PNG"[..]),
        ("/commander/icon-192.png", "image/png", &b"\x89PNG"[..]),
        ("/commander/icon-512.png", "image/png", &b"\x89PNG"[..]),
        ("/commander/icon-maskable-192.png", "image/png", &b"\x89PNG"[..]),
        ("/commander/icon-maskable-512.png", "image/png", &b"\x89PNG"[..]),
        (
            "/commander/manifest.webmanifest",
            "application/manifest+json",
            &b"{"[..],
        ),
        (
            "/commander/cassy-tokens.css",
            "text/css; charset=utf-8",
            &b"/* Cassy Cloud design tokens"[..],
        ),
    ] {
        let asset = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(asset.status(), StatusCode::OK, "{uri}");
        assert_eq!(asset.headers()["content-type"], content_type, "{uri}");
        assert!(
            to_bytes(asset.into_body(), usize::MAX)
                .await
                .unwrap()
                .starts_with(magic),
            "{uri}"
        );
    }

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

    for uri in ["/v1/projects", "/v1/projects/browse?root=missing", "/v1/launch/profiles"] {
        let denied = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(source.read_count(), 0);

    let denied_launch = app.clone().oneshot(
        Request::post("/v1/sessions")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"target":{"kind":"project","id":"unknown"},"supervisor_cli":"claude","profile":"main"}"#))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(denied_launch.status(), StatusCode::UNAUTHORIZED);

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
async fn h4_health_cors_allows_unpaired_trusted_origins_and_preserves_paired_origins() {
    use chrono::Utc;
    use p256::ecdsa::SigningKey;
    use p256::elliptic_curve::rand_core::OsRng;

    let temp = private_tempdir();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
    let now = Utc::now();
    let signing = SigningKey::random(&mut OsRng);
    assert!(!Scope::default_read_only().contains(&Scope::SessionLaunch));
    let invitation = auth
        .mint_pairing("http://127.0.0.1:4173", Scope::default_read_only(), now)
        .unwrap();
    let mut exchange = PairingExchange::test_fixture(
        invitation.token,
        "machine-test",
        "http://127.0.0.1:4173",
        Scope::default_read_only(),
    );
    exchange.public_key_jwk = public_jwk(&signing);
    let credential = auth.exchange_pairing(exchange, now).unwrap();
    let events = MachineEventBus::new(16);
    let app = router(
        HubState::new(
            SessionCatalog::new(RecordingReadModel::with_sessions(vec![fixture_session(
                "factory-a",
            )])),
            Arc::new(PreAuthAuthorizer),
            MachineIdentity {
                id: "machine-test".into(),
            },
            DaemonConnector::new(SessionMultiplexer::new(8), events.clone()),
            events,
        )
        .with_auth(auth.clone())
        .with_effective_origin("http://127.0.0.1:4173"),
    );
    let authorization = format!("DPoP {}", credential.credential);
    let health = app
        .clone()
        .oneshot(
            Request::get("/v1/health")
                .header("origin", "http://127.0.0.1:4173")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        health.headers()["access-control-allow-origin"],
        "http://127.0.0.1:4173"
    );
    assert_eq!(health.headers()["vary"], "Origin");
    let unpaired_trusted_health = app
        .clone()
        .oneshot(
            Request::get("/v1/health")
                .header("origin", "https://hub.petrastella.io")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unpaired_trusted_health.status(), StatusCode::OK);
    assert_eq!(
        unpaired_trusted_health.headers()["access-control-allow-origin"],
        "https://hub.petrastella.io"
    );
    assert_eq!(unpaired_trusted_health.headers()["vary"], "Origin");
    let unpaired_health = app
        .clone()
        .oneshot(
            Request::get("/v1/health")
                .header("origin", "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        !unpaired_health
            .headers()
            .contains_key("access-control-allow-origin")
    );
    let health_preflight = app
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/v1/health")
                .header("origin", "https://hub.petrastella.io")
                .header("access-control-request-method", "GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health_preflight.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        health_preflight.headers()["access-control-allow-origin"],
        "https://hub.petrastella.io"
    );
    assert_eq!(health_preflight.headers()["vary"], "Origin");
    let proof = |method: &str, uri: &str| {
        sign_dpop(
            &signing,
            &credential.credential,
            method,
            uri,
            now,
            &uuid::Uuid::new_v4().to_string(),
        )
    };

    let allowed = app
        .clone()
        .oneshot(
            Request::get("/v1/sessions")
                .header("host", "127.0.0.1:4173")
                .header("sec-fetch-site", "same-origin")
                .header("authorization", &authorization)
                .header("dpop", proof("GET", "/v1/sessions"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);

    let launch_denied = app
        .clone()
        .oneshot(
            Request::post("/v1/sessions")
                .header("origin", "http://127.0.0.1:4173")
                .header("authorization", &authorization)
                .header("dpop", proof("POST", "/v1/sessions"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"target":{"kind":"project","id":"unknown"},"supervisor_cli":"claude"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(launch_denied.status(), StatusCode::FORBIDDEN);

    // cas-55a4: ending a session needs factory:manage, which a read-only
    // pairing does not hold; the refusal names the scope.
    let end_denied = app
        .clone()
        .oneshot(
            Request::delete("/v1/sessions/factory-a")
                .header("origin", "http://127.0.0.1:4173")
                .header("authorization", &authorization)
                .header("dpop", proof("DELETE", "/v1/sessions/factory-a"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(end_denied.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(end_denied.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["required_scope"], "factory:manage");

    let extra_args = app.clone().oneshot(
        Request::post("/v1/sessions")
            .header("origin", "http://127.0.0.1:4173")
            .header("authorization", &authorization)
            .header("dpop", proof("POST", "/v1/sessions"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"target":{"kind":"project","id":"unknown"},"supervisor_cli":"claude","args":["--cwd","/tmp/other"]}"#))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(extra_args.status(), StatusCode::UNPROCESSABLE_ENTITY);

    for (site, host) in [
        (None, "127.0.0.1:4173"),
        (Some("cross-site"), "127.0.0.1:4173"),
        (Some("same-origin"), "127.0.0.1:9999"),
    ] {
        let mut request = Request::get("/v1/sessions")
            .header("host", host)
            .header("authorization", &authorization)
            .header("dpop", proof("GET", "/v1/sessions"));
        if let Some(site) = site {
            request = request.header("sec-fetch-site", site);
        }
        assert_eq!(
            app.clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    let mutation = app
        .clone()
        .oneshot(
            Request::post("/v1/auth/websocket-ticket")
                .header("host", "127.0.0.1:4173")
                .header("sec-fetch-site", "same-origin")
                .header("authorization", authorization)
                .header("dpop", proof("POST", "/v1/auth/websocket-ticket"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"session":"factory-a"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mutation.status(), StatusCode::UNAUTHORIZED);
    assert!(
        std::fs::read_to_string(temp.path().join("hub/audit.jsonl"))
            .unwrap()
            .contains("dpop_auth"),
        "the accepted real-browser read reaches DPoP verification and audit"
    );

    let launch_scopes: std::collections::BTreeSet<Scope> = [Scope::MachineRead, Scope::SessionLaunch].into_iter().collect();
    let invitation = auth
        .mint_pairing("http://127.0.0.1:4173", launch_scopes.clone(), Utc::now())
        .unwrap();
    let mut launch_exchange = PairingExchange::test_fixture(
        invitation.token,
        "machine-test",
        "http://127.0.0.1:4173",
        launch_scopes,
    );
    launch_exchange.public_key_jwk = public_jwk(&signing);
    let launch_credential = auth.exchange_pairing(launch_exchange, Utc::now()).unwrap();
    let before_revoke = app.clone().oneshot(
        Request::post("/v1/sessions")
            .header("origin", "http://127.0.0.1:4173")
            .header("authorization", format!("DPoP {}", launch_credential.credential))
            .header("dpop", sign_dpop(&signing, &launch_credential.credential, "POST", "/v1/sessions", Utc::now(), &uuid::Uuid::new_v4().to_string()))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"target":{"kind":"project","id":"unknown"},"supervisor_cli":"bogus"}"#))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(before_revoke.status(), StatusCode::BAD_REQUEST);
    for body in [
        r#"{"target":{"kind":"project","id":"unknown"},"supervisor_cli":"claude; echo unsafe"}"#,
        r#"{"target":{"kind":"project","id":"unknown"},"supervisor_cli":"claude","name":"../escape"}"#,
    ] {
        let refused = app.clone().oneshot(
            Request::post("/v1/sessions")
                .header("origin", "http://127.0.0.1:4173")
                .header("authorization", format!("DPoP {}", launch_credential.credential))
                .header("dpop", sign_dpop(&signing, &launch_credential.credential, "POST", "/v1/sessions", Utc::now(), &uuid::Uuid::new_v4().to_string()))
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        ).await.unwrap();
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    }
    auth.revoke_device(&launch_credential.device_id, chrono::Utc::now())
        .unwrap();
    let revoked_launch = app
        .oneshot(
            Request::post("/v1/sessions")
                .header("origin", "http://127.0.0.1:4173")
                .header("authorization", format!("DPoP {}", launch_credential.credential))
                .header("dpop", sign_dpop(&signing, &launch_credential.credential, "POST", "/v1/sessions", Utc::now(), &uuid::Uuid::new_v4().to_string()))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"target":{"kind":"project","id":"unknown"},"supervisor_cli":"claude"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked_launch.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn h4_pairing_preflight_allows_only_the_exact_bootstrap_shape() {
    let events = MachineEventBus::new(16);
    let app = router(HubState::new(
        SessionCatalog::new(RecordingReadModel::with_sessions(vec![])),
        Arc::new(PreAuthAuthorizer),
        MachineIdentity {
            id: "machine-test".into(),
        },
        DaemonConnector::new(SessionMultiplexer::new(8), events.clone()),
        events,
    ));
    let preflight = |path: &str, origin: &str, method: &str, headers: &str| {
        Request::builder()
            .method("OPTIONS")
            .uri(path)
            .header("origin", origin)
            .header("access-control-request-method", method)
            .header("access-control-request-headers", headers)
            .body(Body::empty())
            .unwrap()
    };

    let allowed = app
        .clone()
        .oneshot(preflight(
            "/v1/auth/pairing/exchange",
            "http://127.0.0.1:4173",
            "POST",
            "content-type",
        ))
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        allowed.headers()["access-control-allow-origin"],
        "http://127.0.0.1:4173"
    );
    assert_eq!(allowed.headers()["vary"], "Origin");
    assert_eq!(allowed.headers()["access-control-allow-methods"], "POST");
    assert_eq!(
        allowed.headers()["access-control-allow-headers"],
        "Content-Type"
    );
    assert!(
        !allowed
            .headers()
            .contains_key("access-control-allow-credentials")
    );

    for request in [
        preflight(
            "/v1/auth/pairing/exchange",
            "http://192.168.1.8:4173",
            "POST",
            "content-type",
        ),
        preflight("/v1/auth/pairing/exchange", "null", "POST", "content-type"),
        preflight(
            "/v1/auth/pairing/exchange",
            "http://127.0.0.1:4173",
            "DELETE",
            "content-type",
        ),
        preflight(
            "/v1/auth/pairing/exchange",
            "http://127.0.0.1:4173",
            "POST",
            "content-type,authorization",
        ),
        preflight(
            "/v1/auth/websocket-ticket",
            "http://127.0.0.1:4173",
            "POST",
            "content-type",
        ),
    ] {
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
}

#[tokio::test]
async fn h2_pair_02_pairing_exchange_cors_covers_bound_browser_responses() {
    use chrono::Utc;

    let temp = private_tempdir();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
    let now = Utc::now();
    let origin = "https://controller.example";
    let events = MachineEventBus::new(16);
    let app = router(
        HubState::new(
            SessionCatalog::new(RecordingReadModel::with_sessions(vec![])),
            Arc::new(PreAuthAuthorizer),
            MachineIdentity {
                id: "machine-test".into(),
            },
            DaemonConnector::new(SessionMultiplexer::new(8), events.clone()),
            events,
        )
        .with_auth(auth.clone())
        .with_response_transport(TransportSecurity::TrustedLoopbackTlsProxy),
    );

    let preflight = app
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/v1/auth/pairing/exchange")
                .header("origin", origin)
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", "content-type")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
    assert_eq!(preflight.headers()["access-control-allow-origin"], origin);
    assert_eq!(preflight.headers()["vary"], "Origin");
    assert_eq!(
        preflight.headers()["strict-transport-security"],
        "max-age=31536000"
    );

    let refused_invitation = auth
        .mint_pairing(origin, Scope::default_read_only(), now)
        .unwrap();
    let mut refused_exchange = PairingExchange::test_fixture(
        refused_invitation.token,
        "machine-test",
        origin,
        Scope::default_read_only(),
    );
    refused_exchange.requested_scopes.insert(Scope::HubAdmin);
    assert!(auth.list_devices().unwrap().is_empty());
    let refused = app
        .clone()
        .oneshot(
            Request::post("/v1/auth/pairing/exchange")
                .header("origin", origin)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&refused_exchange).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        refused.headers()["access-control-allow-origin"],
        origin,
        "a browser must be able to read the generic refusal for its exactly bound pairing"
    );
    assert_eq!(refused.headers()["vary"], "Origin");
    assert_eq!(
        refused.headers()["strict-transport-security"],
        "max-age=31536000"
    );
    assert!(
        !refused
            .headers()
            .contains_key("access-control-allow-credentials")
    );
    assert!(auth.list_devices().unwrap().is_empty());

    let hostile = app
        .clone()
        .oneshot(
            Request::post("/v1/auth/pairing/exchange")
                .header("origin", "https://evil.example")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&PairingExchange {
                        installation: None,
                        controller_origin: "https://evil.example".into(),
                        ..refused_exchange.clone()
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hostile.status(), StatusCode::UNAUTHORIZED);
    assert!(
        !hostile
            .headers()
            .contains_key("access-control-allow-origin")
    );
    assert!(auth.list_devices().unwrap().is_empty());

    let accepted_invitation = auth
        .mint_pairing(origin, Scope::default_read_only(), now)
        .unwrap();
    let accepted_exchange = PairingExchange::test_fixture(
        accepted_invitation.token,
        "machine-test",
        origin,
        Scope::default_read_only(),
    );
    let accepted_request = || {
        Request::post("/v1/auth/pairing/exchange")
            .header("origin", origin)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&accepted_exchange).unwrap()))
            .unwrap()
    };
    let accepted = app.clone().oneshot(accepted_request()).await.unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_eq!(accepted.headers()["access-control-allow-origin"], origin);
    assert_eq!(accepted.headers()["vary"], "Origin");
    assert_eq!(auth.list_devices().unwrap().len(), 1);

    let replay = app.oneshot(accepted_request()).await.unwrap();
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(replay.headers()["access-control-allow-origin"], origin);
    assert_eq!(replay.headers()["vary"], "Origin");
    assert_eq!(auth.list_devices().unwrap().len(), 1);
}

#[tokio::test]
async fn h2_pair_02_bound_sixth_exchange_is_throttled_without_disclosing_unbound_requests() {
    use chrono::Utc;

    let temp = private_tempdir();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
    let now = Utc::now();
    let origin = "https://controller.example";
    let invitation = auth
        .mint_pairing(origin, Scope::default_read_only(), now)
        .unwrap();
    let mut refused_exchange = PairingExchange::test_fixture(
        invitation.token,
        "machine-test",
        origin,
        Scope::default_read_only(),
    );
    refused_exchange.requested_scopes.insert(Scope::HubAdmin);
    let events = MachineEventBus::new(16);
    let app = router(
        HubState::new(
            SessionCatalog::new(RecordingReadModel::with_sessions(vec![])),
            Arc::new(PreAuthAuthorizer),
            MachineIdentity {
                id: "machine-test".into(),
            },
            DaemonConnector::new(SessionMultiplexer::new(8), events.clone()),
            events,
        )
        .with_auth(auth.clone())
        .with_response_transport(TransportSecurity::TrustedLoopbackTlsProxy),
    );
    let request = |exchange: &PairingExchange| {
        Request::post("/v1/auth/pairing/exchange")
            .header("origin", origin)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(exchange).unwrap()))
            .unwrap()
    };

    for _ in 0..5 {
        let refused = app
            .clone()
            .oneshot(request(&refused_exchange))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(refused.headers()["access-control-allow-origin"], origin);
        assert!(!refused.headers().contains_key("retry-after"));
    }

    let throttled = app
        .clone()
        .oneshot(request(&refused_exchange))
        .await
        .unwrap();
    assert_eq!(throttled.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(throttled.headers()["access-control-allow-origin"], origin);
    assert_eq!(throttled.headers()["vary"], "Origin");
    assert_eq!(
        throttled.headers()["access-control-expose-headers"],
        "Retry-After, X-Cas-Request-Id"
    );
    assert!(
        !throttled
            .headers()
            .contains_key("access-control-allow-credentials")
    );
    let retry_after = throttled.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!((1..=60).contains(&retry_after));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &to_bytes(throttled.into_body(), usize::MAX).await.unwrap()
        )
        .unwrap(),
        serde_json::json!({"error":"slow_down"})
    );
    assert!(auth.list_devices().unwrap().is_empty());

    let unbound_exchange = PairingExchange {
                        installation: None,
        token: "unknown-pairing-capability".into(),
        ..refused_exchange
    };
    let unbound = app.oneshot(request(&unbound_exchange)).await.unwrap();
    assert_eq!(unbound.status(), StatusCode::UNAUTHORIZED);
    assert!(
        !unbound
            .headers()
            .contains_key("access-control-allow-origin")
    );
    assert!(
        !unbound
            .headers()
            .contains_key("access-control-expose-headers")
    );
    assert!(!unbound.headers().contains_key("retry-after"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &to_bytes(unbound.into_body(), usize::MAX).await.unwrap()
        )
        .unwrap(),
        serde_json::json!({"error":"unauthorized"})
    );
}

#[test]
fn h2_pair_02_pairing_throttle_reports_the_remaining_window() {
    use chrono::{Duration, Utc};

    let temp = private_tempdir();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
    let now = Utc::now();
    let origin = "https://controller.example";
    let invitation = auth
        .mint_pairing(origin, Scope::default_read_only(), now)
        .unwrap();
    let mut exchange = PairingExchange::test_fixture(
        invitation.token,
        "machine-test",
        origin,
        Scope::default_read_only(),
    );
    exchange.source = origin.into();
    exchange.requested_scopes.insert(Scope::HubAdmin);

    for offset in [0, 5, 10, 15, 20] {
        assert!(matches!(
            auth.exchange_pairing(exchange.clone(), now + Duration::seconds(offset)),
            Err(PairingExchangeError::Opaque(_))
        ));
    }
    assert!(matches!(
        auth.exchange_pairing(exchange.clone(), now + Duration::seconds(30)),
        Err(PairingExchangeError::Throttled {
            retry_after_seconds: 30
        })
    ));
    assert!(matches!(
        auth.exchange_pairing(exchange, now + Duration::seconds(60)),
        Err(PairingExchangeError::Opaque(_))
    ));
}

#[tokio::test]
async fn h5_machine_identity_advertises_transport_and_untrusted_cloud_suggestions() {
    let events = MachineEventBus::new(16);
    let state = HubState::new(
        SessionCatalog::new(RecordingReadModel::with_sessions(vec![])),
        Arc::new(ExactOriginReadAuthorizer("https://controller.example")),
        MachineIdentity {
            id: "machine-test".into(),
        },
        DaemonConnector::new(SessionMultiplexer::new(8), events.clone()),
        events,
    )
    .with_machine_metadata(MachineMetadata {
        transport: MachineTransport {
            kind: "tailscale_serve".into(),
            public_url: Some("https://target.tail.ts.net/".into()),
        },
        cloud_devices: vec![CloudDeviceSuggestion {
            id: "device-hint".into(),
            name: "Laptop".into(),
            status: Some("online".into()),
            hub_url: Some("https://laptop.tail.ts.net/".into()),
            ssh_host: None,
        }],
    });
    let response = router(state)
        .oneshot(
            Request::get("/v1/machine")
                .header("origin", "https://controller.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["transport"]["kind"], "tailscale_serve");
    assert_eq!(
        body["transport"]["public_url"],
        "https://target.tail.ts.net/"
    );
    assert_eq!(body["cloud_devices"][0]["id"], "device-hint");
    assert!(body["default_supervisor_cli"].as_str().is_some());
    assert!(
        body["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == "cloud_device_suggestions")
    );
    assert!(
        body["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == "machine_multiplex_v2")
    );
}

#[tokio::test]
async fn h4_csp_03_commander_assets_are_self_hosted_and_strictly_sandboxed() {
    let events = MachineEventBus::new(4);
    let state = HubState::new(
        SessionCatalog::new(RecordingReadModel::with_sessions(vec![])),
        Arc::new(PreAuthAuthorizer),
        MachineIdentity {
            id: "machine-test".into(),
        },
        DaemonConnector::new(SessionMultiplexer::new(4), events.clone()),
        events,
    );
    let app = router(state);
    let response = app
        .clone()
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "text/html; charset=utf-8"
    );
    // Owned: the body is consumed below, and the CSP is still checked after.
    let csp = response.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    for required in [
        "default-src 'none'",
        "script-src 'self' 'wasm-unsafe-eval'",
        "style-src 'self'",
        "object-src 'none'",
        "base-uri 'none'",
        "frame-ancestors 'none'",
        "form-action 'none'",
        "worker-src 'none'",
    ] {
        assert!(csp.contains(required), "missing CSP directive {required}");
    }
    assert!(!csp.contains("'unsafe-inline'"));
    assert!(!csp.contains("'unsafe-eval'"));
    assert!(csp.contains("'wasm-unsafe-eval'"));
    assert!(csp.contains("http://127.0.0.1:*"));
    assert!(csp.contains("ws://127.0.0.1:*"));
    assert!(!csp.contains(" http: "));
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let html = std::str::from_utf8(&body).unwrap();
    assert!(html.contains("/commander/app.js"));
    let relay_metadata =
        "name=\"cas-pairing-relay-origin\" content=\"https://petra-stella-cloud.vercel.app\"";
    assert!(html.contains(relay_metadata));
    // cas-9b7d: the cloud operator inbox is the second reviewed origin. Its
    // API is named exactly in connect-src (production, /api/operator/ only).
    let inbox_metadata =
        "name=\"cas-operator-inbox-origin\" content=\"https://petra-stella-cloud.vercel.app\"";
    assert!(html.contains(inbox_metadata));
    assert_eq!(
        csp.matches(super::server::OPERATOR_INBOX_CSP_SOURCE).count(),
        1,
        "the operator inbox API source is pinned once in connect-src: {csp}"
    );
    let connect_src = csp
        .split(';')
        .map(str::trim)
        .find(|directive| directive.starts_with("connect-src "))
        .expect("connect-src directive");
    assert_eq!(
        connect_src,
        "connect-src 'self' https: wss: http://127.0.0.1:* ws://127.0.0.1:* https://petra-stella-cloud.vercel.app/api/operator/",
        "connect-src names exactly the reviewed sources"
    );
    assert!(
        !html
            .replacen(relay_metadata, "", 1)
            .replacen(inbox_metadata, "", 1)
            .contains("https://"),
        "the reviewed pairing relay and operator inbox must be the embedded page's only external origins"
    );
    assert!(!html.contains("<script>"), "inline scripts are forbidden");

    let relay_response = app
        .oneshot(
            Request::post("/api/hub/pairing/requests")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        relay_response.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "the controller hub must not grow a pairing relay or control proxy"
    );
}

#[tokio::test]
async fn h0_tls_hsts_policy_is_bound_to_server_transport_not_client_headers() {
    let state = || {
        let events = MachineEventBus::new(4);
        HubState::new(
            SessionCatalog::new(RecordingReadModel::with_sessions(vec![])),
            Arc::new(PreAuthAuthorizer),
            MachineIdentity {
                id: "machine-test".into(),
            },
            DaemonConnector::new(SessionMultiplexer::new(4), events.clone()),
            events,
        )
    };
    let spoofed_plaintext = router(state())
        .oneshot(
            Request::get("/")
                .header("host", "machine.tail.example")
                .header("forwarded", "proto=https;host=machine.tail.example")
                .header("x-forwarded-proto", "https")
                .header("x-forwarded-host", "machine.tail.example")
                .header("tailscale-user-login", "spoof@example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(spoofed_plaintext.status(), StatusCode::OK);
    assert!(
        !spoofed_plaintext
            .headers()
            .contains_key("strict-transport-security"),
        "client-controlled proxy and identity headers cannot opt plaintext into HSTS"
    );

    let tls_response =
        router(state().with_response_transport(TransportSecurity::TrustedLoopbackTlsProxy))
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
    assert_eq!(tls_response.status(), StatusCode::OK);
    let headers = tls_response.headers();
    assert_eq!(
        headers.get_all("strict-transport-security").iter().count(),
        1
    );
    assert_eq!(headers["strict-transport-security"], "max-age=31536000");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["x-frame-options"], "DENY");
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
}

#[tokio::test]
async fn h1_real_daemon_connector_transforms_welcome_preserves_output_and_one_upstream() {
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
                kind: PaneKind::Supervisor,
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
        pane_bootstrap: vec![PaneBootstrap {
            pane_id: "worker-1".into(),
            epoch: 1_723_456_789_012,
            cols: 120,
            rows: 40,
            scrollback_start_row: 17,
            scrollback_end_row: 817,
        }],
    };
    let output = DaemonMessage::Output {
        pane_id: "worker-1".into(),
        data: b"\x1b[32mlive bytes\x1b[0m".to_vec(),
    };
    let welcome_bytes = serde_json::to_vec(&welcome).unwrap();
    let mut expected_welcome = welcome.clone();
    let DaemonMessage::Welcome {
        scrollback: expected_scrollback,
        ..
    } = &mut expected_welcome
    else {
        unreachable!()
    };
    *expected_scrollback = None;
    let expected_welcome_bytes = serde_json::to_vec(&expected_welcome).unwrap();
    assert!(
        expected_welcome_bytes.len() <= super::connector::COMMANDER_WELCOME_METADATA_HARD_BYTES,
        "canonical Welcome must remain within the metadata hard ceiling"
    );
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
        .attach("factory-a", port, ["worker-1"], None)
        .await
        .unwrap();
    assert_eq!(
        first.recv().await.unwrap().bytes,
        expected_welcome_bytes,
        "v3 Welcome must be transformed exactly to metadata-only form"
    );

    let mut second = connector
        .attach("factory-a", port, ["worker-1"], None)
        .await
        .unwrap();
    assert_eq!(
        second.recv().await.unwrap().bytes,
        expected_welcome_bytes,
        "late viewers rehydrate from the same deterministic canonical Welcome"
    );

    release_output.notify_waiters();
    assert_eq!(first.recv().await.unwrap().bytes, output_bytes);
    assert_eq!(second.recv().await.unwrap().bytes, output_bytes);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(25), first.recv())
            .await
            .is_err(),
        "PTY output must be delivered exactly once"
    );
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

#[tokio::test]
async fn commander_attention_event_is_immediate_then_durably_patched_in_place() {
    let temp = private_tempdir();
    let path = temp.path().join("events.json");
    let events = MachineEventBus::open(16, &path).unwrap();
    let mut broadcast = events.subscribe();
    let mut enrichment = events.enable_enrichment();
    events.set_session_context(
        "factory-a",
        SessionAttentionContext {
            title: "Refactor authentication".into(),
            phase: "testing".into(),
        },
    );

    events.observe_daemon(
        "factory-a",
        &DaemonMessage::Error {
            message: "serde panic in auth.rs:44".into(),
            client_ref: None,
        },
    );
    let immediate = broadcast.recv().await.unwrap();
    assert_eq!(immediate.kind, MachineEventKind::DaemonError);
    assert!(immediate.enrichment_pending);
    assert!(immediate.enrichment.is_none());
    assert_eq!(immediate.session_context.as_ref().unwrap().phase, "testing");
    assert_eq!(
        enrichment.recv().await.unwrap().sequence,
        immediate.sequence
    );

    events.finish_enrichment(
        immediate.sequence,
        Some(AttentionEnrichment {
            severity: AttentionSeverity::Critical,
            summary: "Authentication worker crashed".into(),
            detail: Some("auth.rs:44 serde panic".into()),
            action: AttentionAction::Retry,
            fingerprint: "auth.rs-serde-panic".into(),
        }),
    );
    let patch = broadcast.recv().await.unwrap();
    assert_eq!(patch.sequence, immediate.sequence);
    assert_eq!(patch.revision, 1);
    assert!(!patch.enrichment_pending);
    assert_eq!(
        patch.enrichment.as_ref().unwrap().fingerprint,
        "auth.rs-serde-panic"
    );

    drop(events);
    let reopened = MachineEventBus::open(16, &path).unwrap();
    assert_eq!(reopened.history(), vec![patch]);
}

#[tokio::test]
async fn commander_attention_api_off_is_complete_without_pending_state() {
    let events = MachineEventBus::new(4);
    let mut broadcast = events.subscribe();
    events.observe_daemon(
        "factory-a",
        &DaemonMessage::Error {
            message: "raw error remains actionable".into(),
            client_ref: None,
        },
    );

    let event = broadcast.recv().await.unwrap();
    assert_eq!(event.kind, MachineEventKind::DaemonError);
    assert!(!event.enrichment_pending);
    assert!(event.enrichment.is_none());
    assert_eq!(
        event.payload.unwrap()["message"],
        "raw error remains actionable"
    );
}

#[test]
fn h1_runtime_state_is_single_instance_and_round_trips() {
    let temp = private_tempdir();
    let paths = HubRuntimePaths::new(temp.path().join("hub"));
    let first_lock = paths.acquire_instance_lock().unwrap();
    assert!(paths.acquire_instance_lock().is_err());
    let owner = paths.read_lock_owner().unwrap();
    assert_eq!(owner.pid, std::process::id());
    assert_eq!(owner.phase, "starting");

    let record = HubProcessRecord {
        pid: std::process::id(),
        sid: None,
        pgid: None,
        bind: "127.0.0.1".into(),
        port: 4173,
        version: env!("CARGO_PKG_VERSION").into(),
        started_at: "2026-08-09T00:00:00Z".into(),
        cgroup: None,
        launched_by: None,
        launched_at: None,
        public_url: None,
        tailscale_serve_port: None,
        tailscale_cli: None,
        tailscale_serve_target: None,
        transport_warning: None,
    };
    paths.write_process_record(&record).unwrap();
    assert_eq!(paths.read_process_record().unwrap(), record);

    drop(first_lock);
    assert!(paths.read_lock_owner().is_none());
    assert!(paths.acquire_instance_lock().is_ok());
}

#[test]
fn h2_pair_02_pairing_is_bound_persistent_single_use_and_fragment_only() {
    use chrono::{Duration, Utc};

    let temp = private_tempdir();
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

#[test]
fn h2_pair_03_invitation_url_declares_the_scope_ceiling_it_minted() {
    use chrono::Utc;

    let temp = private_tempdir();
    let now = Utc::now();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();

    // Commander cannot request a scope this invitation does not grant unless the
    // invitation says what it granted; without it the form guessed all six and
    // every default `cas hub pair` failed its first exchange with a bare 401.
    let read_only = auth
        .mint_pairing(
            "https://controller.example",
            Scope::default_read_only(),
            now,
        )
        .unwrap();
    assert!(
        read_only
            .url
            .ends_with("&scopes=machine-read,session-read,pane-read"),
        "invitation url must declare its ceiling: {}",
        read_only.url
    );
    assert!(read_only.url.contains("#pair="));
    assert!(!read_only.url.contains('?'));

    let control = auth
        .mint_pairing(
            "https://controller.example",
            [
                Scope::MachineRead,
                Scope::SessionRead,
                Scope::PaneRead,
                Scope::PaneInput,
                Scope::MessageSend,
                Scope::PaneInterrupt,
            ]
            .into_iter()
            .collect(),
            now,
        )
        .unwrap();
    assert!(
        control.url.ends_with(
            "&scopes=machine-read,session-read,pane-read,pane-input,message-send,pane-interrupt"
        ),
        "control invitation url must declare its ceiling: {}",
        control.url
    );

    // The declared ceiling is exactly what the exchange enforces.
    let declared = control
        .url
        .rsplit_once("&scopes=")
        .map(|(_, scopes)| scopes.to_string())
        .unwrap();
    let parsed: std::collections::BTreeSet<Scope> = declared
        .split(',')
        .map(|scope| Scope::parse(scope).unwrap())
        .collect();
    assert_eq!(parsed, control.scopes);
}

#[test]
fn h2_pair_04_invitation_url_prefills_hub_address_and_machine_name() {
    use chrono::Utc;

    let temp = private_tempdir();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
    let invitation = auth
        .mint_pairing(
            "https://controller.example",
            Scope::default_read_only(),
            Utc::now(),
        )
        .unwrap()
        .with_prefill(PairingPrefill {
            hub_url: Some("https://studio.tail.ts.net".into()),
            machine_label: Some("Studio Mac & co".into()),
        });
    let (_, fragment) = invitation.url.split_once('#').unwrap();
    assert!(
        fragment.starts_with(&format!("pair={}&hub=machine-test&", invitation.token)),
        "{}",
        invitation.url
    );
    assert!(
        fragment
            .contains("&hub_url=https%3A%2F%2Fstudio.tail.ts.net&machine=Studio%20Mac%20%26%20co&"),
        "prefill must be percent-encoded so it cannot inject parameters: {}",
        invitation.url
    );
    // `scopes` stays last, so a reader that takes everything after it still works.
    assert!(
        invitation
            .url
            .ends_with("&scopes=machine-read,session-read,pane-read"),
        "{}",
        invitation.url
    );
    // The hosted relay delivers the address and name itself; its URL is unchanged.
    let relay = invitation.url_for(PairingInvitationTarget::HostedRelay);
    assert_eq!(
        relay,
        format!(
            "https://controller.example/#pair={}&hub=machine-test",
            invitation.token
        )
    );

    // Blank values are omitted rather than printed as empty parameters.
    let blank = auth
        .mint_pairing(
            "https://controller.example",
            Scope::default_read_only(),
            Utc::now(),
        )
        .unwrap()
        .with_prefill(PairingPrefill {
            hub_url: None,
            machine_label: Some("  ".into()),
        });
    assert!(!blank.url.contains("hub_url="), "{}", blank.url);
    assert!(!blank.url.contains("machine="), "{}", blank.url);
}

#[tokio::test]
async fn h2_ws_04_ticket_is_five_minute_bound_single_use_under_race() {
    use chrono::{Duration, Utc};

    let temp = private_tempdir();
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

    let temp = private_tempdir();
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

    let temp = private_tempdir();
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

    let temp = private_tempdir();
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
            pane_bootstrap: Vec::new(),
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
    let app = router(state);
    let pairing_response = app
        .clone()
        .oneshot(
            Request::post("/v1/auth/pairing/exchange")
                .header("origin", "https://controller.example")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&exchange).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pairing_response.status(), StatusCode::OK);
    let credential: serde_json::Value = serde_json::from_slice(
        &to_bytes(pairing_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let credential_secret = credential["credential"].as_str().unwrap();
    let device_id = credential["device_id"].as_str().unwrap().to_owned();
    let proof = sign_dpop(
        &signing,
        credential_secret,
        "GET",
        "/v1/bootstrap",
        now,
        "running-hub-context",
    );
    let context = running_hub_auth
        .authenticate_dpop(
            &format!("DPoP {credential_secret}"),
            &proof,
            "https://controller.example",
            "GET",
            "/v1/bootstrap",
            now,
        )
        .unwrap();

    let hub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hub_address = hub_listener.local_addr().unwrap();
    let hub = tokio::spawn(async move {
        axum::serve(hub_listener, app).await.unwrap();
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

    cli_auth.revoke_device(&device_id, Utc::now()).unwrap();
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

    let temp = private_tempdir();
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
        client_ref: None,
        in_reply_to: None,
        attribution: MessageAttribution {
            device_id: None,
            credential_id: None,
            device_label: None,
            operator_label: None,
            controller_origin: None,
            request_id: None,
            scopes: Vec::new(),
            operator_verified: false,
        },
    };
    let resize = ClientMessage::ResizePane {
        pane_id: "worker-1".into(),
        cols: 80,
        rows: 24,
    };
    let keyframe = ClientMessage::RequestPaneKeyframe {
        pane_id: "worker-1".into(),
    };
    let scrollback = ClientMessage::ScrollbackRequest {
        pane_id: "worker-1".into(),
        generation: 42,
        start_row: 0,
        count: 200,
    };
    let history = ClientMessage::ConversationHistoryRequest {
        request_id: "history-1".into(),
        before: None,
        limit: 50,
        device_id: "phone-7".into(),
    };

    assert_eq!(required_scope(&input), Some(Scope::PaneInput));
    assert_eq!(required_scope(&resize), Some(Scope::PaneRead));
    assert_eq!(required_scope(&keyframe), Some(Scope::PaneRead));
    assert_eq!(required_scope(&scrollback), Some(Scope::PaneRead));
    assert_eq!(required_scope(&history), Some(Scope::PaneRead));
    assert!(super::server::is_pane_read_message(&keyframe));
    assert!(super::server::is_pane_read_message(&scrollback));
    assert!(super::server::is_pane_read_message(&history));
    assert!(
        !super::server::is_pane_read_message(&resize),
        "ResizePane retains may_resize_panes lease policy"
    );
    assert!(
        !super::server::is_pane_read_message(&input),
        "input remains a leased mutation"
    );
    assert!(
        !super::server::is_pane_read_message(&semantic),
        "SendMessage remains a leased mutation"
    );
    assert_eq!(required_scope(&targeted), Some(Scope::PaneInterrupt));
    assert_eq!(required_scope(&semantic), Some(Scope::MessageSend));
    assert_eq!(required_scope(&ClientMessage::Interrupt), None);
}

/// cas-a8ea8: the operator's `in_reply_to` survives the hub. handle_client_message
/// re-parses the frame into `ClientMessage`, rebuilds only the attribution, and
/// forwards the value through `connector.send` — so the reference the browser
/// put on the frame is what the daemon binds the reply to. A legacy frame
/// without it still decodes and forwards without the key.
#[test]
fn hub_forwards_the_operator_in_reply_to_reference_unchanged() {
    use std::collections::BTreeSet;

    let scopes: BTreeSet<Scope> = [Scope::MessageSend].into_iter().collect();
    let context = AuthContext {
        device_id: "device-real".into(),
        credential_id: "credential-real".into(),
        device_label: "Daniel's phone".into(),
        operator_label: "Daniel".into(),
        controller_origin: "https://controller.example".into(),
        scopes,
        request_id: "request-1".into(),
    };
    // The exact frame hub-web's supervisorMessage() emits for a quick reply.
    let frame = serde_json::json!({
        "SendMessage": {
            "client_ref": "send-43",
            "in_reply_to": 52,
            "target": "patient-pelican-9",
            "text": "Yes, go ahead",
            "summary": "Cassy Cloud message",
            "urgent": false,
            "attribution": {
                "device_id": null, "credential_id": null, "device_label": null,
                "operator_label": null, "controller_origin": null, "request_id": null
            }
        }
    });
    let mut message: ClientMessage = serde_json::from_value(frame).unwrap();
    assert_eq!(required_scope(&message), Some(Scope::MessageSend));
    let ClientMessage::SendMessage { attribution, .. } = &mut message else {
        panic!("fixture is a SendMessage");
    };
    *attribution = super::server::verified_attribution(&context);
    let ClientMessage::SendMessage { in_reply_to, client_ref, attribution, .. } = &message else {
        unreachable!()
    };
    assert_eq!(*in_reply_to, Some(52));
    assert_eq!(client_ref.as_deref(), Some("send-43"));
    assert!(attribution.operator_verified);
    let forwarded = serde_json::to_value(&message).unwrap();
    assert_eq!(forwarded["SendMessage"]["in_reply_to"], serde_json::json!(52));
    assert_eq!(forwarded["SendMessage"]["client_ref"], serde_json::json!("send-43"));

    let legacy = serde_json::json!({
        "SendMessage": {
            "target": "patient-pelican-9", "text": "Plain", "summary": null, "urgent": false,
            "attribution": {
                "device_id": null, "credential_id": null, "device_label": null,
                "operator_label": null, "controller_origin": null, "request_id": null
            }
        }
    });
    let legacy: ClientMessage = serde_json::from_value(legacy).unwrap();
    assert!(matches!(legacy, ClientMessage::SendMessage { in_reply_to: None, .. }));
    assert!(serde_json::to_value(&legacy).unwrap()["SendMessage"].get("in_reply_to").is_none());
}

#[test]
fn hub_history_response_is_consumed_by_only_the_requesting_socket() {
    let mut pending = std::collections::HashSet::from([("factory-1".into(), "history-1".into())]);
    let frame = serde_json::to_vec(&DaemonMessage::ConversationHistory {
        request_id: "history-1".into(),
        messages: Vec::new(),
        replies: Vec::new(),
        has_earlier: false,
        next_before: None,
        earlier_messages: Vec::new(),
        earlier_replies: Vec::new(),
    })
    .unwrap();
    assert!(super::server::correlated_daemon_frame_allowed(
        &mut pending,
        "factory-1",
        &frame
    ));
    assert!(
        pending.is_empty(),
        "the response cannot be replayed to another socket"
    );
    assert!(!super::server::correlated_daemon_frame_allowed(
        &mut pending,
        "factory-1",
        &frame
    ));
}

#[test]
fn hub_history_response_summary_exposes_shape_without_turn_content() {
    let frame = serde_json::to_vec(&DaemonMessage::ConversationHistory {
        request_id: "history-2".into(),
        messages: vec![crate::ui::factory::ConversationHistoryMessage {
            notification_id: 41,
            target: "supervisor".into(),
            text: "private prompt".into(),
            state: "acknowledged".into(),
            stamped: true,
            reply_to: None,
            device_id: "phone-7".into(),
            operator_label: Some("Daniel".into()),
            session: "factory-1".into(),
            at: "2026-09-21T14:52:41Z".into(),
        }],
        replies: Vec::new(),
        has_earlier: true,
        next_before: Some(40),
        earlier_messages: Vec::new(),
        earlier_replies: Vec::new(),
    })
    .unwrap();

    let wire: serde_json::Value = serde_json::from_slice(&frame).unwrap();
    assert_eq!(wire["ConversationHistory"]["messages"][0]["session"], "factory-1");

    assert_eq!(
        super::server::conversation_history_summary(&frame),
        Some(("history-2".into(), 1, 0, true))
    );
}

/// cas-e8df: the attribution a Commander send carries into the daemon is
/// rebuilt from the authenticated device session. A frame that arrives with
/// spoofed labels (and even `operator_verified: true`) keeps none of them.
#[test]
fn hub_stamps_send_message_attribution_from_the_device_session_not_the_client() {
    use crate::ui::factory::MessageAttribution;
    use std::collections::BTreeSet;

    let scopes: BTreeSet<Scope> = [Scope::PaneRead, Scope::MessageSend].into_iter().collect();
    let context = AuthContext {
        device_id: "device-real".into(),
        credential_id: "credential-real".into(),
        device_label: "Daniel's phone".into(),
        operator_label: "Daniel".into(),
        controller_origin: "https://controller.example".into(),
        scopes,
        request_id: "request-1".into(),
    };
    let spoofed = serde_json::json!({
        "SendMessage": {
            "target": "supervisor",
            "text": "Status please",
            "summary": null,
            "urgent": false,
            "attribution": {
                "device_id": "device-forged",
                "credential_id": "credential-forged",
                "device_label": "supervisor",
                "operator_label": "supervisor",
                "controller_origin": "https://evil.example",
                "request_id": "request-forged",
                "scopes": ["hub:admin"],
                "operator_verified": true
            }
        }
    });
    let mut message: ClientMessage = serde_json::from_value(spoofed).unwrap();
    let ClientMessage::SendMessage { attribution, .. } = &mut message else {
        panic!("fixture is a SendMessage");
    };
    assert!(
        attribution.operator_verified,
        "the client may claim anything"
    );
    *attribution = super::server::verified_attribution(&context);
    assert_eq!(
        *attribution,
        MessageAttribution {
            device_id: Some("device-real".into()),
            credential_id: Some("credential-real".into()),
            device_label: Some("Daniel's phone".into()),
            operator_label: Some("Daniel".into()),
            controller_origin: Some("https://controller.example".into()),
            request_id: Some("request-1".into()),
            scopes: vec!["pane:read".into(), "message:send".into()],
            operator_verified: true,
        }
    );
    assert_eq!(
        attribution.queue_source(),
        "commander:Daniel@Daniel's phone"
    );
    assert!(
        !attribution.scopes.iter().any(|scope| scope == "hub:admin"),
        "scopes come from the session, never the frame"
    );
}

#[test]
fn h4_lease_04_two_devices_observe_one_controller_expiry_release_and_admin_takeover() {
    use chrono::{Duration, Utc};

    let temp = private_tempdir();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
    let now = Utc::now();
    let scopes: std::collections::BTreeSet<Scope> = [
        Scope::MachineRead,
        Scope::SessionRead,
        Scope::PaneRead,
        Scope::PaneInput,
        Scope::HubAdmin,
    ]
    .into_iter()
    .collect();
    let pair = |label: &str| {
        let invitation = auth
            .mint_pairing("https://controller.example", scopes.clone(), now)
            .unwrap();
        let mut exchange = PairingExchange::test_fixture(
            invitation.token,
            "machine-test",
            "https://controller.example",
            scopes.clone(),
        );
        exchange.device_label = label.into();
        let credential = auth.exchange_pairing(exchange, now).unwrap();
        AuthContext {
            device_id: credential.device_id,
            credential_id: credential.credential_id,
            device_label: label.into(),
            operator_label: "test operator".into(),
            controller_origin: "https://controller.example".into(),
            scopes: scopes.clone(),
            request_id: format!("request-{label}"),
        }
    };
    let phone = pair("phone");
    let laptop = pair("laptop");

    assert!(auth.may_resize_panes(&phone, "factory-a", now).unwrap());
    assert!(auth.may_resize_panes(&laptop, "factory-a", now).unwrap());
    auth.acquire_lease(&phone, "factory-a", now).unwrap();
    assert!(auth.may_resize_panes(&phone, "factory-a", now).unwrap());
    assert!(!auth.may_resize_panes(&laptop, "factory-a", now).unwrap());
    assert!(
        auth.lease_status(&phone, "factory-a", now)
            .unwrap()
            .held_by_me
    );
    let observed = auth.lease_status(&laptop, "factory-a", now).unwrap();
    assert_eq!(observed.controller_label.as_deref(), Some("phone"));
    assert!(!observed.held_by_me);
    assert!(auth.acquire_lease(&laptop, "factory-a", now).is_err());

    auth.acquire_or_force_lease(&laptop, "factory-a", now, true)
        .unwrap();
    assert_eq!(
        auth.lease_status(&phone, "factory-a", now)
            .unwrap()
            .controller_label
            .as_deref(),
        Some("laptop")
    );
    auth.release_lease(&laptop, "factory-a", now).unwrap();
    assert!(auth.may_resize_panes(&phone, "factory-a", now).unwrap());
    assert!(
        auth.lease_status(&phone, "factory-a", now)
            .unwrap()
            .controller_device_id
            .is_none()
    );

    auth.acquire_lease(&phone, "factory-a", now).unwrap();
    assert!(
        auth.lease_status(&laptop, "factory-a", now + Duration::seconds(31))
            .unwrap()
            .controller_device_id
            .is_none(),
        "expired leases stop enabling every viewer"
    );
}

#[test]
fn h2_perm_01_rejects_loose_or_symlinked_machine_auth_state() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let temp = private_tempdir();
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

    let temp = private_tempdir();
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

#[test]
fn expired_device_credential_refreshes_once_but_revoked_never_does() {
    use chrono::{Duration, Utc};
    use p256::ecdsa::SigningKey;
    use p256::elliptic_curve::rand_core::OsRng;

    let temp = private_tempdir();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
    let issued = Utc::now();
    let signing = SigningKey::random(&mut OsRng);
    let invitation = auth
        .mint_pairing(
            "https://controller.example",
            Scope::default_read_only(),
            issued,
        )
        .unwrap();
    let mut exchange = PairingExchange::test_fixture(
        invitation.token,
        "machine-test",
        "https://controller.example",
        Scope::default_read_only(),
    );
    exchange.public_key_jwk = public_jwk(&signing);
    let credential = auth.exchange_pairing(exchange, issued).unwrap();

    // Keep the credential active through its ordinary 30-day idle windows.
    for day in [29, 58, 87] {
        let active_at = issued + Duration::days(day);
        let active_proof = sign_dpop(
            &signing,
            &credential.credential,
            "GET",
            "/v1/machine",
            active_at,
            &format!("active-before-expiry-{day}"),
        );
        auth.authenticate_dpop(
            &format!("DPoP {}", credential.credential),
            &active_proof,
            "https://controller.example",
            "GET",
            "/v1/machine",
            active_at,
        )
        .unwrap();
    }

    let expired_at = issued + Duration::days(91);
    let refresh_proof = sign_dpop(
        &signing,
        &credential.credential,
        "POST",
        "/v1/auth/refresh",
        expired_at,
        "refresh-expired",
    );
    let refreshed = auth
        .refresh_device_credential(
            &format!("DPoP {}", credential.credential),
            &refresh_proof,
            "https://controller.example",
            "POST",
            "/v1/auth/refresh",
            expired_at,
        )
        .unwrap();
    assert_ne!(refreshed.credential, credential.credential);
    assert_eq!(refreshed.expires_at, expired_at + Duration::days(90));

    auth.revoke_device(&refreshed.device_id, expired_at)
        .unwrap();
    let revoked_proof = sign_dpop(
        &signing,
        &refreshed.credential,
        "POST",
        "/v1/auth/refresh",
        expired_at,
        "refresh-revoked",
    );
    assert!(
        auth.refresh_device_credential(
            &format!("DPoP {}", refreshed.credential),
            &revoked_proof,
            "https://controller.example",
            "POST",
            "/v1/auth/refresh",
            expired_at,
        )
        .is_err()
    );
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

#[test]
fn h2_self_grant_launch_requires_full_control_is_idempotent_and_audited() {
    use chrono::Utc;
    let temp = private_tempdir();
    let root = temp.path().join("hub");
    let auth = AuthStore::open(&root, "machine-test").unwrap();
    let now = Utc::now();
    let full = [
        Scope::MachineRead,
        Scope::SessionRead,
        Scope::PaneRead,
        Scope::PaneInput,
        Scope::MessageSend,
        Scope::PaneInterrupt,
    ]
    .into_iter()
    .collect();
    let (credential, context) = paired_context(&auth, now, full);
    let before = std::fs::read_to_string(root.join(AUDIT_LOG_FILE)).unwrap();
    let scopes = auth.grant_own_session_launch(&context, now).unwrap();
    assert!(scopes.contains(&Scope::SessionLaunch));
    assert!(!scopes.contains(&Scope::FactoryManage));
    assert!(!scopes.contains(&Scope::HubAdmin));
    assert_eq!(
        auth.list_devices()
            .unwrap()
            .iter()
            .find(|device| device.device_id == credential.device_id)
            .unwrap()
            .scopes,
        scopes
    );
    assert_eq!(
        auth.grant_own_session_launch(&context, now).unwrap(),
        scopes
    );
    let audit = std::fs::read_to_string(root.join(AUDIT_LOG_FILE)).unwrap();
    assert_eq!(audit.matches("self_grant_session_launch").count(), 1);
    assert_eq!(audit.lines().count(), before.lines().count() + 1);
    assert!(audit.contains("https://controller.example"));
    let (_read_credential, read_context) = paired_context(&auth, now, Scope::default_read_only());
    assert_eq!(
        auth.grant_own_session_launch(&read_context, now)
            .unwrap_err()
            .to_string(),
        "scope denied"
    );
    assert!(
        !auth
            .list_devices()
            .unwrap()
            .iter()
            .find(|device| device.device_id == read_context.device_id)
            .unwrap()
            .scopes
            .contains(&Scope::SessionLaunch)
    );
}

#[tokio::test]
async fn h2_self_grant_http_rejects_other_scopes_and_read_only_devices() {
    use chrono::Utc;
    use p256::ecdsa::SigningKey;
    use p256::elliptic_curve::rand_core::OsRng;

    let temp = private_tempdir();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
    let now = Utc::now();
    let events = MachineEventBus::new(16);
    let app = router(
        HubState::new(
            SessionCatalog::new(RecordingReadModel::with_sessions(vec![])),
            Arc::new(PreAuthAuthorizer),
            MachineIdentity {
                id: "machine-test".into(),
            },
            DaemonConnector::new(SessionMultiplexer::new(8), events.clone()),
            events,
        )
        .with_auth(auth.clone())
        .with_effective_origin("https://controller.example"),
    );
    for scopes in [
        Scope::default_read_only(),
        [
            Scope::MachineRead,
            Scope::SessionRead,
            Scope::PaneRead,
            Scope::PaneInput,
            Scope::MessageSend,
            Scope::PaneInterrupt,
        ]
        .into_iter()
        .collect(),
    ] {
        let signing = SigningKey::random(&mut OsRng);
        let invitation = auth
            .mint_pairing("https://controller.example", scopes.clone(), now)
            .unwrap();
        let mut exchange = PairingExchange::test_fixture(
            invitation.token,
            "machine-test",
            "https://controller.example",
            scopes.clone(),
        );
        exchange.public_key_jwk = public_jwk(&signing);
        let credential = auth.exchange_pairing(exchange, now).unwrap();
        let send = |body: &'static str| {
            let proof = sign_dpop(
                &signing,
                &credential.credential,
                "POST",
                "/v1/auth/scopes",
                now,
                &uuid::Uuid::new_v4().to_string(),
            );
            Request::post("/v1/auth/scopes")
                .header("origin", "https://controller.example")
                .header("authorization", format!("DPoP {}", credential.credential))
                .header("dpop", proof)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap()
        };
        for body in [
            r#"{"add":["factory-manage"]}"#,
            r#"{"add":["session-launch","hub-admin"]}"#,
        ] {
            assert_eq!(
                app.clone().oneshot(send(body)).await.unwrap().status(),
                StatusCode::BAD_REQUEST
            );
        }
        let response = app
            .clone()
            .oneshot(send(r#"{"add":["session-launch"]}"#))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if scopes.contains(&Scope::PaneInput) {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
    }
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

/// `GET /v1/sessions` reported "0 workers" for a session running five, because
/// the read model trusted the session file's `workers[]` — which the factory
/// leaves empty — instead of the live roster the TUI already reads.
#[test]
fn h5_session_worker_roster_comes_from_the_live_registry_not_the_session_file() {
    use crate::store::{AgentStore, SqliteAgentStore, init_cas_dir};
    use crate::ui::factory::{SessionInfo, create_metadata};
    use cas_types::{AgentRole, AgentStatus, AgentType};

    let mut env = crate::test_support::TestEnvGuard::temp_home();
    let project = tempfile::tempdir().unwrap();
    let cas_root = init_cas_dir(project.path()).unwrap();
    let session_name = "hub-roster-session";
    // A hub serves every project on the machine at once, and the process that
    // launched it carries one project's CAS_ROOT (the live hub on this machine
    // runs with cas-src's). That override must not decide which registry a
    // gabber-studio or violet_ps session's roster is read from.
    let unrelated = tempfile::tempdir().unwrap();
    let unrelated_root = init_cas_dir(unrelated.path()).unwrap();
    env.set("CAS_ROOT", &unrelated_root);
    let session = SessionInfo {
        name: session_name.to_string(),
        // Exactly what every session file on a live machine carries: no workers.
        metadata: create_metadata(
            session_name,
            std::process::id(),
            "supervisor-agent",
            &[],
            Some("cas-5d94"),
            Some(project.path().to_str().unwrap()),
            Some(4173),
        ),
        is_running: true,
        socket_exists: true,
    };

    let agents = SqliteAgentStore::open(&cas_root).unwrap();
    agents.init().unwrap();
    let mut supervisor =
        cas_types::Agent::new("roster-supervisor-id".into(), "supervisor-agent".into());
    supervisor.role = AgentRole::Supervisor;
    supervisor.factory_session = Some(session_name.to_string());
    agents.register(&supervisor).unwrap();
    for index in 0..5 {
        let mut worker =
            cas_types::Agent::new(format!("roster-worker-{index}"), format!("worker-{index}"));
        worker.agent_type = AgentType::Worker;
        worker.role = AgentRole::Worker;
        worker.factory_session = Some(session_name.to_string());
        agents.register(&worker).unwrap();
    }

    let mapped = hub_session(&session);
    assert_eq!(
        mapped.workers.len(),
        5,
        "five live workers must be reported"
    );
    assert_eq!(mapped.supervisor, "supervisor-agent");
    assert!(
        !mapped.dormant,
        "a fresh registered supervisor keeps the row live"
    );
    assert_eq!(mapped.epic_id.as_deref(), Some("cas-5d94"));
    assert_eq!(mapped.liveness, DaemonLiveness::Live);

    let mut dead_supervisor = agents.get("roster-supervisor-id").unwrap();
    dead_supervisor.status = AgentStatus::Stale;
    dead_supervisor.last_heartbeat = chrono::Utc::now() - chrono::Duration::seconds(31);
    agents.update(&dead_supervisor).unwrap();
    assert!(
        hub_session(&session).dormant,
        "a stale supervisor registry row must not keep the session visible"
    );

    // A process that still exists cannot keep a nonresponsive Hub conversation live.
    dead_supervisor.status = AgentStatus::Active;
    dead_supervisor.pid = Some(std::process::id());
    agents.update(&dead_supervisor).unwrap();
    assert!(
        hub_session(&session).dormant,
        "an old heartbeat must not be rescued by a surviving process"
    );

    // The roster carries agent names (what Commander shows), not agent ids.
    let mut names = mapped.workers.clone();
    names.sort();
    assert_eq!(
        names,
        vec!["worker-0", "worker-1", "worker-2", "worker-3", "worker-4"]
    );

    // A worker that has shut down or gone silent is not part of the roster.
    let mut shutdown = agents.get("roster-worker-0").unwrap();
    shutdown.status = AgentStatus::Shutdown;
    agents.update(&shutdown).unwrap();
    let mut stale = agents.get("roster-worker-1").unwrap();
    stale.last_heartbeat = chrono::Utc::now() - chrono::Duration::seconds(31);
    agents.update(&stale).unwrap();
    assert_eq!(hub_session(&session).workers.len(), 3);

    // A session that genuinely runs no workers still reports zero, not a guess.
    let empty_project = tempfile::tempdir().unwrap();
    init_cas_dir(empty_project.path()).unwrap();
    let empty = SessionInfo {
        name: "hub-roster-empty".to_string(),
        metadata: create_metadata(
            "hub-roster-empty",
            std::process::id(),
            "supervisor-agent",
            &[],
            None,
            Some(empty_project.path().to_str().unwrap()),
            Some(4174),
        ),
        is_running: true,
        socket_exists: true,
    };
    assert!(hub_session(&empty).workers.is_empty());
    assert!(
        hub_session(&empty).dormant,
        "no registered supervisor is dormant"
    );

    // With no registry to read, the daemon roster is still better than nothing.
    let unreachable = SessionInfo {
        name: "hub-roster-fallback".to_string(),
        metadata: create_metadata(
            "hub-roster-fallback",
            std::process::id(),
            "supervisor-agent",
            &["fallback-worker".to_string()],
            None,
            None,
            Some(4175),
        ),
        is_running: true,
        socket_exists: true,
    };
    assert_eq!(hub_session(&unreachable).workers, vec!["fallback-worker"]);
    assert!(
        hub_session(&unreachable).dormant,
        "without a registry, supervisor liveness is unproven"
    );
}

// cas-37f8: a phone-sized viewer must never shrink the operator's dashboard.
// The daemon answers a refused ResizePane with the authoritative geometry; the
// hub turns that reply into an audit record for the device that asked.

#[test]
fn a_local_dashboard_authority_reply_is_recognised_as_a_refused_resize() {
    let frame = serde_json::to_vec(&DaemonMessage::PaneSize {
        pane_id: "worker-1".into(),
        cols: 203,
        rows: 44,
        authority: crate::ui::factory::PaneSizeAuthority::LocalDashboard,
    })
    .expect("PaneSize frame");

    assert_eq!(
        super::server::refused_pane_resize(&frame),
        Some(("worker-1".to_owned(), 203, 44))
    );
}

#[test]
fn a_viewer_authority_reply_is_not_a_refusal() {
    let frame = serde_json::to_vec(&DaemonMessage::PaneSize {
        pane_id: "worker-1".into(),
        cols: 46,
        rows: 33,
        authority: crate::ui::factory::PaneSizeAuthority::Viewer,
    })
    .expect("PaneSize frame");

    assert_eq!(super::server::refused_pane_resize(&frame), None);
}

#[test]
fn ordinary_relay_traffic_is_never_parsed_as_a_resize_refusal() {
    let output = serde_json::to_vec(&DaemonMessage::Output {
        pane_id: "worker-1".into(),
        data: b"\x1b[2J{\"PaneSize\"".to_vec(),
    })
    .expect("Output frame");

    assert_eq!(super::server::refused_pane_resize(&output), None);
    assert_eq!(super::server::refused_pane_resize(b""), None);
    assert_eq!(super::server::refused_pane_resize(b"not json"), None);
}

/// cas-7103: the catalog lists every live supervisor-led session, even before
/// it has spawned workers; worker-only rows still require an explicit switch.
#[tokio::test]
async fn sessions_catalog_lists_live_supervisors_with_empty_rosters() {
    let mut empty_supervisor = fixture_session("empty-supervisor");
    empty_supervisor.workers.clear();
    let mut bare = fixture_session("bare-shell");
    bare.supervisor = String::new();
    bare.workers = vec!["worker-9".into()];
    let mut hung_empty = fixture_session("hung-empty");
    hung_empty.supervisor = String::new();
    hung_empty.workers.clear();
    hung_empty.liveness = DaemonLiveness::MissingEndpoint;
    let mut dormant = fixture_session("orphaned-supervisor");
    dormant.dormant = true;
    dormant.workers.clear();
    let source = RecordingReadModel::with_sessions(vec![
        fixture_session("factory-a"),
        empty_supervisor,
        bare,
        hung_empty,
        dormant,
    ]);
    let events = MachineEventBus::new(16);
    let state = HubState::new(
        SessionCatalog::new(source),
        Arc::new(ExactOriginReadAuthorizer("http://127.0.0.1:4173")),
        MachineIdentity {
            id: "machine-test".into(),
        },
        DaemonConnector::new(SessionMultiplexer::new(8), events.clone()),
        events,
    );
    let app = router(state);

    let names = |body: serde_json::Value| -> Vec<String> {
        body["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|session| session["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let fetch = |uri: &'static str| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    Request::get(uri)
                        .header("origin", "http://127.0.0.1:4173")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            serde_json::from_slice::<serde_json::Value>(
                &to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            )
            .unwrap()
        }
    };
    let default_catalog = fetch("/v1/sessions").await;
    assert_eq!(
        default_catalog["freshness_threshold_secs"],
        crate::mcp::tools::service::agent_liveness::WORKER_STALE_SECS
    );
    assert_eq!(
        names(default_catalog),
        vec!["factory-a", "empty-supervisor"]
    );
    assert_eq!(
        names(fetch("/v1/sessions?workers=0").await),
        vec!["factory-a", "empty-supervisor"]
    );
    assert_eq!(
        names(fetch("/v1/sessions?workers=1").await),
        vec!["factory-a", "empty-supervisor", "bare-shell"]
    );
    assert_eq!(
        names(fetch("/v1/sessions?dormant=1").await),
        vec!["factory-a", "empty-supervisor", "orphaned-supervisor"]
    );
    assert_eq!(
        names(fetch("/v1/sessions?workers=1&dormant=1").await),
        vec![
            "factory-a",
            "empty-supervisor",
            "bare-shell",
            "hung-empty",
            "orphaned-supervisor"
        ]
    );
}

/// cas-94e1: metadata can outlive the supervisor pane and must be marked
/// dormant rather than treated as a live Commander conversation.
#[test]
fn dormant_supervisor_metadata_is_not_a_live_catalog_row() {
    let mut dormant = fixture_session("orphaned-supervisor");
    dormant.dormant = true;
    assert!(
        super::server::supervisor_sessions(vec![dormant], false, false).is_empty(),
        "a non-empty supervisor name is not proof of a live supervisor"
    );
}

/// cas-0140: an audit row that cannot be written refuses its request, and the
/// hub records why: in memory, in audit-health.json, and so in `cas hub
/// status` and doctor. The record outlives a restart and the next written row
/// clears it. A quiet but healthy log is not reported as a failure.
#[cfg(unix)]
#[test]
fn cas_0140_audit_writer_failure_is_visible_and_clears_after_restart() {
    use chrono::Utc;
    use std::os::unix::fs::PermissionsExt;

    let temp = private_tempdir();
    let state_dir = temp.path().join("hub");
    let log = state_dir.join(super::AUDIT_LOG_FILE);
    let health = state_dir.join(super::AUDIT_HEALTH_FILE);
    let store = AuthStore::open(&state_dir, "machine-test").unwrap();
    store.audit(None, "allowed", "before-failure", None, None, Utc::now()).unwrap();
    let report = super::audit_writer_report(&state_dir, Utc::now());
    assert_eq!(report.status, "ok");
    assert!(report.last_row_at.is_some());
    assert!(!health.exists());

    // The log stops being a private regular file: every write now fails.
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(store.audit(None, "allowed", "during-failure", None, None, Utc::now()).is_err());
    assert!(store.audit(None, "denied", "dpop_auth", None, None, Utc::now()).is_err());
    let recorded = store.audit_health().expect("the failure is recorded in memory");
    assert_eq!(recorded.failures, 2);
    assert_eq!(recorded.last_action, "dpop_auth");
    assert!(recorded.last_error.contains("0600"), "{}", recorded.last_error);
    assert_eq!(super::read_audit_health(&state_dir).unwrap(), Some(recorded.clone()));
    let report = super::audit_writer_report(&state_dir, Utc::now());
    assert!(report.is_failure());
    assert!(report.message.contains("2 failures") && report.message.contains("dpop_auth"), "{}", report.message);

    // A restarted hub still reports it until a row lands.
    drop(store);
    let restarted = AuthStore::open(&state_dir, "machine-test").unwrap();
    assert_eq!(restarted.audit_health(), Some(recorded));
    assert!(super::audit_writer_report(&state_dir, Utc::now()).is_failure());

    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o600)).unwrap();
    restarted.audit(None, "allowed", "after-restart", None, None, Utc::now()).unwrap();
    assert_eq!(restarted.audit_health(), None);
    assert!(!health.exists(), "the first written row clears the failure record");
    assert_eq!(super::audit_writer_report(&state_dir, Utc::now()).status, "ok");
    let actions = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["action"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(actions, ["before-failure", "after-restart"]);
}

/// cas-d636: the soundwave denials (2026-09-26 22:58:05Z) were proofs signed
/// before the phone slept and sent when it woke, 266 s later. A stale proof is
/// now refused as `stale_proof` (retryable, with the skew in its audit row),
/// and a fresh proof from the same pairing recovers at once. Only a
/// definitive refusal (revoked) reads as one.
#[test]
fn cas_d636_a_stale_proof_is_a_retryable_refusal_and_a_fresh_proof_recovers() {
    use chrono::{Duration, Utc};
    use p256::ecdsa::SigningKey;
    use p256::elliptic_curve::rand_core::OsRng;

    let temp = private_tempdir();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
    let now = Utc::now();
    let signing = SigningKey::random(&mut OsRng);
    let invitation = auth
        .mint_pairing("https://controller.example", Scope::default_read_only(), now)
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
    let authenticate = |proof: &str| {
        auth.authenticate_dpop(&authorization, proof, "https://controller.example", "GET", "/v1/machine", now)
    };
    let refused = |proof: &str| *authenticate(proof).unwrap_err().downcast_ref::<AuthRefusal>().expect("a typed refusal");

    let asleep = sign_dpop(&signing, &credential.credential, "GET", "/v1/machine", now - Duration::seconds(266), "slept");
    let stale = refused(&asleep);
    assert_eq!(stale, AuthRefusal::StaleProof { skew_secs: -266 });
    assert!(stale.retryable());
    assert_eq!(stale.code(), "stale_proof");
    assert_eq!(stale.dpop_error(), "invalid_dpop_proof");

    let fresh = sign_dpop(&signing, &credential.credential, "GET", "/v1/machine", now, "woke");
    authenticate(&fresh).expect("a fresh proof from the same pairing recovers");
    assert_eq!(refused(&fresh), AuthRefusal::ProofReplay);
    let wrong_target = sign_dpop(&signing, &credential.credential, "GET", "/v1/sessions", now, "target");
    assert_eq!(refused(&wrong_target), AuthRefusal::InvalidProof);
    let other_key = SigningKey::random(&mut OsRng);
    let foreign = sign_dpop(&other_key, &credential.credential, "GET", "/v1/machine", now, "foreign");
    assert_eq!(refused(&foreign), AuthRefusal::KeyMismatch);
    assert!(!AuthRefusal::KeyMismatch.retryable());

    auth.revoke_device(&credential.device_id, now).unwrap();
    let after_revoke = sign_dpop(&signing, &credential.credential, "GET", "/v1/machine", now, "revoked");
    let revoked = refused(&after_revoke);
    assert_eq!(revoked, AuthRefusal::Revoked);
    assert_eq!(revoked.dpop_error(), "invalid_token");
    assert!(!revoked.retryable());
    let unknown = auth
        .authenticate_dpop("DPoP not-a-credential", &after_revoke, "https://controller.example", "GET", "/v1/machine", now)
        .unwrap_err();
    assert_eq!(unknown.downcast_ref::<AuthRefusal>(), Some(&AuthRefusal::UnknownCredential));

    // Every denial names its reason in the audit log; the stale one its skew.
    let rows = std::fs::read_to_string(temp.path().join("hub/audit.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|row| row["outcome"] == "denied")
        .map(|row| (row["action"].as_str().unwrap().to_owned(), row["reason"].as_str().unwrap().to_owned(), row["detail"].as_str().map(str::to_owned)))
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        [
            ("dpop_auth".to_owned(), "stale_proof".to_owned(), Some("proof iat 266s behind the hub clock".to_owned())),
            ("dpop_replay".to_owned(), "proof_replay".to_owned(), None),
            ("dpop_auth".to_owned(), "invalid_proof".to_owned(), None),
            ("dpop_auth".to_owned(), "key_mismatch".to_owned(), None),
            ("dpop_auth".to_owned(), "revoked".to_owned(), None),
        ]
    );
}

/// cas-d636: the hub's 401 says why, readable cross-origin: the reason,
/// whether a fresh proof can succeed, the hub's clock, and RFC 9449's
/// `WWW-Authenticate: DPoP error=...`.
#[tokio::test]
async fn cas_d636_the_401_carries_a_machine_readable_reason_and_the_hub_clock() {
    use chrono::{Duration, Utc};
    use p256::ecdsa::SigningKey;
    use p256::elliptic_curve::rand_core::OsRng;

    let temp = private_tempdir();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
    let now = Utc::now();
    let signing = SigningKey::random(&mut OsRng);
    let invitation = auth
        .mint_pairing("https://controller.example", Scope::default_read_only(), now)
        .unwrap();
    let mut exchange = PairingExchange::test_fixture(
        invitation.token,
        "machine-test",
        "https://controller.example",
        Scope::default_read_only(),
    );
    exchange.public_key_jwk = public_jwk(&signing);
    let credential = auth.exchange_pairing(exchange, now).unwrap();
    let events = MachineEventBus::new(16);
    let app = router(
        HubState::new(
            SessionCatalog::new(RecordingReadModel::with_sessions(vec![fixture_session("factory-a")])),
            Arc::new(PreAuthAuthorizer),
            MachineIdentity { id: "machine-test".into() },
            DaemonConnector::new(SessionMultiplexer::new(8), events.clone()),
            events,
        )
        .with_auth(auth.clone()),
    );
    let get = |proof: String, authorization: String| {
        Request::get("/v1/machine")
            .header("origin", "https://controller.example")
            .header("authorization", authorization)
            .header("dpop", proof)
            .body(Body::empty())
            .unwrap()
    };
    let authorization = format!("DPoP {}", credential.credential);
    let stale = app
        .clone()
        .oneshot(get(sign_dpop(&signing, &credential.credential, "GET", "/v1/machine", now - Duration::seconds(266), "slept"), authorization.clone()))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(stale.headers()["access-control-allow-origin"], "https://controller.example");
    assert_eq!(stale.headers()["www-authenticate"], "DPoP error=\"invalid_dpop_proof\", error_description=\"stale_proof\"");
    // This origin is bound: correlate the refusal with its safe request ID.
    assert_eq!(stale.headers()["access-control-expose-headers"], "WWW-Authenticate, X-Cas-Request-Id");
    let body: serde_json::Value = serde_json::from_slice(&to_bytes(stale.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["reason"], "stale_proof");
    assert_eq!(body["retryable"], true);
    assert!((body["server_time"].as_i64().unwrap() - Utc::now().timestamp()).abs() <= 5);

    let fresh = app
        .clone()
        .oneshot(get(sign_dpop(&signing, &credential.credential, "GET", "/v1/machine", Utc::now(), "woke"), authorization.clone()))
        .await
        .unwrap();
    assert_eq!(fresh.status(), StatusCode::OK, "the retry with a fresh proof recovers");

    auth.revoke_device(&credential.device_id, Utc::now()).unwrap();
    let revoked = app
        .clone()
        .oneshot(get(sign_dpop(&signing, &credential.credential, "GET", "/v1/machine", Utc::now(), "after"), authorization))
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(revoked.headers()["www-authenticate"], "DPoP error=\"invalid_token\", error_description=\"revoked\"");
    let body: serde_json::Value = serde_json::from_slice(&to_bytes(revoked.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["reason"], "revoked");
    assert_eq!(body["retryable"], false);
}

/// A read model whose reads park until the test opens the gate, standing in for
/// the SQLite stall (cas-e335) that pinned every Tokio worker of a macOS hub.
#[derive(Clone)]
struct StalledReadModel {
    gate: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    entered: Arc<std::sync::atomic::AtomicUsize>,
}

impl StalledReadModel {
    fn new() -> Self {
        Self {
            gate: Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new())),
            entered: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    fn release(&self) {
        let (open, wake) = &*self.gate;
        *open.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        wake.notify_all();
    }

    fn entered(&self) -> usize {
        self.entered.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl SessionReadModel for StalledReadModel {
    fn list_sessions(&self) -> anyhow::Result<Vec<HubSession>> {
        self.entered.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (open, wake) = &*self.gate;
        let mut guard = open.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while !*guard {
            guard = wake
                .wait(guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        Ok(vec![fixture_session("factory-a")])
    }
}

/// Plain-socket `/v1/health` probe that needs nothing from the Tokio runtime
/// under test, so it still reports when every runtime worker is pinned.
fn probe_health_blocking(address: SocketAddr) -> std::result::Result<std::time::Duration, String> {
    use std::io::{Read, Write};

    let started = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(3);
    let mut stream = std::net::TcpStream::connect_timeout(&address, timeout)
        .map_err(|error| format!("connect: {error}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| format!("read timeout: {error}"))?;
    stream
        .write_all(b"GET /v1/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .map_err(|error| format!("write: {error}"))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| format!("no /v1/health answer within {timeout:?}: {error}"))?;
    let response = String::from_utf8_lossy(&response);
    if response.starts_with("HTTP/1.1 200") {
        Ok(started.elapsed())
    } else {
        Err(format!(
            "unexpected /v1/health answer: {}",
            response.lines().next().unwrap_or_default()
        ))
    }
}

/// cas-e335 regression: a stalled session read must not take `/v1/health`
/// down with it. Before the fix `SessionCatalog::list` ran the synchronous read
/// model inline on the async worker, so as many stalled reads as there are
/// workers (the once-a-second catalog poller plus `/v1/sessions` reads) left
/// the hub alive, holding `hub.lock`, and unable to answer health. The body
/// deliberately blocks its own thread with std primitives: nothing here may
/// depend on a runtime worker being free.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e335_health_answers_while_session_reads_are_stalled() {
    let read_model = StalledReadModel::new();
    let catalog = SessionCatalog::new(read_model.clone());
    let events = MachineEventBus::new(16);
    let app = router(HubState::new(
        catalog.clone(),
        Arc::new(PreAuthAuthorizer),
        MachineIdentity {
            id: "machine-test".into(),
        },
        DaemonConnector::new(SessionMultiplexer::new(8), events.clone()),
        events,
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let hub = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // More concurrent readers than runtime workers, as on the stalled hub.
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let catalog = catalog.clone();
            tokio::spawn(async move { catalog.list().await })
        })
        .collect();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while read_model.entered() == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(read_model.entered() > 0, "a session read must be in flight");
    // Let every reader reach the stalled read before probing.
    std::thread::sleep(std::time::Duration::from_millis(200));

    let health = std::thread::spawn(move || probe_health_blocking(address))
        .join()
        .expect("health probe thread");
    let reads_started = read_model.entered();

    // Always unstall before asserting so a failure cannot hang the test.
    read_model.release();
    let outcomes = futures_util::future::join_all(readers).await;
    hub.abort();

    let elapsed = health.expect("/v1/health must answer while session reads are stalled");
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "/v1/health took {elapsed:?} while session reads were stalled"
    );
    assert_eq!(
        reads_started, 1,
        "concurrent catalog callers share one in-flight read"
    );
    for outcome in outcomes {
        assert_eq!(
            outcome.expect("reader task").expect("catalog read").len(),
            1
        );
    }

    // A finished read is never reused: the next call reads again.
    assert_eq!(catalog.list().await.unwrap().len(), 1);
    assert_eq!(read_model.entered(), 2);
}

/// cas-e335 regression: the read model holds each listed project's registry
/// open across catalog passes and releases it once the project is gone.
/// Before the fix every pass opened `<project>/.cas/cas.db` and the last store
/// drop closed it again, once a second; a close racing the next pass's open of
/// the same file deadlocked SQLite on macOS. (`shared_db` now also owns every
/// close; this pin keeps the hub's registries out of its idle sweeps.)
#[test]
fn e335_catalog_passes_keep_listed_registries_open() {
    use crate::store::init_cas_dir;
    use crate::ui::factory::{SessionInfo, create_metadata};

    let _env = crate::test_support::TestEnvGuard::temp_home();
    let project = private_tempdir();
    let cas_root = init_cas_dir(project.path()).unwrap();
    let session_name = "hub-pinned-registry";
    let session = SessionInfo {
        name: session_name.to_string(),
        metadata: create_metadata(
            session_name,
            std::process::id(),
            "supervisor-agent",
            &[],
            None,
            Some(project.path().to_str().unwrap()),
            Some(4173),
        ),
        is_running: true,
        socket_exists: true,
    };
    let db_path = cas_root.join("cas.db");
    let model = LocalSessionReadModel::default();

    let pinned = |model: &LocalSessionReadModel| {
        model
            .registries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    };

    let projected = model.project(std::slice::from_ref(&session));
    assert_eq!(projected.len(), 1);
    assert_eq!(pinned(&model), 1, "the listed project's registry stays open");
    let probe = cas_store::shared_db::shared_connection(&db_path).unwrap();
    let while_pinned = Arc::strong_count(&probe);

    // A second pass reuses the pinned handle rather than reopening it.
    model.project(std::slice::from_ref(&session));
    let again = cas_store::shared_db::shared_connection(&db_path).unwrap();
    assert!(Arc::ptr_eq(&probe, &again));
    drop(again);
    assert_eq!(Arc::strong_count(&probe), while_pinned);

    // Once the project has no listed session its registry is released.
    model.project(&[]);
    assert_eq!(pinned(&model), 0);
    assert_eq!(
        Arc::strong_count(&probe),
        while_pinned - 1,
        "a project that is no longer listed must not stay pinned"
    );
}

/// cas-55a4: the catalog names who a session's newest row was between,
/// never a paired device's label or the row's content.
#[test]
fn last_activity_names_parties_without_device_labels() {
    assert_eq!(
        super::activity_label("supervisor", "worker-1"),
        "supervisor → worker-1"
    );
    assert_eq!(
        super::activity_label("commander:Daniel@Pixel 10", "supervisor"),
        "Commander → supervisor"
    );
    assert_eq!(
        super::activity_label("relay-watchdog", "operator"),
        "relay-watchdog → Commander"
    );
    assert_eq!(
        super::activity_label("lifecycle-wake:worker-died:8290", "supervisor"),
        "lifecycle-wake → supervisor"
    );
    assert_eq!(
        super::activity_label("terminal", "terminal-history"),
        "terminal → supervisor"
    );
}

// --- cas-566b: structured fleet operations (fleet-operations brief, S1) ----

const OPS_SESSION: &str = "factory-ops";
const OPS_TASK: &str = "cas-ops1";
const OPS_TIP: &str = "0123456789abcdef0123456789abcdef01234567";

struct OpsFixture {
    _temp: tempfile::TempDir,
    hub_root: std::path::PathBuf,
    cas_dir: std::path::PathBuf,
    app: axum::Router,
    events: MachineEventBus,
    signing: p256::ecdsa::SigningKey,
    credential: String,
    device_id: String,
}

/// A paired device holding `scopes`, a live session whose project has one
/// awaiting-merge task, and the hub router over them.
fn ops_fixture(scopes: std::collections::BTreeSet<Scope>) -> OpsFixture {
    use p256::ecdsa::SigningKey;
    use p256::elliptic_curve::rand_core::OsRng;

    let temp = private_tempdir();
    let hub_root = temp.path().join("hub");
    let auth = AuthStore::open(&hub_root, "machine-test").unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let cas_dir = crate::store::init_cas_dir(&project).unwrap();
    let mut task = cas_types::Task::new(OPS_TASK.to_string(), "Ship the ops facade".to_string());
    task.status = cas_types::TaskStatus::AwaitingMerge;
    task.assignee = Some("swift-lark-3".to_string());
    task.deliverables.parked_branch = Some("factory/swift-lark-3-cas-ops1".to_string());
    task.deliverables.factory_branch_anchor = Some(OPS_TIP.to_string());
    crate::store::open_task_store(&cas_dir).unwrap().add(&task).unwrap();

    let mut session = fixture_session(OPS_SESSION);
    session.project_dir = Some(project.display().to_string());
    let events = MachineEventBus::new(16);
    let app = router(
        HubState::new(
            SessionCatalog::new(RecordingReadModel::with_sessions(vec![session])),
            Arc::new(PreAuthAuthorizer),
            MachineIdentity {
                id: "machine-test".into(),
            },
            DaemonConnector::new(SessionMultiplexer::new(8), events.clone()),
            events.clone(),
        )
        .with_auth(auth.clone())
        .with_effective_origin("https://controller.example"),
    );
    let now = chrono::Utc::now();
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
    OpsFixture {
        _temp: temp,
        hub_root,
        cas_dir,
        app,
        events,
        signing,
        device_id: credential.device_id.clone(),
        credential: credential.credential,
    }
}

impl OpsFixture {
    async fn call(&self, method: &str, uri: &str, body: Option<serde_json::Value>) -> (StatusCode, serde_json::Value) {
        let proof = sign_dpop(
            &self.signing,
            &self.credential,
            method,
            uri,
            chrono::Utc::now(),
            &uuid::Uuid::new_v4().to_string(),
        );
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("origin", "https://controller.example")
            .header("authorization", format!("DPoP {}", self.credential))
            .header("dpop", proof)
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    async fn operate(&self, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        self.call("POST", &format!("/v1/sessions/{OPS_SESSION}/operations"), Some(body)).await
    }

    fn audit(&self, action: &str) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.hub_root.join(AUDIT_LOG_FILE))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|row| row["action"] == action)
            .collect()
    }

    fn queued(&self) -> Vec<cas_store::QueuedPrompt> {
        crate::store::open_prompt_queue_store(&self.cas_dir)
            .unwrap()
            .peek_all(50)
            .unwrap()
    }
}

fn request_merge(op_id: &str, tip: &str) -> serde_json::Value {
    serde_json::json!({
        "op_id": op_id,
        "op": {"kind": "request_merge", "task_id": OPS_TASK},
        "expected": {"status": "awaiting_merge", "tip": tip},
    })
}

fn control_scopes() -> std::collections::BTreeSet<Scope> {
    [
        Scope::MachineRead,
        Scope::SessionRead,
        Scope::PaneRead,
        Scope::MessageSend,
    ]
    .into_iter()
    .collect()
}

/// O1: the hub's "ask the supervisor to merge" reaches the supervisor as the
/// same queue row an MCP `coordination message` produces (target, text,
/// session, summary, priority, urgency), stamped with the device that asked,
/// with requested and outcome audit rows and a FleetChanged event.
#[tokio::test]
async fn operations_request_merge_reuses_message_send() {
    let fixture = ops_fixture(control_scopes());
    let mut events = fixture.events.subscribe();

    let (status, body) = fixture
        .operate(request_merge("6f1c2d3e-0000-4000-8000-000000000001", OPS_TIP))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"]["kind"], "request_merge", "{body}");
    assert_eq!(body["op_id"], "6f1c2d3e-0000-4000-8000-000000000001");

    let rows = fixture.queued();
    assert_eq!(rows.len(), 1, "exactly one supervisor message: {rows:?}");
    let hub = &rows[0];
    let expected_text = crate::ops::fleet::request_merge_text(
        OPS_TASK,
        "Ship the ops facade",
        Some("factory/swift-lark-3-cas-ops1"),
        Some(OPS_TIP),
    );
    assert!(hub.prompt.contains(OPS_TASK) && hub.prompt.contains(OPS_TIP), "{}", hub.prompt);
    assert_eq!(hub.prompt, expected_text);

    // Parity with the row an MCP coordination message to the supervisor makes.
    let mcp_id = crate::store::open_prompt_queue_store(&fixture.cas_dir)
        .unwrap()
        .enqueue_urgent_with_outcome(
            "supervisor",
            "supervisor",
            &expected_text,
            Some(OPS_SESSION),
            hub.summary.as_deref(),
            None,
            false,
            None,
        )
        .unwrap()
        .id();
    let mcp = fixture
        .queued()
        .into_iter()
        .find(|row| row.id == mcp_id)
        .unwrap();
    assert_eq!(hub.target, mcp.target);
    assert_eq!(hub.prompt, mcp.prompt);
    assert_eq!(hub.factory_session, mcp.factory_session);
    assert_eq!(hub.summary, mcp.summary);
    assert_eq!(hub.priority, mcp.priority);
    assert_eq!(hub.urgent, mcp.urgent);
    let stamp = hub.operator.as_ref().expect("the device's operator stamp");
    assert!(stamp.verified);
    assert_eq!(stamp.device_id, fixture.device_id);

    let audit = fixture.audit("operation:request_merge");
    let outcomes: Vec<_> = audit.iter().map(|row| row["outcome"].as_str().unwrap()).collect();
    assert_eq!(outcomes, ["requested", "allowed"], "{audit:?}");
    assert!(audit.iter().all(|row| row["target_session"] == OPS_SESSION && row["required_scope"] == "message:send"));

    let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
        .await
        .expect("FleetChanged is emitted")
        .unwrap();
    assert_eq!(event.kind, MachineEventKind::FleetChanged);
    assert_eq!(event.session.as_deref(), Some(OPS_SESSION));

    // A device without factory:operate learns which scope assignment needs.
    let (status, body) = fixture
        .operate(serde_json::json!({
            "op_id": "6f1c2d3e-0000-4000-8000-0000000000ff",
            "op": {"kind": "assign_task", "task_id": OPS_TASK, "assignee": null},
            "expected": {},
        }))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["required_scope"], "factory:operate", "assign_task needs factory:operate");
}

/// cas-ab04 (GH #1169 part 2): a paired device holding factory:manage grants
/// a task write access to a folder. The hub itself writes the grant to the
/// project's operator policy, recording the device; the task gets a note,
/// the supervisor gets a verified operator receipt in the conversation, and
/// every step is audited. Revoke removes it.
#[tokio::test]
async fn cas_ab04_paired_device_grants_and_revokes_write_access() {
    let mut scopes = control_scopes();
    scopes.insert(Scope::FactoryManage);
    let fixture = ops_fixture(scopes);
    let granted = fixture._temp.path().join("soundwave-config/docs/requests");
    std::fs::create_dir_all(&granted).unwrap();
    let granted = granted.canonicalize().unwrap();
    let uri = format!("/v1/sessions/{OPS_SESSION}/write-grants");

    let (status, body) = fixture
        .call(
            "POST",
            &uri,
            Some(serde_json::json!({
                "action": "grant",
                "task": OPS_TASK,
                "path": granted.display().to_string(),
                "mode": "create+edit",
                "reason": "INGEST request files",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let device = format!("commander-device:{}", fixture.device_id);
    assert_eq!(body["grant"]["granted_by"], device.as_str(), "{body}");
    assert_eq!(body["grant"]["task"], OPS_TASK);

    let policy = crate::config::operator_policy::load_operator_policy(&fixture.cas_dir).unwrap();
    assert_eq!(policy.grants.len(), 1, "{policy:?}");
    assert_eq!(policy.grants[0].path, granted);
    assert_eq!(policy.grants[0].granted_by.as_deref(), Some(device.as_str()));
    let notes = crate::store::open_task_store(&fixture.cas_dir).unwrap().get(OPS_TASK).unwrap().notes;
    assert!(notes.contains("operator write grant") && notes.contains(&fixture.device_id), "{notes}");

    let rows = fixture.queued();
    let receipt = rows.iter().find(|row| row.prompt.contains("write access")).expect("a receipt to the supervisor");
    assert!(receipt.prompt.contains(OPS_TASK) && receipt.prompt.contains(&granted.display().to_string()), "{}", receipt.prompt);
    let stamp = receipt.operator.as_ref().expect("the device's operator stamp");
    assert!(stamp.verified);
    assert_eq!(stamp.device_id, fixture.device_id);
    let outcomes: Vec<_> = fixture
        .audit("write_grant")
        .iter()
        .map(|row| row["outcome"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(outcomes, ["requested", "allowed"]);

    let (status, body) = fixture
        .call(
            "POST",
            &uri,
            Some(serde_json::json!({"action": "revoke", "task": OPS_TASK})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], 1, "{body}");
    assert!(crate::config::operator_policy::load_operator_policy(&fixture.cas_dir).unwrap().grants.is_empty());

    let (status, body) = fixture
        .call(
            "POST",
            &uri,
            Some(serde_json::json!({"action": "grant", "task": OPS_TASK, "path": granted.display().to_string(), "reason": " "})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a grant needs a reason: {body}");
}

/// cas-ab04: no device session, or a device without factory:manage, cannot
/// create a grant. An agent has no device credential, so it lands here.
#[tokio::test]
async fn cas_ab04_write_grants_refuse_unauthenticated_and_unscoped_callers() {
    let fixture = ops_fixture(control_scopes());
    let uri = format!("/v1/sessions/{OPS_SESSION}/write-grants");
    let body = serde_json::json!({
        "action": "grant",
        "task": OPS_TASK,
        "path": fixture._temp.path().display().to_string(),
        "reason": "agent forging a grant",
    });

    let (status, response) = fixture.call("POST", &uri, Some(body.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{response}");
    assert_eq!(response["required_scope"], "factory:manage");

    let bare = Request::builder()
        .method("POST")
        .uri(&uri)
        .header("origin", "https://controller.example")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = fixture.app.clone().oneshot(bare).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    assert!(
        !crate::config::operator_policy::operator_policy_path(&fixture.cas_dir).exists(),
        "no grant was written"
    );
}

/// A retried `op_id` returns the first outcome and sends nothing twice.
#[tokio::test]
async fn operations_op_id_is_idempotent() {
    let fixture = ops_fixture(control_scopes());
    let op_id = "6f1c2d3e-0000-4000-8000-000000000002";

    let first = fixture.operate(request_merge(op_id, OPS_TIP)).await;
    let retry = fixture.operate(request_merge(op_id, OPS_TIP)).await;
    assert_eq!(first.0, StatusCode::OK, "{}", first.1);
    assert_eq!(retry, first, "a retry returns the first outcome");
    assert_eq!(fixture.queued().len(), 1, "a retry never sends twice");
    assert_eq!(
        fixture
            .audit("operation:request_merge")
            .iter()
            .filter(|row| row["outcome"] == "requested")
            .count(),
        1,
        "a replay runs nothing, so it requests nothing"
    );

    let another = fixture
        .operate(request_merge("6f1c2d3e-0000-4000-8000-000000000003", OPS_TIP))
        .await;
    assert_eq!(another.0, StatusCode::OK);
    assert_eq!(fixture.queued().len(), 2, "a new op_id is a new operation");
}

/// An `expected` precondition the fleet no longer meets returns 409 stale
/// with the current state, and nothing is sent or emitted.
#[tokio::test]
async fn operations_stale_expected_returns_409_without_side_effects() {
    let fixture = ops_fixture(control_scopes());
    let mut events = fixture.events.subscribe();

    let (status, body) = fixture
        .operate(request_merge(
            "6f1c2d3e-0000-4000-8000-000000000004",
            "ffffffffffffffffffffffffffffffffffffffff",
        ))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "stale");
    assert_eq!(body["current"]["status"], "awaiting_merge");
    assert_eq!(body["current"]["tip"], OPS_TIP);

    let stale_status = fixture
        .operate(serde_json::json!({
            "op_id": "6f1c2d3e-0000-4000-8000-000000000005",
            "op": {"kind": "request_merge", "task_id": OPS_TASK},
            "expected": {"status": "in_progress", "tip": OPS_TIP},
        }))
        .await;
    assert_eq!(stale_status.0, StatusCode::CONFLICT, "{}", stale_status.1);

    assert!(fixture.queued().is_empty(), "a stale operation sends nothing");
    let outcomes: Vec<_> = fixture
        .audit("operation:request_merge")
        .iter()
        .map(|row| row["outcome"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(outcomes, ["requested", "stale", "requested", "stale"]);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), events.recv())
            .await
            .is_err(),
        "a stale operation changes nothing, so it emits nothing"
    );
}

fn operate_scopes() -> std::collections::BTreeSet<Scope> {
    let mut scopes = control_scopes();
    // factory:operate gates O2, O3, O4 and O5 (S2, cas-9b08).
    scopes.insert(Scope::FactoryOperate);
    scopes
}

/// Add an open, unassigned task and return it as stored.
fn ready_task(fixture: &OpsFixture, id: &str) -> cas_types::Task {
    let store = crate::store::open_task_store(&fixture.cas_dir).unwrap();
    let task = cas_types::Task::new(id.to_string(), "Wire the assign op".to_string());
    store.add(&task).unwrap();
    store.get(id).unwrap()
}

fn stored_task(fixture: &OpsFixture, id: &str) -> cas_types::Task {
    crate::store::open_task_store(&fixture.cas_dir).unwrap().get(id).unwrap()
}

fn assign_op(
    op_id: &str,
    task_id: &str,
    assignee: Option<&str>,
    updated_at: &str,
    current: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "op_id": op_id,
        "op": {"kind": "assign_task", "task_id": task_id, "assignee": assignee},
        "expected": {"updated_at": updated_at, "assignee": current},
    })
}

/// A task note without its `[YYYY-MM-DD HH:MM] ` stamp, so two updates made
/// a minute apart compare equal.
fn unstamped_notes(task: &cas_types::Task) -> Vec<String> {
    task.notes
        .split("\n\n")
        .map(|note| {
            note.split_once("] ")
                .filter(|(stamp, _)| stamp.starts_with('['))
                .map_or(note, |(_, body)| body)
                .to_string()
        })
        .collect()
}

async fn supervisor_task_update(fixture: &OpsFixture, id: &str, assignee: &str, notes: Option<&str>) {
    let core = crate::mcp::CasCore::with_daemon(fixture.cas_dir.clone(), None, None);
    let request: crate::mcp::tools::TaskUpdateRequest = serde_json::from_value(serde_json::json!({
        "id": id,
        "assignee": assignee,
        "notes": notes,
    }))
    .unwrap();
    core.cas_task_update_with_target(request, None, None, false, None, None)
        .await
        .expect("the supervisor's task update succeeds");
}

/// O5 (S3): assigning from the hub runs the supervisor's `task_update`: the
/// same assignee and the same task note as that call, audited, followed by
/// FleetChanged, and answered with the inverse operation Undo sends.
/// Unassigning is the same operation with a null assignee.
#[tokio::test]
async fn operations_assign_task_matches_task_update() {
    let fixture = ops_fixture(operate_scopes());
    let mut events = fixture.events.subscribe();
    let hub_task = ready_task(&fixture, "cas-asg1");
    let mcp_task = ready_task(&fixture, "cas-asg2");

    let (status, body) = fixture
        .operate(assign_op(
            "6f1c2d3e-0000-4000-8000-000000000101",
            "cas-asg1",
            Some("swift-lark-3"),
            &hub_task.updated_at.to_rfc3339(),
            None,
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let outcome = &body["outcome"];
    assert_eq!(outcome["kind"], "assign_task", "{body}");
    assert_eq!(outcome["task_id"], "cas-asg1");
    assert_eq!(outcome["assignee"], "swift-lark-3");
    assert!(outcome["prior_assignee"].is_null(), "{body}");

    let assigned = stored_task(&fixture, "cas-asg1");
    assert_eq!(assigned.assignee.as_deref(), Some("swift-lark-3"));
    assert_eq!(outcome["updated_at"], assigned.updated_at.to_rfc3339());
    let note = unstamped_notes(&assigned).pop().unwrap_or_default();
    assert!(
        note.contains("swift-lark-3") && note.contains("Commander"),
        "the assignment leaves a task note naming the assignee and Commander: {note:?}"
    );

    // The supervisor's own task update with that note yields the same task.
    supervisor_task_update(&fixture, "cas-asg2", "swift-lark-3", Some(&note)).await;
    let reference = stored_task(&fixture, "cas-asg2");
    assert_eq!(assigned.assignee, reference.assignee);
    assert_eq!(unstamped_notes(&assigned), unstamped_notes(&reference));
    assert_eq!(assigned.status, reference.status);
    assert_eq!(
        assigned.deliverables.handoff_branches,
        reference.deliverables.handoff_branches
    );
    assert_ne!(assigned.updated_at, hub_task.updated_at, "the update is stamped");
    let _ = mcp_task;

    // Undo is the inverse operation, preconditioned on the state just made.
    assert_eq!(
        outcome["inverse"],
        serde_json::json!({
            "op": {"kind": "assign_task", "task_id": "cas-asg1", "assignee": null},
            "expected": {"updated_at": assigned.updated_at.to_rfc3339(), "assignee": "swift-lark-3"},
        })
    );
    let audit = fixture.audit("operation:assign_task");
    let outcomes: Vec<_> = audit.iter().map(|row| row["outcome"].as_str().unwrap()).collect();
    assert_eq!(outcomes, ["requested", "allowed"], "{audit:?}");
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
        .await
        .expect("FleetChanged is emitted")
        .unwrap();
    assert_eq!(event.kind, MachineEventKind::FleetChanged);

    // Unassign: the inverse, sent as Undo would send it.
    let mut undo = outcome["inverse"].clone();
    undo["op_id"] = "6f1c2d3e-0000-4000-8000-000000000102".into();
    let (status, body) = fixture.operate(undo).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["outcome"]["assignee"].is_null(), "{body}");
    assert_eq!(body["outcome"]["prior_assignee"], "swift-lark-3");
    let unassigned = stored_task(&fixture, "cas-asg1");
    assert_eq!(unassigned.assignee, None);
    supervisor_task_update(&fixture, "cas-asg2", "", None).await;
    assert_eq!(stored_task(&fixture, "cas-asg2").assignee, None);
}

/// O5 (S3): assigning a task another device has assigned since the operator
/// looked returns 409 stale with the current assignee and changes nothing; so
/// does any update since (a changed `updated_at`).
#[tokio::test]
async fn operations_assign_stale_assignee() {
    let fixture = ops_fixture(operate_scopes());
    let mut events = fixture.events.subscribe();
    let seen = ready_task(&fixture, "cas-asg3");
    let seen_at = seen.updated_at.to_rfc3339();

    // Another device (or the supervisor) assigns it first.
    supervisor_task_update(&fixture, "cas-asg3", "other-worker", None).await;
    let current = stored_task(&fixture, "cas-asg3");

    let (status, body) = fixture
        .operate(assign_op(
            "6f1c2d3e-0000-4000-8000-000000000103",
            "cas-asg3",
            Some("swift-lark-3"),
            &seen_at,
            None,
        ))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "stale");
    assert_eq!(body["current"]["assignee"], "other-worker");
    assert_eq!(body["current"]["updated_at"], current.updated_at.to_rfc3339());
    let after = stored_task(&fixture, "cas-asg3");
    assert_eq!(after.assignee.as_deref(), Some("other-worker"));
    assert_eq!(after.updated_at, current.updated_at, "a stale assign writes nothing");

    // The assignee matches but the task changed since: still stale.
    let (status, body) = fixture
        .operate(assign_op(
            "6f1c2d3e-0000-4000-8000-000000000104",
            "cas-asg3",
            Some("swift-lark-3"),
            &seen_at,
            Some("other-worker"),
        ))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(stored_task(&fixture, "cas-asg3").assignee.as_deref(), Some("other-worker"));

    let outcomes: Vec<_> = fixture
        .audit("operation:assign_task")
        .iter()
        .map(|row| row["outcome"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(outcomes, ["requested", "stale", "requested", "stale"]);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), events.recv())
            .await
            .is_err(),
        "a stale operation changes nothing, so it emits nothing"
    );
}

fn add_epic(fixture: &OpsFixture, id: &str) {
    let mut epic = cas_types::Task::new(id.to_string(), format!("Epic {id}"));
    epic.task_type = cas_types::TaskType::Epic;
    crate::store::open_task_store(&fixture.cas_dir).unwrap().add(&epic).unwrap();
}

fn focus_op(op_id: &str, epic_id: Option<&str>, current: Option<&str>) -> serde_json::Value {
    let op = match epic_id {
        Some(epic_id) => serde_json::json!({"kind": "focus_epic", "epic_id": epic_id}),
        None => serde_json::json!({"kind": "focus_epic", "clear": true}),
    };
    serde_json::json!({"op_id": op_id, "op": op, "expected": {"epic_id": current}})
}

/// O2 (S3): a focus answers with its inverse operation, which Undo sends as
/// a new operation: it restores the previous focus (a pin or no pin), and it
/// is itself stale once the focus has moved on.
#[tokio::test]
async fn operations_focus_epic_inverse() {
    let _home = crate::test_env_guard::TestEnvGuard::temp_home();
    let fixture = ops_fixture(operate_scopes());
    add_epic(&fixture, "cas-epa1");
    add_epic(&fixture, "cas-epa2");
    let metadata_path = crate::ui::factory::metadata_path(OPS_SESSION);
    std::fs::create_dir_all(metadata_path.parent().unwrap()).unwrap();
    let metadata = crate::ui::factory::create_metadata(OPS_SESSION, 12345, "supervisor", &[], None, None, None);
    std::fs::write(&metadata_path, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();
    let pinned = || crate::ops::fleet::pinned_epic(OPS_SESSION);

    // No pin -> pin: the inverse clears, expecting the new pin.
    let (status, body) = fixture
        .operate(focus_op("6f1c2d3e-0000-4000-8000-000000000201", Some("cas-epa1"), None))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(pinned().as_deref(), Some("cas-epa1"));
    assert_eq!(body["outcome"]["prior_epic_id"], serde_json::Value::Null, "{body}");
    assert_eq!(
        body["outcome"]["inverse"],
        serde_json::json!({
            "op": {"kind": "focus_epic", "clear": true},
            "expected": {"epic_id": "cas-epa1"},
        })
    );
    let mut undo = body["outcome"]["inverse"].clone();
    undo["op_id"] = "6f1c2d3e-0000-4000-8000-000000000202".into();
    let (status, body) = fixture.operate(undo).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(pinned(), None, "Undo restores no pin");

    // Pin -> another pin: the inverse re-pins the first epic.
    fixture
        .operate(focus_op("6f1c2d3e-0000-4000-8000-000000000203", Some("cas-epa1"), None))
        .await;
    let (status, body) = fixture
        .operate(focus_op("6f1c2d3e-0000-4000-8000-000000000204", Some("cas-epa2"), Some("cas-epa1")))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"]["prior_epic_id"], "cas-epa1");
    assert_eq!(
        body["outcome"]["inverse"],
        serde_json::json!({
            "op": {"kind": "focus_epic", "epic_id": "cas-epa1"},
            "expected": {"epic_id": "cas-epa2"},
        })
    );
    let inverse = body["outcome"]["inverse"].clone();
    let mut undo = inverse.clone();
    undo["op_id"] = "6f1c2d3e-0000-4000-8000-000000000205".into();
    let (status, body) = fixture.operate(undo).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(pinned().as_deref(), Some("cas-epa1"), "Undo restores the previous pin");

    // An Undo sent after the focus moved on is stale and changes nothing.
    let mut late = inverse;
    late["op_id"] = "6f1c2d3e-0000-4000-8000-000000000206".into();
    let (status, body) = fixture.operate(late).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["current"]["epic_id"], "cas-epa1");
    assert_eq!(pinned().as_deref(), Some("cas-epa1"));
}

/// End session was the one Commander mutation with no audit row (brief O8).
#[tokio::test]
async fn end_session_writes_audit_row() {
    let _home = crate::test_env_guard::TestEnvGuard::temp_home();
    let mut scopes = control_scopes();
    scopes.insert(Scope::FactoryManage);
    let fixture = ops_fixture(scopes);

    let (status, _) = fixture
        .call("DELETE", "/v1/sessions/factory-gone-566b", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let audit = fixture.audit("session_end");
    let outcomes: Vec<_> = audit.iter().map(|row| row["outcome"].as_str().unwrap()).collect();
    assert_eq!(outcomes, ["requested", "not_found"], "{audit:?}");
    for row in &audit {
        assert_eq!(row["target_session"], "factory-gone-566b");
        assert_eq!(row["required_scope"], "factory:manage");
        assert_eq!(row["device_id"], fixture.device_id.as_str());
    }
}

// --- cas-9b08: worker lifecycle operations and factory:operate (brief S2) ---

const OPS_WORKER: &str = "swift-lark-3";
const OPS_GENERATION: &str = "swift-lark-3-gen-2";

impl OpsFixture {
    /// A live worker of the session whose current registration (its spawn
    /// generation) is `generation`.
    fn register_worker(&self, generation: &str) {
        let mut agent = cas_types::Agent::new(generation.to_string(), OPS_WORKER.to_string());
        agent.role = cas_types::AgentRole::Worker;
        agent.factory_session = Some(OPS_SESSION.to_string());
        agent.heartbeat();
        crate::store::open_agent_store(&self.cas_dir)
            .unwrap()
            .register(&agent)
            .unwrap();
    }

    fn add_open_epic(&self) {
        let mut epic = cas_types::Task::new("cas-ops-epic".to_string(), "Ops epic".to_string());
        epic.task_type = cas_types::TaskType::Epic;
        crate::store::open_task_store(&self.cas_dir).unwrap().add(&epic).unwrap();
    }

    /// Pin the project's workers to a harness with no account probe, so
    /// the spawn body's login preflight does not depend on which CLIs this
    /// machine has logged in (`probe_account_auth` runs the real CLI).
    fn pin_worker_harness(&self) {
        let path = self.cas_dir.join("config.toml");
        let mut config = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(!config.contains("[llm.worker]"), "fixture config already pins a worker harness");
        config.push_str("\n[llm.worker]\nharness = \"grok\"\n");
        std::fs::write(&path, config).unwrap();
    }

    fn spawn_queue(&self) -> Vec<cas_store::SpawnRequest> {
        crate::store::open_spawn_queue_store(&self.cas_dir)
            .unwrap()
            .peek(20)
            .unwrap()
    }
}

/// O3: adding a worker from the hub goes through the same MCP
/// `factory_spawn_workers` body, so it lands in the session's spawn queue
/// exactly as a supervisor's spawn would, under factory:operate.
#[tokio::test]
async fn operations_spawn_uses_factory_spawn_workers_queue() {
    let _home = crate::test_env_guard::TestEnvGuard::temp_home();
    let fixture = ops_fixture(operate_scopes());
    fixture.add_open_epic();
    fixture.pin_worker_harness();
    let mut events = fixture.events.subscribe();

    let (status, body) = fixture
        .operate(serde_json::json!({
            "op_id": "9b080000-0000-4000-8000-000000000001",
            "op": {"kind": "spawn_workers", "count": 1},
            "expected": {},
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"]["kind"], "spawn_workers", "{body}");

    let queued = fixture.spawn_queue();
    assert_eq!(queued.len(), 1, "one spawn request: {queued:?}");
    assert_eq!(queued[0].action, cas_store::SpawnAction::Spawn);
    assert_eq!(queued[0].count, Some(1));
    assert_eq!(queued[0].factory_session.as_deref(), Some(OPS_SESSION));

    let audit = fixture.audit("operation:spawn_workers");
    let outcomes: Vec<_> = audit.iter().map(|row| row["outcome"].as_str().unwrap()).collect();
    assert_eq!(outcomes, ["requested", "allowed"], "{audit:?}");
    assert!(audit.iter().all(|row| row["required_scope"] == "factory:operate"));
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
        .await
        .expect("FleetChanged is emitted")
        .unwrap();
    assert_eq!(event.kind, MachineEventKind::FleetChanged);

    // Bounds from the wire contract: 1-4 workers.
    let (status, _) = fixture
        .operate(serde_json::json!({
            "op_id": "9b080000-0000-4000-8000-000000000002",
            "op": {"kind": "spawn_workers", "count": 5},
            "expected": {},
        }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(fixture.spawn_queue().len(), 1, "a refused spawn queues nothing");

}

/// O6 and O7 are destructive: factory:operate is refused with the scope that
/// is needed, and factory:manage stops the worker through
/// `factory_shutdown_workers`.
#[tokio::test]
async fn operations_stop_requires_factory_manage() {
    let _home = crate::test_env_guard::TestEnvGuard::temp_home();
    let operator = ops_fixture(operate_scopes());
    operator.register_worker(OPS_GENERATION);
    for (op_id, op) in [
        (
            "9b080000-0000-4000-8000-000000000011",
            serde_json::json!({"kind": "shutdown_workers", "workers": [OPS_WORKER]}),
        ),
        (
            "9b080000-0000-4000-8000-000000000012",
            serde_json::json!({"kind": "recycle_worker", "worker": OPS_WORKER}),
        ),
    ] {
        let (status, body) = operator
            .operate(serde_json::json!({
                "op_id": op_id,
                "op": op,
                "expected": {"worker": OPS_WORKER, "generation": OPS_GENERATION},
            }))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["error"], "scope_denied");
        assert_eq!(body["required_scope"], "factory:manage");
    }
    assert!(operator.spawn_queue().is_empty(), "a refused stop queues nothing");

    let mut scopes = operate_scopes();
    scopes.insert(Scope::FactoryManage);
    let manager = ops_fixture(scopes);
    manager.register_worker(OPS_GENERATION);
    let (status, body) = manager
        .operate(serde_json::json!({
            "op_id": "9b080000-0000-4000-8000-000000000013",
            "op": {"kind": "shutdown_workers", "workers": [OPS_WORKER]},
            "expected": {"worker": OPS_WORKER, "generation": OPS_GENERATION},
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"]["kind"], "shutdown_workers");
    let queued = manager.spawn_queue();
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert_eq!(queued[0].action, cas_store::SpawnAction::Shutdown);
    assert_eq!(queued[0].worker_names, vec![OPS_WORKER.to_string()]);
    let audit = manager.audit("operation:shutdown_workers");
    assert!(audit.iter().all(|row| row["required_scope"] == "factory:manage"), "{audit:?}");
}

/// Worker operations name the spawn generation the operator saw. A worker
/// that has since restarted is a different generation: 409 stale, the
/// current generation, and nothing queued or held.
#[tokio::test]
async fn operations_worker_generation_stale() {
    let _home = crate::test_env_guard::TestEnvGuard::temp_home();
    let mut scopes = operate_scopes();
    scopes.insert(Scope::FactoryManage);
    let fixture = ops_fixture(scopes);
    fixture.register_worker(OPS_GENERATION);

    for (op_id, op) in [
        (
            "9b080000-0000-4000-8000-000000000021",
            serde_json::json!({"kind": "shutdown_workers", "workers": [OPS_WORKER]}),
        ),
        (
            "9b080000-0000-4000-8000-000000000022",
            serde_json::json!({"kind": "recycle_worker", "worker": OPS_WORKER}),
        ),
        (
            "9b080000-0000-4000-8000-000000000023",
            serde_json::json!({"kind": "set_worker_hold", "worker": OPS_WORKER, "hold": true}),
        ),
    ] {
        let (status, body) = fixture
            .operate(serde_json::json!({
                "op_id": op_id,
                "op": op,
                "expected": {"worker": OPS_WORKER, "generation": "swift-lark-3-gen-1"},
            }))
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"], "stale");
        assert_eq!(body["current"]["worker"], OPS_WORKER);
        assert_eq!(body["current"]["generation"], OPS_GENERATION);
    }
    assert!(fixture.spawn_queue().is_empty(), "a stale operation queues nothing");
    assert_eq!(
        crate::ui::factory::worker_holds_from_session_metadata_named(OPS_SESSION)
            .unwrap_or_default()
            .len(),
        0,
        "a stale hold holds nothing"
    );
}

/// factory:operate is a real scope in every spelling, and the one scope a
/// control device may grant itself besides session launch. factory:manage is
/// never self-granted.
#[tokio::test]
async fn scope_factory_operate_roundtrip() {
    let _home = crate::test_env_guard::TestEnvGuard::temp_home();
    assert_eq!(Scope::parse("factory:operate").unwrap(), Scope::FactoryOperate);
    assert_eq!(Scope::parse("factory-operate").unwrap(), Scope::FactoryOperate);
    assert_eq!(Scope::FactoryOperate.as_str(), "factory:operate");
    assert_eq!(Scope::FactoryOperate.as_wire(), "factory-operate");

    let mut full_control = control_scopes();
    full_control.extend([Scope::PaneInput, Scope::PaneInterrupt]);
    let control = ops_fixture(full_control);
    let grant = |body: &'static str| {
        let fixture = &control;
        async move {
            fixture
                .call("POST", "/v1/auth/scopes", Some(serde_json::from_str(body).unwrap()))
                .await
        }
    };
    let (status, _) = grant(r#"{"add":["factory-manage"]}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "factory:manage is never self-granted");
    let (status, body) = grant(r#"{"add":["factory-operate"]}"#).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["scopes"].as_array().unwrap().iter().any(|scope| scope == "factory:operate"
            || scope == "factory-operate"
            || scope == "FactoryOperate"),
        "{body}"
    );
    assert_eq!(control.audit("self_grant_factory_operate").len(), 1);
    let (status, _) = grant(r#"{"add":["factory-operate"]}"#).await;
    assert_eq!(status, StatusCode::OK, "granting again is idempotent");
    assert_eq!(control.audit("self_grant_factory_operate").len(), 1);

    // The granted scope is usable at once.
    control.add_open_epic();
    let (status, body) = control
        .operate(serde_json::json!({
            "op_id": "9b080000-0000-4000-8000-000000000031",
            "op": {"kind": "focus_epic", "epic_id": "cas-ops-epic"},
            "expected": {"epic_id": null},
        }))
        .await;
    assert_ne!(status, StatusCode::FORBIDDEN, "{body}");

    let read_only = ops_fixture(Scope::default_read_only());
    let (status, _) = read_only
        .call(
            "POST",
            "/v1/auth/scopes",
            Some(serde_json::json!({"add": ["factory-operate"]})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a read-only pairing cannot grant itself control");
}

/// cas-eaa3: Commander's app switcher opens Explorer on the session's project,
/// so the session wire shape carries the project's cloud identity, the same
/// canonical id sync uses, and omits it when there is none to name.
#[test]
fn eaa3_session_reports_the_projects_cloud_identity_for_explorer() {
    let project = tempfile::tempdir().unwrap();
    let cas_root = project.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    std::fs::write(
        cas_root.join("config.toml"),
        "[project]\ncanonical_id = \"github.com/acme/widget\"\n",
    )
    .unwrap();
    assert_eq!(
        cloud_project_id(&cas_root).as_deref(),
        Some("github.com/acme/widget"),
        "the pinned identity Explorer filters on"
    );

    let mut session = fixture_session("factory-main");
    assert!(
        !serde_json::to_value(&session)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("cloud_project_id"),
        "no identity, no field"
    );
    session.cloud_project_id = Some("github.com/acme/widget".into());
    assert_eq!(
        serde_json::to_value(&session).unwrap()["cloud_project_id"],
        "github.com/acme/widget"
    );
}
