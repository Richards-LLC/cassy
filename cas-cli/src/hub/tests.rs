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
// runtime and the parent-side reaper/connector chain to receive CPU time. The
// normal isolated run takes about 3 seconds, so the former 5-second deadline
// flakes under full-suite build contention. Fifteen seconds keeps a bounded
// failure while leaving enough scheduling headroom for a busy developer host.
const H1_DEATH_REAL_PROCESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

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
async fn h4_health_cors_allows_unpaired_trusted_origins_and_preserves_paired_origins() {
    use chrono::Utc;
    use p256::ecdsa::SigningKey;
    use p256::elliptic_curve::rand_core::OsRng;

    let temp = private_tempdir();
    let auth = AuthStore::open(temp.path().join("hub"), "machine-test").unwrap();
    let now = Utc::now();
    let signing = SigningKey::random(&mut OsRng);
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
        "Retry-After"
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
    let csp = response.headers()["content-security-policy"]
        .to_str()
        .unwrap();
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
    assert!(
        !html.replacen(relay_metadata, "", 1).contains("https://"),
        "the reviewed pairing relay must be the embedded page's only external origin"
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
        transport_warning: None,
    };
    paths.write_process_record(&record).unwrap();
    assert_eq!(paths.read_process_record().unwrap(), record);

    drop(first_lock);
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
        attribution: MessageAttribution {
            device_id: None,
            credential_id: None,
            device_label: None,
            operator_label: None,
            controller_origin: None,
            request_id: None,
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

    assert_eq!(required_scope(&input), Some(Scope::PaneInput));
    assert_eq!(required_scope(&resize), Some(Scope::PaneRead));
    assert_eq!(required_scope(&keyframe), Some(Scope::PaneRead));
    assert_eq!(required_scope(&scrollback), Some(Scope::PaneRead));
    assert!(super::server::is_pane_read_message(&keyframe));
    assert!(super::server::is_pane_read_message(&scrollback));
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
    // gabber-studio or mecha_cassy session's roster is read from.
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
    for index in 0..5 {
        let mut worker =
            cas_types::Agent::new(format!("roster-worker-{index}"), format!("worker-{index}"));
        worker.agent_type = AgentType::Worker;
        worker.role = AgentRole::Worker;
        worker.factory_session = Some(session_name.to_string());
        agents.register(&worker).unwrap();
    }

    let mapped = hub_session(&session);
    assert_eq!(mapped.workers.len(), 5, "five live workers must be reported");
    assert_eq!(mapped.supervisor, "supervisor-agent");
    assert_eq!(mapped.epic_id.as_deref(), Some("cas-5d94"));
    assert_eq!(mapped.liveness, DaemonLiveness::Live);
    // The roster carries agent names (what Commander shows), not agent ids.
    let mut names = mapped.workers.clone();
    names.sort();
    assert_eq!(names, vec!["worker-0", "worker-1", "worker-2", "worker-3", "worker-4"]);

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
