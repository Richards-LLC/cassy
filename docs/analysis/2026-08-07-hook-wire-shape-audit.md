# Hook payload wire-shape audit (cas-f3e3, follow-up to cas-78d3 / GH #165)

**Status: COMPLETE — audit re-grounded on CAPTURED LIVE PAYLOADS, fixes applied.**

> **Revision 2 (live capture).** The first pass built the table from the harness's published
> docs. The supervisor required re-grounding it on captured wire, on the reasoning that trusting
> docs over the wire is the same error class that caused GH #165. That was the right call: the
> capture **contradicted the docs on MessageDisplay** and exposed a second dead handler. Rows
> below are now marked with their evidence — `WIRE` (observed in a capture) or `DOC` (documented
> only, not yet observed).

## Why this audit exists

cas-78d3: the `UserPromptSubmit` handler was dead for a full release because `HookInput`
aliased the wire key `prompt` onto the wrong field. All seven cas-7a01 surfacing tests passed
throughout, because every one built `HookInput` by struct literal and never exercised the
deserialization contract. This sweep asks the same question of every other hook event.

## Method

**Primary evidence: captured live payloads** (Claude Code **2.1.224**). Secondary: the harness's
published contract — https://code.claude.com/docs/en/hooks. Deliberately **not** inferred from
CAS's own structs, since that circularity is what hid GH #165. CAS side read from
`crates/cas-core/src/hooks/types.rs`.

### How the capture was taken (reproducible, and safe to re-run)

An isolated Claude config dir was built at `/tmp/wirecap`:

- `.credentials.json` copied from an already-authenticated config dir;
- `settings.json` containing **only** a capture hook, registered for all 15 known events, whose
  command is a script that appends stdin to `payloads.jsonl` and exits 0;
- **no CAS hooks and no MCP servers**, so the running fleet was never touched and no `cas hook`
  ran during capture.

Then two headless turns in a scratch cwd:

    CLAUDE_CONFIG_DIR=/tmp/wirecap claude -p "<prompt>" --allowedTools "Bash,Task"

Run 1 (bash call + a `Task` subagent) yielded SessionStart, UserPromptSubmit, MessageDisplay,
PreToolUse, PostToolUse, SubagentStart, SubagentStop, Stop, SessionEnd. Run 2 (a command exiting
non-zero) added PostToolUseFailure.

The later interactive session captured `Notification`, `PreCompact`, and
`PermissionRequest`. `PermissionDenied` and `StopFailure` remained absent even after the operator
denied a network request and attempted an Esc interrupt, so they remain `DOC` with those explicit
capture limits recorded below.

Every captured payload is reproduced verbatim as a test fixture in
`crates/cas-core/src/hooks/types.rs` (`mod tests`), so the evidence lives in CI, not only here.

Serde note that shapes every verdict below: unknown JSON keys are **ignored**, and every field is
`#[serde(default)]`. So a key CAS does not declare is silently dropped, never an error. A missing
declaration is therefore only a *defect* where a handler needs the value — which is exactly what
makes this class of bug invisible.

## Table

| Wire key | Event(s) | Evidence | CAS field | Verdict |
|---|---|---|---|---|
| `session_id` | all | WIRE | `session_id` (+alias `sessionId`) | OK |
| `transcript_path` | all | WIRE | `transcript_path` (+alias `transcriptPath`) | OK |
| `cwd` | all | WIRE | `cwd` | OK |
| `hook_event_name` | all | WIRE | `hook_event_name` (+alias `hookEventName`) | OK |
| `permission_mode` | most | WIRE | `permission_mode` (+alias `permissionMode`) | OK — observed value `"default"`. Absent on SessionStart/SubagentStart. |
| `prompt_id` | nearly all | WIRE | — none — | undeclared, unconsumed — no impact |
| `source` | SessionStart | WIRE | `source` | OK — observed `"startup"` |
| `reason` | SessionEnd | WIRE | `reason` | OK — observed `"other"` |
| `prompt` | UserPromptSubmit | WIRE | `user_prompt` (+alias `prompt`) | OK — **fixed by cas-78d3**, now re-confirmed on the wire |
| `tool_name` | Pre/PostToolUse, PostToolUseFailure, PermissionRequest | WIRE | `tool_name` (+alias `toolName`) | OK — verified on interactive `Write` and `Bash` permission prompts. |
| `tool_input` | Pre/PostToolUse, PostToolUseFailure, PermissionRequest | WIRE | `tool_input` (+alias `toolInput`) | OK — `handle_permission_request` reads `file_path` from it. |
| `tool_use_id` | Pre/PostToolUse, PostToolUseFailure | WIRE | `tool_use_id` (+alias `toolUseId`) | OK |
| `tool_response` | PostToolUse | WIRE | `tool_response` (+alias `toolResult`) | OK — field name matches the wire key; the `toolResult` alias is Grok-only surplus |
| `duration_ms` | PostToolUse, PostToolUseFailure | WIRE | — none — | undeclared, unconsumed — no impact |
| `error`, `is_interrupt` | PostToolUseFailure | WIRE | — none — | undeclared. `PostToolUseFailure` routes to `handle_verifier_spawn_cleanup`, which needs neither. No impact today; `error` is the obvious thing a future consumer would want. |
| **`delta`** | **MessageDisplay** | **WIRE** | **`message` (+alias `delta`)** | **was MISMATCH — FIXED, see Finding A** |
| `final`, `index` | MessageDisplay | WIRE | `message_is_final` (rename `final`), `index` | newly declared — see Finding A |
| `message_id`, `turn_id` | MessageDisplay | WIRE | — none — | undeclared, unconsumed — no impact |
| **`stop_hook_active`** | **Stop, SubagentStop** | **WIRE** | **`stop_hook_active` (+alias `stopHookActive`)** | **was MISMATCH — FIXED, see Finding 1.** Observed value `false` in both captures. |
| `last_assistant_message` | Stop, SubagentStop | WIRE | — none — | undeclared, unconsumed — no impact |
| `background_tasks`, `session_crons` | Stop, SubagentStop | WIRE | — none — | undeclared, unconsumed — no impact. `background_tasks` entries carry `{id, type, status, description, agent_type}`. |
| `agent_transcript_path` | SubagentStop | WIRE | — none — | undeclared, unconsumed — no impact |
| `agent_id` | SubagentStart/Stop | WIRE | `agent_id` (+alias `agentId`) | OK |
| `agent_type` | SubagentStart/Stop | WIRE | `agent_type` (+alias `agentType`) | OK — observed `"general-purpose"` |
| *(no key)* | SubagentStart/Stop | WIRE | `subagent_type` (+alias `subagentType`) | **DEAD FIELD — Finding 3 resolved.** No such key is sent; the real one is `agent_type`. |
| *(no key)* | SubagentStart | WIRE | `subagent_prompt` | DEAD FIELD. SubagentStart sends no prompt at all — independent proof that cas-78d3's alias move cost nothing. |
| `message` | Notification | WIRE | `message` | **verified in interactive capture** — text is `"Claude is waiting for your input"`; no current handler reads it. |
| `notification_type` | Notification | WIRE | — none — | verified value `"idle_prompt"`; undeclared, unconsumed — no impact. |
| `title` | Notification | DOC | — none — | documented, but absent from the captured idle-notification envelope; undeclared, unconsumed — no impact. |
| `permission_suggestions` | PermissionRequest | WIRE | — none — | observed on both interactive prompts; undeclared, unconsumed — no impact. |
| `trigger` | PreCompact | WIRE | — none — | observed value `"manual"`; undeclared, unconsumed (latent — Finding 2). |
| `custom_instructions` | PreCompact | WIRE | — none — | observed as `null`; undeclared, unconsumed (latent — Finding 2). |

Grok Build's camelCase envelope (`hookEventName`, `sessionId`, `toolResult`, `workspaceRoot`,
`toolInputTruncated`) is unchanged by this pass and is still covered by
`test_parse_grok_post_tool_use_input`.

## Finding A — MessageDisplay sends `delta`, not `message` (REAL — the capture's headline)

**This is the finding the doc-only pass missed, and the reason the captured-wire mandate was
right.** The docs describe MessageDisplay as carrying the assistant message; the wire does not
send a key called `message` at all. Captured payload:

    {"session_id":…, "transcript_path":…, "cwd":…, "prompt_id":…,
     "hook_event_name":"MessageDisplay",
     "turn_id":…, "message_id":…, "index":0, "final":true,
     "delta":"I'll run the bash command and spawn the agent."}

The text arrives under **`delta`**, and it is a **stream chunk** — `index` plus a `final` flag —
not a whole message.

Consequence before the fix: `HookInput::message` was `None` on every MessageDisplay event, so
`handle_message_display` returned at `message_display.rs:39-42` every time. The entire cas-97ba
React-Ink Box-in-Text crash guard and the secret-redaction pass were **unreachable in
production** — the identical failure shape as GH #165, hidden the identical way (all four of its
tests built `HookInput` by struct literal).

**Severity, bounded honestly:** the guard ships default-off behind `[hooks]
message_display_guard`, so nothing regressed for users. What is true is that the feature could
never have worked if anyone had switched it on. No incident; a dead feature.

**Fix applied:** alias `delta` onto `message` (`Notification` keeps using `message`, so both
spellings must work and the alias is additive); declare `final` as `message_is_final` and
`index`; read through a new `HookInput::display_text()` which returns `None` for a blank chunk
**and for a non-final chunk**. The chunk rule is a real consequence of the wire shape, not
gold-plating: both transforms reason over a whole message, and a fence opened in chunk N or a
secret split across a chunk boundary cannot be judged from one chunk. An absent `final` is
treated as final, so a harness that omits it is not silently skipped.

**Not verified, and not claimed:** the headless captures produced single-chunk messages
(`index: 0`, `final: true`). Multi-chunk streaming behaviour — in particular whether the renderer
applies `updatedMessage` per chunk — was not observed. The final-chunk gate is the conservative
reading; if interactive capture later shows multi-chunk deltas, revisit whether a
last-chunk-only transform is sufficient.

## Finding 1 — `stop_hook_active` is never read, and CAS blocks Stop in five places (REAL)

`stop_hook_active` is documented as `true` when Claude Code is **already continuing as the result
of a stop hook**. It is the harness's loop-prevention signal, and the docs say to check it to avoid
infinite hook loops.

Evidence:
- `grep -rn "stop_hook_active"` over `crates/cas-core/src` and `cas-cli/src` → **zero hits.** The
  key is not declared on `HookInput` and is read nowhere.
- CAS blocks Stop in five places:
  `cas-cli/src/hooks/handlers/handlers_middle/session_stop/stop_flow.rs:46` (`block_stop`) and
  `:584`, `:594`, `:604`, `:615` (`block_stop_with_context`).

So CAS can block Stop repeatedly with no knowledge that it is already inside a stop-hook-induced
continuation.

**Honest severity bound.** Whether this actually loops depends on whether the blocking condition
self-clears once Claude continues. If a blocker is satisfied by the extra work (e.g. the agent
closes the open task it was blocked on), it terminates. If it is not (a condition Claude cannot
clear by continuing), nothing else stops it — `stop_hook_active` is the only brake and CAS has not
wired it. I did **not** reproduce a live loop; this is a code-level hazard with the mechanism named,
not a measured incident. Do not report it as an observed loop.

**Confirmed on the wire.** The key really is sent, on **both** `Stop` and `SubagentStop`, value
`false` in both captured turns.

**Fix applied.** `stop_hook_active: Option<bool>` (+alias `stopHookActive`) declared with the
loop-prevention contract in its doc comment, read through `HookInput::stop_hook_is_reentrant()`,
and honoured at all five block sites in `stop_flow.rs` — the `block_exit_on_open` guard at the top
and the four maintenance blockers, which are gated together by `if !is_factory_worker &&
!stop_is_reentrant`.

**The open design question, answered: all five, not just the re-entrant ones.** All four
maintenance blockers ask the agent to spawn a subagent (learning-reviewer, rule-reviewer,
duplicate-detector, session-summarizer). If the agent declines, the underlying condition is
unchanged at the next Stop and the same block fires again — they are precisely the
non-self-clearing class the harness's brake exists for. The exit-blocker site is included on the
same reasoning: an open task the agent will not close does not close itself.

**Deliberately NOT gated:** `handle_loop_iteration` (stop_flow.rs:58-62), which also returns a
block. That is the `cas loop` feature, where re-blocking is the entire point, and it has its own
termination bound in `max_iterations`. Suppressing it on `stop_hook_active` would break the
feature rather than protect it.

**Severity unchanged and still bounded: no live loop was ever reproduced.** This wires the
documented brake. It is not a fix for a measured incident and must not be reported as one.

## Finding 2 — PreCompact `trigger` / `custom_instructions` undeclared (LATENT)

Both are documented (`trigger` ∈ {`manual`, `auto`}; `custom_instructions` carries the user's
`/compact` text, empty for `auto`). Neither is declared or read. `handle_pre_compact` currently
cannot distinguish a user-initiated compaction from an automatic one, nor see the user's
instructions. No present-day defect — nothing consumes them — so this is a capability gap, not a
live bug. Declare them when a handler needs them; do not add dead fields for their own sake.

## Finding 3 — `subagent_type` corresponds to no real wire key (RESOLVED)

Confirmed against captured `SubagentStart` and `SubagentStop` payloads: both carry `agent_id` and
`agent_type`, and neither carries `subagent_type` or `subagentType`. The field is dead on arrival
and must not gate behaviour. Same verdict for `subagent_prompt`: SubagentStart sends no prompt
field at all.

**Not deleted, deliberately.** Removing the fields would churn a large number of unrelated struct
literals across `cas-cli/src/mcp/**` and the test tree for no behavioural gain, which the task
scope forbids. Instead both carry a doc comment naming them dead, and
`subagentstart_payload_uses_agent_type_not_subagent_type` asserts they stay `None` on the real
payload — so nobody can re-derive a dependency on them without a red test.

**Do not confuse this with the `subagent_type` key that genuinely exists**: that one is a member
of the `tool_input` object for the `Task`/`Agent` tool (read as JSON in `code_review_dispatch.rs`),
which is an entirely different thing from a top-level `HookInput` field.

## Still not observed (open — do NOT guess)

The interactive follow-up captured the following additional events. Their verbatim raw envelopes
are retained under `crates/cas-core/src/hooks/fixtures/` so they survive capture cleanup:

- `Notification` is now captured (see the table). The exact interactive `idle_prompt` envelope was
  `{session_id, transcript_path, cwd, prompt_id, hook_event_name:"Notification",
  message:"Claude is waiting for your input", notification_type:"idle_prompt"}`. The earlier
  task wording was stale: current `handle_notification` switches on `hook_event_name` and does
  **not** read `input.message`; the verified `message` field is therefore not a live consumer key.
- `PreCompact` — the operator's `/compact` emitted `{trigger:"manual",
  custom_instructions:null}`. Both keys remain undeclared because `handle_pre_compact` consumes
  neither; this confirms Finding 2 is a latent capability gap, not a live key mismatch.
- `PermissionRequest` — approved `Write` and denied `Bash`/`curl` attempts both emitted
  `{tool_name, tool_input, permission_suggestions}`. The handler's consumed `tool_name` and
  `tool_input.file_path` keys match the wire; `permission_suggestions` is unconsumed.
- `PermissionDenied` — **DOC retained**: the operator denied the `curl` permission prompt, but
  Claude Code 2.1.224 emitted no `PermissionDenied` envelope in the capture.
- `StopFailure` — **DOC retained**: the operator attempted Esc during a response, but no
  `StopFailure` envelope was emitted; only normal `Stop`/`SubagentStop` records appeared.

Observed-but-not-exhaustive enums: `permission_mode` was only ever `"default"`;
`SessionStart.source` only `"startup"`; `SessionEnd.reason` only `"other"`. One capture cannot
enumerate a domain — these are single observations, not confirmations of the full set.

## Guard (deliverable 2) — implemented

Convention adopted, and recorded as a CAS project rule so future hook tests inherit it:
**hook-payload tests must parse raw JSON in the real wire shape**, and the fixture must come from
a captured live payload — never from docs, and never read off CAS's own structs. A test that
builds `HookInput` by struct literal cannot catch a deserialization-contract bug: proven twice
now, by GH #165 (seven green tests, dead feature, full release) and by Finding A above (four
green tests, dead feature). Struct literals remain fine for tests about handler *logic*, once the
payload contract is separately pinned by a parse test.

Converted to the parse path:

- `cas-cli/src/hooks/handlers/handlers_tests/message_display.rs` — `md_input` now parses the
  captured MessageDisplay envelope. This is the file whose struct literal hid Finding A.
- `cas-cli/tests/hook_schema.rs` — `base_input` now parses the captured common envelope,
  including the undeclared `prompt_id`, so it also pins that an unknown harness key can never
  make a hook fail to deserialize.
- `crates/cas-core/src/hooks/types.rs` — captured payloads for MessageDisplay, Stop, SubagentStop,
  SubagentStart, SessionStart, SessionEnd, PostToolUse and PostToolUseFailure added verbatim as
  parse tests, so the capture evidence lives in CI rather than only in this document.
