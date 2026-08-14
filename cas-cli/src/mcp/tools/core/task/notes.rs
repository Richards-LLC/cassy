use crate::mcp::tools::core::imports::*;

impl CasCore {
    pub async fn cas_task_notes_read(
        &self,
        Parameters(req): Parameters<IdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;
        let task = task_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        if task.notes.is_empty() {
            return Ok(Self::success(format!(
                "No notes recorded for task {}",
                task.id
            )));
        }

        Ok(Self::success(format!(
            "Notes for task {}\n{}\n\n{}",
            task.id,
            "=".repeat(task.id.len() + 15),
            task.notes
        )))
    }

    pub async fn cas_task_notes(
        &self,
        Parameters(req): Parameters<TaskNotesRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        let mut task = task_store.get(&req.id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("Task not found: {e}")),
            data: None,
        })?;

        // Format the note with type prefix and timestamp
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
        let type_prefix = match req.note_type.to_lowercase().as_str() {
            "blocker" => "🚫 BLOCKER",
            "decision" => "✅ DECISION",
            "discovery" => "💡 DISCOVERY",
            "question" => "❓ QUESTION",
            _ => "📝 PROGRESS",
        };

        let formatted_note = format!("[{}] {} {}", timestamp, type_prefix, req.note);

        // Append to existing notes
        if task.notes.is_empty() {
            task.notes = formatted_note;
        } else {
            task.notes = format!("{}\n\n{}", task.notes, formatted_note);
        }

        task.updated_at = chrono::Utc::now();

        task_store.update(&task).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to update task: {e}")),
            data: None,
        })?;

        if let Err(e) = self.record_task_note_activity(&req.id, &req.note_type, &req.note) {
            tracing::warn!(
                task_id = %req.id,
                error = %e,
                "failed to record task note activity"
            );
        }

        Ok(Self::success(format!(
            "Added {} note to task {}",
            req.note_type, req.id
        )))
    }

    fn record_task_note_activity(
        &self,
        task_id: &str,
        note_type: &str,
        note: &str,
    ) -> anyhow::Result<()> {
        use cas_types::{Event, EventEntityType, EventType};

        let agent_id = self
            .get_agent_id()
            .map_err(|e| anyhow::anyhow!(e.message.to_string()))?;
        let event_store = crate::store::open_event_store(&self.cas_root)?;
        let summary = format!(
            "Task note added ({note_type}): {}",
            truncate_str(note, 120)
        );
        let event = Event::new(
            EventType::TaskNoteAdded,
            EventEntityType::Task,
            task_id,
            summary,
        )
        .with_session(agent_id)
        .with_metadata(serde_json::json!({
            "note_type": note_type,
        }));

        event_store.record(&event)?;
        Ok(())
    }
}
