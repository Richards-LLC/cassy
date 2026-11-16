//! A slow merge keeps its execution and its outcome after the MCP reply.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rmcp::model::{CallToolResult, Content};
use serde_json::{Value, json};

use super::{CasService, mutation_receipt::Receipt};

type ToolResult = Result<CallToolResult, rmcp::ErrorData>;
type Joined = Result<ToolResult, tokio::task::JoinError>;

#[derive(Clone)]
pub(super) struct MergeReceipt {
    id: String,
    path: PathBuf,
    task: Option<String>,
    source: Option<String>,
}

impl MergeReceipt {
    pub(super) fn begin(root: &Path, arguments: Option<&serde_json::Map<String, Value>>) -> std::io::Result<Self> {
        let id = uuid::Uuid::new_v4().to_string();
        let get = |key| arguments.and_then(|args| args.get(key)).and_then(Value::as_str).map(str::to_string);
        let receipt = Self { path: root.join("merge-receipts").join(format!("{id}.json")), id,
            task: get("task_id"), source: get("id") };
        receipt.save(json!({"state":"preparing", "mutation_outcome":"IN_PROGRESS"}))?;
        Ok(receipt)
    }

    fn save(&self, mut value: Value) -> std::io::Result<()> {
        value["receipt_id"] = self.id.clone().into();
        value["task_id"] = json!(self.task);
        value["source"] = json!(self.source);
        value["process_id"] = std::process::id().into();
        let directory = self.path.parent().expect("receipt directory");
        std::fs::create_dir_all(directory)?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(&serde_json::to_vec_pretty(&value)?)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        Ok(())
    }

    pub(super) fn finish(&self, joined: &Joined, receipt: &Receipt, task_status: Option<&str>) -> std::io::Result<()> {
        let result = match joined {
            Ok(Ok(result)) => json!({"ok":result}),
            Ok(Err(error)) => json!({"error":error}),
            Err(error) => json!({"handler_error":error.to_string()}),
        };
        self.save(json!({"state":"completed", "result":result, "task_status":task_status,
            "mutation_outcome":if receipt.commit.get().is_some() { "COMMITTED" } else { "NOT_COMMITTED" },
            "commit":receipt.commit.get().map(|commit| &commit.description),
            "merge_close_outcome":receipt.followup.lock().ok().and_then(|followup| followup.clone())}))
    }

    pub(super) fn answer(&self, receipt: &Receipt) -> CallToolResult {
        let committed = receipt.commit.get();
        let outcome = if committed.is_some() { "COMMITTED" } else { "IN_PROGRESS" };
        let state = if committed.is_some() { "merged" } else { "preparing" };
        let mut answer = CallToolResult::success(vec![Content::text(format!(
            "{outcome}: merge {state}. {}Receipt: {}. Work continues in the background; a supervisor notice reports the outcome. Do not retry.",
            committed.map(|commit| format!("{}; ", commit.description)).unwrap_or_default(), self.path.display()))]);
        answer.structured_content = Some(json!({"mutation_outcome":outcome, "state":state,
            "receipt_id":self.id, "receipt_path":self.path, "task_id":self.task}));
        answer
    }

    pub(super) fn watch(self, service: CasService, handle: tokio::task::JoinHandle<ToolResult>, receipt: Arc<Receipt>) {
        // Backend replacement waits for both the execution and its durable notice.
        let work = service.tool_work.permit();
        tokio::spawn(async move {
            let _work = work;
            let joined = handle.await;
            let summary = super::server_handler::late_close_summary(&joined);
            let recorded = tokio::task::spawn_blocking(move || -> Result<(), String> {
                let status = self.task.as_deref().and_then(|task| {
                    service.inner.open_task_store().ok()?.get(task).ok()
                }).map(|task| task.status.to_string());
                self.finish(&joined, &receipt, status.as_deref()).map_err(|error| error.to_string())?;
                let close = receipt.followup.lock().ok().and_then(|followup| followup.clone());
                let failed = !matches!(&joined, Ok(Ok(result)) if result.is_error != Some(true));
                let outcome = match close {
                    Some(close) if failed => format!("{close}. Merge failure: {summary}"),
                    Some(close) => close,
                    None => summary,
                };
                let detail = format!("Merge outcome for {} (receipt {}); task status now `{}`. Close outcome: {outcome}. Full result: {}",
                    self.source.as_deref().unwrap_or("<unknown>"), self.id,
                    status.as_deref().unwrap_or("unknown"), self.path.display());
                if let Some(task) = &self.task {
                    let noted = (|| -> Result<(), String> {
                        let store = service.inner.open_task_store().map_err(|error| error.to_string())?;
                        // qa_evidence::is_system_outcome_note consumes this prefix.
                        store.append_note(task, &format!("[{}] 📝 PROGRESS POST_COMMIT_OUTCOME: {detail}", chrono::Utc::now().format("%Y-%m-%d %H:%M")))
                            .map(|_| ()).map_err(|error| error.to_string())
                    })();
                    if let Err(error) = noted {
                        tracing::warn!(%task, %error, "Merge receipt saved; task note unavailable");
                    }
                }
                let agents = service.inner.open_agent_store().map_err(|error| error.to_string())?
                    .list(None).map_err(|error| error.to_string())?;
                let owner = service.inner.get_registered_agent_id_read_only().ok();
                let recipient = crate::supervisor_notices::resolve(&agents, owner.as_deref(), None);
                crate::supervisor_notices::enqueue_with_context(&service.inner.cas_root, &recipient,
                    "merge_outcome", &detail, &self.id, Some(json!({"receipt_id":self.id, "receipt_path":self.path,
                        "task_status":status, "merge_close_outcome":receipt.followup.lock().ok().and_then(|followup| followup.clone())})))
                    .map_err(|error| error.to_string())
            }).await;
            if !matches!(&recorded, Ok(Ok(()))) {
                tracing::warn!(?recorded, "MCP merge outcome could not be recorded");
            }
        });
    }
}
