# Hub pairing works; recovery still asks the user to guess

The current browser can complete both pairing journeys on desktop and a phone-sized touch viewport. The highest-impact remaining defect is state visibility: after a failed exchange, the dialog can keep “Pairing…” disabled until the user blurs an autofocused field. Cancellation and storage failures also hide or contradict the next action. Fix these state transitions before simplifying the presentation. Confidence is high for the reproduced browser behavior and traced Rust contracts; physical-phone, production relay and real VPN behavior were not exercised.

| Review question | Verdict |
|---|---|
| Can a phone start without an invitation? | Yes: create code, approve on the machine, then confirm in the browser. |
| Can an existing machine invitation finish directly? | Yes: it opens the confirmation form without creating another code. It still needs a hub address and two labels. |
| Can users recover without guessing? | Often, but recoverable failures can leave the button disabled, and some states silently discard or conceal context. |
| Review scope | Current CLI, browser and hub server source; isolated browser and Rust fixtures. Product source remained read-only. |
| Snapshot and date | `9905c4c6679b909659d695cc1b88d1ba77b0f13a`, September 4, 2026. Pairing product paths match fetched `origin/main` at `bbc1629b`. |
| Audience and ownership | Practitioner review for the hub integration owner and visual-overhaul owner. Review task: cas-2226. |

## Evidence and measured journeys

The browser evidence comes from the actual frontend compiled from the snapshot, with synthetic API responses and real browser DOM, focus, WebCrypto and IndexedDB. Every external API call was intercepted, and remaining outbound requests were blocked. No production pairing, approval, credential inspection or VPN change occurred. Synthetic identifiers are not usable credentials. Desktop was Chrome 152.0.7977.82 at 1440×900; phone was the same engine at 390×844 with touch/mobile emulation. These are browser viewport observations, not physical-device certification.

The evidence set contains **52 scenario runs**: 22 primary cases plus four relay cases, each on both surfaces. Three further checks covered keyboard tab order, a 390×360 reduced viewport and 844×390 touch landscape. The 390×360 check approximates available space with a keyboard open; it does not invoke an Android or iOS keyboard. Existing scoped tests passed: **83 frontend tests and 23 Rust CLI/auth tests**.

### Step count and where the work occurs

Counts below treat a text field fill as one action, not one action per character. Browser action counts exclude opening the site and scrolling. Machine commands and terminal consent are separate, source-traced steps, not fabricated browser taps. Camera/QR handling, app switching, command transfer, and typing latency vary by device and were not timed.

| Journey | Observed browser actions | Machine steps and boundary | Friction and result |
|---|---:|---|---|
| Phone or desktop starts without an invitation | 4: open Pair, Create pairing code, fill Operator label, Pair | Run `cas hub authorize <code>` and approve the exact origin/requested capabilities. A hub without a public address also needs configuration. | Browser creation, poll, approval display, exchange, persistence and reload passed. Copy command is an optional fifth browser action; moving it to another device is outside this count. |
| Existing `cas hub pair` link | 4 after opening: correct Hub URL, fill Machine label, fill Operator label, Pair | Run `cas hub pair --origin <Commander origin>`; open its link or scan its QR. No second code or authorization command is required. | All four browser actions passed on both sizes. Hub URL initially equals the Commander origin, which is wrong when the hub is another host. Device label is prefilled. |
| Existing link delivered to an already open tab | 0 actions to reveal the form | Deliver the new URL fragment. | The fragment is consumed and scrubbed, and the form opens with Hub URL focused. Same-tab delivery is working. |
| Offline exchange, then network restored | 2 extra observed actions: tap heading to blur, Pair | Restore reachability; no new invitation required if the request was never processed. | The first action is an accidental workaround for the disabled retry button. Desired count is one explicit Retry. |
| Code expires while waiting | 1 action to create a replacement code | Run and approve the new command on the machine. | Expiry is visible; fresh-code creation recovers. Reusing the expired capability is not proposed. |
| Pairing relay temporarily unavailable | 1 additional Create pairing code action after service recovery | No authorization was sent to a machine. | Error is visible and creation retry succeeds. |
| Credential installed, hub temporarily offline | 0 pairing actions after network recovery | Hub connectivity returns. | Stored machine persists; automatic reconnect succeeds. This is a transport state, separate from pairing. |

CLI provenance: `cas-cli/src/cli/hub.rs:1143` prints expiry, scopes, link and QR; `hub_reverse_pairing.rs:312` resolves the hub, claims, confirms, mints and completes delivery. `hub_reverse_pairing.rs:399` requires an affirmative terminal confirmation by default and prints the exact origin with read/control capabilities separated. The fixtures exercise claim retry, scope reduction and stopped-hub recovery; the terminal prompt was source-traced rather than approved against a live service.

### Failure and recovery matrix

“Both” means the named case was run at both browser sizes. Backend facts come from the named Rust tests or source trace, not the browser response stub. A generic failure response can represent several server conditions; the browser cannot safely infer which one occurred.

| Condition | Evidence | Observed state and recovery | Assessment |
|---|---|---|---|
| Fresh relay request → machine claimed → authorized | Both: `relay-pending`; Rust claim/nonce/consent tests | Copyable command, countdown, claimed-state next step and machine details appear; final Pair stores a credential. | Working. Preserve the explicit terminal and browser confirmation boundaries. |
| Fresh direct invitation | Both: `legacy`; Rust single-use test | Form opens directly; read scopes are selected and unavailable control scopes disabled; pairing succeeds after required fields. | Working, with excess address/label input. |
| Reload after successful pairing | Both: `relay`, `relay-pending` | Machine catalog survives; no new invitation is needed. | Working. Persistence is distinct from a live session. |
| Malformed link | Both: `malformed` | Fragment is scrubbed, no dialog or explanation opens; page looks like ordinary first use. | Dead end without feedback. Keep scrubbing, add a safe message. |
| Pending invitation already expired at reload | Both: `expired-load` | Store clears it; page returns to ordinary first use without saying why. | Explain expiry and offer a new invitation. |
| Expired waiting code | Both: `relay-expired` | Dialog says expired; Create pairing code obtains another request. | Working recovery, cluttered by the unrelated disabled Pair control. |
| Used or expired exchange invitation | Both: `used`, `expired`; Rust `h2_pair_02_pairing_is_bound_persistent_single_use_and_fragment_only` | Server-style 401 clears pending state and gives a remint command in a toast; dialog stays on disabled Pairing until blur. | Security rejection works; current error presentation fails. |
| Explicit origin mismatch in delivered invitation | Both: `wrong-origin`; `pairing-exchange.ts:52` | Rejected before any exchange call; pending invitation cleared; explanation goes to toast while stale form persists. | Preserve rejection, fix visibility and next action. |
| Other host or unbound origin with opaque CORS refusal | Both: `wrong-host` models browser fetch rejection; Rust CORS fixture | Same “could not reach” message as offline. | Browser cannot distinguish wrong host, CORS, DNS and offline from this signal. Do not assert a unique cause. |
| Hub unreachable during exchange | Both: `offline` | Pending invitation retained; retry succeeds after heading blur and network restoration. | Recoverable, but disabled-button state is wrong. |
| Reachability probe fails after relay approval | Both: `relay-health`; `main.ts:673` | Authorized form opens before health result; focus can defer the result. Pair still exists. Source warning asks for a fresh code. | Advisory probe must not force a remint; offer reachability retry while invitation remains valid. |
| URL returns 404 | Both: `not-found` | Check Hub URL message; invitation retained; successful retry after blur. | Clear intent, but delivered relay form has no editable URL. Prefer checking machine-side published address when fixed by invitation. |
| Hub returns 500 | Both: `server-error` | Invitation retained; “wait a few seconds” message; retry succeeds after blur. | Preserve bounded retry; avoid promising the invitation is definitely unused. |
| Actual hub exchange rate limit | Both: `rate-limit-actual` models traced 401; `auth.rs:542`, `server.rs:837` | Server maps the rate-limit error to generic 401; browser clears pending state. | Client's helpful 429 branch is unreachable for this server path. |
| Hypothetical 429 exchange response | Both: `rate-limit-copy` | Invitation retained and wait-a-minute copy selected; retry succeeds after blur. | Demonstrates the client branch, not current server behavior. |
| Hub processes exchange, response is lost | Both: `lost-response`; server consumption at `auth.rs:577` | UI says invitation is still open; retry receives 401 because fixture has consumed it. | Claim of certainty is false under this valid failure ordering. |
| Browser credential write fails after exchange | Both: `storage`, real IndexedDB write boundary fault injected | Generic toast “Uncaught exception in event handler”; dialog retains Creating text. After restoring storage and retrying, consumed invitation is rejected. | Requires a durable storage-specific recovery state. No false success toast occurred. |
| Cancel before exchange / Escape | Both: `cancel`; cancellation unit tests | Dialog closes and pending invitation is discarded; reopening requires a fresh request. | Working local discard semantics. Does not revoke the server invitation. |
| Cancel while exchange is in flight | Both: `cancel-flight`; coordinator/rollback unit tests | Delayed reply cannot install a selected machine or show success. | Preserve generation invalidation and staged/active rollback checks. |
| Cancel when session storage removal and tombstone write both fail | Both: `cancel-storage`; pending-store tests | Keep-page-open warning is generated inside the already closed dialog. Reopening reveals it. | Dangerous visibility gap: reload safety is unknown, but the instruction is hidden. |
| Pair succeeds; hub subsequently fails health | Both: `paired-offline` | “Fixture workstation paired” toast accompanies a reconnecting machine. Connection returns without re-pair. | Pairing success is true about credential storage; distinguish it from Connected. |
| Freshly stored credential is rejected/revoked | Both: `revoked`; Rust expiry-vs-revoke test | Catalog remains; auth failure offers Re-pair, while immediate toast still says paired. | Separate saved credential from verified connection. Revocation must never auto-refresh. |

## Ranked findings and reasoning

Priorities reflect user impact, not a claim of production exploitation. P1 items interrupt safe completion or recovery; P2 items add friction or ambiguity. Effort is an estimate of scope, not a delivery promise.

| Rank | Priority | Finding | Minimal change | Estimated scope |
|---|---|---|---|---|
| F1 | P1 | Failed exchange leaves disabled Pairing and stale status until focus moves | Update pairing status/buttons in place; preserve fields/focus across state changes | Medium: render boundary plus browser regression |
| F2 | P1 | Failed cancellation cleanup hides the keep-page-open warning | Keep cleanup failure visible in the modal; offer Retry cleanup | Small–medium: cancellation and recovery UI |
| F3 | P1 | Storage failure after exchange lacks a truthful recovery state | Show credential-not-saved state with storage recovery and fresh-link guidance | Medium: install failure classification and UI |
| F4 | P1 | Network rejection claims unused invitation; throttling destroys recoverable state | Distinguish uncertain exchange outcome; classify bound throttling safely | Medium: typed server outcomes, CORS and UI tests |
| F5 | P2 | Direct link requires an address it does not carry and defaults to the wrong host | Explain the expected hub address; explore a versioned local-link hint | Small for copy; medium for link contract |
| F6 | P2 | Invalid or reload-expired invitation vanishes without explanation | Carry a nonsecret parse/expiry result into first-use state | Small: parser/store result and UI |
| F7 | P2 | Entry mixes two methods; capabilities and labels require translation | State-specific primary action and plain capability summaries | Small: copy/layout; coordinate with visual overhaul |
| F8 | P2 | Paired and connected appear as one outcome; re-pair loses machine context | Separate saved/connecting/connected and name the machine being repaired | Small–medium: connection/repair presentation |

### F1 — Update status without rebuilding the form

Repro on either size: load an authorized invitation, fill Operator label, submit, and reject the exchange fetch. The submission renders an in-flight shell and reopens its dialog, so `autofocus` moves focus to Device label. When the error settles, `render()` sees an editable element and defers the new shell. The toast says to tap Pair again while Pair remains disabled. Tapping the heading triggers blur, after which Pair becomes enabled and retry succeeds.

The behavior crosses `main.ts:530` (in-flight render), `main.ts:583` (failure render), `main.ts:1373` (autofocus), and `main.ts:1734` (pairing status in shell signature). `render-model.ts` treats every focused input as composing; the live-region branch does not update the pairing controls. The same mechanism hides terminal failures and delays the post-approval health result. This is reproducible even without typing during the request.

`mobile-offline-result.png` additionally shows the body-level toast behind the modal/backdrop while the dialog itself still displays Creating. A correct toast string in the DOM is insufficient evidence of an actionable error. The status and retry action must be inside the current dialog and announced there.

Proposed change: retain the form DOM while updating its status, busy attributes and buttons. Focus only on entry to a genuinely new step. Preserve the existing heartbeat draft/caret protections; do not fix pairing by globally disabling deferred rendering. Test error arrival with Device label focused, keyboard submit, replacement invitation and a pointer gesture in progress.

### F2 — Cancellation needs visible cleanup completion

`main.ts:719` closes the dialog before `pendingPairingStore.clear()` reports whether removal or a tombstone was durable. The failure sentence at `main.ts:727` explicitly tells the operator to keep the page open. In both fixture runs, the dialog was closed, there was no toast, and that warning became visible only after opening Pair again.

Keep successful cancellation as discard. When cleanup is uncertain, keep a focused, persistent “Could not finish cancellation” state visible and provide a cleanup retry. Do not offer to resume the old invitation. Make the security boundary clear: browser Cancel blocks local continuation; it does not remotely invalidate copies of the invitation. The current path never calls a hub revocation endpoint. The server invitation remains subject to its existing expiry/single-use policy.

### F3 — Treat storage failure after consumption as its own outcome

The hub consumes the one-time invitation and persists the device before returning (`auth.rs:576–599`). The browser then stages and activates its catalog record (`pairing-exchange.ts:129`). When the actual IndexedDB write boundary is faulted, `pairMachine()` takes its generic-error path and leaves `pairingStatus` at Creating (`main.ts:600`). Restoring storage does not unconsume the invitation.

Do not show raw storage exceptions or describe the credential as installed. Proposed state: “The machine approved this browser, but this browser could not save access. Restore browser storage, then get a fresh invitation.” Retain rollback and quarantine checks, and make pending cleanup visible. If a future design retries installation from an in-memory credential, it needs explicit cancellation/expiry ownership and must not serialize that credential into a new ad hoc store. This review recommends the simpler fresh-invitation recovery first.

### F4 — Preserve uncertainty and classify safe retries

A rejected `fetch()` does not prove a POST never reached its server. The browser's catch at `pairing-exchange.ts:87` and message at `pairing-messages.ts:66` assume exactly that. The lost-response fixture consumes the invitation and throws; the user is told it is still open, then gets a remint error on retry. A storage/audit failure after server mutation can produce similar uncertainty.

Minimal copy: “We could not confirm pairing. Check the connection, then retry. If the machine already used this invitation, you will need a new one.” Keep the invitation for a bounded retry, but never promise it remains unused or replay a consumed server capability. Do not introduce relaxed origin checks, broader scopes or credential bypass.

Separately, `auth.rs:552` enforces five attempts per minute for the source, but `server.rs:837–840` turns all bound failures into 401. `pairing-messages.ts:32` expects 429 to preserve the invitation. Add a typed throttled result and `Retry-After` only within the existing safe response/CORS boundary; unknown/unbound invitations should remain opaque. A new server test must demonstrate that the sixth bound attempt yields a retry state without widening information disclosure. The current 429 browser fixture is explicitly hypothetical.

### F5 — Carry or clearly request the machine's address

Direct links contain a token, hub ID and local-link scope ceiling (`auth.rs:247`), but no reachable address. `pairing-draft.ts:20` seeds Hub URL from the Commander page origin. On a hosted page this is a plausible-looking, wrong default for a remote machine. The four-field form then asks for Machine label, Device label and Operator label without explaining their purpose; only Device label is prefilled.

Immediate change: label the field “Machine's hub address” with concrete machine-side discovery instructions, and explain the two labels: the display name of the machine and who is using this browser. Preserve direct confirmation when an invitation is present. Longer term, a versioned, validated hub-address hint could remove one fill, but it must retain explicit target review. **Do not add new fragment fields to HostedRelay URLs**: the recent fix deliberately retains a two-key relay contract. LocalCommander and HostedRelay are separate projections.

### F6 — Scrub secrets and retain nonsecret failure context

`fragment.ts:36` returns null for malformed input while its `finally` always scrubs the fragment. `pending-pairing.ts:96` clears an expired stored invitation and returns null. Both become ordinary unpaired startup. This is safe secret handling but poor recovery feedback.

Return a typed, nonsecret outcome such as invalid-link or expired-invitation alongside the absence of a capability. Show “This pairing link is invalid” or “This invitation expired” and a fresh-link/code action. Never echo the token, URL fragment or stored raw value into the message, analytics, screenshots or logs. Already-open-tab delivery remains supported.

### F7 — One next action for the actual state

`main.ts:1378` places Close, disabled Pair and Create pairing code in one entry state, while also saying Pair requires a machine-generated URL. On a phone “Pair this machine” can refer to the phone rather than the workstation. A separate review in cas-ad97 already identified scope vocabulary and pairing-form vocabulary as follow-ups; they remain in current source.

For no invitation, use “Pair a machine” with Create pairing code as the primary action and concise machine-link instructions as a secondary path. For an existing invitation, open its confirmation form immediately with Pair as primary. Do not require or suggest creating a second code. For a waiting code, show “On the machine, run this command,” expiry and cancellation. Add “Read sessions and terminals” / “Type, send messages and interrupt” summaries while keeping exact origin and detailed scopes visible for consent.

The optional Email code field has `type=email` but is in a section handled by a button, not a validated form; server behavior was not probed for malformed email. Treat delivery/validation as an explicit follow-up, not a confirmed mail failure. The Copy command control measured 32px high; ordinary dialog actions measured 40px. These are compact touch targets, not evidence that actions are unreachable. The visible scope label is the hit target, so the 13px checkbox glyph alone is not an accessibility finding.

### F8 — Say when access is saved and when the machine is live

`main.ts:608–615` persists/selects the machine and starts a connection; the submit handler immediately says “Machine paired” (`main.ts:2341`). In `paired-offline`, this toast accompanies a machine waiting to reconnect; in `revoked`, it accompanies an authentication failure. The toast truthfully reports stored pairing, but does not prove live access.

Use a short progression: “Access saved — connecting to Fixture workstation,” then Connected after authenticated transport is ready, or a persistent connection failure with Retry / Re-pair. Existing `connection.ts:185` already models resolving, health, auth, stream attachment and live; reuse it. Re-pair should name the selected machine and explain how to replace its credential instead of reopening a context-free creation prompt. Preserve automatic refresh for eligible expired credentials and refusal for revoked credentials.

## Proposed minimal journey and states

This is a proposal, not behavior shipped by this review. Exact origin approval, capability ceilings, single-use tokens, staged credential activation and cancel-discard remain required.

| State | Primary action | User-facing meaning | Recovery or next state |
|---|---|---|---|
| No invitation | Create pairing code | Start from this browser and approve on the machine | Waiting code; secondary machine-link instructions |
| Invitation opened | Pair | Review target machine/address and capabilities; identify this browser/operator | Exchanging; never create another code merely to continue |
| Waiting / claimed code | Copy command; terminal approval occurs separately | Which machine action is still required and when the code expires | Authorized confirmation, Cancel, or expired replacement |
| Exchanging | Busy indicator; Cancel available | Creating and saving browser access | Saved/connecting, recoverable error, uncertain outcome, or storage failure |
| Recoverable / uncertain exchange error | Retry | What can be checked; no claim the server definitely did nothing | Success or fresh invitation if already consumed |
| Invalid / expired / used invitation | Create fresh code or obtain fresh link | Why this request cannot continue | A new request; no reuse of the old capability |
| Cancellation cleanup pending | Retry cleanup | Keep page open; old request cannot continue locally | Cancelled after durable cleanup proof |
| Access saved / connecting | Retry connection if needed | Pairing is stored; checking that the machine can be used | Connected or machine-specific Re-pair |
| Connected | Open a session | Authenticated connection is live | Automatic reconnect, eligible refresh, or explicit repair |

This table also supplies the full text equivalent of the proposed flow: start without a capability by creating a code; start with a capability at confirmation; both paths converge at exchange, durable save and connection. Errors return to the nearest safe retry boundary, and terminal invitation failures require replacement. There is no automatic approval or scope expansion.

## Counter-evidence, limits and falsifiers

The recent hosted-pairing defect is **already fixed** in this baseline: `ce217ef0` uses `PairingInvitationTarget::HostedRelay`, and `0cefcc67` pins its two-key contract. The current code binds `window.fetch`; old unbound-fetch failures are not a finding. Fragment watchers handle already-open Android-style URL delivery. Scope ceilings prevent browser over-requesting. Cancellation invalidates in-flight operations and catalog staging protects reload from activating a cancelled credential. Existing tests cover these mechanisms.

A wrong initial fixture returned all six scopes to a direct read-only invitation; the client correctly rejected it. The fixture was corrected to return only requested scopes and rerun. That rejection is positive security evidence, not a pairing defect. An initial harness wait also targeted a background control hidden by a modal; final runs wait on the relevant form. Neither harness failure is counted among the product findings.

The layout did not horizontally overflow at 1440, 390 or 844px. In the 390×360 reduced viewport, the internally scrolling form could reach its required fields and the 40px Pair action, and submission succeeded. Keyboard tab traversal reached the fields, selected scopes, Copy command, Cancel and Pair. The first tab could return to Hub URL during startup rebuild; the more consequential repeatable focus regression is F1. No claim is made that mobile layout is globally broken.

What would falsify the findings: a browser rerun where an error arriving with a field focused updates the visible status and enables retry without blur would overturn F1; a visible cleanup warning before navigation would overturn F2; a truthful storage failure state would overturn F3. A server-side typed rate-limit outcome with preserved CORS constraints would invalidate the rate-limit portion of F4. Those observations must be made on the deployed source revision, not inferred from unit tests that only compare message strings.

Not verified: production relay availability/email delivery, device camera QR handling, real Android/iOS keyboard/VoiceOver/TalkBack behavior, physical VPN/DNS/network switching, end-to-end live CLI-to-cloud-to-phone transport, or adversarial exploitation. The review traces all components and verifies local boundaries; it is not a production pairing certification. The synthetic hub reports no optional session capabilities, so a version-skew warning in some screenshots is fixture context, not a new product finding. The safe fixtures deliberately avoid live invitations, token capture and credential bypass. A follow-up physical-device smoke should use disposable isolated hub state and explicit operator approval at the normal consent screen.

## Actionable handoff

1. **Integration owner: schedule F1–F4 as bounded behavioral fixes.** First preserve visible status/focus, then visible cleanup/storage states, then typed retry outcomes. Add browser regressions that observe actual enabled buttons and visible modal text, plus server assertions for rate limiting and uncertain completion. Keep changes scoped to each failure boundary.
2. **Visual-overhaul owner: align F5–F8 with the two starting journeys.** Entry without invitation has Create code; an existing invitation opens directly to Pair. Use plain capability summaries and explain labels while retaining exact consent details. Do not change HostedRelay wire shape in a visual patch.
3. **Verifier/operator: run physical-device follow-up after fixes.** Exercise same-tab link opening, actual keyboard, QR, VPN transition, Cancel during response delay and storage denial using disposable fixtures. Do not accept a success toast as proof of Connected.

Initial findings and the confirmed focus/cancellation issues were sent directly to `tender-panda-58`, with supervisor copies (notifications 25379 and 25383). Product changes and new fix-task ownership remain with the supervisor after this review; no fixes were folded into the audit branch.

## Provenance and reproduction

Markdown is the source of this report; its sibling HTML is a self-contained rendering. Snapshot: `9905c4c6679b909659d695cc1b88d1ba77b0f13a`. Product-path comparison against fetched `origin/main` was empty. `hub-web/DESIGN.md` was read and checked against the render, cancellation and styling code. Prior cas-ad97 findings informed the vocabulary/recovery checks; the mobile/visual context is cas-6564 and cas-b433. No old screenshot or old test result is counted as fresh proof.

Source locations use the snapshot's line numbers. Shorthand file names above resolve to these paths:

- [`main.ts`](../../hub-web/src/main.ts): form/exchange 525, relay polling 662, cancel 717, markup 1364, render signature 1734, submit feedback 2335.
- [`fragment.ts`](../../hub-web/src/fragment.ts): validation and scrubbing 28; same-tab watcher 69.
- [`pending-pairing.ts`](../../hub-web/src/pending-pairing.ts): expiry/load 93, save 108, cancellation clear 123.
- [`pairing-exchange.ts`](../../hub-web/src/pairing-exchange.ts): origin 52, fetch 74, failure 87, staging 129.
- [`pairing-messages.ts`](../../hub-web/src/pairing-messages.ts): retry classification 30, reachability copy 66.
- [`pairing-draft.ts`](../../hub-web/src/pairing-draft.ts): initial address and labels 17.
- [`render-model.ts`](../../hub-web/src/render-model.ts) and [`styles.css`](../../hub-web/src/styles.css): editable-focus deferral; scrolling dialog 1630/1677.
- [`auth.rs`](../../cas-cli/src/hub/auth.rs): invitation projections 247, mint 493, throttling 542, consumption/persistence 576.
- [`server.rs`](../../cas-cli/src/hub/server.rs): exchange/CORS response mapping 818.
- [`hub.rs`](../../cas-cli/src/cli/hub.rs): pair command 1143.
- [`hub_reverse_pairing.rs`](../../cas-cli/src/cli/hub_reverse_pairing.rs): claim/approve/deliver 312, consent 399, hub resolution 466, CLI recovery messages 781.
- [`connection.ts`](../../hub-web/src/connection.ts): connection progression 185, expired refresh and revoke classification 235.

Evidence root: `/home/pippenz/.cas/artifacts/cas-2226/`. Browser scripts, structured observations, focused screenshots and logs live there. `browser-review.json` contains 44 cases; `relay-review.json` contains eight; `interaction-proof.json` contains three geometry/keyboard checks. `unit.log` records 83 passes; `rust.log` and `rust.exit` record 23 passes and exit 0. The Rust compile took 3m00s; the resulting scoped run took 47ms. Test counts count tests or scenario runs, not independent user studies. No user-completion timing was measured.

Reproduce from the snapshot (Playwright and Chrome paths in the scripts are environment-specific):

```bash
npm --prefix hub-web ci --ignore-scripts
npm --prefix hub-web run build -- --outDir "$PWD/target/cas-2226/site"
npm --prefix hub-web test -- src/pairing.test.ts src/pairing-scopes.test.ts src/connection-state.test.ts src/connection-lifecycle.test.ts
scripts/run-scoped-tests.sh -p cas --lib -E 'test(hub::tests::h2_pair) | test(hub::tests::h4_pairing) | test(hub::tests::expired_device) | test(hub_reverse_pairing::tests::)'
node target/cas-2226/browser-review.cjs
REVIEW_RUN=relay-review REVIEW_CASES=relay-pending,relay-expired,relay-service,relay-health node target/cas-2226/browser-review.cjs
node target/cas-2226/interaction.cjs
```

Generated app assets stayed under the worktree's ignored target directory. Only the report Markdown and HTML are deliverables in this commit. Long-running compilation and scenario batches ran detached so supervisor messages remained receivable. Report validation separately checks content fidelity, offline rendering, keyboard access, mobile width and A4 output; those receipts are in the same artifact directory.
