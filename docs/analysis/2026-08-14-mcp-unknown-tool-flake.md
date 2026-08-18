# `test_mcp_unknown_tool` CI flake: retained-evidence verdict

Issue: [GH #164](https://github.com/pippenz/cas/issues/164)  
Task: `cas-3ce4`  
Original failing run: [31218177756, attempt 1](https://github.com/pippenz/cas/actions/runs/31218177756/attempts/1)  
Parent run: [31216239877](https://github.com/pippenz/cas/actions/runs/31216239877)

## Verdict

The underlying event is irreducible from the retained evidence. It did not
reproduce in the required provoking experiment: 1,000 fresh test processes at
24-way concurrency all passed. No production-code defect is established, so a
production change would be speculative.

The earlier `cas-7bb94` change remains correct and is preserved: it bounds the
test harness's stdout wait, preventing an unobserved non-response from consuming
nextest's 600-second timeout. Its diagnostic, however, said the server had
"accepted the request and never answered." The original log contains no phase
markers and does not establish that. This task corrects the diagnostic to list
the three possibilities it can actually distinguish only with more evidence:
request not read, handler not completed, or response not flushed.

## What the CI record proves

- Head `3d58332e` differs from parent `af44ad4d` only in an insta doctor
  snapshot. The MCP test, server, `rmcp` version, and lockfile implementation are
  identical.
- The parent completed `test_mcp_unknown_tool` in 0.254 seconds. Attempt 1 at
  the head left only that test running until nextest terminated it at 600.012
  seconds. Attempt 2 on the same head passed.
- Attempt 1 emitted no harness phase marker, server stderr, process state, or
  stack trace. It therefore cannot locate the wait inside server startup, MCP
  initialization, `tools/call`, response delivery, or process teardown. (`cas
  init` had its separate 300-second watchdog, making that phase inconsistent
  with a test that remained alive for 600 seconds.)
- The old unbounded `BufReader::read_line` explains why any absent line became a
  600-second wedge. It does not explain why the line was absent.

## Code-path and shared-state audit

The test creates a fresh `TempDir`, `HOME`, `XDG_CONFIG_HOME`, `CAS_ROOT`, and
`CAS_DIR` for each process. It scrubs inherited `CAS_*` variables. There is no
application database, socket, index, or configuration path shared by two
repetitions; only host resources such as the kernel and scheduler are shared.

After initialization the test sends one `tools/call` request for
`nonexistent_tool`. In `rmcp` 0.14.0, `ToolRouter::call` performs an immutable
`HashMap::get` and immediately returns `invalid_params("tool not found")` on a
miss. Cassy wraps that future in the existing 55-second handler timeout. There is
no alternate unknown-tool handler or state-dependent branch to select.

Upstream `rmcp` issue
[modelcontextprotocol/rust-sdk#941](https://github.com/modelcontextprotocol/rust-sdk/issues/941)
describes the same outward symptom, but its mechanism does not apply. That bug
was introduced after 0.14.0 when the transport replaced `FramedRead` with a
`read_until` buffer that was cleared after cancellation. Cassy's pinned 0.14.0
uses `FramedRead` plus a codec-owned persistent buffer. The upstream fix
([#947](https://github.com/modelcontextprotocol/rust-sdk/pull/947)) therefore
cannot explain the August 7 Cassy run.

## Provoking experiment

The already-built integration-test executable was invoked directly so every
iteration was a fresh OS process while avoiding Cargo serialization. Every
process ran exactly `test_mcp_unknown_tool`, which in turn spawned fresh
`cas init` and `cas serve` children. An external 15-second deadline prevented
the harness's 90-second diagnostic bound from hiding a wedge.

```text
iterations:  1,000
concurrency: 24
passes:      1,000
failures:    0
timeouts:    0
elapsed:     18.51s
head:        fb69a7dbc8605cd60edf9c94b102d71ae5fa5958
```

Durable artifacts:

- `/home/pippenz/.cas/artifacts/cas-3ce4/run-stress.sh`
- `/home/pippenz/.cas/artifacts/cas-3ce4/stress-before-v1.log`
- result-log SHA-256:
  `3f7c889d47548b26420b26791febf6cf6659674505d0ba94bd14a755d7c5a122`

## Decision

Preserve the bounded-read protection, correct the unsupported causal wording,
and make no production change. A future occurrence is actionable only if it
captures a phase or process stack in addition to the request id and child
status. Without that new observation, attributing the one historical event to
Cassy, `rmcp`, SQLite, the kernel, or runner scheduling would be guesswork.
