# GitHub reconciliation and release scope — 2026-10-17

As of 2026-10-17T18:48:31.356394+00:00. Task `cas-fee8`; source reviewed at `7cb794a27861714da22814e9ef6da32e5e66069b`.

**Result: 27 open before → 27 open after; 0 closures, 0 dedupes, 0 new GitHub issues.** Seven canonical tasks were queued, with no new worker starts. Eleven issue comments were posted; eight of those were amended in place for the explicit release-scope decision. The 15 issues closed earlier in this UTC day are a separate historical count, not closures by this sweep.

No remaining issue had independently verified complete acceptance. Source-only fixes, released partial fixes, deferred design decisions and absent live/upstream proof are distinguished below. Closing a Cassy task does not close its GitHub issue.

## Counts by accountable supervisor

Counts below are GitHub issues, not task rows or completed product fixes. Accepted peer ownership is preserved.

| Supervisor / disposition | Open issues | Canonical task states |
| --- | ---: | --- |
| true-leopard-45@soundwave | 24 | blocked 5, open 16, awaiting_merge 2, in_progress 1 |
| daring-leopard-21@prowl | 2 | open 1, blocked 1 |
| Explicit deferred decision; no active fix owner | 1 | closed 1 |

There are 16 open/unassigned canonical tracks and 1 open/assigned-or-held tracks. There are 24 product-fix tracks, 2 rollout/proof tracks (#1215 and #1236), and 1 deferred architectural/measurement disposition (#1247). QA tasks, this administrative sweep, and completed diagnostic subtasks are not counted as product fixes. Queued/unassigned canonical tasks are listed by their actual state below; the held Prowl six-ask lane is not silently reassigned.

## Every open issue

| Issue | Canonical task / disposition | Owner | State | Release | Remaining acceptance |
| --- | --- | --- | --- | --- | --- |
| [#1215](https://github.com/Richards-LLC/cassy/issues/1215) | `cas-23a8` | prowl | open | 3.64 | Six-ask/two-machine acceptance incomplete; source shipping and Cloud receipts do not prove owner-only actions. Merge lock/identity proposals remain explicitly unproved. |
| [#1236](https://github.com/Richards-LLC/cassy/issues/1236) | `cas-2e5a` | soundwave | blocked | 3.63 | CI redesign merged only on epic; rollout and >=10 real-delivery median/ejection proof pending installation and operator switch. |
| [#1246](https://github.com/Richards-LLC/cassy/issues/1246) | `cas-979e` | soundwave | blocked | 3.63 | Released inline fix6e355 and worker follow-up901f do not close missing live supervisor acceptance; distinct Codex recurrence353b remains blocked. |
| [#1247](https://github.com/Richards-LLC/cassy/issues/1247) | `cas-77d6` | deferred | closed | deferred | Visibility is in main/v3.62; shared service and macOS allocator explicitly deferred; long-session/native benefit unproved. Closed task is not full issue acceptance. |
| [#1249](https://github.com/Richards-LLC/cassy/issues/1249) | `cas-bf3e` | soundwave | open / unassigned | 3.64 | Queued approved cache-variable passthrough; launcher proof pending. |
| [#1251](https://github.com/Richards-LLC/cassy/issues/1251) | `cas-e592` | soundwave | open / unassigned | 3.64 | Queued Violet attachment trace; CLI construction tests do not prove returned reply attachments. |
| [#1253](https://github.com/Richards-LLC/cassy/issues/1253) | `cas-e4d5` | soundwave | open / unassigned | 3.63 | Original startup guidance retained3.63; current checklist still treats missing-source/stale as mandatory rebuild/reconnect. |
| [#1254](https://github.com/Richards-LLC/cassy/issues/1254) | `cas-9a51` | soundwave | open / unassigned | 3.64 | Queued component bug routing; current resolve_issue_repo prefers project tracker. |
| [#1256](https://github.com/Richards-LLC/cassy/issues/1256) | `cas-bdfc` | prowl | blocked | 3.63; peer retarget to3.64 pending | Peer-owned guidance/wake trace blocked; preserve accepted split and peer retarget ownership. |
| [#1257](https://github.com/Richards-LLC/cassy/issues/1257) | `cas-f9b7` | soundwave | blocked | 3.63 | Cloud184 lossless storage/roundtrip and actual supported drain unproved.7c6b diagnostics only on epic, not released. |
| [#1258](https://github.com/Richards-LLC/cassy/issues/1258) | `cas-2b85` | soundwave | open / unassigned | 3.64 | Queued missing-Serve-shim diagnosis; not verified fixed. |
| [#1259](https://github.com/Richards-LLC/cassy/issues/1259) | `cas-1e26` | soundwave | open / unassigned | 3.63 | Original update false-complete follow-up remains queued on3.63; no guard bypass or successful refresh claimed. |
| [#1260](https://github.com/Richards-LLC/cassy/issues/1260) | `cas-24d3` | soundwave | open / unassigned | 3.64 | Queued legacy-column/schema diagnostic and preserving repair. |
| [#1261](https://github.com/Richards-LLC/cassy/issues/1261) | `cas-5c12` | soundwave | open / unassigned | 3.64 | Queued unenrolled-outbox SLO denominator correction. |
| [#1262](https://github.com/Richards-LLC/cassy/issues/1262) | `cas-85de` | soundwave | open / unassigned | 3.64 | Queued confirmed alias-fold guidance; no Accounting mutation. |
| [#1263](https://github.com/Richards-LLC/cassy/issues/1263) | `cas-edb0` | soundwave | awaiting_merge | 3.63 | Retained trusted-hook prerequisite; blocked on trusted supported hook source, not fixed by registry cleanup. |
| [#1264](https://github.com/Richards-LLC/cassy/issues/1264) | `cas-978b` | soundwave | open / unassigned | 3.64 | Queued authenticated watchdog reply-route investigation; producer unconfirmed. |
| [#1265](https://github.com/Richards-LLC/cassy/issues/1265) | `cas-16c0` | soundwave | open / unassigned | 3.64 | Queued explicit-account connector isolation; authorized real-profile discovery proof required. |
| [#1266](https://github.com/Richards-LLC/cassy/issues/1266) | `cas-b1d0` | soundwave | open / unassigned | 3.64 | Queued verified-unmanaged guidance; managed denials must remain fail-closed. |
| [#1267](https://github.com/Richards-LLC/cassy/issues/1267) | `cas-9021db58f400ac61` | soundwave | open / unassigned | 3.64 | Accepted canonical source proposal; bf6a superseded. Cloud/client mapping contract still pending; all Violet work3.64. |
| [#1268](https://github.com/Richards-LLC/cassy/issues/1268) | `cas-5986` | soundwave | awaiting_merge | 3.63 | Retained data-safety fix awaiting merge/QA; no live queue purge or transport success inferred from rejection. |
| [#1269](https://github.com/Richards-LLC/cassy/issues/1269) | `cas-353b` | soundwave | blocked | 3.63 | Valid transport RED/e7 dependency GREEN retained; installed idle-model turn proof missing. Gabber mid-turn observation separate. |
| [#1270](https://github.com/Richards-LLC/cassy/issues/1270) | `cas-7e98` | soundwave | open / unassigned | 3.64 | Queued effective known-repos JSON flag wiring. |
| [#1271](https://github.com/Richards-LLC/cassy/issues/1271) | `cas-d16e` | soundwave | open / unassigned | 3.64 | Queued successful-write/failed-pattern-response decoder investigation. |
| [#1272](https://github.com/Richards-LLC/cassy/issues/1272) | `cas-f869` | soundwave | open / unassigned | 3.64 | Existing canonical reply-intent client; optional occurrence identity; no duplicate task. |
| [#1273](https://github.com/Richards-LLC/cassy/issues/1273) | `cas-8875` | soundwave | in_progress | 3.64 | Existing canonical source-message age correction; distinct from replay ordering. |
| [#1274](https://github.com/Richards-LLC/cassy/issues/1274) | `cas-b94d` | soundwave | blocked | 3.64 | METADATA ONLY: existing replay-order containment task retained; no incident bodies/comments read or repeated. |

## Release-scope ledger

The operator/supervisor decision in notification4197521 makes newly discovered noncritical work and **all Violet work, including critical Violet fixes, 3.64**. This is a release assignment, never a claim that containment or live acceptance is finished. Foreign Violet projects/epics were not imported or moved. Root freezes further starts except containment and verified original release blockers.

| Scope | Tasks | Treatment |
| --- | --- | --- |
| Original startup | `cas-979e`, `cas-353b`, `cas-e4d5` | Keep3.63; actual startup/model-turn proof still owed. e4d5 is guidance, not the cadence fix. |
| Original updater / cleanup / data safety | `cas-edb0`, `cas-5986`, `cas-4585`, `cas-70a8`, `cas-6b56` | Retain3.63; trusted hooks, no-loss transport, cleanup, readable output and captures keep their existing gates. |
| Existing operator/upstream blockers | `cas-f9b7`, `cas-2e5a`, plus the original startup proof | Preserve Cloud184 lossless drain, post-install/operator CI enablement and live-kit gates; no closure or bypass. |
| Violet source and proof | Root-moved eight tasks listed below |3.64 even where critical; retain IDs, evidence and source branches. No live activation/replay change here. |
| New noncritical follow-ups | Thirteen worker-moved tasks listed below |3.64; no start, cancellation, deletion or extra3.63 blocker. |
| Peer-owned retargeting | `cas-49b9`, `cas-bdfc` | Prowl owner controls retargeting; not altered by this worker. |

### Exact transfer receipt

**13 worker moves + 8 root moves independently verified = 21 tasks with sole parent `cas-ebf2` and exact v49 WorkTarget. Six explicitly retained tasks were verified on `cas-5c13` and exact v48 WorkTarget.**

V49 target: `epic/burn-down-v49-ships-as-3-64-noncritical-follow-ups-cas-ebf2`.

| Operation / owner | Task IDs |
| --- | --- | --- |
| Moved by this worker (13) | `cas-24d3`, `cas-5c12`, `cas-85de`, `cas-2b85`, `cas-7e98`, `cas-d16e`, `cas-be05`, `cas-98ba`, `cas-37b8`, `cas-bf3e`, `cas-9a51`, `cas-978b`, `cas-16c0` |
| Moved by root, independently verified (8) | `cas-23a8`, `cas-b94d`, `cas-9021db58f400ac61`, `cas-6ace`, `cas-8875`, `cas-f869`, `cas-e592`, `cas-b1d0` |
| Explicitly retained and verified (6) | `cas-e4d5`, `cas-edb0`, `cas-5986`, `cas-4585`, `cas-70a8`, `cas-6b56` |

For all13 worker moves, old v48 parent edges are absent and tombstoned, and IDs/status/assignee/source branch/external refs equal the pre-move snapshot. Independent verification caught a dual parent on root-owned6ace; root removed its obsolete5c13 edge (4197548), and the final read confirms its sole parent is now ebf2. No parked source branch was reset.

## Evidence reviewed and closure decisions

- `source-ancestry.json`:10a894c23 is in main and v3.62; b658381fa (CI redesign) and b25d1934d (NUL diagnostics) are only on the epic;86a414616 inline cadence fix is released in3.61. These partial facts do not satisfy broader issue acceptance.
- `.github/workflows/ci.yml:126,140,359` retains CAS_LANE_SLIM gating; CI run39037851334 was read as evidence only. No switch was changed and no >=10-delivery speedup was invented.
- `docs/design/shared-serve-proposal.md:3` explicitly defers shared serve and macOS allocator changes; the long-session/native uncertainty stays visible on #1247.
- `system.rs::resolve_issue_repo` still prefers issues.repo; the Codex checklist still has blanket stale/rebuild guidance. Existing Violet reply-file plan tests do not prove the reported attachment result. These are queued source investigations, not claimed fixes.
- #1246/1256/1269 overlap in symptom but have different historical inline, guidance and non-inline/turn-acceptance scopes; no duplicate closure. #1273 age and #1274 ordering remain distinct, and #1266 unmanaged guidance does not replace #1267 mapping.
- Historical replay #1274 was inspected through issue metadata and canonical task metadata only. No secret incident body was read, repeated, or committed. All database inspections used SQLite URI mode=ro. No builds, source fixes, runtime installs/restarts, factory probes, Slack posts or live-store repair occurred.
- Dependency review found353b blocked by979e and37b8 blocked by8cbe; neither blocker was complete, so no blocking edge was removed. Parent-edge retargeting was separately explicitly authorized.

## Exact GitHub comment receipts

| Issue | Receipt |
| --- | --- |
| #1249 | [comment](https://github.com/Richards-LLC/cassy/issues/1249#issuecomment-6238809105) |
| #1251 | [comment](https://github.com/Richards-LLC/cassy/issues/1251#issuecomment-6238809511) |
| #1253 | [comment](https://github.com/Richards-LLC/cassy/issues/1253#issuecomment-6238809889) |
| #1254 | [comment](https://github.com/Richards-LLC/cassy/issues/1254#issuecomment-6238810149) |
| #1264 | [comment](https://github.com/Richards-LLC/cassy/issues/1264#issuecomment-6238810460) |
| #1265 | [comment](https://github.com/Richards-LLC/cassy/issues/1265#issuecomment-6238810849) |
| #1266 | [comment](https://github.com/Richards-LLC/cassy/issues/1266#issuecomment-6238811224) |
| #1236 | [comment](https://github.com/Richards-LLC/cassy/issues/1236#issuecomment-6238811702) |
| #1247 | [comment](https://github.com/Richards-LLC/cassy/issues/1247#issuecomment-6238812107) |
| #1269 | [comment](https://github.com/Richards-LLC/cassy/issues/1269#issuecomment-6238812417) |
| #1215 | [comment](https://github.com/Richards-LLC/cassy/issues/1215#issuecomment-6238812684) |

Seven intake mappings:1249→bf3e,1251→e592,1253→e4d5,1254→9a51,1264→978b,1265→16c0,1266→b1d0. All remain queued/unassigned; only e4d5 remains3.63 under the explicit original-startup exception. Existing1272/f869,1273/8875,1274/b94d were reused without duplicates.

Machine-readable proof and exact metadata/comment payloads are in the configured `cas-fee8` artifacts directory: `reconciliation.json`, `retarget-proof.json`, `source-ancestry.json`, `comment-receipts.json`, `comment-edit-receipts.json`, `open-before.json`, `open-after.json`, `closed-today-before.json`, and all-status metadata snapshots. Raw issue bodies are not part of this tracked ledger.
