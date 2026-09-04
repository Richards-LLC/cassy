//! Regression guard for cas-c0411 — the `cas init` watchdog budget must be
//! reachable by a batch runner's whole process tree.
//!
//! `cas init` aborts itself after a wall-clock budget so a hang cannot squat a
//! CPU core (cas-bf06). That budget is an assumption about the host, and the
//! v3.15.1 release gate broke it: with three isolation re-runs compiling and six
//! idle `cas serve` daemons resident, a test's child `cas init` reached the
//! 300 s default and aborted, failing the gate's archive-mode row on timing
//! alone. The gate now raises `CAS_INIT_TIMEOUT_SECS` for its children instead
//! of disabling their watchdog.
//!
//! Almost every `cas init` in this suite is spawned through `CasSandbox`, whose
//! `configure_command` deliberately removes every inherited `CAS_*` variable.
//! That scrub is what would silently swallow the raised budget, so the
//! forwarding is asserted here rather than assumed.
//!
//! The resolution matrix itself (default, opt-out, raise, lower, garbage) is
//! unit-tested next to the knobs in `cas::cli::init`. There is deliberately no
//! end-to-end "watchdog fires" test: proving a real abort means racing a sub-
//! second init against the shortest budget that can be set, which is a flake
//! generator, and the abort path is one `thread::sleep` past the value asserted
//! here.

use std::process::Command;

mod support;
use support::{CasSandbox, INIT_TIMEOUT_SECS_ENV};

#[test]
fn the_sandbox_forwards_the_init_watchdog_budget_through_its_cas_star_scrub() {
    // Asserted on `get_envs` rather than by mutating process-global
    // environment, which would race the other tests in this binary.
    let sandbox = CasSandbox::new();

    let mut cmd = Command::new("true");
    cmd.env(INIT_TIMEOUT_SECS_ENV, "900");
    sandbox.configure_command(&mut cmd);

    let forwarded = cmd
        .get_envs()
        .find_map(|(key, value)| (key == INIT_TIMEOUT_SECS_ENV).then_some(value).flatten());

    assert_eq!(
        forwarded.map(|value| value.to_string_lossy().to_string()),
        Some("900".to_string()),
        "the sandbox dropped {INIT_TIMEOUT_SECS_ENV} during its CAS_* scrub, so a batch \
         runner cannot raise the budget for the `cas init` it spawns"
    );
}

#[test]
fn forwarding_the_budget_does_not_loosen_any_store_or_root_pinning() {
    // The carve-out must stay a timing knob. If it ever grew into a second way
    // to point a sandboxed child at a different store, this is where that shows.
    let sandbox = CasSandbox::new();

    let mut cmd = Command::new("true");
    cmd.env(INIT_TIMEOUT_SECS_ENV, "900");
    cmd.env("CAS_ROOT", "/nowhere/real");
    cmd.env("CAS_DIR", "/nowhere/real");
    sandbox.configure_command(&mut cmd);

    support::assert_command_is_sandboxed(&cmd, &sandbox);
}
