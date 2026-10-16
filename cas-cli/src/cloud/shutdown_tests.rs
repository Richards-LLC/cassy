use super::*;
use crate::cloud::CloudConfig;
use crate::store::{init_cas_dir, open_agent_store};
use crate::test_support::TestEnvGuard;
use wiremock::matchers::{method, path as http_path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(endpoint: &str) -> CloudCoordinator {
    CloudCoordinator::new(CloudConfig {
        endpoint: endpoint.into(),
        token: Some("test-token".into()),
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test]
async fn cas_4ac8_persistent_rate_limit_is_scheduled_then_expires_visibly() {
    let env = TestEnvGuard::temp_home();
    let root = init_cas_dir(env.home()).unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(http_path("/api/agents/ended/shutdown"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "600"))
        .expect(1)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    enqueue(&root, "ended", "session-one", &endpoint).unwrap();
    let original = read(&path(&root, "ended", "session-one")).unwrap();
    enqueue(&root, "ended", "session-one", &endpoint).unwrap();
    assert_eq!(
        read(&path(&root, "ended", "session-one"))
            .unwrap()
            .created_at,
        original.created_at
    );
    let r = root.clone();
    let now = Utc::now();
    tokio::task::spawn_blocking(move || {
        let coordinator = client(&endpoint);
        drain(&r, &coordinator, now).unwrap();
        drain(&r, &coordinator, now + chrono::Duration::seconds(10)).unwrap();
        drain(&r, &coordinator, now + chrono::Duration::hours(25)).unwrap();
    })
    .await
    .unwrap();
    let intent = read(&path(&root, "ended", "session-one")).unwrap();
    assert!(intent.next_retry_at >= now + chrono::Duration::seconds(600));
    assert!(health(&root, now).contains("rate limited"));
    assert!(health(&root, now + chrono::Duration::hours(25)).contains("expired 1"));
    assert!(intent.completed_at.is_none());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn cas_4ac8_drain_retries_fallback_and_completed_intent_fences_old_session() {
    let env = TestEnvGuard::temp_home();
    let root = init_cas_dir(env.home()).unwrap();
    let server = MockServer::start().await;
    let times = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = times.clone();
    Mock::given(method("POST"))
        .and(http_path("/api/agents/ended/shutdown"))
        .respond_with(move |_: &wiremock::Request| {
            recorded.lock().unwrap().push(Instant::now());
            ResponseTemplate::new(429).insert_header("Retry-After", "invalid")
        })
        .up_to_n_times(2)
        .with_priority(1)
        .expect(2)
        .mount(&server)
        .await;
    let recorded = times.clone();
    Mock::given(method("POST"))
        .and(http_path("/api/agents/ended/shutdown"))
        .respond_with(move |_: &wiremock::Request| {
            recorded.lock().unwrap().push(Instant::now());
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"released_locks":0}))
        })
        .expect(1)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    enqueue(&root, "ended", "old", &endpoint).unwrap();
    let r = root.clone();
    let started = Instant::now();
    tokio::task::spawn_blocking(move || {
        let coordinator = client(&endpoint);
        drain(&r, &coordinator, Utc::now()).unwrap();
        drain(&r, &coordinator, Utc::now()).unwrap();
    })
    .await
    .unwrap();
    assert!(started.elapsed() >= std::time::Duration::from_millis(600));
    assert!(health(&root, Utc::now()).is_empty());
    let mut old = Agent::new("ended".into(), "same".into());
    old.cc_session_id = Some("old".into());
    assert!(is_ended(&root, &old));
    let coordinator = client(&server.uri());
    assert_eq!(
        crate::cloud::presence::tick(&coordinator, &root, &old, Utc::now()),
        crate::cloud::presence::TickAction::Waiting
    );
    assert_eq!(
        crate::cloud::presence::register_now(&coordinator, &root, &old, Utc::now()).state,
        crate::cloud::presence::PresenceState::Failing
    );
    old.cc_session_id = Some("new".into());
    assert!(!is_ended(&root, &old));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    let recorded = times.lock().unwrap();
    assert!(recorded[1].duration_since(recorded[0]) >= std::time::Duration::from_millis(200));
    assert!(recorded[2].duration_since(recorded[1]) >= std::time::Duration::from_millis(400));
}

#[tokio::test]
async fn cas_4ac8_drain_respects_lease_and_rebound_identity() {
    let env = TestEnvGuard::temp_home();
    let root = init_cas_dir(env.home()).unwrap();
    let server = MockServer::start().await;
    let endpoint = server.uri();
    enqueue(&root, "ended", "old", &endpoint).unwrap();
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(DIR).join("drain.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    drain(&root, &client(&endpoint), Utc::now()).unwrap();
    assert!(health(&root, Utc::now()).contains("pending 1"));
    drop(lock);
    let mut rebound = Agent::new("ended".into(), "same".into());
    rebound.cc_session_id = Some("new".into());
    open_agent_store(&root).unwrap().register(&rebound).unwrap();
    drain(&root, &client(&endpoint), Utc::now()).unwrap();
    assert!(health(&root, Utc::now()).is_empty());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn cas_4ac8_shutdown_health_footer() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    enqueue(root, "retired-agent", "session", "http://example.invalid").unwrap();
    let file = path(root, "retired-agent", "session");
    let mut intent = read(&file).unwrap();
    intent.cause = "Shutdown: rate limited (429)".into();
    write(&file, &intent, false).unwrap();
    let pending = health(root, Utc::now());
    let expired = health(root, Utc::now() + chrono::Duration::hours(25));
    assert!(pending.contains("pending 1, expired 0"));
    assert!(pending.contains("Retry: automatic"));
    assert!(expired.contains("pending 0, expired 1"));
    assert!(expired.contains("ls .cas/cloud_shutdown"));
    assert!(
        pending
            .lines()
            .chain(expired.lines())
            .all(|line| line.chars().count() <= 72)
    );
    eprintln!("CLEANUP_HEALTH_BEGIN\n{pending}{expired}CLEANUP_HEALTH_END");
}
