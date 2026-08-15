use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast, mpsc};

use super::DaemonDeathDiagnostic;
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
    DaemonError,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttentionSeverity {
    Critical,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttentionAction {
    Repair,
    ViewPane,
    Retry,
    OpenPr,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttentionEnrichment {
    pub severity: AttentionSeverity,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub action: AttentionAction,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionAttentionContext {
    pub title: String,
    pub phase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineEvent {
    pub sequence: u64,
    #[serde(default)]
    pub revision: u32,
    pub kind: MachineEventKind,
    pub session: Option<String>,
    pub pane_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<DaemonDeathDiagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_context: Option<SessionAttentionContext>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub enrichment_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enrichment: Option<AttentionEnrichment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enriched_at: Option<String>,
    pub at: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone)]
pub struct MachineEventBus {
    tx: broadcast::Sender<MachineEvent>,
    sequence: Arc<AtomicU64>,
    sessions: Arc<Mutex<HashSet<String>>>,
    session_contexts: Arc<StdMutex<HashMap<String, SessionAttentionContext>>>,
    history: Arc<StdMutex<VecDeque<MachineEvent>>>,
    history_capacity: usize,
    persistence_path: Option<Arc<PathBuf>>,
    enrichment_tx: Arc<StdMutex<Option<mpsc::UnboundedSender<MachineEvent>>>>,
}

impl MachineEventBus {
    pub fn new(capacity: usize) -> Self {
        Self::from_history(capacity, VecDeque::new(), None)
    }

    pub fn open(capacity: usize, path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut history = match fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice::<VecDeque<MachineEvent>>(&bytes).unwrap_or_else(|error| {
                    tracing::warn!(%error, "discarding invalid Commander event history");
                    VecDeque::new()
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => VecDeque::new(),
            Err(error) => return Err(error.into()),
        };
        // A hub crash ends any in-flight best-effort call. Never replay a
        // permanent shimmer after restart; the raw event is the final state.
        for event in &mut history {
            if event.enrichment_pending {
                event.enrichment_pending = false;
                event.revision = event.revision.saturating_add(1);
            }
        }
        let bus = Self::from_history(capacity, history, Some(path));
        {
            let history = bus.history.lock().expect("event history lock poisoned");
            bus.persist_locked(&history);
        }
        Ok(bus)
    }

    fn from_history(
        capacity: usize,
        mut history: VecDeque<MachineEvent>,
        persistence_path: Option<PathBuf>,
    ) -> Self {
        let capacity = capacity.max(1);
        while history.len() > capacity {
            history.pop_front();
        }
        let next_sequence = history
            .back()
            .map_or(1, |event| event.sequence.saturating_add(1));
        let (tx, _) = broadcast::channel(capacity.max(1));
        Self {
            tx,
            sequence: Arc::new(AtomicU64::new(next_sequence)),
            sessions: Arc::new(Mutex::new(HashSet::new())),
            session_contexts: Arc::new(StdMutex::new(HashMap::new())),
            history: Arc::new(StdMutex::new(history)),
            history_capacity: capacity,
            persistence_path: persistence_path.map(Arc::new),
            enrichment_tx: Arc::new(StdMutex::new(None)),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<MachineEvent> {
        self.tx.subscribe()
    }

    pub fn history(&self) -> Vec<MachineEvent> {
        self.history
            .lock()
            .expect("event history lock poisoned")
            .iter()
            .cloned()
            .collect()
    }

    pub(crate) fn enable_enrichment(&self) -> mpsc::UnboundedReceiver<MachineEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        *self
            .enrichment_tx
            .lock()
            .expect("enrichment queue lock poisoned") = Some(tx);
        rx
    }

    pub(crate) fn finish_enrichment(&self, sequence: u64, enrichment: Option<AttentionEnrichment>) {
        let patched = {
            let mut history = self.history.lock().expect("event history lock poisoned");
            let Some(index) = history.iter().position(|event| event.sequence == sequence) else {
                return;
            };
            let event = &mut history[index];
            if !event.enrichment_pending {
                return;
            }
            event.enrichment_pending = false;
            event.enrichment = enrichment;
            event.enriched_at = event
                .enrichment
                .as_ref()
                .map(|_| chrono::Utc::now().to_rfc3339());
            event.revision = event.revision.saturating_add(1);
            let patched = event.clone();
            self.persist_locked(&history);
            patched
        };
        let _ = self.tx.send(patched);
    }

    pub(crate) fn set_session_context(
        &self,
        session: impl Into<String>,
        context: SessionAttentionContext,
    ) {
        self.session_contexts
            .lock()
            .expect("session context lock poisoned")
            .insert(session.into(), context);
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
            self.emit(
                MachineEventKind::SessionAdded,
                Some(session),
                None,
                None,
                None,
            );
        }
        for session in removed {
            self.emit(
                MachineEventKind::SessionRemoved,
                Some(session),
                None,
                None,
                None,
            );
        }
    }

    pub fn observe_daemon(&self, session: &str, message: &DaemonMessage) {
        if let DaemonMessage::SessionSummary { summary } = message {
            self.set_session_context(
                session,
                SessionAttentionContext {
                    title: summary.title.clone(),
                    phase: summary.phase.clone(),
                },
            );
            return;
        }
        let (kind, pane_id, payload) = match message {
            DaemonMessage::PaneAdded { pane } => {
                (MachineEventKind::PaneAdded, Some(pane.id.clone()), None)
            }
            DaemonMessage::PaneExited { pane_id, .. } => {
                (MachineEventKind::PaneExited, Some(pane_id.clone()), None)
            }
            DaemonMessage::PaneRemoved { pane_id } => {
                (MachineEventKind::PaneRemoved, Some(pane_id.clone()), None)
            }
            DaemonMessage::Error { message } => (
                MachineEventKind::DaemonError,
                None,
                Some(serde_json::json!({"message": message})),
            ),
            _ => return,
        };
        self.emit(kind, Some(session.to_owned()), pane_id, None, payload);
    }

    pub(crate) fn daemon_disconnected(&self, session: &str, diagnostic: DaemonDeathDiagnostic) {
        self.emit(
            MachineEventKind::DaemonDisconnected,
            Some(session.to_owned()),
            None,
            Some(diagnostic),
            None,
        );
    }

    pub(crate) fn controller_changed(&self, session: &str) {
        self.emit(
            MachineEventKind::ControllerChanged,
            Some(session.to_owned()),
            None,
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
        payload: Option<serde_json::Value>,
    ) {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let enrichment = self
            .enrichment_tx
            .lock()
            .expect("enrichment queue lock poisoned")
            .clone();
        let enrichment_pending = kind == MachineEventKind::DaemonError && enrichment.is_some();
        let session_context = session.as_ref().and_then(|session| {
            self.session_contexts
                .lock()
                .expect("session context lock poisoned")
                .get(session)
                .cloned()
        });
        let event = MachineEvent {
            sequence,
            revision: 0,
            kind,
            session,
            pane_id,
            diagnostic,
            payload,
            session_context,
            enrichment_pending,
            enrichment: None,
            enriched_at: None,
            at: chrono::Utc::now().to_rfc3339(),
        };
        {
            let mut history = self.history.lock().expect("event history lock poisoned");
            history.push_back(event.clone());
            while history.len() > self.history_capacity {
                history.pop_front();
            }
            self.persist_locked(&history);
        }
        let _ = self.tx.send(event.clone());
        if enrichment_pending {
            let _ = enrichment.expect("pending requires queue").send(event);
        }
    }

    fn persist_locked(&self, history: &VecDeque<MachineEvent>) {
        let Some(path) = self.persistence_path.as_deref() else {
            return;
        };
        let Some(parent) = path.parent() else {
            return;
        };
        if fs::create_dir_all(parent).is_err() {
            return;
        }
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let Ok(bytes) = serde_json::to_vec(history) else {
            return;
        };
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let Ok(mut file) = options.open(&temporary) else {
            return;
        };
        if file.write_all(&bytes).is_ok() && file.sync_all().is_ok() {
            let _ = fs::rename(temporary, path);
        } else {
            let _ = fs::remove_file(temporary);
        }
    }
}
