//! Operator recovery decisions for historical deliveries (GH #2270).
//!
//! A delivery already contained in its target can never gain a pre-merge
//! verdict. The operator authorizes closing a batch of them with one
//! `cas task recovery-decision --scope <epic-id> --note "..."`; each close
//! then cites the printed decision ID and its containment commit. The CLI
//! refuses an agent context, the PreToolUse hook refuses agent shells running
//! it (`operator_only_cas_invocation`), and the record lives under
//! `.cas/operator/`, which agents may not write. A task note never counts.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Decision file, relative to the Cassy root (operator-written only).
pub const DECISIONS_FILE: &str = "operator/recovery-decisions.toml";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Decisions {
    #[serde(default)]
    decisions: Vec<RecoveryDecision>,
}

/// One operator decision covering the deliveries of one epic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryDecision {
    pub id: String,
    pub scope: String,
    pub recorded_at: DateTime<Utc>,
    pub note: String,
}

fn path(cas_root: &Path) -> PathBuf {
    cas_root.join(DECISIONS_FILE)
}

fn load(cas_root: &Path) -> Result<Decisions> {
    let path = path(cas_root);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("invalid recovery decisions {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Decisions::default()),
        Err(error) => Err(error.into()),
    }
}

/// Authorize `task`'s close under decision `id`. An unreadable file grants
/// nothing; the decision must exist and its scope must be the task's epic.
pub fn authorize(
    cas_root: &Path,
    store: &dyn cas_store::TaskStore,
    task_id: &str,
    id: &str,
) -> std::result::Result<RecoveryDecision, String> {
    let decision = load(cas_root)
        .map_err(|error| format!("recovery decisions are unreadable: {error:#}"))?
        .decisions
        .into_iter()
        .find(|decision| decision.id == id.trim())
        .ok_or_else(|| format!("no operator recovery decision `{id}` is recorded on this machine"))?;
    let epic = store.get_parent_epic(task_id).ok().flatten().map(|epic| epic.id);
    if epic.as_deref() != Some(decision.scope.as_str()) {
        return Err(format!(
            "operator recovery decision `{}` covers {}, but task {task_id} belongs to {}",
            decision.id,
            decision.scope,
            epic.as_deref().unwrap_or("no epic")
        ));
    }
    Ok(decision)
}

/// Record the operator's decision for every delivery in epic `scope`.
/// The caller has established the operator.
pub fn record(
    cas_root: &Path,
    store: &dyn cas_store::TaskStore,
    scope: &str,
    note: &str,
    now: DateTime<Utc>,
) -> Result<RecoveryDecision> {
    let note = note.trim();
    ensure!(!note.is_empty(), "--note is required: say what you decided and why");
    let epic = store.get(scope.trim()).with_context(|| format!("epic {scope} not found"))?;
    ensure!(epic.task_type == cas_types::TaskType::Epic, "{} is not an epic", epic.id);
    let mut state = load(cas_root)?;
    let decision = RecoveryDecision {
        id: format!("ord-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
        scope: epic.id.clone(),
        recorded_at: now,
        note: note.to_string(),
    };
    state.decisions.push(decision.clone());
    let path = path(cas_root);
    let parent = path.parent().context("recovery decisions have no parent")?;
    std::fs::create_dir_all(parent)?;
    let text = format!(
        "# Operator recovery decisions (GH #2270). Written only by the operator's own\n\
         # `cas task recovery-decision`. Agents are refused when they try to write this file.\n{}",
        toml::to_string_pretty(&state)?
    );
    let staging = parent.join(format!(".recovery-decisions.{}.tmp", std::process::id()));
    std::fs::write(&staging, text)?;
    std::fs::rename(&staging, &path)?;
    store.append_note(
        &epic.id,
        &format!(
            "[{}] ✅ DECISION operator recovery decision {} for contained historical deliveries in {}: {note}",
            now.format("%Y-%m-%d %H:%M"),
            decision.id,
            epic.id
        ),
    )?;
    Ok(decision)
}
