# L3 — Runtime prompts in Rust: hook injections, spawn/startup envelopes, MCP tool descriptions

Lane L3 of the skills and prompts audit, 2026-09. This is a findings-only report: no repository code was edited and no cargo command was run.
Code base: `4836e56f7` (v3.31.0). The installed `cas` binary (`cas 3.31.0 (4836e56 2026-09-24)`) is that same commit, so the live measurements below
match the source.
Scored against the L1 rubric v1 (`~/.cas/artifacts/cas-63c5/rubric.md`).
Surfaces are labelled with the rubric's terms:

- `always`: sent every session or every turn
- `per-invoke`: sent each time a tool or skill is loaded or a message is sent
- `on-demand`: read only when needed

Token estimates use bytes ÷ 4.

Supporting files are in `~/.cas/artifacts/cas-988a/`:

- `tools-list-3.31.0.json`: the live `tools/list` response
- `sessionstart-{worker,supervisor,plain,codexworker}.txt`: real SessionStart payloads rendered by the 3.31.0 hook against a
  snapshot of the cas-src database (`sqlite3 .backup` copy, `CAS_ROOT` pointed at a scratch dir, cloud off)

## Verdict

The runtime's always-loaded prompt surfaces are over the harness limits that the code itself documents. Two separate problems follow.

**SessionStart context overflows.** Worker SessionStart is 11,820 B and supervisor SessionStart is 13,181 B. Both are above Claude
Code's 10,000-character hook cap. Past that cap, Claude Code persists the payload to a file and injects only a ~2 KB preview.
The hook warns about this on its own stderr:

> "protected guidance/context alone exceeds the 9216B budget"

**The largest MCP tool description is cut.** The `coordination` description is 2,834 characters and Claude Code truncates it
at 2,048. The tail it loses contains the only statement of a `sync_all_workers` exception, and the `force` parameter
description says the opposite.

Beyond the size problems:

- **Wrong tool calls in envelopes.** Several templates hand agents calls that fail as written:
  - Claude workers are told to call Codex-prefixed `mcp__cs__*` tools.
  - Suggested `coordination action=message` calls omit the required `summary`.
  - Local-merge workers are told to `git push origin`, which a hook denies.
  - Verifier guidance omits the required `dispatch_id`.
- **Drift.** The same rule is maintained in several hand-written copies:
  - three worker contracts
  - four verification-timeout texts
  - five commit-receipt recovery texts
  - two request structs with the same parameter descriptions
  - action lists that disagree between a tool's description and its `action` parameter

## Measured: the 15 heaviest prompt surfaces

| # | Surface | Source | Bytes | ≈tokens | Shipped | Multiplier / note |
|---|---|---|---:|---:|---|---|
| 1 | MCP `tools/list`, all 15 cas tools | `crates/cas-mcp/src/types*.rs`, `cas-cli/src/mcp/tools/service/mod.rs` | 69,619 | 17,400 | on-demand (Claude ToolSearch select) | Harnesses that load MCP schemas eagerly pay the full cost every turn (not measured for Codex/Grok). |
| 2 | Tools the CLAUDE.md block and worker brief tell agents to select: `task`+`coordination`+`search`+`memory` | same | 43,568 | 10,900 | per session | These four are selected in effectively every factory session. |
| 3 | `mcp__cas__coordination` schema | `ops_secondary.rs:838`, `service/mod.rs:541` | 18,276 | 4,570 | per session | 61 params; 8,330 B of them are supervisor-only. |
| 4 | Supervisor intro with `<cas-session-start-fallback>` | `ui/factory/app/mod.rs:2596-2614` | 13,902 | 3,480 | always (once per launch) | This is a user turn, so the 10K hook cap does not apply. |
| 5 | SessionStart, supervisor | `hooks/handlers/handlers_session.rs:16`, `cas-core/.../build_start.rs` | 13,181 | 3,300 | always | Over the 10,000-char cap, so the model gets a file plus a ~2 KB preview. |
| 6 | `mcp__cas__task` schema | `crates/cas-mcp/src/types.rs:144`, `service/mod.rs:318` | 14,407 | 3,600 | per session | 56 params; 4,645 B of them are supervisor-only. |
| 7 | SessionStart, worker (Claude or Codex) | same | 11,820 | 2,955 | always | Over the cap. |
| 8 | `session-learn` classifier prompt on Stop (without transcript) | `handlers_session.rs:1678-1683` | 9,660 + ≤50,000 transcript | 2,415+ | per session (opt-in) | Off by default (`session_learn_auto=false`). |
| 9 | SessionStart, plain or primary session | same | 9,046 | 2,260 | always | Compacted to fit. |
| 10 | `cas-worker.md` body inside worker SessionStart | `builtins.rs:26,3540` | 7,990 | 2,000 | always | The test cap is 8,000 B, leaving 10 B of headroom. |
| 11 | `mcp__cas__search` schema | `ops_secondary.rs:7`, `service/mod.rs:900` | 7,514 | 1,880 | per session | |
| 12 | `CODEX_WORKER_INSTRUCTIONS` | `crates/cas-pty/src/pty.rs:20` | 4,446 | 1,110 | always (developer instructions) | |
| 13 | Claude worker startup brief (`claude_worker_contract`), as received | `crates/cas-pty/src/pty.rs:313` | 4,140 | 1,035 | once per spawn | |
| 14 | `🔁 Current Handoff` block in SessionStart | `build_start.rs:377` | 3,578 | 895 | always | Protected by default. |
| 15 | `📚 Project Knowledge` index in SessionStart | `build_start.rs:80-135` | 2,446 | 610 | always | Protected by default; ordered alphabetically, not by relevance. |

Next below the cut:

| Surface | Bytes | Shipped | Note |
|---|---:|---|---|
| Ambient-recall packet (`ambient_recall.rs:3257`) | 1,590–1,674 | per turn | Measured on this session's two turns. |
| Generated CLAUDE.md "USE Cassy" block (`cli/init/docs_and_skill.rs:10-23`) | 1,547 | always | Loaded ×3 in this worktree: home, Petrastella and repo CLAUDE.md. |
| `USAGE_REMINDER` (`cas-core/src/hooks/context/mod.rs:520`) | 1,016 | always | |
| `TaskAssigned` + reply footer | 1,028 | per assignment | |
| MERGE REQUIRED close rejection, composite (`close_ops.rs:10906`) | ~2,850 | per rejection | |

How the numbers were taken:

- Tool schemas: the `tools/list` JSON-RPC response from `cas serve` (installed binary), each tool serialised compactly.
- SessionStart: `cas hook SessionStart` run with worker, supervisor and no-role environments against the database snapshot.
- Envelopes: string literals in the source, plus the live transcript of this worker session.

## Findings (severity-ranked)

Columns follow the rubric: `Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens`.

### P0: misleads an agent today

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P0 | per-invoke (every assignment, stall nudge, reply footer) | `cas-cli/src/ui/factory/director/prompts.rs:1357-1393`, `:744`; caller `ui/factory/app/mod.rs:1362` | Claude workers receive **Codex** tool names. `worker_prefix` and the footer come from the session-wide `worker_cli`, not from the recipient's harness. `mcp__cs__` exists only as the Codex server name (`crates/cas-pty/src/pty.rs:1813`). | This worker (Claude) received: "View full details: mcp__cs__task action=show id=cas-988a" and "use: \`mcp__cs__coordination action=message target=witty-lion-10 …\`" | Resolve the prefix per recipient with `app.harness_for(worker)`, which the spawn brief already uses (`app/mod.rs:2153`). Pass that value to `generate_prompt_at` and `with_response_instructions`. | 0 B; saves a failed call plus a recovery turn (~300–800) per assignment |
| P0 | always / per-invoke | `prompts.rs:744`, `prompts.rs:1388`, `crates/cas-pty/src/pty.rs:20,41,315,1404`; schema `ops_secondary.rs` (`summary`) | Every suggested `coordination action=message` call **omits `summary`**. The handler rejects the call without it. The schema does not mark `summary` required and describes it only as "shown as a preview in the UI". | Handler: `agent_search_system/message.rs:637-645`, "summary required — a short one-line preview…". Template: "\`{prefix}coordination action=message target={respond_to} message=\"...\"\`" | Add `summary="..."` to every template. State "required for action=message" in the `summary` parameter description. | +4 per template; saves a failed call each time |
| P0 | per-invoke (close rejection) | `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs:10918` (in the 10906 block), `:10945`, `:11786` (in the 11772 block) | The MERGE REQUIRED and MERGE REALITY remediation tells **local_merge** workers to `git push origin`, but the PreToolUse hook denies that push. | Remediation text: "`git push origin {factory_branch}`". Hook at `hooks/handlers/handlers_events/pre_tool.rs:224-229`: "🚫 LOCAL-MERGE DELIVERY: git push origin is disabled…" | Branch the remediation on `task.delivery_mode` and reuse the local-merge line at `close_ops.rs:15970`. | −60…−120 per rejection; saves a denied call |
| P0 | per-invoke (verification handoff) | `close_ops.rs:6381`, `:6731` | The suggested direct-verdict call `verification action=add` **omits the `dispatch_id` it requires**. In the 6731 case the same message also carries the correct form at 6761, so it contradicts itself. | Validator at `mcp/tools/core/workflow/verification_tools.rs:337-345`: "Verification requires dispatch_id naming an exact active proof boundary." | Always print `dispatch_id={id}`. On the legacy path, say "retry close to mint a dispatch, then add". | ~0; saves a failed call |

### P1: routing, format, or over budget

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P1 | always | `cas-cli/src/hooks/handlers/session_budget.rs:91-112` (degradable list); `crates/cas-core/src/hooks/context/build_start.rs:80-135` (knowledge), `:377` (handoff) | **SessionStart is over the harness cap for every factory role.** Measured payloads: worker 11,820 B, supervisor 13,181 B, Codex-worker 11,799 B. Only the plain session (9,046 B) fits. The degradable list omits `## 📚 Project Knowledge` and `## 🔁 Current Handoff`, and the module says "Anything not listed here is protected", so both are kept verbatim while real guidance is lost to the preview. | The module's own doc (`session_budget.rs:4-8`) describes the persisted file and ~2 KB preview. Claude Code docs: "Hook output strings, including `additionalContext` … are capped at 10,000 characters … saved to a file and replaced with a preview". 18 persisted `tool-results/hook-*-additionalContext.txt` files (10–12 KB) from 2026-09-05…17 exist in `~/.claude*/projects`. The hook's stderr: "SessionStart payload is 11820 bytes with every degradable section dropped — protected guidance/context alone exceeds the 9216B budget." | Add Knowledge (pull: `knowledge action=read`) and Handoff (pull: `memory action=get id=…`) to `DEGRADABLE_BASE_SECTIONS`, with compact summaries that keep the handoff id and title. Add an assembled-payload test for the worker and supervisor roles; the current tests budget only components. | worker −2,600 B (−650); supervisor −4,000 B (−1,000); and the guidance actually reaches the model |
| P1 | always | `crates/cas-core/src/hooks/context/build_start.rs:80-135` | The Knowledge index is sorted `(page_type, title, id)` and cut at the token budget. Every session therefore sees the same first 11 of 148 pages alphabetically, whatever the task. In cas-src these are gabber-studio PostHog pages. The stated reason, a byte-stable prompt-cache prefix, does not hold: the same payload carries the session UUID earlier (`build_start.rs:213`). | `sessionstart-worker.txt`: "cas-kn019 [architecture] PostHog Integration Architecture … cas-kn014 PostHog Environment and Identity Model …" in a cas-src worker session | Rank by relevance to the task or role and cap at 5 lines, or emit only the count plus the pull command. Separately, audit why gabber knowledge pages are in the cas-src store. | −1,500 B (−375) |
| P1 | always (worker startup) | `cas-cli/src/ui/factory/app/mod.rs:2591-2614`; `cas-cli/src/hooks/session_start_fallback.rs:20-33` | **Claude workers on a non-default config dir probably get no SessionStart context.** The code records that Claude 2.1.231 "can skip SessionStart entirely for a native-team supervisor launched under a non-default config dir". The fallback bundle is attached only to the supervisor intro (`is_custom_claude_supervisor`); worker briefs never carry it. | The cas-src `sessions` table (written by every SessionStart run, `handlers_session.rs:37`) has no row after `2026-09-11T13:06`, while 89 `agent_registered` events have occurred since 2026-09-20. The scratch run proves the hook writes that row. This worker (`CLAUDE_CONFIG_DIR=~/.claude-daniel@…`) never saw a `📋 CAS Context` block. Confirm by adding a hook-entry trace. | Extend the custom-profile fallback to worker briefs (`claude_worker_contract`), or rely on the brief plus the skill and stop paying for a duplicate SessionStart bundle. Either way, add telemetry for "SessionStart fired" per role. | Either restores ~2,000 tokens of guidance or confirms it can be dropped |
| P1 | per session (tool schema) | `cas-cli/src/mcp/tools/service/mod.rs:541` | The `coordination` description is **2,834 characters; Claude Code truncates at 2,048**. The lost tail holds the config_dir capture rule, the worktree action list, the shutdown precondition, and "sync_all_workers … always skips a supervision-live worker-owned worktree". That last rule contradicts the `force` parameter, which the model does see: "consent to rebase worktrees … whose assignee is mid-task". | Claude Code MCP docs: "truncates tool descriptions and server instructions at 2KB each … put critical details near the start". anthropics/claude-code#81268 reports that the cut is silent and that `/mcp` shows the full text. This session's own tool list ends in "…need no cerem… [truncated]". | Cut the description to ≤1,500 chars: a purpose line plus action groups. Move per-action rules into the parameter descriptions they govern, which are not truncated. Add a unit test asserting every tool `description.len() ≤ 2048`. | −1,300 B (−330) and removes the contradiction |
| P1 | always (SessionStart) | `cas-cli/src/hooks/handlers/handlers_session.rs:11` | The role-mismatch banner hardcodes `mcp__cs__coordination`. It is added after `remap_tool_prefix` (which only rewrites `mcp__cas__`), so Claude supervisors are told to call a Codex-only tool. | "run \`mcp__cs__coordination action=whoami\` and \`cas doctor\`" | Author against `mcp__cas__` and remap, or use `own_tool_prefix()`. | 0 |
| P1 | per-invoke (on `worktrees.enabled`) | `hooks/handlers/handlers_events/pre_tool.rs:566-571`, `:536`; `close_ops.rs:6862` | The WORKTREE MERGE JAIL denies every tool until the agent spawns a `worktree-merger` agent, which **does not exist** (`builtins/agents/` holds only 5 agents). The unjail check also accepts only `tool_name == "Task"`, not `"Agent"`. The result is a dead end. Default-off (`worktrees.enabled=false`), so P1 rather than P0. | "You MUST spawn the 'worktree-merger' agent … subagent_type=\"worktree-merger\"" | Point the message at `{prefix}coordination action=worktree_merge id=<branch> task_id=<id>`, or delete the jail. | ~0 |
| P1 | per-invoke | `close_ops.rs:11772` (MERGE REALITY) | The steps contradict the epic-branch merge model: "4. Open a PR targeting {parent_branch} and merge it", against "do NOT run `gh pr create --base {parent_branch}`" at `:10888/10897`. The commands are also incomplete: `git rebase --onto {factory_branch}` has no arguments, and "Retry: `task action=close`" has no `id=`. | quoted | Reuse the parent-kind and delivery-mode branching from 10906, and emit complete commands. | ~0 |
| P1 | always | `crates/cas-core/src/hooks/context/mod.rs:520-541` | `USAGE_REMINDER` uses `<IMPORTANT>` and "PROACTIVELY … Don't wait to be asked", which is rubric-flagged emphasis in always-loaded text. It restates the generated CLAUDE.md block (`cli/init/docs_and_skill.rs:10-23`) and the worker brief ("Always use CAS MCP tools…"), so the rule arrives 3–5 times per worker session. It also advertises "semantic understanding" for `mcp__cas__search`, whose tool description says "search (BM25 full-text)"; semantic search exists only when cloud is logged in (`hybrid_search/semantic.rs:1`). | quoted | Reduce to ≤300 B: the three tool names plus one line on when to use `search` versus Grep. Drop the `<IMPORTANT>` block; `cas-worker.md:107-108` already covers memory. | −700 B (−175) always |

### P2: efficiency, structure, drift

| Sev | Surface | file:line | Defect | Evidence | Fix | Δ tokens |
|---|---|---|---|---|---|---|
| P2 | per session (tool schema) | rmcp 0.16 `handler/server/common.rs:26-27` (`AddNullable`) + `#[serde(default)]` on every `Option<>` | **16.4% of the tool payload is schema boilerplate**: 291× `"default":null`, 296× `"nullable":true`, non-standard `"format":"uint"/"int64"`, per-tool `$schema`/`title`. `nullable` is an OpenAPI 3.0 keyword, not JSON Schema 2020-12. The MCP spec says `inputSchema` defaults to 2020-12. | Stripping those keys takes 69,635 → 58,207 B (−11,428 B). By tool: coordination −2,150, task −1,882, search −1,471, memory −743. | Post-process the schemas in `list_tools` (strip null defaults, `nullable`, non-standard `format`, `title`, `$schema`), or generate with `SchemaSettings::draft2020_12()` without the `AddNullable` transform. | −2,860 total; −1,560 for the 4 always-selected tools |
| P2 | per session (tool schema) | `crates/cas-mcp/src/types/ops_secondary.rs:838-1256` (CoordinationRequest), `types.rs:144` (TaskRequest) | Supervisor and admin parameters load into every worker. 40 of coordination's 61 params (8,330 B) serve only spawn, worktree, server, db, gc, loop and queue actions. In `task`, 4,645 B of params are supervisor-only (proof_scope_fix, stranded_branch_override, negative_result*, supervisor_override, external_verification_receipt, …). | measured on `tools-list-3.31.0.json` | Split `coordination` into a worker-facing tool (whoami, message, inbox, remind, heartbeat) and a `factory` tool for the supervisor. A worker's brief then selects only the small one. Anthropic's tool guidance: "selectively implementing tools whose names reflect natural subdivisions of tasks … reduce the number of tools and tool descriptions loaded into the agent's context". | −2,000…−3,000 per worker session |
| P2 | per session | `service/mod.rs:318` vs `types.rs:147`; `service/mod.rs:541` vs `ops_secondary.rs:841` | The action lists in each tool's description and in its `action` parameter **disagree**. `task` action param omits `request_changes` and `reset`, both dispatched at `service/mod.rs:376,387`. `coordination` action param omits `epic_status`, `server_start/stop/list`, and the description omits `interrupt`. `action` is a free `String`, not an enum. | diff of the two lists (script in the search manifest) | Make `action` a schemars `enum` generated from the dispatch table. It is then validated and listed once; remove the prose action list from the description. Anthropic: "enforcing with strict data models". | −300…−500 and no more drift |
| P2 | on-demand (source) | `ops_secondary.rs:570-837` (FactoryRequest) vs `:838-1256` (CoordinationRequest) | Two request structs keep hand-copied parameter descriptions, and 18 of the 36 shared fields already differ. `reason`, `summary` and `target` have different meanings in the two. The agent-visible `reason` says only "Reason for cancelling", though it is also the `shutdown_workers` audit reason. | e.g. `summary`: F="worker_status: one liveness line…", C="A short one-line summary of the message" | Derive one from the other, or share constants. Rename the colliding fields. | 0 (drift) |
| P2 | per turn | `cas-cli/src/ambient_recall.rs:1773-1796` (stopwords), `hooks/turn_context.rs:206` | Ambient recall builds its query from the raw teammate-message text, **including envelope attribute values**. The stoplist removes `teammate_id` but not values such as `director`, `color`, `green`. The packet also re-injects the task the agent just read. | This session: "why=lexical match: director,color", "why=lexical match: color,green" (TUI pane-colour memories). The next packet's first hit was "[cas-988a] Task L3 audit…", right after `task show cas-988a`. | Strip `<teammate-message …>` / `[cas #N …]` headers before term extraction instead of adding stopwords. Exclude task ids already present in the turn. | −400…−800 per turn |
| P3 | per turn | `ambient_recall.rs:3449-3454` | Each recall line carries nanosecond provenance and a fixed tail that the agent does not use. | "\| provenance=cas.db/read-only:2026-03-30-5@2026-09-19T00:27:41.106319674+00:00", "— body available by tool" ×3 | Keep the id plus `why`; move provenance to the tool-pull response. | −90 per packet |
| P2 | always + once per spawn | `crates/cas-pty/src/pty.rs:313-349` vs `cas-cli/src/builtins/skills/cas-worker.md` | Claude workers receive the rules twice: the 7,990 B skill body at SessionStart plus the 4 KB `claude_worker_contract`, which says "See the cas-worker skill". Duplicated rules: ONE task (`pty.rs:320` / `cas-worker.md:19,100`), blocker/merge_request (`:323` / `:93`), no cargo (`:336` / `:26-27`), <20% checkpoint (`:336-338` / `:66,112`), SILENT EXECUTION (`:349` / `:12`), MERGE REQUIRED re-close (`:326` / `:37-40`). | quoted | Cut the Claude contract to what the skill lacks (name, prefix, no `session_start`, never foreground-block), about 800 B, if P1 above confirms workers get SessionStart. If they do not, cut the skill pointer instead. | −790 per spawn |
| P2 | always | `pty.rs:20` / `:41` / `:313` | Three hand-maintained worker contracts (Codex, Grok, Claude) have drifted. Only some carry "WORK HALTED, do not fight it", the verification-required handoff, "Message from <sender>" framing, or the skill pointer. The parity test checks markers only. | — | One `worker_contract(prefix, name, harness_extras)` renderer. | 0 (drift) |
| P2 | per spawn | `crates/cas-pty/src/pty.rs:20,41,331` | The suggested `remind` call omits the required `remind_message` (`mcp/tools/service/factory_remind.rs:147-152`). | "\`…coordination action=remind remind_delay_secs=<n>\`" | Add `remind_message="…"`. | +4 |
| P2 | per spawn | `pty.rs:20,41,320` vs `cas-worker.md:28-29` | The contracts always say "commit and push"; the skill says local_merge keeps the commit local. | quoted | "commit (and push unless `delivery_mode=local_merge`)". | ~0 |
| P2 | per message | `cas-cli/src/ui/factory/app/mod.rs:2768,2818`; `agent_search_system/message.rs:127-146` | Runtime-generated startup contracts are queued with source `"cas"`, which the provenance classifier labels `agent-authored`. The trust header the skill tells workers to read is therefore wrong on the first message. | This brief's header: "[cas #34968 agent-authored 0s first]" | Map source `cas` to a `runtime` label. | 0 |
| P2 | per-invoke | `close_ops.rs:10906` (1,933 B literal; ~2,850 B composite), `:10945` | MERGE REQUIRED explains polling internals: "Polling marks messages seen without consuming daemon transport delivery. The polling claim is at-most-once…". Step 4 dictates a ~300 B `message=` template. The negative-result/request_changes paragraph repeats at `:10932` and `:10964`. | quoted | Step 1 → "Run `{coord} action=inbox_poll` until `No unread messages`; follow a merged/request-changes reply." Share the repeated paragraph. | −300 per rejection |
| P2 | per-invoke | `close_ops.rs:6766` assembling `:6706`, `:6533`, `:6761`, `:6784` | The worker VERIFICATION REQUIRED message addresses four roles (worker, verifier, supervisor, worker again) and echoes the close reason twice, once inside a fenced block. A fenced echo of free text can nest fences, the Ink-crash trigger named in CLAUDE.md. `:6543` "Epic verification runs on master" is stale. | "IMPORTANT: The {verifier_agent} MUST validate this close reason" | Workers get only the handoff line. Never re-fence the reason. | −150 plus the reason length |
| P2 | per-invoke | `close_ops.rs:5133`, `:5182`, `:6200`, `:6374` | Four VERIFICATION TIMED OUT variants have drifted. `5182` gives no next action; `6374` repeats the missing-`dispatch_id` bug. | "requires named registered-supervisor recovery before close." | One `verification_timeout_message()` helper. | ~0 (drift) |
| P2 | per-invoke | `close_ops.rs:13962`, `:14010`, `:13786`, `:11298`, `:17325/17330` | The commit_receipt recovery steps are restated five times, in different orders, with different verification commands. | — | One `commit_receipt_recovery_steps()`. | −250 |
| P2 | per-invoke | `pre_tool.rs:815-833`, `:870`, `:878`, `:896` | The verifier-authority denials give no concrete next call. | "…deadline has elapsed; use the recorded recovery path." | Name the call: "Retry `{prefix}task action=close id=<id>` to mint a dispatch, then spawn." | ~0 |
| P2 | per turn (supervisor) | `hooks/handlers/handlers_middle/prompt_capture.rs:23-27`; `pre_tool.rs:4135` | Per-turn text restates rules that PreToolUse already denies (`pre_tool.rs:138-160,177`): "AskUserQuestion is BLOCKED… Never SendMessage…". The SendMessage receipt claims "Message delivered" when the code only enqueued it. | quoted | Drop the enforced rules from the per-turn reminder. Say "queued (id N)". | −100 per supervisor turn |
| P2 | per-invoke | `supervisor_push.rs:413`; `hooks/handlers/handlers_events/neon_sql_guard.rs:241` | Some remedies are impossible for the agent. `drain_lifecycle_outbox` is a Rust function, not an MCP action. The Neon guard tells workers to create branches themselves, while `coordination` says `db_branch_create` is supervisor-only and "a worker asks with a blocker message". | quoted | Give one route per rule, callable by the recipient. | ~0 |
| P2 | per-invoke | `close_ops.rs:1768-1771` before `:1800` | The `risk=platform` proof check runs before the workers' `DeferredToAssembly` exit, so a worker is asked for a macOS `cargo` receipt that `pre_tool.rs:60-69` forbids it to produce. | — | Defer platform proof to assembly for workers, or name the supervisor/CI route. | ~0 |
| P2 | per session (Stop, opt-in) | `handlers_session.rs:1678-1683`, `:1760`; `builtins/skills/session-learn/SKILL.md:44,76-96` | The session-learn classifier sends the whole SKILL.md, frontmatter included, to a single-turn, tool-less model call. It tells that model to "scan the existing memory store via `mcp__cas__search`", which it cannot do. It also ships maintainer-only sections ("Decision: in-process vs subprocess", "Kill switch"). The doc comment says Haiku; the code calls `claude-opus-5-5`. | quoted | Keep a dedicated classifier prompt (signals, schema, one example) and pass known-duplicate candidates in. Reuse the skill only for humans. | −1,200 per Stop when enabled |
| P2 | always | `cas-cli/src/hooks/handlers/handlers_session.rs:298` (protected staleness) | The project-overview freshness warning is protected and shown to workers: "Run `/project-overview` before planning cross-cutting work". That is out of scope for a worker. `docs/PRODUCT_OVERVIEW.md` really is missing in cas-src, so the warning fires every session. | `sessionstart-worker.txt` tail | Show it to supervisors only, or make it degradable. | −65 per worker session |

### P3: polish

| Sev | Surface | file:line | Defect | Fix |
|---|---|---|---|---|
| P3 | per session | `ops_secondary.rs:932` (`force`), `service/mod.rs:109` | Internal ticket id in agent-visible text: "use allow_trunk for that (cas-0b32)". Guard texts do the same: `pre_tool.rs:63` (cas-4cbb), `:1766` (GH #120), `stale_close_guard.rs:61` (cas-b269), `lifecycle.rs:12/16/86` (cas-9fff). | Remove. |
| P3 | per session | `ops_secondary.rs:756/764` (and 1110/1118) | Stale values: `cli` says "'claude' (default) or 'codex'", but grok and opencode are valid (`crates/cas-mux/src/harness.rs:35`). The `model` example is `claude-opus-4-5`. | List the harness enum; update the example. |
| P3 | per session | `service/mod.rs:318`, `crates/cas-mcp/src/types.rs:271-273` | "IMPORTANT for 'close': verification must pass first" appears in the task description and again, as "IMPORTANT: … BEFORE", in the `reason` parameter. The rule is enforced by close. | State it once, without caps. |
| P3 | always | `cas-cli/src/mcp/tools/service/server_handler.rs:36` | Server `instructions` ("CAS (Coding Agent System) provides unified memory, tasks, rules, and skills.") uses the old name. These instructions are always injected by Claude Code (`mcp_instructions_delta`), so they could carry the one useful hint instead, e.g. "Select task, coordination, search, memory via ToolSearch before first use." | Rewrite to ≤200 chars of routing guidance. |
| P3 | per session | all tools | No `title` and no `annotations`. The MCP spec (2026-07-28) makes both optional. Multi-action tools cannot honestly declare `readOnlyHint`, which is another argument for the split above. | Add `title`; add annotations after the split. |
| P3 | always | `build_start.rs:213` | "**Session:** \`<uuid>\` (auto-registers on first CAS tool use)" is stale: SessionStart already registers the agent, and the worker brief says not to call `session_start`. The UUID also breaks cache stability of the payload. | Drop the line. |
| P3 | always | `sessionstart-plain.txt:91` | An empty "## Quick ..." heading renders after Active Rules. The source was not located, so the cause is unconfirmed; probably a truncated heading in a rule or skill preview. | Find it and drop empty headings. |
| P3 | per-invoke | `lifecycle.rs:882,917`, `close_ops.rs:13437-13540` | A doubled `\\` line continuation inside `format!` leaks " \" plus 13–29 spaces into the output. | Use a single `\`. |
| P3 | per-invoke | `pre_tool.rs:1709/1733/1775`, `factory_isolation.rs:269`; `close_ops.rs:6295/6381/6647/6731/6747`; `qa_pass.rs:621`; `close_ops.rs:1414/1844` | Text specific to one harness or repo is shown to everyone: "not the Claude Code PreToolUse harness" (to Codex/Grok, ×4), `Task(subagent_type=…)` syntax, hardcoded `mcp__cas__verification`, `-p cas`. | Render per harness and project. |
| P3 | per-invoke | `stale_close_guard.rs:69`, `proof_scope.rs:302/308`, `close_ops.rs:120/125/172/180/1646/1771/8583/13962/14010`, `qa_pass.rs:208` | Bare `task action=` versus `{tool_prefix}task action=` in neighbouring messages. | One convention (prefixed). |
| P3 | per-invoke | `prompts.rs:1386`; `pty.rs:17,20`, `app/mod.rs:2551` | "A concise execution plan is optional" contradicts SILENT EXECUTION. Codex prompts forbid `/cas-start`, `/cas-context`, `/cas-end`, which no longer exist anywhere in the source. | Delete both. |
| P3 | per-invoke | `prompts.rs:1393` vs `cas-worker.md:109-110` | The reply footer targets the supervisor's pane name (`target=witty-lion-10`); the skill says to target the literal `supervisor`. | Use `supervisor`. |
| P3 | per spawn | `queue_and_events.rs:496` + `prompts.rs:1381` | A pre-assigned spawn gets both the spawn brief and `TaskAssigned` for the same task. | Suppress `TaskAssigned` when the spawn brief covers the same (task, worker) pair. |

## MCP tool-description guidance used (cited)

- **Claude Code MCP docs** (`code.claude.com/docs/en/mcp`): "Claude Code truncates tool descriptions and server instructions at 2KB each.
  Keep them concise to avoid truncation, and put critical details near the start."
- **anthropics/claude-code#81268** (2026-07-25): the tool `description` is cut at exactly 2,048 characters with no log. `/mcp` displays the
  untruncated text, so authors cannot see the cut. (#41593 earlier documented a model hallucinating functions after 45 of 60 signatures were cut.)
- **Claude Code hooks docs** (`code.claude.com/docs/en/hooks`): "Hook output strings, including `additionalContext` … are capped at 10,000
  characters. Output that exceeds this limit is saved to a file and replaced with a preview and file path". The ~2 KB preview is
  reported in #44086; making the cap configurable is requested in #64626.
- **Anthropic, Define tools** (`docs.anthropic.com/en/docs/agents-and-tools/tool-use/implement-tool-use`): "Provide extremely detailed
  descriptions … Aim for at least 3–4 sentences"; use `input_examples` for complex inputs; "Use meaningful namespacing in tool names".
  In Claude Code this conflicts with the 2 KB cap. The resolution is a short tool description with the detail moved into parameter
  descriptions, which are not subject to the 2 KB cap.
- **Anthropic Engineering, Writing effective tools for agents**: consolidate and subdivide tools along natural task boundaries to reduce the tool
  descriptions loaded into context; "Avoid ambiguity by clearly describing (and enforcing with strict data models) expected inputs"; and on errors,
  "prompt-engineer your error responses to clearly communicate specific and actionable improvements". Claude Code limits tool
  responses to 25,000 tokens by default.
- **MCP spec 2026-07-28, server/tools**: a tool has `name`, optional `title`, `description`, `inputSchema` ("Defaults to 2020-12 if no `$schema`
  field is present"; must be a valid JSON Schema object), optional `outputSchema` and `annotations`. Annotations are untrusted unless the server is trusted.
  Servers SHOULD return tools in a deterministic order. The spec sets no length limit, so the limit is the client's.

Scored against those sources:

- `coordination` fails the 2 KB cap.
- `task` (1,546) and `search` (798) fit, but put their action lists in prose rather than an enum.
- No tool uses `input_examples`/examples. For the action-multiplexed tools, one worked call per common action in the relevant parameter
  description would cost less than the current prose lists.
- Error texts (P0/P2 rows above) are often long but not actionable: missing parameters, impossible routes.

## Cross-lane references (not re-reported)

- Bundled non-SKILL.md files never refresh after first install (`sync_builtin_detailed`). This is the L1/L2 P0. It matters here because
  `cas-worker/references/*.md` are the pull targets named at the end of the worker SessionStart guidance.
- Grok in cas-src resolves skills from `.claude/skills`. Cross-lane P0.
- Skill-listing descriptions (a 26,354 B `skill_listing` attachment in this session) belong to L2 and are not scored here.

## Recommended order of fixes

1. **P0 envelope calls**: per-recipient prefix, `summary=` in templates, delivery-mode-aware remediation, `dispatch_id`. These are small
   code changes that stop repeated failed calls.
2. **SessionStart budget**: make Knowledge and Handoff degradable, and add a test that asserts the assembled payload is ≤ 9,216 B for each role. Then answer the
   custom-profile worker question (P1) before trimming the contract/skill duplication (P2), because the fix depends on which one actually arrives.
3. **MCP schema diet**: add a ≤2,048-char description test, turn `action` into an enum, strip the rmcp nullable boilerplate, and split out a
   supervisor `factory` tool. Together about −4.5k tokens per worker session.
4. **Helper extraction** for close-gate texts (verification timeout, commit-receipt recovery, merge remediation) to stop the drift.

## Search manifest

| Command / probe | Hits |
|---|---:|
| `cas serve` + JSON-RPC `initialize`/`tools/list` (installed 3.31.0) | 15 tools, 69,619 B |
| `grep -c '"default":null'` / `'"nullable":true'` on tools/list | 291 / 296 |
| `grep -rn 'mcp__cs__' cas-cli/src --include=*.rs` (non-test) | 119 |
| `grep -rn 'provides unified memory, tasks, rules, and skills'` | 1 |
| `cas hook SessionStart` × {plain, worker, supervisor, codex worker} on DB snapshot | 9,046 / 11,820 / 13,181 / 11,799 B |
| `find ~/.claude*/projects -name 'hook-*-additionalContext.txt'` | 18 (2026-09-05…17) |
| `sqlite3 cas.db 'select max(started_at) from sessions'` (cas-src, read-only) | 2026-09-11T13:06 |
| `events where event_type='agent_registered' and created_at>'2026-09-20'` | 89 |
| `grep -n 'DEGRADABLE_BASE_SECTIONS' session_budget.rs` | 1 (6 base entries + Codex note; no Knowledge/Handoff) |
| `grep -rn 'claude_custom_config_context_fallback\|cas-session-start-fallback'` | 1 call site (supervisor intro only) |
| `grep -rn 'worktree-merger' cas-cli/src/builtins/agents` | 0 |
| `grep -n 'summary required' message.rs` | 1 |
| action-list diff (description vs `action` param) | task −2, coordination −4/+1 |
| FactoryRequest vs CoordinationRequest description diff | 18 of 36 shared fields differ |
| `grep teammate\|agent-authored ambient_recall.rs` stoplist | teammate/teammate_id present; director/color/green absent |
| `grep -rn 'Quick' crates cas-cli/src` for the "## Quick ..." heading | 0 relevant (source unresolved) |
| exa-search: Claude Code MCP 2048 truncation; hooks 10,000 cap; Anthropic tool docs; MCP spec tools | 5 queries, sources cited above |
