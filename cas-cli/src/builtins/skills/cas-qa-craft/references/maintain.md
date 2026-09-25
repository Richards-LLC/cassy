# Verification kit maintenance sweep

Run once per release, in the background, for projects that have
`docs/qa/features/`. The sweep keeps `docs/qa/verify.md` and the feature map
true for features nobody touched this release; touched features are kept
current by the close-time drift check and by QA use. Nothing waits on the
sweep.

**Hard cap: about 15 minutes.** When the cap expires, record what was not
reached and stop.

## Procedure

1. Run the static check from the project root and fix doc-only failures
   (a renamed Touches glob, a missing index line):
   `node <skills-dir>/cas-qa-craft/scripts/check-feature-map.mjs --root .`
2. Launch and Doctor from `docs/qa/verify.md`.
3. For each feature not touched since the last sweep, oldest first, follow
   its "Driving it" steps once against the real build.
4. Triage every step that did not behave as written into exactly one bucket:

   | Bucket | Meaning | Action |
   | --- | --- | --- |
   | doc drift | The product changed on purpose; the doc is stale. | Edit the feature file or `verify.md`. |
   | harness gap | Launch, Doctor or the drive tooling is broken or missing. | Fix `verify.md` if it is a doc fix; otherwise file a task. |
   | product regression | The product no longer does what the user expects. | File one task per defect with the failing step and capture. |

5. Cleanup from `docs/qa/verify.md`: kill only what you started; evidence
   survives.
6. Record one outcome line at the top of `docs/qa/features/README.md`:
   - `Sweep: <date> <commit> clean`: every reached feature drove as written.
   - `Sweep: <date> <commit> changed: <files>`: docs were corrected.
   - `Sweep: <date> <commit> blocked: <reason>`: the cap expired or a
     harness gap stopped the sweep; name the features not reached.

## Boundaries

The sweep edits only `docs/qa/`. It never edits product code, even for a
one-line regression fix: product regressions become tasks. It does not
re-drive features touched this release unless the static check fails for them.
