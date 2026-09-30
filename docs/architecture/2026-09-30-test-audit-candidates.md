# Test delivered frames through one terminal exchange

<figure id="hero-figure" aria-labelledby="hero-title"><svg viewBox="0 0 324 186" role="img" aria-labelledby="hero-title hero-desc"><title id="hero-title">The guard sees itself while production can reverse</title><desc id="hero-desc">Both the original and reversed production order pass because the guard searches its own earlier string literals.</desc><text x="12" y="19">Guard sees its own strings</text><rect class="current" x="12" y="30" width="130" height="40"/><text x="77" y="55" text-anchor="middle">request literal</text><path class="arrow" d="M142,50 h28 l-4 -3 m4 3 l-4 3"/><rect class="current" x="170" y="30" width="142" height="40"/><text x="241" y="55" text-anchor="middle">drain literal</text><text x="12" y="99">Reversed production still passes</text><rect class="decisive" x="12" y="110" width="130" height="40"/><text x="77" y="135" text-anchor="middle">PTY drain</text><path class="arrow" d="M142,130 h28 l-4 -3 m4 3 l-4 3"/><rect class="current" x="170" y="110" width="142" height="40"/><text x="241" y="135" text-anchor="middle">keyframe</text><text x="12" y="176">Test finds lines 816 → 819 in both cases.</text></svg><figcaption>Source mutation inspection at 7ebc592583e21aa3d1d507c41467e193e259325c; 2026-09-30 UTC. This proves the guard is ineffective, not that output loss occurred.</figcaption></figure>

## Decision context

Practitioner decision brief · 2026-09-30. Recommend the daemon terminal-exchange
candidate for supervisor selection and scheduling. The confirmed defect is a
self-matching test; no terminal-output loss was reproduced. The interface below
is proposed, and no production refactor ships in this report.

Scope: the 16 SOURCE-TEXT entries in the supplied test-audit.txt and the
builtins.rs prose-pin cluster, inspected at `7ebc592583e21aa3d1d507c41467e193e259325c`.
Two independent read-only explorations examined actual callers and tests.
The scan is targeted, not a whole-repository architectural assessment.

## Options

| Candidate | Strength | Dependency category | Cost / effort | Risk | Reversibility | Outcome |
| --- | --- | --- | --- | --- | --- | --- |
| Terminal exchange — recommended | Strong | In-process + local-substitutable PTY/WS | Medium; focused daemon seam and protocol test | Activity accounting or ordering regression | Private interface can be inlined | Frame delivery replaces ineffective ordering guard |
| Canonical harness catalog construction | Worth exploring | In-process; installation uses local filesystem | Medium; catalog compatibility and consumers | Public constants and observable file order | Retain constants during migration | Common membership and harness exceptions have one owner |
| Task delivery evidence measurement | Speculative | In-process + local-substitutable Git repositories | High; close, receipts and epic status | Authorization/status semantics can drift | Stage consumers separately | One measurement hides topology and content attribution |

## Why terminal exchange

The deciding criterion is a demonstrated gap between a test's claimed contract
and what it can detect. `lifecycle.rs:813` includes its own file; searches at
`:816` and `:819` find their own string literals before production at `:1910`
and `:1914`. Swapping only the production statements in memory keeps the
asserted comparison true. No Rust test was executed in this worker scan.

| Inspection | First request / drain matches | Actual request / drain | Existing comparison |
| --- | --- | --- | --- |
| Baseline source | 816 / 819 | 1910 / 1914 | True |
| Production order reversed in memory | 816 / 819 | 1914 / 1910 | True |

### Candidate 1: own terminal exchange

<figure aria-labelledby="terminal-title"><svg viewBox="0 0 324 162" role="img" aria-labelledby="terminal-title terminal-desc"><title id="terminal-title">One operation owns snapshot, drain and ordered enqueue</title><desc id="terminal-desc">Before: Daemon / WS input / PTY drain. After (planned): Daemon / Exchange.</desc><text x="12" y="18">Before · call order</text><rect x="12.0" y="28" width="88.0" height="38" class="current"/><text x="56.0" y="52" text-anchor="middle">Daemon</text><path d="M100.0,47 h12 m0 0 l-4 -3 m4 3 l-4 3" class="arrow"/><rect x="112.0" y="28" width="88.0" height="38" class="current"/><text x="156.0" y="52" text-anchor="middle">WS input</text><path d="M200.0,47 h12 m0 0 l-4 -3 m4 3 l-4 3" class="arrow"/><rect x="212.0" y="28" width="88.0" height="38" class="current"/><text x="256.0" y="52" text-anchor="middle">PTY drain</text><text x="12" y="102">After · planned</text><rect x="12.0" y="112" width="138.0" height="38" class="planned"/><text x="81.0" y="136" text-anchor="middle">Daemon</text><path d="M150.0,131 h12 m0 0 l-4 -3 m4 3 l-4 3" class="arrow"/><rect x="162.0" y="112" width="138.0" height="38" class="planned"/><text x="231.0" y="136" text-anchor="middle">Exchange</text></svg><figcaption>Before: Daemon / WS input / PTY drain. After (planned): Daemon / Exchange.</figcaption></figure>

Arrows show call order; the proposed exchange owns the sequence.

Source: `lifecycle.rs:1910–1918`, `runtime/ws_client.rs:375–399`,
`runtime/queue_and_events.rs:2958–2969`, inspected at the scope revision above.
The after path is planned. WS request captures a pane snapshot and feeds the
encoded keyframe; PTY event handling later feeds encoded output to WS clients.

Problem: the caller owns the ordering contract across separate operations,
while the source guard fails to observe production order.

Proposed interface sketch: a private terminal-exchange operation accepts
pending requests, panes and client outboxes and returns exchange activity.
It owns snapshot capture, backend drain and ordered frame enqueue. Preserve
protocol encoding and pane-name resolution; this is not a new public API.

Deletion test: wrapping the two existing calls merely moves complexity.
Moving request/snapshot/drain/enqueue ownership together earns depth: deleting
the new module would expose those sequencing facts to daemon/client code again.

Benefits: callers learn activity rather than ordering; a focused test observes
real encoded keyframe then output, with bytes pending, and reconstructs terminal
state. In-process snapshot/protocol behavior needs no mock; a deterministic
queued-byte PTY stand-in and local transport exercise the varying local seam.

Counter-evidence: the current production order is correct. An existing broader
frame-delivery test may already cover this scenario; check it before adding a
second harness. A public trait or callback scheduler would add interface cost
without proving transport delivery.

### Candidate 2: derive harness catalogs

<figure aria-labelledby="catalog-title"><svg viewBox="0 0 324 162" role="img" aria-labelledby="catalog-title catalog-desc"><title id="catalog-title">Canonical content plus explicit private harness exceptions</title><desc id="catalog-desc">Before: Claude / Codex / Grok. After (planned): Shared / Rules / Catalog.</desc><text x="12" y="18">Before · independent paths</text><rect x="12.0" y="28" width="88.0" height="38" class="current"/><text x="56.0" y="52" text-anchor="middle">Claude</text><rect x="112.0" y="28" width="88.0" height="38" class="current"/><text x="156.0" y="52" text-anchor="middle">Codex</text><rect x="212.0" y="28" width="88.0" height="38" class="current"/><text x="256.0" y="52" text-anchor="middle">Grok</text><text x="12" y="102">After · planned</text><rect x="12.0" y="112" width="88.0" height="38" class="planned"/><text x="56.0" y="136" text-anchor="middle">Shared</text><path d="M100.0,131 h12 m0 0 l-4 -3 m4 3 l-4 3" class="arrow"/><rect x="112.0" y="112" width="88.0" height="38" class="planned"/><text x="156.0" y="136" text-anchor="middle">Rules</text><path d="M200.0,131 h12 m0 0 l-4 -3 m4 3 l-4 3" class="arrow"/><rect x="212.0" y="112" width="88.0" height="38" class="planned"/><text x="256.0" y="136" text-anchor="middle">Catalog</text></svg><figcaption>Before: Claude / Codex / Grok. After (planned): Shared / Rules / Catalog.</figcaption></figure>

Before boxes are independent lists. Planned arrows resolve canonical content
and private exceptions into one harness catalog.

Source: `builtins.rs:94`, `:736`, `:1373`, `:2342`, `:3127`, `:4006`;
`cli/doctor.rs:547`, `:669`; `tests/support/builtin_catalog.rs:9–29`;
`builtin_flavor_drift_test.rs:79–130`, `:247`. Line numbers refer to the scope
revision, before this delivery adds the new skill.

At that revision Claude has 158 skill entries, Codex 161 and Grok 158.
Grok membership and embedded sources equal canonical membership. Codex has
157 common entries, replaces the shared supervisor checklist and adds three
policy YAML entries: 315 registrations repeat shared declarations, not bodies.
Grok ordering differs; membership parity does not establish order compatibility.

Problem: adding common content touches three complete production lists; tests
separately model permitted differences. Prose-pin proliferation amplifies the
routing knowledge, but moving phrases to a registry does not own catalog assembly.

Proposal: keep catalog accessor entry points, backed by a canonical embedded
registry and narrow private harness exceptions. Reject duplicate paths, missing
replacement targets and obsolete exceptions. Preserve actual agent availability
and generated frontmatter projections; filesystem installation remains separate.

Deletion test: today's selector merely dispatches. Deleting the proposed
construction module would make sync, preview, doctor and tests recover
inheritance, exceptions and projection independently. Moving arrays to another
file would fail this test.

Trade-off: aliasing only Grok is cheap but leaves policy scattered; per-file
capability metadata adds a policy language that current variation does not need.
The common-caller design deserves design-it-twice before changing public constant
compatibility or the observable order of `SyncResult.updated_files`.

### Candidate 3: measure task delivery evidence once

<figure aria-labelledby="delivery-title"><svg viewBox="0 0 324 162" role="img" aria-labelledby="delivery-title delivery-desc"><title id="delivery-title">Shared measurement preserves separate authorization policies</title><desc id="delivery-desc">Before: Close / Receipt / Status. After (planned): Git state / Measure / Policies.</desc><text x="12" y="18">Before · independent paths</text><rect x="12.0" y="28" width="88.0" height="38" class="current"/><text x="56.0" y="52" text-anchor="middle">Close</text><rect x="112.0" y="28" width="88.0" height="38" class="current"/><text x="156.0" y="52" text-anchor="middle">Receipt</text><rect x="212.0" y="28" width="88.0" height="38" class="current"/><text x="256.0" y="52" text-anchor="middle">Status</text><text x="12" y="102">After · planned</text><rect x="12.0" y="112" width="88.0" height="38" class="planned"/><text x="56.0" y="136" text-anchor="middle">Git state</text><path d="M100.0,131 h12 m0 0 l-4 -3 m4 3 l-4 3" class="arrow"/><rect x="112.0" y="112" width="88.0" height="38" class="planned"/><text x="156.0" y="136" text-anchor="middle">Measure</text><path d="M200.0,131 h12 m0 0 l-4 -3 m4 3 l-4 3" class="arrow"/><rect x="212.0" y="112" width="88.0" height="38" class="planned"/><text x="256.0" y="136" text-anchor="middle">Policies</text></svg><figcaption>Before: Close / Receipt / Status. After (planned): Git state / Measure / Policies.</figcaption></figure>

Before boxes reconstruct evidence independently. Planned arrows pass Git
evidence through measurement to the distinct policies.

Source: `mcp/tools/core/task/lifecycle/close_ops.rs:10716`, `:14181`,
`:14282`, `:15735`, `:19281`; consumer `service/factory_ops.rs:6330`.
Existing real-Git tests at `close_ops.rs:25060` and `:25431` remain valuable.

Problem: close, receipt validation and epic status reconstruct anchor/target
selection, merge attribution and content outcomes along multiple paths.

Proposal: one read-only measurement returns resolved anchor/target, ancestry,
attribution, content outcome and uncertainty. Callers retain their distinct
mutation authorization and presentation policies. Remote fetch remains outside
measurement; reading origin tracking refs is a local operation.

Deletion test: extracting the already deep content-presence helper merely moves
it. The candidate earns depth only when consumers stop rebuilding the same
measurement. Real temporary Git repositories test topology and content; command
mocks cannot establish that behavior.

Trade-off: this has greater potential reach but touches close authorization.
No divergence was executed or established here. Retain the existing behavioral
fixtures; moving the entire close file would only relocate complexity.

## What we give up

Catalog construction could remove repeated registration from every future skill
change. It loses first priority because public constants, order and consumer
migration require more compatibility work, while terminal exchange has a
concrete false-confidence reproducer. Prose consolidation stays with cas-e229;
test-shape lint stays with cas-0c55.

## Reversal cost

Terminal exchange stays private and can be inlined without changing protocol
messages. Keep the behavioral test at the delivery interface. Rollback is
medium effort if callers have already migrated; no storage migration is proposed.

## Audit disposition

The audit is a search aid, not a verdict that 16 tests should be removed.

| Audited entry | Observed surface | Disposition |
| --- | --- | --- |
| skill_hygiene::release_notes_are_generic_procedure_and_rubric_driven | Instruction contract, test :74 | Retain justified doc contract; prose registry owns consolidation |
| factory_codex_skill_guardrails::supervisor_reference_tree_uses_current_lifecycle_contract | Instruction/reference contract, test :309 | Retain contract; deduplicate pins in existing work |
| worktree_surface::process_home_mutation_uses_the_canonical_test_env_guard | HOME guard usage, test :39 | Structural guard; retain |
| retrieval_eval::the_production_runner_documents_its_unreplicated_surface | Documentation contract, test :630 | Retain justified documentation contract |
| retrieval_eval::the_full_harness_has_a_named_sixty_second_budget | Times run on actual fixture, test :684 | Mixed behavioral/source; preserve timing proof |
| retrieval_eval::the_documented_rebaseline_switch_is_the_one_the_harness_reads | Reads own source, test :1206 | Weak self-reference; inspect actual configuration boundary |
| warning_hygiene::warning_only_symbols_are_scoped_to_the_builds_that_use_them | Compilation hygiene, test :17 | Structural contract; retain |
| factory_target_cache::symlink_target_escape_is_reported_and_never_removed | Real temporary symlink fixture, source :985 | Behavioral test; audit classification misleading |
| builtins::test_worker_discipline_reference_is_linked_and_assembly_proof_is_named | Instruction contract, source :4833 | Registry consolidation remains separate work |
| server_signals::every_long_lived_server_entry_point_ignores_sigpipe | Source entry-point wiring, source :181 | Supplement; real process behavior already at :107/:117 |
| server_signals::short_lived_commands_keep_mains_sigpipe_default | Source wiring, source :256 | Supplement; retain explicit scope distinction |
| mcp/runtime::casb123_mcp_startup_arms_parent_watchdog_before_schema_migration | Source ordering, source :1612 | Wiring supplement; not executed watchdog proof |
| close_ops::reachable_anchor_with_dropped_content_blocks_close_and_epic_status_cas_b278 | Real Git fixture, source :25060 | Behavioral test; preserve |
| close_ops::reachable_anchor_with_later_refactor_proceeds_cas_b278 | Real Git fixture, source :25431 | Behavioral test; preserve |
| daemon/lifecycle::websocket_keyframe_requests_stay_before_pty_delta_drain | Own-source first matches, source :813 | Replace after frame-delivery proof exists |
| migrations::test_every_migration_file_is_declared_and_registered | Orphan inventory, source :592 | Intentional structural contract; retain |

## Open questions and next action

Candidate task **cas-60c5** records terminal exchange, evidence, constraints and
behavioral acceptance. It remains unassigned and unstarted; the supervisor owns
selection and scheduling. This report implements the scan/report/task stage.
Chosen production interface and design trade-offs remain pending; settle them
in Cassy spec and memory through cas-codebase-design's design-it-twice.

Falsification: an existing frame-delivery proof that demonstrably fails under
production-order reversal reduces this candidate to removing an ineffective
supplement; a proposed extraction that merely sequences two callbacks fails the
depth claim. No execution evidence supports the task-delivery refactor yet.

## Provenance

- Examined commit: `7ebc592583e21aa3d1d507c41467e193e259325c`; one snapshot, no time-window trend.
- Input: supplied `video-eval/test-audit.txt`, SOURCE-TEXT section; 16 named
  entries. Builtin phrase count comes from the sibling consolidation brief,
  not a newly measured assertion count here.
- Commands: `git show 7ebc592583e21aa3d1d507c41467e193e259325c:cas-cli/src/builtins.rs`;
  `rg -n 'process_ws_client_input|RequestPaneKeyframe|Output' cas-cli/src/ui/factory/daemon/runtime/ws_client.rs`;
  `sed -n '800,830p' cas-cli/src/ui/factory/daemon/runtime/lifecycle.rs`;
  `sed -n '1890,1950p' cas-cli/src/ui/factory/daemon/runtime/lifecycle.rs`.
- Reproducible inspection: `python3 /home/pippenz/.cas/artifacts/cas-ddb6/inspect-hotspots.py`;
  raw output `inspection.log` beside that script. It substitutes only the two
  production statements in memory and compares first and last string matches.
- Proof boundary: read-only code inspection and Python source mutation only.
  Rust execution, timing improvement and shipped architectural benefit are
  not verified; the supervisor's assembly owns Rust proof.
