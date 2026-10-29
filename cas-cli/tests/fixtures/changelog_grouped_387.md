# Changelog

## [9.99.1] - 2026-10-28

### Fixed — steadier Commander connections

- **A connection that recovers** — Was: slow status reads and repeated retries
  could make Commander flap between connected and disconnected or stay waiting
  after a machine recovered. → Now: health checks run apart from slow catalog
  work, retries are bounded and sessions retain their own backoff. The
  measured recovery check detected a drop in about 2 seconds and recovered in
  about 4 seconds.
- **A smaller, faster sign-in store** — Was: expired connection tickets
  accumulated, and authenticated requests repeatedly locked, parsed and
  rewrote the whole store. → Now: tickets are pruned, identity reads are
  cached and audit writes are batched and rotated. In the measured check, the
  store shrank from 7.96 MB to 88 KB and lock-wait p95 fell from 504 ms to
  0.07 ms. Revoked access still takes effect on cached streams.
- **Connection state means what it says** — Was: machine startup could look
  like a failed conversation, a brief authentication delay triggered needless
  recovery, and refused access could leave Working on. → Now: startup stays
  Connecting, one transient deadline gets a bounded grace period, and lost or
  refused access clears Working immediately.

### Added — more readable Commander conversations

- **Readable tables and callouts** — Was: prose tables squeezed words into
  one-character columns, inline formatting showed literally, and headings and
  caveats had little hierarchy. → Now: whole words wrap inside tables, bold
  and code render inline, heading levels remain distinct, and Note, Warning
  and Caution have readable treatments.
- **Code you can copy and wrap** — Was: long code blocks were difficult to
  read and copying required selecting the contents. → Now: code frames have
  Copy and Wrap controls. Splitting a long message preserves fenced source,
  and code resembling a short status reply stays code.
- **Attachments stay with their previews** — Was: previews sat apart from file
  chips, failures reserved blank panels, and your sent images appeared as
  server paths. → Now: files and previews form one group, failures stay
  compact and retry after reconnect, and sent files remain thumbnails or named
  chips after reload and on another device.
- **More faithful Show work detail** — Was: background commands and recorded
  outcomes could lose final output or carry incomplete reasoning and change
  detail. → Now: background commands retain their final output, typed work
  events carry reasoning and edit facts when available, and older event
  formats still render.

### Changed — projects and progress in Commander

- **Find the right conversation** — Was: the left column mixed sessions into a
  flat list with little project or account context. → Now: project cards group
  conversations, Needs you brings decisions forward, and machine chips and
  filters narrow the view. Rows show the reported harness, account and
  connection state. Collapse is remembered, search keeps the grouping, and
  phone controls stay reachable.
- **See progress without the whole board** — Was: Tasks & progress dumped the
  project board into a fixed narrow panel with unavailable controls taking up
  space. → Now: the selected release leads with merged/total, merge rate and
  local last-merge time. Needs you, In progress and Ready are scoped to the
  current session's open releases; All tasks opens the wider board. Current
  work, liveness and last activity say when they are not reported. Permissions
  collapse to one line with a re-pairing link. Pointer or keyboard resizing is
  remembered per browser, and scrollbars follow the theme and forced colors.
- **Phone feedback stays usable** — Was: the taller task panel could hide
  results or let feedback overlap a task row. → Now: new feedback appears
  after the tasks, stays readable and clickable, and does not pull you down
  again on each metadata refresh.

### Fixed — conversation history you can trust

- **A clear session ending** — Was: leaving with `/exit` could leave a stale
  work panel and no clear outcome. → Now: a dated Session ended receipt
  retains the stop outcome and anything still running. The row, header and
  live announcements agree, and the composer explains why it cannot send to
  that session.
- **Terminal input belongs in the conversation** — Was: a terminal-typed
  message could arrive late, out of order or without matching the reply it
  prompted. → Now: terminal input is captured and ordered by when it was
  entered, with stable identity linking it to the conversation history.
- **Sent messages keep their identity** — Was: a message accepted into durable
  history could still show Not confirmed, or be confused with another send
  carrying the same text. → Now: history first matches the original client
  reference, keeping delivery and reply state faithful across replay.

### Changed — Claude usage, accounts and Mac support

- **Claude usage comes from Claude** — Was: usage reporting read a
  subscription credential and called the private usage endpoint directly. →
  Now: Cassy reads the official Claude tool's native statusline output instead
  of making that direct subscription-authenticated request.
- **Choose an account with useful context** — Was: the picker emphasized the
  shorter usage window, included unusable entries and left its recommendation
  unclear. → Now: weekly usage comes first, usable accounts have one readable
  row and the recommendation is clear. An unknown or empty weekly window stays
  unknown, rather than implying unused capacity.
- **Type ahead without losing the answer** — Was: text entered in the account
  picker could stay queued or be misread at the duplicate-account question. →
  Now: buffered text, editing keys, newline and end-of-input retain their
  meaning in the existing email prompt and confirmation.
- **Mac logins and startup work as expected** — Was: profile spelling could
  miss a Keychain login, readable Mac credentials could be refused, and Codex
  startup printed raw handoff walls or invalid hook output. → Now: profile
  paths normalize consistently, readable credentials are accepted, and native
  Codex startup uses compact handoffs and valid output. Claude command help
  works without a default login; an empty inherited Codex home is treated as
  unset.
- **Portable Mac push checks** — Was: the Commander distribution check
  depended on GNU-only file commands. → Now: the same check works on macOS and
  Linux.

### Fixed — task delivery, diagnostics and safe local operations

- **Review follows the actual delivery** — Was: close could classify
  accumulated unrelated changes as part of a delivery, confuse independent
  review evidence with the original proof, or accept a stale correction tip. →
  Now: classification uses the recorded task start and delivery commits,
  review evidence keeps its own identity, and the explicit final receipt must
  match the correction and destination.
- **Merged work stays recognizable** — Was: renames, approved deletions,
  adoption after restart and identical-tree corrections could make merged work
  look dropped or stale. → Now: content checks retain accepted identities,
  branch rebindings follow the current target, and verified landed merge-queue
  proof can be reused where it applies.
- **Review and completion notices arrive** — Was: review started after a
  background merge could silently expire, completion relays could disappear
  behind later state changes, and delayed notices looked current. → Now:
  review requests are delivered, durable completion relays survive later
  transitions, delayed notices say they are delayed, and obsolete review
  rounds retire after a waiver or replacement.
- **Less repetition, clearer task state** — Was: routine reads repeated
  acknowledged bodies, lost structured blocker state or called held work
  stalled. → Now: recipient references preserve intent across restart without
  replaying settled bodies, reads retain stored execution state and source
  branches, and held work does not raise false alerts. Dependency readback
  distinguishes preparation from delivery gates.
- **Shared evidence is retrievable** — Was: committed Cloud artifacts could be
  unavailable on another machine or depend on a local task lookup. → Now:
  shared metadata and synced proof references resolve through visible Cloud
  scopes, with actionable refusals and no authenticated redirect following.
- **Doctor reports partial checks honestly** — Was: a bad lookup could skip
  remaining repositories or claim no shipped issues; task references could be
  read as repositories and a missing queue column as database contention. →
  Now: checks start with the configured repository, keep partial results and
  name unavailable checks. References and schema errors are classified
  correctly. Expired-reminder history also handles mixed timestamp formats.
- **Guards understand literal clipboard text** — Was: copying the operator's
  own write-roots command could be refused while some expanding heredoc bodies
  escaped inspection. → Now: literal clipboard content is data, expanding
  bodies are inspected, missing literal bodies fail closed, and memory writes
  are scoped to the active profile and bound project.
- **Targeted cleanup and test commands** — Was: a global scan cap could
  prevent inspecting one retired build cache, cleanup missed child processes,
  and resumed sessions could not stop their own registered server. → Now: a
  named retired cache can be reclaimed independently while retaining embedded
  proof, process-tree cleanup completes, and server ownership survives a
  resumed session. Named CommonJS browser tests are accepted, and automatic
  push checks use the branch being pushed.

### Changed — the hosted Commander follows the release

- **Released Commander deploys automatically** — Was: publishing the runtime
  did not update the hosted Commander. → Now: the release workflow deploys the
  released bundle after publication. A pinned deployment CLI receives no
  GitHub token, and transient post-promotion read failures are retried before
  rolling back a good deployment.

