# Journey evaluation — hub-web/dist 7bface8f

**No Blocking finding. The 3.52 cut may proceed on this dist.** The carried HUB-J9 "Jump to latest" flake (cas-6a9b) is resolved in this dist. One new finding is about suite evidence, not the product: HUB-J11's "The network drops" screenshot catches the reconnect instead of the outage. It is filed as P3 cas-793b. The three Normal findings from 3.51.0 are unchanged, because their fixes are not in this dist.

3.52 ships one hub-web change on top of 3.51.1. The 3.51.0 and 3.51.1 dist is 62930625, evaluated in `2026-10-14-hub-web-62930625.md`.

- **"Jump to latest" scroll fix (cas-6a9b, HUB-J9 step 5).** In `conversation-view.ts`, a scroll that moves the thread up and off its tail now always counts as the reader. Following stops and "Jump to latest" appears before the pin-pending, height and content-growth guards run. Before this, a reader who scrolled up within two frames of a pin was ignored, and the pending re-pin took them back to the bottom.

`git diff v3.51.1 7bacdd73b -- hub-web/src` changes only `conversation-view.ts` (+19 lines) and its unit test. `hub-web/e2e` and `docs/qa/journeys.md` are unchanged. The shipped `hub-web/dist/app.js` contains the fix (`if(l<this.scrolledTo-1&&!Bo(this.element)){this.following=!1,this.jump.hidden=!1,…`), and the v3.51.1 bundle does not. `git merge-base --is-ancestor 354e260f0 7bacdd73b` is true.

In this run, all 141 tests pass (`verified-web-tests: PASS (141 playwright tests passed)`, 13.1 min, suite_exit 0). Every `result.json` has empty `page_errors` and `frame_defects`.

## Receipt

- hub_web_dist: 7bface8f5af16e01cdabafbfc36956cddcd8a46a
- evaluated_commit: 7bacdd73b38256798a2865a9de5e93e3acadfdfb
- suite_run: /home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-baa3 — 21 journeys, 21 PASS; native 141 passed, 0 failed, scope full, workers=4, suite_exit 0
- evaluator: bright-crow-84 (taste lane; not an implementer of the release epic. It has no commits on epic cas-baa3, did no QA for cas-6a9b, and launched no suite for this report)
- label: real-bundle, protocol-double
- blocking_findings: 0

`git rev-parse 7bacdd73b:hub-web/dist` gives 7bface8f, which matches the `hub_web_dist` line in `journeys/JOURNEYS.md`.

### Evidence reviewed

The receipts of the 3.51.0 run (dist 62930625) are no longer on disk. This run was therefore compared with the 3.50.0 run (`release-3.50.0/journey-eval`, dist 09747d57), and every difference was checked against the 3.51.0 changes that the 3.51.0 report lists. The scripts, outputs, crops and extracted frames are in `cas-449d/` in the artifacts:

- `scripts/compare.py`, `residual.py` and `sheet.py`;
- outputs in `out/`;
- contact sheets in `out/sheets/`;
- frames in `frames/`.

- **result.json.** All 141 are PASS, with no page errors and no frame defects (`out/results.txt`).
- **Every stage screenshot.** All 446 were read on 20 contact sheets: 12 wide (287 screenshots) and 8 narrow (159). Pixel comparison with 3.50.0 (`out/shots.txt`):
  - 147 identical;
  - 21 within rendering noise;
  - 245 differing;
  - 30 with no 3.50.0 counterpart (3.51.0's new HUB-J21, HUB-J9 sideways and HUB-J2 parts, plus the HUB-J8 every-tap parts);
  - 3 with a changed size (the HUB-J2 admin-invitation part, the cas-f7c5 spec split).

  The 245 differ because of 3.51.0's lockup chevron, footer build id, rail permission lines and Paired machines line.
- **Thread-column residual.** The 7bface8f change only affects where a thread is scrolled. So the comparison was repeated on the conversation column alone (1280: x 392–1030, y 130–625; 390: y 120–750; `scripts/residual.py`, `out/residual.txt`). 36 screenshots differ there. Each was classified:
  - Paired machines and pairing dialogs that cover the thread column. They show 3.51.0's "Also needed to grant a task write access…" line and the HUB-J2 spec split (`out/J2-ident-J03-pair.png`, `out/J2-admin-J02-pair.png`, `out/J12-pm-J01-pair.png`).
  - HUB-J20's phone sheet (3.51.0).
  - HUB-J17's sideways sheet, which moves down about 10 px under the longer 3.51.0 permission text (`out/J17-844-J06-pair.png`).
  - HUB-J19's random sign-in code (`out/J19-J01-pair.png`).
  - Typing-dot phase (16–26 px blobs at x≈412–438) and caret blink.
  - **HUB-J11 J02, F01 below** (`out/J11-J02-pair.png`).

  No thread on any journey is scrolled differently from 3.50.0. That includes the HUB-J4 reading-position parts (cas-c2cb), HUB-J7's long-question jump, and HUB-J12's "Jump to latest" with a download chip.
- **final.aria.yml.** Of 131 common trees, 31 are identical. The rest differ only by the 3.51.0 changes the 3.51.0 report lists:
  - the lockup becoming `button "Cassy Cloud apps"` (99 trees);
  - the rail's per-group permission line (59);
  - Paired machines' new line (10);
  - the footer build id;
  - the double's random port;
  - HUB-J20's title, lead line and cleared form.

  Three other differences depend on timing or the spec:
  - HUB-J12 "a held message another tab delivers" catches `status: working`.
  - One HUB-J2 part ends on the empty-conversation screen (cas-f7c5 spec split).
  - HUB-J19's receipt age reads "17s" instead of "12s" (see Stage timings).

  No aria change comes from 7bface8f (`out/aria-diff.txt`).
- **HUB-J9, the changed journey.** I read every stage screenshot and every 2 fps screencast frame (`frames/J9/`, sheet `out/J9-frames-a.png`).
  - In "Scroll back through a long thread", the 14th turn arrives. 142 ms later, the spec sets `scrollTop = 0` (trace action 160). That falls inside the window in which the pre-fix handler dropped the scroll.
  - "Jump to latest" is visible 98 ms later (action 161), which is the action that failed in the original cas-7c94 flake.
  - The chip has its own row between the thread and the composer, under "Build step 7", and covers no turn (f036).
  - One tap returns to "Build step 14 of 14" with the chip gone (J06, f038).
  - J06 is pixel-identical to the 3.50.0 J06.

Limits of the evidence:

- This is one run at workers=4, and the flake was intermittent: 11/12 and 10/12 in the 3.48 reruns. The cas-6a9b status below therefore also rests on the delivery's recorded proof.
- No screen-reader audio was captured.

## Scores

Scores are 0–3 for each dimension: 0 none, 1 minor, 2 noticeable, 3 blocking. The rubric and severity routing are in `docs/qa/journey-evaluation.md`. The core journeys are HUB-J1, J2, J3, J5 and J6. The Run column is copied from the suite. PASS means the journey reached its goal.

| ID | Run | Dead end | Copy | Steps | Context | Waits | Severity | Findings / tasks |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| HUB-J1 | PASS | 0 | 0 | 0 | 0 | 0 | Note | — |
| HUB-J2 | PASS | 0 | 0 | 0 | 0 | 0 | Note | — |
| HUB-J3 | PASS | 0 | 0 | 0 | 0 | 0 | Note | N2 (carried) |
| HUB-J4 | PASS | 0 | 0 | 0 | 0 | 0 | Note | Reading-position parts unchanged |
| HUB-J5 | PASS | 0 | 0 | 0 | 0 | 0 | Note | — |
| HUB-J6 | PASS | 0 | 0 | 0 | 0 | 0 | Note | — |
| HUB-J7 | PASS | 0 | 0 | 0 | 0 | 0 | Note | — |
| HUB-J8 | PASS | 0 | 1 | 0 | 0 | 0 | Note | J8 note (carried) |
| HUB-J9 | PASS | 0 | 0 | 0 | 0 | 0 | Note | cas-6a9b resolved (see Previous findings) |
| HUB-J10 | PASS | 0 | 0 | 0 | 0 | 0 | Note | J10 note (carried) |
| HUB-J11 | PASS | 0 | 0 | 0 | 0 | 0 | Note | F01 cas-793b (suite evidence, no user friction) |
| HUB-J12 | PASS | 0 | 0 | 0 | 0 | 0 | Note | — |
| HUB-J13 | PASS | 0 | 1 | 0 | 0 | 0 | Note | J13 notes (carried) |
| HUB-J14 | PASS | 0 | 0 | 0 | 0 | 0 | Note | N2 (carried) |
| HUB-J15 | PASS | 0 | 0 | 0 | 0 | 0 | Note | — |
| HUB-J16 | PASS | 0 | 0 | 0 | 0 | 0 | Note | J16 notes (carried) |
| HUB-J17 | PASS | 0 | 1 | 0 | 0 | 0 | Normal | cas-ba64 (carried 3.51.0 F01) |
| HUB-J18 | PASS | 0 | 0 | 0 | 0 | 0 | Note | — |
| HUB-J19 | PASS | 0 | 1 | 0 | 0 | 1 | Note | Carried J19 notes; receipt waited one poll (5.4 s) |
| HUB-J20 | PASS | 0 | 1 | 0 | 0 | 0 | Normal | cas-ba64, cas-a8d9 (carried 3.51.0 F01, F03) |
| HUB-J21 | PASS | 0 | 0 | 1 | 0 | 0 | Normal | cas-2a26 (carried 3.51.0 F02) |

HUB-J9's Waits score drops from 1 in 3.51.0 to 0, because the flake behind it is fixed.

## Findings

This dist introduces one new finding. It is about the suite's evidence, not what an operator sees. The three 3.51.0 Normal findings are carried, because their fixes are not in this dist.

| Finding | Severity | What a user / evaluator sees | Receipt and fix | Task |
| --- | --- | --- | --- | --- |
| F01: HUB-J11's "The network drops" screenshot shows the connection already back | Note for the product (no user friction). Filed as P3 because the fix is concrete and the receipt misleads evaluators | HUB-J11, stage "The network drops". J02.png shows "· Live", "Connected", no outage banner, and "Are you there?" as "Sending…". The stage itself asserts the outage, and the screencast shows it correctly at about 0:10.5 (`frames/J11/f022.png`): the banner "Lost connection to Atlas · Linux. Reconnecting…", the header and row "Reconnecting", and the held message "Waiting for the connection — sends when it's back" with Cancel. 3.50.0's J02 caught the outage. | **Receipts:** HUB-J11 J02.png; `out/J11-J02-pair.png` (3.50.0 on the left, this run on the right); `frames/J11/f022.png`. **Cause:** `reconnect.journey.ts:70-139` ends the stage with `hub.release(PELICAN)`. The stage screenshot (`journey.ts:118-127`) is taken after that, so it races the double's reconnect. **Fix:** move the release to the start of "It reconnects on its own". | cas-793b |
| 3.51.0 F01 (carried): the permission copy promises "Add it in Paired machines", but Paired machines offers no way to add it there | Normal (HUB-J20 and HUB-J17 Copy 1) | Unchanged. The same rail text appears in 59 final trees, and Paired machines shows the same order and the disabled "Stop and restart". | **Receipts:** HUB-J7 J06 (rail, 1280); HUB-J9 J09 (Paired machines). **Fix:** as in the task. | cas-ba64 |
| 3.51.0 F02 (carried): the Cassy Cloud apps menu stays open after Explorer opens | Normal (HUB-J21 Steps 1) | Unchanged. The popover is still open in Commander after the switch, at 1280 and 390 dark. | **Receipts:** HUB-J21 J02 and J04; phone part J02 and J04. | cas-2a26 |
| 3.51.0 F03 (carried): at 1280×844, the Write access receipt's last line is in the rail's bottom fade | Normal (HUB-J20 Copy 1) | Unchanged. "closes." is drawn lighter than the receipt's other lines, and "WORKERS · 1" is under the fade. | **Receipts:** HUB-J20 J04; crop `out/J20-J04-fade.png`. | cas-a8d9 |

### Notes (report only)

- **HUB-J19's receipt waited one poll.** "soundwave returns and accepts the reply" took 6.8 s, against 1.6 s in 3.50.0. The trace shows that "soundwave received it" appeared at the second inbox poll, 5.4 s after the first (trace action 104; requests at 0:23.29 and 0:28.30). Throughout the wait, the row shows "Waiting for soundwave" with a status ring (J04), so progress is visible. This is the carried "receipt waits for one poll" note. In 3.50.0, the first poll happened to catch it. The aria age "17s" against "12s" follows from this.
- **HUB-J8's focus stage took 10.4 s, against 5.7 s.** The extra 4.4 s is between trace actions 113 and 114. In that gap the spec waits for the next catalog fetch (`switch-machines.journey.ts:132-134`) while Raw output keeps focus. This is the harness, not the UI.
- Carried over from 3.51.0 and still present:
  - the current app is marked by an outline only;
  - the machine line is hidden on a sideways phone (deliberate, cas-8526);
  - the empty band over the phone Write access title;
  - the folder path breaking at its hyphen;
  - N1, the footer build id;
  - N2, row text running together in J14;
  - own-message ticks (deliberate);
  - the J8 "Dismiss this notice" name;
  - the J10 one-frame high-contrast flash;
  - the J13 notes;
  - the J16 notes;
  - the J19 two-tap return after a reload;
  - "create+edit" in the Write access confirmation and receipt.

## Previous findings

The comparison is with `2026-10-14-hub-web-62930625.md` (3.51.0, also shipped as 3.51.1).

| Earlier finding | This dist's evidence |
| --- | --- |
| F01 (carried since 3.48.1, cas-6a9b): HUB-J9 "Jump to latest" sometimes doesn't appear after scrolling back through a long thread | **Resolved.** The fix (ee044f318, rebuilt in 354e260f0) is in the shipped bundle and not in v3.51.1's. In this run, the scroll lands 142 ms after the newest turn, inside the window the old handler dropped. "Jump to latest" appears 98 ms later, on its own row, and one tap returns to the newest turn (trace actions 159–167; J06; f036–f038). The delivery's recorded proof (cas-6a9b notes, 2026-10-11): a red-first vitest that failed before the fix; a throttled repro that failed 2/15 before the fix and passed 30/30 after; HUB-J9 ×20 at workers=4, 20/20; vitest 1304/1304; independent QA cas-fa06 passed. Those artifacts are no longer on disk, so this report relies on the task notes for them. |
| F01 (cas-ba64): permission copy | Still present. The fix is not in this dist. |
| F02 (cas-2a26): app switcher stays open | Still present. The fix is not in this dist. |
| F03 (cas-a8d9): receipt in the fade | Still present. The fix is not in this dist. |
| G01 phone identity (cas-d043, cas-dd8b) | Unchanged. |

## Stage timings

Across 416 stages shared with 3.50.0, the median stage took 1.01× its 3.50.0 time, and the total was 1.02× (`out/timing.txt`). HUB-J9 "Scroll back through a long thread" took 1.2 s, against 1.1 s. These are the slowest stages of this run. Every one shows progress throughout.

| Journey | Stage | Time | 3.50.0 | Observed feedback |
| --- | --- | --- | --- | --- |
| HUB-J8 | Every tap opens the conversation it pressed, while the list is changing (390, touch) | 78.1 s | 75.0 s (3.51.0) | 50 taps, each opening its conversation. The time is the harness. |
| HUB-J8 | The same, at 1280 by mouse | 73.2 s | 70.2 s (3.51.0) | The same |
| HUB-J12 | Re-pair says starting sessions won't come with a code (part) | 27.8 s | 24.7 s | A dialog walk with no unexplained wait |
| HUB-J12 | Two healthy machines survive another session's clean exit and its replay (part) | 27.5 s | 25.6 s | Both conversations stay Live |
| HUB-J3 | Reopen the palette to the full list | 26.7 s | 26.0 s | Repeated palette and filter actions |
| HUB-J12 | A hundred machine events arrive in one second | 22.4 s | 25.2 s | No flicker |
| HUB-J12 | Past the two-minute hold, Not sent with Retry | 18.0 s | 17.7 s | A deliberate clock advance |
| HUB-J12 | Three unconfirmed messages read as one notice | 17.6 s | 17.1 s | A deliberate clock advance |
| HUB-J2 | M01 five repairs update one browser installation (part) | 14.9 s | 15.5 s (3.51.0) | Each repair settles visibly |
| HUB-J8 | Keyboard focus lands somewhere real on every route | 10.4 s | 5.7 s | The spec waits for a catalog fetch; focus stays on Raw output (see Notes) |
| HUB-J19 | soundwave returns and accepts the reply; a reload keeps everything | 6.8 s | 1.6 s | "Waiting for soundwave" with a status ring until the next poll (see Notes) |
| HUB-J20 | Review, confirm and see the receipt (1280) | 6.5 s | 3.1 s | 3.51.0's new assertions, as in the 3.51.0 report |

## Not covered

The protocol-double limits are unchanged from `2026-10-06-hub-web-9fade21f.md` § Not covered. The 3.51.0 gaps also still apply:

- the Explorer stub (HUB-J21);
- cas-42c0's 403 refusal;
- cas-ab04's hub side;
- the emulated notch (HUB-J9 sideways).

For this release's one delivery:

- **cas-6a9b, the scroll fix.** The suite drives the reader's scroll with a scripted `scrollTop = 0` plus a scroll event. It does not use a real touch fling, momentum scroll or a real on-screen keyboard resize (HUB-J9 Gaps). Its claim that a keyboard shrink or a clamp keeps the tail rests on the delivery's third vitest case, not on a journey. That case was not re-run for this report: a local unit run was held behind the host's memory admission while the supervisor's assembly proof ran.
- **The rest of cas-baa3** (release latency, the release train, worktree and CHANGELOG tooling) changes no hub-web source, so no journey shows it.

A real hub's reconnect timing and a real-device Glass frame time remain unmeasured.
