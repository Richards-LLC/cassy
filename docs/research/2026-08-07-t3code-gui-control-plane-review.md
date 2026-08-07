# t3code review — GUI control plane for CAS factory sessions

Date: 2026-08-07
Source under review: `/mnt/datacube/staging/t3code-main` (Theo/T3's "t3code" monorepo, zip snapshot, treated read-only)
Motivating question: a graphical way to control CAS sessions on multiple systems **without burning Claude/Codex subscription quota** — observe and control existing CLI sessions, never re-implement agent calls over metered APIs.

---

## 1. What t3code is

**Product**: "an agent harness control surface" — its own words (`README.md:3`). It controls coding-agent CLIs already installed and authenticated on your machine: "Works with your subscriptions on Claude Code, Codex, Cursor, Grok Build, and OpenCode. If they're set up on your computer, T3 Code can control them" (`README.md:5`). This is *exactly* the constraint pippenz cares about — t3code is subscription-account-native by design, and its README's install prerequisite is literally "run `claude auth login` / `codex login`" (`README.md:17-23`).

**Stack** (root `package.json`, `pnpm-workspace.yaml`):

| Component | What it is |
|---|---|
| `apps/server` | The product's brain. Node (22.16+/24) headless server, npm package **`t3`** with bin `t3` (`apps/server/package.json:5,10`). Effect-TS, event-sourced orchestration, SQLite via `@effect/sql-sqlite-bun`, `node-pty` terminals. Run via `npx t3@latest`. |
| `apps/web` | React 19 + TanStack Router SPA; the actual UI, served locally by the server or hosted at `app.t3.codes`. Terminal emulation via **libghostty compiled to WASM** (`apps/web/package.json:10` `build:ghostty-wasm`, `apps/web/src/terminal/ghostty/`). |
| `apps/desktop` | Thin **Electron 41** shell around the web app + server (`apps/desktop/package.json:27`); adds SSH launch, Tailscale endpoint detection, auto-update. |
| `apps/mobile` | Expo / React Native iOS+Android client — same client-runtime, different platform layer. |
| `apps/marketing` | Website. |
| `packages/client-runtime` | Shared non-visual client core: connection supervisor, RPC session, Atom state (`docs/internals/overview.md` "Shared client runtime"). |
| `packages/contracts` | The typed Effect RPC contract between clients and server (`packages/contracts/src/rpc.ts`). |
| `packages/effect-acp` | Agent Client Protocol (ACP) implementation — JSON-RPC over stdio to agent CLIs. |
| `packages/effect-codex-app-server` | Client for Codex's `codex app-server` JSON-RPC protocol. |
| `packages/ssh`, `packages/tailscale` | Remote launch/tunnel helpers (see §3). |
| `native/resource-monitor` | Tiny Rust binary (`sysinfo` + serde, ~1 file: `native/resource-monitor/src/main.rs`) run as a supervised child process emitting NDJSON process/power telemetry — deliberately *not* a Node addon so a crash can't take down the server (`docs/internals/resource-telemetry.md:22-35`). |

**License**: **MIT**, "Copyright (c) 2026 T3 Tools Inc." (`LICENSE:1-3`). No dual-licensing, no CLA-encumbered exceptions found in-repo. **We can legally fork, vendor, and modify any of it**, including selling derivatives, with attribution (keep the copyright + permission notice). The repo itself says: "we want you to have everything you need to fork and build the editor that you want" (`README.md:11`).

**Maturity signals**: version 0.0.32 across apps; README: "We are very very early in this project. Expect bugs" and "(mostly) not accepting contributions" (`README.md:73-75`). Counter-signals: unusually good internals docs (`docs/internals/*.md` are accurate against the code), tests beside nearly every module, a real event-sourced core with transactional projection, CI docs, release smoke scripts. Alpha product, senior engineering.

---

## 2. Agent execution model — CLI spawns, not metered APIs

The server is the sole execution boundary ("every provider process, terminal, git operation, and filesystem read happens there, never in the client" — `docs/internals/overview.md:6-8`). Five provider drivers, registered in `apps/server/src/provider/builtInDrivers.ts`, each spawning the **user's locally installed, locally authenticated CLI**:

| Provider | Transport | Evidence |
|---|---|---|
| Claude | `@anthropic-ai/claude-agent-sdk` `query()` which spawns the local `claude` binary | `apps/server/src/provider/Layers/ClaudeAdapter.ts:1653` (the `query({...})` call), `:4096` `pathToClaudeCodeExecutable: claudeBinaryPath`; the driver resolves `executable: "claude"` (`apps/server/src/provider/Drivers/ClaudeDriver.ts:77`) via `ClaudeExecutable.ts` (whole file is about finding a spawnable `claude` path). `ClaudeHome.ts:29` even preserves `$HOME/Library/Keychains` access "so the spawned CLI can find its stored" credentials — i.e. subscription OAuth creds are the expected auth. |
| Codex | spawns `codex app-server` and speaks its JSON-RPC protocol | `apps/server/src/provider/Layers/codexLaunchArgs.ts:14` (`codexAppServerArgs = ["app-server", ...]`), `CodexSessionRuntime.ts:849` (`ChildProcessSpawner`), protocol client in `packages/effect-codex-app-server`. |
| Cursor | spawns the `agent` CLI in ACP mode | header comment "Cursor CLI (`agent acp`) via ACP" (`apps/server/src/provider/Layers/CursorAdapter.ts:2`); args `["acp"]` (`apps/server/src/provider/acp/CursorAcpSupport.test.ts:57`). |
| Grok | spawns `grok agent stdio` in ACP mode | `apps/server/src/provider/acp/GrokAcpSupport.ts:38-39` — `command: grokSettings?.binaryPath || "grok"`, `args: ["agent", "stdio"]`. |
| OpenCode | spawns/owns an `opencode` server process via `@opencode-ai/sdk` | `apps/server/src/provider/Layers/OpenCodeAdapter.ts:235` "tears down the OpenCode server process for scope-owned servers". |

**No metered inference path exists in the server.** Grepping `apps/server/src` for API keys turns up only: PostHog analytics (`telemetry/AnalyticsService.ts:112`), env-plumbing tests, and *auth-method classification* — `ClaudeProvider.ts:518-546` inspects whether the user's CLI account is `apiKey` vs subscription so the UI can display it, it does not make Anthropic API calls itself.

**Session lifecycle / quota discipline**:
- One adapter instance per thread; `ProviderInstanceRegistry` owns live processes, `ProviderService` routes by thread so orchestration never knows which agent is behind it (`docs/internals/providers.md`).
- Sessions are **resumed**, not respawned (`CodexSessionRuntime.ts:485` logs "thread resume fell back to fresh start" only as a fallback).
- Output is a **push stream** from the CLI process, normalized by `ProviderRuntimeIngestion` into orchestration commands — there is no polling of agents anywhere.
- Client-facing control verbs are commands, not model calls: `thread.turn.start`, `thread.turn.interrupt`, `thread.approval.respond`, `thread.user-input.respond`, `thread.session.stop` (`docs/internals/providers.md`, defined in `packages/contracts/src/orchestration.ts`).
- Assistant deltas can be batched (`MAX_BUFFERED_ASSISTANT_CHARS` = 24,000 in `ProviderRuntimeIngestion`) — a bandwidth optimization for mobile, again zero extra tokens.

Verdict on the constraint: **t3code's whole execution model is the subscription-friendly one.** N clients watching a thread cost zero extra model calls; only user-initiated turns consume quota.

---

## 3. Multi-machine story — shipped, and deliberately decentralized

`docs/internals/remote.md:4-5`: "Remote environments are shipped, not planned. Direct, bearer-paired, relay-tunneled, Tailscale, and desktop-managed SSH access all exist today."

**Core model**: one `ExecutionEnvironment` = one running t3 server, identified by a stable `environmentId` persisted at `<stateDir>/environment-id` (`apps/server/src/environment/ServerEnvironment.ts`). Clients hold a **local** list of known environments and connect to each one directly over a single authenticated WebSocket. **There is no central control plane**: the hosted web app "does not give the hosted app a server-side control plane or a copy of session state" and "does not proxy HTTP or WebSocket traffic" (`docs/internals/remote.md`, "Known environments" / "Hosted pairing request"). Multi-machine = the client fans out.

**Four connection target kinds** (`packages/client-runtime/src/connection/model.ts`):
1. `PrimaryConnectionTarget` — the platform-managed local server.
2. `BearerConnectionTarget` — any manually paired direct ws/wss endpoint (Tailscale URLs ride this path; Tailscale is "an endpoint provider and transport, not a distinct runtime concept").
3. `RelayConnectionTarget` — managed "T3 Connect" relay: Clerk-authenticated broker that provisions a **Cloudflare tunnel** hostname; app traffic flows over the tunnel, not through the relay worker (`docs/internals/remote.md` "Relay-tunneled access", `infra/relay/src/http/Api.ts`, `docs/internals/t3-connect.md`).
4. `SshConnectionTarget` — desktop-managed SSH: probes the host, writes a launcher script under `~/.t3/ssh-launch/<host-key>/`, starts or reuses a remote t3 server, port-forwards loopback back (`apps/desktop/src/ssh/DesktopSshEnvironment.ts`, `packages/ssh/src/tunnel.ts`, `docs/user/remote-access.md`).

**Tailscale integration is server-side too**: with `tailscaleServeEnabled`, the server acquires a `tailscale serve` HTTPS mapping for its port at startup (`ensureTailscaleServe` in `apps/server/src/server.ts`, using `packages/tailscale/src/tailscale.ts`), advertising `https://machine.tailnet.ts.net/`.

**Pairing & auth**:
- `t3 serve` / `t3 pair` mint a **one-time pairing token**, printed as URL + QR; the device exchanges it for a session; future access is session-based (`docs/user/remote-access.md` "How Pairing Works").
- Hosted-web pairing puts the token in the **URL hash** so it never reaches the hosted origin (`packages/shared/src/remote.ts` `setPairingTokenOnUrl` etc.).
- WebSocket auth is a dedicated **short-lived ticket**: client presents its bearer/DPoP credential to `POST /api/auth/websocket-ticket`, gets a `kind: "websocket"` ticket with 5-minute TTL (`DEFAULT_WEBSOCKET_TOKEN_TTL` in `apps/server/src/auth/SessionStore.ts`), appends only that to the socket URL. Every RPC method then enforces a per-method scope via `RPC_REQUIRED_SCOPE` (`docs/internals/overview.md` "The RPC boundary", `docs/internals/environment-auth.md`).
- `t3 auth` lists/revokes sessions and credentials.

`dev:share` (`scripts/dev-runner.ts:22`, `scripts/lib/dev-share.ts`) is dev tooling to publish a dev server through the same share path — a convenience wrapper, not a distinct architecture.

---

## 4. UI / state-sync architecture

**Transport**: one Effect RPC group over one WebSocket at `/ws` — not REST, not tRPC, not polling. `packages/contracts/src/rpc.ts:168` declares `WS_METHODS`; streaming members carry `stream: true` (e.g. `:326`, `:400`, `:494`). Subscriptions replace any broadcast bus: `orchestration.subscribeThread`, `orchestration.subscribeShell`, `subscribeServerConfig`, `subscribeVcsStatus` (`rpc.ts:258`), `subscribeTerminalEvents`/`subscribeTerminalMetadata` (`:259-260`), `subscribeResourceTelemetry` (`:267`), `terminal.attach`. "A client subscribes to what it needs and the server pushes only on that subscription" (`docs/internals/overview.md`).

**State model**: server side is event-sourced — clients dispatch typed commands (`orchestration.dispatchCommand`), a single-fiber `OrchestrationEngine` turns them into persisted events in one SQL transaction with the projection, then publishes committed events to subscribers (`apps/server/src/orchestration/Layers/OrchestrationEngine.ts`, `decider.ts`, `projector.ts`; `docs/internals/overview.md` "Orchestration is event-sourced"). Client side: `packages/client-runtime` owns connection supervision (retry forever, exp backoff capped 16s, offline-aware — `docs/internals/connection-runtime.md` "Connection State"), and exposes domain state as `@effect/atom-react` atoms that React renders. Components never construct transports.

**Session-control affordances** (the things a CAS GUI needs):
- Session/thread list per environment, grouped across environments by `RepositoryIdentity` (UI-only grouping — `docs/internals/remote.md`).
- Live agent output: streamed deltas via `subscribeThread`; markdown-rendered chat.
- **Approve/deny**: `thread.approval.respond`; **message injection**: `thread.turn.start` with user input; **interrupt**: `thread.turn.interrupt`; **kill**: `thread.session.stop` (`docs/internals/providers.md`).
- **Real terminals**: server spawns PTYs via `node-pty` / `Bun.spawn` (`apps/server/src/terminal/NodePtyAdapter.ts`, `BunPtyAdapter.ts:131`), client attaches (`terminal.attach` stream) and renders with a **libghostty WASM** terminal emulator (`apps/web/src/terminal/ghostty/{core,surface,renderer}.ts`, built by `apps/web/scripts/build-libghostty-wasm.sh`).
- Checkpoint/diff review per turn via hidden git refs (`CheckpointStore` / `CheckpointDiffQuery`, `docs/internals/overview.md` "Checkpointing").
- Host telemetry (CPU/mem per agent process) via the Rust resource-monitor, streamed by `subscribeResourceTelemetry`.

---

## 5. Mapping to CAS — recommended architecture

### What CAS already has (more than expected)

- **A live WS control protocol on the factory daemon, today.** The daemon binds a WebSocket listener (`cas-cli/src/ui/factory/daemon/runtime/lifecycle.rs:66` — `TcpListener::bind("127.0.0.1:0")`, port recorded in session metadata `protocol.rs:289`) and serves the same `DaemonMessage`/`ClientMessage` protocol the TUI uses (`runtime/ws_client.rs:13` `accept_ws_clients`, frames are raw JSON over WS Binary). The protocol is already GUI-shaped: `ClientMessage::{Attach{request_scrollback}, Input{pane_id,data}, InputFocused, Focus, Resize}` (`cas-cli/src/ui/factory/protocol.rs:11-51`) and `DaemonMessage::{Welcome{state,scrollback}, Output{pane_id,data}, PaneAdded, PaneExited, FocusChanged, ...}` (`protocol.rs:128-168`). `Output.data` is raw VT bytes — exactly what a ghostty/xterm surface consumes.
- **An HTTP bridge with auth.** `cas bridge` runs a `tiny_http` server with bearer-token auth + CORS (`cas-cli/src/bridge/server/mod.rs:34-74,122`), SSE event streams per session (`/v1/sessions/<name>/events`, `mod.rs:139-160`), pane tail snapshots (`/v1/sessions/<name>/panes/<pane_id>/tail`, `routes.rs:53`), inbox peek/poll/ack, `targets`, `activity`, and factory start (`bridge/server/factory.rs`).
- **Cross-machine discovery + attach (terminal-grade).** `cas attach device:factory-id` resolves the device via cloud `GET /api/devices` (Bearer) and execs `ssh -t <host> -- cas attach <id>` (`cas-cli/src/cli/factory/remote_attach.rs:1-50`).
- **Zero-token status surfaces.** `worker_status`, `worker_activity`, `epic_status`, `spawn_workers` are MCP tools reading SQLite/PTY state (`cas-cli/src/mcp/tools/service/factory_ops.rs`); message delivery to workers is PTY injection (`daemon/runtime/delivery.rs` → `Mux::inject`) — identical cost to a human typing.

### Recommended shape (t3code's model, CAS's substrate)

Adopt t3code's **decentralized** topology: no central control plane, one authenticated endpoint per machine, a client that holds a list of known machines and fans out. Concretely:

**Per machine (Rust, in cas-src):**
1. Promote the daemon's loopback WS + the bridge HTTP into one **`cas hub`** endpoint per machine (or extend `cas bridge`): single configurable bind/port, serving (a) HTTP: list factory sessions on this machine (from session metadata files + `.cas/cas.db`), worker/epic status, SSE events; (b) WS: proxy attach to any local factory daemon's existing `DaemonMessage` stream (the hub dials the daemon's loopback `ws_port` so daemons stay loopback-only).
2. **Auth**: copy t3code's pairing design — one-time pairing token printed as URL/QR, exchanged for a session; WS upgrade authenticated by a short-lived ticket endpoint rather than a long-lived token in the URL. (`apps/server/src/auth/SessionStore.ts` + `docs/internals/environment-auth.md` are the reference implementation.)
3. **Transport security**: recommend tailnet-first exactly as t3code's docs do; optional `tailscale serve` integration is ~200 lines (`packages/tailscale/src/tailscale.ts` shows the shape: `tailscale serve --https=<port>` on, off, status-check).

**Client (one web app, new):**
4. A web SPA (servable from the hub itself and openable on phone/laptop) with: machine list → factory list → pane grid; ghostty-WASM or xterm.js surfaces fed by `DaemonMessage::Output`; input box → `ClientMessage::Input` (message injection = what the supervisor's `deliver_to_worker` already does); interrupt button → inject the interrupt byte sequence / call a bridge route; status sidebars fed by bridge SSE + `worker_status` JSON. Browser-local list of paired machines, t3-style.

**Centrally: nothing mandatory.** The existing CAS cloud `GET /api/devices` can serve as optional discovery (it already powers `remote_attach`). Do not build a relay first; tailnet covers the pippenz fleet.

**Quota safety (the hard constraint), by construction:**
- The GUI only ever attaches to PTYs the factory already runs, reads SQLite, and injects keystrokes. Zero model calls added.
- Push-only: WS Output frames and SSE; no polling loops that touch agents. (Status endpoints read the DB, not the workers.)
- N viewers of one pane = one PTY = one subscription session. Session count never multiplies.
- Explicitly out of scope: t3code's provider drivers. They *own* agent sessions; in CAS the factory daemon owns them. Running both would double session count and fight over the CLIs.

### What CAS lacks today (the actual work list)

1. Auth + non-loopback bind for the daemon WS (currently `127.0.0.1:0`, unauthenticated — `lifecycle.rs:66`).
2. A per-machine session index / hub that multiplexes N factory daemons behind one stable port (today: one ephemeral WS port per daemon, discoverable only via local session metadata).
3. A pairing flow (token mint, QR, session store, revocation — `cas bridge` has a static token, not sessions).
4. Any web UI at all (CAS is TUI-only).
5. A machine-level aggregate event stream (bridge SSE is per-session).
6. Unification: the daemon WS protocol and the bridge HTTP API grew separately; the hub should present one coherent surface.

---

## 6. Reuse verdict

**License allows forking anything (MIT).** The right split:

**Vendor/fork directly:**
- `apps/web/src/terminal/ghostty/` + `apps/web/scripts/build-libghostty-wasm.sh` — a working libghostty-WASM terminal component (keyCodes, surface, renderer, ABI shim). This is the hardest client piece and it's done. (Build needs zig; cas-src already vendors zig — `.context/zig`.)
- `native/resource-monitor/` — MIT Rust sysinfo NDJSON monitor; could drop into cas-src nearly unchanged if per-worker CPU/mem telemetry is wanted.
- `packages/shared/src/remote.ts` pairing-URL helpers and the `SessionStore` websocket-ticket pattern (translate to Rust; the design, TTLs, and hash-fragment token rules are the value).
- `packages/tailscale/src/tailscale.ts` — small, self-contained `tailscale serve` management; direct port to Rust.

**Pattern-inspiration only:**
- The remote model itself: ExecutionEnvironment identity file, four-target connection taxonomy, advertised-endpoint hints, "access vs launch" separation, client-local environment catalog (`docs/internals/remote.md` is effectively a design doc for the CAS hub).
- Connection supervisor policy: retry-forever with 16s-capped backoff, offline wakeups, auth failures block instead of retry (`docs/internals/connection-runtime.md`).
- Per-method authz scopes on one socket (`RPC_REQUIRED_SCOPE`).
- SSH launch flow (`packages/ssh/src/tunnel.ts`) — CAS's `remote_attach.rs` already does the terminal version; the tunnel-then-connect-WS version is the upgrade path.

**Do not reuse:**
- `apps/server` orchestration + provider drivers. It's an Effect-TS event-sourced runtime that assumes *it* is the execution boundary; CAS's daemon/mux/store already are. Adopting it would mean a second agent-owning harness (session multiplication — the exact failure mode to avoid) and a Node runtime dependency CAS doesn't have.
- Clerk/relay infra (`infra/relay`) — needs a Cloudflare+Clerk+PlanetScale deployment; wrong first move for a tailnet-sized fleet.
- The mobile app (nice reference for later; the web app on a phone is enough initially).

**Net: build the hub fresh in Rust inside cas-src (it's ~80% extension of existing bridge + daemon WS code), build a new thin web client, and fork t3code's ghostty terminal component and auth/pairing/tailscale patterns into it.**

---

## 7. Open questions

1. Should CAS cloud (`/api/devices`) become the optional discovery/roster layer for the hub, or is a browser-local machine list (t3-style) + tailnet MagicDNS names enough? (t3code ships without any central roster.)
2. Hub scope: panes + status only, or also task/epic/memory read surfaces from `cas.db` (bridge already exposes some)? Recommend read-only task/epic views in v1 — still zero tokens.
3. Interrupt semantics over the wire: raw ESC/Ctrl-C byte injection into the PTY vs a first-class daemon `ClientMessage::Interrupt{pane_id}` — the latter is cleaner and lets the daemon route through its existing interrupt handling.
4. Does the GUI need write-path approval prompts (Claude permission prompts render inside the PTY today, so pane input already covers it), or should hooks surface approvals as structured events later?
5. libghostty-WASM vs xterm.js for v1: ghostty is the better emulator and already integrated in t3code, but xterm.js is a plain npm install; decide after measuring the ghostty build's zig-version pinning against our vendored zig.
6. The t3code snapshot has no git history (zip) — if we fork the ghostty component, pull from the live GitHub repo (`github.com/pingdotgg/t3code`) to track upstream fixes, and preserve the MIT notice in vendored files.
