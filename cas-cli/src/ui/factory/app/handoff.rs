//! Rebuild ephemeral UI/store caches around inherited live panes.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(crate) struct AppHandoff {
    pub(crate) cas_dir: PathBuf,
    project_dir: PathBuf,
    pub(crate) session: String,
    pub(crate) mux: cas_mux::MuxHandoff,
    supervisor_name: String,
    worker_names: Vec<String>,
    supervisor_cli: cas_mux::SupervisorCli,
    worker_cli: cas_mux::SupervisorCli,
    worktree_config: Option<crate::worktree::WorktreeConfig>,
    worktree_repo: Option<PathBuf>,
    tabbed: bool,
    auto_prompt: AutoPromptConfig,
    notify_enabled: bool,
    notify_backend: String,
    notify_also_bell: bool,
    record_enabled: bool,
    recording_session_id: Option<String>,
    lead_session_id: Option<String>,
    session_to_pane: HashMap<String, String>,
    operator_draft_hold: bool,
}

impl FactoryApp {
    pub(crate) async fn prepare_handoff(&mut self) -> anyhow::Result<AppHandoff> {
        let now = Instant::now();
        let mut blockers: Vec<_> = self
            .pending_workers
            .iter()
            .map(|worker| {
                format!(
                    "kind=provisioning worker={} elapsed={}s",
                    worker.name,
                    now.saturating_duration_since(worker.started_at).as_secs(),
                )
            })
            .collect();
        blockers.extend(self.worktree_cleanup_jobs.iter().map(|job| {
            format!(
                "kind=worktree_cleanup worker={} occurrence={} elapsed={}s",
                job.worker,
                job.occurrence,
                now.saturating_duration_since(job.started).as_secs(),
            )
        }));
        for (worker, count) in &self.crash_cleanups_pending {
            let tracked = self
                .worktree_cleanup_jobs
                .iter()
                .filter(|job| &job.worker == worker)
                .count();
            if *count > tracked || tracked == 0 {
                // Crash jobs outside this collection have no recorded clock or
                // occurrence. Report that gap without inventing either value.
                blockers.push(format!(
                    "kind=crash_cleanup worker={worker} pending={} occurrence=untracked elapsed=unknown",
                    count.saturating_sub(tracked),
                ));
            }
        }
        blockers.sort();
        anyhow::ensure!(
            blockers.is_empty(),
            "Reexec blocked by active jobs: {}. Recovery: once workers are idle, fully restart this factory. \
             Cleanup jobs and worker-name reservations stay active until completion or factory shutdown.",
            blockers.join("; "),
        );
        Ok(AppHandoff {
            cas_dir: self.cas_dir.clone(),
            project_dir: self.project_dir.clone(),
            session: self
                .factory_session
                .clone()
                .ok_or_else(|| anyhow::anyhow!("No factory session"))?,
            mux: self.mux.prepare_handoff().await?,
            supervisor_name: self.supervisor_name.clone(),
            worker_names: self.worker_names.clone(),
            supervisor_cli: self.supervisor_cli,
            worker_cli: self.worker_cli,
            worktree_config: self.worktree_manager.as_ref().map(|m| m.handoff_config()),
            worktree_repo: self
                .worktree_manager
                .as_ref()
                .map(|m| m.repo_root().to_path_buf()),
            tabbed: self.tabbed_workers,
            auto_prompt: self.auto_prompt.clone(),
            notify_enabled: self.notifier.is_enabled(),
            notify_backend: match self.notifier.configuration().backend {
                super::super::notification::NotifyBackend::Native => "native",
                super::super::notification::NotifyBackend::Bell => "bell",
                super::super::notification::NotifyBackend::ITerm2 => "iterm2",
            }
            .into(),
            notify_also_bell: self.notifier.configuration().also_bell,
            record_enabled: self.record_enabled,
            recording_session_id: self.recording_session_id.clone(),
            lead_session_id: self.lead_session_id.clone(),
            session_to_pane: self.session_to_pane.clone(),
            operator_draft_hold: self.operator_draft_hold,
        })
    }

    pub(crate) fn from_handoff(state: &AppHandoff) -> anyhow::Result<Self> {
        let mux = cas_mux::Mux::from_handoff(&state.mux)?;
        let manager = match (&state.worktree_config, &state.worktree_repo) {
            (Some(config), Some(repo)) => Some(WorktreeManager::new(repo, config.clone())?),
            _ => None,
        };
        let data = DirectorData::load_for_project(
            &state.cas_dir,
            None,
            false,
            crate::cloud::resolve_canonical_id(&state.cas_dir).as_deref(),
        )?;
        let mut app = Self::from_init_result(
            state.cas_dir.clone(),
            mux,
            manager,
            data,
            state.supervisor_name.clone(),
            state.worker_names.clone(),
            super::super::notification::NotifyConfig {
                enabled: state.notify_enabled,
                backend: match state.notify_backend.as_str() {
                    "bell" => super::super::notification::NotifyBackend::Bell,
                    "iterm2" => super::super::notification::NotifyBackend::ITerm2,
                    _ => super::super::notification::NotifyBackend::Native,
                },
                also_bell: state.notify_also_bell,
            },
            state.tabbed,
            state.auto_prompt.clone(),
            state.supervisor_cli,
            state.worker_cli,
            state.mux.cols,
            state.mux.rows,
            state.record_enabled,
            state.recording_session_id.clone(),
            state.lead_session_id.clone(),
            state.project_dir.clone(),
        )?;
        app.set_factory_session(state.session.clone());
        app.session_to_pane = state.session_to_pane.clone();
        app.operator_draft_hold = state.operator_draft_hold;
        Ok(app)
    }
}
