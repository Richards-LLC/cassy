use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast};

use super::{DaemonDeathDiagnostic, diagnose_daemon_death};
use crate::ui::factory::DaemonMessage;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MachineEventKind {
    SessionAdded,
    SessionRemoved,
    PaneAdded,
    PaneExited,
    PaneRemoved,
    DaemonDisconnected,
    ControllerChanged,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineEvent {
    pub sequence: u64,
    pub kind: MachineEventKind,
    pub session: Option<String>,
    pub pane_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<DaemonDeathDiagnostic>,
    pub at: String,
}

#[derive(Clone)]
pub struct MachineEventBus {
    tx: broadcast::Sender<MachineEvent>,
    sequence: Arc<AtomicU64>,
    sessions: Arc<Mutex<HashSet<String>>>,
}

impl MachineEventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(1));
        Self {
            tx,
            sequence: Arc::new(AtomicU64::new(1)),
            sessions: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<MachineEvent> {
        self.tx.subscribe()
    }

    pub async fn reconcile_sessions<I, S>(&self, sessions: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let next: HashSet<String> = sessions.into_iter().map(Into::into).collect();
        let mut current = self.sessions.lock().await;
        let mut added: Vec<_> = next.difference(&current).cloned().collect();
        let mut removed: Vec<_> = current.difference(&next).cloned().collect();
        added.sort();
        removed.sort();
        *current = next;
        drop(current);
        for session in added {
            self.emit(MachineEventKind::SessionAdded, Some(session), None, None);
        }
        for session in removed {
            self.emit(MachineEventKind::SessionRemoved, Some(session), None, None);
        }
    }

    pub fn observe_daemon(&self, session: &str, message: &DaemonMessage) {
        let (kind, pane_id) = match message {
            DaemonMessage::PaneAdded { pane } => {
                (MachineEventKind::PaneAdded, Some(pane.id.clone()))
            }
            DaemonMessage::PaneExited { pane_id, .. } => {
                (MachineEventKind::PaneExited, Some(pane_id.clone()))
            }
            DaemonMessage::PaneRemoved { pane_id } => {
                (MachineEventKind::PaneRemoved, Some(pane_id.clone()))
            }
            _ => return,
        };
        self.emit(kind, Some(session.to_owned()), pane_id, None);
    }

    pub(crate) fn daemon_disconnected(&self, session: &str) {
        self.emit(
            MachineEventKind::DaemonDisconnected,
            Some(session.to_owned()),
            None,
            Some(diagnose_daemon_death(None, false)),
        );
    }

    pub(crate) fn controller_changed(&self, session: &str) {
        self.emit(
            MachineEventKind::ControllerChanged,
            Some(session.to_owned()),
            None,
            None,
        );
    }

    fn emit(
        &self,
        kind: MachineEventKind,
        session: Option<String>,
        pane_id: Option<String>,
        diagnostic: Option<DaemonDeathDiagnostic>,
    ) {
        let _ = self.tx.send(MachineEvent {
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            kind,
            session,
            pane_id,
            diagnostic,
            at: chrono::Utc::now().to_rfc3339(),
        });
    }
}
