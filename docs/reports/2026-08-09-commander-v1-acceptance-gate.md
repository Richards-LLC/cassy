# Commander v1 assembled acceptance gate

**Verdict: RELEASABLE at public `v2.61.1`. Confidence: high.** The binding prowl + soundwave
phone-browser, security, fan-out, controller-arbitration, crash, compatibility, and zero-agent rows
carry forward as passed. Public `v2.61.1` contains both stock macOS app-bundle CLI discovery and the
legacy-receipt compatibility correction. On stock prowl, the exact upgrade restart truthfully removed
the prior CAS-owned mapping, selected the absolute signed app CLI, republished the stable URL, preserved
machine identity, and returned HTTPS `200` without a wrapper, symlink, or manual reset. Every binding
Commander v1 row is now green.

The normative contract is
[`docs/specs/2026-08-08-commander-security-architecture.md`](../specs/2026-08-08-commander-security-architecture.md),
especially `H7-ADV-01`, `H7-FLEET-02`, `H7-INVARIANT-03`, and `H7-CRASH-04`.

## Overview

| Field | Executed result |
| --- | --- |
| Question | Does the exact public `v2.61.1` prowl restart recheck clear the final Commander v1 gate while every other prowl + soundwave row carries forward? |
| Verdict | **Yes. Stock macOS upgrade restart removed the owned mapping, discovered the signed app CLI, republished Serve, preserved identity/URL, and returned HTTPS `200` with no manual recovery.** |
| Confidence | High; the public asset, atomic install, stock PATH, before/after and teardown receipts, selected CLI path, identity, HTTPS result, and unchanged factory daemons were observed directly. |
| Executed source boundary | Clean public release peel `b5a37cb5675d4ae74b609d6479f824375f4c7efa`; annotated tag object `496075b676b5c7d7a747433f063635eaa49ea5e5`; contains `cas-0c54` merge `997939d7` and `cas-a13a` corrective `42d0a69c`. |
| Public release | `v2.61.1`; GitHub Release published 2026-08-10 23:01 UTC, neither draft nor prerelease. |
| macOS ARM64 asset | 19,381,860 bytes; archive SHA-256 `7e268a030834bd7372ad6bcef2d69ed5b6f3bb1a7e43c4102ea3d630c92b53ba`; extracted binary SHA-256 `97ed6e4c0a3e879a0fe600659833c3de61dfd3ae500037784c8ee21116e67893`. |
| Binding machines | `prowl` controller hub and distinct `soundwave` target hub. Unicron and shield are explicitly out of scope. |
| Browser | Real Google Chrome `151.0.7922.108`; isolated phone metrics `390×844`; controller origin `prowl`. |
| Physical Android | Offline and explicitly **not claimed**. |
| Evidence window | 2026-08-10 23:01–23:02 UTC for the `v2.61.1` recheck, with binding non-restart rows carried from the 19:51–19:58 UTC H7 pass. |
| Author | H7 assembled release gate (`cas-3d85`) and focused re-verification (`cas-f382`). |

## Evidence

| Observation | Redacted source | What it proves |
| --- | --- | --- |
| Public `v2.61.1` is an immutable annotated tag whose peel contains both corrections. A fresh macOS download matched GitHub's 19,381,860-byte size and SHA-256 `7e268a03…`; the extracted arm64 binary reports `2.61.1` / `b5a37cb`. It was atomically installed on prowl with exact `v2.61.0` SHA `6e9c3637…` backed up. | GitHub release/tag reads; `cas-f382/fast-panda-84/v2611-public-asset-receipt.txt`; `v2611-install-receipt.txt` | The focused row used independently verified public bytes and preserved a rollback receipt. Soundwave remained intentionally untouched under the carry-forward contract. |
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
| Stock proof used an explicit system PATH with no `tailscale`, no `/opt/homebrew/bin/tailscale`, and no scratch wrapper; Tailscale.app was running. Public `v2.61.0` stopped PID `59538` truthfully and started PID `12234`, recording `/Applications/Tailscale.app/Contents/MacOS/Tailscale` as the selected CLI. | `cas-f382/fast-panda-84/stock-path.txt`; `prowl-v2610-restart.log`; process record | `cas-a13a` is present and app-bundle discovery works under the exact stock condition that blocked `v2.60.0`. |
| The same restart reported `invalid CAS Tailscale receipt`, preserved the stale `v2.60.0` root mapping, and started loopback-only because HTTPS port 443 was owned by that old mapping. The stable URL returned `502`; machine-id SHA/mtime and both factory `cas serve` PIDs stayed unchanged. | Old schema-version-1 receipt; `prowl-v2610-restart.log`; `failed-restart-state.log`; `cas-cli/src/hub/tailscale.rs` | The newly required receipt `executable` field has no deserialization default, so the supported public upgrade path cannot tear down and republish its own prior mapping. Stop/start truth remains accurate, but the binding endpoint recovery fails. |
| After an explicit manual reset of only the exact CAS-owned stale mapping, another stock-PATH `v2.61.0` restart started PID `12867`, wrote the new receipt with the absolute app CLI, preserved machine identity and URL, and returned HTTPS `200`. `cas hub service status` also ran without installing supervision. | `manual-recovery-reset.log`; `prowl-v2610-restart-after-manual-reset.log`; `recovered-state.log` | This diagnostic recovery did not green `v2.61.0`; it safely restored prowl for the later public `v2.61.1` row. No factory daemon was restarted. |
| With public `v2.61.1` atomically installed, the fresh stock-PATH restart stopped PID `12867` with `tailscale_serve_removed=true` and no warning, then started PID `62771`. The teardown receipt records the prior mapping and empty `status_after`; the new receipt selects `/Applications/Tailscale.app/Contents/MacOS/Tailscale` and owns the new mapping. Machine-id SHA/mtime and public URL are unchanged; HTTPS is `200`. | `v2611-stock-prestate.log`; `v2611-restart.log`; `v2611-poststate.log`; `v2611-row-assertions.txt` | The final operational row passes without wrapper, symlink, or manual reset. Stop/start and Serve ownership receipts are truthful, and the public endpoint recovers on the supported upgrade path. |
| Canonical Linux CI run `31425559709` completed successfully at exact source SHA `69c3a1c6a24c1107865e3666e1cfa33ef9797615`. Fresh local Commander hub/H1–H5, protocol, and session suites reached green; the installed soundwave public ELF independently passed the strict no-EVEX/AVX-512 audit at SHA-256 `8ec9dea6…`. | Exact-commit CI receipt; local scoped-suite output; `public-linux-isa.log` | The assembled workspace, release, vendor, and ISA guard is green at the exact source boundary. The separate macOS-only full-suite findings below are not a Commander seam. |

## Reasoning chain

1. Immutable release identity plus installed-byte checks bind the focused restart behavior to public
   `v2.61.1`; the non-restart matrix carries from the accepted public `v2.60.0` pass by contract.
2. Real Chrome on prowl's origin paired the soundwave hub, observed two panes through three viewers,
   enforced a single controller, and exercised the full adversarial and revocation matrix.
3. Both protocol phases opened exactly one daemon upstream. The SIGILL receipt matched the killed PID
   and start fingerprint, and the restarted legacy fixture reattached without adding sessions or agents.
4. Process/session deltas are either zero or exactly attributed to an unrelated factory worker. The
   browser trace shows push/event traffic, not idle agent polling.
5. With no PATH entry or wrapper, `v2.61.0` selects the signed app-bundle CLI exactly as intended, so
   the original discovery defect is fixed.
6. `v2.61.0` exposed a receipt-compatibility defect, and `cas-0c54` corrected it. Public `v2.61.1`
   truthfully removed the owned mapping and republished Serve with stable identity/URL and HTTPS `200`
   under the exact stock conditions. The last failed row is now green, so the carried matrix is fully green.

## Acceptance matrix

| Binding gate | State | Executed conclusion |
| --- | --- | --- |
| Public artifact | **PASS** | Exact immutable public `v2.61.1` macOS bytes were independently verified and installed on prowl; the peel contains both required corrections. |
| Clean-home, TLS/HSTS, hostile Origin, plaintext refusal | **PASS** | Banked public-artifact receipts remain green. |
| `H7-ADV-01` | **PASS** | Hostile browser cases, DPoP/pairing/ticket replay, copied key, method mismatch, scope escalation, and revocation rejected; child suite owns accelerated expiry coverage. |
| `H7-FLEET-02` | **PASS** | Prowl + soundwave, real Chrome phone viewport, direct observation/control, arbitration, and attributed audit passed. |
| `H7-INVARIANT-03` | **PASS** | Three viewers used one upstream; two panes rendered; no logical session or Commander-attributable model-process delta; no idle agent polling. |
| `H7-CRASH-04` | **PASS** | Exact SIGILL produced typed actionable diagnosis for both viewers; other session and controller-hub reads stayed healthy. |
| Daemon restart / protocol compatibility | **PASS** | Protocol-v1 fixture restarted and both viewers reattached through one new upstream. |
| macOS hub restart / Serve republish | **PASS** | Stock public `v2.61.1` removed the prior owned mapping, selected the app CLI, republished the same URL, preserved machine identity, and returned HTTPS `200` without manual recovery. |
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
does not create a Commander gate failure or weaken the public `v2.61.1` green restart evidence.

## What would falsify this

The executed public `v2.61.1` row falsified the prior blocking conclusion. This releasable verdict
would be overturned by a reproduction from supported public state in which stock-path restart fails
to remove the unchanged CAS-owned mapping, changes machine identity or public URL, requires a manual
Serve operation, or does not restore HTTPS `200`.

## Next actions

| Action | Owner | Completion proof |
| --- | --- | --- |
| Complete P0 corrective `cas-0c54` and publish immutable `v2.61.1`. | Runtime/release owners | **DONE:** tag peel, assets, digests, and focused legacy fixtures recorded. |
| Rerun only the stock macOS upgrade-restart row and Serve-republish spot check on prowl. | H7 verifier | **DONE:** no wrapper/PATH contamination/manual reset; stable machine-id and URL; truthful teardown/republish; HTTPS `200`. |
| Flip Commander v1 to releasable while carrying forward every other executed row. | H7 verifier | **DONE:** paired Markdown/HTML updated from public `v2.61.1`; Slack remains unposted pending explicit user approval. |

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
- Prowl's two pre-existing factory daemons remained PIDs `9324` and `21707` throughout the recheck. The
  hub is healthy on public `v2.61.1` as PID `62771`; the focused row required no manual reset, machine
  identity and public URL are unchanged, and no wrapper or `/opt/homebrew/bin/tailscale` entry exists.
- Physical Android remains unclaimed. Slack remains explicitly unposted.

## Provenance

- Markdown source: `docs/reports/2026-08-09-commander-v1-acceptance-gate.md`
- Human review surface: `docs/reports/2026-08-09-commander-v1-acceptance-gate.html`
- Explicitly unposted draft: `docs/release-notes/2026-08-09-commander-v1-slack-draft.md`
- Focused recheck source boundary: public peel `b5a37cb5675d4ae74b609d6479f824375f4c7efa`,
  annotated tag `496075b676b5c7d7a747433f063635eaa49ea5e5`, containing `997939d7` and `42d0a69c`.
- Public macOS ARM64 asset: 19,381,860 bytes, archive SHA-256 `7e268a030834bd7372ad6bcef2d69ed5b6f3bb1a7e43c4102ea3d630c92b53ba`, extracted SHA-256 `97ed6e4c0a3e879a0fe600659833c3de61dfd3ae500037784c8ee21116e67893`.
- Canonical assembled receipt: Linux CI run `31425559709`, completed successfully at exact source SHA
  `69c3a1c6a24c1107865e3666e1cfa33ef9797615`; macOS test-infrastructure follow-up: `cas-d20f`.
- Durable redacted artifacts: `.cas/artifacts/cas-3d85/ready-viper-55/` — browser network/auth result,
  fixture event ledgers, exact crash receipts, count snapshots, filtered audit, and restart receipts,
  observed 2026-08-10 19:51–19:58 UTC.
- Focused recheck artifacts: `.cas/artifacts/cas-f382/fast-panda-84/` — public archives, install/backup
  receipts, stock-PATH proofs, restart and teardown receipts, failed `v2.61.0` state, and green
  `v2.61.1` assertions, observed 2026-08-10 22:03–23:02 UTC.
- Commands: exact Git/release and installed-binary reads; SSH process/session/service/auth probes;
  deterministic protocol-v2/v1 fixtures; Chrome CDP at `390×844`; DPoP/pairing/ticket adversarial
  requests; exact PID `SIGILL`; guarded device revocation; metadata restoration; source workspace,
  canonical exact-commit Linux workspace/release/vendor proof, local Commander-scoped suites, and exact
  installed-public-Linux ISA audit.
