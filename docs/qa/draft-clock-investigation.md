# Commander draft under an artificial clock — cas-e265

The original empty-draft failure did not reproduce in **21 completed clock-trigger probes** against the same production bundle. This is a non-reproduction result, not a product fix or proof that the original failure was harmless. The historical cause remains unknown; the supervisor explicitly accepted that limit (#2125448).

## Build and experiment

Base epic: `cb5ee1fad9ce1be33230dd2730a5cc15e9c04300`. Investigation commit `2cc9bb89c` restored the original trigger and added test-side textarea lifecycle recording. No product source or dist edits, rebuilds, or Rust execution. Playwright 1.63.0 served committed `hub-web/dist` at `/commander/`.

Historical test revision `d4e6c9230` and this checkout have identical bundle blobs:

| Asset | Git blob |
| --- | --- |
| `dist/app.js` | `1f833de71795fe90dbe7f1cee6e8441697e8704c` |
| `dist/app.css` | `8e732aeabfc684b65c1a52e056c25ba4978f3ebd` |

The originating worker confirmed there was **no injected pairing/HTTP delay**. Both journeys ran concurrently in two workers. HUB-J5 opened cas-src / patient-pelican-9, filled `Please verify the gate first.\nKeep this half-written reply.`, set caret 7, retained the field reference, then awaited:

```ts
await Promise.all([
  page.waitForResponse(response => new URL(response.url()).pathname === "/v1/sessions"),
  page.clock.runFor(5_500),
]);
```

The clock was the standard installed, advancing journey clock. We restored exactly that stage and ran both entire journeys; no timeout increases or retries. Traces were retained on failure.

```sh
npm run journeys -- reply-typed.journey.ts pair-link.journey.ts --workers=2 --repeat-each=20 --trace=retain-on-failure
npm run journeys -- reply-typed.journey.ts pair-link.journey.ts --workers=2 --repeat-each=2 --trace=retain-on-failure
```

| Run | HUB-J5 full passes / attempts | HUB-J2 full passes / attempts | Completed draft clock probes | Wall / exit |
| --- | --- | --- | --- | --- |
| Original matrix | 16 / 20 | 18 / 20 | 19, all draft assertions passed | 1368.75s / 1 |
| Supplemental matrix | 2 / 2 | 2 / 2 | 2, both passed | 135.73s / 0 |
| Total | 18 / 22 | 20 / 22 | **21, zero draft mismatches** | 38 full passes, 6 timeouts |

The matrix is **not** reported as 40 passing tests. Its four HUB-J5 failures were:

- Repeat 16: timeout in the later long-name/overflow stage. Heartbeat draft assertion `call@317` and switchback assertion `call@370` passed.
- Repeat 17: timeout clicking the later Show unsent control. The same two draft assertions passed.
- Repeat 18: timeout during caret setup, before `runFor`; no recorded draft input event. Excluded from the completed-probe count.
- Repeat 19: timeout waiting for the later refused-message bubble. Heartbeat assertion `call@232` and switchback assertion `call@317` passed.

The two HUB-J2 failures timed out in the invitation stage or page setup. Host load rose to approximately 140–200 during the late failures; it had fallen earlier in the run. A failed full journey can still have completed the narrowly identified draft probe; the retained traces establish which did.

## Mechanisms ruled out

A fake clock cannot skip a scheduled draft-capture tick: there is no such tick. [`captureMessageDraft`](../../hub-web/src/main.ts#L2142) synchronously saves the visible field under its DOM thread key. [`render`](../../hub-web/src/main.ts#L2854) calls it at line 2858, before the render decision and `app.innerHTML` replacement at line 3086. Restoration at line 3198 stamps the replacement's thread key, text and caret. The identical built bundle contains capture `Gg` and restore `Vg` at `dist/app.js:96`, and invokes capture at the start of render `_`, before its shell assignment; restoration is at `dist/app.js:184`. This rules out the proposed **replacement-before-capture ordering inside that render**, not every possible product bug.

HUB-J2 cannot directly replace HUB-J5's composer: the two tests have independent browser contexts and protocol doubles. Their shared resource is host capacity. There was no re-pair transition inside the failing HUB-J5 stage. [`restoreLastSession`](../../hub-web/src/main.ts#L951) also declines automatic restoration once a session is selected; the separate paired-session opener at line 965 acts only on the newly paired machine's first catalog.

All six reproduced timeout failures differ from the reported historical empty-value assertion. Three failing HUB-J5 traces positively preserve the draft through both heartbeat and shell replacement. The fourth never executes the clock trigger. They are not evidence of draft loss.

## Remaining hypotheses, ranked

1. **Harness/browser scheduling under host contention — leading hypothesis, unconfirmed.** A 5500ms browser-only jump can cross 3000ms protocol deadlines before Node route replies, signing, socket frames or body decoding settle. The same defect class was established while developing the acknowledged protocol clock in cas-8d75. Contention also separates Playwright's focus/selection and text-insertion operations. This explains a timing hazard, but does not by itself explain an empty captured draft. Confirmation needs a mismatch trace showing whether a real input event reached the current live node, its identity/thread before and after the jump, and any write/replacement that followed. A controlled gate could then replay that exact ordering and compare acknowledged clock advancement.
2. **A product persistence race outside the examined synchronous ordering — possible, not reproduced.** Confirmation requires a typed draft on the live node and unchanged selected thread followed by a product caller clearing or replacing it without preserving the value. The new diagnostic records the input, last textarea write and actual shell-render caller stack. If that occurs, retain the trace, identify the source path and escalate before product edits.
3. **An unexpected conversation change — less likely in this setup.** An empty composer for a different thread would be a different failure than erasing the original thread's draft. Confirmation requires differing old/current thread keys and heading/selection state, then returning to the original conversation to check its saved draft. Separate-context pairing and the selected-session restore guard argue against this explanation for the original stage.

The original log survives in fierce-stork-18's `target/cas-9d89-journeys.log`. Its failed trace was overwritten by a later successful natural-response run (start 2026-09-30T22:38:20.781Z, 578 actions / 202 requests / 0 errors, no `runFor`). It cannot answer these questions. No historical cause is asserted.

## Next-occurrence diagnostic

`draft-diagnostic.ts` records stable WeakMap node IDs, current/reference connectivity and thread keys, text/caret, last input, last textarea assignment and the last shell-changing render's caller stack (bundle function and file:line:column). It observes native DOM setters and input events without timers, a fake-clock change, or product debug hooks. The caller stack is the observable render reason; it does not invent a product state label. Only the latest records are retained.

Both heartbeat and switchback draft checks attach `composer-draft-mismatch` JSON on failure and rethrow the original assertion. Diagnostic collection failures annotate the report rather than masking that assertion. Passing runs write no diagnostic attachment. The natural-response heartbeat remains the normal journey behavior.

Detailed logs, failure traces, extracted lifecycle JSON, the original-trigger patch and diagnostic probe evidence are under the task artifact directory `cas-e265/`. Validation results are recorded in the task's delivery notes.
