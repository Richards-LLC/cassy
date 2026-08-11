# Commander v1 assembled acceptance gate

**Verdict: NOT RELEASABLE at public `v2.61.1`. Confidence: high.** A fresh 2026-08-11 closure rerun
used byte-identical public Linux artifacts on soundwave and unicron plus real Chrome 151 at `390×844`.
Pairing, direct two-machine access, three-viewer fan-out, controller arbitration, resize, targeted
interrupt, and attributed messaging reached the deterministic fixture through exactly one upstream.
The required live-viewer hub restart then timed out after 10 seconds because the old hub PID or machine
lock remained live; it correctly launched no competing replacement. The run stopped fail-closed before
the adversarial, `SIGILL`, and legacy-protocol continuation rows. Prior green evidence remains historical,
but it cannot override this newer binding failure.

The normative contract is
[`docs/specs/2026-08-08-commander-security-architecture.md`](../specs/2026-08-08-commander-security-architecture.md),
especially `H7-ADV-01`, `H7-FLEET-02`, `H7-INVARIANT-03`, and `H7-CRASH-04`.

## Overview

| Field | Executed result |
| --- | --- |
| Question | Does a fresh full two-machine phone-class rerun against exact public `v2.61.1` complete every binding H7 row? |
| Verdict | **No. Live-viewer `cas hub restart --tailscale-serve` timed out after 10 seconds with the old hub PID or machine lock still live; no replacement started.** |
| Confidence | High; public tag/workflow/assets, installed bytes, real Chrome DOM/network behavior, fixture event ledger, exact restart stderr, process/Serve aftermath, and zero-residue cleanup were observed directly. |
| Executed source boundary | Clean public release peel `b5a37cb5675d4ae74b609d6479f824375f4c7efa`; annotated tag object `496075b676b5c7d7a747433f063635eaa49ea5e5`; contains `cas-0c54` merge `997939d7` and `cas-a13a` corrective `42d0a69c`. |
| Public release | `v2.61.1`; GitHub Release published 2026-08-10 23:01 UTC, neither draft nor prerelease. |
| Linux x86-64 asset | 21,998,541 bytes; archive SHA-256 `d40a089b1af31a2ed083d57d6b1d53d0640194cad09988fff24043de5d370c27`; extracted binary SHA-256 `fb0fc976fa738b50280043195adaadbcd765a900b47e4c649ce6c96a86c2f383`. |
| Binding machines | `soundwave` controller hub and distinct `unicron` target hub, both running the exact public Linux binary. |
| Browser | Real Google Chrome `151.0.7922.108`; isolated phone metrics `390×844`; controller origin `soundwave`. |
| Physical Android | Offline and explicitly **not claimed**. |
| Evidence window | 2026-08-11 13:04–13:21 UTC for the fresh closure rerun; earlier evidence is retained below as history. |
| Author | H7 assembled release gate (`cas-3d85`), closure rerun worker `proud-newt-50`. |

## 2026-08-11 closure rerun evidence

| Observation | Redacted source | What it proves |
| --- | --- | --- |
| Annotated tag object `496075b6…` peels `b5a37cb5…`; official Release run `31439277281` completed successfully. Fresh Linux and macOS archives matched GitHub sizes and SHA-256 metadata and contained only `cas` plus `LICENSE`. | GitHub tag/release/workflow reads; fresh archive manifests and hashes | The rerun is bound to immutable public `v2.61.1`, not post-release main or a local build. |
| Soundwave's prior dirty local build and unicron's public `v2.60.0` binary were preserved under SHA-addressed backups, then atomically replaced. Both installed binaries became byte-identical public `v2.61.1` SHA-256 `fb0fc976…`. | Install and backup receipts under `.cas/artifacts/cas-3d85/proud-newt-50/` | Both real machines executed the same published artifact. |
| Supervisor-authorized cleanup removed exactly one dead soundwave HTTPS:443 mapping to `127.0.0.1:42759` after proving its systemd hub service inactive and no listener, hub process, runtime, or receipt. Before/after Serve state is retained. | `soundwave-stale-serve-before.json`, service status, `soundwave-stale-serve-after.json` | Known legacy-receipt residue was removed narrowly before the run; it was not mistaken for live operator state. |
| Real Chrome 151 at `390×844` paired three soundwave devices and one direct unicron device, opened the temporary zero-worker two-pane session, enforced observer/controller UI, completed controller takeover, and sent resize, targeted interrupt, and attributed message operations. The fixture recorded one upstream connection, maximum concurrent upstreams `1`. | `browser-result-v2611.stderr`; `fixture-events-v2611.jsonl`; redacted device inventories | `H7-FLEET-02` and `H7-INVARIANT-03` progressed through direct fleet access, arbitration, fan-out, and control without creating another upstream. |
| With those live viewers attached, exact public `cas --json hub restart --tailscale-serve` exited `1`: `cas hub pid 1244423 or its machine lock remained live after 10.0s; no replacement was started`. The browsers then closed, the old hub exited, and soundwave Serve became `{}`. | `browser-result-v2611.exit`, `browser-result-v2611.stderr`, `soundwave-hub-after-restart-timeout.*`, `soundwave-serve-after-restart-timeout.json` | Singleton safety is truthful, but the required live-viewer restart recovery fails. This is the release-blocking row tracked by GH #217 / `cas-017a`. |
| All 3 soundwave and 1 unicron H7 devices were exactly revoked; both hubs and the fixture stopped; Serve is `{}` on both hosts; the temporary session/exit receipt, invitations, profiles, listeners, and harness files are absent from live state. Logical sessions returned `3→3` on soundwave and `0→0` on unicron. | Final auth, hub, Serve, listener, session, and CAS server-registry receipts | The failed run left zero live authority or test runtime residue. |

The earlier `cas-f382` proof remains valid for its narrow stock macOS upgrade-restart condition, which
ran without attached live viewers. It cannot establish the assembled live-viewer restart row exercised
here. The newer failure therefore narrows—not erases—the earlier evidence and supersedes its overall
RELEASABLE conclusion.

## Prior accepted evidence (historical)

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

1. Immutable tag, workflow, asset, and installed-byte checks bind both machines to public `v2.61.1`.
2. After two harness-only same-origin navigation retries that banked no product rows, the checked final
   harness paired both machines and proved real fan-out, arbitration, control, attribution, and one upstream.
3. The assembled live-viewer restart failed exactly at the singleton handoff: the old PID or machine
   lock remained live for 10 seconds, so the CLI correctly refused to start a competing replacement.
4. That truthful refusal still fails the required recovery behavior. Fail-closed sequencing therefore
   stopped the run before adversarial, crash, and compatibility continuation rows; historical passes
   cannot be promoted into fresh results.
5. Cleanup revoked exactly four devices and restored both hosts to zero Commander runtime residue and
   their original logical-session counts. The surviving soundwave Codex delta belongs to unrelated
   factory worktrees, not Commander.
6. `cas-f382` proved a narrower stock macOS restart without attached live viewers. The new result does
   not invalidate that observation, but it does disprove the broader `v2.61.1` release conclusion.

## Acceptance matrix

| Binding gate | State | Executed conclusion |
| --- | --- | --- |
| Public artifact | **PASS** | Immutable public `v2.61.1` Linux bytes were independently verified and installed byte-identically on soundwave and unicron; the macOS archive was re-verified too. |
| Clean-home, TLS/HSTS, hostile Origin, plaintext refusal | **NOT REEXECUTED** | Historical public-artifact receipts remain evidence, but the binding rerun stopped before this complete hostile matrix. |
| `H7-ADV-01` | **NOT REEXECUTED** | Fail-closed stop at live-viewer restart prevented the adversarial continuation; no historical row is rebanked. |
| `H7-FLEET-02` | **PARTIAL** | Soundwave + unicron, phone Chrome, direct observation/control, arbitration, and attributed operations passed before the restart abort. |
| `H7-INVARIANT-03` | **PARTIAL** | Three soundwave viewers and one unicron viewer used one upstream; the full post-restart/no-polling continuation was not reached. |
| `H7-CRASH-04` | **NOT REEXECUTED** | The run stopped before SIGILL injection. |
| Daemon restart / protocol compatibility | **NOT REEXECUTED** | The run stopped before the protocol-v1 continuation. |
| Live-viewer hub restart / Serve republish | **FAIL** | Public `v2.61.1` timed out after 10 seconds because the old PID or machine lock remained live; no replacement started. GH #217 / `cas-017a`. |
| Assembled workspace / release / vendor / ISA | **CARRIED** | Prior canonical evidence remains valid but was not the binding row under test. |
| Cleanup and authority | **PASS** | Four devices revoked; hubs, Serve mappings, fixture, listeners, temporary metadata, invitations, and profiles absent from live state; session counts restored. |

## Additional environment finding — outside the Commander verdict

Prowl's macOS full-workspace runner exposed three test-infrastructure findings, now tracked by P2 bug
`cas-d20f`: its default soft descriptor limit of 256 caused an `EMFILE` cascade; after raising only the
proof subprocess limit, one socket-election race failed once and passed twice in isolation; and
`retrieval_parity_test::excluded_rows_do_not_shift_the_ranks_of_real_rows` reproduced a deterministic
macOS list-rank mismatch even in isolation. The failed receipts are preserved as
`assembled-emfile.*` and `assembled-flake.*` under the durable task artifacts. Canonical Linux CI run
`31425559709` passes the full suite at the exact same `69c3a1c6` commit, as do the referenced earlier
Linux source boundaries. The retrieval test has no Commander seam, so this is recorded and filed but
does not explain or weaken the new assembled live-viewer restart failure.

## What would falsify this

A corrective public release would falsify this blocking conclusion only if a fresh full H7 continuation,
with live viewers attached, exits restart successfully, establishes exactly one replacement hub and
Serve mapping, reconnects the viewers without manual recovery, and then completes every remaining
adversarial, crash, compatibility, count, and cleanup row.

## Next actions

| Action | Owner | Completion proof |
| --- | --- | --- |
| Fix the live-viewer restart handoff defect. | GH #217 / `cas-017a` owner | Public corrective source and focused regression proof. |
| Cut the next immutable public release. | Runtime/release owners | Tag peel, workflow, assets, sizes, and SHA-256 receipts. |
| Run a fresh full H7 continuation bound only to that public artifact. | H7 verifier | Every binding row green in one fail-closed sequence, with zero residue. Slack remains unposted until then. |

## Cleanup and redaction

- No pairing capability, credential, proof, private key, WebSocket ticket, Authorization value, tailnet
  IP, terminal content, prompt content, or raw secret is retained in the report or repository.
- Exactly four H7 devices (three soundwave, one unicron) are revoked; active count is zero on both hosts.
  Profiles, invitations, harness scratch, and the temporary zero-worker fixture metadata were moved to
  recoverable Trash; the baseline absence of that metadata is restored.
- Both public-byte hubs and the fixture are stopped. Tailscale Serve is `{}` on both hosts; no hub
  process, port `39459`/`4173` listener, temporary session, or daemon-exit receipt remains live.
- Logical sessions returned `3→3` on soundwave and `0→0` on unicron. Claude/Codex/Grok changed
  `3/17/0→3/18/0` on soundwave and stayed `0/0/0` on unicron; surviving post-baseline Codex processes
  are rooted in unrelated factory worktrees, not Commander.
- Pre-run binaries were preserved in SHA-addressed backups before both hosts received public `v2.61.1`.
- Physical Android remains unclaimed. Slack remains explicitly unposted.

## Provenance

- Markdown source: `docs/reports/2026-08-09-commander-v1-acceptance-gate.md`
- Human review surface: `docs/reports/2026-08-09-commander-v1-acceptance-gate.html`
- Explicitly unposted draft: `docs/release-notes/2026-08-09-commander-v1-slack-draft.md`
- Closure-rerun artifacts: `.cas/artifacts/cas-3d85/proud-newt-50/` — immutable release and install
  receipts, stale-Serve before/after proof, checked browser harness, fixture ledger, exact restart failure,
  final auth/process/session/Serve/listener receipts, observed 2026-08-11 13:04–13:21 UTC.
- Focused recheck source boundary: public peel `b5a37cb5675d4ae74b609d6479f824375f4c7efa`,
  annotated tag `496075b676b5c7d7a747433f063635eaa49ea5e5`, containing `997939d7` and `42d0a69c`.
- Public macOS ARM64 asset: 19,381,860 bytes, archive SHA-256 `7e268a030834bd7372ad6bcef2d69ed5b6f3bb1a7e43c4102ea3d630c92b53ba`, extracted SHA-256 `97ed6e4c0a3e879a0fe600659833c3de61dfd3ae500037784c8ee21116e67893`.
- Public Linux x86-64 asset: 21,998,541 bytes, archive SHA-256 `d40a089b1af31a2ed083d57d6b1d53d0640194cad09988fff24043de5d370c27`, extracted SHA-256 `fb0fc976fa738b50280043195adaadbcd765a900b47e4c649ce6c96a86c2f383`.
- Canonical assembled receipt: Linux CI run `31425559709`, completed successfully at exact source SHA
  `69c3a1c6a24c1107865e3666e1cfa33ef9797615`; macOS test-infrastructure follow-up: `cas-d20f`.
- Durable redacted artifacts: `.cas/artifacts/cas-3d85/ready-viper-55/` — browser network/auth result,
  fixture event ledgers, exact crash receipts, count snapshots, filtered audit, and restart receipts,
  observed 2026-08-10 19:51–19:58 UTC.
- Focused recheck artifacts: `.cas/artifacts/cas-f382/fast-panda-84/` — public archives, install/backup
  receipts, stock-PATH proofs, restart and teardown receipts, failed `v2.61.0` state, and green
  `v2.61.1` assertions, observed 2026-08-10 22:03–23:02 UTC.
- Current commands: exact Git/release and installed-binary reads; batch SSH process/session/service/auth
  probes; public hub start/restart/stop; Tailscale Serve before/after reads; deterministic protocol-v2
  fixture; checked Chrome CDP harness at `390×844`; guarded device revocation and metadata restoration.
  Earlier commands below remain historical evidence only.
