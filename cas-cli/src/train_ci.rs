//! Legacy artifact path shared with cut-state migration. Train CI is retired.
use std::path::{Path, PathBuf};
use anyhow::Result;

pub(crate) fn directory(root: &Path, train: &str) -> Result<PathBuf> {
    let factory = crate::config::Config::load(root)?.factory();
    Ok(
        crate::config::resolved_factory_artifact_paths(root, factory.artifacts_root.as_deref())
            .task_dirs(train)[0]
            .join("train-ci"),
    )
}
