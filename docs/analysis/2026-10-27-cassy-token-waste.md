# Cassy input footprint: a 14-day transcript spike

Task: cas-da94 · Factory: cassy-tender-finch-45 · Decision: prioritize bounded task
details, role-scoped skill discovery, and smaller ambient recall packets.

**Measured:** 73.51 million characters of Cassy and workflow-related input within
226.64 million characters of recorded input (32.43%). The ten largest reduction
candidates offer a **40.22 million character upper bound** under the explicit
budgets below. That is 17.75% of recorded input, or roughly 10.06 million token
equivalents at four characters per token. This is a planning estimate, not a
measured saving, a billing estimate, or proof that all those characters were waste.

## Scope and method

The window is exactly **2026-10-13 17:57:00 UTC through 2026-10-27 17:57:00 UTC**,
exclusive at the end. The tally began with the requested supervisor's Claude
session `b34c9303-968c-4ef1-b7ce-c9d466b0876e` in the requested account root, then
widened to Claude Cassy/cas-src project and worktree transcripts, subagents, and
Codex rollouts whose session metadata identifies those repositories. Other
projects' text is not included in the report.

The final corpus is **753 logical sessions: 725 Claude and 28 Codex**, spanning
735.40 elapsed session-hours. Roles inferred from registry identity, explicit
startup/task titles, supervisor hook markers, and actual verification/QA actions
are: 25 supervisors, 90 workers, 405 verifiers, 42 QA sessions, and 191 unknown.
These are observational labels; an unknown session is not silently called a
worker. Short verifier subagents explain much of the session count.

The script inspected 1,123 candidate JSONL files (5.48 GB on disk). It skipped
19,284 alias-path file discoveries, merged four physical continuation files into
their logical sessions, and skipped 3,888 other-project Claude files. Several
account roots resolve to the same underlying files. In addition to inode
deduplication, Claude sessions merge by session UUID, subagents by parent UUID
and agent stem, and repeated canonical record UUIDs are excluded. The four
continuation merges did not remove input characters in this window; they did
correct the session count. The manifest contains opaque logical-session hashes,
roles, timestamps, and counters, with no source paths or transcript bodies.

The reader streams one JSONL record at a time, caps records at 16 MiB, and would
report oversized or malformed records. Neither occurred in this run. It counts:

- Claude canonical user text, tool results, and **rendered** attachment text.
  It excludes assistant prose/thinking, API-request/blob mirrors, queue metadata,
  system telemetry, and 153,494 nonrendered hook/tool attachments. Tool-use IDs
  link results to the tool and requested action.
- Codex input-role `response_item` messages, actual tool outputs, and the recorded
  base instructions once per session. It excludes `event_msg` message mirrors,
  reasoning, turn-context snapshots, and token-count telemetry from text totals.
- Disjoint spans for task detail, sibling notes, predicted QA appendices, mail
  bodies, inbox framing, reminders, and recall. A task's QA table is not also
  charged to its task detail. A hook envelope and its message body are not both
  charged to mail. Exact repeat text is a diagnostic overlay, not extra input.

**Denominators matter.** “Input share” below means share of newly recorded input
characters, not cumulative provider input tokens. Claude usage counters include
cache reads and growing conversation prefixes; rollouts omit some hidden system
prompts, tool schemas, and initial CLAUDE.md content. Allocating billed input
tokens by these character shares would be unjustified. At four characters per
token, 73.51 million characters is 18.38 million token equivalents. Actual
tokenization varies by model and content; the measured characters are primary.

“Per hour” divides occurrence counts by the sum of each logical session's
first-to-last in-window timestamps. It includes idle time and is not an active
generation-hour measure. Occurrences are classified text spans, not necessarily
one tool call or one model request.

## Ranked top ten

Rank is by the hypothetical removal of text above a per-occurrence default
budget: `sum(max(0, observed_chars - budget_chars))`. Those budgets are a proposed
product policy, not a fitted prediction. Required evidence and instructions must
remain accessible by explicit full/detail reads. An action or safety exemption
could reduce each saving, potentially to zero. The budgets and exact arithmetic
are in `CAPS` and `counterfactual_saving_chars` in the accompanying script/data.

Shares are **all recorded input / classified Cassy-workflow input**. Sizes and
budgets are characters. “Match” is the action proxy defined after the table.
Producer references P1–P10 resolve to full code paths below.

| Rank · source · producer | Input share | Median / p95; occurrences/hour | Useful, duplicate, or unresolved | Concrete change; proposed default budget | Upper-bound saving, characters (token equivalents) |
| --- | --- | --- | --- | --- | --- |
| 1 · Task show/notes/start detail · P1 | 6.49% / 20.02% | 1,288 / 9,207; 7.15/h | Core task scope is useful. Long note/description bodies are a reduction candidate; no claim they were ignored. | Default to current scope, latest decision, and proof references; page note bodies by byte budget and offer explicit full reads. 2,000/span. | 8,826,009 (2,206,502) |
| 2 · Skill catalog listings · P2 | 4.16% / 12.84% | 25,448 / 28,140; 0.62/h | Discovery is useful; loading the whole catalog for every short-lived verifier is a candidate. Some catalog entries belong to other installed tools. | Project a small role/task catalog and expose the rest through lookup; preserve explicit skill invocation and harness parity. 2,500/listing. | 8,411,863 (2,102,966) |
| 3 · Ambient recall · P3 | 3.60% / 11.11% | 1,742 / 6,131; 4.69/h | 296/3,446 packets matched a later relevant action (8.6%); the rest are unresolved, not proven ignored. 5,170 Code cards contributed 2,647,418 characters. | Prioritize exact task/epic evidence; suppress Code cards for supervisors unless requested; return IDs and short reasons, fetch bodies on demand. 800/packet. | 5,513,820 (1,378,455) |
| 4 · Skill/reference body reads · P4 | 4.02% / 12.38% | 2,028 / 19,636; 2.54/h | Often required instructions; not automatically waste. Only one byte-identical within-session repeat was found, so simple identical-read caching cannot claim this total. | Consolidate overlapping worker/check/delivery rules and load conditional references only for the current phase; retain every mandatory rule. 4,000/read. | 5,011,825 (1,252,956) |
| 5 · Merge and close gate responses · P5 | 2.01% / 6.19% | 1,494 / 4,012; 3.24/h | Gate identity and recovery steps are useful. Repeated rubric/proof prose can be referenced. | Return gate code, bound SHA, missing check, required next action, and a durable detail reference. Preserve full actionable failures. 800/response. | 2,940,370 (735,093) |
| 6 · Same-ID mail bodies seen again · P6 | 1.23% / 3.80% | 435 / 2,943; 4.84/h | 3,561 repeat bodies, 2,791,333 characters; 932 matched later actions (26.2%). Some once-only unacknowledged repeats are intentional. This does not establish redelivery after acknowledgment. | Keep recipient delivery/ack state across hook, Teams, and inbox paths; terminalize acknowledged rows; use an ID/delta notice for eligible repeats while retaining first delivery and unresolved critical bodies. Zero repeated-body budget is the optimistic ceiling. | 2,791,333 (697,833) |
| 7 · First/otherwise unclassified incoming mail bodies · P7 | 2.88% / 8.88% | 583 / 2,265; 11.44/h | 2,586/8,416 matched actions (30.7%); some remaining messages need no mutation. Reports may repeat already-linked QA proof. | Put action, task, SHA, gate state, and one evidence-manifest reference in the relay. Keep detailed proofs in the linked record, with explicit expansion. 800/body. | 2,188,484 (547,121) |
| 8 · Other MCP outputs · P8 | 2.30% / 7.09% | 328 / 2,865; 10.02/h | Mixed necessary data and receipts; no blanket ignored-content claim. Verification add responses alone emitted 1,374,388 characters before category splitting. | Bound successful receipts and default search/context bodies; return IDs and artifact references, preserve failure findings, and add pagination/full reads. 1,000/response. | 1,913,093 (478,273) |
| 9 · Lifecycle relays · P9 | 1.01% / 3.12% | 524 / 1,055; 5.36/h | 982/3,944 matched actions (24.9%). Started/idle/state relays often duplicate state visible elsewhere; failure relays remain valuable. | Digest routine transitions by worker/task episode and emit action-bearing deltas; keep blockers immediate. 240/relay. | 1,344,424 (336,106) |
| 10 · Session/hook context outside named spans · P10 | 0.84% / 2.59% | 60 / 316; 9.02/h | Most spans are small wrappers; the long tail reaches 9,960 characters. Startup context may be necessary. | Put stable project/role context behind a fingerprint; inject missing or changed context on resume and retrieve task-specific context explicitly. 800/span. | 1,283,062 (320,766) |

The action proxy checks the **next 12 actual tool calls in the logical session**
for a mutation concerning a task named in the packet, or an explicit acknowledgment
of its message ID. Mutation includes start/close, dispatch/spawn/merge, verification,
and QA verdicts. A match does not prove causality: several packets about the same
task may match one later action. No match does not prove neglect: the model may
read code, decide no change is necessary, act after the horizon, or perform a
useful action without a task ID. Read-only tool responses and skill loads do not
have a comparable usefulness verdict. No automated count is presented as proof
of an ignored message.

## Producing paths and existing mitigations

Paths and lines refer to source revision
`5a234f9fc58f6cecec193d4e7f503e616454b667`. They identify the producing seam in
this checkout; the corpus spans earlier runtime versions as well. A follow-up
must reproduce remaining overhead on the current runtime before changing it.

1. **P1:** `cas-cli/src/mcp/tools/core/task/query.rs:79` (`cas_task_show`),
   `query.rs:40` (`task_show_notes`),
   `cas-cli/src/mcp/tools/core/task/notes.rs:17` (`cas_task_notes_read`), and
   `cas-cli/src/mcp/tools/core/task/lifecycle.rs:2987` (sibling notes).
   Task show already keeps the newest five notes; that is a count cap, not a
   byte cap. Full notes reads still return the complete body. Start already
   has `brief=true` and skips sibling retrieval in that mode.
2. **P2:** `cas-cli/src/builtins.rs:2756` (`skill_catalog_for_harness`),
   `builtins.rs:3609` (project versus complete user-level catalogs).
   Cassy installs the metadata; Claude/Codex render the listing. The harness
   renderer itself is outside this repository. Project filtering already exists
   for a few optional stack skills; user catalogs intentionally remain complete.
3. **P3:** `cas-cli/src/ambient_recall.rs:3244` (`render_packet`) and `:3448`
   (`render_card`); `cas-cli/src/hooks/handlers/handlers_middle/prompt_capture.rs:127`
   calls recall after inbox delivery. Recall already has role policies,
   candidate/packet caps, and seen/authored evidence suppression. The proposal
   adjusts selection and presentation without removing those protections.
4. **P4:** `cas-cli/src/builtins/skills/cas-worker.md` and its
   `cas-worker/references/{discipline,details,close-gate}.md`, together with the
   role contract renderer `crates/cas-pty/src/pty.rs:365` (`worker_contract`).
   Skill bodies are filesystem reads requested by the model, rather than a
   single runtime injection function. The tally classifies actual Read/Skill or
   cat/sed/head results tied to a skill path. Mixed shell reads are a heuristic
   limitation and can include adjacent documentation.
5. **P5:** `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops/gate_text.rs:178`
   (`worker_verification_required`), `gate_text.rs:156`
   (`merge_gate_exits_paragraph`), and
   `cas-cli/src/mcp/tools/core/workflow/worktree_ops.rs:3875` (`worktree_merge`).
   Recent gate text is already shorter in several cases. Failure recovery
   details and immutable delivery identities must survive any compact mode.
6. **P6:** `cas-cli/src/hooks/handlers/handlers_middle/factory_inbox.rs:779`
   (`polled_unacked_note`) and `:746` (`render_block`);
   `cas-cli/src/mcp/tools/service/agent_search_system/message.rs:3755`
   (full inbox redelivery) and `:3876` (`message_ack`);
   `crates/cas-store/src/prompt_queue_store.rs` (recipient seen/settled state).
   The current code deliberately permits a one-time polled-but-unacknowledged
   repeat and settles rows afterward. Do not remove that recovery path on the
   assumption every repeated body was handled.
7. **P7:** `cas-cli/src/hooks/handlers/handlers_middle/factory_inbox.rs:1074`
   (`render_rows_with_header`), and
   `cas-cli/src/mcp/tools/service/agent_search_system/message.rs:3771`
   (first inbox delivery). Verification handoffs separately embed the complete
   proposed close reason at `cas-cli/src/prompt_revalidation.rs:1429`; worker
   and supervisor messages can repeat the same evidence links. A manifest
   reference should replace copied proof bodies, without dropping the proof.
8. **P8:** `cas-cli/src/mcp/tools/core/workflow/verification_tools.rs:1052`
   (verification receipt echoes the full summary),
   `cas-cli/src/mcp/tools/service/agent_search_system/search_context.rs`
   (search/context output), and task/factory action response renderers.
   The accompanying `mcp_output_breakdown` separates tool families/actions so
   this mixed bucket can be implemented one response type at a time.
9. **P9:** `cas-cli/src/mcp/tools/core/task/lifecycle/supervisor_push.rs:1298`
   (lifecycle relay),
   `cas-cli/src/ui/factory/daemon/runtime/lifecycle.rs:1272`
   (`enqueue_worker_attention_relay_inner`). Episode keys/idempotent enqueue
   already suppress several duplicates. The next change should digest routine
   events rather than weaken blocker or independent-verification dispatches.
10. **P10:** `cas-cli/src/hooks/handlers/handlers_session.rs:23`
    (`handle_session_start`), `:226` (context selection),
    `cas-cli/src/hooks/context.rs:114` (`build_context_with_token_budget`), and
    `cas-cli/src/hooks/handlers/handlers_middle/prompt_capture.rs:83`
    (`handle_user_prompt_submit`). The bucket excludes separately measured
    recall, reminder, inbox, and catalog spans. Some residual wrappers come from
    the harness, so Cassy cannot remove every character in this category.

## The requested factory, including the supervisor

The exact requested supervisor is **one logical session**, split across account
continuation files: 5.86 elapsed hours, 1,183,960 recorded input characters,
813,658 classified workflow characters (68.72%). The named account root alone
was only part of that session. The factory cohort contains 25 identified logical
sessions: one supervisor, two workers, two QA sessions, 19 verifier subagents,
and one unknown helper. It is a positive-membership cohort, not a claim to have
found every retired worker: registry rows can disappear and old launches may
omit the factory name.

For this supervisor, recall alone emitted **190,201 characters in 61 packets**
(16.06% of recorded input). That included 141 Code cards totaling 72,465
characters, and 125 Knowledge cards totaling 57,829 characters. Only 10/61
packets matched the bounded action proxy. Dropping just supervisor Code cards
would remove at most 72,465 directly measured characters before any replacement
context, about 18,116 token equivalents. This is a more conservative target than
dropping every packet without an action match.

The supervisor received 114 reminder spans totaling 47,376 characters (4.00% of
recorded input), and 34 verification/QA dispatch spans totaling 53,615 characters
(4.53%). The latter have 15 matching actions and should retain their dispatch,
owner, task, deadline, bound SHA, and verdict command even when the close reason
becomes a reference. Seven repeated message bodies totaled 12,346 characters;
all seven matched a later action, reinforcing why repetition alone cannot prove
uselessness.

## Smaller sources and the original director incident

These were measured even though they fall below the top-ten byte-budget ranking:

| Source | Characters | Median / p95 | Occurrences/hour | Interpretation and proposed direction |
| --- | --- | --- | --- | --- |
| Coordination success/ack/status receipts | 2,059,616 | 272 / 581 | 8.54 | Keep message ID, route, stage and confirmation semantics; collapse repeated explanation. A 200-character default models up to 1,148,449 characters saved. |
| Inbox framing outside bodies | 1,627,272 | 131 / 279 | 14.64 | Use one short turn-level framing line; retain sender/ID provenance. |
| Supervisor reminders | 1,051,188 | 410 / 429 | 3.46 | 2,504 exact repeated spans; stable contract plus short changed-rule reminder could save up to 847,348 characters at an 80-character default. Producer: `prompt_capture.rs:19`, `build_supervisor_reminder`. |
| Sibling notes on task start | 559,101 | 34,431 / 47,113 | 0.03 | Only 20 spans, but very large when present. Use the existing brief start by default and require relevant sibling expansion. |
| Predicted QA appendices | 587,266 | 569 / 754 | 1.21 | 890 occurrences. Omit empty/non-route tables and render changed impact once; retain the authoritative per-delivery impact and journey evidence. Producer: `task/lifecycle/qa_impact.rs:131`; appended to task notes during creation. |
| Verification/QA dispatch bodies | 872,947 | 1,393 / 2,216 | 0.82 | 310/604 matched actions. Compact the copied close reason, preserve dispatch/round/SHA semantics. Producer: `prompt_revalidation.rs:1392` and `task/lifecycle/supervisor_push.rs:1168`. |
| Director/stall prompts | 587,104 | 289 / 927 | 1.71 | 454/1,258 matched actions. Fix epic scope and coalesce episodes; the ongoing cas-c73a addresses the 52-task cross-epic nudge. |
| READY TO MERGE bodies recognized at message start | 2,085 | 558 / 558 | 0.01 | Only four recognized first bodies, three with matches. This is not evidence of a widespread duplicate-ready-body problem. Later copies may be in the repeated-mail bucket; embedded mentions are not counted as relays. |

The director is still worth fixing despite its modest direct character share:
an unnecessary wake can replay a large cached conversation and provoke tool
calls or unrelated assignments. This tally measures the new prompt text, not
that causal downstream cost. Its producing paths are
`crates/cas-factory/src/director.rs:476` (ready-task collection),
`cas-cli/src/ui/factory/daemon/runtime/lifecycle.rs:269` (stall relay), and
`:1386` (blocker framing). READY TO MERGE production is
`cas-cli/src/ui/factory/app/merge_alert.rs:330` and `app/mod.rs:1917`.

## Queue and log corroboration

Read-only SQLite queries found 13,717 in-window prompt-queue rows totaling
10,093,913 characters; 562 rows and 511,469 characters belong to the named
factory. The factory had 41 director rows totaling 40,093 characters. There
were 57 target/body groups with exact queue-row repetition, representing 212
extra rows and 75,979 extra characters in the broader queue; none in the named
factory cohort. This is a different question from one row being rendered
twice. Delivery stage, acknowledgment, and assumed-seen counts remain separate
in the JSON; none is treated as proof that a model acted.

Eight dated Cassy log files contributed 1,390,464 lines (239,826,453 bytes) to a
content-free marker tally: 22 `supervisor_stalled` and 23 `worker_idle` matches.
Log selection is by overlapping UTC date file, so boundary days are coarse.
No log bytes or queue bytes are added to transcript input totals. Suppressed,
abandoned, queued, or acknowledged rows are not assumed to have reached a model.

## Follow-up titles, one per ranked row

1. **Bound task show and notes by bytes; make scoped brief reads the factory default.**
2. **Project a role/task skill catalog at launch and discover the full catalog on demand.**
3. **Suppress unrequested supervisor Code recall and cap ambient packets around exact task/epic evidence.**
4. **Consolidate duplicate worker/skill obligations and load phase-specific references once.**
5. **Return compact merge/close gate receipts with immutable detail references.**
6. **Unify recipient delivery state across hook/inbox/Teams and replace eligible repeat bodies with deltas.**
7. **Replace copied QA-bundle lists and proof prose in relays with one evidence manifest.**
8. **Bound successful MCP verification/search/context receipts while preserving full failure findings.**
9. **Digest routine lifecycle transitions by episode while keeping blockers immediate.**
10. **Inject stable SessionStart context by fingerprint and send only changed resume context.**

For each implementation, replay a fixed sanitized fixture through the real
producer, measure bytes before/after with the same command, and prove that
required actions and evidence remain available. For repeat delivery, separately
exercise first delivery, polled but unacknowledged, explicitly acknowledged,
replied-to, urgent, restarted, and two-recipient paths. Then compare live
same-role/session cohorts, including model requests and cache input, to determine
whether byte savings actually reduce total tokens or improve completion.

## Reproduction and evidence

The standard-library script is
[`scripts/tally_cassy_input.py`](scripts/tally_cassy_input.py). Run it locally
against the original private sources; do not upload those sources. Set
`TASK_ARTIFACT_DIR` to the configured cas-da94 artifact directory and `CAS_REPO`
to the main checkout:

```bash
python3 docs/analysis/scripts/tally_cassy_input.py \
  --start 2026-10-13T17:57:00Z --end 2026-10-27T17:57:00Z \
  --supervisor-session b34c9303-968c-4ef1-b7ce-c9d466b0876e \
  --db "$CAS_REPO/.cas/cas.db" --logs "$CAS_REPO/.cas/logs" \
  --output "$TASK_ARTIFACT_DIR/tally.json"
```

The artifact bundle contains this report, the exact script, `tally.json`, the
numeric-only run log, and the acceptance receipt. Date filtering freezes the
transcript window; live registry, queue stages, and log files can still change,
so the published numeric snapshot is the result of record. No private database
or transcript copy is included. No actual transcript snippets, secrets,
credentials, customer text, or customer project names are published.

Limitations: source/action classification is heuristic for wrapper tools and
mixed shell reads; hidden prompt/schema text is not observable here; role and
factory membership inference is incomplete; causal downstream wake cost is not
measured; successful action correlation is not usefulness proof. Shared-parent
subagent context may affect true provider input beyond the canonical messages
recorded in each file. These limitations prevent a defensible billed-token
attribution or guaranteed savings forecast.

Design voice: `DESIGN.md@b43940e43`, inherited Commander voice; Markdown research
output. No product/runtime behavior changed.
