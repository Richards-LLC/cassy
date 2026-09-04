use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::cloud::syncer::{
    CloudSyncer, PushItemizedFailure, PushRowResult, SyncResult, TeamPushResponse,
    itemized_failures_for, row_results_for,
};
use crate::cloud::{
    EntityType, QueuedSync, SyncOperation, canonical_project_id_with_pin,
};
use crate::error::CasError;
use chrono::Utc;

fn stamp_task_origin_project(value: &mut serde_json::Value, project_id: &str) {
    if let Some(task) = value.as_object_mut() {
        // Global tasks are personal-only and deliberately carry no project
        // identity in their queued payload. Leave both fields untouched if a
        // legacy caller routes one through this helper.
        if task.get("scope").and_then(serde_json::Value::as_str) == Some("global") {
            return;
        }

        // Team task rows are project-scoped. Legacy queue payloads may have
        // been serialized before Task::scope existed, but the cloud contract
        // requires the explicit field or it may accept the batch while
        // silently skipping the row.
        task.insert(
            "scope".to_string(),
            serde_json::Value::String("project".to_string()),
        );

        // Supervisor reassignment is carried by a non-empty origin_project in
        // the queued payload. Only legacy rows without a usable identity need
        // to inherit the project performing the push.
        let explicit_origin = task
            .get("origin_project")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|origin| !origin.is_empty());
        if let Some(origin) = explicit_origin {
            if let Some(canonical) = canonical_project_id_with_pin(origin, Some(project_id)) {
                task.insert(
                    "origin_project".to_string(),
                    serde_json::Value::String(canonical),
                );
            }
        } else {
            task.insert(
                "origin_project".to_string(),
                serde_json::Value::String(
                    canonical_project_id_with_pin(project_id, Some(project_id))
                        .unwrap_or_else(|| project_id.to_string()),
                ),
            );
        }
    }
}

fn stamp_task_dependency_origin_project(value: &mut serde_json::Value, project_id: &str) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let explicit_origin = object
        .get("origin_project")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|origin| !origin.is_empty());
    if let Some(origin) = explicit_origin {
        if let Some(canonical) = canonical_project_id_with_pin(origin, Some(project_id)) {
            object.insert(
                "origin_project".to_string(),
                serde_json::Value::String(canonical),
            );
        }
    } else {
        object.insert(
            "origin_project".to_string(),
            serde_json::Value::String(
                canonical_project_id_with_pin(project_id, Some(project_id))
                    .unwrap_or_else(|| project_id.to_string()),
            ),
        );
    }
}

impl CloudSyncer {
    pub fn push_team(&self, team_id: &str) -> Result<SyncResult, CasError> {
        let mut result = SyncResult::default();
        let start = Instant::now();

        self.requeue_version_gated_items()?;

        if !self.is_available() {
            return Ok(result);
        }

        // Keep team-scoped writes behind the same per-root identity guard as
        // personal writes. Otherwise a `cas cloud sync` from an unpinned,
        // no-remote fixture could still mint a team bucket after personal
        // push declined the root.
        if let Some(refusal) = self.ephemeral_project_refusal() {
            tracing::warn!("[Cassy sync] {refusal}");
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
            if item.operation == SyncOperation::Delete && item.project_id.is_none() {
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

        // Include project_canonical_id (required for project scoping)
        let project_id = self.personal_push_project_id()?;

        // cas-8ca5 / contract §5: include the normalized git remote so the
        // server's project resolver can map an unpinned machine onto the team's
        // canonical bucket instead of fragmenting onto github.com/<org>/<repo>.
        // Lowercased to match the server's `normalizeGitRemote` rule.
        let git_remote = self
            .push_cas_root
            .as_deref()
            .and_then(crate::cloud::normalized_git_remote_for_push);

        let has_deletes = queued
            .iter()
            .any(|item| item.operation == SyncOperation::Delete);

        // A project move must remove the old (project, id) key before the
        // replacement upsert is sent. A failed move-delete blocks only its
        // matching upsert, leaving both rows queued with the same diagnostic.
        let move_deletes: Vec<&QueuedSync> = queued
            .iter()
            .filter(|item| item.operation == SyncOperation::Delete && item.project_id.is_some())
            .collect();
        let mut blocked_upserts = HashSet::new();
        if !move_deletes.is_empty() {
            let (successful, failures) =
                self.send_team_deletes(team_id, &move_deletes, token, &project_id);
            for queue_id in successful {
                let _ = self.queue.mark_synced(queue_id);
            }
            for (queue_id, error) in failures {
                let _ = self.queue.mark_failed(queue_id, &error);
                if let Some(delete) = queued.iter().find(|item| item.id == queue_id) {
                    for upsert in queued.iter().filter(|item| {
                        item.operation == SyncOperation::Upsert
                            && item.entity_type == delete.entity_type
                            && item.entity_id == delete.entity_id
                    }) {
                        let _ = self.queue.mark_failed(upsert.id, &error);
                        blocked_upserts.insert(upsert.id);
                    }
                }
                result.errors.push(error);
            }
        }

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
            (EntityType::TaskDependency, "task_dependencies"),
        ] {
            let (synced, errors) = self.push_team_upserts_for_type(
                team_id,
                &queued,
                entity_type,
                entity_key,
                token,
                &project_id,
                git_remote.as_deref(),
                &blocked_upserts,
            );
            Self::add_team_count(&mut result, entity_key, synced);
            result.errors.extend(errors);
        }

        let normal_deletes: Vec<&QueuedSync> = queued
            .iter()
            .filter(|item| item.operation == SyncOperation::Delete && item.project_id.is_none())
            .collect();
        if result.errors.is_empty() && has_deletes && !normal_deletes.is_empty() {
            // Ordinary deletes retain the historical after-upsert ordering.
            let (successful, failures) =
                self.send_team_deletes(team_id, &normal_deletes, token, &project_id);
            for queue_id in successful {
                let _ = self.queue.mark_synced(queue_id);
            }
            for (queue_id, error) in failures {
                let _ = self.queue.mark_failed(queue_id, &error);
                result.errors.push(error);
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
        blocked_upserts: &HashSet<i64>,
    ) -> (usize, Vec<String>) {
        // A move replacement (and every later edit to a moved task) carries
        // its destination in the queue row. Keep those rows out of the
        // pusher's envelope: the cloud keys the upsert by this envelope's
        // project_canonical_id, not by the row's origin_project field.
        let mut upserts_by_project: HashMap<String, Vec<(&QueuedSync, serde_json::Value)>> =
            HashMap::new();

        for item in queued.iter().filter(|item| {
            item.operation == SyncOperation::Upsert
                && item.entity_type == entity_type
                && !blocked_upserts.contains(&item.id)
        }) {
            match item.payload.as_deref() {
                Some(payload) => match serde_json::from_str::<serde_json::Value>(payload) {
                    Ok(mut value) => {
                        let target_project = item.project_id.as_deref().unwrap_or(project_id);
                        // Rows queued before origin_project existed still need
                        // the target scoped identity when they are retried. The
                        // outer project_canonical_id is not a substitute: task
                        // consumers also rely on the row-level field.
                        if entity_type == EntityType::Task {
                            stamp_task_origin_project(&mut value, target_project);
                        }
                        if entity_type == EntityType::TaskDependency {
                            stamp_task_dependency_origin_project(&mut value, target_project);
                        }
                        upserts_by_project
                            .entry(target_project.to_string())
                            .or_default()
                            .push((item, value));
                    }
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

        let sub_batches: Vec<_> = upserts_by_project
            .into_iter()
            .flat_map(|(target_project, upserts)| {
                self.split_into_sub_batches(upserts)
                    .into_iter()
                    .map(move |sub_batch| (target_project.clone(), sub_batch))
            })
            .collect();

        for (target_project, sub_batch) in sub_batches {
            let (batch_items, values): (Vec<&QueuedSync>, Vec<serde_json::Value>) =
                sub_batch.into_iter().unzip();
            let sent_count = values.len();

            match self
                .push_team_sub_batch(
                    team_id,
                    entity_key,
                    values,
                    token,
                    &target_project,
                    git_remote,
                )
            {
                Ok(response) => {
                    if let Some(body) = response.as_ref() {
                        self.maybe_adopt_team_canonical_id(body);
                    }

                    let raw_response = response.as_ref().map_or("", |body| body.raw_body.as_str());
                    match response.as_ref() {
                        Some(body) => match Self::team_row_results_for(
                            body,
                            entity_key,
                            batch_items.iter().map(|item| item.entity_id.clone()),
                        ) {
                            Ok(Some(rows)) => {
                                self.settle_team_row_results(
                                    &batch_items,
                                    entity_key,
                                    raw_response,
                                    rows,
                                    &mut synced,
                                    &mut errors,
                                );
                                continue;
                            }
                            Err(error) => {
                                let diagnostic = format!(
                                    "team {entity_key} push returned invalid per-row results: {error}; marking {} row(s) failed; server response: {raw_response}",
                                    batch_items.len()
                                );
                                for item in &batch_items {
                                    let _ = self.queue.mark_failed(item.id, &diagnostic);
                                }
                                errors.push(diagnostic);
                                continue;
                            }
                            Ok(None) => {}
                        },
                        None => {}
                    }

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
                        if skipped > batch_items.len() {
                            let diagnostic = format!(
                                "cloud reported {skipped} skipped team {entity_key} row(s) for a {}-row sub-batch; marking sub-batch failed; server response: {raw_response}",
                                batch_items.len()
                            );
                            for item in &batch_items {
                                let _ = self.queue.mark_failed(item.id, &diagnostic);
                            }
                            errors.push(diagnostic);
                            continue;
                        }
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
                                    "cloud skipped {skipped} of {} team {entity_key} row(s); treating skips as LWW acknowledgements",
                                    batch_items.len(),
                                );
                                tracing::warn!(
                                    entity_type = entity_key,
                                    skipped,
                                    batch_size = batch_items.len(),
                                    "{diagnostic}"
                                );
                                for item in &batch_items {
                                    let _ = self.queue.mark_synced(item.id);
                                }
                                synced += batch_items.len();
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

                        let mut failure_details = Vec::new();
                        for item in &batch_items {
                            if let Some(failure) = itemized.get(&item.entity_id) {
                                let reason = match failure {
                                    PushItemizedFailure::Rejection(rejection) => {
                                        rejection.reason.as_str().to_string()
                                    }
                                    PushItemizedFailure::Invalid(invalid) => {
                                        format!(
                                            "{}: {}",
                                            invalid.reason.as_str(),
                                            invalid.detail
                                        )
                                    }
                                };
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
                                failure_details.push(format!("{} ({reason})", item.entity_id));
                            } else {
                                let _ = self.queue.mark_synced(item.id);
                                synced += 1;
                            }
                        }
                        if !failure_details.is_empty() {
                            errors.push(format!(
                                "cloud rejected {} of {} team {entity_key} row(s): {}",
                                failure_details.len(),
                                batch_items.len(),
                                failure_details.join(", ")
                            ));
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

    fn settle_team_row_results(
        &self,
        batch_items: &[&QueuedSync],
        entity_key: &str,
        raw_response: &str,
        rows: HashMap<String, PushRowResult>,
        synced: &mut usize,
        errors: &mut Vec<String>,
    ) {
        let mut rejected = Vec::new();
        for item in batch_items {
            let row = rows
                .get(&item.entity_id)
                .expect("row_results_for validates every queue identity");
            if row.acknowledges() {
                let _ = self.queue.mark_synced(item.id);
                *synced += 1;
                continue;
            }

            let reason = row.reason.as_deref().unwrap_or("unspecified");
            let diagnostic = format!(
                "cloud rejected team {entity_key} {}: reason={reason} ({}); server response: {raw_response}",
                item.entity_id,
                crate::cloud::syncer::push_reason_hint(reason)
            );
            let _ = self
                .queue
                .record_row_outcome(item.id, "rejected", Some(reason));
            if row.rejection_is_retryable() {
                let _ = self.queue.mark_failed(item.id, &diagnostic);
            } else {
                let _ = self
                    .queue
                    .park_failed(item.id, &diagnostic, self.config.max_retries);
            }
            rejected.push(format!("{} ({reason})", item.entity_id));
        }

        if !rejected.is_empty() {
            errors.push(format!(
                "cloud rejected {} of {} team {entity_key} row(s): {}",
                rejected.len(),
                batch_items.len(),
                rejected.join(", ")
            ));
        }
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

    fn team_row_results_for(
        response: &TeamPushResponse,
        entity_key: &str,
        queued_ids: impl Iterator<Item = String>,
    ) -> Result<Option<HashMap<String, PushRowResult>>, String> {
        let queued_ids = queued_ids.collect::<Vec<_>>();
        if let Some(rows) = response.rows.as_ref() {
            let wrapped = serde_json::json!({"rows": rows});
            return row_results_for(&wrapped, "rows", queued_ids.into_iter());
        }

        let Some(synced) = response.synced.as_object() else {
            return Ok(None);
        };
        let Some(entity) = synced.get(entity_key) else {
            return Ok(None);
        };
        row_results_for(
            entity,
            &format!("synced.{entity_key}"),
            queued_ids.into_iter(),
        )
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
            "task_dependencies" => result.pushed_task_dependencies += count,
            _ => {}
        }
    }

    fn maybe_adopt_team_canonical_id(&self, body: &TeamPushResponse) {
        // cas-8ca5 / contract §5: adopt the server's canonical id when our git
        // remote matches the returned git_remote. Stops an unpinned machine from
        // continuing to sync the fragmented per-remote bucket instead of the
        // team's slug.
        if let Some(cas_root) = self.push_cas_root.as_deref() {
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
                        tracing::info!(
                            canonical_id = %adopted,
                            "cas-8ca5: adopted server canonical project id"
                        );
                        tracing::info!(
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

    /// Send team delete requests in queue order, retaining each row's target
    /// project when this is a project move. Normal deletes use the current
    /// pushing project as before.
    fn send_team_deletes(
        &self,
        team_id: &str,
        items: &[&QueuedSync],
        token: &str,
        project_id: &str,
    ) -> (Vec<i64>, Vec<(i64, String)>) {
        let mut successful = Vec::new();
        let mut errors = Vec::new();

        for item in items {
            let target_project = item.project_id.as_deref().unwrap_or(project_id);
            match self.send_team_delete(
                team_id,
                item.entity_type,
                &item.entity_id,
                token,
                target_project,
            ) {
                Ok(()) => successful.push(item.id),
                Err(error) => errors.push((
                    item.id,
                    format!("{} delete {}: {error}", item.entity_type, item.entity_id),
                )),
            }
        }

        (successful, errors)
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

#[cfg(test)]
mod tests {
    #[test]
    fn queued_task_payloads_receive_current_project_identity() {
        let mut value = serde_json::json!({"id": "cas-legacy", "title": "old payload"});
        super::stamp_task_origin_project(&mut value, "acme/accounting");
        assert_eq!(
            value.get("scope").and_then(|value| value.as_str()),
            Some("project")
        );
        assert_eq!(
            value.get("origin_project").and_then(|value| value.as_str()),
            Some("acme/accounting")
        );
    }

    #[test]
    fn explicit_task_origin_project_survives_team_push_stamping() {
        let mut value = serde_json::json!({
            "id": "cas-reassigned",
            "scope": "project",
            "origin_project": "pulse-card",
        });

        super::stamp_task_origin_project(&mut value, "acme/accounting");

        assert_eq!(
            value.get("origin_project").and_then(|value| value.as_str()),
            Some("pulse-card")
        );
    }

    #[test]
    fn missing_task_origin_project_receives_current_project_identity() {
        let mut value = serde_json::json!({"id": "cas-legacy", "scope": "project"});

        super::stamp_task_origin_project(&mut value, "acme/accounting");

        assert_eq!(
            value.get("origin_project").and_then(|value| value.as_str()),
            Some("acme/accounting")
        );
    }

    #[test]
    fn empty_task_origin_project_receives_current_project_identity() {
        let mut value = serde_json::json!({
            "id": "cas-legacy",
            "scope": "project",
            "origin_project": "",
        });

        super::stamp_task_origin_project(&mut value, "acme/accounting");

        assert_eq!(
            value.get("origin_project").and_then(|value| value.as_str()),
            Some("acme/accounting")
        );
    }

    #[test]
    fn global_task_origin_project_remains_unstamped() {
        let mut value = serde_json::json!({
            "id": "cas-global",
            "scope": "global",
            "origin_project": null,
        });

        super::stamp_task_origin_project(&mut value, "acme/accounting");

        assert_eq!(
            value.get("scope").and_then(|value| value.as_str()),
            Some("global")
        );
        assert!(
            value
                .get("origin_project")
                .is_some_and(serde_json::Value::is_null)
        );
    }

    #[test]
    fn canonical_identity_team_push_stamps_remote_alias_as_canonical() {
        let mut value = serde_json::json!({
            "id": "cas-alias",
            "scope": "project",
        });

        super::stamp_task_origin_project(
            &mut value,
            "git@GitHub.com:Richards-LLC/gabber-studio.git",
        );

        assert_eq!(
            value.get("origin_project").and_then(|value| value.as_str()),
            Some("github.com/richards-llc/gabber-studio")
        );
    }

    #[test]
    fn canonical_identity_team_push_stamps_dependency_alias_as_canonical() {
        let mut value = serde_json::json!({
            "id": "edge-alias",
            "origin_project": "git@GitHub.com:Richards-LLC/gabber-studio.git",
        });

        super::stamp_task_dependency_origin_project(&mut value, "gabber-studio");

        assert_eq!(
            value.get("origin_project").and_then(|value| value.as_str()),
            Some("gabber-studio")
        );
    }
}
