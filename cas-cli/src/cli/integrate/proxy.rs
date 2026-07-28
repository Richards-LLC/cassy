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
        self.with_engine(|rt, engine| {
            rt.block_on(async {
                engine
                    .call_tool(server_name, tool, args)
                    .await
                    .with_context(|| format!("calling {server_name}.{tool}"))
            })
        })
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
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some((rt, engine)) = guard.take() {
            rt.block_on(async move {
                engine.shutdown().await;
            });
            // rt drops here, joining its blocking pool.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn server_name_is_threaded_into_diagnostics() {
        let client = ProxyClient::new("neon");
        assert_eq!(client.server_name(), "neon");
        drop(client);
    }

    #[test]
    fn first_call_attempts_lazy_init_and_engine_stays_uninstalled_on_empty_config() {
        // Hermetic env: no proxy.toml anywhere → load_merged returns
        // an empty servers map → ensure! fires BEFORE the engine is
        // installed into self.state. engine_constructed must remain
        // false so a follow-on retry hits the same code path.
        let _env_guard = crate::hooks::test_env_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        // SAFETY: test_env_lock serializes every cooperating env writer;
        // the Restore guard restores both variables on drop.
        unsafe {
            std::env::set_var("HOME", tmp.path());
            std::env::set_var("XDG_CONFIG_HOME", tmp.path().join(".config"));
        }
        struct Restore {
            home: Option<std::ffi::OsString>,
            xdg: Option<std::ffi::OsString>,
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe {
                    match &self.home {
                        Some(v) => std::env::set_var("HOME", v),
                        None => std::env::remove_var("HOME"),
                    }
                    match &self.xdg {
                        Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                        None => std::env::remove_var("XDG_CONFIG_HOME"),
                    }
                }
            }
        }
        let _g = Restore {
            home: prev_home,
            xdg: prev_xdg,
        };

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
