//! Lifecycle-owned legacy drain. GUI control only queues a durable pause;
//! unfinished tick/sweep handles are never awaited on the GUI input path.
use super::violet_activity::{PollReport, SupervisorPresence, SweepRequest};
use crate::coordination::binding_intake::{IntakeJournal, IntakeScope, IntakeState, PauseRecord};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::task::JoinHandle;
use uuid::Uuid;

pub(crate) type TickResult = (PollReport, Vec<SweepRequest>, Option<SupervisorPresence>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PauseRequest {
    pub operation_id: Uuid,
    pub nonce: Uuid,
    pub canonical_cas_dir: PathBuf,
    pub factory_session: String,
    pub scope: IntakeScope,
    pub expected_generation: u64,
}

/// Positive identity evidence, unlike the intentionally tolerant liveness test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeIdentity {
    pub canonical_cas_dir: PathBuf,
    pub factory_session: String,
    pub pid: u32,
    pub pid_starttime: u64,
    pub build: String,
}
impl RuntimeIdentity {
    pub(crate) fn current(cas_dir: &Path, factory_session: &str) -> Result<Self, String> {
        let pid = std::process::id();
        let identity = Self {
            canonical_cas_dir: cas_dir.canonicalize().map_err(|e| e.to_string())?,
            factory_session: factory_session.into(),
            pid,
            pid_starttime: crate::mcp::daemon::read_pid_starttime(pid)
                .ok_or("runtime process start identity unavailable")?,
            build: option_env!("CAS_GIT_HASH").unwrap_or("unknown").into(),
        };
        identity.validate()?;
        Ok(identity)
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.factory_session.is_empty()
            || self.pid == 0
            || self.pid_starttime == 0
            || self.build.len() < 7
            || !self.build.bytes().all(|b| b.is_ascii_hexdigit())
            || self.canonical_cas_dir.canonicalize().ok().as_ref() != Some(&self.canonical_cas_dir)
            || crate::mcp::daemon::read_pid_starttime(self.pid) != Some(self.pid_starttime)
        {
            return Err("unknown or stale runtime identity; update/restart must be coordinated before reconciliation".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrainAcknowledgement {
    pub nonce: Uuid,
    pub pause: PauseRecord,
    pub runtime: RuntimeIdentity,
}
impl DrainAcknowledgement {
    /// The caller supplies identity from independently resolved local session
    /// metadata, never the responding peer's own assertion or PATH version.
    pub fn verify(
        &self,
        request: &PauseRequest,
        expected: &RuntimeIdentity,
        capabilities: &[crate::ui::factory::protocol::ProtocolCapability],
    ) -> Result<(), String> {
        use crate::ui::factory::protocol::ProtocolCapability;
        if !capabilities.contains(&ProtocolCapability::BindingIntakeControlV1) {
            return Err("runtime lacks binding_intake_control_v1; reconciliation refused".into());
        }
        expected.validate()?;
        if &self.runtime != expected
            || self.nonce != request.nonce
            || request.nonce.is_nil()
            || self.pause.operation_id != request.operation_id
            || self.pause.scope != request.scope
            || self.pause.generation
                != request
                    .expected_generation
                    .checked_add(1)
                    .ok_or("generation overflow")?
            || self.runtime.canonical_cas_dir != request.canonical_cas_dir
            || self.runtime.factory_session != request.factory_session
        {
            return Err("intake drain acknowledgement identity/scope/generation mismatch".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntakeReply {
    pub nonce: Uuid,
    pub state: Option<IntakeState>,
    pub acknowledgement: Option<DrainAcknowledgement>,
    pub error: Option<String>,
}
impl IntakeReply {
    pub(crate) fn refused(nonce: Uuid, error: impl Into<String>) -> Self {
        Self {
            nonce,
            state: None,
            acknowledgement: None,
            error: Some(error.into()),
        }
    }
}

struct PendingPause {
    client_id: usize,
    nonce: Uuid,
    pause: PauseRecord,
    runtime: RuntimeIdentity,
}

#[derive(Default)]
pub(crate) struct IntakeController {
    pub tick: Option<JoinHandle<TickResult>>,
    /// Includes the nested blocking apply. Never replace/drop an unreaped task.
    pub sweep: Option<JoinHandle<Result<(), String>>>,
    pending: Vec<PendingPause>,
    failure: Option<String>,
}
impl IntakeController {
    pub fn pause(
        &mut self,
        client_id: usize,
        request: PauseRequest,
        runtime: RuntimeIdentity,
    ) -> Result<(), String> {
        if self.pending.len() >= 64 {
            return Err("too many pending intake controls; reread status".into());
        }
        runtime.validate()?;
        if request.nonce.is_nil()
            || request.factory_session != runtime.factory_session
            || request.canonical_cas_dir != runtime.canonical_cas_dir
        {
            return Err("pause request targets another runtime/root/session".into());
        }
        // Publication precedes draining/control completion. Disconnect/drop does
        // not roll it back. No method here resumes or clears a pause.
        let pause = IntakeJournal::new(&runtime.canonical_cas_dir).pause(
            request.scope,
            request.operation_id,
            request.expected_generation,
        )?;
        self.pending.push(PendingPause {
            client_id,
            nonce: request.nonce,
            pause,
            runtime,
        });
        Ok(())
    }
    /// A failed tick/sweep refuses only the pauses waiting on it. With none
    /// pending it proves nothing about a later pause, so it is not latched.
    pub fn failed(&mut self, error: impl Into<String>) {
        if !self.pending.is_empty() {
            self.failure = Some(error.into());
        }
    }
    pub async fn reap_sweep(&mut self) {
        if self.sweep.as_ref().is_some_and(JoinHandle::is_finished) {
            match self.sweep.take().expect("finished sweep").await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => self.failed(error),
                Err(error) => self.failed(format!("legacy sweep/apply did not finish: {error}")),
            }
        }
    }
    /// Called only by the lifecycle after it has consumed completed tick output.
    /// Returning no reply while busy keeps GUI traffic and the main loop live.
    pub fn replies(&mut self) -> Vec<(usize, IntakeReply)> {
        // Both owned handles must have been consumed, including nested apply.
        if self.tick.is_some() || self.sweep.is_some() {
            return Vec::new();
        }
        // Reported once, to the pauses it affected; the next pause starts clean.
        let failure = self.failure.take();
        self.pending
            .drain(..)
            .map(|pending| {
                let result = (|| {
                    if let Some(error) = &failure {
                        return Err(error.clone());
                    }
                    pending.runtime.validate()?;
                    let state = IntakeJournal::new(&pending.runtime.canonical_cas_dir).read()?;
                    if !state.pauses.contains(&pending.pause) {
                        return Err("durable pause changed before drain".into());
                    }
                    Ok(state)
                })();
                let reply = match result {
                    Ok(state) => IntakeReply {
                        nonce: pending.nonce,
                        state: Some(state),
                        acknowledgement: Some(DrainAcknowledgement {
                            nonce: pending.nonce,
                            pause: pending.pause,
                            runtime: pending.runtime,
                        }),
                        error: None,
                    },
                    Err(error) => IntakeReply::refused(pending.nonce, error),
                };
                (pending.client_id, reply)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::factory::{ClientMessage, DaemonMessage};
    use futures_util::{SinkExt, StreamExt};
    #[tokio::test]
    async fn cas_9021_ws_and_trusted_relay_refuse_local_intake_control() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let mut daemon = super::super::provisioning_tests::daemon(temp.path());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (client, accepted) = tokio::join!(
            tokio::net::TcpStream::connect(listener.local_addr().unwrap()),
            listener.accept()
        );
        use tokio_tungstenite::{WebSocketStream, tungstenite::protocol::Role};
        let server =
            WebSocketStream::from_raw_socket(accepted.unwrap().0, Role::Server, None).await;
        let mut remote =
            WebSocketStream::from_raw_socket(client.unwrap(), Role::Client, None).await;
        let (sink, stream) = server.split();
        daemon.ws_clients.insert(
            0,
            crate::ui::factory::daemon::WsConnection {
                sink,
                stream,
                pane_sizes: Default::default(),
                relay_trusted: false,
            },
        );
        let identity = RuntimeIdentity::current(temp.path(), "session").unwrap();
        let request = PauseRequest {
            operation_id: Uuid::new_v4(),
            nonce: Uuid::new_v4(),
            canonical_cas_dir: identity.canonical_cas_dir,
            factory_session: "session".into(),
            scope: IntakeScope {
                team_id: Uuid::new_v4().to_string(),
                canonical_project_id: "project".into(),
                channel_id: "C123".into(),
            },
            expected_generation: 0,
        };
        assert!(
            !crate::ui::factory::protocol::daemon_capabilities().contains(
                &crate::ui::factory::protocol::ProtocolCapability::BindingIntakeControlV1
            )
        );
        for trusted in [false, true] {
            daemon.ws_clients.get_mut(&0).unwrap().relay_trusted = trusted;
            for message in [
                ClientMessage::BindingIntakePause {
                    request: request.clone(),
                },
                ClientMessage::BindingIntakeStatus {
                    nonce: request.nonce,
                },
            ] {
                assert_eq!(
                    crate::hub::required_scope(&message),
                    None,
                    "no remote scope exposes local control"
                );
                daemon.handle_ws_message(0, message).await;
                daemon.flush_ws_client_output().await;
                let bytes = tokio::time::timeout(std::time::Duration::from_secs(2), remote.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
                    .into_data();
                match serde_json::from_slice::<DaemonMessage>(&bytes).unwrap() {
                    DaemonMessage::BindingIntake { reply } => {
                        assert_eq!(reply.nonce, request.nonce);
                        assert!(reply.error.unwrap().contains("WS/relay refused"));
                        assert!(reply.acknowledgement.is_none());
                    }
                    other => panic!("{other:?}"),
                }
                assert!(
                    !IntakeJournal::new(temp.path()).path().exists(),
                    "remote control must never publish policy"
                );
            }
        }
        remote.close(None).await.unwrap();
    }
}
