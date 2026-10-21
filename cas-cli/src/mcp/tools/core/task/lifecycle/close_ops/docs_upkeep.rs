//! Delivery-scoped documentation upkeep (G5/G6).
use super::*;
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(super) fn at_park(
    _store: &dyn cas_store::TaskStore, _task: &mut Task, _cas_root: &Path,
    _repo: &Path, _target: &str, _window: Option<&TaskCommitReceiptWindow>,
    _receipt: Option<&str>, _override_reason: Option<&str>,
) -> Result<Vec<String>, String> { Ok(vec![]) }

#[cfg(test)]
#[path = "docs_upkeep_tests.rs"]
mod tests;
