# Commander v1 assembled acceptance gate

**Verdict: NOT RELEASABLE. Confidence: high.** Public `v2.55.1` is authentic, identical on
both real machines, and its strict Linux ISA gate passes. The assembled gate nevertheless found two
binding release blockers: a clean installed machine cannot start Commander until an undocumented
`~/.cas` parent directory is created, and an observed daemon `SIGILL` is reported to the real browser
as `unknown`. P0 follow-ups are `cas-efcb` and `cas-ad6f`. This report does not waive either failure.

The normative contract is
[`docs/specs/2026-08-08-commander-security-architecture.md`](../specs/2026-08-08-commander-security-architecture.md),
especially `H7-ADV-01`, `H7-FLEET-02`, `H7-INVARIANT-03`, and `H7-CRASH-04`.

## Overview

| Field | Executed result |
| --- | --- |
| Source | clean `HEAD == origin/main == 8805d2f43cff4fc96b72a293155a252f145457b6` |
| Public release | `v2.55.1`; annotated tag object `f8ea676f…` peels exactly to `8805d2f4…` |
| Linux asset | 21,760,348 bytes; archive SHA-256 `85c64c0…`; binary SHA-256 `89b9111b…` |
| Machines | `soundwave` and distinct `unicron`, Linux x86_64, same installed public binary |
| Browser | real Google Chrome `151.0.7922.75`, mobile metrics `390×844` |
| Physical Android | offline and explicitly **not claimed** |
| Evidence window | 2026-08-09 09:03–09:35 UTC |

## Evidence

| Observation | Redacted source | What it proves |
| --- | --- | --- |
| Official run `31303524670` completed successfully at exact tag SHA `8805d2f4…`; fresh download matched public API size/digest and contained only `cas` + `LICENSE`. Extracted binary reports `cas 2.55.1 (8805d2f 2026-08-09)` and passes the strict full-executable zero-EVEX audit. | Git/GitHub API, independent download, release guard | Published-artifact identity and ISA are green. |
| Both installed binaries have SHA-256 `89b9111b…` and the exact public version. Before→after session records stayed soundwave `5→5` and unicron `0→0`; unicron processes stayed `0/0/0`. Soundwave Claude/Codex/Grok changed `0/3/0→0/5/0` only because the supervisor spawned the P0 fix workers `hv-hub-clean-home` and `hv-daemon-death-sigill` during H7; the authoritative agent list attributes exactly those two new Codex processes. | Local/batch-SSH probes plus CAS agent registry | Commander created no session or agent. The global process count was externally perturbed, so strict unchanged-process proof is honestly deferred. |
| On clean unicron, `cas hub start --tailscale-serve` failed with `No such file or directory` because `~/.cas` did not exist. The H5 runbook documents no mkdir/init prerequisite. Mode-0700 `~/.cas` was created only as an explicitly recorded continuation workaround. | Exact public binary; source inspection of non-recursive hub directory creation | Fresh-machine setup is not runbook-complete and violates the no-undocumented-step gate. |
| After the workaround and documented Tailscale operator setup, both hubs served verified TLS 1.3 health; public plaintext listeners on `0.0.0.0` were refused. Unicron survived an abrupt hub kill truthfully: status failed closed, then restart preserved its machine identity and public URL. | Public binaries, Tailscale Serve, verified curl, status/restart receipts | Remote transport refusal and hub restart behavior are green after the recorded workaround. |
| Chrome paired directly with both hubs under the soundwave controller origin. The UI showed both catalog entries connected, opened the deterministic dormant session, rendered one Ghostty canvas, acquired control, resized, sent targeted interrupt and attributed message. Fragment was removed; cookies/Web Storage were empty; only IndexedDB existed. | Chrome DevTools protocol with secrets omitted by construction | Real phone-class pairing, controller-origin catalog, VT surface, scoped control, and storage topology execute through the public artifact. |
| Sanitized browser trace contained pairing, DPoP API fetches, one long-lived `/v1/events` stream per hub, ticket issuance, lease traffic, and WSS attach; no interval session/status polling appeared. Fixture telemetry recorded one upstream maximum and the exact `Attach`, `ResizePane`, `InterruptPane`, and `SendMessage` variants. | Sanitized Chrome network events and no-content fixture counters | Push-driven browser behavior and the control seam execute without agent creation. Simultaneous multi-viewer/slow-viewer proof remains for the post-fix rerun. |
| Wrong Origin API, hostile preflight, cookie-less CSRF mutation, and wrong-Origin WebSocket upgrade each returned 401. Hub security state was 0700 with credential/auth/audit/process/Serve files 0600; H7 browser devices were revoked and active count returned to zero on both hubs. | Redacted HTTP status matrix, mode scan, auth list/revoke | Basic hostile-origin containment and cleanup are green. Expiry/replay/scope-escalation remain binding post-fix rerun cases. |
| The registered no-agent daemon fixture was killed by exact PID with `SIGILL`. Chrome received `daemon_disconnected` with `cause.kind = unknown`, not SIGILL. The fixture record was restored and registered server stopped. | Browser attention event + registry PID `2420488` + source seam | `H7-CRASH-04` fails in the assembled product; unit classification did not reach the live connector event. |
| Web typecheck, all 11 browser invariants, production build, checked-in dist parity, both pinned WASM digests, upstream T3/Ghostty pins, and all MIT notices passed. | Locked web scripts, SHA-256, upstream commit APIs, notice files | Browser/vendoring gates are green. |
| The fresh assembled chain passed all workspace tests, the optimized `cas` build, the ring-only/no-AWS-LC dependency guard, strict zero-EVEX ISA audit, migration-guard behavior tests, and the release migration guard. The locally built executable SHA-256 was `668197e4…`. | Durable `/tmp/cas-3d85-assembled.{log,exit}.3` receipt; exit `0` | Full assembled source/release guards are green; they do not waive either real published-artifact blocker. |

## Acceptance matrix

| Binding gate | State | Executed conclusion |
| --- | --- | --- |
| `H7-ADV-01` | **PARTIAL / release blocked** | Hostile Origin, preflight, CSRF, CSWSH, storage channel and revocation cleanup passed; the full expiry/replay/copied-key/scope matrix must rerun against the corrective public artifact. |
| `H7-FLEET-02` | **FAIL** | Two machines and real Chrome worked only after an undocumented clean-host mkdir workaround. |
| `H7-INVARIANT-03` | **PARTIAL** | One observed upstream, push-only trace, VT/control variants, unchanged session counts, and registry-attributed external process delta are established; simultaneous fan-out/backpressure and an unperturbed process window must rerun post-fix. |
| `H7-CRASH-04` | **FAIL** | Exact fixture `SIGILL` surfaced as `unknown`. |
| Compatibility | **SOURCE GATE PASS / live rerun pending** | The fresh full workspace suite, including additive protocol coverage, passed; the corrective public-artifact rerun must still exercise the old/new live matrix. |

## Reasoning chain

1. The immutable public artifact and two-machine install are genuine, so these are product findings,
   not local-build substitutions.
2. The documented first start fails on a clean installed host. The manual 0700 parent creation allowed
   continued evidence collection but cannot become an implicit requirement.
3. The real browser exercised the published hub-to-daemon path, and the exact SIGILL termination still
   became an `unknown` diagnostic. That contradicts a binding invariant directly.
4. Either failure independently prevents release. Remaining partial gates cannot convert this verdict
   to green; they must rerun on a new public artifact containing both fixes.

## What would falsify this verdict

A new immutable release must contain both reviewed fixes, start on a clean installed machine without
manual initialization, identify `SIGILL` honestly in Chrome, and pass the complete hostile-auth,
two-device arbitration, simultaneous fan-out/slow-viewer, compatibility, restart, no-polling, full-suite,
and unchanged before/after count matrix. Local source fixes alone are insufficient.

## Cleanup and redaction

- No pairing token, credential, proof, key, ticket, Authorization value, tailnet IP, prompt content, or
  raw audit payload is retained in this report.
- H7-created browser credentials were revoked on both hubs; active H7 device count is zero.
- The fixture PID was terminated, its pre-existing session metadata restored exactly, and its registered
  server entry marked dead. Physical Android remains unclaimed.
- Slack remains explicitly unposted. The adjacent draft must not be posted from this failed gate.

## Next actions

| Action | Owner | Completion proof |
| --- | --- | --- |
| Fix clean-host hub parent bootstrap (`cas-efcb`). | Release engineering | New public binary starts from installed-only unicron state. |
| Bind daemon exit evidence into live diagnostics (`cas-ad6f`). | Hub/runtime | Real Chrome reports the exact SIGILL evidence without invention. |
| Publish a new immutable corrective release. | Release owner | Tag peel, workflow, asset digests, strict ISA, identical two-host install. |
| Rerun all H7 rows and only then reconsider Slack. | Fresh H7 worker | Green paired MD/HTML report; Slack still unposted until review. |

## Provenance

- Markdown source: `docs/reports/2026-08-09-commander-v1-acceptance-gate.md`
- Human review surface: `docs/reports/2026-08-09-commander-v1-acceptance-gate.html`
- Unposted draft: `docs/release-notes/2026-08-09-commander-v1-slack-draft.md`
- Commands: Git/GitHub tag and release queries; independent `curl`/SHA/tar/version/ISA checks;
  local + batch SSH count/version/status probes; `cas hub` start/status/restart/pair/auth; verified TLS
  curl; hostile HTTP/WS handshakes; Chrome 151 CDP at 390×844; registered deterministic daemon fixture;
  locked web typecheck/test/build/dist and vendor integrity checks; fresh workspace tests, optimized build,
  dependency/ISA guards, and migration guards with durable exit-0 receipt.
