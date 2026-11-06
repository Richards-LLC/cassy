//! Serializable launch policy and panes; transient caches are rebuilt at boot.
use super::*;
use crate::pane::PaneHandoff;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MuxHandoff {
    pub panes: Vec<PaneHandoff>,
    pub focused: Option<String>,
    pub rows: u16,
    pub cols: u16,
    pub default_worker_spec: WorkerSpec,
    pub worker_specs: HashMap<String, WorkerSpec>,
    pub worker_startup_prompts: HashMap<String, String>,
    pub supervisor_cli: SupervisorCli,
    pub factory_session: Option<String>,
    pub supervisor_launch: Option<SupervisorLaunch>,
}

impl Mux {
    /// No delayed submit or interrupted injection may cross an exec boundary.
    pub fn handoff_ready(&self) -> bool {
        self.pending_injects.is_empty() && self.panes.values().all(|pane| !pane.inject_submit_pending())
    }

    pub async fn prepare_handoff(&mut self) -> Result<MuxHandoff> {
        if !self.handoff_ready() { return Err(Error::pty("A pane injection is still settling; retry reexec")); }
        let mut panes = Vec::new();
        for pane in self.panes.values_mut() { panes.push(pane.prepare_handoff().await?); }
        Ok(MuxHandoff {
            panes, focused: self.focused.clone(), rows: self.rows, cols: self.cols,
            default_worker_spec: self.default_worker_spec.clone(), worker_specs: self.worker_specs.clone(),
            worker_startup_prompts: self.worker_startup_prompts.clone(),
            supervisor_cli: self.supervisor_cli, factory_session: self.factory_session.clone(),
            supervisor_launch: self.supervisor_launch.clone(),
        })
    }

    pub fn from_handoff(state: &MuxHandoff) -> Result<Self> {
        let mut mux = Self::new(state.rows, state.cols);
        mux.default_worker_spec = state.default_worker_spec.clone();
        mux.worker_specs = state.worker_specs.clone();
        mux.worker_startup_prompts = state.worker_startup_prompts.clone();
        mux.supervisor_cli = state.supervisor_cli;
        mux.factory_session = state.factory_session.clone();
        mux.supervisor_launch = state.supervisor_launch.clone();
        for pane in &state.panes { mux.add_pane(Pane::from_handoff(pane)?); }
        if let Some(focus) = &state.focused { mux.focus(focus)?; }
        Ok(mux)
    }

    pub fn resume_after_handoff(&mut self) -> Result<()> {
        for pane in self.panes.values_mut() { pane.resume_after_handoff()?; }
        Ok(())
    }
}
