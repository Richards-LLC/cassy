//! Shared MCP proxy transport for the `cas integrate <platform>` handlers.
//!
//! Hoisted from cas-7417's `vercel::mcp_proxy_client` and cas-1549's
//! `LiveNeonClient` after both shipped near-identical copies of:
//!
//! - [`proxy_config_path`] — locate `<cas_root>/proxy.toml` (else fall
//!   through to cmcp_core's user-level lookup).
//! - [`unwrap_envelope`] — strip the MCP `{ content: [{ type: "text",
//!   text: "<json>" }] }` wrapper and surface `isError` envelopes as
//!   `Err`.
//! - [`ProxyClient`] — cached `(tokio::runtime::Runtime, cmcp_core::ProxyEngine)`
//!   lazily built on first call, reused for the lifetime of the client,
//!   shut down inside the held runtime on `Drop`. Generic by upstream
//!   server name (`"vercel"`, `"neon"`, future `"github"` …).
//!
//! Owner: task **cas-36fd0**. The whole module is gated behind the
//! `mcp-proxy` feature; non-feature builds rely on per-handler `Err`
//! stubs that surface the rebuild instruction.
//!
//! ## Drop discipline
//!
//! [`ProxyClient`] **must not** be dropped from inside an active tokio
//! runtime — `rt.block_on(...)` in [`Drop`] panics with "Cannot start a
//! runtime from within a runtime". Today the only constructor sites
//! (`vercel::default_client`, `neon::LiveNeonClient::default`) are
//! reached from `cas integrate` running on sync `main`, so this is fine.
//! Future async callers must call [`ProxyClient::shutdown`] explicitly
//! from a non-async context before allowing the value to drop.

#![cfg(feature = "mcp-proxy")]

use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, anyhow};
use serde_json::Value;
use tokio::runtime::Runtime;

/// Resolve a proxy config path: first `<cas_root>/proxy.toml` if cas is
/// initialized AND that file exists, else `None` (cmcp_core's
/// `Config::load_merged(None)` then falls back to the user-level
/// `~/.config/code-mode-mcp/config.toml`).
pub fn proxy_config_path() -> Option<PathBuf> {
    crate::store::find_cas_root()
        .ok()
        .map(|r| r.join("proxy.toml"))
        .filter(|p| p.exists())
}

/// MCP tool calls return an envelope of the form
/// `{ content: [{ type: "text", text: "<json>" }, ...], isError: bool }`.
/// Strip the wrapper, parse the inner JSON, and surface failures as `Err`:
///
/// - `{ isError: true, ... }` → `Err` (transport / upstream-tool failure).
/// - `{ content: [{ text: "" }] }` (or any all-empty text concatenation) →
///   `Ok(Value::Null)` so callers can distinguish "tool returned nothing"
///   from "JSON parse failed".
/// - Bare object/array (no envelope) → returned unchanged. Some test
///   fixtures and older MCP servers skip the wrapper.
/// - `{ content: [...] }` with non-empty text that fails JSON parse → `Err`.
pub fn unwrap_envelope(value: &Value) -> anyhow::Result<Value> {
    let Value::Object(map) = value else {
        return Ok(value.clone());
    };
    if map.get("isError").and_then(|v| v.as_bool()) == Some(true) {
        anyhow::bail!("MCP returned isError=true: {value}");
    }
    let Some(Value::Array(content)) = map.get("content") else {
        return Ok(value.clone());
    };
    let mut buf = String::new();
    for item in content {
        if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
            buf.push_str(t);
        }
    }
    if buf.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&buf)
        .with_context(|| format!("parsing MCP text content: {buf}"))
}

/// Shared (runtime, engine) state. `Mutex<Option<…>>` so we can `.take()`
/// on Drop and run `engine.shutdown().await` inside the held runtime
/// before the runtime is dropped.
type ProxyState = (Runtime, cmcp_core::ProxyEngine);

/// Lazily-initialized MCP proxy client. Construct one per upstream server
/// name; reuse across calls. `Drop` shuts down the engine and joins the
/// runtime.
///
/// # Drop discipline
///
/// **Must NOT be dropped from inside an active tokio runtime.**
/// `Drop` calls `rt.block_on(engine.shutdown())` on the cached
/// current-thread runtime, which panics with "Cannot start a runtime
/// from within a runtime" when invoked while another runtime is
/// already executing on the thread. Future async callers must call
/// the explicit shutdown path (TODO: expose a `close(self)`) before
/// the value drops, or construct the client in a sync scope and
/// arrange for it to drop there.
pub struct ProxyClient {
    /// Upstream MCP server identifier (e.g. `"vercel"`, `"neon"`). Stored
    /// as `&'static str` since every call site passes a string literal.
    server_name: &'static str,
    state: Mutex<Option<ProxyState>>,
}

impl ProxyClient {
    /// Construct a new proxy client for the named upstream server.
    ///
    /// **See [Drop discipline](Self#drop-discipline) before storing the
    /// returned value in async-owned state.**
    pub fn new(server_name: &'static str) -> Self {
        Self {
            server_name,
            state: Mutex::new(None),
        }
    }

    /// Test-visible accessor: true once the engine has been lazily
    /// constructed. Callers can use this to assert engine reuse across
    /// multiple calls without depending on a live MCP transport.
    #[cfg(test)]
    pub fn engine_constructed(&self) -> bool {
        self.state.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Borrowed accessor for the configured server name (for diagnostics).
    pub fn server_name(&self) -> &'static str {
        self.server_name
    }

    /// Lazily build the (runtime, engine) pair on first call, then run
    /// `f` against it. Subsequent calls reuse the existing engine.
    fn with_engine<F, T>(&self, f: F) -> anyhow::Result<T>
    where
        F: FnOnce(&Runtime, &cmcp_core::ProxyEngine) -> anyhow::Result<T>,
    {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| anyhow!("ProxyClient mutex poisoned"))?;
        if guard.is_none() {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("building tokio runtime")?;
            let cfg = cmcp_core::config::Config::load_merged(
                proxy_config_path().as_deref(),
            )
            .context("loading MCP proxy config")?;
            anyhow::ensure!(
                !cfg.servers.is_empty(),
                "no MCP servers configured. Run `cas mcp add {} ...` or check ~/.config/code-mode-mcp/config.toml.",
                self.server_name
            );
            let engine = rt
                .block_on(cmcp_core::ProxyEngine::from_configs(cfg.servers))
                .context("starting MCP proxy engine")?;
            *guard = Some((rt, engine));
        }
        let (rt, engine) = guard.as_ref().unwrap();
        f(rt, engine)
    }

    /// Call `<server_name>.<tool>` through the cached engine and return
    /// the raw envelope value. Callers should pipe the result through
    /// [`unwrap_envelope`] (or a higher-level parser) to extract the
    /// inner payload.
    pub fn call(
        &self,
        tool: &str,
        args: Option<serde_json::Map<String, Value>>,
    ) -> anyhow::Result<Value> {
        let server_name = self.server_name; // &'static str — Copy
        // This synchronous CLI path is not an MCP agent session. Keep its
        // identity explicit and non-privileged so a future proxy policy can
        // deny integration calls rather than seeing an anonymous bypass.
        let caller = cmcp_core::ProxyCaller {
            agent_id: "cas-integrate-cli".to_string(),
            role: crate::types::AgentRole::Standard,
            session_id: format!("cas-integrate-{}", std::process::id()),
            factory_session: None,
            active_task_ids: Vec::new(),
        };
        self.with_engine(|rt, engine| {
            rt.block_on(async {
                engine
                    .call_tool(&caller, server_name, tool, args)
                    .await
                    .with_context(|| format!("calling {server_name}.{tool}"))
            })
        })
    }

    /// Shut down a lazily-created proxy engine before this client is dropped.
    ///
    /// Call this from a synchronous context when a client may otherwise be
    /// dropped while a Tokio runtime is active. It is safe to call more than
    /// once; later calls have no engine left to stop. Like [`Drop`], this
    /// method must not run from inside an active Tokio runtime.
    pub fn shutdown(&self) {
        // Recover from a poisoned Mutex so a prior failed call cannot leave
        // an upstream MCP child alive until process exit.
        let mut guard = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some((rt, engine)) = guard.take() {
            rt.block_on(async move {
                engine.shutdown().await;
            });
        }
    }
}

impl Drop for ProxyClient {
    fn drop(&mut self) {
        // Shut down the engine inside the runtime that owns it. Recover
        // from a poisoned Mutex via PoisonError::into_inner — otherwise
        // a panic during a prior `with_engine` would silently skip
        // shutdown and leak the upstream MCP child for the lifetime of
        // the parent.
        //
        // Drop MUST NOT be invoked from inside an active tokio runtime
        // (see module doc).
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use serde_json::json;

    // --- unwrap_envelope --------------------------------------------------

    #[test]
    fn unwrap_envelope_passes_bare_object_through() {
        let v = json!({ "id": "abc", "name": "x" });
        assert_eq!(unwrap_envelope(&v).unwrap(), v);
    }

    #[test]
    fn unwrap_envelope_passes_bare_array_through() {
        let v = json!([{ "id": "a" }, { "id": "b" }]);
        assert_eq!(unwrap_envelope(&v).unwrap(), v);
    }

    #[test]
    fn unwrap_envelope_strips_text_content_envelope_and_parses_inner_json() {
        let v = json!({
            "content": [{
                "type": "text",
                "text": "{\"id\":\"abc\",\"name\":\"hello\"}"
            }]
        });
        let inner = unwrap_envelope(&v).unwrap();
        assert_eq!(inner, json!({ "id": "abc", "name": "hello" }));
    }

    #[test]
    fn unwrap_envelope_concatenates_multiple_text_parts() {
        let v = json!({
            "content": [
                { "type": "text", "text": "{\"k\":" },
                { "type": "text", "text": "1}" }
            ]
        });
        let inner = unwrap_envelope(&v).unwrap();
        assert_eq!(inner, json!({ "k": 1 }));
    }

    #[test]
    fn unwrap_envelope_returns_null_on_empty_text_content() {
        let v = json!({ "content": [{ "type": "text", "text": "" }] });
        assert_eq!(unwrap_envelope(&v).unwrap(), Value::Null);
    }

    #[test]
    fn unwrap_envelope_propagates_is_error_envelope_as_err() {
        let v = json!({
            "isError": true,
            "content": [{ "type": "text", "text": "auth failure" }]
        });
        let err = unwrap_envelope(&v).unwrap_err().to_string();
        assert!(err.contains("isError=true"), "got: {err}");
    }

    #[test]
    fn unwrap_envelope_returns_err_on_unparseable_inner_json() {
        let v = json!({
            "content": [{ "type": "text", "text": "this is not json" }]
        });
        let err = unwrap_envelope(&v).unwrap_err().to_string();
        assert!(err.contains("parsing MCP text content"), "got: {err}");
    }

    #[test]
    fn unwrap_envelope_passes_object_without_content_array_through() {
        // {projects: [...]} is one of the older shapes; unwrap_envelope
        // returns it unchanged so the platform-specific parser can pick
        // the right wrapper key.
        let v = json!({ "projects": [{ "id": "a" }] });
        assert_eq!(unwrap_envelope(&v).unwrap(), v);
    }

    // --- ProxyClient lifecycle (no live MCP) ------------------------------

    #[test]
    fn new_does_not_construct_engine() {
        let client = ProxyClient::new("vercel");
        assert!(!client.engine_constructed());
        assert_eq!(client.server_name(), "vercel");
        // Drop with no engine constructed must not panic.
        drop(client);
    }

    #[test]
    fn explicit_shutdown_is_idempotent_before_lazy_engine_initialization() {
        let client = ProxyClient::new("vercel");
        client.shutdown();
        client.shutdown();
        assert!(!client.engine_constructed());
    }

    #[test]
    fn server_name_is_threaded_into_diagnostics() {
        let client = ProxyClient::new("neon");
        assert_eq!(client.server_name(), "neon");
        drop(client);
    }

    /// cas-4ccc: a hermetic test must not read the *host's* project config.
    ///
    /// `proxy_config_path` resolves through `find_cas_root`, which walks up
    /// from the current directory (and maps a git worktree onto its main
    /// repository's `.cas`). Every factory worktree lives under
    /// `<repo>/.cas/worktrees/<name>`, so on 2026-09-03 a `proxy.toml` the
    /// operator created in the main checkout became visible to three tests
    /// that had set only HOME and XDG_CONFIG_HOME: the loader handed cmcp_core
    /// a configured http server, reqwest built a client outside `main()`, and
    /// they panicked with "No provider set". Moving the file aside made them
    /// pass with no code change — the tests were never hermetic.
    ///
    /// This plants the same shape in an ancestor of the test's own directory,
    /// so it fails on any machine rather than only on one that happens to have
    /// the operator's file.
    #[test]
    fn ancestor_project_proxy_config_is_invisible_to_a_hermetic_test() {
        let mut env = TestEnvGuard::temp_home();
        let xdg = env.home().join(".config");
        env.set("XDG_CONFIG_HOME", xdg);

        // <ancestor>/.cas/proxy.toml — exactly what the main checkout has.
        let ancestor = env.home().join("checkout");
        std::fs::create_dir_all(ancestor.join(".cas")).expect("ancestor .cas");
        std::fs::write(
            ancestor.join(".cas").join("proxy.toml"),
            "[servers.mecha-cassy]\ntype = \"http\"\nurl = \"https://example.invalid/mcp\"\n",
        )
        .expect("ancestor proxy.toml");

        // …and the test runs from a directory below it, like a worktree does.
        let work = ancestor.join(".cas").join("worktrees").join("worker");
        std::fs::create_dir_all(&work).expect("worktree dir");
        env.set_current_dir(&work);

        assert_eq!(
            proxy_config_path(),
            None,
            "a hermetic test must not resolve an ancestor project's proxy.toml"
        );

        // The whole point: the client must reach its own empty-config error
        // instead of building a real http transport for the ancestor's server.
        let client = ProxyClient::new("vercel");
        let err = client.call("list_projects", None).unwrap_err();
        assert!(
            err.to_string().contains("no MCP servers configured"),
            "expected the empty-config Err; got: {err}"
        );
        drop(client);
    }

    #[test]
    fn first_call_attempts_lazy_init_and_engine_stays_uninstalled_on_empty_config() {
        // Hermetic env: no proxy.toml anywhere → load_merged returns
        // an empty servers map → ensure! fires BEFORE the engine is
        // installed into self.state. engine_constructed must remain
        // false so a follow-on retry hits the same code path.
        let mut env = TestEnvGuard::temp_home();
        let xdg = env.home().join(".config");
        env.set("XDG_CONFIG_HOME", xdg);

        let client = ProxyClient::new("vercel");
        assert!(!client.engine_constructed());
        let err = client.call("list_projects", None).unwrap_err();
        assert!(
            err.to_string().contains("no MCP servers configured"),
            "expected empty-config Err; got: {err}"
        );
        assert!(
            !client.engine_constructed(),
            "ensure! fires before installing the engine; engine_constructed must remain false"
        );
        // Drop without engine must not panic.
        drop(client);
    }
}
