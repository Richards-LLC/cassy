//! Regression guard for cas-78c8 / GH #156 — tests must never reach a real
//! CAS store.
//!
//! The integration suite spent months writing its five fixture memories into
//! the developer's `~/.cas/cas.db` and the cas-src project database: 994 of
//! 1696 rows. Nothing failed, because nothing was watching. These tests watch.
//!
//! The mechanism under test is `CAS_TEST_PROTECTED_DBS`, honoured by
//! `cas_store::shared_db::shared_connection` — the single choke point every
//! production store open funnels through. What matters is that the tripwire
//! survives the trip through a *spawned* `cas` process, since that is the shape
//! the original leak took: the test itself was tidy, the subprocess it spawned
//! was not.

use std::process::Command;

mod support;
use support::CasSandbox;

/// Stand in for "the developer's real store": a second sandbox that this test
/// declares off-limits. Using a sandbox rather than the actual `~/.cas` means
/// the guard is exercised without the test needing to touch a real database —
/// which would be the exact sin it exists to detect.
fn protected_store() -> CasSandbox {
    CasSandbox::new()
}

fn run_status_against(sandbox: &CasSandbox, protected: &str) -> std::process::Output {
    let mut cmd: Command = sandbox.command();
    cmd.env(cas_store::shared_db::PROTECTED_DBS_ENV, protected);
    cmd.args(["status"]);
    cmd.output().expect("run cas status")
}

#[test]
fn spawned_cas_aborts_when_it_opens_a_protected_database() {
    let sandbox = protected_store();
    let protected = sandbox.cas_root().join("cas.db");

    let output = run_status_against(&sandbox, &protected.display().to_string());
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "cas opened a protected database and exited successfully; stderr: {stderr}"
    );
    assert!(
        stderr.contains("refusing to open protected database"),
        "expected the protected-database panic, got: {stderr}"
    );
}

#[test]
fn the_cas_directory_form_of_the_protected_list_also_fires() {
    let sandbox = protected_store();

    // Operators point the guard at `.cas` directories, not `cas.db` files —
    // `scripts/check-real-store-untouched.sh` accepts both, so both must work.
    let output = run_status_against(&sandbox, &sandbox.cas_root().display().to_string());
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("refusing to open protected database"),
        "the .cas-directory form of the protected list did not fire; stderr: {stderr}"
    );
}

#[test]
fn an_unrelated_protected_database_does_not_break_a_sandboxed_run() {
    let sandbox = protected_store();
    let elsewhere = CasSandbox::new();

    // The negative half of the guard: a tripwire that fails everything proves
    // nothing. `elsewhere` is a real, initialized store that this run simply
    // never touches, so the command must succeed.
    let output = run_status_against(
        &sandbox,
        &elsewhere.cas_root().join("cas.db").display().to_string(),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !stderr.contains("refusing to open protected database"),
        "the guard fired on a database this run never opened; stderr: {stderr}"
    );
    assert!(
        output.status.success(),
        "cas status failed for an unrelated reason; stderr: {stderr}"
    );
}

#[test]
fn the_sandbox_forwards_the_tripwire_through_its_cas_star_scrub() {
    // `CasSandbox::configure_command` removes every inherited `CAS_*` variable
    // before pinning its own, and `CAS_TEST_PROTECTED_DBS` is one of them. If
    // the scrub dropped it, `scripts/check-real-store-untouched.sh` would
    // export the tripwire, every sandboxed subprocess would run unguarded, and
    // the harness would report a clean run no matter what happened.
    //
    // Asserted on `get_envs` rather than by mutating process-global
    // environment, which would race the other tests in this binary.
    let sandbox = CasSandbox::new();
    let protected = "/nowhere/real/cas.db";

    let mut cmd = Command::new("true");
    cmd.env(cas_store::shared_db::PROTECTED_DBS_ENV, protected);
    sandbox.configure_command(&mut cmd);

    let forwarded = cmd.get_envs().find_map(|(key, value)| {
        (key == cas_store::shared_db::PROTECTED_DBS_ENV)
            .then_some(value)
            .flatten()
    });

    assert_eq!(
        forwarded.map(|v| v.to_string_lossy().to_string()),
        Some(protected.to_string()),
        "the sandbox dropped {} during its CAS_* scrub",
        cas_store::shared_db::PROTECTED_DBS_ENV
    );
}
