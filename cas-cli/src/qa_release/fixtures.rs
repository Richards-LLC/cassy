//! Recipe lifecycle: cleanup on normal return/unwind, durable retry after a crash.
use super::{CallError, ReleaseConfig, Request, Verb, call};
use crate::qa_registry::FixtureRecipe;
use cas_store::FixtureCleanup;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("fixture recipe {recipe} refuses environment {env}")]
    WrongEnvironment { recipe: String, env: String },
    #[error("NOT_EXERCISED{{fixture_setup}}: {0}")]
    NotExercised(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixtureReceipt {
    pub cleanup_id: String,
    pub handle: String,
    pub outputs: serde_json::Map<String, Value>,
}

#[derive(Debug, Default, Serialize)]
pub struct CleanupReport {
    pub cleaned: usize,
    pub orphan_debt: Vec<OrphanDebt>,
}

/// Teardown debt is separate from a suite verdict and cannot change it.
#[derive(Debug, Serialize)]
pub struct OrphanDebt {
    pub cleanup_id: String,
    pub error: String,
}

/// Hold for the entire suite. `finish` reports debt; Drop also attempts cleanup
/// for early returns and panic unwinds. SIGKILL recovery is `sweep` after TTL.
pub struct FixtureSuite {
    project_root: PathBuf,
    cas_root: PathBuf,
    config: ReleaseConfig,
    env: String,
    sha: String,
    rows: Vec<FixtureCleanup>,
}

impl FixtureSuite {
    pub fn new(
        project_root: &Path,
        cas_root: &Path,
        config: &ReleaseConfig,
        env: &str,
        sha: &str,
    ) -> Self {
        Self {
            project_root: project_root.into(),
            cas_root: cas_root.into(),
            config: config.clone(),
            env: env.into(),
            sha: sha.into(),
            rows: Vec::new(),
        }
    }

    pub fn setup(&mut self, recipe: &FixtureRecipe) -> Result<FixtureReceipt, SetupError> {
        // This refusal must precede all adapter calls, including teardown.
        if !recipe.envs.contains(&self.env) {
            return Err(SetupError::WrongEnvironment {
                recipe: recipe.name.clone(),
                env: self.env.clone(),
            });
        }
        if !recipe.valid() {
            return Err(SetupError::NotExercised("invalid fixture recipe".into()));
        }
        // Open the journal before creating remote resources.
        cas_store::fixture_cleanups(&self.cas_root, None)
            .map_err(|e| SetupError::NotExercised(e.to_string()))?;
        let id = uuid::Uuid::new_v4().to_string();
        let data = call(
            &self.project_root,
            &self.config,
            Verb::FixtureSetup,
            Request {
                env: &self.env,
                sha: &self.sha,
                args: json!({"name": recipe.name,
                "provides": recipe.provides, "ttl_mins": recipe.ttl_mins, "cleanup_id": id}),
                ..Request::default()
            },
        )
        .map_err(|e| SetupError::NotExercised(e.to_string()))?;
        let handle = data
            .get("handle")
            .and_then(Value::as_str)
            .filter(|h| !h.trim().is_empty())
            .ok_or_else(|| SetupError::NotExercised("fixture-setup returned no handle".into()))?;
        let row = FixtureCleanup {
            id: id.clone(),
            handle: handle.into(),
            env: self.env.clone(),
            sha: self.sha.clone(),
            recipe: recipe.name.clone(),
            expires_at: Utc::now().timestamp() + i64::from(recipe.ttl_mins) * 60,
            orphan_error: None,
        };
        if let Err(error) = cas_store::record_fixture_cleanup(&self.cas_root, &row) {
            // If journaling fails, immediately undo setup before returning.
            let cleanup = teardown(&self.project_root, &self.config, &row).err();
            return Err(SetupError::NotExercised(format!(
                "cleanup journal: {error}; teardown: {cleanup:?}"
            )));
        }
        self.rows.push(row);
        let outputs = data
            .get("outputs")
            .and_then(Value::as_object)
            .filter(|o| {
                recipe
                    .provides
                    .iter()
                    .all(|name| o.get(name).is_some_and(|v| !v.is_null()))
            })
            .ok_or_else(|| {
                SetupError::NotExercised("fixture-setup omitted declared outputs".into())
            })?;
        Ok(FixtureReceipt {
            cleanup_id: id,
            handle: handle.into(),
            outputs: outputs.clone(),
        })
    }

    pub fn finish(mut self) -> CleanupReport {
        self.cleanup()
    }

    fn cleanup(&mut self) -> CleanupReport {
        cleanup_rows(
            &self.project_root,
            &self.cas_root,
            &self.config,
            self.rows.drain(..).rev(),
        )
    }
}

impl Drop for FixtureSuite {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

fn teardown(
    project_root: &Path,
    config: &ReleaseConfig,
    row: &FixtureCleanup,
) -> Result<(), CallError> {
    let data = call(
        project_root,
        config,
        Verb::FixtureTeardown,
        Request {
            env: &row.env,
            sha: &row.sha,
            args: json!({"name": row.recipe, "handle": row.handle, "cleanup_id": row.id}),
            ..Request::default()
        },
    )?;
    if data.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(CallError::Protocol(
            Verb::FixtureTeardown.as_str(),
            "teardown did not confirm ok".into(),
        ));
    }
    Ok(())
}

fn cleanup_rows(
    project: &Path,
    root: &Path,
    config: &ReleaseConfig,
    rows: impl IntoIterator<Item = FixtureCleanup>,
) -> CleanupReport {
    let mut report = CleanupReport::default();
    for row in rows {
        let error = teardown(project, config, &row).err().map(|e| e.to_string());
        let journal = cas_store::settle_fixture_cleanup(root, &row.id, error.as_deref());
        match (error, journal) {
            (None, Ok(())) => report.cleaned += 1,
            (error, journal) => report.orphan_debt.push(OrphanDebt {
                cleanup_id: row.id,
                error: error
                    .unwrap_or_else(|| format!("cleanup journal: {}", journal.unwrap_err())),
            }),
        }
    }
    report
}

/// Expired rows survive a process crash and are replayed through the adapter.
/// Adapters must make teardown idempotent (an absent handle is already clean).
pub fn sweep(
    project_root: &Path,
    cas_root: &Path,
    config: &ReleaseConfig,
) -> anyhow::Result<CleanupReport> {
    sweep_at(project_root, cas_root, config, Utc::now().timestamp())
}

pub(super) fn sweep_at(
    project_root: &Path,
    cas_root: &Path,
    config: &ReleaseConfig,
    now: i64,
) -> anyhow::Result<CleanupReport> {
    let rows = cas_store::fixture_cleanups(cas_root, Some(now))?;
    Ok(cleanup_rows(project_root, cas_root, config, rows))
}
