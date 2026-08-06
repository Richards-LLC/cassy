use chrono::Utc;
use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write;
use std::time::Instant;
use tracing::warn;

use crate::cloud::syncer::{CloudSyncer, PushResponse, SyncResult};
use crate::cloud::{QueuedSync, SyncOperation, get_project_canonical_id};
use crate::error::CasError;
use crate::types::Session;

impl CloudSyncer {
    pub fn push(&self) -> Result<SyncResult, CasError> {
        self.push_with_sessions(&[])
    }

    /// Push queued changes and sessions to cloud
    pub fn push_with_sessions(&self, sessions: &[Session]) -> Result<SyncResult, CasError> {
        let mut result = SyncResult::default();
        let start = Instant::now();

        if !self.is_available() {
            return Ok(result);
        }

        let pending = self
            .queue
            .pending_by_type(self.config.batch_size, self.config.max_retries)?;

        // Check if there's anything to push
        if pending.is_empty() && sessions.is_empty() {
            result.duration_ms = start.elapsed().as_millis() as u64;
            return Ok(result);
        }

        let token = self
            .cloud_config
            .token
            .as_ref()
            .ok_or_else(|| CasError::Other("Not logged in".to_string()))?;

        // Push each entity type
        if !pending.entries.is_empty() {
            match self.push_batch(&pending.entries, "entries", token) {
                Ok(count) => result.pushed_entries = count,
                Err(e) => {
                    result.errors.push(format!("Entry push failed: {e}"));
                    self.mark_batch_failed(&pending.entries, &e.to_string());
                }
            }
        }

        if !pending.tasks.is_empty() {
            match self.push_batch(&pending.tasks, "tasks", token) {
                Ok(count) => result.pushed_tasks = count,
                Err(e) => {
                    result.errors.push(format!("Task push failed: {e}"));
                    self.mark_batch_failed(&pending.tasks, &e.to_string());
                }
            }
        }

        if !pending.rules.is_empty() {
            match self.push_batch(&pending.rules, "rules", token) {
                Ok(count) => result.pushed_rules = count,
                Err(e) => {
                    result.errors.push(format!("Rule push failed: {e}"));
                    self.mark_batch_failed(&pending.rules, &e.to_string());
                }
            }
        }

        if !pending.skills.is_empty() {
            match self.push_batch(&pending.skills, "skills", token) {
                Ok(count) => result.pushed_skills = count,
                Err(e) => {
                    result.errors.push(format!("Skill push failed: {e}"));
                    self.mark_batch_failed(&pending.skills, &e.to_string());
                }
            }
        }

        // Push sessions (queued or directly passed)
        if !pending.sessions.is_empty() {
            match self.push_batch(&pending.sessions, "sessions", token) {
                Ok(count) => result.pushed_sessions = count,
                Err(e) => {
                    result.errors.push(format!("Session push failed: {e}"));
                    self.mark_batch_failed(&pending.sessions, &e.to_string());
                }
            }
        } else if !sessions.is_empty() {
            // Fallback to directly-passed sessions
            match self.push_sessions(sessions, token) {
                Ok(count) => result.pushed_sessions = count,
                Err(e) => {
                    result.errors.push(format!("Session push failed: {e}"));
                }
            }
        }

        // Push verifications
        if !pending.verifications.is_empty() {
            match self.push_batch(&pending.verifications, "verifications", token) {
                Ok(count) => result.pushed_verifications = count,
                Err(e) => {
                    result.errors.push(format!("Verification push failed: {e}"));
                    self.mark_batch_failed(&pending.verifications, &e.to_string());
                }
            }
        }

        // Push events
        if !pending.events.is_empty() {
            match self.push_batch(&pending.events, "events", token) {
                Ok(count) => result.pushed_events = count,
                Err(e) => {
                    result.errors.push(format!("Event push failed: {e}"));
                    self.mark_batch_failed(&pending.events, &e.to_string());
                }
            }
        }

        // Push prompts
        if !pending.prompts.is_empty() {
            match self.push_batch(&pending.prompts, "prompts", token) {
                Ok(count) => result.pushed_prompts = count,
                Err(e) => {
                    result.errors.push(format!("Prompt push failed: {e}"));
                    self.mark_batch_failed(&pending.prompts, &e.to_string());
                }
            }
        }

        // Push file changes
        if !pending.file_changes.is_empty() {
            match self.push_batch(&pending.file_changes, "file_changes", token) {
                Ok(count) => result.pushed_file_changes = count,
                Err(e) => {
                    result.errors.push(format!("FileChange push failed: {e}"));
                    self.mark_batch_failed(&pending.file_changes, &e.to_string());
                }
            }
        }

        // Push commit links
        if !pending.commit_links.is_empty() {
            match self.push_batch(&pending.commit_links, "commit_links", token) {
                Ok(count) => result.pushed_commit_links = count,
                Err(e) => {
                    result.errors.push(format!("CommitLink push failed: {e}"));
                    self.mark_batch_failed(&pending.commit_links, &e.to_string());
                }
            }
        }

        // Push agents
        if !pending.agents.is_empty() {
            match self.push_batch(&pending.agents, "agents", token) {
                Ok(count) => result.pushed_agents = count,
                Err(e) => {
                    result.errors.push(format!("Agent push failed: {e}"));
                    self.mark_batch_failed(&pending.agents, &e.to_string());
                }
            }
        }

        // Push worktrees
        if !pending.worktrees.is_empty() {
            match self.push_batch(&pending.worktrees, "worktrees", token) {
                Ok(count) => result.pushed_worktrees = count,
                Err(e) => {
                    result.errors.push(format!("Worktree push failed: {e}"));
                    self.mark_batch_failed(&pending.worktrees, &e.to_string());
                }
            }
        }

        // Update last push timestamp
        let _ = self
            .queue
            .set_metadata("last_push_at", &Utc::now().to_rfc3339());

        result.duration_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }

    /// Push sessions to cloud
    fn push_sessions(&self, sessions: &[Session], token: &str) -> Result<usize, CasError> {
        if sessions.is_empty() {
            return Ok(0);
        }

        let push_url = format!("{}/api/sync/push", self.cloud_config.endpoint);

        let mut payload = serde_json::Map::new();
        payload.insert("sessions".to_string(), serde_json::json!(sessions));

        // Include team_id if configured
        if let Some(team_id) = &self.cloud_config.team_id {
            payload.insert("team_id".to_string(), serde_json::json!(team_id));
        }

        // Include project_canonical_id (required for project scoping)
        let project_id = get_project_canonical_id()
            .ok_or_else(|| CasError::Other("Cannot sync: not inside a CAS project directory".to_string()))?;
        payload.insert(
            "project_canonical_id".to_string(),
            serde_json::json!(project_id),
        );

        // Include client version info for server-side compatibility checks
        Self::insert_client_version(&mut payload);

        let json_bytes = serde_json::to_vec(&payload)
            .map_err(|e| CasError::Other(format!("JSON serialization failed: {e}")))?;
        let compressed = Self::gzip_json(&json_bytes)?;

        let response = ureq::post(&push_url)
            .timeout(self.config.timeout)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json")
            .set("Content-Encoding", "gzip")
            .send_bytes(&compressed);

        match response {
            Ok(resp) if resp.status() == 200 || resp.status() == 201 => {
                // Update last session push timestamp
                let _ = self
                    .queue
                    .set_metadata("last_session_push_at", &Utc::now().to_rfc3339());
                Ok(sessions.len())
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!(
                    "Session push failed with status {status}: {body}"
                )))
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!(
                    "Session push failed with status {code}: {body}"
                )))
            }
            Err(ureq::Error::Transport(e)) => Err(CasError::Other(format!("Network error: {e}"))),
        }
    }

    fn push_batch(
        &self,
        items: &[QueuedSync],
        entity_type: &str,
        token: &str,
    ) -> Result<usize, CasError> {
        // Separate upserts and deletes
        let upsert_items: Vec<&QueuedSync> = items
            .iter()
            .filter(|i| i.operation == SyncOperation::Upsert)
            .collect();

        let deletes: Vec<&str> = items
            .iter()
            .filter(|i| i.operation == SyncOperation::Delete)
            .map(|i| i.entity_id.as_str())
            .collect();

        // Parse payloads to (item, json_value) tuples.
        // Items with a missing or unparseable payload cannot be pushed; park
        // them as failed immediately (mark_failed increments retry_count) so
        // they surface under `failed` in queue stats and don't consume a batch
        // slot on every push cycle.  After max_retries calls they leave
        // `pending` entirely, advancing `oldest_item` past them (defect B /
        // cas-8dd8 poison-head fix).
        let mut upsert_entries: Vec<(&QueuedSync, serde_json::Value)> = Vec::new();
        for item in &upsert_items {
            match item.payload.as_deref() {
                Some(payload) => match serde_json::from_str::<serde_json::Value>(payload) {
                    Ok(v) => upsert_entries.push((*item, v)),
                    Err(_) => {
                        let _ = self
                            .queue
                            .mark_failed(item.id, "invalid JSON payload for upsert");
                    }
                },
                None => {
                    let _ = self
                        .queue
                        .mark_failed(item.id, "missing payload for upsert operation");
                }
            }
        }

        let mut synced_count = 0;

        // Split upserts into size-limited sub-batches (consuming values to avoid cloning)
        if !upsert_entries.is_empty() {
            let sub_batches = self.split_into_sub_batches(upsert_entries);

            for sub_batch in sub_batches {
                let (batch_items, values): (Vec<&QueuedSync>, Vec<serde_json::Value>) =
                    sub_batch.into_iter().unzip();

                match self.push_sub_batch(values, entity_type, token) {
                    Ok(response) => {
                        // Defensive cross-check against the server-side
                        // `ON CONFLICT DO UPDATE ... WHERE false` silent-skip
                        // path (cas-0bdc / cas-d656): if the server reports
                        // any rows skipped for this entity type, we cannot
                        // know *which* items in `batch_items` were dropped
                        // (the server returns a count, not IDs). Conservative
                        // policy: leave the entire sub-batch un-marked-synced
                        // so all items remain retryable in the local queue.
                        // The next push cycle will re-send them; if the
                        // underlying cross-project conflict still stands the
                        // user will keep seeing the warning until the
                        // project_canonical_id mismatch is resolved.
                        //
                        // Backward-compat: older cloud builds omit `skipped`
                        // entirely, in which case `skipped_count` is 0 and
                        // we fall through to the legacy mark-synced path.
                        let skipped_count = response.skipped_count_for(entity_type);
                        if skipped_count > 0 {
                            let batch_size = batch_items.len();
                            warn!(
                                entity_type = entity_type,
                                skipped = skipped_count,
                                batch_size = batch_size,
                                "Cloud server skipped {skipped_count} {entity_type} row(s) in a sub-batch of {batch_size} \
                                 (likely cross-project project_canonical_id conflict); leaving sub-batch un-synced for retry",
                            );
                            // Do NOT mark items synced and do NOT return an
                            // Err here: the HTTP call itself succeeded; only
                            // some rows were silently rejected. Leaving items
                            // un-touched keeps them retryable in the local
                            // queue. Continuing the loop lets later
                            // sub-batches and entity types proceed normally.
                        } else {
                            for item in &batch_items {
                                let _ = self.queue.mark_synced(item.id);
                                synced_count += 1;
                            }
                        }
                    }
                    Err(e) => {
                        // Mark this sub-batch as failed but continue with others
                        for item in &batch_items {
                            let _ = self.queue.mark_failed(item.id, &e.to_string());
                        }
                        // If any sub-batch fails, report the error
                        return Err(e);
                    }
                }
            }
        }

        // Send individual delete requests
        for cas_id in deletes {
            let delete_url = format!(
                "{}/api/sync/{}/{}",
                self.cloud_config.endpoint, entity_type, cas_id
            );

            let response = ureq::delete(&delete_url)
                .timeout(self.config.timeout)
                .set("Authorization", &format!("Bearer {token}"))
                .call();

            match response {
                Ok(resp) if resp.status() == 200 || resp.status() == 404 => {
                    // Mark delete as synced (404 means already deleted, that's fine)
                    if let Some(item) = items
                        .iter()
                        .find(|i| i.entity_id == cas_id && i.operation == SyncOperation::Delete)
                    {
                        let _ = self.queue.mark_synced(item.id);
                        synced_count += 1;
                    }
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.into_string().unwrap_or_default();
                    eprintln!("Delete {cas_id} failed with status {status}: {body}");
                }
                Err(e) => {
                    eprintln!("Delete {cas_id} failed: {e}");
                }
            }
        }

        Ok(synced_count)
    }

    /// Split upsert entries into sub-batches that each stay under max_payload_bytes.
    /// Takes ownership of entries to avoid cloning serde_json::Value.
    pub(crate) fn split_into_sub_batches<'a>(
        &self,
        entries: Vec<(&'a QueuedSync, serde_json::Value)>,
    ) -> Vec<Vec<(&'a QueuedSync, serde_json::Value)>> {
        let max_bytes = self.config.max_payload_bytes;
        let overhead = 256;
        let mut batches = Vec::new();
        let mut current_batch: Vec<(&QueuedSync, serde_json::Value)> = Vec::new();
        let mut current_size = overhead;

        for (item, value) in entries {
            let item_size = item.payload.as_ref().map(|p| p.len()).unwrap_or(256);
            let item_total = item_size + 1;

            if !current_batch.is_empty() && current_size + item_total > max_bytes {
                batches.push(current_batch);
                current_batch = Vec::new();
                current_size = overhead;
            }

            current_batch.push((item, value));
            current_size += item_total;
        }

        if !current_batch.is_empty() {
            batches.push(current_batch);
        }

        batches
    }

    /// Gzip-compress a JSON payload.
    pub(crate) fn gzip_json(json_bytes: &[u8]) -> Result<Vec<u8>, CasError> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder
            .write_all(json_bytes)
            .map_err(|e| CasError::Other(format!("Gzip compression failed: {e}")))?;
        encoder
            .finish()
            .map_err(|e| CasError::Other(format!("Gzip finalize failed: {e}")))
    }

    /// Push a single sub-batch of upsert values with retry.
    ///
    /// Returns the parsed [`PushResponse`] on success. Callers should
    /// inspect `PushResponse::skipped_count_for(entity_type)` and treat any
    /// non-zero count as a signal that the server silently skipped some
    /// rows (see `PushResponse` docs and cas-f645 for the cross-project
    /// conflict contract). When the response body is empty or fails to
    /// parse (e.g. older cloud build returning a different shape), a
    /// `PushResponse::default()` is returned — `skipped` is then `None`,
    /// which `skipped_count_for` reports as `0`, preserving legacy
    /// "trust the 200" behavior.
    pub(super) fn push_sub_batch(
        &self,
        values: Vec<serde_json::Value>,
        entity_type: &str,
        token: &str,
    ) -> Result<PushResponse, CasError> {
        let push_url = format!("{}/api/sync/push", self.cloud_config.endpoint);

        let mut payload = serde_json::Map::new();
        payload.insert(
            entity_type.to_string(),
            serde_json::Value::Array(values.to_vec()),
        );

        if let Some(team_id) = &self.cloud_config.team_id {
            payload.insert("team_id".to_string(), serde_json::json!(team_id));
        }

        let project_id = get_project_canonical_id()
            .ok_or_else(|| CasError::Other("Cannot sync: not inside a CAS project directory".to_string()))?;
        payload.insert(
            "project_canonical_id".to_string(),
            serde_json::json!(project_id),
        );

        // Include client version info for server-side compatibility checks
        Self::insert_client_version(&mut payload);

        // Serialize and compress the payload
        let json_bytes = serde_json::to_vec(&payload)
            .map_err(|e| CasError::Other(format!("JSON serialization failed: {e}")))?;
        let compressed = Self::gzip_json(&json_bytes)?;

        let mut last_error = None;
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(self.config.backoff_duration(attempt as u32));
            }

            let response = ureq::post(&push_url)
                .timeout(self.config.timeout)
                .set("Authorization", &format!("Bearer {token}"))
                .set("Content-Type", "application/json")
                .set("Content-Encoding", "gzip")
                .send_bytes(&compressed);

            match response {
                Ok(resp) => {
                    if resp.status() == 200 || resp.status() == 201 {
                        // Read body so we can defensively inspect the
                        // server's `skipped` field. Treat parse failures
                        // (empty body, older cloud shape) as
                        // `PushResponse::default()` for backward compat —
                        // the 2xx status is the source of truth that the
                        // HTTP exchange itself succeeded.
                        let body = resp.into_string().unwrap_or_default();
                        let parsed: PushResponse = if body.is_empty() {
                            PushResponse::default()
                        } else {
                            serde_json::from_str(&body).unwrap_or_default()
                        };
                        return Ok(parsed);
                    } else {
                        let status = resp.status();
                        let body = resp.into_string().unwrap_or_default();
                        last_error = Some(CasError::Other(format!(
                            "Push failed with status {status}: {body}"
                        )));
                        if (400..500).contains(&status) {
                            break;
                        }
                    }
                }
                Err(ureq::Error::Status(code, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    last_error = Some(CasError::Other(format!(
                        "Push failed with status {code}: {body}"
                    )));
                    if (400..500).contains(&code) {
                        break;
                    }
                }
                Err(ureq::Error::Transport(e)) => {
                    last_error = Some(CasError::Other(format!("Network error: {e}")));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| CasError::Other("Push failed".to_string())))
    }

    fn mark_batch_failed(&self, items: &[QueuedSync], error: &str) {
        for item in items {
            let _ = self.queue.mark_failed(item.id, error);
        }
    }

    /// Insert client version fields into a push payload.
    pub(crate) fn insert_client_version(payload: &mut serde_json::Map<String, serde_json::Value>) {
        payload.insert(
            "client_version".to_string(),
            serde_json::json!(env!("CARGO_PKG_VERSION")),
        );
        payload.insert(
            "client_build".to_string(),
            serde_json::json!(option_env!("CAS_GIT_HASH").unwrap_or("unknown")),
        );
    }
}
