# User-journey catalog

Critical end-to-end flows, one section per user-facing surface. Each journey
goes from a real entry point to a user goal and crosses feature boundaries
on purpose. The format, the suite and the release-time evaluation are
described in [journey-evaluation.md](journey-evaluation.md).

Machine-read by `scripts/journeys-for-diff.py`; validate edits with
`scripts/journeys-for-diff.py --check`. Keep each field on one line.
In **Steps**, the text before ` — ` must match the suite's stage
(`test.step`) title exactly.

## hub-web

Cassy Commander, the browser app that `cas hub` serves at `/commander/`.
Suite: `hub-web/e2e/journeys/`. Run it with `npm run journeys` in
`hub-web/`, or with `scripts/journey-eval.sh <dir>` to collect receipts.
The journeys project uses UTC and an advancing browser clock starting at
`2026-09-30T12:00:00Z`. Node protocol fixtures share that injected clock;
calendar-day fixtures use `journeyDay()` rather than subtracting hours.
`npm run journeys` checks for ambient `Date.now()`/`new Date()` in journey
sources before running the suite. Use `performance.now()` for elapsed time.
To exercise early-morning boundaries, run with
`HUB_JOURNEY_NOW=2026-09-30T00:30:00Z` or `2026-09-30T05:59:00Z`.
The protocol-double journeys also watch each animation frame and fails if an open
conversation shows the terminal canvas or sits on a bare panel for more than
250 ms (`frame_defects` in `result.json`).

- **Surface-wide:** `hub-web/src/main.ts`, `hub-web/src/styles.css`, `hub-web/src/types.ts`, `hub-web/src/terminal*`, `hub-web/src/terminal/*`, `hub-web/index.html`, `hub-web/package-lock.json`, `hub-web/vite.config.ts`, `hub-web/dist/*`, `hub-web/e2e/journeys/hub-double.ts`, `hub-web/e2e/journeys/journey.ts`, `hub-web/e2e/journeys/clock.ts`, `hub-web/e2e/journeys/world.ts`, `hub-web/playwright.config.ts`

### HUB-J1 · First open and pair a machine with a code

- **Entry:** `/commander/` in a browser that has never paired a machine
- **Goal:** my machine's supervisor appears and I can open its conversation
- **Touches:** `hub-web/src/pair*.ts`, `hub-web/src/pending-pairing.ts`, `hub-web/src/storage.ts`, `hub-web/src/first-connection.ts`, `hub-web/src/conversation-shell.ts`, `hub-web/src/paired-machines.ts`, `hub-web/src/connection.ts`, `hub-web/src/dpop.ts`
- **Suite:** `hub-web/e2e/journeys/pair-code.journey.ts`
- **Gaps:** the relay and the machine-side `cas hub authorize` are doubled; real pairing is proven by `docs/design/hub-web/pairing-verification.md`

#### Steps

1. Open Cassy Commander for the first time — the empty state explains what to do
2. Ask for a pairing code — "Pair a machine", then "Create pairing code" shows `cas hub authorize <code>`
3. Approve on the machine — the dialog follows the machine: waiting, claimed, authorized
4. Confirm and pair this browser — enter the operator label, then press Pair
5. See the machine's supervisor ready to talk to — a toast says the machine is connected without covering the composer or any heading, and its row opens a conversation that says, in plain words, "No messages from the cas-src supervisor in this session yet — nothing is waiting on you"; the card offers no other view, and the header carries Raw output and "Interrupt the cas-src supervisor"

#### Expected experience

- One obvious "Pair a machine" action on the empty screen, at every width.
- The code and the exact command to run are shown together; copying the command works.
- While waiting, the status says what the machine is doing, with a visible countdown.
- The confirm step names the machine and the scopes in plain words before anything is stored.
- After pairing, the user lands on a list with the supervisor in it. No reload and no second step.

#### Edge paths

- The relay is unreachable: "The pairing service is unavailable."
- The code expires: "This pairing request has expired." A fresh code must be one action away.
- The approved machine is not reachable from this device: the Tailscale / DNS guidance appears.
- The machine refuses the exchange (401/403): the dialog returns to the create step with advice.
- Reloading while waiting resumes the same code (sessionStorage).

### HUB-J2 · Pair a machine from a cas hub pair link

- **Entry:** the `#pair=…&hub=…&scopes=…` link printed by `cas hub pair`, opened in a browser
- **Goal:** the link pairs this browser and I reach the supervisor
- **Touches:** `hub-web/src/fragment.ts`, `hub-web/src/pair*.ts`, `hub-web/src/pending-pairing.ts`, `hub-web/src/storage.ts`, `hub-web/src/connection.ts`
- **Suite:** `hub-web/e2e/journeys/pair-link.journey.ts`
- **Gaps:** the exchange endpoint is doubled

#### Steps

1. Open the link the machine printed — the dialog opens by itself and the secret leaves the address bar
2. Confirm the machine — the hub address and machine name arrive filled in from the link; only your name is left, with the focus ring on it
3. Check the technical details — the origin and the granted scope boxes, in sentence case
4. Pair — the exchange goes to the link's machine
5. Reach the supervisor — the machine's supervisor is listed and opens

#### Expected experience

- The invitation is recognised at once, with no extra click to start.
- Scopes the link did not grant are visibly unavailable, with the command to get them.
- The hub address field says what to type (the placeholder shows a real example).

#### Edge paths

- A malformed or truncated link: the "invalid or incomplete" message offers the code flow instead.
- A link without `scopes` pre-ticks only the read-only three.
- A link opened in a tab that is already open (`hashchange`).

### HUB-J3 · Find the conversation that needs me

- **Entry:** `/commander/` with two paired machines, each running one supervisor
- **Goal:** I can tell which conversation has something new and get to it quickly
- **Touches:** `hub-web/src/conversation-list.ts`, `hub-web/src/palette-commands.ts`, `hub-web/src/worker-visibility.ts`, `hub-web/src/dormant-visibility.ts`, `hub-web/src/session-selection.ts`, `hub-web/src/conversation-shell.ts`, `hub-web/src/attention*.ts`, `hub-web/src/time.ts`, `hub-web/src/session-connection.ts`, `hub-web/src/connection-state-view.ts`
- **Suite:** `hub-web/e2e/journeys/find-conversation.journey.ts`, `hub-web/e2e/journeys/phone-host-line.journey.ts`
- **Gaps:** none

#### Steps

1. See every machine's supervisors in one list — every row is titled by its project, then its machine, with the supervisor codename beneath
2. Each row's time is its own session's activity — lighthouse reads "3h" and its row is named with "3 hours ago"; no time is a catalog check, and across a catalog poll none turns "now" or runs backwards
3. Notice a new reply while away — the row shows an unread count
4. Find the conversation through the list search — "Search conversations (Ctrl K)" at the top of the list filters rows by project, machine or supervisor; the row Enter opens is marked ("Enter ↵") and is the field's active descendant; opening a result clears the search and brings the whole list back; the header names the project once
5. Find the conversation from the keyboard — Ctrl+K lands in the search, type the project, Enter: the conversation is open and the reply box has focus
   - An empty thread's card also leads with the project, with machine and codename beneath it
   - A 40-character machine name ellipsises in its row and never runs under the time stamp, on desktop and at 390px
6. Jump to a supervisor by name — the command palette ("Type a command or conversation"; grouped Conversations / Machines / Appearance / Advanced, the same groups with a conversation open, Advanced collapsed and holding only the dormant-session switch; "Dismiss all info" only when something is outstanding) filters by supervisor or project and opens the conversation; each "Jump to" row leads with the project, the codename first in its description (right of the title on a desktop, on the line beneath on a phone); a filter that matches nothing says "No commands or conversations match"; the command Enter runs is marked and is the filter's active descendant ("light" marks "Jump to lighthouse")
7. Jump to a supervisor from the keyboard — Ctrl+K twice opens the palette, type the name, Enter: the palette closes, the conversation is open and the reply box has focus
   - Open Paired machines from the palette, then a conversation — the palette gives way to Paired machines and stays closed afterwards; it never comes back over the next conversation opened
8. Open a conversation over a slow relay: one calm line, and the footer stays Connected — "Opening the conversation…", the attempt and relay stage only behind a closed Details; one loading look from the attach to the first history page, centred in the reading area, still for its first second and then a quiet pulse, with the header and row on Live and the composer's width steady (cas-813a)
   - A first open that misses the 3-second mark retries calmly, and the footer stays Connected — the first retry of a conversation that has never opened still reads "Opening the conversation…" with the retry behind Details; no unavailable notice, no retry timeline, and the footer never drops to "1 connected"; a second failure shows as a real one

#### Expected experience

- Rows read project first, then machine, so two machines never look alike; the generated codename is tertiary.
- New replies and questions waiting for me are visible on the row without opening it.
- A visible search field finds a conversation by project, machine or supervisor; the palette does too, from anywhere.

#### Edge paths

- Nothing live: "No live supervisors listed", with a route to dormant sessions.
- A machine becomes unreachable while a message is pending: the row stays with "Unreachable · message pending".
- At 390px the machine name's glyphs fit vertically inside the clipped host line, in light and dark. Its full machine and supervisor identity stays available when the codename yields; long machine names still ellipsise horizontally. The header parts also cover desktop identity, forced colors, reduced motion, increased contrast, and a keyboard revisit (cas-9412).
- Many rows: the list is not sorted by attention.
- On a Mac, every surface names the palette chord "⌘K" (the list search and the Appearance & commands tooltip, with a conversation open too), never "Ctrl K", and ⌘K reaches the search and then the palette. This runs as a separate HUB-J3 part; the journeys declare a Linux keyboard platform by default, so they read the same on any host (cas-2a33).

### HUB-J4 · Read the conversation history

- **Entry:** a conversation with earlier turns from past days
- **Goal:** I can read what was said before, back to the start
- **Touches:** `hub-web/src/conversation-history.ts`, `hub-web/src/conversation-view.ts`, `hub-web/src/thread-model.ts`, `hub-web/src/markdown-renderer.ts`, `hub-web/src/time.ts`, `hub-web/src/attachment-sheet.ts`, `hub-web/src/artifact-open.ts`
- **Suite:** `hub-web/e2e/journeys/read-history.journey.ts`
- **Gaps:** history pages come from the double; real rows are covered by `hub-web/scripts/conversation-history-qa.mjs`

#### Steps

1. Open the conversation and see the recent turns — the latest exchange is on screen at once
2. Load earlier turns — "Load earlier" fetches the previous page
3. Reach the start of the conversation — "No earlier history" appears, with day separators
4. Open a report the supervisor sent — opening the file shows the hosted copy in a new tab through a short-lived signed link from the machine; every failure is said on the file card itself, never in a toast far from it, and leaves no tab open; a file that was never uploaded to Cloud says so, and opening it again opens no tab at all; Cloud failing ("wait a minute, then open it again") says what to do; a connected machine that sends nothing says it is connected but didn't send the file, never that it is off; when the connection then drops, that card says it in the banner's words ("Lost connection to <machine>. Reconnecting… Open the file again when it's back") without another click and without a second announcement, never "is connected" beside Reconnecting
5. The machine reconnects as the reader tabs back to Load earlier — a separate part on a multiplex machine: the reader has loaded one earlier page and moved on to the composer; the machine drops and comes back, and the reader tabs back to "Load earlier" the moment the header reads Live; focus stays on the button through the shell rebuild, and Enter loads the start of the conversation, landing on "No earlier history" (cas-d362)
6. The reader tabs to Load earlier as the machine comes back, and sees it — four separate parts on a multiplex machine with a tall history, at desktop 1280 and phone 390, with the reader resting in the composer (the thread follows its tail) or outside the thread (the thread stays on their earlier page): the focused "Load earlier" stays inside the thread through the reconnect, never pinned or put back above it, and Enter still loads the start of the conversation (cas-c2cb)

#### Expected experience

- Recent turns appear without any action, grouped by speaker and day.
- Loading earlier keeps the reading position; the button shows progress.
- The end of history is stated, not implied.

#### Edge paths

- The hub never answers a history request ("Loading earlier…" has no timeout).
- The socket closes while loading.
- A hub without history support: the thread starts empty.

### HUB-J5 · Reply by typing

- **Entry:** an open conversation with a live supervisor
- **Goal:** my message reaches the supervisor and I see its answer
- **Touches:** `hub-web/src/composer-markup.ts`, `hub-web/src/supervisor-message.ts`, `hub-web/src/conversation-view.ts`, `hub-web/src/live-regions.ts`, `hub-web/src/thread-model.ts`, `hub-web/src/conversation-history.ts`, `hub-web/src/refusal.ts`, `hub-web/src/swipe-dismiss.ts`, `hub-web/src/connection.ts`
- **Suite:** `hub-web/e2e/journeys/reply-typed.journey.ts`
- **Gaps:** delivery by a running daemon and operator stamping are doubled

#### Steps

1. Open the conversation — the composer names the supervisor
2. Write and send — the message appears at once as "Sending…" and the composer clears
3. See it delivered — the hub's receipt turns "Sending…" into "Delivered" with a check
4. See it answered — the supervisor's reply arrives under it and "Delivered" steps aside
5. A refused message says why — "Not sent", a plain reason and the next step on the message, said once (the composer only points at it); the list does not preview it as said
   - A refused Take control keeps focus on the message — when another device holds the session the take is refused; the message names the device in control, once, in plain words, and the composer only points at it; its Take control reads "Waiting for <device>" and is not pressable until that device releases control, then comes back on its own; keyboard focus stays on it, never the page body
6. Take control from the message, then retry — the refused message carries the Take control its refusal names (the conversation header has none); once control is taken it drops Take control and says Retry will send it, its actions are 44px targets on a phone, and Retry then sends it
7. Dismiss a refused message and bring it back — Dismiss (the corner ×, by keyboard or mouse) or a sideways swipe on a touch screen takes the refused message out of the thread; the composer stops pointing at it and the list stops previewing it; a "1 unsent message" chip (a 44px target on a phone) brings it back with Edit and Retry, focus landing on Retry; a short swipe settles back, and under reduced motion a swipe dismisses at once with no slide
8. Edit and resend retires the refused message — it collapses to "Not sent · replaced by your edit" with no Retry
9. A late receipt after the supervisor talks on never offers Retry — the supervisor's turn crosses the send and the receipt comes 3.4 s later; the message goes from "Sending…" to delivered without ever showing "Not confirmed" or Retry, and it is sent once
10. A message Cassy can't confirm offers Retry — with no receipt, 5 seconds after the supervisor talks on (or 15 seconds after the send) "Sending…" gives way to "Not confirmed · Cassy couldn't confirm delivery to <supervisor>. Retry sends it again."; the retry goes out and is delivered
11. Not confirmed settles once the supervisor replies after it — a supervisor turn that arrives after the give-up turns the card into "Not confirmed · The supervisor has replied since; send it again only if it missed this." with no Retry, so nothing invites a duplicate send; a quiet underlined "Send again" resends it without retyping
12. Focus on the opening card moves into the conversation — with keyboard focus on the connection card's Details while a slow relay opens the conversation, focus lands in the composer when the conversation replaces the card, never the page body; focus elsewhere (the list search) stays where it is

#### Expected experience

- Enter sends and Shift+Enter adds a new line.
- The user can tell sent from delivered without reading attributes.
- The composer status is in plain words, never protocol vocabulary.
- A refusal says why in plain words, names the next step, and offers Edit and Retry right on the message; a control refusal also offers Take control there, so the step it names is always on screen.
- Once its edit is sent, a refused message cannot be retried.
- A failed message never has to stay on screen: it can be dismissed or swiped away, and it is one tap to bring back.
- A message never says "Sending…" forever: without a receipt it turns "Not confirmed" and offers Retry, without claiming it was not sent, and it names Cassy, never "the hub".
- Once the supervisor has replied since, a "Not confirmed" message stops offering Retry; only a quiet "Send again" remains.
- A receipt that is only a few seconds late never flashes "Not confirmed", so there is no Retry that could send the message twice.
- A screen reader hears who spoke and when for each message group ("You, 12:45"), the status as "Live", and meets no dead attach control.

#### Edge paths

- Another device controls the session; nobody holds control (the page takes control first).
- The connection is reconnecting: the message is not sent and the user is told.
- A device paired without `message:send`: the composer explains the fix.

### HUB-J6 · Reply by voice

- **Entry:** an open conversation in a browser with speech recognition
- **Goal:** I speak my reply, check it, and send it
- **Touches:** `hub-web/src/speech-input.ts`, `hub-web/src/composer-markup.ts`, `hub-web/src/supervisor-message.ts`
- **Suite:** `hub-web/e2e/journeys/reply-voice.journey.ts`
- **Gaps:** speech recognition is stubbed; a real microphone and recognizer are not exercised

#### Steps

1. Open the conversation — the mic control is ready
2. Dictate the reply — "Start listening", speak, and the words land in the composer
3. Review and send — edit the transcript, then send it like a typed reply

#### Expected experience

- It is clear when the mic is listening; the placeholder says to speak, then review.
- Nothing is sent without the user pressing Send.
- The transcript lands at the cursor, so it can extend a typed draft.

#### Edge paths

- Mic permission denied: voice turns off and typing still works.
- No speech heard; a browser without speech recognition shows "Voice input unavailable".

### HUB-J7 · Answer a question in the thread

- **Entry:** an open conversation where the supervisor asks a question with choices
- **Goal:** I answer with one tap and the supervisor acts on it
- **Touches:** `hub-web/src/attention-objects.ts`, `hub-web/src/attention-view.ts`, `hub-web/src/conversation-history.ts`, `hub-web/src/conversation-view.ts`, `hub-web/src/context-rail.ts`, `hub-web/src/swipe-dismiss.ts`, `hub-web/src/dismissed-asks.ts`, `hub-web/src/thread-model.ts`, `hub-web/src/conversation-list.ts`
- **Suite:** `hub-web/e2e/journeys/answer-ask.journey.ts`
- **Gaps:** none

#### Steps

1. Open the conversation — the thread is live
2. A question from an ended session does not wait — the previous session's unanswered question is not in this session's thread: it sits in a collapsed "Earlier session patient-pelican-8" section with no choices, said by patient-pelican-8 (its own supervisor); nothing is pinned and the context rail lists only the live blocker
3. The supervisor asks a question — the full question and its choices stay in the thread, and a compact bookmark above the composer points to it without duplicating the question or its choices; the machine's earlier blocker, stamped by a clock that runs ahead and carrying no session, is this session's own turn: it sits above the question at its arrival time, labelled by this session's supervisor, with no "session … started" line below it, and is marked "machine clock ahead"
4. Answer with one tap — the pin clears, the thread records the chosen answer, and nothing is left waiting in the context rail, even when the machine's clock runs ahead; the answer shows the time it was sent, under today
5. See the supervisor act on the answer — the reply follows, and a new blocker after the answer waits
6. Jump to a long question and dismiss its bookmark — the supervisor posting an FYI and a status update leaves the question waiting with its choices in the thread; its compact bookmark says "Waiting on you: open the PR to main and cut a release?" and carries no choices; on a phone (390px, dark), focusing the composer keeps the bookmark 44–48px tall, and with the keyboard up (about 440px of page) at least three lines of the conversation stay readable; a tap jumps to and focuses the full question without expanding a duplicate card; a swipe dismisses the bookmark, and its copy in the thread says "Dismissed. You can still answer here." and keeps its choices
7. Reply to a machine a day ahead — no future day header: the machine's turn sits under Today at its arrival time, marked "machine clock ahead", and the reply shows the time it was sent below it, with no session line between them
8. Reopen the page — minutes later, the thread rebuilt from history keeps every turn where the visit showed it and at the same time (not the reload's), in the machine's order, under Today, with times reading in order
9. The supervisor answers live — a separate part on a machine whose clock runs five minutes ahead, which nothing in the thread has shown yet: the answer shows its arrival, unmarked, and its row reads "now"
10. Reload three minutes later — the answer shows the same time and still no mark, exactly as the visit showed it (cas-9e33), and its row reads "3m", not "now" (cas-24fe)
11. Come back five minutes later — the row reads "8m"; the reload measured the machine's lead, so the next live answer is marked "machine clock ahead" and its row reads "now"
12. Read a question on a short phone screen — at 390px by 440px, the waiting bookmark remains visible
13. A double tap jumps without sending — the full question in the thread gains focus, no reply is sent, and no duplicate card opens above the composer
14. Deliberately choose an option in the thread — the chosen answer is sent in reply to that question, and the bookmark clears

#### Expected experience

- The question is impossible to miss: a compact bookmark points to its full text and choice buttons in the thread.
- The bookmark never takes the whole screen: while the operator writes on a phone it is a one-line bar, and it can be dismissed.
- A question from a session that has ended is not shown as "Waiting on you"; a question the supervisor is still waiting on keeps its bookmark and its in-thread choices while the supervisor posts progress.
- After answering, the question stays readable in the thread with the answer shown.
- Turns read in time order under the right day, even when the machine's clock runs ahead: no future day header, and a quiet "machine clock ahead" instead of a time from the future.

#### Edge paths

- The hub refuses the answer: the question pins again.
- Typing a free-text reply also answers the pinned question.
- A question with no declared options leaves the reply to the composer; it does not invent choices.

### HUB-J8 · Switch between machines without losing my place

- **Entry:** two paired machines, a conversation open on one of them
- **Goal:** I work on the other machine, and my draft on the first is still there when I return
- **Touches:** `hub-web/src/session-selection.ts`, `hub-web/src/conversation-shell.ts`, `hub-web/src/conversation-list.ts`, `hub-web/src/machine-accent.ts`, `hub-web/src/paired-machines.ts`, `hub-web/src/composer-markup.ts`, `hub-web/src/worker-visibility.ts`
- **Suite:** `hub-web/e2e/journeys/switch-machines.journey.ts`
- **Gaps:** none

#### Steps

1. Start a draft on the Linux machine — the header names the project and the machine
2. Switch to the Mac and send there — the other thread starts with an empty composer; the message goes to that machine
3. Come back to the draft — the first thread's draft is intact
4. Name the palette chord one way on every surface — on a Linux keyboard the list search reads "Search conversations (Ctrl K)" and Appearance & commands "(Ctrl K twice)"; the tab title names the open conversation ("cas-src patient-pelican-9 — Cassy Cloud"); the conversation list and the open conversation are the whole screen; Ctrl K reaches the search, again the palette, and Escape leaves the conversation and its draft as they were
5. Keyboard focus lands somewhere real on every route — opening a conversation from the list by Enter or a click lands in its reply box; focus the operator moves to Raw output just after a pick stays there through the attach and a render round (cas-7eaf); a palette Jump by keyboard opens the conversation without leaving focus on the page body
6. Pair a third machine; the others keep their colours — each machine's accent is stored when it first pairs, so a new pairing (even one whose id sorts first) never re-colours the fleet, the new machine gets its own accent, and the colours survive a reload; a draft survives the pair link and the reload, and once sent does not come back
7. Know each conversation and machine by name — the conversation header leads with the project, then "Atlas · Linux · patient-pelican-9", its avatar the machine's own initial; every list row's name and every palette "Jump to" row lead with the project, the codename first in the line beneath
8. A supervisor with no workers yet is listed everywhere — a live supervisor that has not spawned workers is in the conversation list and the palette's Jump rows, which count the same; a stale or supervisor-less session is hidden from both
9. Hear the open conversation as the page heading — the goal state's level-1 heading is the open conversation's project, and the tab title names it with its codename
10. Every tap opens the conversation it pressed, while the list is changing — fifty taps across two machines (two projects on one, a group of two sessions on the other), each pressed while the group re-sorts and a session starts or ends; every tap highlights its row within a frame and opens its conversation within about 100 ms, at 1280 px with a mouse and at 390 px by touch, and the last tap wins (cas-4646)

#### Expected experience

- The header always says which machine the user is talking to.
- Drafts belong to their thread and never leak into another.

#### Edge paths

- Reload restores the last machine and conversation.
- Remove a machine from this browser (the Paired machines dialog).

### HUB-J9 · On a phone: from the list to a reply and back

- **Entry:** `/commander/` on a 390 px wide phone with two paired machines, plus one that is switched off
- **Goal:** I reply to a supervisor from my phone and get back to the list
- **Touches:** `hub-web/src/viewport.ts`, `hub-web/src/conversation-shell.ts`, `hub-web/src/composer-markup.ts`
- **Suite:** `hub-web/e2e/journeys/phone.journey.ts`, `hub-web/e2e/journeys/conversation-layout.journey.ts`
- **Gaps:** a real on-screen keyboard resize is not emulated

#### Steps

1. Open the list on a phone — full-width list, no sideways scrolling, and the search offers no keyboard shortcut
2. Tap a conversation — the thread replaces the list, with a back control
3. Reply with the phone keyboard — send, then see the answer
4. Go back to the list — the row shows the latest turn
5. Scroll back through a long thread — "Jump to latest" takes its own row above the composer, never over a turn, and one tap returns to the newest turn
6. Jump from the palette with the keyboard's Enter — opened with a tap, the palette's filter takes the phone keyboard; Enter opens the match with the keyboard gone, not in the reply box
7. See the switched-off machine named plainly — the footer counts it with a warning dot, and Paired machines says "Can't reach · retrying", shows times on the thread's 24-hour clock and "Version unknown until it connects"
8. Pair another machine and read its header at once — pairing from the phone opens its conversation, and the "connected" toast sits below the thread header, never over the back link, project and host, and on the list below the brand row

#### Expected experience

- One column at a time; every target is big enough to tap.
- The composer stays visible above the keyboard.
- The header always names the machine: a long machine name keeps its place (its OS word goes first, never cut mid-word) and the generated codename yields, ellipsised and then stepped aside; the line's title keeps both (cas-766c; suite stage "A long machine name keeps its place ahead of the codename in the header").

#### Edge paths

- The "Write to a supervisor" button opens a thread straight away.
- Attention and machine problems live in the desktop rail and are hidden on a phone.
- The supervisor pane's host beside the thread is hidden, inert and out of the accessibility tree: it never widens the conversation or takes a hit, and Raw output opens over it as a bottom sheet whose Close returns focus to Raw output, in light and dark (cas-ff3d, cas-0546).

### HUB-J10 · Switch to dark and keep reading

- **Entry:** an open conversation in the light theme
- **Goal:** I switch to dark and the choice sticks
- **Touches:** `hub-web/src/scheme.ts`, `hub-web/src/appearanceFonts.ts`, `hub-web/src/cloud-brand.ts`, `hub-web/scripts/generate-tokens.mjs`
- **Suite:** `hub-web/e2e/journeys/dark-theme.journey.ts`
- **Gaps:** none

#### Steps

1. Open the conversation in the light theme — a status reply is visible
2. Choose the dark appearance — from "Appearance & commands" (Ctrl/Cmd+K)
3. Keep reading in dark — the thread and composer stay readable
4. The choice survives a reload — dark is still applied
5. High contrast keeps the open conversation and Send marked — with forced colours on, in dark and light, the open row is filled with Highlight (under the pointer and with focus too) and Send is a filled button with an edge

#### Expected experience

- Appearance is found where the user expects it, and the change is instant.
- Dark is fully dark: no light panels and no unreadable text.

#### Edge paths

- "System" follows the device setting and changes live.
- Nothing shows which appearance is currently selected.

### HUB-J11 · The connection drops mid-conversation and recovers

- **Entry:** an open conversation when the network to the machine drops
- **Goal:** I see what is happening, and it recovers without me doing anything
- **Touches:** `hub-web/src/connection*.ts`, `hub-web/src/session-connection.ts`, `hub-web/src/abort-signals.ts`, `hub-web/src/deferred-render.ts`, `hub-web/src/browser-support.ts`
- **Suite:** `hub-web/e2e/journeys/reconnect.journey.ts`, `hub-web/e2e/journeys/real-hub.journey.ts`
- **Gaps:** the regular suite uses a protocol double; the separate real-hub part proves Linux recovery with a real disposable hub and real operator queue (see [local run instructions](real-hub-journey.md)); macOS remains follow-up

#### Steps

1. Open the conversation — the thread is live
2. The network drops — "Lost connection to Atlas · Linux. Reconnecting…" appears; the header and the row say Reconnecting, and the footer counts 1 of 2 connected with a warning dot; a send is held in the thread ("Waiting for the connection — sends when it's back") and not sent; the attention rail raises no transport alarm of its own, and its counts agree; a screen reader hears the outage once, from the banner (the header's Reconnecting is not a second announcement)
3. It reconnects on its own — the banner and the waiting line clear, everything says Live again (the header announces the return), the held message goes out exactly once and is delivered, and no transport alarm is left
4. Sending works again — a message goes through and is answered
5. On a phone, the banner stays readable through an outage — no toast sits on the reconnect banner, in light and dark; after it reconnects, every turn keeps its place (the day line still heads the thread, the session's own thread has no session line, and the held message stays above the ones sent after it)
6. During an outage, Interrupt and Raw output say why they wait — the Attention rail names the outage instead of "All clear"; the conversation header's Interrupt and Raw output stay on screen at 1280 and 390, marked unavailable and described by "Lost connection to Atlas · Linux. Interrupt and raw output return when it reconnects.", so the banner stays the one outage line announced; pressing either says why in a toast, sends no interrupt and opens no drawer; once the session is back both are available again and the rail says "All clear"

#### Expected experience

- The user always knows whether the conversation is live: the header, the row and the footer never disagree.
- The attention rail defers to the banner while it reconnects; only a failure that will not retry gets a card, in the same plain words.
- A transport alarm resolves itself when the connection comes back.
- Nothing typed is lost, and recovery needs no action.

#### Edge paths

- Pairing revoked (401/403): "Needs pairing", with a re-pair route (hidden on a phone).
- The machine is offline for a long time: attempts back off, at most 10 s apart; a held message not sent within 2 minutes turns "Not sent" with Retry and Edit.

### HUB-J12 · Switch networks without losing the conversation

- **Entry:** an open conversation on a phone or laptop that changes network: local network to Tailscale, Tailscale off and on, Wi-Fi to cellular, sleep and wake
- **Goal:** the conversation reconnects by itself and every message I send is delivered exactly once
- **Touches:** `hub-web/src/connection*.ts`, `hub-web/src/conversation-history.ts`, `hub-web/src/conversation-view.ts`, `hub-web/src/main.ts`
- **Suite:** `hub-web/e2e/journeys/network-switch.journey.ts`: one main test and several part tests marked `journeyPart`. Each part writes its receipts under `journeys/HUB-J12/parts/<part>/`, and `scripts/journey-bundles.py` folds every part's stages, cells and verdict into the HUB-J12 bundle and its JOURNEYS.md total (cas-1f7e)
- **Gaps:** the protocol double reproduces what a switch leaves behind (reset sockets, half-open sockets, offline, no event at all); a real phone moving between radios and a laptop truly sleeping are not driven, and a sleeping page is simulated by its visibility events

#### Steps

1. Open the conversation — the thread is live over the machine socket
2. The route changes under the page — the sockets reset and are replaced within seconds; a message sent then goes out once
3. Tailscale goes off, then on again — no browser event says so; after two unanswered heartbeats the header, row, footer and Tasks panel all say Unsteady ("Connection unsteady — checking…"), never "Degraded", and a message written then waits in the thread instead of going into the dead socket; once the dead socket is noticed everything says Reconnecting and the composer uses the banner's words; within 15 s of Tailscale returning it is Live again without a reload, and the waiting messages go out once each, in order, and are delivered. If the heartbeats answer again before that, a message held while unsteady goes out once without a reconnect
4. Wi-Fi hands over to cellular — going offline says Reconnecting at once; a message written meanwhile waits; coming online reconnects within 5 s and sends it once
5. The page wakes on a half-open socket — waking checks the socket and replaces it within seconds, well before the heartbeat would notice; a message then goes out once
6. The session's daemon link drops for a moment — the hub is reachable but cannot reach the session's daemon, so it refuses the send as retryable (upstream_unavailable); the message waits in the thread instead of reading "Not sent", and goes out once when the session is live again; while the link stays down the page retries with a growing pause (about 1, 2, 4, then 8 s), and past the two-minute hold the message reads Not sent with Retry ("The session didn't come back while it waited."), never "re-pair this device" or "will go out by itself"; meanwhile the banner reads "Reconnecting to <project>… <machine> is still connected." and the footer stays Connected ("Lost connection to <machine>" is kept for a real machine drop)
7. A proof refused after a switch retries on its own — the hub refuses the first proofs after a switch as stale (a 401 that names its reason); they are retried with a fresh proof, and a proof refused twice backs off like a lost network; it is Live again by itself, a message goes out once, and nothing asks to re-pair. Only a definitive refusal (revoked, unknown key) shows re-pair
8. On a legacy socket, a second message sent before the refusal arrives waits too — a hub without the machine protocol stops reading a session's socket once it refuses a send, so a message written before that refusal reached the page is held with the first; both go out once, in order, when the session is back; and once the session has stayed live for 10 s, the next drop retries after about 1 s again, receipt or not
9. A revoked pairing says so and offers Re-pair, on a phone too — a definitive refusal shows "Needs pairing", the banner says the machine needs pairing again instead of "Reconnecting…", the rail card is headed "Machine needs pairing", and the banner carries a Re-pair control (44 px on a phone) that opens pairing; a message waiting to send reads Not sent ("<machine> needs pairing again. Re-pair <machine>, then retry.") with Retry, and one already sent without a receipt reads Not confirmed, never Sending…
10. The whole machine drops, then returns — while it retries, the banner alone tells it ("Lost connection to <machine>. Reconnecting…"): no Attention card in transport terms ("Reconnecting to hub", "Stuck dialing", heartbeat counts), and no control toast over the conversation; a card appears only for a failure that will not retry, worded like the banner. While it is down the header's Interrupt says "Interrupt and raw output return when it reconnects"; once it is back, control this browser held is taken back by itself (unless another device took it), Interrupt is available again, and no "Control released" notice is left on screen or to a screen reader. A refused pairing leaves no "connection dropped" notice: Interrupt and Raw output say "Re-pair it to interrupt the supervisor or read its raw output" through later heartbeats, and a press interrupts nothing

#### Expected experience

- A network switch never needs a reload, and never leaves the page claiming Live on a dead socket.
- A message is never silently lost: it goes out once when it can, or says "Not sent" with Retry.
- Recovery is bounded: at most 10 s after the network returns, sooner when the browser says it changed.

#### Edge paths

- A message sent into a socket that later proves dead gets no receipt: it turns "Not confirmed" with Retry. Sending it again automatically would need the hub to recognise a repeat (its client_ref); until it does, the operator decides.
- The machine is unreachable for more than 2 minutes: a held message turns "Not sent" with Retry and Edit.

### HUB-J13 · Start a new session from Commander

- **Entry:** `/commander/` on a 390 px phone paired with full control access but without `session:launch`
- **Goal:** I start a supervisor on a project from my phone, without SSH, and land in its conversation
- **Touches:** `hub-web/src/launch-session.ts`, `hub-web/src/pairing-scopes.ts`, `hub-web/src/connection.ts`, `hub-web/src/conversation-shell.ts`
- **Suite:** `hub-web/e2e/journeys/launch-session.journey.ts`
- **Gaps:** the hub's project catalog, folder browse and session start are doubled; that a hub-started session survives a hub restart and the tab closing is proven at epic assembly on a real machine

#### Steps

1. A paired controller enables launch from Commander — "+ New session" (named by its goal before the permission too) opens the sheet's grant view, which names the machine, confirms "Start new sessions", and opens the launch form without a new pairing
2. The granted scope stays available — after reload "New session" opens the launch form instead of the grant view
3. Open New session and find the project — the most recently used project leads, a running project offers Attach, Browse is offered because the machine has launch folders, filtering narrows the list, and Claude is marked as the machine's default
4. Choose the account — every Claude account on the machine is listed with the default preselected; a logged-out one can't be picked and shows its `cas claude login <name>` command with Copy; a long address wraps; Grok has no account step; the summary names the chosen account
5. Start it and land on its supervisor — Start shows progress ("Starting <project> with <supervisor> (<account>) on <machine>…"), sends the account, and lands in the new session's conversation once the machine lists it
6. The session outlives the tab — after a reload the new session is still running and its conversation reopens
7. A running project attaches instead of starting again — Attach opens the existing session's conversation
8. A launch refused by the machine says why — the machine's refusal (here, the CLI isn't logged in) is a plain heading and the next step, with the machine's own message behind a disclosure; Back keeps the choices
9. Browse a launch folder and start a repository in it — folders open, only repository roots are selectable, and Start lands in the new session
10. A long machine name fits the phone consent — the full name wraps in the grant button and confirmation; Close stays visible and the page does not scroll sideways

#### Expected experience

- One obvious New session action on the "Conversations" heading's row (Pair a machine sits with the appearance control above), and in the command palette; a paired controller can allow it on the chosen machine while a read-only device sees invitation instructions.
- Nothing is started twice: a running project attaches.
- A refusal names what to fix on which machine; it never reads as a lost pairing.

#### Edge paths

- The hub refuses the scope at start (403): the sheet switches to the grant path.
- A read-only pairing cannot self-grant session launch; it needs a control invitation.
- Keyboard focus moves from Allow to Confirm to the project search after grant, and a very long machine label stays readable on a phone.
- The session does not come up within 90 s: the sheet says it was started and may still be starting.
- A machine without launch folders hides Browse.
- The machine can't list a CLI's accounts: the step says so with Try again, and the launch uses the machine's default account.
- The chosen account was removed or logged out before Start (400 invalid_profile / 422 not_logged_in): plain advice, and the list refreshes.

### HUB-J14 · Tell a project's live sessions apart

- **Entry:** `/commander/` on a desktop paired with `factory:manage`, one machine running three live sessions of one project
- **Goal:** each session opens onto its own conversation, or an honest empty state with its live activity, never another session's old thread; I can retire a stale one
- **Touches:** `hub-web/src/conversation-history.ts`, `hub-web/src/conversation-view.ts`, `hub-web/src/conversation-list.ts`, `hub-web/src/connection.ts`
- **Suite:** `hub-web/e2e/journeys/session-identity.journey.ts`
- **Gaps:** the daemon's session-bound history page, the catalog's last activity and the hub's End session are doubled; that ending a session stops its daemon is proven on a real machine at epic assembly

#### Steps

1. See a project's live sessions together — the project's rows sit under one heading naming the conversation count (the footer's noun), the most recently active one is marked "Most recent", each row's time is its own last activity, and each row leads with its codename and what its session last did in plain words ("Messaged bright-robin-85") before any is opened; at 1280×720 every row is in view
2. Open a session that has not written yet — the thread says "No messages from the gabber-studio supervisor in this session yet — nothing is waiting on you", shows "Last active 2m ago" with no internal jargon, and the older session's thread is only a collapsed "Earlier session noble-cheetah-84, Yesterday" section
3. Read an earlier session's messages — the section opens to its turns, each with its day and time, and offers no actions
4. Read the empty session's raw output — the header's Raw output opens the supervisor's terminal text ("The supervisor is ready.") though it has not written to me; Escape returns to Raw output and the empty thread
5. The empty thread follows the connection — off the network it says it is reconnecting to Atlas · Linux and offers no action of its own; the header's Raw output and Interrupt stay, marked unavailable and saying why ("Lost connection to Atlas · Linux. Interrupt and raw output return when it reconnects.") to the eye and to a screen reader, and pressing one says why instead of opening; back on, the plain copy returns and both are available again
6. On a phone, the header's Raw output and Interrupt are full-size targets — at 390 each is at least 44 px each way, and Raw output opens and closes
7. Each session shows its own conversation — the lowest row stays in view when it opens; another session's turns, even from a daemon that still sends project-wide history, appear only in its earlier section; a conversation with history never flashes a "no messages" card while its first page loads
8. End a stale session — End session asks first, names what stops, and only the confirmation ends it; the row leaves the group

#### Expected experience

- Several live sessions of one project never read as copies: they are grouped, the active one is marked, and each shows its own time.
- A session's thread holds only its own turns; another session's question never pins, waits or reads as answered here.
- Earlier turns always carry a date once they are not today's.

#### Edge paths

- A turn with no session recorded is filed under "Earlier messages with no session recorded".
- A device without `factory:manage` sees no End session; the hub refuses the call with 403 `scope_denied`.
- Under Needs pairing, Reconnecting or Unreachable the empty thread says why new messages cannot arrive; before its first page has loaded it claims no messages at all (cas-010f).
- Ending a session whose daemon PID now belongs to another process only cleans up its metadata.

### HUB-J15 · See a delivery problem as attention, not conversation

- **Entry:** `/commander/` on a desktop, one live session whose supervisor missed a relayed update
- **Goal:** the session's thread is only its conversation; a delivery problem is one attention item that goes away once resolved, and nothing claims I replied when I didn't
- **Touches:** `hub-web/src/operator-notices.ts`, `hub-web/src/attention-objects.ts`, `hub-web/src/attention.ts`, `hub-web/src/attention-view.ts`, `hub-web/src/thread-model.ts`, `hub-web/src/connection.ts`, `hub-web/src/conversation-view.ts`, `hub-web/src/conversation-shell.ts`
- **Suite:** `hub-web/e2e/journeys/delivery-notice.journey.ts`
- **Gaps:** the daemon's notice state and resolution announcement are doubled; that the daemon announces a real relay reaching the supervisor is proven by its Rust tests and at epic assembly

#### Steps

1. Open the session: only its conversation — the blocker and the operator's later message are in the thread, the watchdog notice is not, and yesterday's turn shows "Sep 29, 17:20"
2. A blocker I never answered does not say I replied — the blocker reads "You've written since this" with no tick, never "you replied"
3. The delivery problem is one attention item — the notice is a single warning dated "Sep 29, 17:49", headed "The supervisor missed an update: a worker stopped", with the worker named in plain words and no baked age; the group is "Accounting · happy-cheetah-1", has no redundant Dismiss group for one item, and a repeat adds nothing
4. On a phone, the delivery problem is one tap from the conversation — at 390×844 an Attention badge reading 1 sits in the thread header; tapping it opens the session's Attention as a sheet, focused on Close, showing the notice and its date
5. Keyboard stays in the sheet — Shift+Tab from Close moves to the collapsed Details' summary (never a control hidden inside it); twelve Tabs stay inside the modal sheet, each moving to a new stop and wrapping from Details to Close; focus on Dismiss stays there across a 5 s heartbeat; with a notice's Details open, Copy occupies its own row above the full raw diagnostic at390 and1280px; with Copy focused, a minute crossing keeps them, and so does a ten-minute wake that rebuilds the page (cas-f486); the conversation behind it is inert
6. A palette opened over the sheet closes first — Ctrl+K opens the command palette over the sheet; Escape closes the palette, the sheet stays open and focus is back on the control it left
7. Close it and keep reading — Escape closes the sheet even with focus dropped to the page, focus returns to the badge and stays on it when a catalog change rebuilds the page, and the thread is as it was
8. A sheet left open on a phone is a plain rail on a desktop — reopened, then resized to 1280×800: the rail has no dialog role, aria-modal or sheet label, the badge is not expanded, nothing is inert, and the notice is in the side panel
9. It retires once the update gets through — the resolution removes it, and a reload does not bring it back
10. An answer to an earlier session's question stays here — the supervisor's answer to a question from the ended session arrives in this thread with "Reply to “Did the bank feed reconcile?” · wise-lion-31", quoting the question's first line; if its history is unavailable it explicitly says "Reply to your message in earlier session wise-lion-31"; no earlier-session section opens for it

#### Expected experience

- The thread holds what the supervisor and the operator said; plumbing notices live in Attention.
- "Acknowledged — you replied" appears only for a reply sent to that card.
- Every turn not from today carries its date, and so does every Attention item.
- On a phone, a session's open Attention is never more than one tap from its conversation.
- A reply belongs to the session it is sent from; another session's turn is only quoted.

#### Edge paths

- A notice from another session never appears here, in the thread or its earlier sections.
- A dismissed notice stays dismissed when history replays it.
- A session that leaves the catalog retires its open notices.

### HUB-J16 · End a session from my phone

- **Entry:** `/commander/` on a phone (390×844, touch), one project with seven live sessions on one machine, and a device that may end sessions
- **Goal:** see at least six of the sessions at once, and end one with a confirmation that appears immediately, is focused and is fully visible
- **Touches:** `hub-web/src/conversation-list.ts`, `hub-web/src/styles.css`, `hub-web/src/main.ts`
- **Suite:** `hub-web/e2e/journeys/end-session.journey.ts`
- **Gaps:** the hub double ends the session; that the daemon stops its supervisor and workers is covered by the hub's Rust tests

#### Steps

1. See six sessions at once — under "gabber-studio · 7 conversations on Atlas", each row leads with its codename, End session is a glyph button in its own column at the row's end, and at least six rows are fully in view
2. Tap a session's time to open it, then come back — the time belongs to the row (End is a 44px column of its own beside it), the session opens, and back on the list all seven are still there
3. End session asks at once, focused on Cancel — on the last row the confirmation is present in the same task as the tap, Cancel has focus, both buttons are fully in view, and no row leaves the list
4. Cancel, then end it from the keyboard — Cancel returns focus to End session; Enter, Shift+Tab to the confirmation's End session and Enter end it, the heading reads 6 sessions, and focus lands on the row before the ended last row, never on the page

#### Expected experience

- A phone shows the sessions, not a column of End session lines.
- Ending a session is a deliberate two-step that never blanks the list or loses focus.

#### Edge paths

- A desktop keeps the worded End session line under each row (HUB-J14 covers it, including the last row's confirmation in view).
- A failed end shows its error under the row and offers End session again.

### HUB-J17 · Run the fleet from a conversation

- **Entry:** `/commander/` at 1280, an open conversation whose session has workers, a ready task and an awaiting-merge task, on a device paired with factory:operate and factory:manage
- **Goal:** pause a worker and undo it, assign a ready task to an idle worker, stop another worker after confirming, and ask the supervisor to merge after seeing the exact message, with every result announced and nothing refreshing on a timer
- **Touches:** `hub-web/src/fleet-ops.ts`, `hub-web/src/fleet-ops-view.ts`, `hub-web/src/fleet-permissions.ts`, `hub-web/src/main.ts`, `hub-web/src/connection.ts`, `hub-web/src/styles.css`
- **Suite:** `hub-web/e2e/journeys/fleet-ops.journey.ts`
- **Gaps:** the hub double serves `POST /v1/sessions/<s>/operations` with the brief's contract (op_id dedupe, 409 stale, FleetChanged); the real endpoint (S1, S2) is verified when those land; the phone part is S6

#### Steps

1. Pause a worker and undo it — the agent row's ⋯ menu lists Pause, Restart… and Stop…, Pause runs at once, the row and the live region say it, and Undo (8 s) resumes it
2. Assign a ready task to an idle worker — Assign… lists the idle workers, choosing one assigns it, and Undo unassigns it
3. Stop another worker after confirming — Stop… opens an inline confirmation naming what stops, Cancel first and focused; confirming stops it, and a stale stop says what changed
4. Ask the supervisor to merge — the awaiting-merge task's button shows the exact message first; Send sends it once and the row reads "Asked … ago"
5. A pairing without factory:manage — Stop… is disabled, saying it is not allowed on this pairing and naming the command that adds it

#### Expected experience

- Every action is one deliberate tap, destructive ones confirmed, reversible ones undoable.
- Every result is announced in one live region; menus close with Escape back to their opener.

#### Edge paths

- A stale precondition (the worker restarted, the task was assigned elsewhere) changes nothing and says what changed.
- A refused operation shows the hub's detail on the row.
- On a phone, a pending operation keeps its notice above the composer through a shell rebuild and Raw output; a completion that lands while Raw output is open keeps its Undo or refusal, reachable by keyboard once the sheet closes (M12).

### HUB-J18 · Interrupt or read the supervisor from its conversation

- **Entry:** `/commander/` at 1280 with two paired machines, a live cas-src conversation open, on a device whose pairing may force a take (hub-admin); parts at 390 light, 390 dark and 1280 dark, the light phone without hub-admin
- **Goal:** I stop what the supervisor is doing, or read what its terminal shows, without leaving the conversation, from the keyboard alone
- **Touches:** `hub-web/src/conversation-shell.ts`, `hub-web/src/transcript-view.ts`, `hub-web/src/connection-state-view.ts`, `hub-web/src/connection.ts`, `hub-web/src/early-thread.ts`
- **Suite:** `hub-web/e2e/journeys/interrupt-raw-output.journey.ts`: one main test and three part tests marked `journeyPart` (390 light, 390 dark, 1280 dark), receipts under `journeys/HUB-J18/parts/<part>/`
- **Gaps:** the hub double records the InterruptPane frame and models another device's lease (refused unless forced by hub-admin, as hub/auth.rs does); that the supervisor's pane really stops is proven by the hub's Rust tests

#### Steps

1. Open a conversation; only the thread shows — the header carries Raw output and "Interrupt the cas-src supervisor" (visible "Interrupt"), both available; the supervisor's pane surface is attached beside the thread but hidden, inert and out of the accessibility tree; no control, palette command or text names a terminal view
2. Interrupt the supervisor from the keyboard — Tab from the list search reaches Interrupt without stopping in the hidden pane; Enter sends one InterruptPane for the supervisor pane, the toast says "Interrupted the cas-src supervisor.", and focus stays on Interrupt
3. Take control from another device to interrupt — with Studio iPad in control, Interrupt forces the take and says so: "Took control from Studio iPad. Interrupted the cas-src supervisor."
4. Read the raw output from the keyboard — Enter on Raw output opens the "Raw output" drawer, described as read-only, with the supervisor's text ("The supervisor is ready."); it takes no text and typing in it sends nothing; Escape closes it and returns focus to Raw output

#### Expected experience

- The conversation is the only surface: interrupting and reading the raw output never leave it.
- Interrupt reads as the destructive action, in its own tone, and keeps its word on a phone; Raw output keeps its full name under its icon; both are full-size targets at 390.
- Taking control from another device is never silent.

#### Edge paths

- Another device holds control and this pairing cannot force a take: "Studio iPad is in control of this session. Interrupt works once it releases control.", and nothing is interrupted.
- On a phone Raw output is a bottom sheet across the width; on a desktop a drawer on the right edge.
- While the conversation is down both say why and wait (HUB-J11, HUB-J12, HUB-J14).

### HUB-J19 · Read my inbox on a new phone while the machine is off

- **Entry:** `/commander/` at 390×844 on a browser profile with no paired machine and no inbox sign-in, while soundwave (enrolled in the account's operator inbox) is off; a second desktop profile joins later
- **Goal:** I sign in with my account alone, read the supervisor's messages from the last weeks, and leave a reply that waits for soundwave until it is back and receives it
- **Touches:** `hub-web/src/inbox/*`, `hub-web/src/conversation-shell.ts`, `hub-web/test/operator-cloud-double.ts`
- **Suite:** `hub-web/e2e/journeys/operator-inbox.journey.ts`, with the cloud answered by `hub-web/e2e/journeys/operator-cloud-route.ts`
- **Gaps:** the cloud is a protocol double (PoP, enrollment check, epoch wraps, coverage, ACKs, commands), so the deployed-cloud two-profile journey of cas-9b7d S5 is the acceptance gate; soundwave's reservation and admission are modeled by the double, and proven by the hub's Rust tests (`operator_inbox::commands`)

#### Steps

1. Sign in on a brand-new phone with no machine paired — "Operator inbox" under the Conversations heading opens the inbox dialog, which says the cloud holds the keys and "this isn’t end-to-end encryption"; Sign in shows a code of the form ABCD-EFGH and "Approve on Petra Stella Cloud" linking to the approval page with that code
2. Approve it from the account; the weeks of messages are there — after the account approves the code, the dialog lists "soundwave · amber-fox-29"; opening it shows all three supervisor messages (20, 9 and 1 days old), and each was stored on the phone and acknowledged once
3. Reply while soundwave is off: it waits for soundwave — "Reply — soundwave gets it when it’s back" queues "Go. Cut the release." and shows "Waiting for soundwave"; the cloud holds one pending command for soundwave
4. A second browser profile sees the history and the queued reply — a desktop profile signs in with its own code and reads the same history and the queued reply; approved for reading only, it says it "can read this conversation but not leave replies"
5. soundwave returns and accepts the reply; a reload keeps everything — the reply reads "soundwave received it"; after a reload the inbox is still signed in with the history and the status, and no new sign-in was asked

#### Expected experience

- The inbox needs no machine connection: retained messages read while every machine is off.
- Sign-in is approved by the account, never by an older device.
- A reply to an offline machine never claims more than the machine confirmed: waiting for the machine until its receipt, received after.

#### Edge paths

- A revoked or expired device returns to signed-out and its stored inbox is wiped (unit-tested in `src/inbox/controller.test.ts`).
- An account reset warns once and continues on the new inbox.
- Expired history shows "Older history has expired. Messages are kept for 90 days." instead of an empty success.

### HUB-J20 · Grant a task write access outside its worktree from my phone

- **Entry:** `/commander/` at 390 (dark) and 1280 (light), an open conversation whose session has a task in progress, on a device paired with factory:manage
- **Goal:** grant the agents on a task create and edit access to a folder outside their worktree until the task closes, see the receipt, and revoke it; a pairing without factory:manage cannot
- **Touches:** `hub-web/src/write-grant.ts`, `hub-web/src/write-grant-view.ts`, `hub-web/src/fleet-ops-view.ts`, `hub-web/src/fleet-permissions.ts`, `hub-web/src/main.ts`, `hub-web/src/connection.ts`, `hub-web/src/styles.css`
- **Suite:** `hub-web/e2e/journeys/write-grant.journey.ts`
- **Gaps:** the hub double answers `POST /v1/sessions/<s>/write-grants` like the hub; that the hub writes `.cas/operator/write-policy.toml` with the device id, notes the task and posts the operator receipt to the supervisor is proven by the Rust hub tests (cas-ab04)

#### Steps

1. Open Write access on the current task — "Write access…" opens a panel on the first task in progress, with create and edit allowed and delete not
2. An incomplete grant says what is missing — Review grant with no folder says to enter one, and nothing is sent
3. Review, confirm and see the receipt — the confirmation asks "Grant agents on cas-1234 create+edit in ~/soundwave-config/docs/requests until the task closes?", Cancel first; Grant sends it once and the receipt, naming the resolved folder, is shown and announced
4. Revoke it after confirming — Revoke… asks to revoke every grant for the task; confirming says how many were removed
5. A pairing without factory:manage — Write access… is disabled, saying it is not allowed on this pairing

#### Expected experience

- A grant is never sent without a confirmation that names the task, the modes and the folder.
- The receipt and every refusal are in words, in the panel and the live region.

#### Edge paths

- A closed task or a folder that does not resolve is refused by the hub, and the panel shows the hub's reason.


### HUB-J21 · Switch from Commander to Explorer for my project

- **Entry:** `/commander/` at 1280 (light) and 390 (dark), two paired machines, one session whose project has a Cassy Cloud identity and one without
- **Goal:** reach Cassy Cloud's Explorer from Commander, on its home or on the open conversation's project, in its own tab, carrying nothing but the path and the project id
- **Touches:** `hub-web/src/app-switcher.ts`, `hub-web/src/cloud-brand.ts`, `hub-web/src/conversation-shell.ts`, `hub-web/src/main.ts`, `hub-web/src/styles.css`, `hub-web/index.html`
- **Suite:** `hub-web/e2e/journeys/app-switcher.journey.ts`
- **Gaps:** Explorer is stubbed at the cloud origin; Explorer's own switcher back to Commander and its token adoption are tracked in Richards-LLC/petra-stella-cloud#148; that the hub reports `cloud_project_id` from the project's canonical id is proven by the Rust hub test (cas-eaa3)

#### Steps

1. Commander is the current app — the list header's lockup ("Cassy Cloud apps") opens the switcher, with Commander current and Explorer as a link
2. Switch to Explorer from the list — Explorer opens at the cloud origin's /explorer in its own tab, with no referrer, cookie or authorization sent, and Commander stays as it was
3. Open this project's tasks in Explorer — from Tasks & progress, "All tasks in Explorer" opens /explorer/tasks?project_id=<the project's cloud id>; on desktop the switcher points there too
4. A project without a cloud identity opens Explorer's home — its conversation offers no project link, and the switcher opens /explorer

#### Expected experience

- Commander and Explorer read as one app: the same switcher, labels and position in both.
- Switching never signs anything in or out; each app keeps its own sign-in.

#### Edge paths

- With no cloud origin configured, Explorer is a disabled item that says so.
