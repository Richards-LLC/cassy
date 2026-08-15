use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::{Context, Result, ensure};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

use super::death::diagnose_disconnect;
use super::{
    DaemonExitEvidenceStore, DaemonIdentity, MachineEventBus, ProxyFrame, SessionMultiplexer,
    ViewerReceiver,
};
use crate::ui::factory::{
    COMMANDER_REPLAY_BYTES_PER_PANE, ClientMessage, DaemonMessage,
};

// Existing session daemons keep running their original binary across a hub
// upgrade. Accept the observed 44.7 MB legacy Welcome with narrow headroom so
// the hub can compact it before relaying; newly-started daemons emit bounded
// replay directly.
const COMMANDER_LEGACY_UPSTREAM_MAX_MESSAGE_BYTES: usize = 48 * 1024 * 1024;
// The browser-facing canonical snapshot must stay far below the legacy input.
// At 64 KiB per active pane, 8 MiB allows a large multi-worker session while
// still catching a return to unbounded/stale-pane replay.
const COMMANDER_RELAY_MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

fn compact_welcome_replay(message: &mut DaemonMessage) -> Option<(usize, usize)> {
    let DaemonMessage::Welcome {
        state, scrollback, ..
    } = message
    else {
        return None;
    };
    let scrollback = scrollback.as_mut()?;
    let active: std::collections::HashSet<&str> =
        state.panes.iter().map(|pane| pane.id.as_str()).collect();
    let before = scrollback
        .values()
        .flat_map(|chunks| chunks.iter())
        .map(Vec::len)
        .sum();
    scrollback.retain(|pane_id, _| active.contains(pane_id.as_str()));
    for chunks in scrollback.values_mut() {
        let total = chunks.iter().map(Vec::len).sum::<usize>();
        let mut skip = total.saturating_sub(COMMANDER_REPLAY_BYTES_PER_PANE);
        let mut tail = Vec::with_capacity(total.min(COMMANDER_REPLAY_BYTES_PER_PANE));
        for chunk in chunks.iter() {
            let start = skip.min(chunk.len());
            skip -= start;
            tail.extend_from_slice(&chunk[start..]);
        }
        *chunks = vec![tail];
    }
    let after = scrollback
        .values()
        .flat_map(|chunks| chunks.iter())
        .map(Vec::len)
        .sum();
    Some((before, after))
}

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
    let config = WebSocketConfig {
        max_message_size: Some(COMMANDER_LEGACY_UPSTREAM_MAX_MESSAGE_BYTES),
        max_frame_size: Some(COMMANDER_LEGACY_UPSTREAM_MAX_MESSAGE_BYTES),
        ..WebSocketConfig::default()
    };
    let (mut socket, _) = connect_async_with_config(&url, Some(config), false)
        .await
        .with_context(|| format!("connect to daemon for session '{session}'"))?;
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
        let upstream_bytes = match message {
            Message::Binary(bytes) => bytes,
            Message::Text(text) => text.as_bytes().to_vec(),
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
        };
        let mut daemon: DaemonMessage = serde_json::from_slice(&upstream_bytes)
            .context("daemon sent an invalid Commander protocol frame")?;
        let bytes = if let Some((replay_before, replay_after)) = compact_welcome_replay(&mut daemon)
        {
            let compacted = serde_json::to_vec(&daemon)?;
            ensure!(
                compacted.len() <= COMMANDER_RELAY_MAX_MESSAGE_BYTES,
                "bounded Commander Welcome is still too large: {} > {} bytes",
                compacted.len(),
                COMMANDER_RELAY_MAX_MESSAGE_BYTES
            );
            tracing::info!(
                upstream_bytes = upstream_bytes.len(),
                relay_bytes = compacted.len(),
                replay_before,
                replay_after,
                "compacted Commander Welcome for relay"
            );
            compacted
        } else {
            upstream_bytes
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::factory::{PaneInfo, PaneKind, SessionState};

    #[test]
    fn welcome_replay_drops_departed_panes_and_keeps_a_bounded_active_tail() {
        let mut message = DaemonMessage::Welcome {
            session_name: "factory-a".into(),
            state: SessionState {
                focused_pane: Some("active".into()),
                panes: vec![PaneInfo {
                    id: "active".into(),
                    kind: PaneKind::Supervisor,
                    focused: true,
                    title: "active".into(),
                    exited: false,
                }],
                epic_id: None,
                epic_title: None,
                cols: 80,
                rows: 24,
            },
            scrollback: Some(HashMap::from([
                (
                    "active".into(),
                    vec![vec![b'a'; COMMANDER_REPLAY_BYTES_PER_PANE + 17]],
                ),
                ("departed".into(), vec![vec![b'x'; 1024]]),
            ])),
            protocol_version: 2,
            capabilities: Vec::new(),
        };

        let (before, after) = compact_welcome_replay(&mut message).unwrap();

        assert_eq!(before, COMMANDER_REPLAY_BYTES_PER_PANE + 17 + 1024);
        assert_eq!(after, COMMANDER_REPLAY_BYTES_PER_PANE);
        let DaemonMessage::Welcome {
            scrollback: Some(scrollback),
            ..
        } = message
        else {
            panic!("expected Welcome scrollback");
        };
        assert_eq!(
            scrollback.keys().map(String::as_str).collect::<Vec<_>>(),
            ["active"]
        );
        assert_eq!(scrollback["active"][0].len(), COMMANDER_REPLAY_BYTES_PER_PANE);
    }
}
