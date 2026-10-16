use super::*;
use crate::cloud::CloudConfig;
use crate::store::{init_cas_dir, open_agent_store};
use crate::test_support::TestEnvGuard;
use wiremock::matchers::{method, path as http_path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// Stable incarnation across repeated enqueue calls; production passes the real row.
fn ended_agent(agent: &str, session: &str) -> Agent {
    let mut row = Agent::new(agent.into(), agent.into());
    row.registered_at = DateTime::<Utc>::UNIX_EPOCH;
    row.cc_session_id = Some(session.into());
    row
}

fn enqueue(root: &Path, agent: &str, session: &str, endpoint: &str) -> std::io::Result<()> {
    super::enqueue(root, &ended_agent(agent, session), session, endpoint)
}

fn path(root: &Path, agent: &str, session: &str) -> PathBuf {
    incarnation_path(root, &ended_agent(agent, session), session)
}

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
        let scheduled = read(&path(&r, "ended", "session-one")).unwrap();
        assert!(scheduled.next_retry_at >= now + chrono::Duration::seconds(600));
        drain(&r, &coordinator, now + chrono::Duration::seconds(10)).unwrap();
        drain(&r, &coordinator, now + chrono::Duration::hours(25)).unwrap();
    })
    .await
    .unwrap();
    assert!(!path(&root, "ended", "session-one").exists());
    let retained: ExpiredHealth = serde_json::from_slice(
        &std::fs::read(
            root.join(DIR)
                .join("expired")
                .join(path(&root, "ended", "session-one").file_name().unwrap()),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(retained.cause.contains("rate limited"));
    assert!(health(&root, now).contains("rate limited"));
    assert!(health(&root, now + chrono::Duration::hours(25)).contains("expired 1"));
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
    let mut old = ended_agent("ended", "old");
    old.cc_session_id = Some("old".into());
    assert!(is_ended(&root, &old).unwrap());
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
    assert!(!is_ended(&root, &old).unwrap());
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
    // pin: operator-facing cleanup counts, named cause and retry/inspection actions.
    assert_eq!(
        format!("{pending}{expired}"),
        include_str!("../../../docs/design/cli/cloud-shutdown.capture.txt")
    );
}

#[tokio::test]
async fn cas_4ac8_resume_same_session_supersedes_pending_completed_and_expired() {
    let env = TestEnvGuard::temp_home();
    let root = init_cas_dir(env.home()).unwrap();
    let server = MockServer::start().await;
    for state in ["pending", "completed", "expired"] {
        enqueue(&root, state, "same-session", &server.uri()).unwrap();
        let file = path(&root, state, "same-session");
        let mut intent = read(&file).unwrap();
        if state == "completed" {
            intent.completed_at = Some(Utc::now());
        }
        if state == "expired" {
            intent.expires_at = Utc::now() - chrono::Duration::seconds(1);
        }
        write(&file, &intent, false).unwrap();
        let mut resumed = Agent::new(state.into(), state.into());
        resumed.cc_session_id = Some("same-session".into());
        resumed.registered_at = intent.created_at + chrono::Duration::seconds(1);
        open_agent_store(&root).unwrap().register(&resumed).unwrap();
        assert!(
            !is_ended(&root, &resumed).unwrap(),
            "{state} must allow a newer incarnation"
        );
    }
    drain(&root, &client(&server.uri()), Utc::now()).unwrap();
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(health(&root, Utc::now()).is_empty());
}

#[test]
fn cas_4ac8_presence_readers_share_lease_and_exclude_drain() {
    let temp = tempfile::tempdir().unwrap();
    let first = presence_lease(temp.path()).unwrap().unwrap();
    let second = presence_lease(temp.path()).unwrap();
    assert!(
        second.is_some(),
        "different presence readers must run concurrently"
    );
    let exclusive = OpenOptions::new()
        .read(true)
        .write(true)
        .open(temp.path().join(DIR).join("drain.lock"))
        .unwrap();
    assert!(exclusive.try_lock_exclusive().is_err());
    drop(first);
    drop(second);
    exclusive.try_lock_exclusive().unwrap();
    assert!(presence_lease(temp.path()).unwrap().is_none());
}

#[test]
fn cas_4ac8_completed_only_drain_needs_no_lease_or_store() {
    let env = TestEnvGuard::temp_home();
    let root = env.home();
    enqueue(root, "ended", "session", "http://example.invalid").unwrap();
    let file = path(root, "ended", "session");
    let mut intent = read(&file).unwrap();
    intent.completed_at = Some(Utc::now());
    write(&file, &intent, false).unwrap();
    drain(root, &client("http://example.invalid"), Utc::now()).unwrap();
    assert!(!root.join(DIR).join("drain.lock").exists());
}

#[test]
fn cas_4ac8_unreadable_intent_reports_io_error() {
    let env = TestEnvGuard::temp_home();
    let root = env.home();
    let mut agent = Agent::new("ended".into(), "ended".into());
    agent.cc_session_id = Some("session".into());
    let file = super::path(root, "ended", "session");
    std::fs::create_dir_all(&file).unwrap();
    let outcome =
        crate::cloud::presence::tick(&client("http://example.invalid"), root, &agent, Utc::now());
    assert!(
        matches!(outcome, crate::cloud::presence::TickAction::Failed(ref cause) if cause.contains("shutdown intent")),
        "{outcome:?}"
    );
}

#[test]
fn cas_4ac8_retry_after_is_capped_for_seconds_and_http_dates() {
    for value in [
        "86400".to_string(),
        u64::MAX.to_string(),
        (Utc::now() + chrono::Duration::days(365)).to_rfc2822(),
    ] {
        // Response construction from raw HTTP retains the real header parser.
        let raw = format!(
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: {value}\r\nContent-Length: 0\r\n\r\n"
        );
        let response: ureq::Response = raw.parse().unwrap();
        assert_eq!(
            rate_limit::retry_after(&response),
            Some(std::time::Duration::from_secs(900)),
            "{value}"
        );
        drop(response);
    }
}

#[test]
fn cas_4ac8_legacy_fence_allows_resumes_and_expiry() {
    let temp = tempfile::tempdir().unwrap();
    let mut old = ended_agent("ended", "same");
    enqueue(temp.path(), "ended", "same", "http://example.invalid").unwrap();
    let file = path(temp.path(), "ended", "same");
    let mut intent = read(&file).unwrap();
    let legacy = super::path(temp.path(), "ended", "same");
    std::fs::rename(&file, &legacy).unwrap();
    assert!(is_ended(temp.path(), &old).unwrap());
    old.registered_at = intent.created_at + chrono::Duration::seconds(1);
    assert!(!is_ended(temp.path(), &old).unwrap());
    old.registered_at = DateTime::<Utc>::UNIX_EPOCH;
    intent.expires_at = Utc::now() - chrono::Duration::seconds(1);
    write(&legacy, &intent, false).unwrap();
    assert!(!is_ended(temp.path(), &old).unwrap());
}

#[test]
fn cas_4ac8_round3_scheduled_intent_is_superseded_before_due() {
    let env = TestEnvGuard::temp_home();
    let root = init_cas_dir(env.home()).unwrap();
    enqueue(&root, "resumed", "same", "http://example.invalid").unwrap();
    let file = path(&root, "resumed", "same");
    let mut intent = read(&file).unwrap();
    intent.next_retry_at = Utc::now() + chrono::Duration::minutes(10);
    write(&file, &intent, false).unwrap();
    let mut resumed = ended_agent("resumed", "same");
    resumed.registered_at = Utc::now();
    open_agent_store(&root).unwrap().register(&resumed).unwrap();
    assert!(
        health(&root, Utc::now()).is_empty(),
        "live resume must not show pending shutdown"
    );
    drain(&root, &client("http://example.invalid"), Utc::now()).unwrap();
    assert!(read(&file).unwrap().completed_at.is_some());
}

#[test]
fn cas_4ac8_round4_wrapped_or_reparented_mcp_cannot_supersede_without_session_id() {
    let temp = tempfile::tempdir().unwrap();
    let mut old = ended_agent("pid-bound", "same");
    old.pid = Some(42);
    super::enqueue(temp.path(), &old, "same", "http://example.invalid").unwrap();
    let mut automatic = Agent::new(old.id.clone(), "automatic".into());
    automatic.pid = Some(99);
    automatic.ppid = Some(42);
    assert!(is_ended(temp.path(), &automatic).unwrap());
    automatic.ppid = Some(43);
    assert!(is_ended(temp.path(), &automatic).unwrap(), "PID mismatch is not proof of a new harness session");
}

#[test]
fn cas_4ac8_round4_future_supersession_does_not_need_exclusive_lease() {
    let env = TestEnvGuard::temp_home();
    let root = init_cas_dir(env.home()).unwrap();
    enqueue(&root, "future-resume", "same", "http://example.invalid").unwrap();
    let file = path(&root, "future-resume", "same");
    let mut intent = read(&file).unwrap();
    intent.next_retry_at = Utc::now() + chrono::Duration::minutes(10);
    write(&file, &intent, false).unwrap();
    let mut resumed = ended_agent("future-resume", "same");
    resumed.registered_at = Utc::now();
    open_agent_store(&root).unwrap().register(&resumed).unwrap();
    let _reader = presence_lease(&root).unwrap().unwrap();
    drain(&root, &client("http://example.invalid"), Utc::now()).unwrap();
    assert!(read(&file).unwrap().completed_at.is_some(), "future supersession must proceed while presence readers hold the lease");
}

#[test]
fn cas_4ac8_round4_directory_failure_does_not_fence_unrelated_agent() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join(DIR), "not a directory").unwrap();
    let agent = ended_agent("unrelated", "live");
    assert!(!is_ended(temp.path(), &agent).expect("unrelated presence must survive a failed intent listing"));
}
