---
date: 2026-08-25
topic: opencode-harness-assessment
target: OpenCode 1.18.23 / Qwen3.8-Max
decision: conditional-go
---

# OpenCode harness assessment: Qwen3.8-Max

## Decision

**Conditional GO for an implementation and live-conformance phase; NO-GO for
advertising `cli=opencode` as supported today.** OpenCode has all primitives
needed to start a Cassy worker: an interactive TUI with a scripted initial
prompt, a public auto-approval mode, custom primary-agent prompts, local stdio
MCP, inherited environment, model selection, and lifecycle/tool plugin events.
The existing `cas-mux::Backend` seam is deep enough to contain most launch
differences without another portability framework.

Three gaps block supported production spawning:

1. A fresh OpenCode TUI session does not accept a caller-chosen session ID.
   `--session` resumes an existing session; OpenCode creates a fresh `ses_*`
   identity internally. Cassy therefore cannot make `CAS_SESSION_ID` and the
   harness transcript identity identical at spawn time.
2. Current OpenCode stores sessions in a shared SQLite database, not a
   per-session transcript file. Cassy's transcript/liveness paths need an
   OpenCode-session mapping and an adapter for session/tool activity.
3. Qwen3.8-Max exposes only `low`, `medium`, and `xhigh` reasoning-effort
   variants. Cassy's shared effort vocabulary also includes `minimal` and
   `high`; silently collapsing those values would make the spawn receipt lie.

The host used for this spike has no `opencode` executable. All OpenCode claims
below are grounded in official documentation and upstream source, but none is
a live Cassy factory-conformance result. The source snapshot reviewed was
OpenCode `1.18.23` at commit
[`a7444bf9`](https://github.com/anomalyco/opencode/tree/a7444bf944c219b9eaba2f794847b3001237795f).

This refines the earlier portability direction in
`docs/ideation/2026-04-28-cas-harness-portability-ideation.md`. Since that note,
Cassy has gained three real backend adapters and typed conformance receipts;
OpenCode should extend those seams rather than first introducing a capability
manifest or a new harness protocol.

## Upstream contract

### 1. Launch, permissions, cwd, and environment

OpenCode without a subcommand starts its interactive TUI. Its supported TUI
arguments include a project path, `--model provider/model`, `--prompt`,
`--agent`, `--session` for resume, and `--auto`. The current implementation
also accepts hidden `--yolo` and `--dangerously-skip-permissions` aliases, all
three feeding the same `auto` boolean. The public and supportable contract is
therefore:

```text
opencode <absolute-worktree> \
  --model alibaba/qwen3.8-max \
  --agent cassy-worker \
  --prompt '<factory startup prompt>' \
  --auto
```

Evidence: the [CLI documentation](https://opencode.ai/docs/cli/) and the
[`tui.ts` option/launch path](https://github.com/anomalyco/opencode/blob/a7444bf944c219b9eaba2f794847b3001237795f/packages/opencode/src/cli/cmd/tui.ts#L74-L123).
The implementation changes directory to the selected project and copies the
entire parent environment into its worker thread
([source](https://github.com/anomalyco/opencode/blob/a7444bf944c219b9eaba2f794847b3001237795f/packages/opencode/src/cli/cmd/tui.ts#L198-L214)).
Cassy should pass an absolute project path and also set `PWD` to the same path,
avoiding any relative-path ambiguity.

`--auto` is not an unconditional override: it approves requests that would
otherwise ask, while explicit deny rules remain enforced. OpenCode's
[permission contract](https://opencode.ai/docs/permissions/) also treats
`external_directory` separately. `PtyConfig::opencode` must inject a
higher-precedence worker agent with the required workspace permissions and
preflight the effective policy. Managed denials must fail the spawn with an
actionable error; they must not be bypassed.

`opencode run` is the official non-interactive/scripted mode and supports
`--format json`, `--dir`, `--model`, `--variant`, `--session`, and `--auto`
([CLI docs](https://opencode.ai/docs/cli/),
[`run.ts`](https://github.com/anomalyco/opencode/blob/a7444bf944c219b9eaba2f794847b3001237795f/packages/opencode/src/cli/cmd/run.ts#L137-L246)).
It exits when the session becomes idle, so it is appropriate for conformance
probes, not for Cassy's long-lived interactive worker pane.

**Verdict: GO**, conditional on effective-permission preflight. Use the TUI for
workers and `opencode run` for bounded probes.

### 2. Rules and role instructions

OpenCode automatically loads project and global `AGENTS.md`; it falls back to
project/global `CLAUDE.md` when the OpenCode-native file is absent. It also
supports additional instruction files in `opencode.json`
([rules documentation](https://opencode.ai/docs/rules/)). This repository's
generated `AGENTS.md` therefore already supplies project guidance.

Factory role instructions should use a generated custom **primary agent**, not
pretend the startup user prompt is a system prompt. OpenCode agents support a
custom `prompt`, `model`, `variant`, mode, and permission rules, and can be
selected at launch with `--agent`
([agents documentation](https://opencode.ai/docs/agents/)). A worker-specific
agent can be supplied through `OPENCODE_CONFIG_CONTENT`, which is the final
ordinary config override in OpenCode's documented merge order
([config documentation](https://opencode.ai/docs/config/)). This keeps the
role/model/effort/permission bundle process-local and avoids modifying the
user's project.

**Verdict: GO.** Generate `cassy-worker` and `cassy-supervisor` agent entries
in inline config, select one with `--agent`, and retain `--prompt` only for the
startup workflow that should execute immediately.

### 3. MCP and tool namespace

OpenCode supports local stdio MCP servers configured as a command array plus
optional environment. The minimum injected config is:

```json
{
  "mcp": {
    "cas": {
      "type": "local",
      "command": ["cas", "serve"],
      "enabled": true
    }
  }
}
```

The official [MCP documentation](https://opencode.ai/docs/mcp-servers/) covers
the config shape. Current source sets the MCP cwd to the project and constructs
its child environment from the complete OpenCode process environment followed
by configured overrides
([source](https://github.com/anomalyco/opencode/blob/a7444bf944c219b9eaba2f794847b3001237795f/packages/opencode/src/mcp/index.ts#L341-L357)).
Consequently `CAS_SESSION_ID`, `CAS_AGENT_NAME`, `CAS_AGENT_ROLE`,
`CAS_FACTORY_MODE`, `CAS_FACTORY_WORKER_CLI=opencode`, `CAS_ROOT`, and
`CAS_SUPERVISOR_NAME` can reach `cas serve` by normal inheritance.

OpenCode exposes an MCP tool as sanitized `<server>_<tool>`, not any existing
Cassy prefix. With server name `cas`, the worker sees `cas_task`,
`cas_coordination`, and `cas_verification`
([source](https://github.com/anomalyco/opencode/blob/a7444bf944c219b9eaba2f794847b3001237795f/packages/opencode/src/mcp/catalog.ts#L102-L120)).

**Verdict: GO.** Add `tool_prefix: "cas_"`, update prompt/policy parity, and
spawn-inject the local server through inline config so a project need not
already be OpenCode-integrated.

### 4. Qwen3.8-Max provider, model, auth, and effort

The direct international provider/model selector is
`alibaba/qwen3.8-max`. The Models.dev catalog currently declares provider
`alibaba`, API `https://dashscope-intl.aliyuncs.com/compatible-mode/v1`, auth
environment `DASHSCOPE_API_KEY`, and model `qwen3.8-max`. OpenCode consumes
Models.dev for provider/model discovery
([OpenCode model docs](https://opencode.ai/docs/models/),
[live catalog](https://models.dev/api.json)). Qwen's own getting-started guide
uses the same API key, endpoint, and model ID
([QwenCloud](https://docs.qwencloud.com/developer-guides/getting-started/first-api-call)).
The China-region selector is `alibaba-cn/qwen3.8-max`; it must be an explicit
operator choice rather than inferred.

Authentication can come from `DASHSCOPE_API_KEY` or `opencode auth login` /
`/connect`; OpenCode documents stored credentials at
`~/.local/share/opencode/auth.json`
([provider docs](https://opencode.ai/docs/providers/)). Secrets must not be
copied into task receipts or generated project files.

Qwen3.8-Max supports a one-million-token context, tool calling, and thinking
mode ([model card](https://www.alibabacloud.com/help/en/model-studio/qwen3-8-max)).
Its supported `reasoning_effort` values are exactly `low`, `medium`, and
`xhigh`, with `xhigh` the default; `reasoning_effort` and `thinking_budget`
cannot be combined
([thinking guide](https://docs.qwencloud.com/developer-guides/text-generation/thinking)).
The live Models.dev entry exposes those same three effort variants, and current
OpenCode converts Models.dev `reasoning_options` into model variants
([provider source](https://github.com/anomalyco/opencode/blob/a7444bf944c219b9eaba2f794847b3001237795f/packages/opencode/src/provider/provider.ts#L1260-L1314)).

The interactive TUI does not expose `--variant` in the reviewed version, but a
custom agent's documented `variant` field applies it. Proposed Qwen policy:

| Cassy request | OpenCode/Qwen variant | Result |
|---|---|---|
| omitted | omitted | Qwen default (`xhigh`) |
| `low` | `low` | exact |
| `medium` | `medium` | exact |
| `xhigh` | `xhigh` | exact |
| `minimal` | none | reject before spawn |
| `high` | none | reject before spawn |

Do not map `high` to `xhigh` or `minimal` to `low`: that would make the
resolved `WorkerSpec` and factory receipts disagree with the actual request.
This validation is model-aware; `Backend::effort_arg(Effort)` alone is not the
right place for it.

**Verdict: CONDITIONAL GO.** Model selection and auth are ready; supported
spawning requires model-aware effort validation and a preflight that proves
the requested provider is authenticated.

### 5. Session identity, transcript, liveness, and blame

OpenCode session IDs are `ses_*`. `--session` only resumes an existing session;
fresh TUI startup creates the ID internally. The CLI can list sessions as JSON
and export a complete session as JSON
([CLI session/export docs](https://opencode.ai/docs/cli/)). Current OpenCode
stores session/message/part data in `opencode.db` under its XDG data directory,
normally `~/.local/share/opencode/opencode.db`
([database source](https://github.com/anomalyco/opencode/blob/a7444bf944c219b9eaba2f794847b3001237795f/packages/core/src/database/database.ts#L43-L55)).
There is no per-session JSONL whose path can be derived from the synthetic
`CAS_SESSION_ID`.

Recommended adapter contract:

- Cassy continues generating a known `CAS_SESSION_ID` for MCP registration.
- A generated OpenCode plugin observes the first root `session.created` event
  in this process and persists `{cas_session_id, opencode_session_id,
  directory}` in Cassy's session state.
- Awaited `tool.execute.before`/`tool.execute.after` hooks produce attribution
  and activity signals. `session.status`/`session.idle` events update a
  per-session liveness signal, with process liveness retained as fallback.
- Transcript/blame uses `opencode export <opencode_session_id>` or a bounded
  read-only database projection, never global database mtime.

The generic plugin `event` callback is fire-and-forget in the reviewed source
([source](https://github.com/anomalyco/opencode/blob/a7444bf944c219b9eaba2f794847b3001237795f/packages/opencode/src/plugin/index.ts#L255-L260)),
so the mapping/signal writer must be idempotent and liveness must degrade to
process/heartbeat evidence if an event write is delayed.

**Verdict: NO-GO until the mapping and liveness adapter passes live
conformance.** Basic MCP task work could start without it, but Cassy would not
meet its supported-harness supervision and blame contract.

### 6. Hooks and lifecycle events

OpenCode plugins provide awaited `permission.ask`, `tool.execute.before`,
`tool.execute.after`, `shell.env`, `chat.message`, and experimental system/
compaction transforms, plus a generic event stream
([plugin interface](https://github.com/anomalyco/opencode/blob/a7444bf944c219b9eaba2f794847b3001237795f/packages/plugin/src/index.ts#L222-L308)).
Documented session events include `session.created`, `session.updated`,
`session.status`, `session.idle`, `session.error`, and `session.deleted`
([plugin docs](https://opencode.ai/docs/plugins/)).

This is not Claude Code's JSON command-hook protocol. Replaying Cassy's Claude
hook configuration would be incorrect. A thin OpenCode plugin should translate
only the needed semantics into existing Cassy entry points/signals. In
particular, `tool.execute.before` can support attribution and Cassy gates;
`permission.ask` sees only decisions that reached the ask state, so it cannot
replace all pre-tool policy by itself.

**Verdict: CONDITIONAL GO.** The event surface is sufficient, but it needs a
real adapter and conformance fixtures rather than a capability flag based on
documentation alone.

## Cassy touchpoint matrix

| Touchpoint | Verdict | Required change / evidence |
|---|---|---|
| `crates/cas-pty/src/pty.rs` | GO | Add `PtyConfig::opencode`: command `opencode`, absolute project arg, `--model`, `--agent`, `--prompt`, `--auto`; inject inline agent + MCP config and unconditional `CAS_FACTORY_WORKER_CLI=opencode`. Set `PWD` to cwd. |
| `crates/cas-mux/src/backend/mod.rs` + new `backend/opencode.rs` | GO | Register the fourth adapter in the existing `Backend` seam. Candidate capabilities: hooks/subagents true only after plugin conformance; textbox/paste/cancel bytes must be measured live, not copied from another harness. |
| `crates/cas-mux/src/harness.rs` | GO | Add `SupervisorCli::OpenCode`, parse/serde `opencode`, prefix `cas_`, and measured injection/cancel capability tests. |
| `crates/cas-mux/src/spec.rs` | CONDITIONAL | Add OpenCode to round trips and docs. Preserve full `provider/model`. Add model-aware variant validation rather than forcing Qwen through the global five-value effort mapping. |
| `cas-cli/src/harness_policy.rs` | GO | Add `cas_coordination` / `cas_verification`, `own_tool_prefix() == "cas_"`, env parsing, and all-role tests. |
| `CAS_FACTORY_WORKER_CLI` + spawn queue | GO | Existing string/env plumbing is reusable once parsing accepts `opencode`; persist the value on the agent so later liveness/blame dispatch does not default to Claude. |
| `cas-cli/src/mcp/tools/service/factory_ops.rs` | CONDITIONAL | Accept `cli=opencode`; default to `alibaba/qwen3.8-max`; treat OpenCode as a multi-provider harness in model/CLI validation; add auth/account preflight and exact Qwen effort errors. |
| Account `config_dir` / requester capture | NO-GO as-is | `OPENCODE_CONFIG_DIR` does not relocate `auth.json`, session DB, or state. Define `config_dir` for OpenCode as an account root that sets `XDG_CONFIG_HOME=<root>/config`, `XDG_DATA_HOME=<root>/data`, `XDG_STATE_HOME=<root>/state`, and `XDG_CACHE_HOME=<root>/cache`; capture the root in a Cassy-owned env such as `CAS_OPENCODE_ACCOUNT_DIR`. Preflight `<root>/data/opencode/auth.json` or an allowed provider env key. |
| Transcript/liveness/blame | NO-GO as-is | Add the CAS↔OpenCode session mapping and per-session activity adapter; update worker-status/transcript resolution, wedge detection, and attribution. A shared SQLite/WAL mtime is not session evidence. |
| `crates/cas-pty/src/conformance.rs` + `cas-cli/src/factory_preflight.rs` | NO-GO until live run | Add `Harness::OpenCode`, version probe, typed receipt loading, and a required matrix. This host cannot generate the receipt because `opencode` is absent. |
| Builtin skills/agents and sync | CONDITIONAL | OpenCode can fall back to Claude skills, but supported parity should add an OpenCode projection using `cas_` names and a generated primary-agent prompt. Update flavor-drift tests across all four mirrors. |
| User-visible docs/release notes | REQUIRED on implementation merge | Document `cli=opencode`, full provider/model selectors, account-root meaning, effort subset, and conformance pin. This spike alone is an assessment and does not announce runtime support. |

## Adapter seam and alternatives

The selected seam is the existing `cas_mux::Backend` interface plus one
OpenCode-local plugin/config projection. Callers should learn only the stable
facts already present in `WorkerSpec`: `cli`, full model selector, effort, and
account root. OpenCode-specific argv, inline JSON, tool prefix, role prompt,
session mapping, and XDG layout stay behind the adapter.

Three interfaces were considered:

1. **Minimum surface:** add only `PtyConfig::opencode`, mark it hookless, use a
   synthetic CAS session ID, and rely on process liveness. Smallest diff, but it
   cannot satisfy transcript/blame or prove the requested Qwen effort; deleting
   it merely redistributes OpenCode branches into policy and liveness callers.
2. **Maximum flexibility:** implement the earlier portability note's versioned
   capability manifest, canonical hook taxonomy, and generic projection system
   first. It can model every OpenCode feature, but it expands the public
   interface before a fourth adapter has exposed a missing common abstraction.
3. **Common-caller adapter (recommended):** extend `Backend`, add a private
   OpenCode agent/MCP projection, and add one explicit session-activity adapter.
   It gives current factory callers the same interface while localizing the two
   genuinely new complexities: model-aware variants and non-file transcripts.

The deletion test favors option 3: deleting `backend/opencode.rs` and its
plugin removes the harness while leaving existing spawn callers unchanged;
without those modules, inline config, XDG isolation, prefix translation, and
session mapping would reappear across `factory_ops`, mux, and liveness code.

## Proposed implementation tasks

Sizes assume one experienced Cassy worker and scoped tests; live provider time
and upstream account setup are separate calendar risks.

| Task | Size | Deliverable / exit condition |
|---|---:|---|
| 1. Core selector + launch adapter | M (2–3 days) | `SupervisorCli::OpenCode`, `backend/opencode.rs`, `PtyConfig::opencode`, env/prefix/argv tests, absolute cwd/PWD, startup prompt, inline MCP/agent config. No support claim yet. |
| 2. Spawn/policy/model plumbing | M (2–3 days) | `spawn_workers cli=opencode`; full `provider/model`; `cas_` guidance; OpenCode-aware model validation; Qwen low/medium/xhigh validation; agent metadata round trip. |
| 3. Account-root and auth preflight | M (2 days) | Defined four-XDG `config_dir` layout, requester-root capture, tilde/path validation, direct Alibaba and `alibaba-cn` auth checks, no secret receipts. |
| 4. OpenCode role/plugin projection | L (3–5 days) | Generated primary agents for worker/supervisor, required permissions, awaited tool attribution, CAS↔OpenCode session mapping, per-session status/activity signals, idempotent event handling. |
| 5. Liveness/blame integration | L (3–5 days) | Worker-status/wedge/attribution paths resolve the mapped `ses_*`; bounded transcript export/read; reverse tests for busy→idle, cancel, crash, missing/delayed mapping, and account isolation. |
| 6. Builtin + docs parity | M (2–3 days) | OpenCode skill/agent projection, four-flavor drift tests, reference/model-selection docs, CLI error text, and runtime release-note assessment. |
| 7. Live conformance and preflight pin | M (2–3 days) | Install a pinned OpenCode binary, run the matrix below against `alibaba/qwen3.8-max`, persist a typed receipt, add preflight version probing, then and only then expose support. |

Tasks 1–3 are independently assignable after the selector shape is agreed.
Task 4 precedes task 5. Task 7 gates the support claim and should run after
tasks 1–6.

### Required conformance matrix

The typed OpenCode receipt should fail closed unless every required check
passes:

- exact CLI version and executable path;
- interactive PTY boot, initial prompt submission, large injected follow-up,
  cancel bytes, and post-cancel readiness;
- `--auto` plus effective required permissions, with an explicit deny retained;
- absolute cwd/PWD and inherited CAS identity environment;
- custom role prompt and project `AGENTS.md` visible in model context;
- local `cas serve` discovers the expected tools as `cas_*` and completes
  whoami/mine/show/start/notes/commit/push/close in a disposable repo;
- `alibaba/qwen3.8-max` auth, tool calling, and each supported effort variant
  observed in request/session metadata;
- CAS↔`ses_*` mapping, transcript export, busy/tool/idle signals, crash/cancel,
  and wedge classification;
- two isolated account roots do not share auth, model state, or session rows;
- OpenCode subagent behavior and task attribution if `supports_subagents=true`;
- no regressions in Claude, Codex, or Grok launch/prefix/injection parity.

## Search manifest

Research was performed on 2026-08-25 with Exa queries restricted to official
OpenCode/Qwen/Alibaba documentation and upstream source, followed by direct
reads of the exact OpenCode `dev` commit above and the live Models.dev catalog.

| Area | Queries / paths reviewed | Primary sources retained |
|---|---|---|
| Prior Cassy direction | `docs/ideation/2026-04-28-cas-harness-portability-ideation.md` | local ideation note |
| Cassy launch seams | `crates/cas-pty/src/pty.rs`; `crates/cas-mux/src/{backend,harness,spec,pane,mux}`; `cas-cli/src/harness_policy.rs`; factory spawn/preflight/liveness paths | repository source at task branch |
| Launch + permissions | `OpenCode official docs CLI non-interactive permission bypass cwd env`; exact `tui.ts`/`run.ts` | [CLI](https://opencode.ai/docs/cli/), [permissions](https://opencode.ai/docs/permissions/), pinned upstream source |
| Instructions + agents | `OpenCode rules instructions system prompt AGENTS.md agent prompt variant` | [rules](https://opencode.ai/docs/rules/), [agents](https://opencode.ai/docs/agents/), [config](https://opencode.ai/docs/config/) |
| MCP | `OpenCode MCP local stdio environment tool namespace`; exact `mcp/index.ts` and `mcp/catalog.ts` | [MCP docs](https://opencode.ai/docs/mcp-servers/), pinned upstream source |
| Qwen target | `Qwen 3.8 official model API id reasoning effort`; Models.dev provider entries for `qwen3.8-max` | [Qwen model](https://www.alibabacloud.com/help/en/model-studio/qwen3-8-max), [Qwen auth/API](https://docs.qwencloud.com/developer-guides/getting-started/first-api-call), [thinking](https://docs.qwencloud.com/developer-guides/text-generation/thinking), [Models.dev](https://models.dev/api.json) |
| Sessions + lifecycle | `OpenCode session transcript storage sqlite plugin events`; exact database/auth/plugin/session source | [CLI session/export](https://opencode.ai/docs/cli/), [plugins](https://opencode.ai/docs/plugins/), pinned upstream source |
| Account layout | `OpenCode OPENCODE_CONFIG_DIR auth.json XDG_DATA_HOME`; exact `global.ts`, `auth/index.ts`, database source | [providers](https://opencode.ai/docs/providers/), [config](https://opencode.ai/docs/config/), pinned upstream source |

No community blog or issue report is used as the basis for a GO decision. GitHub
issues found during search were used only to locate upstream source and were
then replaced by documentation or exact-code evidence.
