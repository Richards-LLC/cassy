use crate::ui::factory::app::imports::*;

impl FactoryApp {
    // Recording methods

    /// Check if recording is enabled for this session
    pub fn record_enabled(&self) -> bool {
        self.record_enabled
    }

    /// Get the recording session ID (if recording is enabled)
    pub fn recording_session_id(&self) -> Option<&str> {
        self.recording_session_id.as_deref()
    }

    /// Record a director event for later export
    ///
    /// Called when significant events occur (task completed, epic started, etc.)
    pub fn record_event(&mut self, event: DirectorEvent) {
        if self.record_enabled {
            self.recorded_events.push((Instant::now(), event));
        }
    }

    /// Record multiple director events
    pub fn record_events(&mut self, events: &[DirectorEvent]) {
        if self.record_enabled {
            let now = Instant::now();
            for event in events {
                self.recorded_events.push((now, event.clone()));
            }
        }
    }

    /// Start recording on all panes (supervisor and workers)
    ///
    /// This should be called after the app is created and inside an async context.
    /// Recording files are saved to ~/.cas/recordings/{session_id}/{agent_name}.rec
    pub async fn start_recording(&mut self) -> anyhow::Result<()> {
        use cas_recording::WriterConfig;

        // Mark recording start time for event timestamps
        self.recording_start = Some(Instant::now());

        let session_id = match &self.recording_session_id {
            Some(id) => id.clone(),
            None => {
                tracing::warn!("Recording enabled but no session_id provided");
                return Ok(());
            }
        };

        // Create recordings directory
        let recordings_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cas")
            .join("recordings");

        let config = WriterConfig {
            recordings_dir,
            ..Default::default()
        };

        // Start recording on supervisor
        if let Some(pane) = self.mux.get_mut(&self.supervisor_name)
            && !pane.is_recording() {
            if let Err(e) = pane.start_recording(&session_id, config.clone()).await {
                tracing::error!("Failed to start recording for supervisor: {}", e);
            } else {
                tracing::info!("Started recording for supervisor: {}", self.supervisor_name);
            }
        }

        // Start recording on workers
        for worker_name in &self.worker_names.clone() {
            if let Some(pane) = self.mux.get_mut(worker_name)
                && !pane.is_recording() {
                if let Err(e) = pane.start_recording(&session_id, config.clone()).await {
                    tracing::error!(
                        "Failed to start recording for worker {}: {}",
                        worker_name,
                        e
                    );
                } else {
                    tracing::info!("Started recording for worker: {}", worker_name);
                }
            }
        }

        Ok(())
    }

    /// Start recording for a single pane by name
    ///
    /// Used when workers are spawned dynamically after initial recording setup.
    pub async fn start_recording_for_pane(&mut self, pane_name: &str) -> anyhow::Result<()> {
        use cas_recording::WriterConfig;

        let session_id = match &self.recording_session_id {
            Some(id) => id.clone(),
            None => {
                return Ok(());
            }
        };

        let recordings_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cas")
            .join("recordings");

        let config = WriterConfig {
            recordings_dir,
            ..Default::default()
        };

        if let Some(pane) = self.mux.get_mut(pane_name) {
            if let Err(e) = pane.start_recording(&session_id, config).await {
                tracing::error!("Failed to start recording for {}: {}", pane_name, e);
            } else {
                tracing::info!("Started recording for: {}", pane_name);
            }
        }

        Ok(())
    }

    /// Stop recording for a single pane by name
    ///
    /// Used when workers are shut down during a session.
    pub async fn stop_recording_for_pane(&mut self, pane_name: &str) -> anyhow::Result<()> {
        if let Some(pane) = self.mux.get_mut(pane_name) {
            if let Err(e) = pane.stop_recording().await {
                tracing::error!("Failed to stop recording for {}: {}", pane_name, e);
            } else {
                tracing::info!("Stopped recording for: {}", pane_name);
            }
        }
        Ok(())
    }

    /// Stop recording on all panes and finalize recordings
    ///
    /// Also exports the session to a .casrec archive in the recordings directory.
    pub async fn stop_recording(&mut self) -> anyhow::Result<()> {
        // Stop recording on supervisor
        if let Some(pane) = self.mux.get_mut(&self.supervisor_name) {
            if let Err(e) = pane.stop_recording().await {
                tracing::error!("Failed to stop recording for supervisor: {}", e);
            }
        }

        // Stop recording on workers
        for worker_name in &self.worker_names.clone() {
            if let Some(pane) = self.mux.get_mut(worker_name) {
                if let Err(e) = pane.stop_recording().await {
                    tracing::error!("Failed to stop recording for worker {}: {}", worker_name, e);
                }
            }
        }

        tracing::info!("Stopped recording for all panes");

        // Auto-export to .casrec archive
        if let Some(session_id) = &self.recording_session_id {
            let recordings_dir = dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".cas")
                .join("recordings");

            let output_path = recordings_dir.join(format!("{session_id}.casrec"));

            // Convert collected events to RecordedEvent format
            let recording_start = self.recording_start.unwrap_or_else(Instant::now);
            let events: Vec<cas_recording::RecordedEvent> = self
                .recorded_events
                .iter()
                .map(|(instant, event)| {
                    let timestamp_ms = instant.duration_since(recording_start).as_millis() as u64;
                    cas_recording::RecordedEvent {
                        timestamp_ms,
                        event_type: event.event_type().to_string(),
                        agent: event.target().map(|s| s.to_string()),
                        data: event.to_json(),
                    }
                })
                .collect();

            let config = cas_recording::ExportConfig::default();
            let manifest_extra = cas_recording::ManifestExtra {
                session_name: Some(session_id.clone()),
                project_dir: self
                    .cas_dir
                    .parent()
                    .and_then(|p| p.to_str())
                    .map(|s| s.to_string()),
                epic_id: self.epic_state.epic_id().map(|s| s.to_string()),
                epic_title: self.epic_state.epic_title().map(|s| s.to_string()),
            };
            let events_opt = if events.is_empty() {
                None
            } else {
                Some(events)
            };
            match cas_recording::export_session(
                session_id,
                &recordings_dir,
                &output_path,
                &config,
                Some(manifest_extra),
                events_opt,
            ) {
                Ok(stats) => {
                    tracing::info!(
                        "Exported recording to {} ({} bytes, {:.1}x compression)",
                        output_path.display(),
                        stats.compressed_size,
                        stats.compression_ratio
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to export recording: {}", e);
                }
            }
        }

        Ok(())
    }
}
