use std::time::Instant;

use crate::cloud::syncer::{
    CloudSyncer, ConflictAction, ConflictResolution, PullResponse, SyncResult, TeamPullResponse,
    UpsertResult,
};
use crate::cloud::get_project_canonical_id;
use crate::error::CasError;
use crate::store::{
    CommitLinkStore, EventStore, FileChangeStore, PromptStore, RuleStore, SkillStore, SpecStore,
    Store, TaskStore,
};
use crate::types::{
    CommitLink, Entry, Event, FileChange, Prompt, Rule, Session, Skill, Spec, Task, TaskStatus,
};

/// Path of the cloud sync pull endpoint.
///
/// Single source of truth: this is the only place in shipped source where the
/// pull path literal is written. Every production caller must build its URL
/// through [`build_scoped_pull_url`], which is what keeps the pull scoped to
/// the current project (cas-2eb3 / cas-ed15).
pub(crate) const PULL_PATH: &str = "/api/sync/pull";

/// Build a project-scoped `/api/sync/pull` URL, **failing closed** when the
/// project scope cannot be resolved.
///
/// `extra_params` are appended verbatim (already `key=value` encoded); the
/// `project_id=` parameter is always appended by this function. If
/// `get_project_canonical_id()` returns `None` — i.e. the caller is not inside
/// a CAS project directory, which is the realistic daemon / cwd-independent
/// case — no URL is produced at all and the pull aborts. Omitting the scope
/// would ask the server for *every* project's rows, which is exactly the
/// cross-project contamination this path exists to prevent.
///
/// Returns `(url, resolved_project_id)`; callers that also filter the response
/// client-side reuse the resolved id rather than resolving it a second time.
pub(crate) fn build_scoped_pull_url(
    endpoint: &str,
    extra_params: &[String],
) -> Result<(String, String), CasError> {
    build_scoped_pull_url_with(endpoint, extra_params, get_project_canonical_id)
}

/// [`build_scoped_pull_url`] with an injectable project-scope resolver.
///
/// The resolver is a parameter purely so tests can exercise the unresolvable
/// (`None`) branch without depending on the process-wide cache inside
/// `get_project_canonical_id` or on the test's working directory.
pub(crate) fn build_scoped_pull_url_with(
    endpoint: &str,
    extra_params: &[String],
    resolve_project_id: impl FnOnce() -> Option<String>,
) -> Result<(String, String), CasError> {
    let project_id = resolve_project_id().ok_or_else(|| {
        CasError::Other("Cannot pull: not inside a CAS project directory".to_string())
    })?;
    let mut params: Vec<String> = extra_params.to_vec();
    params.push(format!("project_id={}", project_id.replace('/', "%2F")));
    let url = format!("{endpoint}{PULL_PATH}?{}", params.join("&"));
    Ok((url, project_id))
}

/// Check whether a raw JSON entity belongs to the current project.
///
/// Returns `true` if the entity should be accepted, `false` if it should be skipped.
///
/// An entity is accepted only when its `project_canonical_id` or `project_id` field
/// is a string that exactly matches `current_project_id`. All other cases — missing
/// field, null field, wrong project, or unexpected type — are rejected. The legacy
/// "no field = accept" and "null = accept" paths have been removed now that the cloud
/// always echoes `project_id` in every pull-response row (cas-6479).
fn entity_matches_project(
    raw: &serde_json::Value,
    current_project_id: &str,
    entity_kind: &str,
) -> bool {
    // Check both field names the server might use
    let project_field = raw
        .get("project_canonical_id")
        .or_else(|| raw.get("project_id"));

    let entity_id = raw
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("<unknown>");

    match project_field {
        None => {
            // Missing field — cloud now always includes project_id; treat as unscoped/foreign.
            eprintln!(
                "[CAS sync] WARNING: skipping {entity_kind} '{entity_id}' — no project_id field \
                 (expected '{current_project_id}')"
            );
            false
        }
        Some(serde_json::Value::Null) => {
            // Explicitly null — no longer accepted; cloud must scope all entities.
            eprintln!(
                "[CAS sync] WARNING: skipping {entity_kind} '{entity_id}' — null project_id \
                 (expected '{current_project_id}')"
            );
            false
        }
        Some(serde_json::Value::String(s)) => {
            if s == current_project_id {
                true
            } else {
                eprintln!(
                    "[CAS sync] WARNING: skipping {entity_kind} '{entity_id}' from foreign \
                     project '{s}' (expected '{current_project_id}')"
                );
                false
            }
        }
        Some(_) => {
            // Unexpected type — reject; unexpected field shapes shouldn't be silently accepted.
            eprintln!(
                "[CAS sync] WARNING: skipping {entity_kind} '{entity_id}' — unexpected \
                 project_id type (expected string '{current_project_id}')"
            );
            false
        }
    }
}

/// cas-fc52: detect a teammate's web-initiated close (cloud contract §4).
///
/// The cloud server records a web close as a soft tombstone merged into the
/// task's `data`: `closed_via == "web"` (plus `status="closed"`,
/// `close_reason`, `closed_at`) with a bumped `updated_at`. The marker arrives
/// at the top level of the pulled task JSON (the same level the strongly-typed
/// `Task` deserializes from). `closed_via == "web"` is the discriminator: the
/// client's OWN pushed closes never carry it, so this never reconciles a
/// self-close as web-initiated (no loop, no double-close).
fn is_web_close_tombstone(raw: &serde_json::Value) -> bool {
    raw.get("closed_via").and_then(|v| v.as_str()) == Some("web")
}

/// cas-fc52: apply a teammate's web-initiated close as the authoritative local
/// close (cloud contract §4). Unlike the timestamp-gated [`upsert_task`], this
/// forces the closed status even when the local row looks newer — a web close
/// is an explicit instruction, not a data-merge race.
///
/// Side effects mirror a real close at the task-store level: status becomes
/// `Closed` (carried on the tombstone `Task`, along with `close_reason` /
/// `closed_at`), and the task's `assignee` is cleared so no stale ownership
/// lingers on the closed task. The separate agent-lease row (if any) is left to
/// the existing lease GC — a closed task is not claimable, so a lingering lease
/// is inert.
///
/// Idempotent: a task already locally `Closed` is a no-op.
fn reconcile_web_close(store: &dyn TaskStore, task: Task) -> Result<UpsertResult, CasError> {
    match store.get(&task.id) {
        Ok(local) => {
            if local.status == TaskStatus::Closed {
                // Already closed locally — nothing to do.
                return Ok(UpsertResult::Skipped);
            }
            if local.status == TaskStatus::PendingSupervisorReview {
                // An explicit teammate web close overrides the pending-review
                // gate. Surface it so the bypassed supervisor review is auditable.
                tracing::warn!(
                    task_id = %task.id,
                    "cas-fc52: web close applied to a PendingSupervisorReview task — \
                     supervisor review gate bypassed by explicit teammate close"
                );
            }
            // Merge ONLY the close signal onto the local row. A full overwrite
            // with the remote tombstone would clobber locally-authored,
            // not-yet-pushed content (notes / description / acceptance_criteria):
            // the web close is authoritative about the CLOSE, not the body.
            let mut merged = local;
            merged.status = TaskStatus::Closed;
            merged.close_reason = task.close_reason;
            merged.closed_at = task.closed_at.or(merged.closed_at);
            merged.updated_at = task.updated_at;
            merged.assignee = None;
            store.update(&merged)?;
            tracing::info!(
                task_id = %task.id,
                "cas-fc52: reconciled web-initiated close from cloud pull"
            );
            Ok(UpsertResult::Updated)
        }
        Err(cas_store::StoreError::TaskNotFound(_)) => {
            // Never had this task locally — record the closed tombstone as-is
            // (no local content to preserve).
            let mut tombstone = task;
            tombstone.assignee = None;
            store.add(&tombstone)?;
            Ok(UpsertResult::Created)
        }
        Err(e) => Err(e.into()),
    }
}

impl CloudSyncer {
    #[allow(clippy::too_many_arguments)]
    pub fn pull(
        &self,
        store: &dyn Store,
        task_store: &dyn TaskStore,
        rule_store: &dyn RuleStore,
        skill_store: &dyn SkillStore,
        spec_store: &dyn SpecStore,
        event_store: &dyn EventStore,
        prompt_store: &dyn PromptStore,
        file_change_store: &dyn FileChangeStore,
        commit_link_store: &dyn CommitLinkStore,
    ) -> Result<SyncResult, CasError> {
        let mut result = SyncResult::default();
        let start = Instant::now();

        if !self.is_available() {
            return Ok(result);
        }

        let token = self
            .cloud_config
            .token
            .as_ref()
            .ok_or_else(|| CasError::Other("Not logged in".to_string()))?;

        // Get last pull timestamp
        let since = self.queue.get_metadata("last_pull_at")?;

        let mut params = Vec::new();
        if let Some(since) = &since {
            params.push(format!("since={since}"));
        }
        let (pull_url, project_id) = build_scoped_pull_url(&self.cloud_config.endpoint, &params)?;

        let response = ureq::get(&pull_url)
            .timeout(self.config.timeout)
            .set("Authorization", &format!("Bearer {token}"))
            .call();

        let body: PullResponse = match response {
            Ok(resp) => resp
                .into_json()
                .map_err(|e| CasError::Other(format!("Failed to parse response: {e}")))?,
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                return Err(CasError::Other(format!(
                    "Pull failed with status {code}: {body}"
                )));
            }
            Err(ureq::Error::Transport(e)) => {
                return Err(CasError::Other(format!("Network error: {e}")));
            }
        };

        // Use the already-resolved project ID for client-side entity validation
        let current_project_id = &project_id;

        // Process entries
        for raw_entry in body.entries.unwrap_or_default() {
            if !entity_matches_project(&raw_entry, &current_project_id, "entry") {
                continue;
            }
            let remote_entry: Entry = match serde_json::from_value(raw_entry) {
                Ok(e) => e,
                Err(e) => {
                    result.errors.push(format!("Entry deserialize error: {e}"));
                    continue;
                }
            };
            match self.upsert_entry(store, remote_entry) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_entries += 1;
                }
                Ok(UpsertResult::Skipped) => {
                    result.conflicts_resolved += 1;
                }
                Err(e) => {
                    result.errors.push(format!("Entry error: {e}"));
                }
            }
        }

        // Process tasks
        for raw_task in body.tasks.unwrap_or_default() {
            if !entity_matches_project(&raw_task, &current_project_id, "task") {
                continue;
            }
            // cas-fc52: a teammate's web-initiated close arrives as a soft
            // tombstone (closed_via == "web"). The CLI owns the real local
            // close — apply it authoritatively rather than via the
            // timestamp-gated upsert.
            let web_close = is_web_close_tombstone(&raw_task);
            let remote_task: Task = match serde_json::from_value(raw_task) {
                Ok(t) => t,
                Err(e) => {
                    result.errors.push(format!("Task deserialize error: {e}"));
                    continue;
                }
            };
            let task_outcome = if web_close {
                reconcile_web_close(task_store, remote_task)
            } else {
                self.upsert_task(task_store, remote_task)
            };
            match task_outcome {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_tasks += 1;
                }
                Ok(UpsertResult::Skipped) => {
                    result.conflicts_resolved += 1;
                }
                Err(e) => {
                    result.errors.push(format!("Task error: {e}"));
                }
            }
        }

        // Process rules
        for raw_rule in body.rules.unwrap_or_default() {
            if !entity_matches_project(&raw_rule, &current_project_id, "rule") {
                continue;
            }
            let remote_rule: Rule = match serde_json::from_value(raw_rule) {
                Ok(r) => r,
                Err(e) => {
                    result.errors.push(format!("Rule deserialize error: {e}"));
                    continue;
                }
            };
            match self.upsert_rule(rule_store, remote_rule) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_rules += 1;
                }
                Ok(UpsertResult::Skipped) => {
                    result.conflicts_resolved += 1;
                }
                Err(e) => {
                    result.errors.push(format!("Rule error: {e}"));
                }
            }
        }

        // Process skills
        for raw_skill in body.skills.unwrap_or_default() {
            if !entity_matches_project(&raw_skill, &current_project_id, "skill") {
                continue;
            }
            let remote_skill: Skill = match serde_json::from_value(raw_skill) {
                Ok(s) => s,
                Err(e) => {
                    result.errors.push(format!("Skill deserialize error: {e}"));
                    continue;
                }
            };
            match self.upsert_skill(skill_store, remote_skill) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_skills += 1;
                }
                Ok(UpsertResult::Skipped) => {
                    result.conflicts_resolved += 1;
                }
                Err(e) => {
                    result.errors.push(format!("Skill error: {e}"));
                }
            }
        }

        // cas-bba4: process the 5 entity kinds the inline `cas cloud pull`
        // path used to import unscoped (cas-ed15 dropped them when collapsing
        // through CloudSyncer::pull). Each block mirrors the entries/tasks
        // shape: filter via `entity_matches_project` so foreign rows are
        // skipped, then delegate to a per-kind upsert helper. `specs` arrives
        // empty until cloud ships the matching pull-endpoint extension
        // (docs/requests/FEATURE-cloud-sync-pull-return-specs.md).

        // Process specs
        for raw_spec in body.specs.unwrap_or_default() {
            if !entity_matches_project(&raw_spec, &current_project_id, "spec") {
                continue;
            }
            let remote_spec: Spec = match serde_json::from_value(raw_spec) {
                Ok(s) => s,
                Err(e) => {
                    result.errors.push(format!("Spec deserialize error: {e}"));
                    continue;
                }
            };
            match self.upsert_spec(spec_store, remote_spec) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_specs += 1;
                }
                Ok(UpsertResult::Skipped) => {
                    result.conflicts_resolved += 1;
                }
                Err(e) => {
                    result.errors.push(format!("Spec error: {e}"));
                }
            }
        }

        // Process events. EventStore is append-only (`record()` is straight
        // INSERT, no dedup); matches the pre-cas-ed15 inline path behavior.
        // The `since=` watermark on the request limits volume on incremental
        // pulls. `--full` re-imports duplicates — same as the prior path,
        // not a regression.
        for raw_event in body.events.unwrap_or_default() {
            if !entity_matches_project(&raw_event, &current_project_id, "event") {
                continue;
            }
            let remote_event: Event = match serde_json::from_value(raw_event) {
                Ok(e) => e,
                Err(e) => {
                    result.errors.push(format!("Event deserialize error: {e}"));
                    continue;
                }
            };
            match event_store.record(&remote_event) {
                Ok(_) => result.pulled_events += 1,
                Err(e) => result.errors.push(format!("Event error: {e}")),
            }
        }

        // Process prompts. PromptStore exposes `add()` keyed by id; we
        // dedup-skip on existing rows (the previous inline path used
        // `let _ = prompt_store.add(&prompt)` and silently double-counted
        // on duplicate-key error — we instead surface real errors).
        for raw_prompt in body.prompts.unwrap_or_default() {
            if !entity_matches_project(&raw_prompt, &current_project_id, "prompt") {
                continue;
            }
            let remote_prompt: Prompt = match serde_json::from_value(raw_prompt) {
                Ok(p) => p,
                Err(e) => {
                    result.errors.push(format!("Prompt deserialize error: {e}"));
                    continue;
                }
            };
            match prompt_store.get(&remote_prompt.id) {
                Ok(Some(_)) => {
                    result.conflicts_resolved += 1;
                }
                Ok(None) => match prompt_store.add(&remote_prompt) {
                    Ok(_) => result.pulled_prompts += 1,
                    Err(e) => result.errors.push(format!("Prompt error: {e}")),
                },
                Err(e) => result.errors.push(format!("Prompt lookup error: {e}")),
            }
        }

        // Process file changes (append-only, same shape as prompts).
        for raw_fc in body.file_changes.unwrap_or_default() {
            if !entity_matches_project(&raw_fc, &current_project_id, "file_change") {
                continue;
            }
            let remote_fc: FileChange = match serde_json::from_value(raw_fc) {
                Ok(f) => f,
                Err(e) => {
                    result
                        .errors
                        .push(format!("FileChange deserialize error: {e}"));
                    continue;
                }
            };
            match file_change_store.get(&remote_fc.id) {
                Ok(Some(_)) => {
                    result.conflicts_resolved += 1;
                }
                Ok(None) => match file_change_store.add(&remote_fc) {
                    Ok(_) => result.pulled_file_changes += 1,
                    Err(e) => result.errors.push(format!("FileChange error: {e}")),
                },
                Err(e) => result
                    .errors
                    .push(format!("FileChange lookup error: {e}")),
            }
        }

        // Process commit links (keyed by `commit_hash`).
        for raw_cl in body.commit_links.unwrap_or_default() {
            if !entity_matches_project(&raw_cl, &current_project_id, "commit_link") {
                continue;
            }
            let remote_cl: CommitLink = match serde_json::from_value(raw_cl) {
                Ok(c) => c,
                Err(e) => {
                    result
                        .errors
                        .push(format!("CommitLink deserialize error: {e}"));
                    continue;
                }
            };
            match commit_link_store.get(&remote_cl.commit_hash) {
                Ok(Some(_)) => {
                    result.conflicts_resolved += 1;
                }
                Ok(None) => match commit_link_store.add(&remote_cl) {
                    Ok(_) => result.pulled_commit_links += 1,
                    Err(e) => result.errors.push(format!("CommitLink error: {e}")),
                },
                Err(e) => result
                    .errors
                    .push(format!("CommitLink lookup error: {e}")),
            }
        }

        // Update last pull timestamp
        if let Some(pulled_at) = body.pulled_at {
            let _ = self.queue.set_metadata("last_pull_at", &pulled_at);
        }

        result.duration_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }

    fn upsert_entry(&self, store: &dyn Store, entry: Entry) -> Result<UpsertResult, CasError> {
        match store.get(&entry.id) {
            Ok(local) => {
                // Compare timestamps for conflict resolution (last-write-wins)
                let local_time = local.last_accessed.unwrap_or(local.created);
                let remote_time = entry.last_accessed.unwrap_or(entry.created);

                if remote_time > local_time {
                    store.update(&entry)?;
                    Ok(UpsertResult::Updated)
                } else {
                    Ok(UpsertResult::Skipped)
                }
            }
            Err(cas_store::StoreError::EntryNotFound(_)) => {
                store.add(&entry)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    fn upsert_task(&self, store: &dyn TaskStore, task: Task) -> Result<UpsertResult, CasError> {
        match store.get(&task.id) {
            Ok(local) => {
                if task.updated_at > local.updated_at {
                    store.update(&task)?;
                    Ok(UpsertResult::Updated)
                } else {
                    Ok(UpsertResult::Skipped)
                }
            }
            Err(cas_store::StoreError::TaskNotFound(_)) => {
                store.add(&task)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    fn upsert_rule(&self, store: &dyn RuleStore, rule: Rule) -> Result<UpsertResult, CasError> {
        match store.get(&rule.id) {
            Ok(local) => {
                // Compare by last_accessed or created
                let local_time = local.last_accessed.unwrap_or(local.created);
                let remote_time = rule.last_accessed.unwrap_or(rule.created);

                if remote_time > local_time {
                    store.update(&rule)?;
                    Ok(UpsertResult::Updated)
                } else {
                    Ok(UpsertResult::Skipped)
                }
            }
            Err(cas_store::StoreError::RuleNotFound(_)) => {
                store.add(&rule)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    fn upsert_skill(&self, store: &dyn SkillStore, skill: Skill) -> Result<UpsertResult, CasError> {
        match store.get(&skill.id) {
            Ok(local) => {
                if skill.updated_at > local.updated_at {
                    store.update(&skill)?;
                    Ok(UpsertResult::Updated)
                } else {
                    Ok(UpsertResult::Skipped)
                }
            }
            Err(cas_store::StoreError::SkillNotFound(_)) => {
                store.add(&skill)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    fn upsert_spec(&self, store: &dyn SpecStore, spec: Spec) -> Result<UpsertResult, CasError> {
        // SpecStore::get returns `Result<Spec>` (not Option), with
        // `StoreError::NotFound` when absent — mirrors the task/skill shape.
        match store.get(&spec.id) {
            Ok(local) => {
                if spec.updated_at > local.updated_at {
                    store.update(&spec)?;
                    Ok(UpsertResult::Updated)
                } else {
                    Ok(UpsertResult::Skipped)
                }
            }
            Err(cas_store::StoreError::NotFound(_)) => {
                store.add(&spec)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Upsert entry with configurable conflict resolution for team sync
    fn upsert_entry_with_strategy(
        &self,
        store: &dyn Store,
        entry: Entry,
        strategy: ConflictResolution,
    ) -> Result<UpsertResult, CasError> {
        match store.get(&entry.id) {
            Ok(local) => {
                let local_time = local.last_accessed.unwrap_or(local.created);
                let remote_time = entry.last_accessed.unwrap_or(entry.created);

                let action =
                    self.resolve_conflict("entry", &entry.id, local_time, remote_time, strategy);

                match action {
                    ConflictAction::UseRemote => {
                        store.update(&entry)?;
                        Ok(UpsertResult::Updated)
                    }
                    ConflictAction::UseLocal | ConflictAction::Skip => Ok(UpsertResult::Skipped),
                }
            }
            Err(cas_store::StoreError::EntryNotFound(_)) => {
                store.add(&entry)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Upsert task with configurable conflict resolution for team sync
    fn upsert_task_with_strategy(
        &self,
        store: &dyn TaskStore,
        task: Task,
        strategy: ConflictResolution,
    ) -> Result<UpsertResult, CasError> {
        match store.get(&task.id) {
            Ok(local) => {
                let action = self.resolve_conflict(
                    "task",
                    &task.id,
                    local.updated_at,
                    task.updated_at,
                    strategy,
                );

                match action {
                    ConflictAction::UseRemote => {
                        store.update(&task)?;
                        Ok(UpsertResult::Updated)
                    }
                    ConflictAction::UseLocal | ConflictAction::Skip => Ok(UpsertResult::Skipped),
                }
            }
            Err(cas_store::StoreError::TaskNotFound(_)) => {
                store.add(&task)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Upsert rule with configurable conflict resolution for team sync
    fn upsert_rule_with_strategy(
        &self,
        store: &dyn RuleStore,
        rule: Rule,
        strategy: ConflictResolution,
    ) -> Result<UpsertResult, CasError> {
        match store.get(&rule.id) {
            Ok(local) => {
                let local_time = local.last_accessed.unwrap_or(local.created);
                let remote_time = rule.last_accessed.unwrap_or(rule.created);

                let action =
                    self.resolve_conflict("rule", &rule.id, local_time, remote_time, strategy);

                match action {
                    ConflictAction::UseRemote => {
                        store.update(&rule)?;
                        Ok(UpsertResult::Updated)
                    }
                    ConflictAction::UseLocal | ConflictAction::Skip => Ok(UpsertResult::Skipped),
                }
            }
            Err(cas_store::StoreError::RuleNotFound(_)) => {
                store.add(&rule)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Upsert skill with configurable conflict resolution for team sync
    fn upsert_skill_with_strategy(
        &self,
        store: &dyn SkillStore,
        skill: Skill,
        strategy: ConflictResolution,
    ) -> Result<UpsertResult, CasError> {
        match store.get(&skill.id) {
            Ok(local) => {
                let action = self.resolve_conflict(
                    "skill",
                    &skill.id,
                    local.updated_at,
                    skill.updated_at,
                    strategy,
                );

                match action {
                    ConflictAction::UseRemote => {
                        store.update(&skill)?;
                        Ok(UpsertResult::Updated)
                    }
                    ConflictAction::UseLocal | ConflictAction::Skip => Ok(UpsertResult::Skipped),
                }
            }
            Err(cas_store::StoreError::SkillNotFound(_)) => {
                store.add(&skill)?;
                Ok(UpsertResult::Created)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Full sync: push then pull
    #[allow(clippy::too_many_arguments)]
    pub fn sync(
        &self,
        store: &dyn Store,
        task_store: &dyn TaskStore,
        rule_store: &dyn RuleStore,
        skill_store: &dyn SkillStore,
        spec_store: &dyn SpecStore,
        event_store: &dyn EventStore,
        prompt_store: &dyn PromptStore,
        file_change_store: &dyn FileChangeStore,
        commit_link_store: &dyn CommitLinkStore,
    ) -> Result<SyncResult, CasError> {
        self.sync_with_sessions(
            store,
            task_store,
            rule_store,
            skill_store,
            spec_store,
            event_store,
            prompt_store,
            file_change_store,
            commit_link_store,
            &[],
        )
    }

    /// Full sync with sessions: push (including sessions) then pull
    #[allow(clippy::too_many_arguments)]
    pub fn sync_with_sessions(
        &self,
        store: &dyn Store,
        task_store: &dyn TaskStore,
        rule_store: &dyn RuleStore,
        skill_store: &dyn SkillStore,
        spec_store: &dyn SpecStore,
        event_store: &dyn EventStore,
        prompt_store: &dyn PromptStore,
        file_change_store: &dyn FileChangeStore,
        commit_link_store: &dyn CommitLinkStore,
        sessions: &[Session],
    ) -> Result<SyncResult, CasError> {
        let start = Instant::now();

        // Push personal changes first (with sessions).
        let push_result = self.push_with_sessions(sessions)?;

        // Team-scoped writes are queued separately from personal writes. The
        // automatic daemon enters through this method, so omitting this drain
        // leaves every team row untouched (retry_count=0, last_error=NULL)
        // until somebody happens to run the manual CLI sync path.
        //
        // Keep team failure isolated from pull, matching `cas cloud sync`:
        // push_team records per-row HTTP failures, while setup/config errors
        // are surfaced in the aggregate result without suppressing pull.
        let team_push_result = match self.cloud_config.active_team_id() {
            Some(team_id) => self.push_team(&team_id).unwrap_or_else(|error| SyncResult {
                errors: vec![format!("Team push failed: {error}")],
                ..SyncResult::default()
            }),
            None => SyncResult::default(),
        };

        // Then pull
        let pull_result = self.pull(
            store,
            task_store,
            rule_store,
            skill_store,
            spec_store,
            event_store,
            prompt_store,
            file_change_store,
            commit_link_store,
        )?;

        // Combine results
        Ok(SyncResult {
            // Knowledge pages are synced by the dedicated
            // `push_knowledge_pages` / `pull_knowledge_pages` pair (they need
            // a KnowledgeStore this entry point does not take), so this
            // combined result reports zero for them rather than guessing.
            pushed_knowledge_pages: 0,
            pulled_knowledge_pages: 0,
            pushed_entries: push_result.pushed_entries + team_push_result.pushed_entries,
            pushed_tasks: push_result.pushed_tasks + team_push_result.pushed_tasks,
            pushed_rules: push_result.pushed_rules + team_push_result.pushed_rules,
            pushed_skills: push_result.pushed_skills + team_push_result.pushed_skills,
            pushed_sessions: push_result.pushed_sessions + team_push_result.pushed_sessions,
            pushed_verifications: push_result.pushed_verifications
                + team_push_result.pushed_verifications,
            pushed_events: push_result.pushed_events + team_push_result.pushed_events,
            pushed_prompts: push_result.pushed_prompts + team_push_result.pushed_prompts,
            pushed_file_changes: push_result.pushed_file_changes
                + team_push_result.pushed_file_changes,
            pushed_commit_links: push_result.pushed_commit_links
                + team_push_result.pushed_commit_links,
            pushed_agents: push_result.pushed_agents + team_push_result.pushed_agents,
            pushed_worktrees: push_result.pushed_worktrees + team_push_result.pushed_worktrees,
            pulled_entries: pull_result.pulled_entries,
            pulled_tasks: pull_result.pulled_tasks,
            pulled_rules: pull_result.pulled_rules,
            pulled_skills: pull_result.pulled_skills,
            pulled_specs: pull_result.pulled_specs,
            pulled_events: pull_result.pulled_events,
            pulled_prompts: pull_result.pulled_prompts,
            pulled_file_changes: pull_result.pulled_file_changes,
            pulled_commit_links: pull_result.pulled_commit_links,
            conflicts_resolved: pull_result.conflicts_resolved,
            errors: [
                push_result.errors,
                team_push_result.errors,
                pull_result.errors,
            ]
            .concat(),
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Pull team data from cloud and merge into local store.
    ///
    /// `project_id` is the canonical project ID for the current scope
    /// (typically `cas::cloud::get_project_canonical_id()` at the caller
    /// site). Taking it as a parameter (rather than resolving inside the
    /// function) keeps the watermark scope explicit AND avoids the
    /// process-wide cache in `get_project_canonical_id` which would make
    /// it impossible to exercise the cross-project watermark behavior
    /// in a single test process. The value is used for:
    /// - The `last_team_pull_at_{team_id}_{project_id}` metadata key
    ///   (cas-53d5 — per-(team, project) watermark scoping, fixes the
    ///   "second project sees stale `since=` from the first" regression
    ///   that surfaced as hypothesis #2 of the cas-ffc4 bug doc).
    /// - The `project_id=` URL query param.
    /// - The client-side `entity_matches_project` filter.
    pub fn pull_team(
        &self,
        team_id: &str,
        project_id: &str,
        store: &dyn Store,
        task_store: &dyn TaskStore,
        rule_store: &dyn RuleStore,
        skill_store: &dyn SkillStore,
    ) -> Result<SyncResult, CasError> {
        let mut result = SyncResult::default();
        let start = Instant::now();

        if !self.is_available() {
            return Ok(result);
        }

        let token = self
            .cloud_config
            .token
            .as_ref()
            .ok_or_else(|| CasError::Other("Not logged in".to_string()))?;

        // Get last pull timestamp for this (team_id, project_id) scope.
        // cas-53d5: re-keyed from the old `last_team_pull_at_{team_id}`.
        // Absence of the new-format key is treated as "first sync into
        // this scope" — we send no `since=`, triggering a full backfill.
        // This is the bug fix: previously the global-per-team watermark
        // leaked across projects, causing the second project to skip its
        // historical backfill.
        let since_key = format!("last_team_pull_at_{team_id}_{project_id}");
        let since = self.queue.get_metadata(&since_key)?;

        let mut pull_url = format!(
            "{}/api/teams/{}/sync/pull",
            self.cloud_config.endpoint, team_id
        );
        let mut params = Vec::new();
        if let Some(since) = &since {
            params.push(format!("since={since}"));
        }
        params.push(format!("project_id={}", project_id.replace('/', "%2F")));
        if !params.is_empty() {
            pull_url = format!("{pull_url}?{}", params.join("&"));
        }

        let response = ureq::get(&pull_url)
            .timeout(self.config.timeout)
            .set("Authorization", &format!("Bearer {token}"))
            .call();

        let body: TeamPullResponse = match response {
            Ok(resp) => resp
                .into_json()
                .map_err(|e| CasError::Other(format!("Failed to parse team pull response: {e}")))?,
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                return Err(CasError::Other(format!(
                    "Team pull failed with status {code}: {body}"
                )));
            }
            Err(ureq::Error::Transport(e)) => {
                return Err(CasError::Other(format!("Network error: {e}")));
            }
        };

        // Use configured conflict resolution strategy for team sync
        let strategy = self.config.team_conflict_resolution;
        #[cfg(debug_assertions)]
        eprintln!("[CAS sync] Starting team pull: team={team_id} strategy={strategy:?}");

        // Use the caller-supplied project ID for client-side validation.
        // (cas-53d5: previously resolved internally via
        // `get_project_canonical_id`; now passed in as a function
        // parameter so the watermark key, URL param, and entity-filter
        // all agree on a single explicit scope.)
        let current_project_id = project_id;

        // Process entries
        for raw_entry in body.entries.unwrap_or_default() {
            if !entity_matches_project(&raw_entry, &current_project_id, "entry") {
                continue;
            }
            let remote_entry: Entry = match serde_json::from_value(raw_entry) {
                Ok(e) => e,
                Err(e) => {
                    result.errors.push(format!("Entry deserialize error: {e}"));
                    continue;
                }
            };
            match self.upsert_entry_with_strategy(store, remote_entry, strategy) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_entries += 1;
                }
                Ok(UpsertResult::Skipped) => {
                    result.conflicts_resolved += 1;
                }
                Err(e) => {
                    result.errors.push(format!("Entry error: {e}"));
                }
            }
        }

        // Process tasks
        for raw_task in body.tasks.unwrap_or_default() {
            if !entity_matches_project(&raw_task, &current_project_id, "task") {
                continue;
            }
            let remote_task: Task = match serde_json::from_value(raw_task) {
                Ok(t) => t,
                Err(e) => {
                    result.errors.push(format!("Task deserialize error: {e}"));
                    continue;
                }
            };
            match self.upsert_task_with_strategy(task_store, remote_task, strategy) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_tasks += 1;
                }
                Ok(UpsertResult::Skipped) => {
                    result.conflicts_resolved += 1;
                }
                Err(e) => {
                    result.errors.push(format!("Task error: {e}"));
                }
            }
        }

        // Process rules
        for raw_rule in body.rules.unwrap_or_default() {
            if !entity_matches_project(&raw_rule, &current_project_id, "rule") {
                continue;
            }
            let remote_rule: Rule = match serde_json::from_value(raw_rule) {
                Ok(r) => r,
                Err(e) => {
                    result.errors.push(format!("Rule deserialize error: {e}"));
                    continue;
                }
            };
            match self.upsert_rule_with_strategy(rule_store, remote_rule, strategy) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_rules += 1;
                }
                Ok(UpsertResult::Skipped) => {
                    result.conflicts_resolved += 1;
                }
                Err(e) => {
                    result.errors.push(format!("Rule error: {e}"));
                }
            }
        }

        // Process skills
        for raw_skill in body.skills.unwrap_or_default() {
            if !entity_matches_project(&raw_skill, &current_project_id, "skill") {
                continue;
            }
            let remote_skill: Skill = match serde_json::from_value(raw_skill) {
                Ok(s) => s,
                Err(e) => {
                    result.errors.push(format!("Skill deserialize error: {e}"));
                    continue;
                }
            };
            match self.upsert_skill_with_strategy(skill_store, remote_skill, strategy) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_skills += 1;
                }
                Ok(UpsertResult::Skipped) => {
                    result.conflicts_resolved += 1;
                }
                Err(e) => {
                    result.errors.push(format!("Skill error: {e}"));
                }
            }
        }

        // Update team pull timestamp under the new per-(team, project)
        // key. On successful write, best-effort retire the legacy
        // `last_team_pull_at_{team_id}` global-per-team key — once the
        // new-format key exists for any project under this team, the
        // legacy key is dead metadata that would otherwise sit forever.
        // Best-effort: a delete failure here cannot regress the pull
        // result, so we swallow the error.
        if let Some(pulled_at) = body.pulled_at {
            let _ = self.queue.set_metadata(&since_key, &pulled_at);
            let legacy_key = format!("last_team_pull_at_{team_id}");
            let _ = self.queue.delete_metadata(&legacy_key);
        }

        result.duration_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::{PULL_PATH, build_scoped_pull_url_with, entity_matches_project};
    use serde_json::json;

    // cas-0be9: the pull URL builder must FAIL CLOSED when the project scope
    // cannot be resolved. An unscoped `/api/sync/pull` asks the server for
    // every project's rows — the cas-2eb3 contamination vector.
    #[test]
    fn pull_url_aborts_when_project_scope_is_unresolvable() {
        let err = build_scoped_pull_url_with("https://cloud.example", &[], || None)
            .expect_err("an unresolvable project scope must abort the pull, not drop the scope");
        let message = err.to_string();
        assert!(
            message.contains("not inside a CAS project directory"),
            "unexpected error message: {message}"
        );
    }

    #[test]
    fn pull_url_is_scoped_and_carries_extra_params() {
        let (url, project_id) = build_scoped_pull_url_with(
            "https://cloud.example",
            &["since=2026-01-01T00:00:00Z".to_string()],
            || Some("github.com/owner/repo".to_string()),
        )
        .expect("a resolvable project scope builds a URL");
        assert_eq!(project_id, "github.com/owner/repo");
        assert_eq!(
            url,
            format!(
                "https://cloud.example{PULL_PATH}?since=2026-01-01T00:00:00Z\
                 &project_id=github.com%2Fowner%2Frepo"
            )
        );
    }

    #[test]
    fn test_entity_matches_project_no_field() {
        // No project field — rejected now that cloud always echoes project_id (cas-6479)
        let entity = json!({ "id": "e-001", "content": "hello" });
        assert!(!entity_matches_project(&entity, "github.com/owner/repo", "entry"));
    }

    #[test]
    fn test_entity_matches_project_null_field() {
        // Null project_canonical_id — rejected; cloud must scope all entities (cas-6479)
        let entity = json!({ "id": "e-001", "project_canonical_id": null });
        assert!(!entity_matches_project(&entity, "github.com/owner/repo", "entry"));
    }

    #[test]
    fn test_entity_matches_project_matching() {
        // Matching project — accepted
        let entity = json!({ "id": "e-001", "project_canonical_id": "github.com/owner/repo" });
        assert!(entity_matches_project(&entity, "github.com/owner/repo", "entry"));
    }

    #[test]
    fn test_entity_matches_project_foreign() {
        // Different project — rejected (returns false)
        let entity = json!({ "id": "e-001", "project_canonical_id": "github.com/other/repo" });
        assert!(!entity_matches_project(&entity, "github.com/owner/repo", "entry"));
    }

    #[test]
    fn test_entity_matches_project_id_field_alias() {
        // Also checks `project_id` field as an alias
        let entity = json!({ "id": "t-abc", "project_id": "github.com/owner/repo" });
        assert!(entity_matches_project(&entity, "github.com/owner/repo", "task"));
    }

    #[test]
    fn test_entity_matches_project_id_field_foreign() {
        let entity = json!({ "id": "t-abc", "project_id": "github.com/other/repo" });
        assert!(!entity_matches_project(&entity, "github.com/owner/repo", "task"));
    }

    #[test]
    fn test_entity_matches_project_null_project_id() {
        // Null project_id — rejected; cloud must scope all entities (cas-6479)
        let entity = json!({ "id": "t-abc", "project_id": null });
        assert!(!entity_matches_project(&entity, "github.com/owner/repo", "task"));
    }

    #[test]
    fn test_entity_matches_local_project() {
        // local: prefix IDs work the same way
        let entity = json!({ "id": "p-001", "project_canonical_id": "local:abcd1234ef567890" });
        assert!(entity_matches_project(&entity, "local:abcd1234ef567890", "entry"));
        assert!(!entity_matches_project(&entity, "local:0000000000000000", "entry"));
    }
}

// cas-fc52: web-initiated close reconcile (cloud contract §4)
#[cfg(test)]
mod web_close_tests {
    use super::{is_web_close_tombstone, reconcile_web_close};
    use crate::cloud::syncer::UpsertResult;
    use crate::store::{init_cas_dir, open_task_store};
    use crate::types::{Task, TaskStatus};
    use serde_json::json;
    use tempfile::TempDir;

    fn closed_tombstone(id: &str) -> Task {
        let mut t = Task::new(id.to_string(), format!("title {id}"));
        t.status = TaskStatus::Closed;
        t.close_reason = Some("closed from web by teammate".to_string());
        t.closed_at = Some(chrono::Utc::now());
        t
    }

    #[test]
    fn detects_web_close_marker() {
        assert!(is_web_close_tombstone(
            &json!({ "id": "t1", "closed_via": "web" })
        ));
        // Our own pushed closes never carry closed_via == "web".
        assert!(!is_web_close_tombstone(
            &json!({ "id": "t1", "status": "closed" })
        ));
        assert!(!is_web_close_tombstone(
            &json!({ "id": "t1", "closed_via": "cli" })
        ));
        assert!(!is_web_close_tombstone(
            &json!({ "id": "t1", "closed_via": null })
        ));
    }

    #[test]
    fn reconcile_forces_close_on_open_task_even_if_local_is_newer() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();

        // Local in-progress task, assigned, with a NEWER updated_at than the
        // tombstone — a timestamp-gated upsert would skip it; the web close
        // must apply regardless.
        let mut local = Task::new("t-web".to_string(), "title".to_string());
        local.assignee = Some("agent-x".to_string());
        local.status = TaskStatus::InProgress;
        local.updated_at = chrono::Utc::now() + chrono::Duration::hours(1);
        store.add(&local).unwrap();

        let outcome = reconcile_web_close(&*store, closed_tombstone("t-web")).unwrap();
        assert!(matches!(outcome, UpsertResult::Updated));

        let got = store.get("t-web").unwrap();
        assert_eq!(got.status, TaskStatus::Closed);
        assert_eq!(
            got.close_reason.as_deref(),
            Some("closed from web by teammate")
        );
        assert!(
            got.assignee.is_none(),
            "assignee must be cleared on web close"
        );
    }

    #[test]
    fn reconcile_preserves_locally_authored_content() {
        // P1 (cas-71f7 review): a web close must NOT clobber local-only,
        // not-yet-pushed body content. Only the close signal is applied.
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();

        let mut local = Task::new("t-content".to_string(), "title".to_string());
        local.status = TaskStatus::InProgress;
        local.description = "local description not yet pushed".to_string();
        local.notes = "local working notes".to_string();
        local.updated_at = chrono::Utc::now() + chrono::Duration::hours(1);
        store.add(&local).unwrap();

        // Tombstone carries empty body fields (server snapshot) + the close.
        let outcome = reconcile_web_close(&*store, closed_tombstone("t-content")).unwrap();
        assert!(matches!(outcome, UpsertResult::Updated));

        let got = store.get("t-content").unwrap();
        assert_eq!(got.status, TaskStatus::Closed);
        assert_eq!(
            got.close_reason.as_deref(),
            Some("closed from web by teammate")
        );
        // Local-authored content survives the close.
        assert_eq!(got.description, "local description not yet pushed");
        assert_eq!(got.notes, "local working notes");
        assert!(got.assignee.is_none());
    }

    #[test]
    fn reconcile_is_idempotent_on_already_closed() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();

        let mut local = Task::new("t-done".to_string(), "title".to_string());
        local.status = TaskStatus::Closed;
        local.close_reason = Some("already closed locally".to_string());
        store.add(&local).unwrap();

        let outcome = reconcile_web_close(&*store, closed_tombstone("t-done")).unwrap();
        assert!(matches!(outcome, UpsertResult::Skipped));
        // The no-op must not clobber the pre-existing local close_reason.
        let got = store.get("t-done").unwrap();
        assert_eq!(got.close_reason.as_deref(), Some("already closed locally"));
    }

    #[test]
    fn reconcile_adds_unknown_closed_task() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();

        let outcome = reconcile_web_close(&*store, closed_tombstone("t-new")).unwrap();
        assert!(matches!(outcome, UpsertResult::Created));
        assert_eq!(store.get("t-new").unwrap().status, TaskStatus::Closed);
    }
}
