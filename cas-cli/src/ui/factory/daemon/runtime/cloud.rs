use crate::cloud::CloudConfig;
use crate::ui::factory::daemon::FactoryDaemon;
use crate::ui::factory::daemon::cloud_client::{CloudClientHandle, serialize_factory_state};
use crate::ui::factory::director::DirectorEvent;
use std::path::Path;

/// Decide whether to spawn the factory live-stream WebSocket client for the
/// given cloud config. Returns `false` when the user is not logged in or when
/// the factory cloud client is disabled in `.cas/cloud.json`.
///
/// Extracted from [`FactoryDaemon::try_start_cloud_client`] so it can be unit
/// tested without touching the filesystem.
pub(crate) fn should_spawn_cloud_client(cfg: &CloudConfig) -> bool {
    if !cfg.is_logged_in() {
        return false;
    }
    cfg.factory_cloud_client_enabled
}

impl FactoryDaemon {
    /// Try to start the cloud phone-home client.
    ///
    /// Reads CloudConfig (for endpoint + token) and DeviceConfig (for device_id).
    /// Returns None if not authenticated, if the factory cloud client is
    /// disabled in `.cas/cloud.json` (the default — see cas-4244), or on any
    /// error (phone-home is best-effort).
    pub(crate) fn try_start_cloud_client(session_name: &str) -> Option<CloudClientHandle> {
        use crate::cloud::DeviceConfig;
        use crate::ui::factory::daemon::cloud_client::{self, CloudClientConfig};

        let cloud_config = match CloudConfig::load() {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(
                    "Cloud phone-home skipped: failed to load cloud config: {}",
                    e
                );
                return None;
            }
        };

        if !should_spawn_cloud_client(&cloud_config) {
            if !cloud_config.is_logged_in() {
                tracing::info!("Cloud phone-home skipped: not logged in");
            } else {
                tracing::info!(
                    "factory cloud client disabled (set factory_cloud_client_enabled=true in .cas/cloud.json to re-enable — see cas-4244)"
                );
            }
            return None;
        }

        let token = cloud_config.token.unwrap_or_default();
        let endpoint = cloud_config.endpoint;

        let device_id = DeviceConfig::load().ok().flatten().map(|d| d.device_id);

        let cas_dir = crate::store::detect::find_cas_root().ok();

        let config = CloudClientConfig {
            endpoint,
            token,
            factory_id: session_name.to_string(),
            device_id,
            cas_dir,
            factory_session: Some(session_name.to_string()),
        };

        tracing::info!(
            "Starting cloud phone-home client for factory '{}'",
            session_name
        );
        Some(cloud_client::spawn_cloud_client(config))
    }

    /// Push factory state snapshot to the cloud.
    ///
    /// Called after refresh_data() when cloud phone-home is active.
    pub(super) fn push_cloud_state(&self) {
        if let Some(ref handle) = self.cloud_handle {
            let state = serialize_factory_state(
                &self.session_name,
                self.app.supervisor_name(),
                self.app.director_data(),
            );
            handle.send_state(state);
        }
    }

    /// Push lifecycle events to the cloud.
    ///
    /// Called after event detection when cloud phone-home is active.
    // gate[N-35 notice]: Director events to Cloud
    pub(super) fn push_cloud_events(&self, events: &[DirectorEvent]) {
        if let Some(ref handle) = self.cloud_handle {
            for event in events {
                handle.send_event(event.event_type(), event.to_json());
            }
        }
    }

    /// Upload terminal recordings to the cloud after session ends.
    ///
    /// Reads each agent's .rec file, converts events to JSON format,
    /// and sends via the cloud WebSocket as recording chunks.
    pub(super) fn upload_recordings(&self) {
        let Some(ref handle) = self.cloud_handle else {
            return;
        };

        let Some(session_id) = self.app.recording_session_id() else {
            return;
        };

        let recordings_dir = dirs::home_dir()
            .unwrap_or_default()
            .join(".cas")
            .join("recordings");

        let session_dir = recordings_dir.join(session_id);
        if !session_dir.exists() {
            tracing::debug!("No recordings directory found at {:?}", session_dir);
            return;
        }

        // Find all .rec files in the session directory
        let rec_files: Vec<_> = match std::fs::read_dir(&session_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "rec"))
                .map(|e| e.path())
                .collect(),
            Err(e) => {
                tracing::warn!("Failed to read recordings directory: {}", e);
                return;
            }
        };

        tracing::info!("Uploading {} recording files to cloud", rec_files.len());

        for (chunk_index, rec_path) in rec_files.iter().enumerate() {
            if let Err(e) = upload_single_recording(handle, rec_path, chunk_index as u32) {
                tracing::warn!(
                    "Failed to upload recording {:?}: {}",
                    rec_path.file_name().unwrap_or_default(),
                    e
                );
            }
        }
    }

    /// Disconnect the cloud client (called during cleanup).
    pub(super) fn disconnect_cloud(&self) {
        if let Some(ref handle) = self.cloud_handle {
            handle.disconnect();
        }
    }
}

/// Convert a single .rec file to JSON events and upload via cloud client.
fn upload_single_recording(
    handle: &CloudClientHandle,
    rec_path: &Path,
    chunk_index: u32,
) -> anyhow::Result<()> {
    use base64::Engine;
    use cas_recording::{RecordingEvent, RecordingReader};

    let reader = RecordingReader::open(rec_path)?;
    let header = reader.header();
    let worker_name = header.agent_name.clone();

    // Convert events to JSON format for the frontend player
    let mut json_events = Vec::new();
    for event_result in reader.read_all_events() {
        match event_result? {
            RecordingEvent::Output { timestamp_ms, data } => {
                json_events.push(serde_json::json!({
                    "t": timestamp_ms,
                    "type": "output",
                    "data": base64::engine::general_purpose::STANDARD.encode(&data),
                }));
            }
            RecordingEvent::Resize {
                timestamp_ms,
                cols,
                rows,
            } => {
                json_events.push(serde_json::json!({
                    "t": timestamp_ms,
                    "type": "resize",
                    "cols": cols,
                    "rows": rows,
                }));
            }
            RecordingEvent::Keyframe { .. } => {
                // Skip keyframes — they're for seeking, not playback
            }
        }
    }

    let json_payload = serde_json::json!({
        "header": {
            "cols": header.cols,
            "rows": header.rows,
        },
        "events": json_events,
    });

    // Serialize to JSON bytes and base64 encode for the channel
    let json_bytes = serde_json::to_vec(&json_payload)?;
    let data_base64 = base64::engine::general_purpose::STANDARD.encode(&json_bytes);

    // Calculate timestamps (header.created_at is DateTime<Utc>)
    let started_at = header.created_at.to_rfc3339();
    let duration_ms = reader.duration_ms();
    let ended_at = Some(
        (header.created_at + chrono::TimeDelta::milliseconds(duration_ms as i64)).to_rfc3339(),
    );

    tracing::info!(
        "Uploading recording for '{}': {} events, {} bytes (base64: {} bytes)",
        worker_name,
        json_events.len(),
        json_bytes.len(),
        data_base64.len(),
    );

    handle.send_recording_chunk(
        &worker_name,
        chunk_index,
        data_base64,
        &started_at,
        ended_at.as_deref(),
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logged_in_config() -> CloudConfig {
        CloudConfig {
            token: Some("t".to_string()),
            ..CloudConfig::default()
        }
    }

    #[test]
    fn default_config_does_not_spawn() {
        // Fresh install: not logged in AND flag off. Don't spawn.
        assert!(!should_spawn_cloud_client(&CloudConfig::default()));
    }

    #[test]
    fn logged_in_but_flag_off_does_not_spawn() {
        // cas-4244: even authenticated users must not spawn the WS client
        // while the endpoint returns 404. This is the common case today.
        let cfg = logged_in_config();
        assert!(!cfg.factory_cloud_client_enabled);
        assert!(!should_spawn_cloud_client(&cfg));
    }

    #[test]
    fn logged_in_and_flag_on_spawns() {
        let cfg = CloudConfig {
            factory_cloud_client_enabled: true,
            ..logged_in_config()
        };
        assert!(should_spawn_cloud_client(&cfg));
    }

    #[test]
    fn flag_on_but_not_logged_in_does_not_spawn() {
        // Anonymous users must never spawn, even if the flag is flipped on —
        // the WS client needs a token. Cover both the missing-token case
        // (`None`) and the empty-string case — `is_logged_in()` treats both
        // as not-logged-in, and a regression that relaxed that would silently
        // enable the WS client for empty-tokened users.
        let missing = CloudConfig {
            factory_cloud_client_enabled: true,
            token: None,
            ..CloudConfig::default()
        };
        assert!(!should_spawn_cloud_client(&missing));

        let empty = CloudConfig {
            factory_cloud_client_enabled: true,
            token: Some(String::new()),
            ..CloudConfig::default()
        };
        assert!(!should_spawn_cloud_client(&empty));
    }

    #[test]
    fn flag_persists_through_serde() {
        // Roundtripping through JSON (the on-disk form) must preserve the
        // flag, and a cloud.json written before this field existed must
        // deserialize cleanly with the flag defaulted off.
        let cfg = CloudConfig {
            factory_cloud_client_enabled: true,
            ..logged_in_config()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let roundtripped: CloudConfig = serde_json::from_str(&json).unwrap();
        assert!(roundtripped.factory_cloud_client_enabled);

        let legacy = r#"{"endpoint":"https://cas.dev","token":"t"}"#;
        let parsed: CloudConfig = serde_json::from_str(legacy).unwrap();
        assert!(!parsed.factory_cloud_client_enabled);
    }
}
