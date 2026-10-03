//! A commit observed by this request, never inferred from another call's row.
use std::future::Future;
use std::sync::{Arc, OnceLock};

#[derive(Clone)]
pub(crate) struct Commit {
    pub description: String,
    pub notification_id: Option<i64>,
}

pub(crate) struct Receipt {
    tool: String,
    action: String,
    task_id: Option<String>,
    pub commit: OnceLock<Commit>,
}

tokio::task_local! { static CURRENT: Arc<Receipt>; }

impl Receipt {
    pub fn new(tool: &str, action: &str, task_id: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            tool: tool.into(),
            action: action.into(),
            task_id: task_id.map(str::to_string),
            commit: OnceLock::new(),
        })
    }
}

pub(crate) fn current() -> Option<Arc<Receipt>> {
    CURRENT.try_with(Arc::clone).ok()
}

pub(crate) async fn scope<F: Future>(receipt: Arc<Receipt>, future: F) -> F::Output {
    CURRENT.scope(receipt, future).await
}

pub(crate) fn message_committed(id: i64) {
    if let Some(receipt) = current() {
        let _ = receipt.commit.set(Commit {
            description: format!("message enqueue committed; notification_id: {id}; query message_status before retrying"),
            notification_id: Some(id),
        });
    }
}

pub(crate) fn task_committed(id: &str) {
    if let Some(receipt) = current()
        && receipt.tool == "task"
        && receipt
            .task_id
            .as_deref()
            .is_none_or(|expected| expected == id)
    {
        let _ = receipt.commit.set(Commit {
            description: format!("task `{id}` {} write committed; post-commit work may be incomplete; re-query before retrying", receipt.action),
            notification_id: None,
        });
    }
}
