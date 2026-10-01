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
Every journey also watches each animation frame and fails if an open
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
5. See the machine's supervisor ready to talk to — a toast says the machine is connected without covering the composer or any heading, and its row opens a conversation

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
- **Suite:** `hub-web/e2e/journeys/find-conversation.journey.ts`
- **Gaps:** none

#### Steps

1. See every machine's supervisors in one list — every row is titled by its project, then its machine, with the supervisor codename beneath
2. Notice a new reply while away — the row shows an unread count
3. Find the conversation through the list search — "Search conversations (Ctrl K)" at the top of the list filters rows by project, machine or supervisor; the header names the project once
4. Find the conversation from the keyboard — Ctrl+K lands in the search, type the project, Enter: the conversation is open and the reply box has focus
   - An empty thread's card also leads with the project, with machine and codename beneath it
   - A 40-character machine name ellipsises in its row and never runs under the time stamp, on desktop and at 390px
5. Jump to a supervisor by name — the command palette ("Type a command or conversation"; grouped Conversations / This conversation / Machines / Appearance / Advanced, Advanced collapsed; "Dismiss all info" only when something is outstanding) filters by supervisor or project and opens the conversation; each "Jump to" row leads with the project, the codename first in its description (right of the title on a desktop, on the line beneath on a phone); the control command names what the device can do ("Let other devices type here"), the control term as its hint; a filter that matches nothing says "No commands or conversations match"
6. Jump to a supervisor from the keyboard — Ctrl+K twice opens the palette, type the name, Enter: the palette closes, the conversation is open and the reply box has focus
   - Open Paired machines from the palette, then a conversation — the palette gives way to Paired machines and stays closed afterwards; it never comes back over the next conversation opened
7. Open a conversation over a slow relay: one calm line, and the footer stays Connected — "Opening the conversation…", the attempt and relay stage only behind a closed Details
   - A first open that misses the 3-second mark retries calmly, and the footer stays Connected — the first retry of a conversation that has never opened still reads "Opening the conversation…" with the retry behind Details; no "Terminal unavailable", no retry timeline, and the footer never drops to "1 connected"; a second failure shows as a real one

#### Expected experience

- Rows read project first, then machine, so two machines never look alike; the generated codename is tertiary.
- New replies and questions waiting for me are visible on the row without opening it.
- A visible search field finds a conversation by project, machine or supervisor; the palette does too, from anywhere.

#### Edge paths

- Nothing live: "No live supervisors listed", with a route to dormant sessions.
- A machine becomes unreachable while a message is pending: the row stays with "Unreachable · message pending".
- Many rows: the list is not sorted by attention.

### HUB-J4 · Read the conversation history

- **Entry:** a conversation with earlier turns from past days
- **Goal:** I can read what was said before, back to the start
- **Touches:** `hub-web/src/conversation-history.ts`, `hub-web/src/conversation-view.ts`, `hub-web/src/thread-model.ts`, `hub-web/src/markdown-renderer.ts`, `hub-web/src/operator-thread.ts`, `hub-web/src/time.ts`, `hub-web/src/attachment-sheet.ts`, `hub-web/src/artifact-open.ts`
- **Suite:** `hub-web/e2e/journeys/read-history.journey.ts`
- **Gaps:** history pages come from the double; real rows are covered by `hub-web/scripts/conversation-history-qa.mjs`

#### Steps

1. Open the conversation and see the recent turns — the latest exchange is on screen at once
2. Load earlier turns — "Load earlier" fetches the previous page
3. Reach the start of the conversation — "No earlier history" appears, with day separators
4. Open a report the supervisor sent — opening the file shows the hosted copy in a new tab through a short-lived signed link from the machine; every failure is said on the file card itself, never in a toast far from it, and leaves no tab open; a file that was never uploaded to Cloud says so, and opening it again opens no tab at all; Cloud failing ("wait a minute, then open it again") says what to do; a connected machine that sends nothing says it is connected but didn't send the file, never that it is off

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
- **Touches:** `hub-web/src/composer-markup.ts`, `hub-web/src/supervisor-message.ts`, `hub-web/src/conversation-view.ts`, `hub-web/src/live-regions.ts`, `hub-web/src/operator-thread.ts`, `hub-web/src/thread-model.ts`, `hub-web/src/conversation-history.ts`, `hub-web/src/refusal.ts`, `hub-web/src/swipe-dismiss.ts`
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

### HUB-J7 · Answer a pinned question

- **Entry:** an open conversation where the supervisor asks a question with choices
- **Goal:** I answer with one tap and the supervisor acts on it
- **Touches:** `hub-web/src/attention-objects.ts`, `hub-web/src/attention-view.ts`, `hub-web/src/conversation-history.ts`, `hub-web/src/conversation-view.ts`, `hub-web/src/context-rail.ts`, `hub-web/src/swipe-dismiss.ts`, `hub-web/src/dismissed-asks.ts`
- **Suite:** `hub-web/e2e/journeys/answer-ask.journey.ts`
- **Gaps:** none

#### Steps

1. Open the conversation — the thread is live
2. A question from an ended session does not wait — the previous session's unanswered question is not in this session's thread: it sits in a collapsed "Earlier session patient-pelican-8" section with no choices; nothing is pinned and the context rail lists only the live blocker
3. The supervisor asks a question — it is pinned above the composer with its choices, and the thread keeps a one-line reference to it; the machine's earlier blocker, stamped by a clock that runs ahead, sits above the session line at its arrival time and is marked "machine clock ahead"
4. Answer with one tap — the pin clears, the thread records the chosen answer, and nothing is left waiting in the context rail, even when the machine's clock runs ahead; the answer shows the time it was sent, under today
5. See the supervisor act on the answer — the reply follows, and a new blocker after the answer waits
6. Fold, open and dismiss a question — the supervisor posting an FYI and a status update while it waits leaves the question pinned with its choices; on a desktop the pinned question collapses to a one-line bar ("Waiting on you: open the PR…") and opens again, and writing in the composer leaves it open; on a phone (390px, dark) focusing the composer folds it to the bar, and with the keyboard up (about 440px of page) at least three lines of the latest conversation stay readable; a tap on the bar opens it; a swipe takes it off, and its copy in the thread says "Dismissed. You can still answer here." and keeps its choices
7. Reply to a machine a day ahead — no future day header: the machine's turn sits under Today at its arrival time, marked "machine clock ahead", and the reply shows the time it was sent below it
8. Reopen the page — the thread rebuilt from history keeps every turn where the visit showed it, in the machine's order, under Today, with times reading in order

#### Expected experience

- The question is impossible to miss, and its choices are buttons.
- A question never takes the whole screen: while the operator writes on a phone it is a one-line bar, and it can be dismissed.
- A question from a session that has ended is not shown as "Waiting on you"; a question the supervisor is still waiting on keeps its pin and its choices while the supervisor posts progress.
- After answering, the question stays readable in the thread with the answer shown.
- Turns read in time order under the right day, even when the machine's clock runs ahead: no future day header, and a quiet "machine clock ahead" instead of a time from the future.

#### Edge paths

- The hub refuses the answer: the question pins again.
- Typing a free-text reply also answers the pinned question.
- A question with no options offers "Yes, go ahead" and "Hold".

### HUB-J8 · Switch between machines without losing my place

- **Entry:** two paired machines, a conversation open on one of them
- **Goal:** I work on the other machine, and my draft on the first is still there when I return
- **Touches:** `hub-web/src/session-selection.ts`, `hub-web/src/conversation-shell.ts`, `hub-web/src/machine-accent.ts`, `hub-web/src/paired-machines.ts`, `hub-web/src/composer-markup.ts`, `hub-web/src/worker-visibility.ts`
- **Suite:** `hub-web/e2e/journeys/switch-machines.journey.ts`
- **Gaps:** none

#### Steps

1. Start a draft on the Linux machine — the header names the project and the machine
2. Switch to the Mac and send there — the other thread starts with an empty composer; the message goes to that machine
3. Come back to the draft — the first thread's draft is intact
4. Reopen the session picker after closing it — in Terminal view, one click on the session title reopens the picker after Escape or ×, and it never pops open over the next dialog; Escape and × leave focus on the session title, from the first open on; a filter typed before closing is cleared on the next open, which lists every session again
5. Keep my place in the session picker while updates arrive — the row a keyboard user arrowed or tabbed onto keeps focus, and the filter holds, while hub updates re-render the page
6. See which session is open while pointing at it — in light and dark, the open session keeps its tint under the pointer (lifting as feedback) and still differs from an ordinary hovered row
7. Come back from the terminal to the reply box — returning from Terminal view by keyboard or mouse puts focus in the reply box, never on the page body
8. Keyboard focus lands somewhere real on every route — entering Terminal view lands in the terminal (or on the way back, which is first in the Tab order though drawn at the foot), choosing a session in the picker lands in it, and opening a conversation from the list by Enter or a click lands in its reply box; none leaves focus on the page body
9. Read every session's details on a phone — at 390px each picker row, the open one included, shows project, role, workers and status in full
10. Pair a third machine; the others keep their colours — each machine's accent is stored when it first pairs, so a new pairing (even one whose id sorts first) never re-colours the fleet, the new machine gets its own accent, and the colours survive a reload
11. Know each session and machine by name in Terminal view — the session title, every picker row and every palette "Jump to" row lead with the project, with the supervisor codename secondary; the machine rail and the compact machine chip read two letters of the machine's own name ("AT" for "Atlas · Linux"), never a separator
12. A supervisor with no workers yet is listed everywhere — a live supervisor that has not spawned workers is in the conversation list, the palette's Jump rows and the session picker ("no workers · live"), and the "Switch session — N available" count and the Terminal view fleet board match all three; a stale or supervisor-less session is hidden from every one of them; on the Fleet overview each row leads with its project, and when several sessions share a project each plot row also shows the shortest distinct tail of its codename ("cas-src · pelican-9", "cas-src · otter-5"), and the same codename on two machines adds the machine's rail initials ("otter-5 · AL", "otter-5 · AT"); tags are whole and the project keeps a letter, at 1280 and 390
13. Tell one codename apart on two machines whose initials match — with Atlas and Attic (both "AT") running the same supervisor codename, the Fleet plot marks each machine by the shortest part of its name that differs ("Atl" / "Att"), never the full machine label; a twin tag is capped to fit the 132px column at 390, trimming a long codename tail from the left ("…ter-5 · Atl", "…can-9 · Att"). At 1280 and 390 the tag stays whole and the project keeps a letter

#### Expected experience

- The header always says which machine the user is talking to.
- Drafts belong to their thread and never leak into another.

#### Edge paths

- Reload restores the last machine and conversation.
- Remove a machine from this browser (the Paired machines dialog).

### HUB-J9 · On a phone: from the list to a reply and back

- **Entry:** `/commander/` on a 390 px wide phone with two paired machines, plus one that is switched off
- **Goal:** I reply to a supervisor from my phone and get back to the list
- **Touches:** `hub-web/src/viewport.ts`, `hub-web/src/pane-layout.ts`, `hub-web/src/conversation-shell.ts`, `hub-web/src/composer-markup.ts`
- **Suite:** `hub-web/e2e/journeys/phone.journey.ts`
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

#### Edge paths

- The "Write to a supervisor" button opens a thread straight away.
- Attention and machine problems live in the desktop rail and are hidden on a phone.

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
- **Suite:** `hub-web/e2e/journeys/reconnect.journey.ts`
- **Gaps:** a real network loss (heartbeat misses, offline) is simulated by closing the socket

#### Steps

1. Open the conversation — the thread is live
2. The network drops — "Lost connection to Atlas · Linux. Reconnecting…" appears; the header and the row say Reconnecting, and the footer counts 1 of 2 connected with a warning dot; a send is held in the thread ("Waiting for the connection — sends when it's back") and not sent; the attention rail raises no transport alarm of its own, and its counts agree
3. It reconnects on its own — the banner and the waiting line clear, everything says Live again, the held message goes out exactly once and is delivered, and no transport alarm is left
4. Sending works again — a message goes through and is answered
5. On a phone, the banner stays readable through an outage — no toast sits on the reconnect banner, in light and dark; after it reconnects, every turn keeps its place (the message stays below its session line)
6. In Terminal view, nothing claims all clear or live during an outage — the Attention rail names the outage instead of "All clear", the machine rail says Reconnecting, the header drops CONTROL and shows Reconnecting in place of a latency, Take/Release control and Interrupt say why they are unavailable, and the machine drawer's session row says Reconnecting, not live; all return when the session is back

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
- **Suite:** `hub-web/e2e/journeys/network-switch.journey.ts`
- **Gaps:** the protocol double reproduces what a switch leaves behind (reset sockets, half-open sockets, offline, no event at all); a real phone moving between radios and a laptop truly sleeping are not driven, and a sleeping page is simulated by its visibility events

#### Steps

1. Open the conversation — the thread is live over the machine socket
2. The route changes under the page — the sockets reset and are replaced within seconds; a message sent then goes out once
3. Tailscale goes off, then on again — no browser event says so; the dead socket is noticed, everything says Reconnecting, a message written meanwhile waits in the thread; within 15 s of Tailscale returning it is Live again without a reload, and the waiting message goes out once and is delivered
4. Wi-Fi hands over to cellular — going offline says Reconnecting at once; a message written meanwhile waits; coming online reconnects within 5 s and sends it once
5. The page wakes on a half-open socket — waking checks the socket and replaces it within seconds, well before the heartbeat would notice; a message then goes out once
6. The session's daemon link drops for a moment — the hub is reachable but cannot reach the session's daemon, so it refuses the send as retryable (upstream_unavailable); the message waits in the thread instead of reading "Not sent", and goes out once when the session is live again; while the link stays down the page retries with a growing pause (about 1, 2, 4, then 8 s), and past the two-minute hold the message reads Not sent with Retry ("The session didn't come back while it waited."), never "re-pair this device" or "will go out by itself"; meanwhile the banner reads "Reconnecting to <project>… <machine> is still connected." and the footer stays Connected ("Lost connection to <machine>" is kept for a real machine drop)
7. A proof refused after a switch retries on its own — the hub refuses the first proofs after a switch as stale (a 401 that names its reason); they are retried with a fresh proof, and a proof refused twice backs off like a lost network; it is Live again by itself, a message goes out once, and nothing asks to re-pair. Only a definitive refusal (revoked, unknown key) shows re-pair
8. On a legacy socket, a second message sent before the refusal arrives waits too — a hub without the machine protocol stops reading a session's socket once it refuses a send, so a message written before that refusal reached the page is held with the first; both go out once, in order, when the session is back; and once the session has stayed live for 10 s, the next drop retries after about 1 s again, receipt or not
9. A revoked pairing says so and offers Re-pair, on a phone too — a definitive refusal shows "Needs pairing", the banner says the machine needs pairing again instead of "Reconnecting…", and carries a Re-pair control (44 px on a phone) that opens pairing

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

1. A paired controller enables launch from Commander — "Allow new sessions" names the machine, confirms "Start new sessions", and opens the launch form without a new pairing
2. The granted scope stays available — after reload "New session" replaces the grant path
3. Open New session and find the project — the most recently used project leads, a running project offers Attach, Browse is offered because the machine has launch folders, filtering narrows the list, and Claude is marked as the machine's default
4. Choose the account — every Claude account on the machine is listed with the default preselected; a logged-out one can't be picked and shows its `cas claude login <name>` command with Copy; a long address wraps; Grok has no account step; the summary names the chosen account
5. Start it and land on its supervisor — Start shows progress ("Starting <project> with <supervisor> (<account>) on <machine>…"), sends the account, and lands in the new session's conversation once the machine lists it
6. The session outlives the tab — after a reload the new session is still running and its conversation reopens
7. A running project attaches instead of starting again — Attach opens the existing session's conversation
8. A launch refused by the machine says why — the machine's refusal (here, the CLI isn't logged in) is a plain heading and the next step, with the machine's own message behind a disclosure; Back keeps the choices
9. Browse a launch folder and start a repository in it — folders open, only repository roots are selectable, and Start lands in the new session
10. A long machine name fits the phone consent — the full name wraps in the grant button and confirmation; Close stays visible and the page does not scroll sideways

#### Expected experience

- One obvious New session action beside Pair a machine, and in the command palette; a paired controller can allow it on the chosen machine while a read-only device sees invitation instructions.
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

1. See a project's live sessions together — the project's rows sit under one heading naming the session count, the most recently active one is marked "Most recent", and each row's time is its own last activity
2. Open a session that has not written yet — the thread says "No Commander messages from this session yet", shows its last activity, and the older session's thread is only a collapsed "Earlier session noble-cheetah-84, Yesterday" section
3. Read an earlier session's messages — the section opens to its turns, each with its day and time, and offers no actions
4. Open the Terminal from the empty session — "Open Terminal" switches to the session's Terminal view and back
5. Each session shows its own conversation — another session's turns, even from a daemon that still sends project-wide history, appear only in its earlier section
6. End a stale session — End session asks first, names what stops, and only the confirmation ends it; the row leaves the group

#### Expected experience

- Several live sessions of one project never read as copies: they are grouped, the active one is marked, and each shows its own time.
- A session's thread holds only its own turns; another session's question never pins, waits or reads as answered here.
- Earlier turns always carry a date once they are not today's.

#### Edge paths

- A turn with no session recorded is filed under "Earlier messages with no session recorded".
- A device without `factory:manage` sees no End session; the hub refuses the call with 403 `scope_denied`.
- Ending a session whose daemon PID now belongs to another process only cleans up its metadata.

### HUB-J15 · See a delivery problem as attention, not conversation

- **Entry:** `/commander/` on a desktop, one live session whose supervisor missed a relayed update
- **Goal:** the session's thread is only its conversation; a delivery problem is one attention item that goes away once resolved, and nothing claims I replied when I didn't
- **Touches:** `hub-web/src/operator-notices.ts`, `hub-web/src/attention-objects.ts`, `hub-web/src/attention.ts`, `hub-web/src/thread-model.ts`, `hub-web/src/connection.ts`, `hub-web/src/conversation-view.ts`
- **Suite:** `hub-web/e2e/journeys/delivery-notice.journey.ts`
- **Gaps:** the daemon's notice state and resolution announcement are doubled; that the daemon announces a real relay reaching the supervisor is proven by its Rust tests and at epic assembly

#### Steps

1. Open the session: only its conversation — the blocker and the operator's later message are in the thread, the watchdog notice is not, and yesterday's turn shows "Sep 29, 17:20"
2. A blocker I never answered does not say I replied — the blocker reads "You've written since this" with no tick, never "you replied"
3. The delivery problem is one attention item — the notice is a single warning in Attention, and a repeat of it adds nothing
4. It retires once the update gets through — the resolution removes it, and a reload does not bring it back
5. An answer to an earlier session's question stays here — the supervisor's answer to a question from the ended session arrives in this thread with "re: earlier session wise-lion-31", and no earlier-session section opens for it

#### Expected experience

- The thread holds what the supervisor and the operator said; plumbing notices live in Attention.
- "Acknowledged — you replied" appears only for a reply sent to that card.
- Every turn not from today carries its date.
- A reply belongs to the session it is sent from; another session's turn is only quoted.

#### Edge paths

- A notice from another session never appears here, in the thread or its earlier sections.
- A dismissed notice stays dismissed when history replays it.
- A session that leaves the catalog retires its open notices.
