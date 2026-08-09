use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::tungstenite::Message;

use super::death::diagnose_disconnect;
use super::{
    DaemonExitEvidenceStore, DaemonIdentity, MachineEventBus, ProxyFrame, SessionMultiplexer,
    ViewerReceiver,
};
use crate::ui::factory::{ClientMessage, DaemonMessage};

struct UpstreamSlot {
    running: AtomicBool,
    starts: AtomicUsize,
    sender: Mutex<Option<mpsc::Sender<ClientMessage>>>,
}

#[derive(Clone)]
pub struct DaemonConnector {
    mux: SessionMultiplexer,
    events: MachineEventBus,
    slots: Arc<Mutex<HashMap<String, Arc<UpstreamSlot>>>>,
    exit_evidence: Option<DaemonExitEvidenceStore>,
}

impl DaemonConnector {
    pub fn new(mux: SessionMultiplexer, events: MachineEventBus) -> Self {
        Self {
            mux,
            events,
            slots: Arc::new(Mutex::new(HashMap::new())),
            exit_evidence: DaemonExitEvidenceStore::default_for_user(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_exit_evidence_store(mut self, store: DaemonExitEvidenceStore) -> Self {
        self.exit_evidence = Some(store);
        self
    }

    pub async fn attach<I, S>(
        &self,
        session: &str,
        port: u16,
        panes: I,
        identity: Option<DaemonIdentity>,
    ) -> Result<ViewerReceiver>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let receiver = self.mux.subscribe(session, panes).await;
        let slot = {
            let mut slots = self.slots.lock().await;
            slots
                .entry(session.to_owned())
                .or_insert_with(|| {
                    Arc::new(UpstreamSlot {
                        running: AtomicBool::new(false),
                        starts: AtomicUsize::new(0),
                        sender: Mutex::new(None),
                    })
                })
                .clone()
        };
        if slot
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            slot.starts.fetch_add(1, Ordering::Relaxed);
            let (sender, receiver) = mpsc::channel(64);
            *slot.sender.lock().await = Some(sender);
            let session = session.to_owned();
            let mux = self.mux.clone();
            let events = self.events.clone();
            let exit_evidence = self.exit_evidence.clone();
            tokio::spawn(async move {
                if let Err(error) = run_upstream(&session, port, &mux, &events, receiver).await {
                    tracing::warn!(session, %error, "Commander hub daemon upstream closed");
                }
                slot.running.store(false, Ordering::Release);
                let diagnostic =
                    diagnose_disconnect(identity.as_ref(), exit_evidence.as_ref()).await;
                events.daemon_disconnected(&session, diagnostic);
            });
        }
        Ok(receiver)
    }

    pub async fn upstream_connection_count(&self, session: &str) -> usize {
        self.slots
            .lock()
            .await
            .get(session)
            .map_or(0, |slot| slot.starts.load(Ordering::Relaxed))
    }

    pub async fn send(&self, session: &str, message: ClientMessage) -> Result<()> {
        let slot = self
            .slots
            .lock()
            .await
            .get(session)
            .cloned()
            .context("session has no Commander upstream")?;
        let sender = slot
            .sender
            .lock()
            .await
            .clone()
            .context("session upstream is not ready")?;
        sender
            .send(message)
            .await
            .context("session upstream closed")
    }
}

async fn run_upstream(
    session: &str,
    port: u16,
    mux: &SessionMultiplexer,
    events: &MachineEventBus,
    mut controls: mpsc::Receiver<ClientMessage>,
) -> Result<()> {
    let url = format!("ws://127.0.0.1:{port}");
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .with_context(|| format!("connect to daemon for session '{session}'"))?;
    socket
        .send(Message::Binary(serde_json::to_vec(
            &ClientMessage::Attach {
                request_scrollback: true,
            },
        )?))
        .await?;

    loop {
        let message = tokio::select! {
            message = socket.next() => match message {
                Some(message) => message?,
                None => break,
            },
            control = controls.recv() => match control {
                Some(control) => {
                    socket.send(Message::Binary(serde_json::to_vec(&control)?)).await?;
                    continue;
                }
                None => break,
            }
        };
        let bytes = match message {
            Message::Binary(bytes) => bytes,
            Message::Text(text) => text.as_bytes().to_vec(),
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
        };
        let daemon: DaemonMessage = serde_json::from_slice(&bytes)
            .context("daemon sent an invalid Commander protocol frame")?;
        events.observe_daemon(session, &daemon);
        let pane_id = match &daemon {
            DaemonMessage::Output { pane_id, .. }
            | DaemonMessage::PaneExited { pane_id, .. }
            | DaemonMessage::PaneRemoved { pane_id } => Some(pane_id.clone()),
            DaemonMessage::PaneAdded { pane } => Some(pane.id.clone()),
            _ => None,
        };
        let frame = ProxyFrame { bytes, pane_id };
        if matches!(daemon, DaemonMessage::Welcome { .. }) {
            mux.publish_snapshot(session, frame).await?;
        } else {
            mux.publish(session, frame).await?;
        }
    }
    Ok(())
}
