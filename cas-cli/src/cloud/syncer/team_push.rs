use std::time::Instant;

use crate::cloud::syncer::{
    CloudSyncer, GroupedQueuedItems, PushItemizedFailure, SyncResult, TeamPushResponse,
    itemized_failures_for,
};
use crate::cloud::{EntityType, QueuedSync, SyncOperation, get_project_canonical_id};
use crate::error::CasError;
use chrono::Utc;

impl CloudSyncer {
    pub fn push_team(&self, team_id: &str) -> Result<SyncResult, CasError> {
        let mut result = SyncResult::default();
        let start = Instant::now();

        if !self.is_available() {
            return Ok(result);
        }

        // Fetch (but do NOT delete) pending team items so we can
        // mark_failed / mark_synced per item after the HTTP call completes.
        // Using drain_by_team here would delete items up-front and then
        // re-enqueue them via enqueue_for_team on failure, which resets
        // retry_count to 0 (ON CONFLICT DO UPDATE) — preventing items from
        // ever reaching the `failed` bucket (defect B / cas-8dd8).
        let queued = self
            .queue
            .pending_for_team(team_id, usize::MAX, self.config.max_retries)?;

        if queued.is_empty() {
            result.duration_ms = start.elapsed().as_millis() as u64;
            return Ok(result);
        }

        let token = self
            .cloud_config
            .token
            .as_ref()
            .ok_or_else(|| CasError::Other("Not logged in".to_string()))?;

        // The same stale-tombstone hazard applies to the team copy of a
        // dual-enqueued task/entry delete. Filter those rows before grouping,
        // otherwise `cas cloud sync` would protect the personal row and then
        // immediately replay the destructive team delete.
        let mut sendable = Vec::with_capacity(queued.len());
        for item in queued {
            if item.operation == SyncOperation::Delete {
                match self.queue.neutralize_delete_if_local_entity_exists(&item) {
                    Ok(true) => {
                        tracing::warn!(
                            entity_type = %item.entity_type,
                            entity_id = %item.entity_id,
                            queue_id = item.id,
                            team_id,
                            "Neutralized stale team cloud delete because the entity still exists locally"
                        );
                        continue;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        let error = format!(
                            "Team delete safety check failed for {} {}: {e}",
                            item.entity_type, item.entity_id
                        );
                        let _ = self.queue.mark_failed(item.id, &error);
                        result.errors.push(error);
                        continue;
                    }
                }
            }
            sendable.push(item);
        }
        let queued = sendable;

        if queued.is_empty() {
            result.duration_ms = start.elapsed().as_millis() as u64;
            return Ok(result);
        }

        // Group deletes for the existing one-per-entity delete path. Upserts
        // stay associated with their queue rows below so sub-batches can be
        // marked synced/failed independently.
        let grouped = self.group_queued_items(&queued);

        // Include project_canonical_id (required for project scoping)
        let project_id = get_project_canonical_id().ok_or_else(|| {
            CasError::Other("Cannot sync: not inside a Cassy project directory".to_string())
        })?;

        // cas-8ca5 / contract §5: include the normalized git remote so the
        // server's project resolver can map an unpinned machine onto the team's
        // canonical bucket instead of fragmenting onto github.com/<org>/<repo>.
        // Lowercased to match the server's `normalizeGitRemote` rule.
        let git_remote = crate::store::find_cas_root()
            .ok()
            .and_then(|cas_root| crate::cloud::normalized_git_remote_for_push(&cas_root));

        let has_deletes = !grouped.delete_entries.is_empty()
            || !grouped.delete_tasks.is_empty()
            || !grouped.delete_rules.is_empty()
            || !grouped.delete_skills.is_empty()
            || !grouped.delete_sessions.is_empty()
            || !grouped.delete_verifications.is_empty()
            || !grouped.delete_events.is_empty()
            || !grouped.delete_prompts.is_empty()
            || !grouped.delete_file_changes.is_empty()
            || !grouped.delete_commit_links.is_empty()
            || !grouped.delete_agents.is_empty()
            || !grouped.delete_worktrees.is_empty();

        for (entity_type, entity_key) in [
            (EntityType::Entry, "entries"),
            (EntityType::Task, "tasks"),
            (EntityType::Rule, "rules"),
            (EntityType::Skill, "skills"),
            (EntityType::Session, "sessions"),
            (EntityType::Verification, "verifications"),
            (EntityType::Event, "events"),
            (EntityType::Prompt, "prompts"),
            (EntityType::FileChange, "file_changes"),
            (EntityType::CommitLink, "commit_links"),
            (EntityType::Agent, "agents"),
            (EntityType::Worktree, "worktrees"),
        ] {
            let (synced, errors) = self.push_team_upserts_for_type(
                team_id,
                &queued,
                entity_type,
                entity_key,
                token,
                &project_id,
                git_remote.as_deref(),
            );
            Self::add_team_count(&mut result, entity_key, synced);
            result.errors.extend(errors);
        }

        if result.errors.is_empty() && has_deletes {
            // Process deletes (after successful upserts or if no upserts)
            let (deleted_count, delete_errors) =
                self.send_team_deletes(team_id, &grouped, token, &project_id);
            // Track successful deletes (deleted_count is total across all types)
            if deleted_count > 0 {
                // Note: delete counts aren't tracked separately in SyncResult,
                // they're part of the overall push operation
                let _ = deleted_count; // Acknowledge the count
            }
            if !delete_errors.is_empty() {
                result.errors.extend(delete_errors);
            } else {
                for item in queued
                    .iter()
                    .filter(|item| item.operation == SyncOperation::Delete)
                {
                    let _ = self.queue.mark_synced(item.id);
                }
            }
        }

        // Update team sync timestamp on success
        if result.errors.is_empty() {
            let _ = self.queue.set_metadata(
                &format!("last_team_push_at_{team_id}"),
                &Utc::now().to_rfc3339(),
            );
        }

        result.duration_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }

    fn push_team_upserts_for_type(
        &self,
        team_id: &str,
        queued: &[QueuedSync],
        entity_type: EntityType,
        entity_key: &str,
        token: &str,
        project_id: &str,
        git_remote: Option<&str>,
    ) -> (usize, Vec<String>) {
        let mut upserts = Vec::new();

        for item in queued.iter().filter(|item| {
            item.operation == SyncOperation::Upsert && item.entity_type == entity_type
        }) {
            match item.payload.as_deref() {
                Some(payload) => match serde_json::from_str::<serde_json::Value>(payload) {
                    Ok(value) => upserts.push((item, value)),
                    Err(_) => {
                        let _ = self
                            .queue
                            .mark_failed(item.id, "invalid JSON payload for team upsert");
                    }
                },
                None => {
                    let _ = self
                        .queue
                        .mark_failed(item.id, "missing payload for team upsert operation");
                }
            }
        }

        let mut synced = 0;
        let mut errors = Vec::new();

        for sub_batch in self.split_into_sub_batches(upserts) {
            let (batch_items, values): (Vec<&QueuedSync>, Vec<serde_json::Value>) =
                sub_batch.into_iter().unzip();
            let sent_count = values.len();

            match self
                .push_team_sub_batch(team_id, entity_key, values, token, project_id, git_remote)
            {
                Ok(response) => {
                    if let Some(body) = response.as_ref() {
                        self.maybe_adopt_team_canonical_id(body);
                    }

                    let raw_response = response.as_ref().map_or("", |body| body.raw_body.as_str());
                    let (accepted, skipped) = match response.as_ref() {
                        Some(body) => match Self::team_counts_for(body, entity_key) {
                            Ok(Some(counts)) => counts,
                            Ok(None) => (sent_count, 0),
                            Err(error) => {
                                let diagnostic = format!(
                                    "{entity_key} push returned an unrecognized count/skip signal: {error}; marking {} team row(s) failed; server response: {}",
                                    batch_items.len(),
                                    body.raw_body
                                );
                                for item in &batch_items {
                                    let _ = self.queue.mark_failed(item.id, &diagnostic);
                                }
                                errors.push(diagnostic);
                                continue;
                            }
                        },
                        None => (sent_count, 0),
                    };
                    if skipped > 0 {
                        let itemized = Self::team_itemized_failures_for(
                            response
                                .as_ref()
                                .expect("skipped counts require a team response body"),
                            entity_key,
                            skipped,
                            batch_items.iter().map(|item| item.entity_id.clone()),
                        );
                        let itemized = match itemized {
                            Ok(Some(rejections)) => rejections,
                            Ok(None) => {
                                let diagnostic = format!(
                                    "cloud skipped {skipped} of {} team {entity_key} row(s); marking the indistinguishable sub-batch failed; server response: {}",
                                    batch_items.len(),
                                    raw_response
                                );
                                for item in &batch_items {
                                    let _ = self.queue.mark_failed(item.id, &diagnostic);
                                }
                                errors.push(diagnostic);
                                continue;
                            }
                            Err(error) => {
                                let diagnostic = format!(
                                    "team {entity_key} push returned invalid itemized failures: {error}; marking {} row(s) failed; server response: {}",
                                    batch_items.len(),
                                    raw_response
                                );
                                for item in &batch_items {
                                    let _ = self.queue.mark_failed(item.id, &diagnostic);
                                }
                                errors.push(diagnostic);
                                continue;
                            }
                        };

                        for item in &batch_items {
                            if let Some(failure) = itemized.get(&item.entity_id) {
                                let diagnostic = match failure {
                                    PushItemizedFailure::Rejection(rejection) => format!(
                                        "permanent cloud rejection: reason={}; entity={entity_key}; id={}; existing_project={}",
                                        rejection.reason.as_str(),
                                        rejection.id,
                                        rejection.existing_canonical_id,
                                    ),
                                    PushItemizedFailure::Invalid(invalid) => format!(
                                        "cloud invalid team {entity_key} {}: {} ({}); server response: {}",
                                        invalid.id,
                                        invalid.reason.as_str(),
                                        invalid.detail,
                                        raw_response
                                    ),
                                };
                                if matches!(failure, PushItemizedFailure::Rejection(rejection) if rejection.reason.is_permanent())
                                {
                                    let _ = self.queue.park_failed(
                                        item.id,
                                        &diagnostic,
                                        self.config.max_retries,
                                    );
                                } else {
                                    let _ = self.queue.mark_failed(item.id, &diagnostic);
                                }
                                errors.push(diagnostic);
                            } else {
                                let _ = self.queue.mark_synced(item.id);
                                synced += 1;
                            }
                        }
                        continue;
                    }

                    for item in &batch_items {
                        let _ = self.queue.mark_synced(item.id);
                    }
                    synced += accepted;
                }
                Err(e) => {
                    for item in &batch_items {
                        let _ = self.queue.mark_failed(item.id, &e.to_string());
                    }
                    errors.push(format!("{entity_key} push failed: {e}"));
                }
            }
        }

        (synced, errors)
    }

    fn push_team_sub_batch(
        &self,
        team_id: &str,
        entity_key: &str,
        values: Vec<serde_json::Value>,
        token: &str,
        project_id: &str,
        git_remote: Option<&str>,
    ) -> Result<Option<TeamPushResponse>, CasError> {
        let push_url = format!(
            "{}/api/teams/{}/sync/push",
            self.cloud_config.endpoint, team_id
        );

        let mut payload = serde_json::Map::new();
        payload.insert(entity_key.to_string(), serde_json::Value::Array(values));
        payload.insert(
            "project_canonical_id".to_string(),
            serde_json::json!(project_id),
        );
        if let Some(remote) = git_remote {
            payload.insert("git_remote".to_string(), serde_json::json!(remote));
        }
        Self::insert_client_version(&mut payload);

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
                        let body = resp.into_string().unwrap_or_default();
                        if body.is_empty() {
                            return Ok(None);
                        }
                        let mut parsed = serde_json::from_str::<TeamPushResponse>(&body).ok();
                        if let Some(response) = parsed.as_mut() {
                            response.raw_body = body;
                        }
                        return Ok(parsed);
                    }

                    let status = resp.status();
                    let body = resp.into_string().unwrap_or_default();
                    last_error = Some(CasError::Other(format!(
                        "Team push failed with status {status}: {body}"
                    )));
                    if (400..500).contains(&status) {
                        break;
                    }
                }
                Err(ureq::Error::Status(code, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    last_error = Some(CasError::Other(format!(
                        "Team push failed with status {code}: {body}"
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

        Err(last_error.unwrap_or_else(|| CasError::Other("Team push failed".to_string())))
    }

    /// Return `(accepted, skipped)` for one entity in a team push response.
    /// `None` means an older response omitted the entity entirely, preserving
    /// the historical trust-the-2xx fallback. A present but unknown shape is
    /// an error so queue rows remain retryable.
    fn team_counts_for(
        response: &TeamPushResponse,
        entity_key: &str,
    ) -> Result<Option<(usize, usize)>, String> {
        fn count(value: &serde_json::Value, location: &str) -> Result<usize, String> {
            value
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| format!("unrecognized count at {location}: {value}"))
        }

        let synced = response
            .synced
            .as_object()
            .ok_or_else(|| format!("synced is not an object: {}", response.synced))?;
        let Some(entity) = synced.get(entity_key) else {
            return Ok(None);
        };

        if entity.is_number() {
            return count(entity, &format!("synced.{entity_key}")).map(|n| Some((n, 0)));
        }

        let detail = entity
            .as_object()
            .ok_or_else(|| format!("synced.{entity_key} is not a count object: {entity}"))?;
        let inserted = detail
            .get("inserted")
            .map(|value| count(value, &format!("synced.{entity_key}.inserted")))
            .transpose()?
            .unwrap_or(0);
        let updated = detail
            .get("updated")
            .map(|value| count(value, &format!("synced.{entity_key}.updated")))
            .transpose()?
            .unwrap_or(0);
        let skipped = detail
            .get("skipped")
            .map(|value| count(value, &format!("synced.{entity_key}.skipped")))
            .transpose()?
            .unwrap_or(0);

        if !detail.contains_key("inserted")
            && !detail.contains_key("updated")
            && !detail.contains_key("skipped")
        {
            return Err(format!(
                "synced.{entity_key} has no inserted, updated, or skipped count"
            ));
        }

        Ok(Some((inserted.saturating_add(updated), skipped)))
    }

    fn team_itemized_failures_for(
        response: &TeamPushResponse,
        entity_key: &str,
        skipped: usize,
        queued_ids: impl Iterator<Item = String>,
    ) -> Result<
        Option<std::collections::HashMap<String, crate::cloud::syncer::PushItemizedFailure>>,
        String,
    > {
        let synced = response
            .synced
            .as_object()
            .ok_or_else(|| format!("synced is not an object: {}", response.synced))?;
        let entity = synced
            .get(entity_key)
            .ok_or_else(|| format!("synced.{entity_key} missing despite skipped count"))?;
        itemized_failures_for(entity, &format!("synced.{entity_key}"), skipped, queued_ids)
    }

    fn add_team_count(result: &mut SyncResult, entity_key: &str, count: usize) {
        match entity_key {
            "entries" => result.pushed_entries += count,
            "tasks" => result.pushed_tasks += count,
            "rules" => result.pushed_rules += count,
            "skills" => result.pushed_skills += count,
            "sessions" => result.pushed_sessions += count,
            "verifications" => result.pushed_verifications += count,
            "events" => result.pushed_events += count,
            "prompts" => result.pushed_prompts += count,
            "file_changes" => result.pushed_file_changes += count,
            "commit_links" => result.pushed_commit_links += count,
            "agents" => result.pushed_agents += count,
            "worktrees" => result.pushed_worktrees += count,
            _ => {}
        }
    }

    fn maybe_adopt_team_canonical_id(&self, body: &TeamPushResponse) {
        // cas-8ca5 / contract §5: adopt the server's canonical id when our git
        // remote matches the returned git_remote. Stops an unpinned machine from
        // continuing to sync the fragmented per-remote bucket instead of the
        // team's slug.
        if let Ok(cas_root) = crate::store::find_cas_root() {
            let local_remote = crate::cloud::derive_canonical_id_from_git_remote(&cas_root);
            let current_pin = crate::cloud::canonical_id_from_config_toml(&cas_root);
            if let Some(adopted) = crate::cloud::should_adopt_canonical_id(
                local_remote.as_deref(),
                body.git_remote.as_deref(),
                body.canonical_id.as_deref(),
                current_pin.as_deref(),
            ) {
                match crate::cloud::set_canonical_id_in_config_toml(&cas_root, &adopted) {
                    Ok(()) => {
                        crate::cloud::invalidate_cached_project_id();
                        tracing::info!(
                            canonical_id = %adopted,
                            "cas-8ca5: adopted server canonical project id"
                        );
                        eprintln!(
                            "[Cassy sync] adopted team canonical project id \
                             '{adopted}' (matched git remote)"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "cas-8ca5: failed to persist adopted canonical_id"
                        );
                    }
                }
            }
        }
    }

    /// Group queued items by entity type and operation
    fn group_queued_items(&self, items: &[QueuedSync]) -> GroupedQueuedItems {
        let mut result = GroupedQueuedItems::default();

        for item in items {
            match item.operation {
                SyncOperation::Upsert => {
                    if let Some(payload) = &item.payload {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) {
                            match item.entity_type {
                                EntityType::Entry => result.upsert_entries.push(value),
                                EntityType::Task => result.upsert_tasks.push(value),
                                EntityType::Rule => result.upsert_rules.push(value),
                                EntityType::Skill => result.upsert_skills.push(value),
                                EntityType::Session => result.upsert_sessions.push(value),
                                EntityType::Verification => result.upsert_verifications.push(value),
                                EntityType::Event => result.upsert_events.push(value),
                                EntityType::Prompt => result.upsert_prompts.push(value),
                                EntityType::FileChange => result.upsert_file_changes.push(value),
                                EntityType::CommitLink => result.upsert_commit_links.push(value),
                                EntityType::Agent => result.upsert_agents.push(value),
                                EntityType::Worktree => result.upsert_worktrees.push(value),
                                // Pages ship via `push_knowledge_pages` (it
                                // needs the body from disk), so nothing
                                // enqueues them today. Arm kept explicit so a
                                // future queue producer is a compile error.
                                EntityType::KnowledgePage => {}
                            }
                        }
                    }
                }
                SyncOperation::Delete => match item.entity_type {
                    EntityType::Entry => result.delete_entries.push(item.entity_id.clone()),
                    EntityType::Task => result.delete_tasks.push(item.entity_id.clone()),
                    EntityType::Rule => result.delete_rules.push(item.entity_id.clone()),
                    EntityType::Skill => result.delete_skills.push(item.entity_id.clone()),
                    EntityType::Session => result.delete_sessions.push(item.entity_id.clone()),
                    EntityType::Verification => {
                        result.delete_verifications.push(item.entity_id.clone())
                    }
                    EntityType::Event => result.delete_events.push(item.entity_id.clone()),
                    EntityType::Prompt => result.delete_prompts.push(item.entity_id.clone()),
                    EntityType::FileChange => {
                        result.delete_file_changes.push(item.entity_id.clone())
                    }
                    EntityType::CommitLink => {
                        result.delete_commit_links.push(item.entity_id.clone())
                    }
                    EntityType::Agent => result.delete_agents.push(item.entity_id.clone()),
                    EntityType::Worktree => result.delete_worktrees.push(item.entity_id.clone()),
                    EntityType::KnowledgePage => {}
                },
            }
        }

        result
    }

    /// Send team delete requests for each entity type
    fn send_team_deletes(
        &self,
        team_id: &str,
        grouped: &GroupedQueuedItems,
        token: &str,
        project_id: &str,
    ) -> (usize, Vec<String>) {
        let mut deleted = 0;
        let mut errors = Vec::new();

        // Delete entries
        for cas_id in &grouped.delete_entries {
            match self.send_team_delete(team_id, EntityType::Entry, cas_id, token, project_id) {
                Ok(()) => deleted += 1,
                Err(e) => errors.push(format!("Entry delete {cas_id}: {e}")),
            }
        }

        // Delete tasks
        for cas_id in &grouped.delete_tasks {
            match self.send_team_delete(team_id, EntityType::Task, cas_id, token, project_id) {
                Ok(()) => deleted += 1,
                Err(e) => errors.push(format!("Task delete {cas_id}: {e}")),
            }
        }

        // Delete rules
        for cas_id in &grouped.delete_rules {
            match self.send_team_delete(team_id, EntityType::Rule, cas_id, token, project_id) {
                Ok(()) => deleted += 1,
                Err(e) => errors.push(format!("Rule delete {cas_id}: {e}")),
            }
        }

        // Delete skills
        for cas_id in &grouped.delete_skills {
            match self.send_team_delete(team_id, EntityType::Skill, cas_id, token, project_id) {
                Ok(()) => deleted += 1,
                Err(e) => errors.push(format!("Skill delete {cas_id}: {e}")),
            }
        }

        // Delete sessions
        for cas_id in &grouped.delete_sessions {
            match self.send_team_delete(team_id, EntityType::Session, cas_id, token, project_id) {
                Ok(()) => deleted += 1,
                Err(e) => errors.push(format!("Session delete {cas_id}: {e}")),
            }
        }

        // Delete verifications
        for cas_id in &grouped.delete_verifications {
            match self.send_team_delete(
                team_id,
                EntityType::Verification,
                cas_id,
                token,
                project_id,
            ) {
                Ok(()) => deleted += 1,
                Err(e) => errors.push(format!("Verification delete {cas_id}: {e}")),
            }
        }

        // Delete events
        for cas_id in &grouped.delete_events {
            match self.send_team_delete(team_id, EntityType::Event, cas_id, token, project_id) {
                Ok(()) => deleted += 1,
                Err(e) => errors.push(format!("Event delete {cas_id}: {e}")),
            }
        }

        // Delete prompts
        for cas_id in &grouped.delete_prompts {
            match self.send_team_delete(team_id, EntityType::Prompt, cas_id, token, project_id) {
                Ok(()) => deleted += 1,
                Err(e) => errors.push(format!("Prompt delete {cas_id}: {e}")),
            }
        }

        // Delete file changes
        for cas_id in &grouped.delete_file_changes {
            match self.send_team_delete(team_id, EntityType::FileChange, cas_id, token, project_id)
            {
                Ok(()) => deleted += 1,
                Err(e) => errors.push(format!("FileChange delete {cas_id}: {e}")),
            }
        }

        // Delete commit links
        for cas_id in &grouped.delete_commit_links {
            match self.send_team_delete(team_id, EntityType::CommitLink, cas_id, token, project_id)
            {
                Ok(()) => deleted += 1,
                Err(e) => errors.push(format!("CommitLink delete {cas_id}: {e}")),
            }
        }

        // Delete agents
        for cas_id in &grouped.delete_agents {
            match self.send_team_delete(team_id, EntityType::Agent, cas_id, token, project_id) {
                Ok(()) => deleted += 1,
                Err(e) => errors.push(format!("Agent delete {cas_id}: {e}")),
            }
        }

        (deleted, errors)
    }

    /// Send a single team delete request
    fn send_team_delete(
        &self,
        team_id: &str,
        entity_type: EntityType,
        cas_id: &str,
        token: &str,
        project_id: &str,
    ) -> Result<(), CasError> {
        let delete_url = format!(
            "{}/api/teams/{}/sync/{}/{}?project_id={}",
            self.cloud_config.endpoint,
            team_id,
            entity_type.as_str(),
            cas_id,
            project_id.replace('/', "%2F")
        );

        let response = ureq::delete(&delete_url)
            .timeout(self.config.timeout)
            .set("Authorization", &format!("Bearer {token}"))
            .call();

        match response {
            Ok(resp) if (200..300).contains(&resp.status()) => Ok(()),
            Ok(resp) => {
                let status = resp.status();
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!(
                    "Delete failed with status {status}: {body}"
                )))
            }
            Err(ureq::Error::Status(404, _)) => Ok(()),
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!(
                    "Delete failed with status {code}: {body}"
                )))
            }
            Err(ureq::Error::Transport(e)) => Err(CasError::Other(format!("Network error: {e}"))),
        }
    }
}
