//! Integration smoke test: Claude supervisor spawns Codex workers.
//!
//! Covers two spawn paths end-to-end (no real PTYs, config-only):
//!
//! **Path A — startup via `factory_pane_configs`:**
//! `MuxConfig` with `resolved_worker_specs` routes alice to Codex while the
//! supervisor stays Claude, even though `worker_cli` defaults to Claude.
//!
//! **Path B — dynamic spawn via `build_add_worker_config`:**
//! Simulates what the daemon does when it receives a
//! `ClientMessage::SpawnWorkers { specs: [Some(codex_spec)] }` message.
//! The daemon extracts the spec and calls `mux.add_worker(..., spec)`.
//! Here we call `build_add_worker_config` (the config-only twin) directly,
//! verifying that an explicit Codex spec wins over the Claude session default.
//!
//! See cas-5570 and EPIC cas-b3db for context.

use cas_mux::{Mux, MuxConfig, WorkerSpec};
use cas_mux::SupervisorCli;
use std::path::PathBuf;

// ── Path A: startup via factory_pane_configs ─────────────────────────────────

/// Claude supervisor + resolved_worker_specs routes alice to Codex at startup.
///
/// `MuxConfig.worker_cli` is `Claude` (the session default). alice's entry in
/// `resolved_worker_specs` overrides that to `Codex`. The supervisor must still
/// use `claude`.
#[test]
fn heterogeneous_spawn_factory_pane_configs() {
    let config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        worker_names: vec!["alice".to_string()],
        include_director: false,
        supervisor_cli: SupervisorCli::Claude,
        worker_cli: SupervisorCli::Claude, // session default — overridden by spec
        resolved_worker_specs: vec![WorkerSpec::codex_default("alice")],
        ..MuxConfig::default()
    };

    let configs = Mux::factory_pane_configs(&config);

    let (_, alice_pty) = configs
        .iter()
        .find(|(n, _)| n == "alice")
        .expect("alice must be present in factory_pane_configs output");

    let (_, sup_pty) = configs
        .iter()
        .find(|(n, _)| n == &config.supervisor_name)
        .expect("supervisor must be present in factory_pane_configs output");

    assert_eq!(
        alice_pty.effective_command(),
        "codex",
        "alice with Codex spec must use the codex binary even when worker_cli defaults to Claude"
    );
    assert_eq!(
        sup_pty.effective_command(),
        "claude",
        "supervisor must keep its claude binary in a heterogeneous session"
    );
}

// ── Path B: dynamic spawn via build_add_worker_config ────────────────────────

/// Explicit Codex spec passed at dynamic spawn time overrides the Claude session
/// default, mirroring what the daemon does after receiving
/// `ClientMessage::SpawnWorkers { specs: [Some(codex_spec)] }`.
///
/// The daemon extracts `spec = specs.get(i).cloned().flatten()` and passes it
/// to `mux.add_worker(name, cwd, ..., spec)`. Here we call the config-only
/// twin `build_add_worker_config` to verify the same resolution without a real
/// PTY.
#[test]
fn heterogeneous_spawn_dynamic_add_worker() {
    // Session default is Claude (Mux::new gives builtin_default = Claude).
    let mux = Mux::new(24, 80);

    // Spec extracted from a hypothetical SpawnWorkers { specs: [Some(codex_spec)] }
    let codex_spec = WorkerSpec::codex_default("bob");

    let pty = mux.build_add_worker_config(
        "bob",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        Some(codex_spec),
    );

    assert_eq!(
        pty.effective_command(),
        "codex",
        "explicit Codex spec at dynamic spawn time must override the Claude session default"
    );

    // Sanity-check the fallback: same Mux, no spec → Claude.
    let default_pty = mux.build_add_worker_config(
        "carol",
        PathBuf::from("/tmp/test"),
        None,
        "supervisor",
        None,
        None, // no override → session default (Claude)
    );
    assert_eq!(
        default_pty.effective_command(),
        "claude",
        "no explicit spec must fall back to the Claude session default"
    );
}
