# Commander v1 assembled acceptance gate

**Verdict:** **NOT YET RELEASABLE.** The exact Commander source boundary is clean and the
two-machine/phone-class topology is available, but there is no published artifact containing
Commander to install and test. The latest public release and the installed Machine A binary are
`v2.54.1` at `22fe07c`; the Commander epic tip is `bfa1e0af`, no tag contains it, and Machine B has
no `cas` binary installed. Per the binding ADR, a source build cannot substitute for this
published-artifact invariant. Confidence: **high**.

This is a redacted pre-publication gate and continuation handoff, not a release receipt. It cites
the binding architecture decision at
[`docs/specs/2026-08-08-commander-security-architecture.md`](../specs/2026-08-08-commander-security-architecture.md).

## Overview

| Field | Result |
| --- | --- |
| Question | Can Commander v1 be declared releasable from the assembled epic? |
| Verdict | **No — published-artifact acceptance has not started** |
| Source boundary | `bfa1e0afbd97f1750b41f8687196e999ed5fa46a` |
| Public artifact boundary | `v2.54.1` → `22fe07c46fb9aec159142956d8d45689157552b6` |
| Machine A | `soundwave`, Linux x86_64, tailnet online |
| Machine B | `unicron`, distinct Linux x86_64 host, tailnet reachable and batch SSH available |
| Phone-class browser | Google Chrome 151 available at the required `390×844` viewport |
| Physical phone | Android tailnet peer offline; **not claimed and not required by “phone-class browser” wording** |
| Evidence window | 2026-08-09 06:22–06:42 UTC |
| Author | H7 acceptance worker, machine-local execution |

## Evidence

| Observation | Redacted source | What it proves |
| --- | --- | --- |
| The fetched Commander epic ref equals `bfa1e0afbd97f1750b41f8687196e999ed5fa46a`, and the isolated H7 worktree started clean at that exact commit. | `git fetch`, `git rev-parse`, `git status --porcelain` | The source review is not against a stale or dirty branch. |
| No tag points at or contains `bfa1e0af`; the GitHub release list ends at `v2.54.1`. | `git tag --points-at`, `git tag --contains`, `gh release list` | There is no immutable public Commander artifact to accept. |
| `v2.54.1^{}` resolves to `22fe07c`; Machine A reports `cas 2.54.1 (22fe07c 2026-08-09)`. | Git tag peel and installed `cas --version` | The installed public binary predates Commander and cannot satisfy the gate. |
| Machine B is reachable as a distinct Linux x86_64 host over the tailnet and non-interactive SSH, but `cas` is unavailable there. | Redacted `tailscale ping` and batch SSH identity probe | The physical topology exists, but the same published CAS version is not installed on both hosts. |
| Google Chrome 151 is installed locally; the Android peer is offline. | Browser version probe and redacted Tailscale topology summary | A real phone-sized browser run can use Chrome at `390×844`; no physical-phone claim is made. |
| Before source-only guards, Machine A had Claude/Codex/Grok process counts `0/3/0` and `5` CAS session records; Machine B had `0/0/0` and `0`. The same after measurement was unchanged on both hosts. | Exact-name process count plus `~/.cas/sessions/*.json` record count on each host, before and after | Source-only verification caused no measured process or logical-session-record multiplication. This narrow count does not imply authoritative model-call proof. |
| Commander web typecheck, 11 invariant tests, production build, and checked-in `dist` parity passed at `bfa1e0af`. | `npm ci && npm run typecheck && npm test && npm run build && git diff --exit-code -- dist` | The assembled browser source and committed offline assets are internally consistent. |
| The vendored WASM hashes match the documented pins; pinned T3 Code and Ghostty commits exist and are verified upstream; T3 Code, Ghostty, and symbols-font MIT notices are present. | `sha256sum`, GitHub commit/content API, `hub-web/README.md`, adjacent license files | Vendored terminal artifacts are pinned and attribution is retained. |
| Full workspace tests, release-profile build, and portable x86_64 Ghostty ISA audit completed with exit `0`. The main CAS library group reported `4270 passed / 0 failed / 6 ignored`; every subsequent workspace group reported `0 failed`. The release build finished in 5m34s, and the audited Ghostty archive had no EVEX/AVX-512 with SHA-256 `483ea81a1ae4ed35c3ea2cd110540bb14f08ac5536ec9bca38eb2ccb67acfcd8`. | Fresh yielded source proof at `bfa1e0af`, 2026-08-09 06:32–06:41 UTC | Assembled source/workspace/release guards pass. This still cannot replace downloaded published-asset execution. |

## Acceptance matrix

| Binding gate | State | Evidence now | Evidence still required from the published artifact |
| --- | --- | --- | --- |
| `H7-ADV-01` adversarial browser suite | **NOT RUN against artifact** | H1/H2/H4 source contracts cover origin, narrow preflight, DPoP binding/replay, revocation, scopes, one-use tickets, state permissions, and CSP. | Run hostile Origin/CORS/CSRF/cross-site WebSocket, expired/replayed/revoked/copied-state/scope-escalation probes against both installed release hubs; retain only redacted request metadata and status/audit outcomes. |
| `H7-FLEET-02` two-machine + phone-class browser | **READY, BLOCKED ON ARTIFACT** | Machine A and distinct Machine B are reachable; Chrome 151 can run at `390×844`; physical Android is explicitly unclaimed. | Install the same verified public asset on both machines, use HTTPS/WSS direct tailnet paths, pair B to A's controller origin, exercise direct multi-hub view, controller arbitration, and attributed audit, then restart Machine B and repeat stable identity/version/capability checks. |
| `H7-INVARIANT-03` one upstream, fan-out, backpressure, zero model/session multiplication | **SOURCE-ONLY COUNTS UNCHANGED** | Before/after source-guard process and session-record counts are identical. H1 source tests cover one upstream and bounded-viewer behavior. | Re-capture before/after counts around the installed-artifact demo, add model-request count from an authoritative source, browser network trace proving push/SSE/WS behavior with no agent polling, and one daemon upstream per session while multiple viewers receive correct pane output. |
| `H7-CRASH-04` honest abrupt-death diagnostics | **NOT RUN against artifact** | Source tests distinguish clean exit, signal, `SIGILL`, and unknown without invention. | Terminate one daemon, including `SIGILL` where supported, and capture the typed diagnostic plus proof that other sessions/viewers remain operational; restart daemon and hub separately and verify truthful recovery. |
| Old/new compatibility | **NOT RUN against artifact** | Protocol source tests retain legacy `Interrupt` and additive version/capability defaults. | Exercise old client/new daemon and new client/old daemon using identifiable released versions; record reported capability mismatch and safe degradation. |
| Credential/transport containment | **SOURCE REVIEW ONLY** | Hub-local state, exact Origin/DPoP, loopback-only plaintext, and Tailscale Serve ownership contracts exist in source. | Prove mode `0700/0600`, absence of raw credentials in URLs/logs/project DB, non-loopback plaintext refusal, TLS/WSS remote access, one-use ticket replay failure, and immediate revocation behavior on the installed release. |
| Child suites/workspace/release guards | **SOURCE GUARDS GREEN; ARTIFACT PENDING** | Browser suite, vendoring audit, full workspace tests, release build, and portable ISA audit passed at `bfa1e0af`. | Release workflow must build the immutable assets, publish digests, and allow both independently downloaded assets to identify the published version/commit. |

## Reasoning chain

1. The ADR says a local pre-release build cannot prove an invariant about a published artifact.
2. `bfa1e0af` is the exact assembled source tip, but it is unreachable from every current tag and absent
   from the public release list.
3. The only current public/installed version resolves to `22fe07c`, before Commander, and Machine B has
   no CAS binary at all.
4. Therefore running the two-machine browser flow from a locally compiled `bfa1e0af` binary would be a
   useful development exercise but an invalid release-acceptance claim.
5. The topology is actionable after publication: Machine B is genuinely distinct and reachable, and
   Chrome 151 supplies the specified phone-class viewport. Publication and identical installation,
   not test-harness availability, are the current boundary.

Ruled out: a dirty source tree, a stale epic ref, an unavailable second machine, a missing phone-sized
browser runtime, and missing vendor pins/notices. None of those explains the stop; the missing immutable
artifact does.

## What would falsify this verdict

The verdict becomes eligible to change only when all of the following are true:

1. a public immutable release tag peels to a mainline commit that contains `bfa1e0af`;
2. the release exposes Linux x86_64 and macOS ARM64 assets with recorded SHA-256 digests and a green
   portable x86_64 ISA audit;
3. the same verified Linux asset is installed on Machine A and Machine B and both `cas --version`
   receipts identify that published boundary; and
4. the complete four-invariant matrix above passes using those installed assets, with redacted raw
   evidence and unchanged before/after model-process and logical-session counts.

Publication alone does not falsify the verdict; it only unblocks the acceptance run.

## Exact published-artifact handoff

Owner: release supervisor for publication; fresh H7 continuation for installation and acceptance.

1. Integrate this evidence commit and the Commander source tip into the versioned main release commit.
2. Select the real next version through the repository's release process. Do **not** reuse `v2.54.1`,
   move any existing tag, or infer a version in this report.
3. Before tagging, prove the candidate main commit contains
   `bfa1e0afbd97f1750b41f8687196e999ed5fa46a`, the worktree is clean, and the source guards pass.
4. Create and push the new immutable tag. Record the exact tag, peeled commit, GitHub release URL,
   workflow run, asset names, asset sizes, and SHA-256 digests.
5. Download the published Linux x86_64 asset independently on `soundwave` and `unicron`; verify each
   archive digest against the published receipt, extract safely, and install the binary atomically at a
   stable non-worktree path.
6. Capture `cas --version` on both machines and require exact version/commit equality before starting a
   hub. If either differs, stop.
7. Re-capture process/session/model-request baselines; start each loopback hub through Tailscale Serve;
   record redacted HTTPS health and WSS upgrade outcomes without retaining URLs containing credentials,
   pairing fragments, DPoP proofs, Authorization headers, or tickets.
8. Run the full matrix in the documented fleet runbook with Chrome 151 fixed at `390×844`, including
   multi-viewer fan-out, controller arbitration, malicious requests, replay/revocation, hub restart,
   daemon restart and abrupt death, compatibility, audit attribution, and no-polling network trace.
9. Capture after counts using the same measurement commands. Only an unchanged authoritative
   model-call/logical-session result plus all green gates permits “Commander v1 releasable.”
10. Replace this pre-publication report with (or append) a final published-artifact receipt. Fill the
    real version in the adjacent Slack draft, review it against the rubric, and post only after the
    release is immutable and the acceptance verdict is green.

## Redaction policy applied

- No pairing code, URL fragment, opaque credential, Authorization header, DPoP proof, private/public
  key body, WebSocket ticket, request body, audit payload, tailnet IP, or project-database content was
  captured in this report.
- Machine labels, versions, commit IDs, aggregate process counts, aggregate session-record counts, test
  totals, HTTP status classes, and asset digests are acceptable evidence.
- Future raw browser traces must be sanitized before persistence: delete values for `Authorization`,
  `DPoP`, cookies, fragments, query strings, and ticket/credential/token fields; retain only route
  templates, methods, response status, timing, connection type, and redacted origin labels.

## Next actions

| Action | Owner | Completion proof |
| --- | --- | --- |
| Publish an immutable version containing `bfa1e0af` without moving an existing tag. | Release supervisor | Tag peel, release workflow URL, asset digests, and source ancestry receipt. |
| Install the same downloaded artifact on both real machines. | Fresh H7 continuation | Matching archive digests and matching `cas --version` on `soundwave` and `unicron`. |
| Execute all four H7 invariants and compatibility/restart/security sub-gates. | Fresh H7 continuation | Redacted raw evidence plus final paired Markdown/HTML receipt. |
| Review and post the adjacent Slack draft only after a green final verdict. | Release owner | Exact published version inserted; two posts delivered to `#cas-internal`; saved draft matches posted copy. |

## Provenance

- Markdown source: `docs/reports/2026-08-09-commander-v1-acceptance-gate.md`
- Human review surface: `docs/reports/2026-08-09-commander-v1-acceptance-gate.html`
- Unposted communication draft: `docs/release-notes/2026-08-09-commander-v1-slack-draft.md`
- Source commit examined: `bfa1e0afbd97f1750b41f8687196e999ed5fa46a`
- Binding ADR: `docs/specs/2026-08-08-commander-security-architecture.md`, especially
  `H7-ADV-01`, `H7-FLEET-02`, `H7-INVARIANT-03`, and `H7-CRASH-04`
- Fleet runbook: `docs/commander-fleet-runbook.md`
- Commands used (secrets and network addresses omitted by construction):

  ```sh
  git fetch origin epic/cas-fleet-control-plane-v1-option-b-per-machine-ca-cas-bec9 --tags
  git rev-parse origin/epic/cas-fleet-control-plane-v1-option-b-per-machine-ca-cas-bec9
  git tag --points-at bfa1e0af
  git tag --contains bfa1e0af
  git rev-parse 'v2.54.1^{}'
  gh release list --repo pippenz/cas
  cas --version
  tailscale status --json  # transformed immediately to a redacted topology summary
  tailscale ping --c 1 MACHINE_B
  ssh -o BatchMode=yes MACHINE_B 'hostname; uname -s; uname -m; command -v cas; cas --version'
  npm ci && npm run typecheck && npm test && npm run build
  git diff --exit-code -- hub-web/dist
  sha256sum hub-web/src/terminal/ghostty/vendor/*.wasm
  gh api repos/pingdotgg/t3code/commits/05eb051184ac4d486795ac6f8be29129b8b8845f
  gh api repos/ghostty-org/ghostty/commits/9f62873bf195e4d8a762d768a1405a5f2f7b1697
  cargo test --workspace --quiet
  cargo build -p cas --release
  scripts/check-portable-x86_64-isa.sh RELEASE_GHOSTTY_ARCHIVE
  ```

The process count is the number of exact executable names `claude`, `claude-code`, `codex`, and
`grok` in `ps -eo comm`; the session count is the number of JSON summary records directly under
`~/.cas/sessions`. These are intentionally narrow, repeatable measurements. Final acceptance must add
an authoritative model-request counter rather than infer model calls from process names.
