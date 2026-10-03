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
    /// cas-b1dd: when the request's own terminal mutation committed (a
    /// task's Closed write, a note append). An intermediate write (a gate
    /// note, a parked anchor) sets `commit` but never this, so an early
    /// answer can only ever describe the mutation the caller asked for.
    pub terminal: OnceLock<std::time::Instant>,
}

tokio::task_local! { static CURRENT: Arc<Receipt>; }

impl Receipt {
    pub fn new(tool: &str, action: &str, task_id: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            tool: tool.into(),
            action: action.into(),
            task_id: task_id.map(str::to_string),
            commit: OnceLock::new(),
            terminal: OnceLock::new(),
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

/// cas-b1dd: the request's terminal task mutation (close's Closed write, a
/// note append) committed. Marks the commit too, so a deadline after it
/// reports COMMITTED, and lets the dispatch answer once post-commit work
/// overruns its grace instead of holding the response hostage.
pub(crate) fn task_terminal_committed(id: &str) {
    task_committed(id);
    if let Some(receipt) = current()
        && receipt.tool == "task"
        && receipt
            .task_id
            .as_deref()
            .is_none_or(|expected| expected == id)
    {
        let _ = receipt.terminal.set(std::time::Instant::now());
    }
}

impl Receipt {
    pub(crate) fn action(&self) -> &str {
        &self.action
    }

    pub(crate) fn task_id(&self) -> Option<&str> {
        self.task_id.as_deref()
    }
}
