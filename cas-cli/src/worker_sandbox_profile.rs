//! The Claude worker OS sandbox block for this project (cas-3751a, capability
//! guards stage A). The pure profile lives in `cas_mux::worker_sandbox`; this
//! module supplies the project's resolved roots and protection lists.

use std::path::Path;

use cas_mux::worker_sandbox::{ProfileRoots, claude_sandbox_block};
use cas_types::factory_claude::WorkerSandbox;

/// The worker `sandbox` settings block, or `None` when this host or the
/// project's `factory.claude.worker_sandbox` leaves workers unsandboxed. Only
/// macOS is measured (cas-ac77): sandbox-runtime does not start on the Linux
/// factory host yet.
pub(crate) fn worker_sandbox_block(cas_root: &Path) -> Option<serde_json::Value> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let config = crate::config::Config::load(cas_root).ok()?;
    let factory = config.factory();
    if factory.claude.worker_sandbox == WorkerSandbox::Off {
        return None;
    }
    let mut roots = vec![
        crate::config::resolved_factory_artifact_paths(cas_root, factory.artifacts_root.as_deref()).project_root,
    ];
    if let Some(scratch) = config.staging.as_ref().and_then(|staging| staging.scratch_root.as_deref()) {
        roots.push(scratch.into());
    }
    for root in crate::config::operator_policy::effective_write_roots(
        &factory.write_roots,
        &crate::config::operator_policy::user_write_roots(),
        cas_root,
    ) {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    let protection = config.hooks().pre_tool_use.protection;
    Some(claude_sandbox_block(&ProfileRoots {
        write_roots: &roots,
        cas_root,
        protected_files: &protection.files,
        protected_patterns: &protection.patterns,
    }))
}
