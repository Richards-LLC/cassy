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
Every journey also watches each animation frame and fails if an open
conversation shows the terminal canvas or sits on a bare panel for more than
250 ms (`frame_defects` in `result.json`).

- **Surface-wide:** `hub-web/src/main.ts`, `hub-web/src/styles.css`, `hub-web/src/types.ts`, `hub-web/src/terminal*`, `hub-web/src/terminal/*`, `hub-web/index.html`, `hub-web/package-lock.json`, `hub-web/vite.config.ts`, `hub-web/dist/*`, `hub-web/e2e/journeys/hub-double.ts`, `hub-web/e2e/journeys/journey.ts`, `hub-web/e2e/journeys/world.ts`, `hub-web/playwright.config.ts`

### HUB-J1 · First open and pair a machine with a code

- **Entry:** `/commander/` in a browser that has never paired a machine
- **Goal:** my machine's supervisor appears and I can open its conversation
- **Touches:** `hub-web/src/pair*.ts`, `hub-web/src/pending-pairing.ts`, `hub-web/src/storage.ts`, `hub-web/src/first-connection.ts`, `hub-web/src/conversation-shell.ts`, `hub-web/src/paired-machines.ts`, `hub-web/src/connection.ts`, `hub-web/src/dpop.ts`
- **Suite:** `hub-web/e2e/journeys/pair-code.journey.ts`
- **Gaps:** the relay and the machine-side `cas hub authorize` are doubled; real pairing is proven by `docs/design/hub-web/pairing-verification.md`

**Steps**

1. Open Cassy Commander for the first time — the empty state explains what to do
2. Ask for a pairing code — "Pair a machine", then "Create pairing code" shows `cas hub authorize <code>`
3. Approve on the machine — the dialog follows the machine: waiting, claimed, authorized
4. Confirm and pair this browser — enter the operator label, then press Pair
5. See the machine's supervisor ready to talk to — a toast says the machine is connected, and its row opens a conversation

**Expected experience**

- One obvious "Pair a machine" action on the empty screen, at every width.
- The code and the exact command to run are shown together; copying the command works.
- While waiting, the status says what the machine is doing, with a visible countdown.
- The confirm step names the machine and the scopes in plain words before anything is stored.
- After pairing, the user lands on a list with the supervisor in it. No reload and no second step.

**Edge paths**

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

**Steps**

1. Open the link the machine printed — the dialog opens by itself and the secret leaves the address bar
2. Confirm the machine — the hub address and machine name arrive filled in from the link; only your name is left, with the focus ring on it
3. Check the technical details — the origin and the granted scope boxes, in sentence case
4. Pair — the exchange goes to the link's machine
5. Reach the supervisor — the machine's supervisor is listed and opens

**Expected experience**

- The invitation is recognised at once, with no extra click to start.
- Scopes the link did not grant are visibly unavailable, with the command to get them.
- The hub address field says what to type (the placeholder shows a real example).

**Edge paths**

- A malformed or truncated link: the "invalid or incomplete" message offers the code flow instead.
- A link without `scopes` pre-ticks only the read-only three.
- A link opened in a tab that is already open (`hashchange`).

### HUB-J3 · Find the conversation that needs me

- **Entry:** `/commander/` with two paired machines, each running one supervisor
- **Goal:** I can tell which conversation has something new and get to it quickly
- **Touches:** `hub-web/src/conversation-list.ts`, `hub-web/src/worker-visibility.ts`, `hub-web/src/dormant-visibility.ts`, `hub-web/src/session-selection.ts`, `hub-web/src/conversation-shell.ts`, `hub-web/src/attention*.ts`, `hub-web/src/time.ts`
- **Suite:** `hub-web/e2e/journeys/find-conversation.journey.ts`
- **Gaps:** none

**Steps**

1. See every machine's supervisors in one list — every row is titled by its project, then its machine, with the supervisor codename beneath
2. Notice a new reply while away — the row shows an unread count
3. Find the conversation through the list search — "Search conversations (Ctrl K)" at the top of the list filters rows by project, machine or supervisor; the header names the project once
4. Find the conversation from the keyboard — Ctrl+K lands in the search, type the project, Enter: the conversation is open and the reply box has focus
   - An empty thread's card also leads with the project, with machine and codename beneath it
   - A 40-character machine name ellipsises in its row and never runs under the time stamp, on desktop and at 390px
5. Jump to a supervisor by name — the command palette (grouped Conversations / Appearance / Advanced, Advanced collapsed) filters by supervisor or project and opens the conversation
6. Jump to a supervisor from the keyboard — Ctrl+K twice opens the palette, type the name, Enter: the palette closes, the conversation is open and the reply box has focus

**Expected experience**

- Rows read project first, then machine, so two machines never look alike; the generated codename is tertiary.
- New replies and questions waiting for me are visible on the row without opening it.
- A visible search field finds a conversation by project, machine or supervisor; the palette does too, from anywhere.

**Edge paths**

- Nothing live: "No live supervisors listed", with a route to dormant sessions.
- A machine becomes unreachable while a message is pending: the row stays with "Unreachable · message pending".
- Many rows: the list is not sorted by attention.

### HUB-J4 · Read the conversation history

- **Entry:** a conversation with earlier turns from past days
- **Goal:** I can read what was said before, back to the start
- **Touches:** `hub-web/src/conversation-history.ts`, `hub-web/src/conversation-view.ts`, `hub-web/src/thread-model.ts`, `hub-web/src/markdown-renderer.ts`, `hub-web/src/operator-thread.ts`, `hub-web/src/time.ts`
- **Suite:** `hub-web/e2e/journeys/read-history.journey.ts`
- **Gaps:** history pages come from the double; real rows are covered by `hub-web/scripts/conversation-history-qa.mjs`

**Steps**

1. Open the conversation and see the recent turns — the latest exchange is on screen at once
2. Load earlier turns — "Load earlier" fetches the previous page
3. Reach the start of the conversation — "No earlier history" appears, with day separators

**Expected experience**

- Recent turns appear without any action, grouped by speaker and day.
- Loading earlier keeps the reading position; the button shows progress.
- The end of history is stated, not implied.

**Edge paths**

- The hub never answers a history request ("Loading earlier…" has no timeout).
- The socket closes while loading.
- A hub without history support: the thread starts empty.

### HUB-J5 · Reply by typing

- **Entry:** an open conversation with a live supervisor
- **Goal:** my message reaches the supervisor and I see its answer
- **Touches:** `hub-web/src/composer-markup.ts`, `hub-web/src/supervisor-message.ts`, `hub-web/src/conversation-view.ts`, `hub-web/src/live-regions.ts`, `hub-web/src/operator-thread.ts`, `hub-web/src/thread-model.ts`, `hub-web/src/conversation-history.ts`, `hub-web/src/refusal.ts`
- **Suite:** `hub-web/e2e/journeys/reply-typed.journey.ts`
- **Gaps:** delivery by a running daemon and operator stamping are doubled

**Steps**

1. Open the conversation — the composer names the supervisor
2. Write and send — the message appears at once as "Sending…" and the composer clears
3. See it delivered — the hub's receipt turns "Sending…" into "Delivered" with a check
4. See it answered — the supervisor's reply arrives under it and "Delivered" steps aside
5. A refused message says why — "Not sent", a plain reason and the next step on the message, said once (the composer only points at it); the list does not preview it as said
6. Edit and resend retires the refused message — it collapses to "Not sent · replaced by your edit" with no Retry

**Expected experience**

- Enter sends and Shift+Enter adds a new line.
- The user can tell sent from delivered without reading attributes.
- The composer status is in plain words, never protocol vocabulary.
- A refusal says why in plain words, names the next step, and offers Edit and Retry right on the message.
- Once its edit is sent, a refused message cannot be retried.
- A screen reader hears who spoke and when for each message group ("You, 12:45"), the status as "Live", and meets no dead attach control.

**Edge paths**

- Another device controls the session; nobody holds control (the page takes control first).
- The connection is reconnecting: the message is not sent and the user is told.
- A device paired without `message:send`: the composer explains the fix.

### HUB-J6 · Reply by voice

- **Entry:** an open conversation in a browser with speech recognition
- **Goal:** I speak my reply, check it, and send it
- **Touches:** `hub-web/src/speech-input.ts`, `hub-web/src/composer-markup.ts`, `hub-web/src/supervisor-message.ts`
- **Suite:** `hub-web/e2e/journeys/reply-voice.journey.ts`
- **Gaps:** speech recognition is stubbed; a real microphone and recognizer are not exercised

**Steps**

1. Open the conversation — the mic control is ready
2. Dictate the reply — "Start listening", speak, and the words land in the composer
3. Review and send — edit the transcript, then send it like a typed reply

**Expected experience**

- It is clear when the mic is listening; the placeholder says to speak, then review.
- Nothing is sent without the user pressing Send.
- The transcript lands at the cursor, so it can extend a typed draft.

**Edge paths**

- Mic permission denied: voice turns off and typing still works.
- No speech heard; a browser without speech recognition shows "Voice input unavailable".

### HUB-J7 · Answer a pinned question

- **Entry:** an open conversation where the supervisor asks a question with choices
- **Goal:** I answer with one tap and the supervisor acts on it
- **Touches:** `hub-web/src/attention-objects.ts`, `hub-web/src/attention-view.ts`, `hub-web/src/conversation-history.ts`, `hub-web/src/conversation-view.ts`, `hub-web/src/context-rail.ts`
- **Suite:** `hub-web/e2e/journeys/answer-ask.journey.ts`
- **Gaps:** none

**Steps**

1. Open the conversation — the thread is live
2. The supervisor asks a question — it is pinned above the composer with its choices, and the thread keeps a one-line reference to it
3. Answer with one tap — the pin clears, the thread records the chosen answer, and nothing is left waiting in the context rail, even when the machine's clock runs ahead; the answer shows the time it was sent, under today
4. See the supervisor act on the answer — the reply follows, and a new blocker after the answer waits
5. Reply to a machine a day ahead — the reply shows the time it was sent, under today, below the machine's future-dated turn

**Expected experience**

- The question is impossible to miss, and its choices are buttons.
- After answering, the question stays readable in the thread with the answer shown.

**Edge paths**

- The hub refuses the answer: the question pins again.
- Typing a free-text reply also answers the pinned question.
- A question with no options offers "Yes, go ahead" and "Hold".

### HUB-J8 · Switch between machines without losing my place

- **Entry:** two paired machines, a conversation open on one of them
- **Goal:** I work on the other machine, and my draft on the first is still there when I return
- **Touches:** `hub-web/src/session-selection.ts`, `hub-web/src/conversation-shell.ts`, `hub-web/src/machine-accent.ts`, `hub-web/src/paired-machines.ts`, `hub-web/src/composer-markup.ts`
- **Suite:** `hub-web/e2e/journeys/switch-machines.journey.ts`
- **Gaps:** none

**Steps**

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

**Expected experience**

- The header always says which machine the user is talking to.
- Drafts belong to their thread and never leak into another.

**Edge paths**

- Reload restores the last machine and conversation.
- Remove a machine from this browser (the Paired machines dialog).

### HUB-J9 · On a phone: from the list to a reply and back

- **Entry:** `/commander/` on a 390 px wide phone with two paired machines, plus one that is switched off
- **Goal:** I reply to a supervisor from my phone and get back to the list
- **Touches:** `hub-web/src/viewport.ts`, `hub-web/src/pane-layout.ts`, `hub-web/src/conversation-shell.ts`, `hub-web/src/composer-markup.ts`
- **Suite:** `hub-web/e2e/journeys/phone.journey.ts`
- **Gaps:** a real on-screen keyboard resize is not emulated

**Steps**

1. Open the list on a phone — full-width list, no sideways scrolling
2. Tap a conversation — the thread replaces the list, with a back control
3. Reply with the phone keyboard — send, then see the answer
4. Go back to the list — the row shows the latest turn
5. Scroll back through a long thread — "Jump to latest" takes its own row above the composer, never over a turn, and one tap returns to the newest turn
6. See the switched-off machine named plainly — the footer counts it with a warning dot, and Paired machines says "Can't reach · retrying"
7. Pair another machine and read its header at once — pairing from the phone opens its conversation, and the "connected" toast sits below the thread header, never over the back link, project and host

**Expected experience**

- One column at a time; every target is big enough to tap.
- The composer stays visible above the keyboard.

**Edge paths**

- The "Write to a supervisor" button opens a thread straight away.
- Attention and machine problems live in the desktop rail and are hidden on a phone.

### HUB-J10 · Switch to dark and keep reading

- **Entry:** an open conversation in the light theme
- **Goal:** I switch to dark and the choice sticks
- **Touches:** `hub-web/src/scheme.ts`, `hub-web/src/appearanceFonts.ts`, `hub-web/src/cloud-brand.ts`, `hub-web/scripts/generate-tokens.mjs`
- **Suite:** `hub-web/e2e/journeys/dark-theme.journey.ts`
- **Gaps:** none

**Steps**

1. Open the conversation in the light theme — a status reply is visible
2. Choose the dark appearance — from "Appearance & commands" (Ctrl/Cmd+K)
3. Keep reading in dark — the thread and composer stay readable
4. The choice survives a reload — dark is still applied
5. High contrast keeps the open conversation and Send marked — with forced colours on, in dark and light, the open row is filled with Highlight (under the pointer and with focus too) and Send is a filled button with an edge

**Expected experience**

- Appearance is found where the user expects it, and the change is instant.
- Dark is fully dark: no light panels and no unreadable text.

**Edge paths**

- "System" follows the device setting and changes live.
- Nothing shows which appearance is currently selected.

### HUB-J11 · The connection drops mid-conversation and recovers

- **Entry:** an open conversation when the network to the machine drops
- **Goal:** I see what is happening, and it recovers without me doing anything
- **Touches:** `hub-web/src/connection*.ts`, `hub-web/src/session-connection.ts`, `hub-web/src/abort-signals.ts`, `hub-web/src/deferred-render.ts`, `hub-web/src/browser-support.ts`
- **Suite:** `hub-web/e2e/journeys/reconnect.journey.ts`
- **Gaps:** a real network loss (heartbeat misses, offline) is simulated by closing the socket

**Steps**

1. Open the conversation — the thread is live
2. The network drops — "Lost connection to Atlas · Linux. Reconnecting…" appears; the header and the row say Reconnecting, and the footer counts 1 of 2 connected with a warning dot; a send is refused for the connection
3. It reconnects on its own — the banner and the refusal line clear, everything says Live again, the draft is kept, and no transport alarm is left
4. Sending works again — a message goes through and is answered
5. On a phone, the banner stays readable through an outage — no toast sits on the reconnect banner, in light and dark

**Expected experience**

- The user always knows whether the conversation is live: the header, the row and the footer never disagree.
- A transport alarm resolves itself when the connection comes back.
- Nothing typed is lost, and recovery needs no action.

**Edge paths**

- Pairing revoked (401/403): "Needs pairing", with a re-pair route (hidden on a phone).
- The machine is offline for a long time: attempts back off up to 30 s.
