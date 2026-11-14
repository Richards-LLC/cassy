//! End-to-end regression test for cas-3b51 (EPIC cas-c351).
//!
//! Verifies that the A2 panic catcher applied to every `#[tool]` method
//! on [`super::CasService`] is wired through the real dispatch path, not
//! just the helper module's unit tests. Hits `CasService::system` with
//! a `#[cfg(test)]`-guarded action that forces a `panic!(...)` and
//! asserts the call surfaces as `INTERNAL_ERROR`, the service keeps
//! serving a second call, and the pattern holds across 10 consecutive
//! panics.
//!
//! Lives under `cas-cli/src/mcp/tools/service/` rather than
//! `cas-cli/tests/` because integration tests strip `#[cfg(test)]` from
//! the lib, leaving the injection branch in `system()` unreachable. A
//! unit test inside the crate sees `#[cfg(test)]` active.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::ErrorCode;
use serde_json::json;
use tempfile::TempDir;

use super::{CasService, SystemRequest};
use crate::mcp::server::CasCore;

/// Construct a fresh `CasService` over a temporary DB. The `TempDir` is
/// returned so the caller keeps it alive for the duration of the test;
/// dropping it would delete the backing SQLite file mid-test. Bind the
/// first tuple element to `_dir` (not bare `_`) to hold the guard.
fn make_service() -> (TempDir, CasService) {
    let dir = TempDir::new().expect("tempdir");
    let core = CasCore::with_daemon(dir.path().to_path_buf(), None, None);
    #[cfg(feature = "mcp-proxy")]
    let svc = CasService::new(core, None);
    #[cfg(not(feature = "mcp-proxy"))]
    let svc = CasService::new(core);
    (dir, svc)
}

fn panic_req() -> SystemRequest {
    serde_json::from_value(json!({ "action": "__panic_for_test__" }))
        .expect("static JSON must deserialize")
}

fn version_req() -> SystemRequest {
    serde_json::from_value(json!({ "action": "version" }))
        .expect("static JSON must deserialize")
}

/// The exact payload string the `#[cfg(test)]` injection in `system()`
/// passes to `panic!(...)`. Kept next to the assertions so the two
/// stay locked together — if mod.rs changes the payload wording, the
/// compile-time string match below forces this file to be updated.
const INJECTED_PANIC_PAYLOAD: &str =
    "forced test panic from system handler (cas-3b51 regression)";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panic_in_system_dispatch_returns_internal_error_and_keeps_server_alive() {
    let (_dir, svc) = make_service();

    // Forced panic inside system()'s handler body must be converted by
    // dispatch_with_catch into a structured McpError, not propagated to
    // the caller's task.
    let err = svc
        .system(Parameters(panic_req()))
        .await
        .expect_err("forced panic must surface as Err, not unwind to caller");
    assert_eq!(
        err.code,
        ErrorCode::INTERNAL_ERROR,
        "panic must be reported as INTERNAL_ERROR, got {:?}: {}",
        err.code,
        err.message
    );

    // Assert the payload text is forwarded verbatim — a regression that
    // dropped or truncated the panic message would still satisfy the
    // INTERNAL_ERROR + tool-label checks above, so this is the actual
    // discriminating assertion.
    assert!(
        err.message.contains(INJECTED_PANIC_PAYLOAD),
        "panic payload must reach the client unchanged; got: {}",
        err.message
    );

    // Same CasService instance must still be usable after a panicked
    // handler — that is the whole point of A2. Content check protects
    // against a degraded-but-Ok response (empty body, is_error set).
    let ok = svc
        .system(Parameters(version_req()))
        .await
        .expect("system('version') must succeed after a panicked dispatch");
    assert!(
        !ok.content.is_empty(),
        "version response must have content; degraded Ok would otherwise pass silently"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ten_consecutive_dispatch_panics_do_not_kill_service() {
    let (_dir, svc) = make_service();

    for i in 0..10 {
        let err = svc
            .system(Parameters(panic_req()))
            .await
            .expect_err(&format!("iteration {i} unexpectedly succeeded"));
        assert_eq!(
            err.code,
            ErrorCode::INTERNAL_ERROR,
            "iteration {i} returned wrong code: {:?}",
            err.code
        );

        assert!(
            err.message.contains(INJECTED_PANIC_PAYLOAD),
            "iteration {i} lost the panic payload: {}",
            err.message
        );
    }

    // Service survives the sustained panic storm and still serves a
    // real action.
    let ok = svc
        .system(Parameters(version_req()))
        .await
        .expect("service must still serve version after 10 consecutive panics");
    assert!(
        !ok.content.is_empty(),
        "post-storm version response must have content"
    );
}
