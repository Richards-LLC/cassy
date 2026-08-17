use std::time::Instant;

use serde::de::DeserializeOwned;

use crate::cloud::syncer::{
    CloudSyncer, ConflictAction, ConflictResolution, PullResponse, SyncResult,
    TaskStatusTransition, TeamPullResponse, UpsertResult,
};
use crate::cloud::{EntityType, get_project_canonical_id};
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

/// Deserialize one raw pull entity while retaining its wire identifier in any
/// error. Pull payloads are raw JSON specifically so one malformed row does
/// not abort the rest of a sync; without this boundary the resulting warning
/// was un-actionable because it named neither the row nor its entity type.
fn deserialize_pulled_entity<T: DeserializeOwned>(
    raw: serde_json::Value,
    entity_type: &str,
) -> Result<T, String> {
    let id = raw
        .get("id")
        .or_else(|| raw.get("entity_id"))
        .or_else(|| raw.get("commit_hash"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<missing-id>")
        .to_owned();
    serde_json::from_value(raw)
        .map_err(|error| format!("{entity_type} deserialize error (id={id}): {error}"))
}

const PROPOSAL_PROVENANCE_BEGIN: &str = "--- BEGIN SERVER-ATTESTED PROPOSAL PROVENANCE ---";
const PROPOSAL_PROVENANCE_END: &str = "--- END CLIENT-ASSERTED PROPOSAL PROVENANCE ---";

fn render_provenance_value(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"(unrenderable)\"".to_string())
}

fn unique_complete_provenance_block(notes: &str) -> Option<(usize, usize)> {
    let begins = notes
        .match_indices(PROPOSAL_PROVENANCE_BEGIN)
        .collect::<Vec<_>>();
    let ends = notes
        .match_indices(PROPOSAL_PROVENANCE_END)
        .collect::<Vec<_>>();
    if begins.len() != 1 || ends.len() != 1 {
        return None;
    }
    let start = begins[0].0;
    let end = ends[0].0 + PROPOSAL_PROVENANCE_END.len();
    let marker_is_line = |offset: usize, marker_len: usize| {
        (offset == 0 || notes.as_bytes().get(offset.wrapping_sub(1)) == Some(&b'\n'))
            && (offset + marker_len == notes.len()
                || notes.as_bytes().get(offset + marker_len) == Some(&b'\n'))
    };
    (start < ends[0].0
        && marker_is_line(start, PROPOSAL_PROVENANCE_BEGIN.len())
        && marker_is_line(ends[0].0, PROPOSAL_PROVENANCE_END.len()))
    .then_some((start, end))
}

/// Persist proposal provenance visibly in the ordinary local task record.
/// The cloud proposal row remains authoritative; this rendering is explicitly
/// labeled and is refreshed whenever the materialized task is pulled.
fn render_task_proposal_provenance(raw: &mut serde_json::Value) {
    let Some(raw_provenance) = raw.get("proposal_provenance").cloned() else {
        return;
    };
    let Ok(provenance) =
        serde_json::from_value::<crate::cloud::task_proposals::ProposalProvenance>(raw_provenance)
    else {
        return;
    };
    let server = &provenance.server_attested;
    let client = &provenance.client_asserted;
    let rendered = format!(
        "{PROPOSAL_PROVENANCE_BEGIN}\n  proposal_id: {}\n  target_task_id: {}\n  creator_user_id: {}\n  team_id: {}\n  origin_project_canonical_id: {}\n  target_project_canonical_id: {}\n  received_at: {}\n  client_request_id: {}\n--- END SERVER-ATTESTED PROPOSAL PROVENANCE ---\n\n--- BEGIN CLIENT-ASSERTED PROPOSAL PROVENANCE ---\n  origin_session_id: {}\n  origin_agent_id: {}\n  origin_agent_name: {}\n  origin_agent_role: {}\n  client_version: {}\n  client_build: {}\n{PROPOSAL_PROVENANCE_END}",
        render_provenance_value(&server.proposal_id),
        render_provenance_value(&server.target_task_id),
        render_provenance_value(&server.creator_user_id),
        render_provenance_value(&server.team_id),
        render_provenance_value(&server.origin_project_canonical_id),
        render_provenance_value(&server.target_project_canonical_id),
        render_provenance_value(&server.received_at),
        render_provenance_value(&server.client_request_id),
        client
            .origin_session_id
            .as_deref()
            .map(render_provenance_value)
            .unwrap_or_else(|| "(not asserted)".to_string()),
        client
            .origin_agent_id
            .as_deref()
            .map(render_provenance_value)
            .unwrap_or_else(|| "(not asserted)".to_string()),
        client
            .origin_agent_name
            .as_deref()
            .map(render_provenance_value)
            .unwrap_or_else(|| "(not asserted)".to_string()),
        client
            .origin_agent_role
            .as_deref()
            .map(render_provenance_value)
            .unwrap_or_else(|| "(not asserted)".to_string()),
        client
            .client_version
            .as_deref()
            .map(render_provenance_value)
            .unwrap_or_else(|| "(not asserted)".to_string()),
        client
            .client_build
            .as_deref()
            .map(render_provenance_value)
            .unwrap_or_else(|| "(not asserted)".to_string()),
    );
    let notes = raw
        .get("notes")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let updated = if let Some((start, end)) = unique_complete_provenance_block(notes) {
        format!("{}{}{}", &notes[..start], rendered, &notes[end..])
    } else if notes.is_empty() {
        rendered
    } else {
        format!("{}\n\n{}", notes.trim_end(), rendered)
    };
    raw["notes"] = serde_json::Value::String(updated);
}

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
/// The per-row ingest guard. Shared with the knowledge pull (`knowledge.rs`)
/// so both directions of the sync client apply *one* definition of "is this
/// row mine", rather than a second implementation that can drift from this one.
///
/// The equality is deliberately byte-exact — see the protocol invariant in
/// `docs/`/ARCHITECTURE and `canonical_id_equality_is_byte_exact_by_protocol`
/// below. Normalizing here would silently merge two distinct projects.
pub(crate) fn entity_matches_project(
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
fn reconcile_web_close(
    store: &dyn TaskStore,
    task: Task,
    sync_id: &str,
    source: &str,
) -> Result<UpsertResult, CasError> {
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
            let mut merged = local.clone();
            merged.status = TaskStatus::Closed;
            merged.close_reason = task.close_reason;
            merged.closed_at = task.closed_at.or(merged.closed_at);
            merged.updated_at = task.updated_at;
            merged.assignee = None;
            append_sync_status_provenance(&mut merged, &local, sync_id, source);
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

/// Merge append-only task-note blocks without allowing a whole-row conflict
/// resolution to discard one machine's timeline.
fn merge_task_notes(local: &str, remote: &str) -> String {
    let mut blocks = Vec::<(Option<chrono::NaiveDateTime>, usize, String)>::new();
    for notes in [local, remote] {
        for block in notes.split("\n\n") {
            let block = block.trim();
            if block.is_empty() || blocks.iter().any(|(_, _, existing)| existing == block) {
                continue;
            }
            let timestamp = block
                .strip_prefix('[')
                .and_then(|value| value.split_once(']'))
                .and_then(|(value, _)| {
                    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M").ok()
                });
            let order = blocks.len();
            blocks.push((timestamp, order, block.to_string()));
        }
    }
    blocks.sort_by_key(|(timestamp, order, _)| (timestamp.is_none(), *timestamp, *order));
    blocks
        .into_iter()
        .map(|(_, _, note)| note)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A terminal row is a durable outcome, not merely the latest version of a
/// mutable task body. An incoming active row may replace it only when the
/// remote task carries the audit event written by CAS's authorised `reopen`
/// action after the local terminal timestamp.
fn rejects_terminal_regression(local: &Task, remote: &Task) -> bool {
    if !local.is_terminal() || remote.is_terminal() {
        return false;
    }

    !has_explicit_remote_reopen(remote, local.closed_at)
}

/// CAS's `task reopen` action writes `[YYYY-mm-dd HH:MM] Reopened: <reason>`
/// into the task's replicated note timeline. Treat that timestamped record as
/// the reopening event; a bare active task row, even one with a newer
/// `updated_at`, is not authorization to undo a close/cancellation.
fn has_explicit_remote_reopen(
    remote: &Task,
    local_closed_at: Option<chrono::DateTime<chrono::Utc>>,
) -> bool {
    let Some(local_closed_at) = local_closed_at else {
        return false;
    };
    remote.notes.split("\n\n").any(|note| {
        let Some((timestamp, event)) = note.strip_prefix('[').and_then(|note| note.split_once(']'))
        else {
            return false;
        };
        if !event.trim_start().starts_with("Reopened:") {
            return false;
        }
        let Ok(timestamp) = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M")
        else {
            return false;
        };
        let timestamp = timestamp.and_utc();
        timestamp >= local_closed_at - chrono::Duration::minutes(1)
    })
}

fn append_sync_status_provenance(merged: &mut Task, local: &Task, sync_id: &str, source: &str) {
    if local.status == merged.status {
        return;
    }

    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
    let prior_close_reason = local
        .close_reason
        .as_deref()
        .unwrap_or("<none>")
        .replace(['\n', '\r'], " ");
    let note = format!(
        "[{timestamp}] [CAS_SYNC_STATUS] sync_id={sync_id} source={source} prior_status={} prior_close_reason={prior_close_reason} applied_status={}",
        local.status, merged.status,
    );
    if merged.notes.is_empty() {
        merged.notes = note;
    } else {
        merged.notes.push_str("\n\n");
        merged.notes.push_str(&note);
    }
}

impl CloudSyncer {
    fn record_terminal_regression_conflict(
        &self,
        local: &Task,
        remote: &Task,
    ) -> Result<(), CasError> {
        let discarded_row_json = serde_json::to_string(&serde_json::json!({
            "local": local,
            "rejected_remote": remote,
            "reason": "terminal_status_guard",
        }))
        .map_err(|error| {
            CasError::Other(format!(
                "Could not serialize terminal sync conflict: {error}"
            ))
        })?;
        self.queue.record_conflict(
            EntityType::Task.as_str(),
            &local.id,
            &discarded_row_json,
            "local",
            "terminal_status_guard",
        )?;
        tracing::warn!(
            task_id = %local.id,
            local_status = %local.status,
            remote_status = %remote.status,
            "cloud pull rejected an unproven terminal task regression"
        );
        Ok(())
    }

    fn journal_local_overwrite<T: serde::Serialize>(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        local: &T,
        winner_side: &str,
        strategy: &str,
    ) -> Result<(), CasError> {
        if self
            .queue
            .has_pending_entity_change(entity_type, entity_id)?
        {
            let json = serde_json::to_string(local).map_err(|error| {
                CasError::Other(format!("Could not serialize sync conflict: {error}"))
            })?;
            self.queue.record_conflict(
                entity_type.as_str(),
                entity_id,
                &json,
                winner_side,
                strategy,
            )?;
        }
        Ok(())
    }

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
        let had_prior_watermark = since.is_some();

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
            let remote_entry: Entry = match deserialize_pulled_entity(raw_entry, "entry") {
                Ok(e) => e,
                Err(e) => {
                    result.errors.push(e);
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
        let task_sync_id = uuid::Uuid::new_v4().to_string();
        for mut raw_task in body.tasks.unwrap_or_default() {
            if !entity_matches_project(&raw_task, &current_project_id, "task") {
                continue;
            }
            // cas-fc52: a teammate's web-initiated close arrives as a soft
            // tombstone (closed_via == "web"). The CLI owns the real local
            // close — apply it authoritatively rather than via the
            // timestamp-gated upsert.
            let web_close = is_web_close_tombstone(&raw_task);
            render_task_proposal_provenance(&mut raw_task);
            let remote_task: Task = match deserialize_pulled_entity(raw_task, "task") {
                Ok(t) => t,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            let previous_status = task_store.get(&remote_task.id).ok().map(|task| task.status);
            let task_outcome = if web_close {
                reconcile_web_close(
                    task_store,
                    remote_task.clone(),
                    &task_sync_id,
                    "personal_pull",
                )
            } else {
                self.upsert_task(
                    task_store,
                    remote_task.clone(),
                    &task_sync_id,
                    "personal_pull",
                )
            };
            match task_outcome {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_tasks += 1;
                    if let Some(from) = previous_status.filter(|from| *from != remote_task.status) {
                        result.task_status_transitions.push(TaskStatusTransition {
                            task_id: remote_task.id,
                            project_id: current_project_id.to_string(),
                            source: "personal_pull".to_string(),
                            from,
                            to: remote_task.status,
                        });
                    }
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
            let remote_rule: Rule = match deserialize_pulled_entity(raw_rule, "rule") {
                Ok(r) => r,
                Err(e) => {
                    result.errors.push(e);
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
            let remote_skill: Skill = match deserialize_pulled_entity(raw_skill, "skill") {
                Ok(s) => s,
                Err(e) => {
                    result.errors.push(e);
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
            let remote_spec: Spec = match deserialize_pulled_entity(raw_spec, "spec") {
                Ok(s) => s,
                Err(e) => {
                    result.errors.push(e);
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
            let remote_event: Event = match deserialize_pulled_entity(raw_event, "event") {
                Ok(e) => e,
                Err(e) => {
                    result.errors.push(e);
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
            let remote_prompt: Prompt = match deserialize_pulled_entity(raw_prompt, "prompt") {
                Ok(p) => p,
                Err(e) => {
                    result.errors.push(e);
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
            let remote_fc: FileChange = match deserialize_pulled_entity(raw_fc, "file_change") {
                Ok(f) => f,
                Err(e) => {
                    result.errors.push(e);
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
                Err(e) => result.errors.push(format!("FileChange lookup error: {e}")),
            }
        }

        // Process commit links (keyed by `commit_hash`).
        for raw_cl in body.commit_links.unwrap_or_default() {
            if !entity_matches_project(&raw_cl, &current_project_id, "commit_link") {
                continue;
            }
            let remote_cl: CommitLink = match deserialize_pulled_entity(raw_cl, "commit_link") {
                Ok(c) => c,
                Err(e) => {
                    result.errors.push(e);
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
                Err(e) => result.errors.push(format!("CommitLink lookup error: {e}")),
            }
        }

        // An empty first pull can mean this new machine resolved the wrong
        // canonical project id. Stamping that response's watermark would make
        // a later corrected-id pull incremental and permanently skip the
        // historical backfill (GH #192). Once a project has established a
        // watermark, retain the existing behavior: healthy empty incremental
        // pulls advance to the server clock.
        if (had_prior_watermark || result.total_pulled() > 0)
            && let Some(pulled_at) = body.pulled_at
        {
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
                    self.journal_local_overwrite(
                        EntityType::Entry,
                        &entry.id,
                        &local,
                        "remote",
                        "timestamp_lww",
                    )?;
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

    fn upsert_task(
        &self,
        store: &dyn TaskStore,
        task: Task,
        sync_id: &str,
        source: &str,
    ) -> Result<UpsertResult, CasError> {
        match store.get(&task.id) {
            Ok(local) => {
                if rejects_terminal_regression(&local, &task) {
                    self.record_terminal_regression_conflict(&local, &task)?;
                    return Ok(UpsertResult::Skipped);
                }
                if task.updated_at > local.updated_at {
                    let notes_differ = local.notes != task.notes;
                    self.journal_local_overwrite(
                        EntityType::Task,
                        &task.id,
                        &local,
                        if notes_differ { "merged" } else { "remote" },
                        if notes_differ {
                            "notes_union"
                        } else {
                            "timestamp_lww"
                        },
                    )?;
                    let mut merged = task;
                    if notes_differ {
                        merged.notes = merge_task_notes(&local.notes, &merged.notes);
                    }
                    append_sync_status_provenance(&mut merged, &local, sync_id, source);
                    store.update(&merged)?;
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
                    self.journal_local_overwrite(
                        EntityType::Rule,
                        &rule.id,
                        &local,
                        "remote",
                        "timestamp_lww",
                    )?;
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
                    self.journal_local_overwrite(
                        EntityType::Skill,
                        &skill.id,
                        &local,
                        "remote",
                        "timestamp_lww",
                    )?;
                    store.update(&skill)?;
                    Ok(UpsertResult::Updated)
                } else {
                    Ok(UpsertResult::Skipped)
                }
            }
            // SqliteSkillStore::get reports a missing row as the generic
            // `NotFound`, while some store implementations use the older
            // skill-specific variant. Both mean a team member has not pulled
            // this shared skill yet and must take the insert path.
            Err(cas_store::StoreError::SkillNotFound(_) | cas_store::StoreError::NotFound(_)) => {
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
                        self.journal_local_overwrite(
                            EntityType::Entry,
                            &entry.id,
                            &local,
                            "remote",
                            strategy.as_str(),
                        )?;
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
        sync_id: &str,
        source: &str,
    ) -> Result<UpsertResult, CasError> {
        match store.get(&task.id) {
            Ok(local) => {
                if rejects_terminal_regression(&local, &task) {
                    self.record_terminal_regression_conflict(&local, &task)?;
                    return Ok(UpsertResult::Skipped);
                }
                let action = self.resolve_conflict(
                    "task",
                    &task.id,
                    local.updated_at,
                    task.updated_at,
                    strategy,
                );

                match action {
                    ConflictAction::UseRemote => {
                        let notes_differ = local.notes != task.notes;
                        self.journal_local_overwrite(
                            EntityType::Task,
                            &task.id,
                            &local,
                            if notes_differ { "merged" } else { "remote" },
                            if notes_differ {
                                "notes_union"
                            } else {
                                strategy.as_str()
                            },
                        )?;
                        let mut merged = task;
                        if notes_differ {
                            merged.notes = merge_task_notes(&local.notes, &merged.notes);
                        }
                        append_sync_status_provenance(&mut merged, &local, sync_id, source);
                        store.update(&merged)?;
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
                        self.journal_local_overwrite(
                            EntityType::Rule,
                            &rule.id,
                            &local,
                            "remote",
                            strategy.as_str(),
                        )?;
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
                        self.journal_local_overwrite(
                            EntityType::Skill,
                            &skill.id,
                            &local,
                            "remote",
                            strategy.as_str(),
                        )?;
                        store.update(&skill)?;
                        Ok(UpsertResult::Updated)
                    }
                    ConflictAction::UseLocal | ConflictAction::Skip => Ok(UpsertResult::Skipped),
                }
            }
            // Keep team and personal pulls aligned: a fresh local store may
            // represent an absent skill with either missing-row variant.
            Err(cas_store::StoreError::SkillNotFound(_) | cas_store::StoreError::NotFound(_)) => {
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
            task_status_transitions: pull_result.task_status_transitions,
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
            let remote_entry: Entry = match deserialize_pulled_entity(raw_entry, "entry") {
                Ok(e) => e,
                Err(e) => {
                    result.errors.push(e);
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
        let task_sync_id = uuid::Uuid::new_v4().to_string();
        for mut raw_task in body.tasks.unwrap_or_default() {
            if !entity_matches_project(&raw_task, &current_project_id, "task") {
                continue;
            }
            render_task_proposal_provenance(&mut raw_task);
            let remote_task: Task = match deserialize_pulled_entity(raw_task, "task") {
                Ok(t) => t,
                Err(e) => {
                    result.errors.push(e);
                    continue;
                }
            };
            let previous_status = task_store.get(&remote_task.id).ok().map(|task| task.status);
            match self.upsert_task_with_strategy(
                task_store,
                remote_task.clone(),
                strategy,
                &task_sync_id,
                "team_pull",
            ) {
                Ok(UpsertResult::Created) | Ok(UpsertResult::Updated) => {
                    result.pulled_tasks += 1;
                    if let Some(from) = previous_status.filter(|from| *from != remote_task.status) {
                        result.task_status_transitions.push(TaskStatusTransition {
                            task_id: remote_task.id,
                            project_id: current_project_id.to_string(),
                            source: "team_pull".to_string(),
                            from,
                            to: remote_task.status,
                        });
                    }
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
            let remote_rule: Rule = match deserialize_pulled_entity(raw_rule, "rule") {
                Ok(r) => r,
                Err(e) => {
                    result.errors.push(e);
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
            let remote_skill: Skill = match deserialize_pulled_entity(raw_skill, "skill") {
                Ok(s) => s,
                Err(e) => {
                    result.errors.push(e);
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
    use super::{
        PROPOSAL_PROVENANCE_BEGIN, PROPOSAL_PROVENANCE_END, PULL_PATH,
        build_scoped_pull_url_with, deserialize_pulled_entity, entity_matches_project,
        render_task_proposal_provenance,
    };
    use crate::types::Entry;
    use serde_json::json;

    #[test]
    fn deserialize_error_names_the_wire_entity_type_and_id() {
        let error = deserialize_pulled_entity::<Entry>(
            json!({
                "id": "p-malformed-created",
                "project_id": "cas-src",
                "content": "legacy entry without its creation timestamp",
            }),
            "entry",
        )
        .unwrap_err();

        assert!(error.contains("entry deserialize error"), "{error}");
        assert!(error.contains("id=p-malformed-created"), "{error}");
        assert!(error.contains("missing field `created`"), "{error}");
    }

    #[test]
    fn materialized_task_renders_attested_and_asserted_provenance_visibly() {
        let mut raw = json!({
            "notes": "Receiver notes",
            "proposal_provenance": {
                "server_attested": {
                    "proposal_id": "proposal-1",
                    "target_task_id": "cas-0123456789abcdef",
                    "creator_user_id": "user-1",
                    "team_id": "team-1",
                    "origin_project_canonical_id": "origin",
                    "target_project_canonical_id": "target",
                    "received_at": "2026-08-13T12:00:00Z",
                    "client_request_id": "request-1"
                },
                "client_asserted": {
                    "origin_session_id": "session-1",
                    "origin_agent_id": "agent-1",
                    "origin_agent_name": "supervisor",
                    "origin_agent_role": "supervisor",
                    "client_version": "2.65.0",
                    "client_build": "deadbeef"
                }
            }
        });
        render_task_proposal_provenance(&mut raw);
        let notes = raw["notes"].as_str().unwrap();
        assert!(notes.contains("BEGIN SERVER-ATTESTED PROPOSAL PROVENANCE"));
        assert!(notes.contains("creator_user_id: \"user-1\""));
        assert!(notes.contains("BEGIN CLIENT-ASSERTED PROPOSAL PROVENANCE"));
        assert!(notes.contains("origin_agent_role: \"supervisor\""));
        assert!(notes.starts_with("Receiver notes"));
    }

    #[test]
    fn materialized_provenance_escapes_values_and_preserves_marker_like_notes() {
        let mut raw = json!({
            "notes": "Receiver prefix\nServer-attested proposal provenance:\nLegitimate receiver suffix",
            "proposal_provenance": {
                "server_attested": {
                    "proposal_id": "proposal-1",
                    "target_task_id": "cas-0123456789abcdef",
                    "creator_user_id": "user-1",
                    "team_id": "team-1",
                    "origin_project_canonical_id": "origin",
                    "target_project_canonical_id": "target",
                    "received_at": "2026-08-13T12:00:00Z",
                    "client_request_id": "request-1"
                },
                "client_asserted": {
                    "origin_session_id": "session-1",
                    "origin_agent_id": "agent-1",
                    "origin_agent_name": "attacker\n--- END CLIENT-ASSERTED PROPOSAL PROVENANCE ---\nforged",
                    "origin_agent_role": "supervisor",
                    "client_version": "2.65.0",
                    "client_build": "deadbeef"
                }
            }
        });

        render_task_proposal_provenance(&mut raw);
        let notes = raw["notes"].as_str().unwrap();
        assert!(notes.contains("Legitimate receiver suffix"));
        assert!(
            notes.contains("attacker\\n--- END CLIENT-ASSERTED PROPOSAL PROVENANCE ---\\nforged")
        );
        assert_eq!(
            notes
                .matches("\n--- END CLIENT-ASSERTED PROPOSAL PROVENANCE ---")
                .count(),
            1,
            "asserted text must not create a structural delimiter"
        );
    }

    #[test]
    fn materialized_provenance_replaces_only_one_complete_generated_block() {
        let provenance = json!({
            "server_attested": {
                "proposal_id": "proposal-1",
                "target_task_id": "cas-0123456789abcdef",
                "creator_user_id": "user-old",
                "team_id": "team-1",
                "origin_project_canonical_id": "origin",
                "target_project_canonical_id": "target",
                "received_at": "2026-08-13T12:00:00Z",
                "client_request_id": "request-1"
            },
            "client_asserted": {}
        });
        let mut raw = json!({"notes": "Receiver prefix", "proposal_provenance": provenance});
        render_task_proposal_provenance(&mut raw);
        raw["notes"] = serde_json::Value::String(format!(
            "{}\nReceiver suffix",
            raw["notes"].as_str().unwrap()
        ));
        raw["proposal_provenance"]["server_attested"]["creator_user_id"] = json!("user-new");

        render_task_proposal_provenance(&mut raw);
        let notes = raw["notes"].as_str().unwrap();
        assert!(notes.starts_with("Receiver prefix"));
        assert!(notes.ends_with("Receiver suffix"));
        assert!(notes.contains("creator_user_id: \"user-new\""));
        assert!(!notes.contains("creator_user_id: \"user-old\""));
        assert_eq!(notes.matches(PROPOSAL_PROVENANCE_BEGIN).count(), 1);
        assert_eq!(notes.matches(PROPOSAL_PROVENANCE_END).count(), 1);
    }

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
        assert!(!entity_matches_project(
            &entity,
            "github.com/owner/repo",
            "entry"
        ));
    }

    #[test]
    fn test_entity_matches_project_null_field() {
        // Null project_canonical_id — rejected; cloud must scope all entities (cas-6479)
        let entity = json!({ "id": "e-001", "project_canonical_id": null });
        assert!(!entity_matches_project(
            &entity,
            "github.com/owner/repo",
            "entry"
        ));
    }

    #[test]
    fn test_entity_matches_project_matching() {
        // Matching project — accepted
        let entity = json!({ "id": "e-001", "project_canonical_id": "github.com/owner/repo" });
        assert!(entity_matches_project(
            &entity,
            "github.com/owner/repo",
            "entry"
        ));
    }

    #[test]
    fn test_entity_matches_project_foreign() {
        // Different project — rejected (returns false)
        let entity = json!({ "id": "e-001", "project_canonical_id": "github.com/other/repo" });
        assert!(!entity_matches_project(
            &entity,
            "github.com/owner/repo",
            "entry"
        ));
    }

    #[test]
    fn test_entity_matches_project_id_field_alias() {
        // Also checks `project_id` field as an alias
        let entity = json!({ "id": "t-abc", "project_id": "github.com/owner/repo" });
        assert!(entity_matches_project(
            &entity,
            "github.com/owner/repo",
            "task"
        ));
    }

    #[test]
    fn test_entity_matches_project_id_field_foreign() {
        let entity = json!({ "id": "t-abc", "project_id": "github.com/other/repo" });
        assert!(!entity_matches_project(
            &entity,
            "github.com/owner/repo",
            "task"
        ));
    }

    #[test]
    fn test_entity_matches_project_null_project_id() {
        // Null project_id — rejected; cloud must scope all entities (cas-6479)
        let entity = json!({ "id": "t-abc", "project_id": null });
        assert!(!entity_matches_project(
            &entity,
            "github.com/owner/repo",
            "task"
        ));
    }

    #[test]
    fn test_entity_matches_local_project() {
        // local: prefix IDs work the same way
        let entity = json!({ "id": "p-001", "project_canonical_id": "local:abcd1234ef567890" });
        assert!(entity_matches_project(
            &entity,
            "local:abcd1234ef567890",
            "entry"
        ));
        assert!(!entity_matches_project(
            &entity,
            "local:0000000000000000",
            "entry"
        ));
    }

    /// PROTOCOL INVARIANT — do not "fix" this test by making the comparison
    /// case-insensitive or otherwise normalizing.
    ///
    /// Canonical-id equality is byte-exact on BOTH sides of the wire, by
    /// agreement with the server. Two consequences the next reader needs:
    ///
    /// 1. **Normalizing here would be a data-merge, not a convenience.**
    ///    `Accounting` and `accounting` are two distinct projects as far as
    ///    every stored row is concerned; folding them together would
    ///    cross-contaminate them permanently and unattributably.
    /// 2. **This is the SECOND line of defence, not the first.** The server
    ///    filters on the id the client SENDS and echoes the stored column, so
    ///    an id divergence does not present here as a rejected row — the
    ///    client receives an *empty envelope*, indefinitely, with no warning
    ///    on either side. Silent starvation, not contamination. If you are
    ///    debugging "sync returns nothing", suspect an id mismatch upstream of
    ///    this function rather than assuming this check is dropping rows.
    ///
    /// The remedy for divergence is client-side pinning of the canonical id;
    /// the server deliberately refused to normalize for exactly the reason in
    /// (1).
    #[test]
    fn canonical_id_equality_is_byte_exact_by_protocol() {
        let entity = json!({ "id": "t-1", "project_canonical_id": "github.com/Acme/Ledger" });

        assert!(
            entity_matches_project(&entity, "github.com/Acme/Ledger", "task"),
            "exact match must be accepted"
        );
        // Case variants are DIFFERENT projects. Each of these must be refused.
        for variant in [
            "github.com/acme/ledger",
            "github.com/ACME/LEDGER",
            "github.com/Acme/ledger",
        ] {
            assert!(
                !entity_matches_project(&entity, variant, "task"),
                "case variant '{variant}' must not match — normalizing here would \
                 silently merge two distinct projects"
            );
        }
        // Whitespace and trailing-separator variants are likewise distinct.
        for variant in [
            "github.com/Acme/Ledger ",
            " github.com/Acme/Ledger",
            "github.com/Acme/Ledger/",
        ] {
            assert!(
                !entity_matches_project(&entity, variant, "task"),
                "variant '{variant:?}' must not match: equality is byte-exact"
            );
        }
    }
}

// cas-fc52: web-initiated close reconcile (cloud contract §4)
#[cfg(test)]
mod web_close_tests {
    use super::{CloudSyncer, is_web_close_tombstone, merge_task_notes, reconcile_web_close};
    use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
    use crate::cloud::syncer::{CloudSyncerConfig, UpsertResult};
    use crate::store::{init_cas_dir, open_task_store};
    use crate::types::{Task, TaskStatus};
    use serde_json::json;
    use std::sync::Arc;
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

        let outcome =
            reconcile_web_close(&*store, closed_tombstone("t-web"), "sync-web", "personal_pull")
                .unwrap();
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
        let outcome = reconcile_web_close(
            &*store,
            closed_tombstone("t-content"),
            "sync-web",
            "personal_pull",
        )
        .unwrap();
        assert!(matches!(outcome, UpsertResult::Updated));

        let got = store.get("t-content").unwrap();
        assert_eq!(got.status, TaskStatus::Closed);
        assert_eq!(
            got.close_reason.as_deref(),
            Some("closed from web by teammate")
        );
        // Local-authored content survives the close.
        assert_eq!(got.description, "local description not yet pushed");
        assert!(got.notes.starts_with("local working notes"));
        assert!(got.notes.contains("[CAS_SYNC_STATUS]"));
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

        let outcome =
            reconcile_web_close(&*store, closed_tombstone("t-done"), "sync-web", "personal_pull")
                .unwrap();
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

        let outcome =
            reconcile_web_close(&*store, closed_tombstone("t-new"), "sync-web", "personal_pull")
                .unwrap();
        assert!(matches!(outcome, UpsertResult::Created));
        assert_eq!(store.get("t-new").unwrap().status, TaskStatus::Closed);
    }

    #[test]
    fn task_note_union_is_timestamp_ordered_and_deduplicated() {
        let local = "[2026-08-09 10:30] 📝 PROGRESS local follow-up\n\n[2026-08-09 11:30] 📝 PROGRESS shared";
        let remote =
            "[2026-08-09 09:30] 📝 PROGRESS remote first\n\n[2026-08-09 11:30] 📝 PROGRESS shared";

        assert_eq!(
            merge_task_notes(local, remote),
            "[2026-08-09 09:30] 📝 PROGRESS remote first\n\n[2026-08-09 10:30] 📝 PROGRESS local follow-up\n\n[2026-08-09 11:30] 📝 PROGRESS shared"
        );
    }

    #[test]
    fn local_task_conflict_unions_notes_and_journals_the_losing_row() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();
        let queue = Arc::new(SyncQueue::open(&cas_dir).unwrap());
        queue.init().unwrap();
        let syncer = CloudSyncer::new(queue.clone(), CloudConfig::default(), CloudSyncerConfig::default());

        let mut local = Task::new("cas-conflict".to_string(), "local title".to_string());
        local.notes = "[2026-08-09 10:30] 📝 PROGRESS local note".to_string();
        store.add(&local).unwrap();
        queue.enqueue(EntityType::Task, &local.id, SyncOperation::Upsert, None).unwrap();

        let mut remote = local.clone();
        remote.title = "remote title".to_string();
        remote.notes = "[2026-08-09 09:30] 📝 PROGRESS remote note".to_string();
        remote.updated_at += chrono::Duration::minutes(1);
        assert!(matches!(
            syncer.upsert_task(&*store, remote, "sync-test", "personal_pull"),
            Ok(UpsertResult::Updated)
        ));

        let merged = store.get("cas-conflict").unwrap();
        assert_eq!(merged.title, "remote title");
        assert_eq!(merged.notes, "[2026-08-09 09:30] 📝 PROGRESS remote note\n\n[2026-08-09 10:30] 📝 PROGRESS local note");
        let conflicts = queue.list_conflicts(1).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].winner_side, "merged");
        assert!(conflicts[0].discarded_row_json.contains("local note"));
    }

    #[test]
    fn stale_active_remote_row_cannot_resurrect_closed_task_and_is_retained_as_conflict() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();
        let queue = Arc::new(SyncQueue::open(&cas_dir).unwrap());
        queue.init().unwrap();
        let syncer = CloudSyncer::new(
            queue.clone(),
            CloudConfig::default(),
            CloudSyncerConfig::default(),
        );

        let mut local = Task::new("cas-gh451".to_string(), "closed incident".to_string());
        local.status = TaskStatus::Closed;
        local.closed_at = Some(chrono::Utc::now() - chrono::Duration::hours(2));
        local.close_reason = Some("production incident remediated".to_string());
        store.add(&local).unwrap();

        // Mirrors GH #451: a cloud row is stamped newer, but contains only an
        // active status and no authorised reopen record.
        let mut stale_remote = local.clone();
        stale_remote.status = TaskStatus::Open;
        stale_remote.closed_at = None;
        stale_remote.close_reason = None;
        stale_remote.updated_at = chrono::Utc::now();
        assert!(matches!(
            syncer.upsert_task(&*store, stale_remote, "sync-gh451", "personal_pull"),
            Ok(UpsertResult::Skipped)
        ));

        let retained = store.get("cas-gh451").unwrap();
        assert_eq!(retained.status, TaskStatus::Closed);
        assert_eq!(
            retained.close_reason.as_deref(),
            Some("production incident remediated")
        );
        let conflicts = queue.list_conflicts(1).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].winner_side, "local");
        assert_eq!(conflicts[0].strategy, "terminal_status_guard");
        assert!(conflicts[0].discarded_row_json.contains("rejected_remote"));
    }

    #[test]
    fn team_remote_wins_cannot_bypass_terminal_status_guard() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();
        let queue = Arc::new(SyncQueue::open(&cas_dir).unwrap());
        queue.init().unwrap();
        let syncer = CloudSyncer::new(queue, CloudConfig::default(), CloudSyncerConfig::default());

        let mut local = Task::new("cas-team-terminal".to_string(), "closed task".to_string());
        local.status = TaskStatus::Cancelled;
        local.closed_at = Some(chrono::Utc::now() - chrono::Duration::hours(1));
        store.add(&local).unwrap();

        let mut remote = local.clone();
        remote.status = TaskStatus::InProgress;
        remote.updated_at = chrono::Utc::now();
        assert!(matches!(
            syncer.upsert_task_with_strategy(
                &*store,
                remote,
                crate::cloud::syncer::ConflictResolution::RemoteWins,
                "sync-team",
                "team_pull",
            ),
            Ok(UpsertResult::Skipped)
        ));
        assert_eq!(store.get("cas-team-terminal").unwrap().status, TaskStatus::Cancelled);
    }

    #[test]
    fn explicit_remote_reopen_applies_and_has_machine_provenance() {
        let temp = TempDir::new().unwrap();
        let cas_dir = init_cas_dir(temp.path()).unwrap();
        let store = open_task_store(&cas_dir).unwrap();
        let queue = Arc::new(SyncQueue::open(&cas_dir).unwrap());
        queue.init().unwrap();
        let syncer = CloudSyncer::new(queue, CloudConfig::default(), CloudSyncerConfig::default());

        let mut local = Task::new("cas-reopen".to_string(), "closed task".to_string());
        local.status = TaskStatus::Closed;
        local.closed_at = Some(chrono::Utc::now() - chrono::Duration::hours(1));
        local.close_reason = Some("original delivery merged".to_string());
        store.add(&local).unwrap();

        let mut remote = local.clone();
        remote.status = TaskStatus::Open;
        remote.closed_at = None;
        remote.close_reason = None;
        remote.updated_at = chrono::Utc::now() + chrono::Duration::minutes(1);
        remote.notes = format!(
            "[{}] Reopened: regression found after deployment",
            chrono::Utc::now().format("%Y-%m-%d %H:%M")
        );
        assert!(matches!(
            syncer.upsert_task(&*store, remote, "sync-reopen", "personal_pull"),
            Ok(UpsertResult::Updated)
        ));

        let reopened = store.get("cas-reopen").unwrap();
        assert_eq!(reopened.status, TaskStatus::Open);
        assert!(reopened.notes.contains("[CAS_SYNC_STATUS]"));
        assert!(reopened.notes.contains("sync_id=sync-reopen"));
        assert!(reopened.notes.contains("source=personal_pull"));
        assert!(reopened.notes.contains("prior_status=closed"));
        assert!(
            reopened
                .notes
                .contains("prior_close_reason=original delivery merged")
        );
    }
}
