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

/// cas-3b81 (GH #1142): run a handler whose body is synchronous Git and
/// SQLite work on the blocking pool, keeping this request's receipt in scope.
///
/// `task close` never yields on its ordinary path: run inline, it pinned a
/// runtime worker for its whole run (hundreds of Git spawns). Parallel closes
/// then queued behind each other and behind every other tool call while the
/// 55s budget ran, and the deadline could not cancel them anyway. On the
/// blocking pool, closes run side by side and the runtime stays free to answer
/// other calls and fire deadlines on time.
pub(crate) async fn run_on_blocking_pool<F, T>(future: F) -> Result<T, tokio::task::JoinError>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let receipt = current();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || match receipt {
        Some(receipt) => runtime.block_on(CURRENT.scope(receipt, future)),
        None => runtime.block_on(future),
    })
    .await
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// cas-3b81 (GH #1142): with one runtime worker, two synchronous
    /// close-shaped handlers ran one after the other and a timer queued behind
    /// both. On the blocking pool they overlap, the runtime stays free, and
    /// the request's receipt still observes the handler's commit.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn blocking_pool_handlers_overlap_and_keep_their_receipt_cas_3b81() {
        let started = Instant::now();
        let slow = |id: &'static str| {
            let receipt = Receipt::new("task", "close", Some(id));
            let observed = receipt.clone();
            let handle = tokio::spawn(scope(receipt, async move {
                run_on_blocking_pool(async move {
                    std::thread::sleep(Duration::from_millis(400));
                    task_terminal_committed(id);
                })
                .await
                .unwrap();
            }));
            (handle, observed)
        };
        let (first, first_receipt) = slow("cas-a");
        let (second, second_receipt) = slow("cas-b");
        let tick = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            started.elapsed()
        });
        let tick = tick.await.unwrap();
        first.await.unwrap();
        second.await.unwrap();
        let total = started.elapsed();
        assert!(
            tick < Duration::from_millis(300),
            "runtime pinned: timer fired after {tick:?}"
        );
        assert!(
            total < Duration::from_millis(750),
            "handlers serialized: {total:?}"
        );
        assert!(first_receipt.terminal.get().is_some());
        assert!(second_receipt.terminal.get().is_some());
        assert!(
            first_receipt
                .commit
                .get()
                .is_some_and(|commit| commit.description.contains("cas-a"))
        );
    }
}
