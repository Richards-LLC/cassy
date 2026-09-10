//! Off-by-default worker visibility for Commander streams (cas-6261).
//!
//! The operator asked for a Hub that shows supervisors only: worker panes on a
//! phone crowd the list and cost bandwidth for terminals nobody is reading.
//! The multiplexer can already filter frames by pane id, but a viewer cannot
//! name pane ids before the daemon's Welcome arrives, so this gate learns the
//! worker pane ids from Welcome/StateUpdate frames as they pass and then hides
//! those panes and drops their output until the viewer explicitly asks for
//! workers (`workers=1` on the attach URL or `"workers": true` on the machine
//! subscribe envelope).

use std::collections::HashSet;

use super::{ProxyFrame, ProxyFrameKind};

#[derive(Debug, Default)]
pub struct WorkerGate {
    reveal: bool,
    workers: HashSet<String>,
}

impl WorkerGate {
    pub fn new(reveal: bool) -> Self {
        Self {
            reveal,
            workers: HashSet::new(),
        }
    }

    /// Number of worker panes currently hidden by this gate.
    pub fn hidden_workers(&self) -> usize {
        if self.reveal { 0 } else { self.workers.len() }
    }

    /// Pass a frame through the gate. Returns `None` when the frame belongs to
    /// a hidden worker pane; state frames come back with worker panes removed.
    pub fn admit(&mut self, frame: ProxyFrame) -> Option<ProxyFrame> {
        if self.reveal {
            return Some(frame);
        }
        match frame.kind {
            ProxyFrameKind::Output | ProxyFrameKind::PaneKeyframe => {
                let hidden = frame
                    .pane_id
                    .as_deref()
                    .is_some_and(|pane| self.workers.contains(pane));
                (!hidden).then_some(frame)
            }
            ProxyFrameKind::Other => self.admit_other(frame),
        }
    }

    fn admit_other(&mut self, frame: ProxyFrame) -> Option<ProxyFrame> {
        let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&frame.bytes) else {
            return Some(frame);
        };
        let Some(object) = value.as_object_mut() else {
            return Some(frame);
        };
        if let Some(added) = object.get("PaneAdded") {
            let pane = added.get("pane");
            if pane.and_then(|pane| pane.get("kind")).and_then(serde_json::Value::as_str)
                == Some("Worker")
            {
                if let Some(id) = pane
                    .and_then(|pane| pane.get("id"))
                    .and_then(serde_json::Value::as_str)
                {
                    self.workers.insert(id.to_owned());
                }
                return None;
            }
            return Some(frame);
        }
        let mut changed = false;
        for key in ["Welcome", "StateUpdate"] {
            let Some(inner) = object.get_mut(key) else { continue };
            let mut workers = HashSet::new();
            if let Some(panes) = inner
                .get_mut("state")
                .and_then(|state| state.get_mut("panes"))
                .and_then(serde_json::Value::as_array_mut)
            {
                let before = panes.len();
                panes.retain(|pane| {
                    let worker = pane.get("kind").and_then(serde_json::Value::as_str) == Some("Worker");
                    if worker {
                        if let Some(id) = pane.get("id").and_then(serde_json::Value::as_str) {
                            workers.insert(id.to_owned());
                        }
                    }
                    !worker
                });
                changed |= panes.len() != before;
            }
            if let Some(state) = inner.get_mut("state").and_then(serde_json::Value::as_object_mut)
                && state
                    .get("focused_pane")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|focused| workers.contains(focused))
            {
                state.insert("focused_pane".into(), serde_json::Value::Null);
                changed = true;
            }
            if let Some(bootstrap) = inner
                .get_mut("pane_bootstrap")
                .and_then(serde_json::Value::as_array_mut)
            {
                let before = bootstrap.len();
                bootstrap.retain(|entry| {
                    !entry
                        .get("pane_id")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|id| workers.contains(id))
                });
                changed |= bootstrap.len() != before;
            }
            if let Some(scrollback) = inner
                .get_mut("scrollback")
                .and_then(serde_json::Value::as_object_mut)
            {
                let before = scrollback.len();
                scrollback.retain(|id, _| !workers.contains(id));
                changed |= scrollback.len() != before;
            }
            // A full state frame is authoritative for the roster.
            self.workers = workers;
        }
        if frame
            .pane_id
            .as_deref()
            .is_some_and(|pane| self.workers.contains(pane))
        {
            return None;
        }
        if !changed {
            return Some(frame);
        }
        let bytes = serde_json::to_vec(&value).ok()?;
        Some(ProxyFrame {
            bytes,
            pane_id: frame.pane_id,
            kind: frame.kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::proxy_frame;
    use crate::ui::factory::{DaemonMessage, PaneInfo, PaneKind, SessionState};

    fn pane(id: &str, kind: PaneKind) -> PaneInfo {
        PaneInfo {
            id: id.into(),
            kind,
            focused: false,
            title: id.into(),
            exited: false,
        }
    }

    fn welcome() -> ProxyFrame {
        proxy_frame(DaemonMessage::Welcome {
            session_name: "s".into(),
            state: SessionState {
                focused_pane: Some("worker-1".into()),
                panes: vec![
                    pane("supervisor", PaneKind::Supervisor),
                    pane("worker-1", PaneKind::Worker),
                    pane("worker-2", PaneKind::Worker),
                ],
                epic_id: None,
                epic_title: None,
                cols: 80,
                rows: 24,
            },
            scrollback: None,
            protocol_version: 3,
            capabilities: Vec::new(),
            pane_bootstrap: Vec::new(),
        })
    }

    fn output(pane_id: &str) -> ProxyFrame {
        proxy_frame(DaemonMessage::Output {
            pane_id: pane_id.into(),
            data: b"x".to_vec(),
        })
    }

    #[test]
    fn default_gate_hides_worker_panes_and_their_output() {
        let mut gate = WorkerGate::new(false);
        let admitted = gate.admit(welcome()).expect("welcome passes");
        let value: serde_json::Value = serde_json::from_slice(&admitted.bytes).unwrap();
        let panes = value["Welcome"]["state"]["panes"].as_array().unwrap();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0]["id"], "supervisor");
        assert!(value["Welcome"]["state"]["focused_pane"].is_null());
        assert_eq!(gate.hidden_workers(), 2);
        assert!(gate.admit(output("worker-1")).is_none());
        assert!(gate.admit(output("supervisor")).is_some());
        assert!(
            gate.admit(proxy_frame(DaemonMessage::PaneAdded {
                pane: pane("worker-3", PaneKind::Worker)
            }))
            .is_none()
        );
        assert!(gate.admit(output("worker-3")).is_none());
        assert_eq!(gate.hidden_workers(), 3);
    }

    #[test]
    fn revealed_gate_passes_everything_unchanged() {
        let mut gate = WorkerGate::new(true);
        let original = welcome();
        let admitted = gate.admit(original.clone()).unwrap();
        assert_eq!(admitted.bytes, original.bytes);
        assert!(gate.admit(output("worker-1")).is_some());
        assert_eq!(gate.hidden_workers(), 0);
    }

    #[test]
    fn state_update_replaces_the_hidden_roster() {
        let mut gate = WorkerGate::new(false);
        gate.admit(welcome()).unwrap();
        gate.admit(proxy_frame(DaemonMessage::StateUpdate {
            state: SessionState {
                focused_pane: None,
                panes: vec![pane("supervisor", PaneKind::Supervisor)],
                epic_id: None,
                epic_title: None,
                cols: 80,
                rows: 24,
            },
        }))
        .unwrap();
        assert_eq!(gate.hidden_workers(), 0);
        assert!(gate.admit(output("worker-1")).is_some());
    }
}
