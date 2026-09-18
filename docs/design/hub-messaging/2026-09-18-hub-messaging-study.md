# Hub messaging: show the operator the supervisor's turns, not the pane

<!-- figure: hero -->

> When I'm interacting with the hub I only care about the messages the supervisor is sending me. It should just be my messages and the supervisor messages to me giving me statuses or answering a question. There's no need to stream the whole terminal if we can know which messages the supervisor intends for us.

The direction above (Daniel, 2026-09-18) is a design question and a protocol question. This study answers both: it measures what the pane mirror delivers, collects <!-- refcount --> messaging and activity-feed interfaces as real captures, proposes a six-kind message taxonomy, draws three genuinely different surfaces for it — **Ledger**, **Desk** and **Brief** — at 1280 and 390 px in both schemes, recommends the Ledger, and names the exact code path — with file:line evidence — where a typed supervisor→operator message already half-exists. Nothing is implemented; the closing ledger is the minimal change, and the operator picks the surface before anything is built.

## What the pane delivers today

The hub's conversation view (`hub-web/src/conversation-view.ts`) already draws three things: the operator's own sends (`from-you`), the supervisor's typed replies (`Reply to you`), and then everything the emulator holds as "Live pane text". The third layer is the problem. Three real cas-src supervisor transcripts, classified block by block, put the supervisor's prose at one block in seven.

| Session (start) | Supervisor prose | Your directives | Tool calls | Tool results | Injected worker / director mail | Visible blocks | Prose share |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `151fbf3a` (2026-09-18) | 15 | 6 | 52 | 52 | 4 | 129 | 11.6 % |
| `2ebdfca9` (2026-09-11) | 135 | 30 | 308 | 308 | 83 | 864 | 15.6 % |
| `35cbdf1a` (2026-09-10) | 619 | 133 | 1,466 | 1,466 | 478 | 4,162 | 14.9 % |
| Pooled | 769 | 169 | 1,826 | 1,826 | 565 | 5,155 | 14.9 % |
| Pooled, by characters | 125,171 | 56,753 | 926,181 | 3,282,323 | 777,676 | 5,168,104 | 2.4 % |
Table: Pane composition by author. Blocks are content blocks in the Claude Code transcript; 763 hidden thinking blocks are excluded. Source: `measure-pane.py` over the three transcripts, run 2026-09-18 16:05.

Two caveats keep the number honest. The harness truncates tool output on screen, so the character share (2.4 %) overstates the noise; the block share (14.9 %) is the fair proxy for rows the pane prints. And "prose" includes the narration a supervisor emits between tool calls ("checking the worker status"), so 14.9 % is an upper bound on text actually addressed to the operator. Either way the operator's phone receives six or more parts of traffic for every part of message.

<!-- figure: today -->

The captures above are the checked-in fixture states, not a mock: the `from-you` block is the only distinct voice, the one typed reply is styled as a quotation, and the pane text below both carries no addressee at all.

## The reference interfaces

<!-- refcount --> real captures, each judged on what a status-heavy, asymmetric conversation can steal from it. Consumer chat, team chat, AI-assistant chat, ops and status feeds, and six unconventional readers. Click a frame to enlarge it.

<!-- figure: refs-gallery -->

| # | Product | Category | What the frame shows | Does well | Does badly | Stealable pattern |
| --- | --- | --- | --- | --- | --- | --- |
| 01 | Apple Messages (macOS) | consumer | A group chat with sidebar previews, blue and grey bubbles, a sender name over each bubble, a tapback heart and a photo bubble. | A tiny sender label over the bubble; reactions pinned to a corner; sidebar rows carry the last line. | Ownership by colour alone; no state beyond delivered; photos dominate the column. | Corner chips as cheap receipts (`acked`, `merged`); the sender label as a kind label. |
| 02 | WhatsApp (group) | consumer | Incoming bubbles with coloured sender names, in-bubble timestamps, a photo with a reaction count and double blue ticks. | Colour on the name, not the bubble; time tucked inside; read state inline. | Marketing crop; no threading; consecutive messages are not grouped. | Tick-state in the corner of a directive as its delivery receipt. |
| 03 | Signal (Android) | consumer | A dark chat with a centred system notice ("Maya set the disappearing message timer"), a voice-note bubble and in-bubble time and state icons. | The centred low-contrast system line sits inside the flow without looking like a message. | Very dense metadata in every bubble; no reply threading. | The system line is the shape of a status row between turns. |
| 04 | Telegram (public channel, live) | consumer | A live channel feed: bold channel name per post, a bordered link-preview card, a view count and time on every post. | Each post is self-contained with a footer; previews render as bordered quote-cards; long posts stay readable. | One-way; unbounded post height; a busy decorative background. | The bordered preview card as the container for a receipt or a table inside a message. |
| 05 | Slack (channel and thread) | team | A channel with an embedded canvas card, a `WORKFLOW`-badged bot post with an avatar stack and "3 replies", and the thread pane beside it. | The badge separates automation from people; the reply-count row folds a thread; channel and thread side by side. | Placeholder thread bodies; two composers compete on one screen. | A badge on the sender plus a reply-count row: mark the supervisor's turn and fold worker chatter. |
| 06 | Discord (new thread) | team | A thread being created: title field, one starter message (avatar, name, "Today at 11:26 AM", text) and a composer. | The flattest possible message anatomy: avatar, name, time, body on one baseline. | Only one message; a blurry support-article frame; no status affordances. | Name and `Today at HH:MM` on one line with the body under it — the densest readable row. |
| 07 | Linear Inbox | team | An inbox with Priority and Other tabs; two-line rows: issue ID and title, then the reason ("Didier assigned the issue to you"), age and a status glyph. | Row = what + why + state; unread dots; triage tabs with counts. | A list, not a conversation; low-contrast dark render; truncated titles. | The two-line row (what, why, glyph, age) for the Desk queue and the Ledger's list row. |
| 08 | Height (task chat with Copilot) | team | A task's attribute table (Status, Assignees, Lists) beside a chat pane where "Melika to Copilot" and "Copilot to you" label each turn. | Attributes and chat side by side; the AI is a named participant with an explicit addressee. | Third-party review capture; the AI reply is unbounded prose; no receipts. | The `A → B` addressing label — `↩ your 09:56` on an answer. |
| 09 | Claude.ai (shared chat, live) | AI assistant | A shared conversation: a compact right-aligned user bubble, then the reply as a full-width serif document with headings, bullets and code chips. | The assistant's turn is a document, not a bubble; strong type hierarchy. | No timestamps, no state, no sender labels; long replies scroll forever. | Asymmetric turns: short directives as blocks, supervisor output as structured reading. |
| 10 | ChatGPT (shared chat, live) | AI assistant | A shared chat with a "Today 6:55 AM" divider, an "Uploaded a file" chip, a `Show more` truncation, then a reply that opens with a bold verdict before a code block. | Long input collapses by default; attachments are chips; the verdict leads the detail. | Sidebar chrome eats width; no per-message time; no sign of work in progress. | Lead every receipt with a one-line verdict; collapse long payloads. |
| 11 | Cursor 2.0 agent | AI assistant | An agent window: task title, branch, model cards with file counts and +/−, `3 To-dos 2/3`, a status sentence, a "3 Files Edited" list and a diff pane. | Every status is quantified; the diff sits beside the conversation; "Create PR" is one button. | Marketing render; no timestamps; the human's turn is a one-line card, easy to lose. | The quantified receipt: to-do counter, files, +/− per file. |
| 12 | Devin session | AI assistant | A session: the user's directive, `Used playbook: Test`, a collapsible `Worked for 4m 13s +25 −131`, two PR cards, "Done! Full report attached", and "Devin is ready for instructions". | Work folds into one row; PR cards are first-class receipts; an explicit idle line. | Marketing composite; no timestamps; a wide horizontal layout. | Fold the work, show the outcome; an explicit hand-back line. |
| 13 | GitHub Copilot coding-agent PR (live) | AI assistant | A real PR timeline: three Copilot commits with SHAs and red check marks, `Copilot [AI] changed the title`, "finished work on behalf of adamsitnik", then the human's "@copilot please address my feedback". | One-line events with icons; the `AI` badge beside the actor; commits carry SHA and CI state; resolved threads collapse. | Commits, events and comments mix with little grouping; a wide empty gutter. | The one-line event row (icon · actor · verb · object · time) as the receipt row. |
| 15 | Vercel runtime logs | ops feed | A logs table: time to the centisecond, method, status chip, host, path, a message with a count badge, and a filter rail with warning and error counts. | One row per event in fixed columns; repeats collapse into a count badge; live counts in the rail. | Every row has the same weight; the row that matters relies on status colour. | The monospace receipt row and the count badge for repeated statuses. |
| 16 | Linear issue activity | ops feed | An issue's Activity rail: muted one-line system events ("quinn created the issue · 1 minute ago") interleaved with a full comment card and a pinned composer. | Two tiers on one rail: system events as one muted line, human comments as full cards. | System events carry no payload or link; the rail connector is faint. | Two-tier feed: hairline status rows between full turns. |
| 17 | Sentry issue details | ops feed | An issue page: header with counts, a workflow bar (Resolve, Archive, Priority, Assignee), stack trace, and a right-rail Activity with "Assigned to Keith Ryan 10:30 AM" and "4 comments hidden". | State lives in a persistent header; the activity rail is compact and collapses noise. | Six zones compete on one screen; the docs figure carries annotation labels. | A sticky state header over the conversation; an "N hidden" collapser for repeats. |
| 18 | PagerDuty incident timeline | ops feed | A Time / Activity table: "Assigned to Casey Bennett and reopened" with an expanded detail list, "Resolved by …" in green, a tinted key/value block, workflow rows. | A fixed timestamp column; compound events as one expandable row; verbs colour-coded with words. | Newest-first with no phase grouping; human and automation differ only by wording. | The table-shaped timeline with a fixed time column and expandable compound rows. |
| 19 | GitHub PR conversation (live) | ops feed | A merged PR: sticky "Merged" header with the branch pair, review rows with collapsed files, a condensed commit row with a Verified chip and SHA, comments with role badges. | Three densities on one rail; a sticky outcome header; role badges beside names. | Review rows near-duplicate; commit rows are easy to miss; quoted replies eat space. | The condensed commit row (message · chip · SHA) as the merge receipt; role badges. |
| 20 | GitHub Actions job log | ops feed | A failed job: the failed step auto-expanded with numbered log lines and collapsible groups; other steps folded with status icon, name and duration. | Only the failure opens; steps carry status, name and duration; lines are numbered and linkable. | A cropped documentation figure with an annotation box; no timestamps. | Collapsible steps where only the failure auto-expands; numbered lines a verdict can cite. |
| 21 | incident.io (Slack channel) | ops feed | An incident channel: "@incident what's going on?", the APP's bolded impact sentence, a human severity change recorded as `Severity: ~~Minor~~ → Major`, then next-step buttons. | Directive → status → decision → recorded delta → suggestion with buttons, in one flow. | A marketing composite; no timestamps; unrealistically terse. | The inline state-delta line and next-step buttons attached to the supervisor's turn. |
| 22 | Superhuman split inbox | unconventional | A split inbox: tabs with counts (Important 7 · Calendar 3 · Docs 8), rows with sender, label chips, subject and grey preview, a purple bar on the current row. | Tabs with counts turn one inbox into lanes; chips sit inline before the subject. | Marketing crop; no timestamps or unread marks; the right edge truncates. | Lanes with counts for the feed (Needs you · Receipts · Statuses). |
| 23 | HEY Imbox | unconventional | The Imbox: a large title, a "NEW FOR YOU" divider, rows with initials, oversized subjects, sender folded into the preview, orange unread dots. | One band separates unseen from seen; very high contrast; dates only. | Every row looks alike; no threading or state; dates out of order. | A "new for you" divider between unread supervisor turns and the rest. |
| 24 | Things 3 Today | unconventional | Today: a tinted calendar block, two-line to-dos with muted project subtitles, one red `⚑ today` flag, a "This Evening" section. | Two-line rows; today's context as a tinted block; one flag is the only alert; whitespace groups. | No timestamps or ownership; no in-progress state. | One reserved flag for the item that needs the human today. |
| 25 | Notion Mail priority view | unconventional | Priority: collapsible status groups (Important, To-do, Waiting, No status), each row with an unread dot, sender, subject, state glyph and time. | State groups with icons are the primary structure; headers collapse; the glyph repeats per row. | Large gaps; a generic status vocabulary; no preview text. | Group the feed by state with collapsible headers and a per-row glyph and time. |
| 26 | Apple Journal | unconventional | The Journal feed: entry cards with a media mosaic (photo, podcast art, map pin), a bold title, body, a date footer, a "Walk 9 560 steps" tile. | Each entry is a card with a mosaic, title, body and date; system data rendered as tiles inside it. | Half the frame is onboarding; tall cards; no actor, no state. | An evidence mosaic (test-run tile, diff tile, PR tile) at the top of a receipt. |
| 27 | Arc browser | unconventional | Arc's tinted space sidebar — pinned items, a folder, `+ New Tab`, the active tab highlighted, space dots below — beside the page (cropped to remove the site's promotional strip). | The sidebar is the whole navigation: spaces, folders, pinned and active in one column. | No state or counts in the sidebar; folders and tabs look alike. | The active conversation as the highlighted tab of a tinted workspace column. |
| 28 | Zulip #design (live web-public) | team | A live channel chunked by green topic bars ("design › #9312 typeahead…") with messages beneath, a `SHOW MORE` collapser, a TODAY pill, a composer naming the topic. | Every message sits under a topic so one channel reads as many small threads; the composer names where the reply lands. | Repeated headers interrupt flow; a long truncated topic list. | Topic-bar chunking per task thread; a composer that names its target. |
| 29 | Basecamp message board | team | A post with a byline ("Matthew Rogerson · FYI · Notified 24 people"), body with a highlight and a mention chip, a Comments tab beside a "1 reference to this message" tab, and comments with roles after names. | The byline encodes type and audience; roles next to names; comments counted and separated. | Static marketing image; no state or resolution on the post. | A type-and-audience byline (`Receipt · notified you`) and role tags after names. |
Table: References — 28 real captures under `refs/` (`NN-slug.png`), one line each on what it does well, what it does badly, and the pattern worth stealing for a status-heavy agent↔human conversation. Live captures are noted; the rest are vendors' own product or documentation images.

### Patterns worth stealing

- **The one-line event row** — icon · actor · verb · object · time, with a SHA and a check glyph (13 Copilot's PR timeline, 19 a GitHub PR, 21 incident.io's timeline) → the receipt ledger row and the status row.
- **A verdict sentence before the detail** — ChatGPT bolds the answer first (10); Claude renders a reply as a document with headings (09); Vercel and Sentry lead with the outcome line (15, 17) → every receipt opens with a serif verdict sentence, then its ledger.
- **Fold the work, show the outcome** — Devin's `Worked for 4m 13s +25 −131` rows and PR cards (12), Cursor's `3 To-dos 2/3` and per-file +/− (11), ChatGPT's *Show more* (10), a GitHub Actions log's collapsed steps (20) → statuses coalesce into one row; evidence is an attachment that opens; the pane is a link, not a stream.
- **A system line between turns** — Signal's centred low-contrast notice (03), iMessage's date dividers (01), Slack's `3 replies` avatar row (05) → statuses are hairline rows in `ink-muted`, never bubbles.
- **Two-line triage rows** — Linear's *what + why + state glyph* (07), Superhuman's split inbox (22), HEY's Imbox and screener (23), Things' inbox (24) → the Desk queue item, and the Ledger's list row (`● 1 waiting on you · 4 min`).
- **An explicit hand-back** — *Devin is ready for instructions* (12), *Copilot finished work on behalf of …* (13), PagerDuty's acknowledge/resolve states (18) → the working line and the pinned ask with quick replies.
- **A badge on the sender, not a colour on the bubble** — Slack's `WORKFLOW` badge (05), GitHub's `[AI]` label (13), WhatsApp's coloured sender name (02) → the kind eyebrow (`ANSWER · RECEIPT · WAITING ON YOU`).
- **Address the turn** — Height's *Melika to Copilot* label (08), Telegram's per-post footer (04) → `↩ your 09:56` on answers and `Replying to ·` in the composer.
- **Rich content in its own bordered box** — Telegram's link-preview cards (04), Arc's split view (27), Apple Journal's entry cards (26) → evidence tables scroll inside their own box with a sticky first column, and stack on the phone.
- **Time on a spine** — Linear's activity feed (16), PagerDuty's incident timeline (18), Twist's dated threads (29) → the Ledger's timestamp column with elapsed time printed between a directive and its answer.

## The taxonomy

The hub conversation is asymmetric: one human sends short directives and questions; one supervisor sends statuses, verdicts, tables, merge receipts and answers. Chat bubbles equalise every turn. The proposal instead draws the thread as an **annotated timeline**: mono timestamps on a left spine, the elapsed time between a directive and its answer printed on the spine, and a kind eyebrow on every supervisor turn. Six kinds cover everything the supervisor says to the operator; a seventh line, *working*, is state rather than a message.

| Kind | Sent when | Desktop (1280) | Phone (390) | Colour and rule | Lifecycle |
| --- | --- | --- | --- | --- | --- |
| **Directive** (you) | the operator sends | unchanged from today: `action`-tinted block, right inset, delivery state in the header | same, inset by one step | `action` left rule | delivered → replied |
| **Answer** | the supervisor replies to a directive or question | plain reading prose, no rule; eyebrow `ANSWER · ↩ your 09:56`; `+20 s` printed on the spine | eyebrow carries the stamp; spine drops | none — the default voice | appended; closes the directive |
| **Status** | progress with no decision in it | one hairline row; consecutive statuses coalesce into `▸ 3 statuses · latest …` | same row, wraps | `ink-muted` text, hairlines | replaced in place; never notifies |
| **Receipt** | an outcome with evidence: merge, gate, release, close | serif verdict sentence, then a ruled ledger; the decisive row banded `verdict-soft`; source line with a pane anchor | ledger rows stack as term / value pairs; the decisive row keeps its band | 2 px `line-strong` left rule | appended; permanent |
| **Ask** (waiting on you) | the supervisor needs a call | `surface-hero` panel, 3 px `verdict` rule, eyebrow `WAITING ON YOU · 4 min`, serif question, quick replies that prefill the composer | full-width panel; quick replies wrap | the one indigo element on the screen | pinned until a directive answers it; drives the list row and the rail |
| **Blocker** | something is held and needs the operator | `danger-tint` panel, 2 px `danger` rule, `■ BLOCKER` label — never colour alone | same | `danger` + glyph | pinned beside the ask; cleared by a receipt |
| **Evidence** | a table or long output the operator asked for | caption + verdict line + ruled table that scrolls inside its own box with a sticky first column; `open full width ▸` | scroll box, then a full-width sheet with stacked rows | hairlines only | appended; collapsible |
| *Working* (not a message) | always, while the session is live | hollow ring + mono sentence from the session phase and last output time, `Pane ▸` at the end | same, wraps to two lines | `ink-muted`; no spinner, nothing loops | refreshed in place; goes quiet when a turn lands |
Table: The six message kinds and the working line, with a distinct treatment each. Tokens are hub-web/src/tokens.css names; `surface-hero` is the one house token the hub does not map yet.

The four questions the brief set:

- **A long table or merge receipt on a phone.** A receipt's ledger stacks: each row becomes term / value pairs under a `line-strong` rule, the decisive row keeps its `verdict-soft` band and rule (mockup *thread*, 390). Evidence tables scroll inside their own box with the first column sticky, and `open full width ▸` opens a sheet where the same rows stack (mockup *evidence*, 390). The page never scrolls sideways.
- **"Still working" without streaming the terminal.** The working line is a single mono sentence — `Working · holding the train, watching run 33512 · 2 min ago` — derived from the session summary the daemon already publishes (`SessionCardSummary { phase, blocked_on }`, `cas-cli/src/ui/factory/protocol.rs:358`) and the time of the last `Output` frame. It refreshes in place; it is never appended; there is no spinner.
- **What the supervisor is waiting on.** An *ask* is pinned at the bottom of the thread until a directive answers it; the conversation list row reads `● 1 waiting on you · 4 min`, the context rail repeats the question, and the composer is pre-addressed (`Replying to · fix in-train or ship with allowlist?`). Quick replies are real buttons that fill the composer with the operator's usual one-liners; Send stays explicit.
- **Reaching the raw pane.** `Pane ▸` in the header opens the emulator beside the thread on desktop and as a sheet on the phone (mockup *evidence*, 1280); every receipt and blocker carries `pane line N ▸`, which opens the pane scrolled to the row the message was emitted at. The pane is the escape hatch; the thread is the default.

### Where each kind lives in each option

The components above are shared by all three options — the same ask panel, receipt ledger, blocker tint, status row and working line. The options differ in where those components live and what the operator reads first.

| Kind | A · Ledger | B · Desk | C · Brief |
| --- | --- | --- | --- |
| Directive | inset `action` block on the timeline | a queue item while unanswered (`Your question · awaiting`), then paired with its answer in Done | a dated margin note on desktop, an inline note on the phone, anchored to what it changed |
| Answer | reading prose with `↩ your 09:56` and `+40 s` on the spine | resolves the question item; shown as a pair in Done | the Answers section: question line, answer prose, `answered 09:57 · 40 s` |
| Status | one coalesced hairline row between turns | the item's *Since then* row and the session rail | the Status section: latest line plus `▸ 3 earlier`; the Now line |
| Receipt | verdict line and ledger on the timeline | a Done item; opened with the directive that led to it | an Outcomes row (`09:47 · sentence · details ▾`) that expands to its ledger |
| Ask | sandstone panel pinned at the foot of the thread; list row and rail repeat it | the queue item, selected by default, with a scoped answer box | the Decisions-needed section |
| Blocker | danger panel on the timeline, pinned beside the ask | a queue item, and attached under *Why* on the ask | the Held callout under Now |
| Evidence | inline table with a sticky first column; a sheet on the phone | an attachment on the item; a sheet on the phone | inside the outcome's details |
| *Working* | the line above the composer | the session rail on desktop; the item footer on the phone | the first line under Now |
Table: Kind × option. Every option shows every kind; the treatment is shared, the placement is not.

## Three ways to draw the conversation

Three surfaces, drawn on the same session at the same minute so the layout carries the comparison: one directive, an answer, a merge receipt, three statuses, a question and its answer, a blocker, and one ask that has waited four minutes. Each option names the hub-mobile navigation shell it assumes (`docs/design/hub-mobile/index.md`); the options are about the conversation surface, not the navigation around it.

### A · Ledger — the conversation as a ruled timeline

The thread is an annotated timeline: mono timestamps on a left spine, elapsed time printed under them, a kind eyebrow on every supervisor turn, the operator's directives inset as today. The one unanswered ask is the only indigo element and stays at the foot of the thread until a directive answers it. Statuses coalesce into one row; the working line sits above the composer; `Pane ▸` opens the emulator beside the thread on desktop and as a sheet on the phone. **Shell:** hub-mobile *04 · Supervisor conversations* — thread list, then a full-width thread with an addressed composer; on desktop the list, the thread and a context rail. **Protocol beyond the shared `kind` field:** none; coalescing and pinning are client-side.

<!-- figure: option-ledger -->

### B · Desk — a queue of what needs you

The surface is an inbox, not a thread: every ask, blocker and unanswered question across every paired machine, oldest first. Opening an item shows what the supervisor attached — the blocker, the receipt, the statuses since — and an answer box scoped to that item. Receipts and answered questions move to Done, where a receipt opens with the directive that led to it and its evidence. The session rail carries the working line. **Shell:** hub-mobile *01 · Decision inbox* — requests first, system and supervisor on every row, one response action. **Protocol beyond `kind`:** a durable item lifecycle — an ask stays open until a send carries `reply_to` = its notification id, and every device must agree, so the daemon marks it answered (the `acked_at` column on the operator-targeted `prompt_queue` row) rather than the browser.

<!-- figure: option-desk -->

### C · Brief — a page the supervisor keeps current

One page per session — Now, Decisions needed, Outcomes, Answers, Status — where each new statement of a kind replaces the previous one. The operator's directives are margin notes on desktop and inline notes on the phone, each pointing at the section it changed; `Earlier ▾` opens the drawer of replaced statements, struck through. **Shell:** hub-mobile *05 · The daily brief* — a dated reading page with decisions, changes and evidence links; on desktop the brief beside its source. **Protocol beyond `kind`:** replace semantics — a `supersedes` field (or one slot per kind per session) so a new status or working statement retires the last one durably; receipts and answers accumulate; asks close as in Desk.

<!-- figure: option-brief -->

### Choosing

| Option | The first screen answers | History | On a phone | Protocol delta | Cost of being wrong |
| --- | --- | --- | --- | --- | --- |
| **A · Ledger** | *what did the supervisor say to me, in order?* | complete, in place, elapsed times printed | a familiar thread; ledgers stack | one field: `kind` | low — it degrades to today's thread |
| B · Desk | *what needs me right now?* | in Done, per item | a list, then one item | `kind` + a durable ask lifecycle + cross-machine aggregation | medium — an item model that drifts from the pane misleads |
| C · Brief | *what is true right now?* | in a drawer, struck through | one reading page | `kind` + replace semantics held server-side | high — a latest-state page hides how a decision was reached |
Table: The three options against the questions an operator opens the hub with. The recommended row is banded in the HTML.

The Ledger is recommended. It is the surface the operator described in one sentence — *my messages and the supervisor messages to me* — it keeps every answer beside its question and prints how long the answer took, it reads as an ordinary thread on a phone, and it needs one new field on an envelope the hub already carries. Desk's two strengths — a pinned ask and a cross-machine *needs you* count — are folded into the Ledger's list row and context rail; Brief's *Now* line is the Ledger's working line. Desk stays the candidate if real use turns out to be decision-only check-ins; Brief stays the candidate if sessions come to run for days and the thread grows past a screenful of receipts.

## The seam

<!-- figure: seam-timeline -->

### How an operator message reaches the supervisor

The composer in `hub-web/src/main.ts:2128` submits through `submitSupervisorMessage()` (`main.ts:1867`), which sends one WebSocket frame — `{ SendMessage: { client_ref, target, text, summary, urgent: false, attribution } }` built at `hub-web/src/supervisor-message.ts:8` — on the machine socket (`hub-web/src/connection.ts:826`). The browser refuses to send without the `message-send` scope (`supervisor-message.ts:110`; declared at `hub-web/src/pairing-scopes.ts:6`) and a session lease (`supervisor-message.ts:120`). The hub's `handle_client_message` (`cas-cli/src/hub/server.rs:850`) checks scope and lease together (`server.rs:866`), then overwrites the client's attribution with server-minted identity (`server.rs:883`; `operator_verified: true` at `:919`) before forwarding to the daemon (`server.rs:890`). The daemon writes a `prompt_queue` row (`cas-cli/src/ui/factory/daemon/runtime/delivery.rs:537`; schema at `crates/cas-store/src/prompt_queue_store.rs:349`), drains it (`queue_and_events.rs:3702`), prepends the `[cas #N operator <name>@<device> verified …]` header built at `cas-cli/src/mcp/tools/service/agent_search_system/message.rs:97`, and injects it into the supervisor's harness — `mux.inject` at `delivery.rs:898`, or the Claude-teams inbox at `delivery.rs:879`.

### How the supervisor reaches the hub

Two streams. The SSE stream at `/v1/events` (`server.rs:122`) carries only lifecycle kinds — `MachineEventKind` at `cas-cli/src/hub/events.rs:17` is `SessionAdded, SessionRemoved, PaneAdded, PaneExited, PaneRemoved, DaemonDisconnected, ControllerChanged, DaemonError`. Pane content travels on the pty WebSocket channel: the hub proxies the daemon's `DaemonMessage` frames verbatim (`cas-cli/src/hub/connector.rs:367–391`), the browser writes `Output` bytes into a Ghostty surface (`main.ts:535`), and `ConversationView.update()` (`conversation-view.ts:49`) turns emulator rows into paragraphs. There is no transcript file and no tmux capture in the path.

One typed lane already exists. `DaemonMessage::OperatorReply` (`cas-cli/src/ui/factory/protocol.rs:297`) carries `OperatorReplyPayload { schema_version, reply_to, message, summary, device_id, operator_label }` (`protocol.rs:70`); the browser receives it at `connection.ts:987`, stores it in `ConversationHistory.reply()` (`hub-web/src/conversation-history.ts:38`), and the one branch at `conversation-view.ts:82` — `event.kind === "send" ? "from-you" : "from-supervisor"` — renders it. The supervisor produces it with `mcp__cas__coordination action=message target=operator in_reply_to=N` (`cas-cli/src/builtins/skills/cas-supervisor/references/reference.md:30`).

### Why the hub cannot know today

The guard at `message.rs:894–906`: `target="operator"` is accepted only from a registered supervisor **and only with `in_reply_to`**, and the prior row must be a verified Commander message aimed at this supervisor (`:934`, `:943`, `:963`). Routing follows the same rule — `peek_operator_replies` selects `lower(target) = 'operator' AND recipient_device_id IS NOT NULL` (`crates/cas-store/src/prompt_queue_store.rs:4389–4407`), and the device is stamped from the message being replied to (`message.rs:1019`). So the supervisor can *answer* the operator, and nothing else: a receipt, a status, an ask or a blocker the operator did not first ask about has no lane, and the hub falls back to mirroring the pane. Attention has no such state either: `MACHINE_EVENT_TEMPLATES` (`hub-web/src/attention.ts:63`) knows `awaiting_merge`, `pane_exited`, `daemon_disconnected` and friends, and no `waiting_on_operator`.

### The minimal protocol change

| Field or seam | Change | Where |
| --- | --- | --- |
| `OperatorReplyPayload` | add `kind: OperatorTurnKind` (`answer · status · receipt · ask · blocker · evidence`) and make `reply_to: Option<i64>`; bump `schema_version` 1 → 2 | `cas-cli/src/ui/factory/protocol.rs:70–78` |
| `DaemonMessage::OperatorReply` | carry the same two fields; the hub gate `operator_reply_allowed` is unchanged | `protocol.rs:297–306`, `cas-cli/src/hub/server.rs:751` |
| The guard | keep supervisor-only; require `in_reply_to` only for `kind = answer`; for the other kinds resolve `recipient_device_id` from the most recent verified Commander row for this session (the stamp `stamp_recipient_device` already reads), and, when there is none, to every device holding a session lease | `message.rs:894–906`, `:1019` |
| `prompt_queue` | additive `kind TEXT` column, migrated like `recipient_device_id`; `peek_operator_replies` selects it | `crates/cas-store/src/prompt_queue_store.rs:489`, `:4389` |
| MCP and CLI surface | `action=message target=operator kind=<kind>`; `kind` sits beside `in_reply_to` in the schema; CLI parity per the surface checklist | `crates/cas-mcp/src/types.rs:993` |
| hub-web | `ConversationHistory.reply()` stores `kind`; the branch at `conversation-view.ts:82` becomes a six-way class switch (`from-supervisor kind-<kind>`); statuses coalesce client-side | `hub-web/src/conversation-history.ts:38`, `conversation-view.ts:82` |
| Attention | add `waiting_on_operator` (warning, action `reply`) to the template table so the list row and rail read from the same item; cleared when a `send` answers the ask | `hub-web/src/attention.ts:63`, `main.ts:522` |
| Working line | no protocol change: `SessionCardSummary { phase, blocked_on }` plus the last `Output` frame time | `protocol.rs:358`, `hub-web/src/types.ts:79` |
Table: Proposal ledger — the smallest set of changes that lets the hub know which turns are for the operator. Nothing here is implemented.

### Telling the supervisor to use it

The supervisor skill already says how to answer a verified operator message (`cas-cli/src/builtins/skills/cas-supervisor.md:33`; `references/reference.md:23–33`). The rule grows by one sentence, mirrored verbatim in the Codex and Grok trees (`cas-cli/src/builtins/codex/skills/cas-supervisor.md:33`, `cas-cli/src/builtins/grok/skills/cas-supervisor.md:33`, and each flavour's `references/reference.md`), and pinned by the same substring tests that pin the rest of the skill:

```
Anything you would tell the operator goes through the hub as a typed turn, never
as pane prose: mcp__cas__coordination action=message target=operator kind=<answer|
status|receipt|ask|blocker|evidence> summary="…" message="…" (in_reply_to=N for an
answer). One receipt per merge, gate or release; one ask per decision you need; a
status at most every ten minutes; the pane is not a message.
```

The worker skill is untouched: workers never address the operator (`cas-worker.md:40`).

## Critique

Scored with `cas-ui-craft/references/critique-rubric.md` (1–5; 0 is a mechanical defect). The three options are scored from their 24 renders in `mockups/png/` and the strict visual-qa run over their six states (light and dark, 1280 and 390); the report is scored from the strict visual-QA run and the four headless renders named in Provenance.

| Surface | Distinctiveness | Fit to argument | Hierarchy | Craft | Accessibility |
| --- | --- | --- | --- | --- | --- |
| **A · Ledger** | 4 — timestamps and elapsed time on a spine, a kind eyebrow per turn, one sandstone ask | 5 — the thread is the operator's own sentence: my messages and the supervisor's to me | 4 — the ask is the only indigo element; the receipt ledger competes with it at 1280 | 4 — strict visual-qa PASS on both states in light and dark; no horizontal overflow at 390; receipts stack | 4 — token pairs only, quick replies are buttons, the blocker carries a glyph and a word |
| B · Desk | 4 — a ruled queue with the selected item in sandstone; Done as a ledger | 3 — answers *what needs me*, not *what did you say to me*; the conversation is a click away | 5 — one selected item; everything else a step down | 4 — strict visual-qa PASS on both states in light and dark; the phone header stacks into two rows | 4 — same pairs; the queue rows are 44 px targets |
| C · Brief | 5 — a page with dated margin notes is unlike any chat | 3 — state, not conversation; how a decision was reached lives in a drawer | 4 — Now, then Decisions; the outcome ledger competes with the ask | 4 — strict visual-qa PASS on both states in light and dark; margin notes fold inline at 390; the history drawer becomes a sheet | 4 — replaced statements are labelled *replaced*, not only struck through |
| **This report** | 4 — serif verdict on sandstone over a waffle whose fifteen indigo squares are the argument; the seam drawn as a timeline; options as paired plates in the hub-mobile format | 4 — the waffle is the claim (a thin band of message in a field of traffic); the choice among options is carried by the comparison ledger, not the hero | 4 — verdict, why, figure, rule, then everything a step down; the document title sits above the eyebrow as a muted line | 4 — `node scripts/visual-qa.mjs --strict` PASS: light and dark at 1280 and 390, 0 findings, 0 allowlisted; 844×390 without overflow; print reflows the hero to one column; plates need enlarging at 390 | 4 — JS-off text identical (51,103 characters), one `h1`, `details` open in markup, 0 external requests, the viewer dialog is keyboard-reachable; physical phones untested; 2.3 MB is slow on a weak link |
Table: Rubric scores per option and for this report. Floor for a public surface: distinctiveness, fit and hierarchy each ≥ 4, no 0. The Ledger and the report hold the floor; Desk and Brief fail *fit* by design — they answer a different first question — and are kept as live alternatives, not rejected work.

## Provenance

- Worktree `factory/calm-stork-91`, analysis commit `f9f8e60fb505` (main tip at branch time), 2026-09-18.
- Pane measurement: `python3 docs/design/hub-messaging/measure-pane.py <three transcripts>` over the Claude Code transcripts of supervisor sessions `151fbf3a`, `2ebdfca9`, `35cbdf1a` in the operator's config directory; only counts left the machine, no content.
- Today captures: `node docs/design/hub-messaging/capture-today.mjs` builds `hub-web/fixtures` with Vite and renders `conversation-replied`, `conversation` and `transcript` at 1280×800 and 390×844, light and dark (12 PNG under `refs/today-*.png`).
- Reference captures: `node docs/design/hub-messaging/capture-refs.mjs refs/manifest-a.json` and `refs/manifest-b.json`; receipts with URL, mode, viewport and time in `refs/manifest-*.receipts.json`. Every frame is a real render or a vendor's own product image, saved as PNG; nothing is hotlinked. Captures were taken at device scale 2 and downscaled to at most 1,600 px wide and quantized to 256 colours before commit (`shrink-images.py`) to keep the repository small; the receipts record the original capture sizes.
- Option renders: `docs/design/hub-messaging/mockups/{ledger,desk,brief}.html` on `hub-web/src/tokens.css` plus `mockups/shared.css`, rendered by `mockups/render.mjs` at 1280×800 and 390×844, light and dark, two screens each (24 PNG under `mockups/png/`). Screen states are CSS-only (`#state-…` targets), so the mocks need no JavaScript. Each of the six states passed `node scripts/visual-qa.mjs --strict` in light and dark at 1280 and 390 (receipt `mockups/visual-qa.md`; screenshots under `/home/pippenz/.cas/artifacts/cas-c3c5/visual-qa-mockups/`).
- Seam cites: every `path:line` in this report was read in the worktree on 2026-09-18; the eleven load-bearing lines are listed in the seam timeline.
- Report build: `python3 docs/design/hub-messaging/build.py` converts this markdown and embeds the images as WebP data URIs; the HTML makes no network request.
- Visual QA: `node scripts/visual-qa.mjs docs/design/hub-messaging/2026-09-18-hub-messaging-study.html --strict --artifact-dir /home/pippenz/.cas/artifacts/cas-c3c5/visual-qa` — PASS, 0 findings, 0 allowlisted; the receipt is `visual-qa.md` beside this file, screenshots and JSON under the artifacts root. Extra checks by `qa-extra.mjs`: JS disabled (same text, `details` open), 0 external requests, print media and A4 PDF, 844×390 landscape without horizontal overflow; four review renders by `render-report.mjs`.

<!-- figure: main-end -->
