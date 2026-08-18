//! Panic catcher for MCP tool dispatch.
//!
//! Every `#[tool]` method on [`super::CasService`] delegates its real work
//! through [`dispatch_with_catch`], which runs the future on a dedicated
//! tokio task and converts any panic into a structured [`McpError`]. Without
//! this, a panic inside a handler kills the MCP stdio worker — the client
//! sees a generic "connection closed", the panic trace is lost, and the
//! factory supervisor has to respawn `cas serve` to keep working.
//!
//! We intentionally use `tokio::spawn(...).await` + `JoinError::is_panic()`
//! rather than `std::panic::catch_unwind`:
//!
//!   * `catch_unwind` does not cross `.await` points. A panic that
//!     originates inside an awaited future cannot be caught that way.
//!   * A spawned task that panics unwinds only its own stack. State held
//!     by the caller (the MCP dispatcher, store handles, mutex guards on
//!     `&self`) is not visible to the panicking task.
//!
//! `cas serve` startup installs a process-wide panic hook that writes the
//! panic location + backtrace to `{cas_root}/logs/cas-serve-{date}.log`
//! (see `crate::mcp::server::runtime::install_serve_panic_hook`). That
//! hook runs on the panicking worker thread **before** the `JoinError`
//! surfaces here, so by the time we synthesize the client-facing error
//! the forensic trail is already on disk.
//!
//! This mechanism depends on stack unwinding. A compile-time guard in
//! `cas-cli/src/lib.rs` (`#[cfg(all(not(test), panic = "abort"))]
//! compile_error!`) refuses non-test builds with `panic = "abort"`, so
//! the A2 invariant cannot be silently defeated by a future build
//! profile.

use std::any::Any;
use std::borrow::Cow;
use std::future::Future;

use rmcp::ErrorData as McpError;
use rmcp::model::{CallToolResult, ErrorCode};
use tracing::Instrument;

/// Run `fut` on a dedicated tokio task. On success, return the handler's
/// own result; on panic, return a structured `INTERNAL_ERROR` so the MCP
/// client sees a tool-error response instead of a dropped connection.
///
/// `tool_name` is the stable short label used in the client-facing error
/// message and in the server-side stderr log line. Downstream telemetry
/// treats it as a category, so do not interpolate dynamic data.
pub(super) async fn dispatch_with_catch<F>(
    tool_name: &'static str,
    fut: F,
) -> Result<CallToolResult, McpError>
where
    F: Future<Output = Result<CallToolResult, McpError>> + Send + 'static,
{
    // `.instrument(current())` forwards the MCP request id + tool name
    // span from the caller into the spawned task — tokio::spawn does
    // not propagate tracing context automatically.
    let handle = tokio::spawn(fut.instrument(tracing::Span::current()));
    // Dropping a JoinHandle does NOT abort the task. This guard makes
    // caller-side cancellation (future drop, server_handler's 55s
    // timeout) cascade into the spawned task. No-op if the handler
    // completes first.
    let _abort_guard = AbortOnDrop(handle.abort_handle());

    match handle.await {
        Ok(inner) => inner,
        Err(join_err) => {
            if join_err.is_panic() {
                let msg = panic_message(join_err.into_panic());
                // Mirror a single stderr line so operators tailing the MCP
                // server see the panic without having to open the log file.
                // The full backtrace is already in cas-serve-*.log via the
                // serve startup panic hook.
                eprintln!("[Cassy] tool '{tool_name}' handler panicked: {msg}");
                Err(McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::Owned(format!(
                        "handler panicked in '{tool_name}': {msg}. \
                         See .cas/logs/cas-serve-*.log for backtrace."
                    )),
                    data: None,
                })
            } else {
                // Cancellation. Fires during runtime shutdown or when the
                // abort guard above triggers after our own future was
                // dropped — in the latter case nobody is awaiting this
                // branch, so the return value is discarded. Surface as
                // internal error so the runtime-shutdown case still has
                // an actionable message.
                Err(McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::Owned(format!(
                        "handler task cancelled in '{tool_name}': {join_err}"
                    )),
                    data: None,
                })
            }
        }
    }
}

/// RAII guard that aborts a spawned tokio task on drop. Paired with
/// `JoinHandle::abort_handle()` inside [`dispatch_with_catch`] so that
/// caller-side cancellation (future drop, outer timeout) propagates into
/// the spawned task instead of leaking it.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Best-effort extraction of a message from a panic payload. Rust stores
/// the argument to `panic!(...)` as a boxed `Any`:
///
///   * `panic!("literal")`      → `&'static str`
///   * `panic!("fmt {x}")`      → `String`
///   * `panic_any(custom_type)` → the custom type
///
/// We downcast the first two cases. Unknown payload types return a
/// fallback string so the caller never sees an empty message.
fn panic_message(payload: Box<dyn Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(s) => *s,
        Err(payload) => match payload.downcast::<&'static str>() {
            Ok(s) => (*s).to_string(),
            Err(_) => "<non-string panic payload>".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::Content;

    // The `#[allow(unreachable_code)]` annotations below are required
    // because rustc cannot prove the async block diverges after
    // `panic!()`, so the trailing `Ok(...)` is reported as dead code.
    // The trailing `Ok(...)` is needed purely to fix the async block's
    // output type; the `panic!()` is what the test actually exercises.

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn successful_handler_passes_through() {
        let result = dispatch_with_catch("ok_tool", async {
            Ok(CallToolResult::success(vec![Content::text("hi")]))
        })
        .await;
        let ok = result.expect("expected Ok");
        // Strengthened: verify the handler's own payload is forwarded
        // intact rather than replaced by the wrapper. A regression that
        // swapped the Ok arm would still have returned Ok(...) with
        // different content; this assertion catches that.
        let text = ok
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(text, vec!["hi"], "success payload mutated in transit");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn error_result_passes_through_unchanged() {
        let result = dispatch_with_catch("err_tool", async {
            Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::Borrowed("bad input"),
                data: None,
            })
        })
        .await;
        let err = result.expect_err("expected Err");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert_eq!(err.message, "bad input");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn panic_with_str_literal_returns_internal_error() {
        let result = dispatch_with_catch("str_panic_tool", async {
            panic!("boom");
            #[allow(unreachable_code)]
            Ok(CallToolResult::success(vec![]))
        })
        .await;
        let err = result.expect_err("expected Err from panicking handler");
        assert_eq!(err.code, ErrorCode::INTERNAL_ERROR);
        assert!(
            err.message.contains("handler panicked in 'str_panic_tool'"),
            "missing tool label in message: {}",
            err.message
        );
        assert!(
            err.message.contains("boom"),
            "panic payload not propagated: {}",
            err.message
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn panic_with_formatted_string_returns_internal_error() {
        let result = dispatch_with_catch("fmt_panic_tool", async {
            let n: u32 = 42;
            panic!("answer: {n}");
            #[allow(unreachable_code)]
            Ok(CallToolResult::success(vec![]))
        })
        .await;
        let err = result.expect_err("expected Err");
        assert_eq!(err.code, ErrorCode::INTERNAL_ERROR);
        assert!(
            err.message.contains("answer: 42"),
            "String-payload panic not propagated: {}",
            err.message
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn panic_with_non_string_payload_returns_fallback_message() {
        let result = dispatch_with_catch("non_string_tool", async {
            // panic_any with a value that is neither `&'static str` nor
            // `String` exercises the fallback arm of `panic_message`.
            std::panic::panic_any(42u64);
            #[allow(unreachable_code)]
            Ok(CallToolResult::success(vec![]))
        })
        .await;
        let err = result.expect_err("expected Err");
        assert_eq!(err.code, ErrorCode::INTERNAL_ERROR);
        assert!(
            err.message.contains("<non-string panic payload>"),
            "fallback panic-payload message missing: {}",
            err.message
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ten_consecutive_panics_do_not_kill_runtime() {
        for i in 0..10u32 {
            let result = dispatch_with_catch("spam_tool", async move {
                panic!("boom #{i}");
                #[allow(unreachable_code)]
                Ok(CallToolResult::success(vec![]))
            })
            .await;
            // Strengthened: verify the per-iteration error is specifically
            // an INTERNAL_ERROR that carries this iteration's payload. A
            // bug that returned the wrong error code or dropped the
            // payload would pass the old `result.is_err()` check.
            let err = result.unwrap_err();
            assert_eq!(
                err.code,
                ErrorCode::INTERNAL_ERROR,
                "iteration {i} wrong code: {:?}",
                err.code
            );
            assert!(
                err.message.contains(&format!("boom #{i}")),
                "iteration {i} payload missing: {}",
                err.message
            );
        }

        // Runtime must still be usable after 10 panics.
        let ok = dispatch_with_catch("ok_after_panics", async {
            Ok(CallToolResult::success(vec![Content::text("alive")]))
        })
        .await;
        assert!(
            ok.is_ok(),
            "runtime did not survive 10 consecutive panics: {ok:?}"
        );
    }
}
