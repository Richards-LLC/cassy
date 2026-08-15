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
    COMMANDER_REPLAY_BYTES_PER_PANE, ClientMessage, DaemonMessage, PaneKind, ProtocolCapability,
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
const COMMANDER_WELCOME_METADATA_WARN_BYTES: usize = 32 * 1024;
pub(super) const COMMANDER_WELCOME_METADATA_HARD_BYTES: usize = 64 * 1024;
const COMMANDER_KEYFRAME_WARN_BYTES: usize = 128 * 1024;
const COMMANDER_KEYFRAME_BUDGET_BYTES: usize = 256 * 1024;
const COMMANDER_KEYFRAME_HARD_BYTES: usize = 1024 * 1024;
const COMMANDER_PRE_SUPERVISOR_WARN_BYTES: usize = 256 * 1024;
const COMMANDER_PRE_SUPERVISOR_HARD_BYTES: usize = 512 * 1024;

#[derive(Debug, PartialEq, Eq)]
enum WelcomePreparation {
    Legacy { replay_before: usize, replay_after: usize },
    Metadata { removed_content_bytes: usize },
}

fn prepare_welcome_for_relay(message: &mut DaemonMessage) -> Option<WelcomePreparation> {
    let DaemonMessage::Welcome {
        state,
        scrollback,
        protocol_version,
        capabilities,
        ..
    } = message
    else {
        return None;
    };
    let supports_keyframes = *protocol_version >= 3
        && capabilities.contains(&ProtocolCapability::AuthoritativePaneKeyframes);
    if supports_keyframes {
        let removed_content_bytes = scrollback
            .take()
            .into_iter()
            .flat_map(|buffers| buffers.into_values())
            .flatten()
            .map(|chunk| chunk.len())
            .sum();
        return Some(WelcomePreparation::Metadata {
            removed_content_bytes,
        });
    }
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
    Some(WelcomePreparation::Legacy {
        replay_before: before,
        replay_after: after,
    })
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
    let mut supervisor_pane_id: Option<String> = None;
    let mut pre_supervisor_bytes = 0usize;
    let mut supervisor_ready = false;
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
        let preparation = prepare_welcome_for_relay(&mut daemon);
        let bytes = if let Some(preparation) = preparation {
            let compacted = serde_json::to_vec(&daemon)?;
            match preparation {
                WelcomePreparation::Metadata { removed_content_bytes } => {
                    ensure!(
                        compacted.len() <= COMMANDER_WELCOME_METADATA_HARD_BYTES,
                        "Commander Welcome metadata exceeded hard ceiling: {} > {} bytes",
                        compacted.len(),
                        COMMANDER_WELCOME_METADATA_HARD_BYTES
                    );
                    if compacted.len() > COMMANDER_WELCOME_METADATA_WARN_BYTES {
                        tracing::warn!(
                            metadata_bytes = compacted.len(),
                            warning_bytes = COMMANDER_WELCOME_METADATA_WARN_BYTES,
                            hard_max_bytes = COMMANDER_WELCOME_METADATA_HARD_BYTES,
                            "Commander Welcome metadata exceeded warning ceiling"
                        );
                    }
                    if let DaemonMessage::Welcome { state, .. } = &daemon {
                        supervisor_pane_id = state
                            .panes
                            .iter()
                            .find(|pane| pane.kind == PaneKind::Supervisor)
                            .map(|pane| pane.id.clone());
                        pre_supervisor_bytes = compacted.len();
                        supervisor_ready = false;
                        tracing::info!(
                            metadata_bytes = compacted.len(),
                            pane_count = state.panes.len(),
                            pane_metadata_count = match &daemon {
                                DaemonMessage::Welcome { pane_bootstrap, .. } => pane_bootstrap.len(),
                                _ => 0,
                            },
                            removed_content_bytes,
                            supervisor_pane = supervisor_pane_id.as_deref().unwrap_or("none"),
                            "prepared metadata-only Commander Welcome"
                        );
                    }
                }
                WelcomePreparation::Legacy { replay_before, replay_after } => {
                    ensure!(
                        compacted.len() <= COMMANDER_RELAY_MAX_MESSAGE_BYTES,
                        "bounded legacy Commander Welcome is still too large: {} > {} bytes",
                        compacted.len(),
                        COMMANDER_RELAY_MAX_MESSAGE_BYTES
                    );
                    tracing::info!(
                        upstream_bytes = upstream_bytes.len(),
                        relay_bytes = compacted.len(),
                        replay_before,
                        replay_after,
                        "compacted legacy Commander Welcome for relay"
                    );
                }
            }
            compacted
        } else {
            upstream_bytes
        };
        if matches!(daemon, DaemonMessage::PaneKeyframe { .. }) {
            ensure!(
                bytes.len() <= COMMANDER_KEYFRAME_HARD_BYTES,
                "Commander pane keyframe exceeded protocol hard ceiling: {} > {} bytes",
                bytes.len(),
                COMMANDER_KEYFRAME_HARD_BYTES
            );
            if bytes.len() > COMMANDER_KEYFRAME_WARN_BYTES {
                tracing::warn!(
                    keyframe_bytes = bytes.len(),
                    warning_bytes = COMMANDER_KEYFRAME_WARN_BYTES,
                    budget_bytes = COMMANDER_KEYFRAME_BUDGET_BYTES,
                    hard_max_bytes = COMMANDER_KEYFRAME_HARD_BYTES,
                    "Commander pane keyframe exceeded warning ceiling"
                );
            }
        }
        if !supervisor_ready && supervisor_pane_id.is_some() {
            if !matches!(daemon, DaemonMessage::Welcome { .. }) {
                pre_supervisor_bytes = pre_supervisor_bytes.saturating_add(bytes.len());
            }
            let completes_supervisor = matches!(
                &daemon,
                DaemonMessage::PaneKeyframe { pane_id, .. }
                    if Some(pane_id) == supervisor_pane_id.as_ref()
            );
            if completes_supervisor {
                ensure!(
                    pre_supervisor_bytes <= COMMANDER_PRE_SUPERVISOR_HARD_BYTES,
                    "Commander pre-supervisor-ready bytes exceeded regression ceiling: {} > {} bytes",
                    pre_supervisor_bytes,
                    COMMANDER_PRE_SUPERVISOR_HARD_BYTES
                );
                if pre_supervisor_bytes > COMMANDER_PRE_SUPERVISOR_WARN_BYTES {
                    tracing::warn!(
                        pre_supervisor_bytes,
                        warning_bytes = COMMANDER_PRE_SUPERVISOR_WARN_BYTES,
                        hard_max_bytes = COMMANDER_PRE_SUPERVISOR_HARD_BYTES,
                        "Commander first attach exceeded pre-supervisor-ready budget"
                    );
                } else {
                    tracing::info!(pre_supervisor_bytes, "Commander supervisor keyframe ready");
                }
                supervisor_ready = true;
            }
        }
        events.observe_daemon(session, &daemon);
        let pane_id = match &daemon {
            DaemonMessage::Output { pane_id, .. }
            | DaemonMessage::PaneKeyframe { pane_id, .. }
            | DaemonMessage::ScrollbackPage { pane_id, .. }
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
            pane_bootstrap: Vec::new(),
        };

        let WelcomePreparation::Legacy {
            replay_before: before,
            replay_after: after,
        } = prepare_welcome_for_relay(&mut message).unwrap()
        else {
            panic!("expected legacy preparation");
        };

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

    #[test]
    fn v3_welcome_relay_is_metadata_only_and_keeps_supervisor_identity() {
        let mut message = DaemonMessage::Welcome {
            session_name: "factory-a".into(),
            state: SessionState {
                focused_pane: Some("supervisor".into()),
                panes: vec![PaneInfo {
                    id: "supervisor".into(),
                    kind: PaneKind::Supervisor,
                    focused: true,
                    title: "Supervisor".into(),
                    exited: false,
                }],
                epic_id: None,
                epic_title: None,
                cols: 80,
                rows: 24,
            },
            scrollback: Some(HashMap::from([(
                "supervisor".into(),
                vec![vec![b'x'; COMMANDER_REPLAY_BYTES_PER_PANE]],
            )])),
            protocol_version: 3,
            capabilities: vec![ProtocolCapability::AuthoritativePaneKeyframes],
            pane_bootstrap: vec![crate::ui::factory::PaneBootstrap {
                pane_id: "supervisor".into(),
                epoch: 42,
                cols: 80,
                rows: 24,
                scrollback_start_row: 0,
                scrollback_end_row: 900,
            }],
        };

        assert_eq!(
            prepare_welcome_for_relay(&mut message),
            Some(WelcomePreparation::Metadata {
                removed_content_bytes: COMMANDER_REPLAY_BYTES_PER_PANE,
            })
        );
        let DaemonMessage::Welcome {
            state,
            scrollback,
            pane_bootstrap,
            ..
        } = message
        else {
            panic!("expected Welcome");
        };
        assert!(scrollback.is_none());
        assert_eq!(state.panes[0].kind, PaneKind::Supervisor);
        assert_eq!(pane_bootstrap[0].epoch, 42);
    }
}
