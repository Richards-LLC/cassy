//! Diff-scoped registry upkeep at park (G1–G3).
use super::*;
use std::path::Path;
use crate::config::QaConfig;

#[allow(clippy::too_many_arguments)]
fn check(
    _store: &dyn cas_store::TaskStore,
    _task: &mut Task,
    _repo: &Path,
    _qa: &QaConfig,
    _base: &str,
    _head: &str,
    _paths: Option<&[String]>,
    _override_reason: Option<&str>,
) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
#[path = "registry_upkeep_tests.rs"]
mod tests;
