# Worker steering and projection change review

Task: cas-5ced. Comparison base: `7ebc592583e21aa3d1d507c41467e193e259325c`.
Count whitespace-separated tokens with Python `len(text.split())`; file counts
include frontmatter, tables and command examples. Prompt counts use the decoded
JSON string, including OpenCode's plugin suffix. These are words, not model tokens.
The historical brief's approximate counts used an earlier revision.

| Surface | Before words | After words | Change |
| --- | ---: | ---: | ---: |
| AGENTS.md | 738 | 264 | -474 |
| Worker skill | 873 | 483 | -390 |
| Close delivery reference | 2580 | 641 | -1939 |
| Evidence/sync reference | 1108 | 469 | -639 |
| Check/test command reference | 517 | 517 | +0 |
| Recovery reference | 1633 | 493 | -1140 |
| OpenCode worker launch prompt | 781 | 406 | -375 |
| Four references total | 5838 | 2120 | -3718 |

The launch brief retains assignment, one-task ownership, progress, commit/push,
close/handoff, typed wakes, urgent-stop recovery, availability, backgrounding,
checkpointing and evidence-preserving output. The worker skill keeps the lifecycle
and return contract. Command details, delivery receipts, evidence/store access,
sync and recovery have conditional pointers. Worker references carry delivery
facts; code/test-quality judgments belong to review. The unchanged managed
AGENTS block still matches initialization; the project additions now route
build, diagnosis, architecture and publication branches to their owned sources.

## Snapshot approval note

Changed file: `crates/cas-mux/src/opencode_projection.snapshot.json`, line 30,
`agent.cassy-worker.prompt`. This is the only changed JSON value. The prompt now
routes to the worker skill at startup and to its discipline reference before
checks/tests; duplicate reporting exposition and full command specifications
leave the launch brief. The OpenCode namespace and plugin suffix are preserved.

The assigned task authorizes the trim; supervisor message 1871406 confirms the
judgment-only review split and the upstream smell source. Snapshot diff approval
is requested from the supervisor at merge; this note does not claim that review
has happened. Record the approval receipt here after review.

Refresh method: decode the canonical Rust format string, apply the OpenCode
identity/prefix and append `role_prompt`'s existing suffix. Worker Rust execution
is prohibited in the current session, so the runtime snapshot generator and
`projection_config_is_deterministic_and_contains_both_primary_agents` remain supervisor
assembly obligations. Static source-to-snapshot parity is independent evidence,
not a substitute for executing the renderer.

## Verification ownership

Worker source checks cover canonical contract markers, shared three-harness
registrations, resolved pointers, reduced SessionStart body size, unchanged
managed initialization block, JSON parity, MIT notice and Markdown hygiene.
Supervisor assembly owns `cas-pty` worker contracts/parity, `cas-mux` projection,
`cas` builtin worker/role guidance and SessionStart budget suites, plus the
independent Standards-axis loading integration from cas-fdfa.


## Contract restoration after review

Supervisor request 1882566 records seven executed contract failures missed by
the initial source checks. The corrective delivery rebases onto `aa64c907f` and
restores the pinned guidance before preserving the remaining word-count trim.

| Failed surface | Corrective evidence |
| --- | --- |
| Verifier projection | `.claude/agents/task-verifier.md` equals the canonical generated header plus `task-verifier.body.md`. |
| Worker reminders | Restored the pointer to `../cas-supervisor/references/reminders.md`. |
| Task-field examples | Restored the original exact ID/notes field explanation and start/show/close/notes/blocked examples in `details.md`. |
| Pre-close proof | Worker lifecycle directly points to `verify-before-claim` before close. |
| MCP call shapes | Recovery message includes target, summary, message and typed merge request on one complete command line. |
| Task-ID hygiene exception | Removed the stale close-gate task-ID entry from `scripts/builtin-doc-hygiene.json`. |
| AI-vocabulary exception | Removed the stale worker-body journey entry from `builtin_doc_hygiene_test.rs`. |

Source checks now cover these seven surfaces as well as the original counts,
pointer/contract markers and snapshot parity. They still do not execute Rust
contracts. The supervisor runs all of `integration_contracts` and
`builtin_flavor_drift_test` when this delivery parks; acceptance stays dependent
on that executed evidence, rather than a claim based on the source scan.
