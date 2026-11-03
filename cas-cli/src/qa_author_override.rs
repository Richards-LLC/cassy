//! Operator-only override of the independent QA author rule (cas-26ac).
//!
//! A batch receipt or passed QA round whose author implemented the task is not
//! independent QA. Only the operator can accept one anyway, with
//! `cas qa author-override <task> --note "..."`. The CLI refuses an agent
//! context, the PreToolUse hook refuses agent shells running it
//! (`operator_only_cas_invocation`), and the record lives under
//! `.cas/operator/`, which agents may not write. A task note never counts: the
//! delivering worker can write notes on its own task.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Override file, relative to the Cassy root (operator-written only).
pub const OVERRIDE_FILE: &str = "operator/qa-author-overrides.toml";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Overrides {
    #[serde(default)]
    overrides: Vec<Override>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Override {
    task: String,
    recorded_at: DateTime<Utc>,
    note: String,
}

fn path(cas_root: &Path) -> PathBuf {
    cas_root.join(OVERRIDE_FILE)
}

fn load(cas_root: &Path) -> Result<Overrides> {
    let path = path(cas_root);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("invalid QA author overrides {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Overrides::default()),
        Err(error) => Err(error.into()),
    }
}

/// The operator's recorded words accepting a lineage author for `task`.
/// An unreadable file grants nothing.
pub fn for_task(cas_root: &Path, task: &str) -> Option<String> {
    load(cas_root)
        .ok()?
        .overrides
        .into_iter()
        .rev()
        .find(|entry| entry.task == task)
        .map(|entry| entry.note)
}

/// Record the operator's override. The caller has established the operator.
pub fn record(
    cas_root: &Path,
    store: &dyn cas_store::TaskStore,
    task_id: &str,
    note: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    let note = note.trim();
    ensure!(
        !note.is_empty(),
        "--note is required: say why this author's QA is accepted"
    );
    let task = store
        .get(task_id)
        .with_context(|| format!("task {task_id} not found"))?;
    let mut state = load(cas_root)?;
    state.overrides.retain(|entry| entry.task != task.id);
    state.overrides.push(Override {
        task: task.id.clone(),
        recorded_at: now,
        note: note.to_string(),
    });
    let path = path(cas_root);
    let parent = path
        .parent()
        .context("QA author overrides have no parent")?;
    std::fs::create_dir_all(parent)?;
    let text = format!(
        "# QA author overrides (cas-26ac). Written only by the operator's own\n\
         # `cas qa author-override`. Agents are refused when they try to write this file.\n{}",
        toml::to_string_pretty(&state)?
    );
    let staging = parent.join(format!(".qa-author-overrides.{}.tmp", std::process::id()));
    std::fs::write(&staging, text)?;
    std::fs::rename(&staging, &path)?;
    store.append_note(
        &task.id,
        &format!(
            "[{}] ✅ DECISION operator QA author override (cas-26ac): {note}",
            now.format("%Y-%m-%d %H:%M")
        ),
    )?;
    Ok(())
}
