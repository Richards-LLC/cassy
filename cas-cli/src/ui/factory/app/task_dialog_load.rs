//! Background load for the task detail dialog (GH #1165).
//!
//! The dialog used to open the task store and read the task on every frame,
//! on the daemon thread that also forwards keystrokes. It now reads the task
//! once per open on a helper thread, and the renderer shows "Loading" until
//! the result arrives.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use cas_types::Task;

/// Where the dialog's task read stands.
#[derive(Default)]
pub enum TaskDialogLoad {
    #[default]
    Idle,
    Loading {
        task_id: String,
        result: Receiver<Result<Task, String>>,
    },
    Loaded(Box<Task>),
    Failed {
        task_id: String,
        error: String,
    },
}

impl TaskDialogLoad {
    /// Start reading `task_id` on a helper thread.
    pub fn start(cas_dir: PathBuf, task_id: String) -> Self {
        let (sender, result) = channel();
        let id = task_id.clone();
        let spawned = std::thread::Builder::new()
            .name("task-dialog-load".into())
            .spawn(move || {
                let loaded = crate::store::open_task_store_cached(&cas_dir)
                    .map_err(|error| format!("Failed to open task store: {error}"))
                    .and_then(|store| store.get(&id).map_err(|_| format!("Task {id} not found")));
                let _ = sender.send(loaded);
            });
        match spawned {
            Ok(_) => Self::Loading { task_id, result },
            Err(error) => Self::Failed {
                task_id,
                error: format!("Failed to start task load: {error}"),
            },
        }
    }

    /// True while the helper thread has not reported yet.
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading { .. })
    }

    /// Take a finished result, if any. Never blocks.
    pub fn poll(&mut self) {
        let Self::Loading { task_id, result } = self else {
            return;
        };
        let next = match result.try_recv() {
            Ok(Ok(task)) => Self::Loaded(Box::new(task)),
            Ok(Err(error)) => Self::Failed {
                task_id: task_id.clone(),
                error,
            },
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Self::Failed {
                task_id: task_id.clone(),
                error: "Task load stopped before it finished".into(),
            },
        };
        *self = next;
    }

    /// The loaded task when it is `task_id`.
    pub fn task_for(&self, task_id: &str) -> Option<&Task> {
        match self {
            Self::Loaded(task) if task.id == task_id => Some(task),
            _ => None,
        }
    }

    /// The failure message when the load of `task_id` failed.
    pub fn error_for(&self, task_id: &str) -> Option<&str> {
        match self {
            Self::Failed { task_id: id, error } if id == task_id => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_the_task_off_thread_and_reports_a_missing_one() {
        let dir = tempfile::tempdir().unwrap();
        let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
        crate::store::open_task_store_cached(&cas_dir)
            .unwrap()
            .add(&Task::new("cas-d1a1".into(), "dialog".into()))
            .unwrap();

        let wait = |mut load: TaskDialogLoad| {
            let started = std::time::Instant::now();
            while matches!(load, TaskDialogLoad::Loading { .. })
                && started.elapsed() < std::time::Duration::from_secs(5)
            {
                load.poll();
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            load
        };
        let loaded = wait(TaskDialogLoad::start(cas_dir.clone(), "cas-d1a1".into()));
        assert_eq!(loaded.task_for("cas-d1a1").unwrap().title, "dialog");
        assert!(loaded.task_for("cas-other").is_none());

        let missing = wait(TaskDialogLoad::start(cas_dir, "cas-none".into()));
        assert_eq!(
            missing.error_for("cas-none"),
            Some("Task cas-none not found")
        );
    }
}
