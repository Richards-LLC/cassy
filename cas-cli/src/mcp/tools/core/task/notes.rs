use crate::mcp::tools::core::imports::*;

/// Exact numeric notification references, without prefix collisions (#12 != #123).
pub(crate) fn note_notification_ids(text: &str) -> Vec<i64> {
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '#' && c != '=')
        .filter_map(|token| {
            token
                .strip_prefix('#')
                .or_else(|| token.strip_prefix("notification_id="))
        })
        .filter_map(|id| id.parse::<i64>().ok())
        .filter(|id| *id > 0)
        .collect()
}

impl CasCore {
    pub async fn cas_task_notes_read(
        &self,
        Parameters(req): Parameters<TaskNotesReadRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;
        let task = task_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        let full = if task.notes.is_empty() {
            format!("No notes recorded for task {}", task.id)
        } else {
            format!(
                "Notes for task {}\n{}\n\n{}",
                task.id,
                "=".repeat(task.id.len() + 15),
                task.notes
            )
        };
        let mut clipping = super::query::ReadClipping::default();
        let summary = if task.notes.is_empty() {
            full.clone()
        } else {
            format!(
                "Notes for task {}\n{}",
                task.id,
                super::query::task_show_notes(&task.notes, &task.id, &mut clipping)
            )
        };
        Ok(Self::success(super::query::render_task_read(
            &full, &summary, &task.id, "notes", clipping.clipped, &req.read,
        )?))
    }

    // gate-audit[supervisor_override]: bypass G-131 Explicitly accepts an oversized decision/discovery note after supervisor authorization.
    // gate-audit[override_accepted]: selector Derived oversized-note waiver already authorized under G-131; final note response carries its tag.
    pub async fn cas_task_notes(
        &self,
        Parameters(req): Parameters<TaskNotesRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        let task = task_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        if let Some(message) = super::caller_note_marker_rejection(&req.note) {
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(message),
                data: None,
            });
        }

        let supervisor_override = req.supervisor_override.unwrap_or(false);
        let registered_supervisor = if supervisor_override {
            self.resolve_live_supervisor_authority().map_err(|error| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "SUPERVISOR OVERRIDE REJECTED: supervisor_override=true requires a live registered supervisor ({error:?})."
                )),
                data: None,
            })?;
            true
        } else {
            false
        };
        let override_accepted = crate::mcp::tools::traffic_limits::validate_note_body(
            &req.note_type,
            &req.note,
            &self.load_config(),
            &req.id,
            supervisor_override,
            registered_supervisor,
            req.reason.as_deref(),
        )
        .map_err(|message| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(message),
            data: None,
        })?;

        let outcome_kind = req.note_type.to_ascii_lowercase();
        if matches!(outcome_kind.as_str(), "model_eval_target" | "model_eval_receipt") {
            if outcome_kind == "model_eval_target" {
                self.resolve_live_supervisor_authority().map_err(|error| Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!("MODEL EVAL TARGET REJECTED: only a live registered supervisor declares eval bars ({error:?})"),
                ))?;
            } else if self.resolve_live_supervisor_authority().is_err() {
                let agents = self.open_agent_store()?;
                let caller = agents.get(&self.get_agent_id()?)
                    .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;
                if !super::task_assignee_matches_agent(agents.as_ref(), task.assignee.as_deref(), &caller) {
                    return Err(Self::error(ErrorCode::INVALID_PARAMS,
                        "MODEL EVAL RECEIPT REJECTED: only the task assignee or a live registered supervisor may record model evaluation proof"));
                }
            }
            let verdict = crate::model_eval::record_note(&self.cas_root, task_store.as_ref(), &req.id, &outcome_kind, &req.note)
                .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;
            crate::mcp::tools::service::mutation_receipt::task_terminal_committed(&req.id);
            if let Err(error) = self.record_task_note_activity(&req.id, &outcome_kind, &req.note, None) {
                tracing::warn!(%error, "could not record model evaluation activity");
            }
            return Ok(Self::success(format!("Added {outcome_kind} note to task {}: {verdict}", req.id)));
        }
        if matches!(outcome_kind.as_str(), "outcome_target" | "outcome_receipt") {
            self.resolve_live_supervisor_authority().map_err(|error| Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("OUTCOME RECORD REJECTED: only a live registered supervisor records phase targets and measurements ({error:?})"),
            ))?;
            crate::phase_outcome::record_note(&self.cas_root, task_store.as_ref(), &req.id, &outcome_kind, &req.note)
                .map_err(|error| Self::error(ErrorCode::INVALID_PARAMS, error.to_string()))?;
            crate::mcp::tools::service::mutation_receipt::task_terminal_committed(&req.id);
            if let Err(error) = self.record_task_note_activity(&req.id, &outcome_kind, &req.note, None) {
                tracing::warn!(%error, "could not record phase outcome activity");
            }
            return Ok(Self::success(format!("Added {outcome_kind} note to task {}", req.id)));
        }

        let proof_note = matches!(
            req.note_type.to_ascii_lowercase().as_str(),
            "platform_proof" | "loaded_proof"
        );
        let config = self.load_config();
        let base = crate::config::resolved_factory_artifacts_root(
            config.factory().artifacts_root.as_deref(),
        );
        let prepared = cas_store::SqliteArtifactStore::open(&self.cas_root)
            .map_err(anyhow::Error::from)
            .and_then(|artifacts| {
                crate::artifacts::proof::prepare_note(
                    &artifacts, &req.id, &req.note, &base, proof_note,
                )
            });
        let (note, warnings) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                tracing::warn!(task_id = %req.id, error = %error, "artifact proof lookup unavailable; saving note without references");
                (
                    req.note.clone(),
                    vec![format!(
                        "artifact proof lookup unavailable; note saved without attached references: {error}"
                    )],
                )
            }
        };

        // Format the note with type prefix and timestamp
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
        let type_prefix = match req.note_type.to_lowercase().as_str() {
            "blocker" => "🚫 BLOCKER",
            "decision" => "✅ DECISION",
            "discovery" => "💡 DISCOVERY",
            "question" => "❓ QUESTION",
            "platform_proof" => "🧪 PLATFORM_PROOF",
            "loaded_proof" => "🧪 LOADED_PROOF",
            _ => "📝 PROGRESS",
        };

        let formatted_note = format!("[{}] {} {}", timestamp, type_prefix, note);

        // prepare_note adds only committed ledger records after the caller's
        // raw note has passed the marker rejection above.
        let append = if formatted_note
            .lines()
            .any(cas_store::task_note_markers::is_reserved_line)
        {
            task_store.append_trusted_note(&req.id, &formatted_note)
        } else {
            task_store.append_note(&req.id, &formatted_note)
        };
        append.map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to append task note: {e}")),
            data: None,
        })?;

        crate::mcp::tools::service::mutation_receipt::task_terminal_committed(&req.id);
        if let Err(e) = self.record_task_note_activity(
            &req.id,
            &req.note_type,
            &req.note,
            override_accepted.then_some(req.reason.as_deref().unwrap_or_default()),
        ) {
            tracing::warn!(
                task_id = %req.id,
                error = %e,
                "failed to record task note activity"
            );
        }

        let mut response = format!("Added {} note to task {}", req.note_type, req.id);
        for warning in warnings {
            response.push_str(&format!("\nWarning: {warning}"));
        }
        if override_accepted && req.note.chars().count() > crate::mcp::tools::traffic_limits::note_body_limit(&self.load_config()) {
            response = crate::gates::override_tag("G-131", response);
        }
        Ok(Self::success(response))
    }

    // gate-audit[supervisor_override_reason]: selector Records accepted note-waiver provenance; it does not admit another oversized note.
    fn record_task_note_activity(
        &self,
        task_id: &str,
        note_type: &str,
        note: &str,
        supervisor_override_reason: Option<&str>,
    ) -> anyhow::Result<()> {
        use cas_types::{Event, EventEntityType, EventType};

        let agent_id = self
            .get_agent_id()
            .map_err(|e| anyhow::anyhow!(e.message.to_string()))?;
        let event_store = crate::store::open_event_store(&self.cas_root)?;
        let summary = format!("Task note added ({note_type}): {}", truncate_str(note, 120));
        let mut metadata = match supervisor_override_reason {
            Some(reason) => serde_json::json!({
                "note_type": note_type,
                "supervisor_override": true,
                "supervisor_override_reason": reason,
            }),
            None => serde_json::json!({
                "note_type": note_type,
            }),
        };
        metadata["notification_ids"] = serde_json::json!(note_notification_ids(note));
        let event = Event::new(
            EventType::TaskNoteAdded,
            EventEntityType::Task,
            task_id,
            summary,
        )
        .with_session(agent_id)
        .with_metadata(metadata);

        let event_id = event_store.record(&event)?;
        if note_type.eq_ignore_ascii_case("decision")
            && let Ok(supervisor) = self.resolve_live_supervisor_authority()
        {
            super::block_hold::record_supervisor_decision(&self.cas_root, &supervisor, task_id, event_id)
                .map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }
}
