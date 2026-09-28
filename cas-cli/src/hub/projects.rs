//! Host project discovery and launch-target resolution for Commander.

use std::path::PathBuf;

use anyhow::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchTarget {
    Project { id: String },
    Browse { root_id: String, path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRoot {
    pub path: PathBuf,
}

pub fn resolve_launch_target(_target: &LaunchTarget) -> Result<LaunchRoot> {
    anyhow::bail!("launch target resolution is not implemented")
}
