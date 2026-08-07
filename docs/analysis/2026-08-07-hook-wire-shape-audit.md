# Hook payload wire-shape audit (cas-f3e3, follow-up to cas-78d3 / GH #165)

**Status: AUDIT COMPLETE — fixes NOT yet applied.** Checkpointed at low context for a fresh worker.

## Why this audit exists

cas-78d3: the `UserPromptSubmit` handler was dead for a full release because `HookInput`
aliased the wire key `prompt` onto the wrong field. All seven cas-7a01 surfacing tests passed
throughout, because every one built `HookInput` by struct literal and never exercised the
deserialization contract. This sweep asks the same question of every other hook event.

## Method

Wire keys taken from the harness's own documented contract — https://code.claude.com/docs/en/hooks
(`docs.claude.com/en/docs/claude-code/hooks` 301-redirects there) — deliberately **not** inferred
from CAS's own structs, since that circularity is what hid GH #165. CAS side read from
`crates/cas-core/src/hooks/types.rs`.

Serde note that shapes every verdict below: unknown JSON keys are **ignored**, and every field is
`#[serde(default)]`. So a key CAS does not declare is silently dropped, never an error. A missing
declaration is therefore only a *defect* where a handler needs the value — which is exactly what
makes this class of bug invisible.

## Table

| Wire key | Event(s) | CAS field | Verdict |
|---|---|---|---|
| `session_id` | all | `session_id` (+alias `sessionId`) | OK |
| `transcript_path` | all | `transcript_path` (+alias `transcriptPath`) | OK |
| `cwd` | all | `cwd` | OK |
| `hook_event_name` | all | `hook_event_name` (+alias `hookEventName`) | OK |
| `permission_mode` | all | `permission_mode` (+alias `permissionMode`) | OK |
| `prompt_id` | all | — | absent; not consumed — no impact |
| `source` | SessionStart | `source` | OK |
| `reason` | SessionEnd | `reason` | OK |
| `prompt` | UserPromptSubmit | `user_prompt` (+alias `prompt`) | OK — **fixed by cas-78d3** |
| `tool_name` | Pre/PostToolUse | `tool_name` (+alias `toolName`) | OK |
| `tool_input` | Pre/PostToolUse | `tool_input` (+alias `toolInput`) | OK |
| `tool_use_id` | Pre/PostToolUse | `tool_use_id` (+alias `toolUseId`) | OK |
| `tool_response` | PostToolUse | `tool_response` (+alias `toolResult`) | OK — field name matches the wire key; the `toolResult` alias is harmless surplus |
| `message` | Notification | `message` | OK |
| `agent_id` | SubagentStart/Stop | `agent_id` (+alias `agentId`) | OK |
| `agent_type` | SubagentStart/Stop | `agent_type` (+alias `agentType`) | OK |
| **`stop_hook_active`** | **Stop, SubagentStop** | **— none —** | **MISMATCH — see below** |
| `trigger` | PreCompact | — none — | MISMATCH (latent) |
| `custom_instructions` | PreCompact | — none — | MISMATCH (latent) |
| `last_assistant_message` | Stop, SubagentStop | — none — | absent; not consumed — no impact |
| `agent_transcript_path` | SubagentStop | — none — | absent; not consumed — no impact |
| `notification_type`, `title` | Notification | — none — | absent; not consumed — no impact |
| `subagent_type` | — | `subagent_type` (+alias `subagentType`) | CAS-side only: docs give the subagent's type as `agent_type`. Declared field appears to match no real wire key — verify before relying on it. |

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

**Fix shape (cas-78d3 pattern):** declare `stop_hook_active: Option<bool>` with a doc comment
citing the loop-prevention contract, add an accessor, and have `stop_flow.rs` decline to block when
it is true. Needs a wire-shape regression test, and a decision on whether "already continuing"
should suppress all five blockers or only the re-entrant ones.

## Finding 2 — PreCompact `trigger` / `custom_instructions` undeclared (LATENT)

Both are documented (`trigger` ∈ {`manual`, `auto`}; `custom_instructions` carries the user's
`/compact` text, empty for `auto`). Neither is declared or read. `handle_pre_compact` currently
cannot distinguish a user-initiated compaction from an automatic one, nor see the user's
instructions. No present-day defect — nothing consumes them — so this is a capability gap, not a
live bug. Declare them when a handler needs them; do not add dead fields for their own sake.

## Finding 3 — `subagent_type` may correspond to no real wire key (VERIFY)

The docs give the subagent's type as `agent_type` on SubagentStart/SubagentStop. CAS declares both
`agent_type` and a separate `subagent_type` (+alias `subagentType`). The latter may be vestigial.
Confirm against a captured live payload before removing — the docs' per-event key lists for
SubagentStart were not fully extracted in this pass.

## Not confirmed by documentation (open, do not guess)

- Full key lists for `SubagentStart`, `PermissionRequest`, `PermissionDenied`, `PostToolUseFailure`,
  `MessageDisplay`, `StopFailure`. All are documented events with their own input sections, but the
  complete field lists were not extracted.
- The exact enum for `permission_mode` as sent to hooks (vs the `--permission-mode` CLI flag).
- Whether `SessionStart.source` is exactly {`startup`, `resume`, `clear`, `compact`, `fork`}.

The best evidence for all of these is a **captured live payload**, not docs. A cheap way to get one:
temporarily point a hook command at a script that appends stdin to a file, take one turn, read it.

## Guard (deliverable 2) — not yet implemented

Convention to adopt: **hook-payload tests must parse raw JSON in the real wire shape.** A test that
builds `HookInput` by struct literal cannot catch a deserialization-contract bug — proven by GH #165,
where seven such tests stayed green across a full release while the feature was dead. Struct
literals remain fine for tests about handler *logic* once the payload contract is separately pinned.

Still to do: create the CAS rule, and convert the existing struct-literal tests that guard
deserialization (`cas-cli/src/hooks/handlers/handlers_tests/`, `cas-cli/tests/hook_schema.rs`).
