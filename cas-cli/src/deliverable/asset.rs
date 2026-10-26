//! Embedded assets, materialized into a private directory for one run.
//!
//! The binary holds the only copy a deliverable verb executes: installed
//! skill copies lag behind the binary, so they are never searched.

use std::fs;
use std::path::PathBuf;

use anyhow::Context;

use crate::builtins::BUILTIN_SKILLS;

/// One file to place at `path` inside a [`Workspace`].
#[derive(Clone, Copy, Debug)]
pub struct Asset {
    pub path: &'static str,
    pub content: &'static str,
}

impl Asset {
    /// A file from the builtin skill catalog (for example a skill's script),
    /// placed at `path`.
    pub fn builtin(catalog_path: &str, path: &'static str) -> anyhow::Result<Self> {
        BUILTIN_SKILLS
            .iter()
            .find(|file| file.path == catalog_path)
            .map(|file| Self {
                path,
                content: file.content,
            })
            .ok_or_else(|| anyhow::anyhow!("builtin deliverable asset missing: {catalog_path}"))
    }
}

/// A temporary directory holding materialized assets; removed on drop.
pub struct Workspace {
    dir: tempfile::TempDir,
}

impl Workspace {
    pub fn materialize(assets: &[Asset]) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir().context("could not create deliverable asset workspace")?;
        for asset in assets {
            let target = dir.path().join(asset.path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target, asset.content)
                .with_context(|| format!("could not write deliverable asset {}", asset.path))?;
        }
        Ok(Self { dir })
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }
}
