//! The factory's mailbox duty, including startup recovery and visible health.
use super::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize)]
struct MailboxHealth {
    written_at: chrono::DateTime<chrono::Utc>,
    receiving: bool,
    cause: String,
    #[serde(default)]
    next_retry_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    warnings: Vec<String>,
}

fn health_path(root: &Path, session: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    root.join("factory-daemon").join(format!(
        "{:x}.peer-mailbox.json",
        Sha256::digest(session.as_bytes())
    ))
}

/// State reported by the real polling duty, rather than inferred from login.
pub fn mailbox_status_section(root: &Path, session: Option<&str>) -> String {
    let Some(session) = session.filter(|s| !s.is_empty()) else {
        return "Peer mailbox: not receiving: no factory daemon session.\n".into();
    };
    match std::fs::read(health_path(root, session))
        .ok()
        .and_then(|b| serde_json::from_slice::<MailboxHealth>(&b).ok())
    {
        Some(health)
            if chrono::Utc::now()
                .signed_duration_since(health.written_at)
                .num_seconds()
                <= 90
                || health.next_retry_at.is_some_and(|at| {
                    let cap = health.written_at
                        + chrono::Duration::seconds(
                            super::super::rate_limit::MAX_RETRY_AFTER.as_secs() as i64,
                        );
                    at.min(cap) > chrono::Utc::now()
                }) =>
        {
            if health.receiving {
                let mut text = "Peer mailbox: receiving.\n".to_string();
                for warning in health.warnings {
                    text.push_str(&format!("Warning: {warning}\n"));
                }
                text
            } else {
                let retry = health
                    .next_retry_at
                    .map(|at| format!("automatic at {}", at.to_rfc3339()))
                    .unwrap_or_else(|| "automatic with backoff, at most 30s".into());
                format!(
                    "Peer mailbox: not receiving: {}\nRetry: {retry}.\n",
                    health.cause
                )
            }
        }
        Some(_) => "Peer mailbox: not receiving: daemon mailbox status is stale.\n".into(),
        None => "Peer mailbox: not receiving: daemon has not reported a mailbox poll.\n".into(),
    }
}

/// Allocated even before login; credentials, canonical id and aliases reload
/// on every tick, so login, token replacement and identity repair recover.
pub struct PeerMailboxRuntime {
    cas_dir: PathBuf,
    factory_session: String,
    consumer_id: String,
}

impl PeerMailboxRuntime {
    /// Always returns a runtime; unavailable prerequisites are a retry state.
    pub fn start(cas_dir: &Path, factory_session: &str) -> Option<Self> {
        let runtime = Self {
            cas_dir: cas_dir.to_path_buf(),
            factory_session: factory_session.to_string(),
            consumer_id: format!(
                "{}:{factory_session}",
                cas_types::Agent::get_or_generate_machine_id()
            )
            .chars()
            .take(200)
            .collect(),
        };
        runtime.record_health(false, "waiting for first mailbox poll", &[], None);
        Some(runtime)
    }

    fn record_health(
        &self,
        receiving: bool,
        cause: &str,
        warnings: &[String],
        retry_after: Option<Duration>,
    ) {
        let path = health_path(&self.cas_dir, &self.factory_session);
        let health = MailboxHealth {
            written_at: chrono::Utc::now(),
            receiving,
            cause: cause.into(),
            next_retry_at: retry_after
                .map(|delay| delay.min(super::super::rate_limit::MAX_RETRY_AFTER))
                .and_then(|delay| chrono::Duration::from_std(delay).ok())
                .and_then(|delay| chrono::Utc::now().checked_add_signed(delay)),
            warnings: warnings.to_vec(),
        };
        let result = (|| -> anyhow::Result<()> {
            std::fs::create_dir_all(path.parent().expect("health path has parent"))?;
            let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
            std::fs::write(&tmp, serde_json::to_vec(&health)?)?;
            std::fs::rename(&tmp, &path)?;
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(%error,"could not persist peer mailbox health");
        }
    }

    pub fn tick_blocking(&self) -> DeliveryReport {
        let report = self.tick_inner();
        self.record_health(
            report.errors.is_empty(),
            &report.errors.join("; "),
            &report.warnings,
            report.retry_after,
        );
        report
    }

    fn tick_inner(&self) -> DeliveryReport {
        // The project outbox survives changes of supervisor/session. Its row
        // leases prevent other factory daemons from retrying the same send.
        // An outbound error must not prevent this daemon claiming inbound mail.
        let outbox_error = outbox::tick(&self.cas_dir, &self.factory_session).err();
        let run = || -> Result<DeliveryReport, String> {
            let config = crate::cloud::CloudConfig::load_from_cas_dir(&self.cas_dir)
                .map_err(|error| format!("could not load cloud config: {error}"))?;
            let token = config
                .token
                .as_deref()
                .filter(|token| !token.is_empty())
                .ok_or("not logged in to Cassy Cloud (`cas login`)")?;
            let mailbox = HttpPeerMailbox::new(&config.endpoint, token);
            let canonical_id = crate::cloud::resolve_canonical_id(&self.cas_dir)
                .ok_or("project has no canonical id")?;
            let queue = crate::store::open_prompt_queue_store(&self.cas_dir)
                .map_err(|e| format!("could not open prompt queue: {e}"))?;
            let agents = crate::store::open_agent_store(&self.cas_dir)
                .map_err(|e| format!("could not open agent store: {e}"))?;
            // cas-4736: record epic split proposals and answers before ack.
            let cas_dir = self.cas_dir.clone();
            let history = match retired_recipients(&self.cas_dir, &self.factory_session) {
                Ok(history) => history,
                Err(error) => RecipientHistory {
                    warning: Some(format!("retired peer identity lookup failed: {error}")),
                    ..Default::default()
                },
            };
            let mut report = deliver_claimed_with_history(
                &mailbox,
                queue.as_ref(),
                agents.as_ref(),
                &canonical_id,
                &crate::cloud::project_aliases_from_config_toml(&self.cas_dir),
                &self.factory_session,
                &self.consumer_id,
                &history,
                &|recipient, message| crate::cloud::epic_split::expiry_notice_key(&self.cas_dir, recipient, message),
                &move |recipient, message| {
                    match crate::cloud::epic_split::ingest(&cas_dir, recipient, message)? {
                        crate::cloud::epic_split::Ingested::Ignored(why) => {
                            tracing::info!(message = %message.id, %why, "epic split message ignored");
                        }
                        crate::cloud::epic_split::Ingested::NotASplit => {}
                        recorded => {
                            tracing::info!(message = %message.id, ?recorded, "epic split recorded")
                        }
                    }
                    Ok(())
                },
            );
            report.retry_after = mailbox.retry_after();
            Ok(report)
        };
        let mut report = run().unwrap_or_else(|error| DeliveryReport {
            errors: vec![error],
            ..Default::default()
        });
        if let Some(error) = outbox_error {
            report.errors.push(format!("peer outbox: {error}"));
        }
        if let Err(error) = crate::cloud::epic_split::reconcile(
            &self.cas_dir, &self.factory_session, chrono::Utc::now(),
        ) {
            report.errors.push(format!("epic split reconciliation: {error}"));
        }
        report
    }
}

/// Nonblocking daemon duty: at most one claim task, retry with bounded
/// exponential backoff, and log changes in the named failure cause.
/// FactoryDaemon and integration tests call this same pass.
pub struct PeerMailboxPoller {
    runtime: std::sync::Arc<PeerMailboxRuntime>,
    next_poll: Instant,
    task: Option<tokio::task::JoinHandle<DeliveryReport>>,
    failures: u32,
    last_error: String,
    last_warning: String,
}

impl PeerMailboxPoller {
    pub fn new(root: &Path, session: &str) -> Self {
        Self {
            runtime: std::sync::Arc::new(
                PeerMailboxRuntime::start(root, session).expect("runtime always exists"),
            ),
            next_poll: Instant::now(),
            task: None,
            failures: 0,
            last_error: String::new(),
            last_warning: String::new(),
        }
    }
    pub async fn poll(&mut self, now: Instant) -> Option<DeliveryReport> {
        if self
            .task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
        {
            let report = self
                .task
                .take()
                .expect("finished task")
                .await
                .unwrap_or_else(|e| DeliveryReport {
                    errors: vec![format!("mailbox poll task failed: {e}")],
                    ..Default::default()
                });
            let error = report.errors.join("; ");
            let delay = if error.is_empty() {
                self.failures = 0;
                POLL_INTERVAL
            } else {
                self.failures = self.failures.saturating_add(1);
                Duration::from_secs((5u64 << self.failures.saturating_sub(1).min(3)).min(30))
            };
            let delay = delay
                .max(report.retry_after.unwrap_or_default())
                .min(super::super::rate_limit::MAX_RETRY_AFTER);
            if error != self.last_error {
                if error.is_empty() {
                    tracing::info!("peer mailbox receiving");
                } else {
                    tracing::warn!(cause=%error,"peer mailbox not receiving; retrying with backoff");
                }
                self.last_error = error;
            }
            let warning = report.warnings.join("; ");
            if warning != self.last_warning {
                if !warning.is_empty() {
                    tracing::warn!(cause=%warning, "peer mailbox receiving with recovery warning");
                }
                self.last_warning = warning;
            }
            self.runtime.record_health(
                report.errors.is_empty(),
                &report.errors.join("; "),
                &report.warnings,
                (!report.errors.is_empty()).then_some(delay),
            );
            self.next_poll = now + delay;
            return Some(report);
        }
        if self.task.is_none() && now >= self.next_poll {
            let runtime = std::sync::Arc::clone(&self.runtime);
            self.task = Some(tokio::task::spawn_blocking(move || runtime.tick_blocking()));
        }
        None
    }
}

#[cfg(test)]
mod rate_limit_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn cas_4ac8_mailbox_poller_honors_retry_after_outside_http_budget() {
        let env = crate::test_support::TestEnvGuard::temp_home();
        let root = crate::store::init_cas_dir(env.home()).unwrap();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/peer-messages/claim"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "600"))
            .expect(1)
            .mount(&server)
            .await;
        crate::cloud::set_canonical_id_in_config_toml(&root, "github.com/acme/widgets").unwrap();
        crate::cloud::CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".into()),
            ..Default::default()
        }
        .save_to_cas_dir(&root)
        .unwrap();
        let mut agent = crate::types::Agent::new("supervisor".into(), "supervisor".into());
        agent.role = AgentRole::Supervisor;
        agent.pid = Some(std::process::id());
        crate::mcp::daemon::stamp_pid_fingerprint(&mut agent, std::process::id());
        agent.factory_session = Some("factory-retry".into());
        crate::store::open_agent_store(&root)
            .unwrap()
            .register(&agent)
            .unwrap();
        let mut poller = PeerMailboxPoller::new(&root, "factory-retry");
        let now = Instant::now();
        assert!(poller.poll(now).await.is_none());
        let deadline = Instant::now() + Duration::from_secs(2);
        let report = loop {
            if let Some(report) = poller.poll(Instant::now()).await {
                break report;
            }
            assert!(Instant::now() < deadline, "off-loop poll never completed");
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert!(
            report
                .errors
                .iter()
                .any(|cause| cause.contains("rate limited"))
        );
        poller.poll(now + Duration::from_secs(10)).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "retried before Retry-After"
        );
    }

    #[test]
    fn cas_4ac8_waiting_rate_limit_health_does_not_become_stale_before_retry() {
        let temp = tempfile::tempdir().unwrap();
        let file = health_path(temp.path(), "retry-session");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            file,
            serde_json::to_vec(&serde_json::json!({
                "written_at": chrono::Utc::now() - chrono::Duration::minutes(2),
                "receiving": false,
                "cause": "peer message ack: rate limited (429)",
                "next_retry_at": chrono::Utc::now() + chrono::Duration::minutes(5),
            }))
            .unwrap(),
        )
        .unwrap();
        let status = mailbox_status_section(temp.path(), Some("retry-session"));
        assert!(status.contains("rate limited"), "{status}");
        assert!(!status.contains("stale"), "{status}");
    }
}

#[cfg(test)]
mod bounded_health_tests {
    use super::*;

    #[test]
    fn cas_4ac8_persisted_huge_cooldown_cannot_keep_dead_daemon_fresh() {
        let temp = tempfile::tempdir().unwrap();
        let file = health_path(temp.path(), "huge-delay");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            file,
            serde_json::to_vec(&serde_json::json!({
                "written_at": chrono::Utc::now() - chrono::Duration::minutes(20),
                "receiving": false,
                "cause": "rate limited (429)",
                "next_retry_at": chrono::DateTime::<chrono::Utc>::MAX_UTC,
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(mailbox_status_section(temp.path(), Some("huge-delay")).contains("stale"));
        let runtime = PeerMailboxRuntime::start(temp.path(), "new-delay").unwrap();
        runtime.record_health(false, "rate limited (429)", &[], Some(Duration::MAX));
        let health: MailboxHealth =
            serde_json::from_slice(&std::fs::read(health_path(temp.path(), "new-delay")).unwrap())
                .unwrap();
        let delay = health.next_retry_at.unwrap() - health.written_at;
        assert!(delay >= chrono::Duration::seconds(899) && delay <= chrono::Duration::seconds(901));
    }
}
