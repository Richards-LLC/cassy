use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use super::*;
use crate::ui::factory::DaemonMessage;

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
