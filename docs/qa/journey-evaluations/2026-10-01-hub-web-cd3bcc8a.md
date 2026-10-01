# Journey evaluation — hub-web/dist cd3bcc8a

**All 14 catalog journeys PASS, and nothing blocks the cut.** The only UI change
in this bundle is cas-55a4: a session's Commander thread is now its own
conversation. Other sessions' turns sit in collapsed, dated "Earlier session"
sections. A project's live sessions are grouped in the list, and End session is
offered there. The change works as the catalog describes (HUB-J14, and HUB-J7
stage 2). It also fixes the HUB-J7 half of F06: the ended session's question is
no longer read out as the live session's.

Its rough edges are new Normal findings, all filed at P2:

- **F19, End session.** Pressing End session empties the whole conversation list
  for about 4.4 s before the confirmation appears.
- **F20, row times.** Idle rows read "now" forever, while the row just used
  ticks in seconds.
- **F21, empty thread.** The empty-thread card says "Commander" and ignores the
  connection state.
- **F23, grouped rows.** Grouped rows are hard to tell apart.

F19 is the known cas-d6bf; this run adds that the list itself goes blank. None
of these scores 3, and none sits on a core journey at 2, so none is Blocking or
High. F19 is the one I would fix first (see the Verdict).

## Receipt

- hub_web_dist: cd3bcc8a8c579f875a0bd0cb0c85f3a4d53b646e
- evaluated_commit: 3e00baf343c407fb56d6ed21b820f3461aae8c09
- suite_run: /home/pippenz/.cas/artifacts/cas-src-5afb8f4cc4d727d9ea2d1ba8381fb0af925ef67130c95e75e7dc9720f98d54a7/cas-259d — 14 journeys, 14 PASS
- evaluator: vivid-shark-47 (taste lane; not an implementer of release epics cas-555a or cas-c4d3)
- label: real-bundle, protocol-double
- blocking_findings: 0

### Evidence reviewed

- **Suite.** `scripts/journey-eval.sh` ran on a detached checkout of
  3e00baf34, with `npm ci` and Playwright 1.63.0. 24 Playwright tests passed,
  exit 0. Every `result.json` has empty `page_errors` and `frame_defects`.
- **Screenshots and results.** All 108 stage screenshots, all 14 `result.json`
  and all 14 `final.aria.yml`.
- **Screencasts.** Every screencast was extracted at 2 fps, 1,487 frames in all.
  Frames that changed from the previous kept frame were tiled into 196 contact
  sheets labelled with video time (`eval/sheets/`). Full frames were read where
  a finding depended on them.
- **HUB-J12.** The bundle still has only stages 1–2 (F17). Stages 3–9 were
  judged from per-action screenshots unpacked from the 11
  `playwright/network-switch*/trace.zip` traces (`eval/j12/`).
- **HUB-J14.** Read from all 11 sheets, its frames and trace. F19 was confirmed
  in the trace's aria snapshots and network log.
- **HUB-J4.** Re-run twice in isolation (`rerun-j4-1/`, `rerun-j4-2/`) to test
  F22.
- **Baseline.** The baseline is the 5cb4898c run, `cas-703c`, and its report
  `2026-10-01-hub-web-5cb4898c.md`.
- **How the review was split.** The review ran in five parallel read-only lanes
  of this one evaluator, `eval/review-{A..E}.md`. HUB-J7 and HUB-J14 were
  reviewed directly.
- **Limits.**
  - Sampling at 2 fps can miss a defect shorter than 0.5 s.
  - Stage times are about 1.5–2.3× the baseline's on every stage. The slowdown
    is uniform, which points to host load, so it is not scored as a wait.

### Release change verified (cas-55a4)

- **A session's own thread.**
  - HUB-J14 `J02.png`: calm-puma-34 opens to "No Commander messages from this
    session yet" with "Last activity 2m ago" and Open Terminal. Below it sit
    "Earlier session noble-cheetah-84, Yesterday · 2 messages" and "Earlier
    messages with no session recorded, Yesterday · 1 message".
  - HUB-J14 `@00:12.5–00:14.0`: the section opens to dated turns ("You · Pixel
    10 Yesterday 21:30", "Supervisor Yesterday 21:41") with no actions.
  - HUB-J14 `J05.png`: noble-cheetah-84 shows only its own turns, and
    wild-shark-68's turns sit in "Earlier session wild-shark-68, Today".
- **Ended session's question.** HUB-J7 `J02.png` and
  `@00:10.5–00:11.0`: the ended session's question sits only in the collapsed
  "Earlier session patient-pelican-8" section, with no choices. Nothing is
  pinned, and the rail lists only the live blocker. `final.aria.yml` no longer
  names the live session as the asker of that question.
- **Grouped list.**
  - HUB-J14 `J01.png` reads "gabber-studio · 3 sessions on Atlas", with
    "Most recent" on calm-puma-34 and per-row times "2m", "40m" and "14h".
  - HUB-J14 `@00:33.5`: End session asks "End noble-cheetah-84 on Atlas? Its
    supervisor and workers stop." with End session and Cancel.
  - HUB-J14 `J06.png`: after the confirmation, the row leaves and the heading
    reads "2 sessions on Atlas".
- **No hidden history elsewhere.** In every other journey (one session per
  project) the earlier-sessions region stays empty and hidden. No turn that the
  baseline showed is missing (lanes A–E).

## Scores

Scores are 0–3 for each dimension: 0 none, 1 minor, 2 noticeable, 3 blocking.
The rubric and severity routing are in `docs/qa/journey-evaluation.md`. The
Run column is copied from the suite. PASS means the journey reached its goal.

The core journeys for routing are pairing, finding a conversation and replying:
HUB-J1, J2, J3, J5 and J6. None of them scores a 2, so there is no High
finding. The 2s on J4, J8, J12 and J14 route as Normal.

| ID | Run | Dead end | Copy | Steps | Context | Waits | Severity | Findings / tasks |
|---|---|---|---|---|---|---|---|---|
| HUB-J1 | PASS | 0 | 1 | 0 | 0 | 0 | Normal | F05 cas-865c; F10 cas-813a; F21 cas-010f; N1 |
| HUB-J2 | PASS | 0 | 1 | 0 | 0 | 0 | Normal | F05 cas-865c; F21 cas-010f; F26 cas-b52d; N1 |
| HUB-J3 | PASS | 0 | 1 | 1 | 1 | 1 | Normal | F05 cas-865c; F08 cas-537f; F09 cas-766c; F10 cas-813a; F20 cas-6acf; F21 cas-010f |
| HUB-J4 | PASS | 0 | 1 | 1 | 2 | 0 | Normal | F01 cas-2093; F04 cas-5a8f; F08 cas-537f; F22 cas-acb4b |
| HUB-J5 | PASS | 0 | 1 | 0 | 1 | 1 | Normal | F06 cas-8d52; F07 cas-71f4; F09 cas-766c; F10 cas-813a; F11 cas-b00c; F20 cas-6acf; F21 cas-010f |
| HUB-J6 | PASS | 0 | 1 | 1 | 0 | 0 | Normal | F05 cas-865c; F07 cas-71f4; F21 cas-010f |
| HUB-J7 | PASS | 0 | 1 | 0 | 0 | 0 | Normal | F07 cas-71f4; F10 cas-813a; F27 cas-e829 |
| HUB-J8 | PASS | 0 | 2 | 1 | 2 | 0 | Normal | F03 cas-7b31/cas-a6f0; F07 cas-71f4; F08 cas-537f; F09 cas-766c; F10 cas-813a; F13 cas-cae2/cas-d141; F14 cas-bad9; F15 cas-0cd1; F20 cas-6acf; F23 cas-5d2c; F25 cas-7752 |
| HUB-J9 | PASS | 0 | 1 | 1 | 1 | 0 | Normal | F05 cas-865c; F07 cas-71f4; F09 cas-766c; F10 cas-813a; F12 cas-0739; F20 cas-6acf; F21 cas-010f |
| HUB-J10 | PASS | 0 | 1 | 0 | 1 | 0 | Normal | F05 cas-865c; F07 cas-71f4; F10 cas-813a; F20 cas-6acf; F21 cas-010f; N2 |
| HUB-J11 | PASS | 0 | 1 | 0 | 0 | 0 | Normal | F01 cas-2093; F04 cas-5a8f; F09 cas-766c; F21 cas-010f; F24 cas-eb4b |
| HUB-J12 | PASS | 0 | 2 | 0 | 1 | 1 | Normal | F02 cas-a6f0; F03 cas-7b31; F04 cas-5a8f; F09 cas-766c; F12 cas-0739; F16 cas-0e14; F17 cas-1f7e; F21 cas-010f |
| HUB-J13 | PASS | 0 | 1 | 1 | 0 | 0 | Normal | F05 cas-865c; F13 cas-cae2; F18 cas-cee5; F21 cas-010f |
| HUB-J14 | PASS | 0 | 1 | 0 | 2 | 2 | Normal | F04 cas-5a8f; F19 cas-d6bf; F20 cas-6acf; F21 cas-010f; F23 cas-5d2c |

Before filing, each finding was checked against the open task list. F01–F18 keep
the baseline report's numbers and tasks. F19 reuses cas-d6bf, and F27 reuses
cas-e829, which is in flight. F20–F26 are new P2 tasks.

### Verdict for the release decision

`blocking_findings: 0`. F19 (cas-d6bf) does not block under the rubric:

- It is Waits 2 and Context 2 on HUB-J14, which is not a core journey.
- The list returns by itself at the next 5 s catalog poll.
- Nothing ends without the confirmation.

It is still this release's most visible regression: a click on End session makes
every conversation disappear for about 4 s. I recommend landing cas-d6bf soon
after 3.43.1, or before the cut if it is cheap.

If catalog polling stops while the confirmation is pending, for example when
the machine drops, the list may stay empty. That was not observed and is not
scored.

## Findings

### F19 — Normal — End session empties the whole conversation list for about 4.4 s before asking (HUB-J14)

- **Journey/stage:** HUB-J14, End a stale session.
- **Receipts:**
  - `receipt.webm @00:29.0–00:33.0` (frames f059–f066): the list area is blank.
    Its pixel standard deviation is 0.1, against 38 on neighbouring frames. The
    footer still says "3 conversations", and the open thread stays on screen.
  - In the trace aria, navigation "Choose a supervisor" has 7 children at
    call@196-before (the End session click). It has **0** at call@196-after and
    call@198-before, and 9 at call@198-after. call@198, the wait for the
    confirmation, took about 4.4 s.
  - The network log shows the list returning just after the next
    `GET /v1/sessions` poll, at 21:15:30.717.
  - After the confirmation, `J06.png` closes the open conversation and returns
    to the landing screen.
- **Experience:** one click on a session's End session makes every conversation
  vanish without any progress cue. A user will think the sessions were ended,
  or the app broke. The confirmation appears seconds later.
- **Likely cause (not proven):** `ConversationList.renderEnd`
  (`hub-web/src/conversation-list.ts:197-226`) caches each control by
  signature. Its `rerender` closure therefore keeps the container and rows of
  the render that built it. Re-rendering into a stale container moves the rows
  out of the live list until `renderConversationList` runs again.
- **Fix:**
  - Route the End controls' state changes through `renderConversationList`.
  - Show the confirmation in under 200 ms with focus on Cancel. cas-d6bf
    already covers that, plus clipped buttons and phone crowding.
- **task: cas-d6bf (existing P2).** A discovery note with this evidence was
  added. Waits 2 and Context 2 on HUB-J14.

### F20 — Normal — Row times mix activity with the catalog-check time, so idle rows look newest

- **Journey/stage:** HUB-J3 from Notice a new reply while away; HUB-J5; HUB-J8;
  HUB-J9; HUB-J10; HUB-J14 stage 1.
- **Receipts:**
  - HUB-J3 `J03`–`J11.png`: cas-src, which just got a reply, reads "12s… 51s…
    1m", while lighthouse, with no activity, reads "now" throughout.
    gabber-studio flips "5s" → "now" at `@00:14.0–00:14.5`.
  - HUB-J5 `J15.png`: "5s" against "now" on two idle rows. `final.aria.yml:11-17`
    matches.
  - HUB-J10 `final.aria.yml`: the row's spoken name contains "20s". The
    baseline read "now".
  - HUB-J8 `f293`: two grouped rows both read "now", with title "Catalog
    checked now", and neither is marked Most recent.
- **Cause:** `hub-web/src/main.ts` `renderConversationList` (about :3378). With
  no session activity, `when` falls back to the catalog `updated` time, and the
  5 s poll keeps that at "now".
- **Experience:**
  - In the journey that asks "which conversation has something new" (HUB-J3),
    the time column points at the wrong rows.
  - The time ticks by the second.
  - Screen readers say "20s".
- **Fix:**
  - Show a time only for real activity.
  - Read "now" under a minute.
  - Never move a time backwards without new activity.
  - Speak the time as words.
- **task: cas-6acf (new P2).** It was introduced by cas-55a4 and overlaps F08's
  row-time part. Copy 1 or Context 1 where listed.

### F21 — Normal — The empty-thread card: "Commander", a second Terminal label, no connection state, a false flash

- **Journey/stage:** every empty thread (HUB-J1, J2, J3, J5, J6, J9, J10, J11,
  J12, J13 and J14) and HUB-J10's open.
- **Receipts:**
  - **The sentence.** HUB-J1 `J05.png` and HUB-J5 `J01.png` read "No Commander
    messages from this session yet. The supervisor (patient-pelican-9) will
    write here when it needs a decision." The source is
    `conversation-view.ts:716`. "Commander" appears nowhere else on screen,
    and the codename appears three times on the card.
  - **Two Terminal labels.** "Open Terminal" (`main.ts:408`) clicks the
    header's "Terminal view". HUB-J13 `J05.png` also shows a third label,
    "Terminal".
  - **No connection state.** Under Needs pairing (`eval/j12/3b88a-call2319-after.png`),
    Degraded (`60a1e` call@347–368) and Reconnecting (HUB-J11 `@00:10.0`), the
    card still promises the supervisor "will write here" and offers Open
    Terminal.
  - **A false flash.** HUB-J10 `f010` (`@00:04.5`): while a conversation with
    history is opening, the card claims no messages for about 0.5 s before the
    first turn lands.
  - **Contradictory activity.** HUB-J14 `J02.png` reads "Last activity 2m ago ·
    supervisor → bright-robin-85". For the same session, Terminal view's pane
    header says "No activity …" (`f037`).
- **Experience:**
  - A non-engineer has to decode "Commander messages".
  - The card's main button sends a first-time user away from the focused
    composer.
  - While disconnected, the card makes a promise the banner contradicts.
- **Fix:**
  - Use plain copy ("No messages from the cas-src supervisor in this session
    yet — nothing is waiting on you").
  - Use one Terminal label, as a quieter link.
  - Make the text state-aware when the session is not live.
  - Render the card only after this session's first history page has resolved
    empty.
  - Make the activity wording agree with the pane header.
- **task: cas-010f (new P2).** It was introduced by cas-55a4. Copy 1.

### F22 — Normal — HUB-J4 opened three exchanges above the latest turn in 1 of 3 runs

- **Journey/stage:** HUB-J4, Open the conversation and see the recent turns.
- **Receipts:**
  - Full-suite run, `J01.png` and `frames/HUB-J4/f011` (`@00:05.0`, unchanged
    through `@00:10.0`): the view sits on "Today · Is the release ready to
    cut?" with the file cards cut off, and Jump to latest is showing. The
    latest exchange is off screen.
  - Two isolated reruns (`rerun-j4-1`, `rerun-j4-2`) and baseline cas-703c
    opened at the tail.
- **Experience:** the user lands away from where the conversation ended, which
  breaks catalog step 1. The suite passed because stage 1 does not assert the
  newest turn.
- **Cause:** not traced. It depends on load or timing. Suspects are late layout
  growth from the file cards, or the earlier-sessions node moving during
  hydration.
- **Fix:** keep following the tail while the thread content resizes, and assert
  the newest turn is visible in J4 stage 1.
- **task: cas-acb4b (new P2).** Steps 1, plus Context 2 on HUB-J4, shared with
  F01.

### F23 — Normal — Grouped session rows are hard to tell apart, and the opened row scrolls out of view

- **Journey/stage:** HUB-J14, See a project's live sessions together and Each
  session shows its own conversation; HUB-J8, the list at 1280 px.
- **Receipts:**
  - HUB-J14 `J01.png`: every row repeats "gabber-studio · Atlas" under the
    heading. Each reads "Live" until it has been opened once: wild-shark-68
    becomes "Stem export is at 60%." only after its visit (`@00:22.5`).
  - HUB-J14 `J01.png`: rows are about 138 px tall with their End session
    lines, so the third session is cut off at 720 px.
  - HUB-J14 `@00:24.0` → `@00:25.0`: opening noble-cheetah-84, low in the list,
    re-renders the list at the top, and the selected row drops below the fold
    (`J05.png`).
  - The heading says "3 sessions" while the footer says "3 conversations".
    HUB-J8 `f293` shows a weak grey heading and the open row below the fold.
- **Fix:**
  - Lead each grouped row with what distinguishes it, using the catalog's
    last activity.
  - Use one noun for heading and footer.
  - Keep the list's scroll position, and scroll the selected row into view.
- **task: cas-5d2c (new P2).** It was introduced by cas-55a4. Context 2 on
  HUB-J14 (with F19), and Copy 1.

### F24 — Normal — The HUB-J11 order assertion is now vacuous, and the catalog step is stale (suite integrity)

- **Receipts:**
  - d27dfc65d removed the "session … started" line from a session's own thread.
  - HUB-J11 `J02.png` and `J05.png` show no such line, against the baseline.
  - `reconnect.journey.ts:113` asserts `findIndex(session … started) <
    findIndex("You, ")`. Its left side is now -1, so the assertion always
    passes.
  - Catalog HUB-J11 step 5 still says "the message stays below its session
    line".
- **Experience:** none directly. The suite claims a protection it no longer
  gives. The whole-thread `toEqual` comparison still holds the order.
- **Fix:** anchor the assertion on the day line, and update the catalog.
- **task: cas-eb4b (new P2).** Not a friction score.

### F25 — Normal — A draft is lost silently on reload or same-tab navigation (HUB-J8; older than this release)

- **Receipts:**
  - Sheet s22 `@01:16.5`: the cas-src composer holds "Draft: ask about the
    flaky pairing test".
  - Stage 10 opens a `#pair=` link and reloads (trace actions 379, 380 and
    394).
  - `f177` `@01:28.0`: the draft is gone, and no conversation is open
    (`J10.png`).
- **Cause:** drafts live only in memory (`main.ts:2152`, `conversationDrafts`).
- **Experience:** the journey's own goal, "my draft is still there when I
  return", fails on an accidental refresh or a tab kill. The baseline behaves
  the same but did not score it.
- **Fix:** persist drafts per thread in sessionStorage, and assert in HUB-J8
  that a draft survives a reload.
- **task: cas-7752 (new P2).** Context 2 on HUB-J8.

### F26 — Normal — A read-only pair link hides the withheld scopes below the dialog fold (HUB-J2; older than this release)

- **Receipts:** `frames/HUB-J2/f012` (`@00:04.0–00:08.0`). Opening Technical
  details shows only the origin. The "not granted by this invitation" scope
  boxes are asserted at trace 0:07.239–0:08.522 but appear in no frame.
- **Experience:** a user asking why they can only read never sees what was
  withheld, or the command that grants it.
- **Fix:** scroll the details into view, and state the withheld abilities in
  plain words, with the command and Copy.
- **task: cas-b52d (new P2).** Copy 1.

### F27 — Normal — A blocker reads "Acknowledged — you replied" after the user answered a different question (HUB-J7)

- **Receipts:** HUB-J7 `J04.png` and `final.aria.yml`. The question was
  answered with "Fix in-train". The earlier blocker "The release gate went red"
  then turns "Acknowledged — you replied", although no reply was bound to it.
- **task: cas-e829 (in flight, P1 QA cas-b043).** This dist does not include it.
  Copy 1.

### Baseline findings (F01–F18): status in this bundle

Each line gives the receipt that still reproduces the finding in this run, or
what changed.

- **F01 cas-2093 — still reproduces.** HUB-J4 `@00:32.5`: a reconnect throws
  the reader back to "Mon, Sep 28", and "Load earlier" returns. HUB-J11
  `J02.png`: the banner covers "No earlier history".
- **F02 cas-a6f0 — still reproduces.** HUB-J12 `60a1e` call@347–368: "Degraded"
  against "Reconnecting" against "Not live — reconnecting". `3b88a` call@2319:
  "Authentication blocked" against "needs pairing again".
- **F03 cas-7b31 — still reproduces, partly improved.**
  - HUB-J8 s27–s28 `@01:50.5–01:51.0`: "Stuck dialing atlas.test — node may be
    offline: 4 consecutive heartbeats missed".
  - `J11.png`: the terminal comes back as OBSERVER.
  - Improvement: the "Control released" toast now clears about 2 s after
    recovery on HUB-J8.
  - Regression: on HUB-J12 the toast now covers thread content (`e8a72`
    call@598, `3b88a` call@2319).
- **F04 cas-5a8f — still reproduces.** HUB-J11 `J02.png`/`J04.png`, and HUB-J4
  `final.aria.yml` ends `status: working`. HUB-J14 `J05.png`: a session idle
  since yesterday 21:41 also shows "working".
- **F05 cas-865c — still reproduces.** HUB-J1 `J04.png`, HUB-J3 `J01.png` and
  HUB-J13 `@00:05.0`/`@00:06.5`, which shows two confirmations.
- **F06 cas-8d52 — the HUB-J7 half is fixed; the HUB-J5 half still
  reproduces.** HUB-J7 `final.aria.yml` no longer attributes the ended
  session's question to patient-pelican-9. HUB-J5 `final.aria.yml` still
  reorders the crossed turn after a reload and drops its Delivered ticks
  (`J17.png`).
- **F07 cas-71f4 — still reproduces.** HUB-J5 `J04.png` reads "Sending to
  patient-pelican-9…", and HUB-J7 `J04.png` says "Sending" three ways. The final
  ARIA has `button "Send to <codename>"`. In HUB-J6 `J02.png` the transcript
  lands without focus.
- **F08 cas-537f — still reproduces, with the row-time part changed.** HUB-J3
  `@00:23.0`: the filter sticks after a result is opened. `@00:41.5`: the Enter
  target is unmarked. An opened row now shows its last turn's time (HUB-J4
  "11h"). The new row-time defect is F20.
- **F09 cas-766c — still reproduces.**
  - HUB-J3 `J12.png` is pixel-identical to the baseline.
  - HUB-J5 `J15.png`, HUB-J9 `J08.png` and HUB-J11 `f062` show the phone
    header during an outage.
  - Also here: the phone Terminal header names no machine and spends space on
    "Ctrl K" (HUB-J8 `f160`).
- **F10 cas-813a — still reproduces.**
  - HUB-J3 `@01:23.0–01:25.0` and HUB-J7 `@00:05.0–00:05.5` show the two
    loading looks back to back.
  - The large loading card now also shows on HUB-J10's open and reload.
  - After the HUB-J10 reload, the Tasks column pops in late and shifts Send by
    about 190 px (`f032` → `@00:16.0`).
- **F11 cas-b00c — still reproduces.** HUB-J5 `@00:39.0`: Retry is enabled
  while Studio iPad holds control. `@00:47.0`: the instruction appears twice.
- **F12 cas-0739 — still reproduces.** HUB-J9 `J01.png` reads "6 paired
  machines · 5 connected". HUB-J12 `3b88a` call@2319 reads "2 paired machines ·
  1 connected".
- **F13 cas-cae2/cas-d141 — still reproduces; the phone palette title is
  fixed.**
  - HUB-J8 `J12.png`/`J14.png` read "Fleet overvi…" and "cas… · …ter-5 · Att",
    and two rail buttons read "AT".
  - HUB-J9 `f042` shows "Commands" whole.
  - Two new instances belong here:
    - HUB-J13 `f094`: a long machine name prints over "Connected" in the phone
      footer (`styles.css:3748` has `nowrap` with no ellipsis).
    - HUB-J8 `f268`: at 900 px with the drawer open, the Fleet view has no
      title and its column key reads "1 2 345".
- **F14 cas-bad9 — still reproduces.** HUB-J8 `f295`: the phone drawer is cut
  off by the Attention panel.
- **F15 cas-0cd1 — still reproduces.** HUB-J8 `final.aria.yml` reads "Last
  event 9/30/2026, 12:01:50 PM".
- **F16 cas-0e14 — still reproduces.** HUB-J12 `3b88a` call@2341: Re-pair
  opens the generic "Pair a machine".
- **F17 cas-1f7e — still reproduces.** `journeys/HUB-J12/` has only `J01.png`
  and `J02.png`.
- **F18 cas-cee5 — still reproduces.** HUB-J13 `J03.png` shows the Supervisor
  hint. `@00:08.0`: Workers looks prefilled with "0". `@00:38.0`: the login
  command is in prose with no Copy. `@00:25.5`: "Waiting for bright-heron-21 to
  come up".

### Notes — report only

- **N1, HUB-J1 and J2.** The footer shows "Hub 9fa863b2" and screen readers read
  it out. It is long-standing.
- **N2, HUB-J10 `f021` (`@00:10.0`).** Switching to dark paints one frame with
  Send washed out on its lilac button, milder than before. It settles by
  `@00:10.5`.
- **No session start time.** Since cas-55a4, a session's own thread no longer
  shows when the session started (HUB-J9 `J07.png`, HUB-J11 `J02.png`). This
  was deliberate; carrying "started 12:00" in the header would restore it.
- **Earlier section loses its explanation.** The ended session's unanswered
  question in HUB-J7's earlier section is plain text, with no "No longer
  waiting" line (`@00:10.5`). The section title implies it.
- **Not-paired flash at start.** Each journey's first paint briefly shows "0
  paired machines · Not paired" (HUB-J14 `@00:01.0`). The page is loaded twice
  by the fixture, and the first load is genuinely unpaired, so it is not
  scored.

### Fixed or improved since the previous report

- **cas-55a4.** The ended session's question no longer reads as the live
  session's (F06, HUB-J7 half).
- **F13, phone palette.** The phone palette opens with its title whole (HUB-J9
  `f042`).
- **F03, HUB-J8 toast.** The "Control released" toast clears about 2 s after
  recovery.
- **HUB-J3 first-load flash.** The footer's "1 connected" flash is no longer
  seen at 2 fps.

## Stage timings

These are the measured stage wall times from this run. On every stage they are
about 1.5–2.3× the baseline's. The slowdown is uniform across trace actions
(for example, the same `Evaluate` took 463 ms against 239 ms), which points to
host load, so none is scored as a wait. Annotation overhead adds about 0.3 s per
action.

| Journey | Stage | Time | Evaluation |
|---|---|---|---|
| HUB-J8 | Know each session and machine by name in Terminal view | 38.2 s | An injected heartbeat loss; the banner shows progress; friction is F03 |
| HUB-J8 | Reopen the session picker after closing it | 21.7 s | Repeated open, close and filter cycles; each response is prompt |
| HUB-J8 | A supervisor with no workers yet is listed everywhere | 14.9 s | Many surface checks |
| HUB-J8 | Keep my place in the session picker while updates arrive | 11.6 s | Deliberate updates; focus is kept |
| HUB-J4 | Open a report the supervisor sent | 11.1 s | Includes the injected reconnect (F01) |
| HUB-J3 | Find the conversation through the list search | 10.6 s | Five to eight typed filters |
| HUB-J5 | Focus on the opening card moves into the conversation | 9.0 s | Two deliberately slowed opens with a static "Opening" card (F10) |
| HUB-J11 | In Terminal view, nothing claims all clear or live during an outage | 9.0 s | Injected outage; every surface recovers |
| HUB-J14 | End a stale session | 6.7 s | **About 4.4 s of this is a blank list with no progress cue (F19), scored Waits 2** |

HUB-J12 has stage timings only for stages 1–2 (3.0 s and 3.4 s; F17). Its other
tests run on an injected protocol clock, which shows order and state, not wall
time.

## Not covered

| Journey / gap | Evidence available for this release |
|---|---|
| HUB-J14: the daemon's session-bound history page, catalog last activity and the hub's End session | Doubled here. cas-55a4's notes record real-data SQL checks of the session-scoped history (calm-puma-34 has 0 own rows; noble-cheetah-84 has 17). They also record the hub's 403 test. That **End session actually stops the daemon on a live machine** was not executed for this candidate (cas-55a4 note, 2026-10-01 20:12), and neither was the phone check against real rows after install. |
| HUB-J1 relay and machine-side `cas hub authorize`; HUB-J2 exchange | Doubled. No fresh real pairing proof for this candidate. The release does not touch pairing. |
| HUB-J4 real history pages and artifact hosting | Doubled. `hub-web/scripts/conversation-history-qa.mjs` was not run for this evaluation. |
| HUB-J5 daemon delivery and operator stamping | Doubled. No real-build delivery proof for this candidate. |
| HUB-J6 real microphone and recognizer | Stubbed. Focus and review UI are observable; audio and permissions are not. |
| HUB-J9 real on-screen keyboard | Not emulated. |
| HUB-J11/J12 real radio switch, Tailscale toggle, laptop sleep | Simulated on an injected clock. HUB-J12 stages 3–9 were judged from trace screenshots (F17). |
| HUB-J13 catalog, folder browse, accounts, survival across a hub restart | Doubled. The release does not touch launch. |
| HUB-J3/J7/J8/J10 | No external gaps in the catalog. Screen-reader naming was judged from ARIA snapshots, not with a real screen reader. |

The protocol-double PASS does not discharge the real-build gaps above. Earlier
reports' open findings that are not listed here are neither closed nor waived
by this report.
