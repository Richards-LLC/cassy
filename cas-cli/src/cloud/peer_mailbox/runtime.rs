//! The factory's mailbox duty, including startup recovery and visible health.
use super::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize)]
struct MailboxHealth {
    written_at: chrono::DateTime<chrono::Utc>,
    receiving: bool,
    cause: String,
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
                <= 90 =>
        {
            if health.receiving {
                "Peer mailbox: receiving.\n".into()
            } else {
                format!(
                    "Peer mailbox: not receiving: {}\nRetry: automatic with backoff, at most 30s.\n",
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
        runtime.record_health(false, "waiting for first mailbox poll");
        Some(runtime)
    }

    fn record_health(&self, receiving: bool, cause: &str) {
        let path = health_path(&self.cas_dir, &self.factory_session);
        let health = MailboxHealth {
            written_at: chrono::Utc::now(),
            receiving,
            cause: cause.into(),
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
        self.record_health(report.errors.is_empty(), &report.errors.join("; "));
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
            let has_recipient = agents
                .list(None)
                .map_err(|e| e.to_string())?
                .iter()
                .any(|agent| {
                    agent.role == AgentRole::Supervisor
                        && agent.factory_session.as_deref() == Some(&self.factory_session)
                        && matches!(agent.status, AgentStatus::Active | AgentStatus::Idle)
                });
            if !has_recipient {
                return Err("no live supervisor in this factory session".into());
            }
            // cas-4736: record epic split proposals and answers before ack.
            let cas_dir = self.cas_dir.clone();
            Ok(deliver_claimed_with(
                &mailbox,
                queue.as_ref(),
                agents.as_ref(),
                &canonical_id,
                &crate::cloud::project_aliases_from_config_toml(&self.cas_dir),
                &self.factory_session,
                &self.consumer_id,
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
            ))
        };
        let mut report = run().unwrap_or_else(|error| DeliveryReport {
            errors: vec![error],
            ..Default::default()
        });
        if let Some(error) = outbox_error {
            report.errors.push(format!("peer outbox: {error}"));
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
            if error != self.last_error {
                if error.is_empty() {
                    tracing::info!("peer mailbox receiving");
                } else {
                    tracing::warn!(cause=%error,"peer mailbox not receiving; retrying with backoff");
                }
                self.last_error = error;
            }
            self.runtime
                .record_health(report.errors.is_empty(), &report.errors.join("; "));
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
