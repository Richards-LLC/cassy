# Commander v1 assembled acceptance gate

**Verdict: NOT RELEASABLE at public `v2.60.0`. Confidence: high.** The binding prowl + soundwave
phone-browser, security, fan-out, controller-arbitration, crash, compatibility, and zero-agent rows
passed. One narrow release gate failed: on stock macOS, `cas hub restart --tailscale-serve` cannot find
the Tailscale app-bundle CLI, so restart returns loopback-only and cannot republish the stable HTTPS
Serve endpoint without an undocumented manual step. The corrective `cas-a13a` is merged at epic tip
`e29b22fb`, but no immutable public artifact contains it yet.

The normative contract is
[`docs/specs/2026-08-08-commander-security-architecture.md`](../specs/2026-08-08-commander-security-architecture.md),
especially `H7-ADV-01`, `H7-FLEET-02`, `H7-INVARIANT-03`, and `H7-CRASH-04`.

## Overview

| Field | Executed result |
| --- | --- |
| Question | Is Commander v1 releasable from exact public `v2.60.0` bytes on the operator-bound prowl + soundwave topology? |
| Verdict | **No. All binding live rows passed except stock macOS hub restart cannot republish Tailscale Serve without an undocumented CLI-discovery step.** |
| Confidence | High; the public artifact, installed binaries, real browser, fixture identity, audit attribution, process/session counts, cleanup, and failing restart transition were observed directly. |
| Executed source boundary | Clean `69c3a1c6a24c1107865e3666e1cfa33ef9797615`, equal to `origin/main` before this evidence-only report commit; immutable release peel `0cb8962d`. |
| Public release | `v2.60.0`; official run `31412591350` terminal success. |
| Linux asset | 21,991,526 bytes; archive SHA-256 `b2533266…`; binary SHA-256 `8ec9dea6…`. |
| Binding machines | `prowl` controller hub and distinct `soundwave` target hub. Unicron and shield are explicitly out of scope. |
| Browser | Real Google Chrome `151.0.7922.108`; isolated phone metrics `390×844`; controller origin `prowl`. |
| Physical Android | Offline and explicitly **not claimed**. |
| Evidence window | 2026-08-10 19:51–19:58 UTC, with banked public-release/TLS evidence from earlier H7 passes. |
| Author | H7 assembled release gate (`cas-3d85`). |

## Evidence

| Observation | Redacted source | What it proves |
| --- | --- | --- |
| Public `v2.60.0` tag/workflow/assets were already authenticated; both binding hosts ran the intended public version, and soundwave's installed Linux ELF matched SHA-256 `8ec9dea6…`. | Banked release receipts; fresh local/SSH version, executable, and hub-status checks | This pass exercised public bytes rather than a local corrective build. |
| The accepted pre-auth subset remained green: TLS 1.3, minimal health, HSTS, non-loopback plaintext refusal, wrong Origin, preflight, CSRF, CSWSH, and hostile pairing exchange. | Banked forced-TLS and redacted HTTP receipts under `artifacts/cas-3d85/` | Remote control remains TLS-only and hostile browser origins receive no state or CORS grant. |
| Three fresh soundwave devices paired from the exact prowl controller origin: two full-scope controllers and one read-only observer. Each opened the deterministic two-pane session in Chrome at `390×844`. | Mode-0600 `ready-viper-55/browser.log`; phone screenshot | `H7-FLEET-02` used two real machines and a phone-class browser with direct target-hub access. |
| Controller A acquired the lease; B observed A, then force-took the lease; A observed B; the read-only observer's lease, interrupt, and message controls remained disabled. | Browser DOM assertions and attributed audit records | Concurrent control follows the ADR's single-controller lease and explicit takeover model. |
| Typed input, targeted interrupt, attributed message, and resize controls reached the fixture. Phase 1 recorded exactly one daemon connection despite three browser viewers. | `remote-phase1/fixture-events.jsonl` | Multiple pane viewers fan out through one daemon/session upstream; control mutations are attributed. |
| DPoP first use returned `200`; replay, copied-key proof, method mismatch, and observer scope escalation were browser-rejected. Consumed pairing replay returned `401`; WebSocket ticket replay produced exactly one open and one rejection. | Browser auth matrix; `audit-h7.jsonl` | `H7-ADV-01` rejects replay, stolen/copied key state, and scope escalation without exposing protected response bodies. |
| Revoking controller A moved its connection to `auth-blocked`; B remained connected. All three test devices were then revoked, leaving zero active devices. | Browser state; exact guarded CLI revocations; `auth-after.json` | Revocation is immediate and scoped; another authorized viewer remains operational. |
| Exact process identity `PID 2289931` / starttime `2571583` was killed by `SIGILL`; the durable receipt says signal `4`, `SIGILL`, `core_dumped=true`. Both B and C rendered the actionable SIGILL diagnosis while authenticated sessions and prowl health remained `200`. | `remote-phase1/daemon-exit.sigill.json`; browser diagnostic assertions | `H7-CRASH-04` is evidence-bound and does not collapse abrupt death into an invented or generic cause. |
| A protocol-v1 no-agent fixture restarted after SIGILL. B and C reloaded, reattached, and each rendered two panes; phase 2 again recorded exactly one upstream. | `remote-phase2/fixture-state.json`; `fixture-events.jsonl` | Daemon restart recovers truthfully and the current web/hub path remains compatible with the legacy protocol shape. |
| The 5.6-second browser trace contains event streams and event-triggered/session-action requests only; it contains no model/agent endpoint and no idle polling interval. | Timestamped `networkTrace` in `browser.log` | Commander observes and controls existing work without polling an agent or creating model traffic. |
| Prowl Claude/Codex/Grok and logical sessions stayed `1/1/0` + `1`. Soundwave changed `1/0/0` + `6` → `1/1/0` + `6`; the sole Codex delta is PID `2299566`, independently spawned at 15:56:20 by agent `crisp-spider-66` under unrelated parent factory daemon `2144744`. | `baseline.json`, `after.json`, exact process parent/command and session-name receipts | Commander created no model process and no logical CAS session. The ambient Codex delta is externally attributed. |
| The first prowl restart attempt resolved a supervisor-created `/opt/homebrew/bin/tailscale` symlink; that contaminated path triggered the app bundle-identifier abort and left HTTPS at `502`. The supervisor removed the stray symlink. Stock `v2.60.0` with no PATH entry takes the cleaner CLI-unavailable warning path, but still restarts loopback-only and cannot republish Serve. | `prowl-restart-intermediate.json`; supervisor correction recorded verbatim in the task ledger | The abort shape was environmental contamination; the binding product defect is the same operational outcome on stock macOS: no Serve republish without an undocumented step. |
| A task-local wrapper that invoked the absolute signed app binary recovered prowl to PID `59538`, the same machine-id SHA/mtime and public URL, and HTTPS health `200`. The wrapper and stray system symlink are now absent. | `prowl-restart.json`; fresh absence/status/health assertions | Recovery is possible, but the manual step is exactly why public `v2.60.0` fails the release gate. No prowl factory daemon was restarted. |
| Canonical Linux CI run `31425559709` completed successfully at exact source SHA `69c3a1c6a24c1107865e3666e1cfa33ef9797615`. Fresh local Commander hub/H1–H5, protocol, and session suites reached green; the installed soundwave public ELF independently passed the strict no-EVEX/AVX-512 audit at SHA-256 `8ec9dea6…`. | Exact-commit CI receipt; local scoped-suite output; `public-linux-isa.log` | The assembled workspace, release, vendor, and ISA guard is green at the exact source boundary. The separate macOS-only full-suite findings below are not a Commander seam. |

## Reasoning chain

1. Immutable release identity plus installed-byte checks bind the live behavior to public `v2.60.0`.
2. Real Chrome on prowl's origin paired the soundwave hub, observed two panes through three viewers,
   enforced a single controller, and exercised the full adversarial and revocation matrix.
3. Both protocol phases opened exactly one daemon upstream. The SIGILL receipt matched the killed PID
   and start fingerprint, and the restarted legacy fixture reattached without adding sessions or agents.
4. Process/session deltas are either zero or exactly attributed to an unrelated factory worker. The
   browser trace shows push/event traffic, not idle agent polling.
5. The supervisor-created symlink explains the observed bundle-identifier abort, so that abort is not
   assigned to stock CAS. Removing it does not make stock `v2.60.0` pass: with no discoverable CLI,
   restart still becomes loopback-only and cannot restore the prior HTTPS Serve endpoint.
6. A private wrapper restored service, but an undocumented manual recovery violates the acceptance
   criteria. Therefore one failed restart row outweighs the otherwise-green matrix.

## Acceptance matrix

| Binding gate | State | Executed conclusion |
| --- | --- | --- |
| Public artifact | **PASS** | Exact immutable public `v2.60.0`; binding hosts used the intended published version. |
| Clean-home, TLS/HSTS, hostile Origin, plaintext refusal | **PASS** | Banked public-artifact receipts remain green. |
| `H7-ADV-01` | **PASS** | Hostile browser cases, DPoP/pairing/ticket replay, copied key, method mismatch, scope escalation, and revocation rejected; child suite owns accelerated expiry coverage. |
| `H7-FLEET-02` | **PASS** | Prowl + soundwave, real Chrome phone viewport, direct observation/control, arbitration, and attributed audit passed. |
| `H7-INVARIANT-03` | **PASS** | Three viewers used one upstream; two panes rendered; no logical session or Commander-attributable model-process delta; no idle agent polling. |
| `H7-CRASH-04` | **PASS** | Exact SIGILL produced typed actionable diagnosis for both viewers; other session and controller-hub reads stayed healthy. |
| Daemon restart / protocol compatibility | **PASS** | Protocol-v1 fixture restarted and both viewers reattached through one new upstream. |
| macOS hub restart / Serve republish | **FAIL** | Stock public `v2.60.0` cannot discover the app-bundle CLI; restart cannot republish the stable HTTPS endpoint without an undocumented manual step. |
| Assembled workspace / release / vendor / ISA | **PASS** | Canonical Linux CI run `31425559709` is green at exact SHA `69c3a1c6`; local Commander-scoped suites and the exact installed Linux ELF ISA audit are green. |
| Cleanup and authority | **PASS** | Zero active test devices, fixture/browser processes, fixture listeners, or temporary invitation/profile state; original metadata restored. |

## Additional environment finding — outside the Commander verdict

Prowl's macOS full-workspace runner exposed three test-infrastructure findings, now tracked by P2 bug
`cas-d20f`: its default soft descriptor limit of 256 caused an `EMFILE` cascade; after raising only the
proof subprocess limit, one socket-election race failed once and passed twice in isolation; and
`retrieval_parity_test::excluded_rows_do_not_shift_the_ranks_of_real_rows` reproduced a deterministic
macOS list-rank mismatch even in isolation. The failed receipts are preserved as
`assembled-emfile.*` and `assembled-flake.*` under the durable task artifacts. Canonical Linux CI run
`31425559709` passes the full suite at the exact same `69c3a1c6` commit, as do the referenced earlier
Linux source boundaries. The retrieval test has no Commander seam, so this is recorded and filed but
does not create a second H7 gate failure or weaken the macOS Serve-republish failure.

## What would falsify this

An immutable tagged release containing merged corrective `cas-a13a` (`42d0a69c`, present in epic tip
`e29b22fb`) would falsify the blocking conclusion if, on stock prowl with no wrapper or stray PATH
entry, `cas hub restart --tailscale-serve` preserves machine identity and URL, republishes Serve, and
returns HTTPS health `200`. Only that macOS restart row plus a Serve-republish spot check must be rerun;
all other green rows carry forward from this public-artifact pass.

## Next actions

| Action | Owner | Completion proof |
| --- | --- | --- |
| Publish the next immutable release containing `cas-a13a`; do not move `v2.60.0` or an earlier tag. | Release owner | Tag peel contains `42d0a69c`; official assets and digests are recorded. |
| Rerun only the stock macOS hub-restart row and Serve-republish spot check on prowl. | H7 verifier | No wrapper/PATH contamination; stable machine-id and URL; HTTPS health `200` after restart. |
| If that row is green, flip the Commander v1 verdict to releasable while carrying forward every other executed row here. | H7 verifier | Paired Markdown/HTML updated from the new immutable artifact; Slack draft still requires explicit user posting approval. |

## Cleanup and redaction

- No pairing capability, credential, proof, private key, WebSocket ticket, Authorization value, tailnet
  IP, terminal content, prompt content, or raw secret is retained in the report or repository.
- All three H7 devices are revoked; active device count is zero. The consumed invitation files and six
  isolated Chrome profiles were deleted after the remote `/tmp` filesystem refused recoverable Trash;
  those ephemeral deletions are not recoverable.
- Both fixture processes are stopped, port `39459` is clear, and the exact pre-run session JSON and
  daemon-exit receipt are restored byte-for-byte.
- Soundwave's operator systemd service remains PID `2256851`, active/running, `NRestarts=0`; its unit,
  linger state, and operator devices were not changed.
- Prowl's factory daemon was never restarted. The hub recovered as PID `59538`; the task-local wrapper
  and supervisor-created `/opt/homebrew/bin/tailscale` symlink are both absent.
- Physical Android remains unclaimed. Slack remains explicitly unposted.

## Provenance

- Markdown source: `docs/reports/2026-08-09-commander-v1-acceptance-gate.md`
- Human review surface: `docs/reports/2026-08-09-commander-v1-acceptance-gate.html`
- Explicitly unposted draft: `docs/release-notes/2026-08-09-commander-v1-slack-draft.md`
- Executed source boundary: `69c3a1c6a24c1107865e3666e1cfa33ef9797615`; public peel:
  `0cb8962d`; merged unpublished corrective: `42d0a69c` in epic tip `e29b22fb`.
- Canonical assembled receipt: Linux CI run `31425559709`, completed successfully at exact source SHA
  `69c3a1c6a24c1107865e3666e1cfa33ef9797615`; macOS test-infrastructure follow-up: `cas-d20f`.
- Durable redacted artifacts: `.cas/artifacts/cas-3d85/ready-viper-55/` — browser network/auth result,
  fixture event ledgers, exact crash receipts, count snapshots, filtered audit, and restart receipts,
  observed 2026-08-10 19:51–19:58 UTC.
- Commands: exact Git/release and installed-binary reads; SSH process/session/service/auth probes;
  deterministic protocol-v2/v1 fixtures; Chrome CDP at `390×844`; DPoP/pairing/ticket adversarial
  requests; exact PID `SIGILL`; guarded device revocation; metadata restoration; source workspace,
  canonical exact-commit Linux workspace/release/vendor proof, local Commander-scoped suites, and exact
  installed-public-Linux ISA audit.
