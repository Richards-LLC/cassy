# Spike: OTEL Trace Propagation After Claude Code 2.1.128 OTEL_* Env Strip

**Date:** 2026-05-12  
**Task:** cas-8ad7  
**Author:** rapid-parrot-78  
**Verdict:** No impact. Cassy does not use the OpenTelemetry SDK and emits no spans. The `otel_context.json` propagation mechanism's write side is implemented; its read side is not yet wired up. Claude Code 2.1.128's env-var stripping is irrelevant to Cassy today.

---

## 1. What changed in Claude Code 2.1.128

Claude Code 2.1.128 stopped subprocesses — hooks (SessionStart, PostToolUse, PreToolUse, Stop, etc.), the MCP server, and Bash tool invocations — from inheriting `OTEL_*` environment variables. The intent is to prevent Cassy and other tools from accidentally forwarding telemetry to Claude's own OTLP endpoint when those tools have their own OTEL instrumentation.

The concern for Cassy: if Cassy reads `OTEL_EXPORTER_OTLP_ENDPOINT` (or any `OTEL_*` var) at startup, it would silently lose the exporter config when invoked from Claude Code 2.1.128+.

---

## 2. Cassy OTEL architecture — call-site map

### a. The otel_context.json mechanism

`cas-cli/src/otel.rs` implements the file-based propagation approach:

| Symbol | Purpose |
|---|---|
| `OtelContext::write(cas_root)` | Serializes session metadata to `.cas/otel_context.json` |
| `OtelContext::read(cas_root)` | Deserializes `.cas/otel_context.json` |
| `OtelContext::to_resource_attributes()` | Formats metadata as `key=val,key2=val2` (OTEL_RESOURCE_ATTRIBUTES wire format) |
| `get_otel_context(cas_root)` | Convenience wrapper over `OtelContext::read` |
| `get_resource_attributes(cas_root)` | Calls `to_resource_attributes()` on the file contents |

**Call sites for the write side:**

- `cas-cli/src/hooks/handlers/handlers_session.rs` L121–128 — `SessionStart` hook writes `OtelContext` containing `{session_id, project_id, project_path, permission_mode, active_task_id}` to `.cas/otel_context.json`.
- `cas-cli/src/hooks/handlers/handlers_session.rs` L433 — `SessionEnd` hook calls `OtelContext::remove()` to clean up.

**Call sites for the read side:**

None outside `otel.rs` tests. `get_otel_context()` and `get_resource_attributes()` are defined and tested in-module but are **not called anywhere in the production codebase**.

### b. Cassy's tracing subsystem (not OTEL)

`cas-cli/src/tracing/` is Cassy's internal event log:
- `TraceBuilder` constructs `TraceEvent` records (typed, SQLite-backed)
- `claude_wrapper.rs` wraps Claude API calls with duration recording
- This is entirely separate from OpenTelemetry — no OTLP export, no SDK, no `OTEL_*` env vars

### c. Cargo.toml dependency check

No `opentelemetry`, `opentelemetry-otlp`, or any `otel*` crate appears in:
- `Cargo.toml` (workspace)
- `cas-cli/Cargo.toml`
- Any `crates/*/Cargo.toml`

Cassy has **no OpenTelemetry SDK dependency**.

---

## 3. Impact assessment

| Concern | Status | Reason |
|---|---|---|
| Cassy reads `OTEL_EXPORTER_OTLP_ENDPOINT` at startup | ❌ Not applicable | No OTEL SDK → no env var reading |
| Cassy emits spans that would lose their parent context | ❌ Not applicable | No OTEL exporter → no spans emitted |
| `otel_context.json` propagation broken by env strip | ❌ Not applicable | File I/O is env-var-independent |
| `OTEL_RESOURCE_ATTRIBUTES` no longer flows to Cassy hooks | ❌ Not applicable | Cassy doesn't read this var |
| `service.name` / resource attribution lost | ❌ Not applicable | Cassy doesn't set these via env |

**The CC 2.1.128 change has zero impact on Cassy.**

---

## 4. Live test recipe (for future reference)

If Cassy ever gains an OTEL SDK exporter, the test would be:

```bash
# 1. Start a local OTEL collector (e.g., otelcol-contrib or Jaeger)
docker run -p 4317:4317 jaegertracing/all-in-one

# 2. Invoke Cassy from a bare shell with OTEL env vars
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
OTEL_SERVICE_NAME=cas-test \
  cas hook SessionStart <<'EOF'
{"session_id":"test-sess","hook_event_name":"SessionStart"}
EOF

# 3. Invoke Cassy from inside a Claude Code session
# (Claude Code 2.1.128+ strips OTEL_* before calling hooks)

# 4. Compare spans in the collector UI
# Expected if file-propagation is correctly wired:
#   - Both invocations emit spans with identical cas.* resource attributes
#   - The in-CC invocation doesn't lose span identity despite env strip
```

Today this test would produce **no Cassy spans in either case** — which is correct given no SDK is linked.

---

## 5. Gap identified: read side of otel_context.json is unimplemented

The write side (`SessionStart` → `otel_context.json`) is wired. The read side (`get_resource_attributes()` → `OTEL_RESOURCE_ATTRIBUTES` → span annotation) is not called from anywhere.

When Cassy adds OTEL span export in the future, the correct path for the hooks and MCP server would be:
1. Read `.cas/otel_context.json` at startup (already works, env-var-independent ✅)
2. Call `get_resource_attributes()` to build the resource attributes string
3. Initialize the OTEL SDK with those attributes (do NOT read `OTEL_RESOURCE_ATTRIBUTES` from env — the CC 2.1.128 strip would break that path)

The file-based design sidesteps the env-var strip problem correctly — the architecture decision was sound even though the read side isn't wired yet.

---

## 6. Recommendation

**No remediation needed.** Cassy is unaffected by CC 2.1.128's env-var stripping.

When implementing OTEL span export for Cassy in the future:
- Use `get_resource_attributes()` to initialize the SDK resource (reads the file, not env)
- Do NOT add a fallback to `OTEL_RESOURCE_ATTRIBUTES` env — it won't be available in Claude Code sessions
- The exporter endpoint (`OTEL_EXPORTER_OTLP_ENDPOINT`) can still be configured via env on bare-shell invocations, but should also have a Cassy config file override for in-CC use

**No follow-on remediation task filed.** Consider a `feat(otel): wire get_resource_attributes into SDK init` task when OTEL export is prioritized.
