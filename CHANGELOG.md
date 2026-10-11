# Changelog

All notable changes to CAS are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — a task one machine works is not started on another (cas-5f28)

- When the project is logged in to Cassy Cloud, `task start` and
  `task action=claim` also claim the task in the cloud. A session of the same
  repository on another machine (or in another clone) that tries to start it
  is refused. The refusal names the holder and its machine, and shows how to
  message them. `force=true` takes the task over and says so.
- Claims are scoped to the repository, so the same task id in two of your
  repositories never collides.
- Claims are released on close, release, reset, transfer, park and cancel.
  The daemon heartbeat renews them every two minutes.
- When the holder's machine stops, its claim runs out and a peer may start
  the task with a stale-claim warning. When the cloud is unreachable, the
  local lease applies, with a "peers not checked" warning.
- Two sessions working children of the same epic (subtask start, epic start,
  `focus_epic`) are told about each other; nothing is refused.
- `spawn_workers` warns when a peer holds the task it pre-assigns.
- `task start` of a pulled task already in progress under an assignee who is
  not on this host names that assignee.

## [3.50.0] - 2026-10-10

### Changed — many agents on one machine (#1165)

- The factory daemon's main loop no longer waits on database or file locks.
  Each pass gets a 50 ms budget for store work; CI-watch reads and writes, the
  lifecycle outbox, attention relays and the task dialog moved to an ordered
  background store worker, and the task store is opened once per process
  instead of reconciling under a lock on every open. `loop.json` now reports
  the slowest, p99 and over-100 ms pass counts per window.
- Opening the task store never waits on `task-sync-intents.lock`. The lock is
  a shared per-process lease, mutations serialize per task, and the
  background reconcile takes a bounded write lock and defers when the store
  is busy. Reconciling runs once per process at open, then every 60 seconds.
- Shared database connections release their process-wide lock between write
  attempts, so one blocked writer no longer stalls every other caller for up
  to 31 seconds. History indexing takes the write lock up front and leaves
  unchanged search rows alone.
- Each project keeps one code index. Only the main checkout's Cassy process
  indexes, reconciles and purges it; factory workers and linked worktrees read
  it. Copies left by removed worktrees are purged in bounded batches at
  start-up and after worktree removal. `cas doctor` lists file counts per
  repository under Indexes and `--fix` purges stray copies. Deleting a file's
  symbols and finding callers use indexes instead of scanning the whole
  table, and only the main checkout's process computes code embeddings.
- A Codex worker starts one `cas serve`, not two. Cassy disables any other
  Cassy server entry in the Codex configuration it launches with, and
  `cas init`/`cas update` keep a single entry.
- The factory loop's two-second refresh no longer reads the store on the
  loop. It used to read every task, agent and recent event there, then read
  them again before delivering prompts, which cost 100–400 ms every few passes
  with many agents. Both reads now run on a background reader with its own
  database connection, and the loop applies the finished snapshot on a later
  pass: panels lag by at most one refresh, and prompts are still checked
  against a read taken after the change was detected. Every store respects
  the 50 ms pass budget when it takes its shared database connection, instead
  of waiting on another thread's. `loop.json` adds `phase_latency`: for each loop phase, how often
  it ran, its slowest run and how many runs took 100 ms or more, with the
  refresh split into its steps.
- `scripts/many-agent-load.py` reproduces the many-agent stall on a scratch
  copy of a project database and checks the latency targets; see
  `cas-cli/docs/MANY_AGENT_LOAD_HARNESS.md`.

### Changed — bounded storage

- High-volume telemetry events (supervisor injections, file edits,
  heartbeats, subagent events) are deleted after
  `factory.event_telemetry_retention_days` (default 14; 0 keeps them).
  Task, commit and verification history is kept. The main checkout's Cassy
  process runs the cleanup every 15 minutes in batches of at most 1,000 rows,
  whether or not the machine is idle. An identical retry of one message
  delivery is recorded once, and a retry that delivered nothing new is
  recorded as `reoffered` rather than `ok`.
- Stored prompt transcripts are trimmed after
  `factory.prompt_transcript_retention_days` (default 7); the prompt row and
  its links stay. Delivered message-queue and supervisor-queue rows are
  removed after their retention windows
  (`factory.supervisor_queue_retention_days`, default 14) in the same batched
  cleanup.
- The message-queue cleanup also works on a store created by `cas init`. The
  column it reads to tell whether a supervisor notification was delivered is
  now added by a migration; before, only the supervisor queue's first use
  added it, and until then every cleanup failed and removed nothing.

### Added — Violet push-wake follow-ups

- A Slack message that mentions people and not Violet now wakes the
  supervisor as a hand-off for those people (`addressed="human"`), with the
  ids it names, so it is left for them.
- `cas factory status` shows each Violet channel watch (age, last human
  message, next check), the most recent stop and its reason, and the Slack
  relay's health; `--json` includes them. `cas doctor` adds a `violet wake`
  row that warns after repeated relay failures or when watches are active
  but nothing has been claimed for five minutes.
- `cas integrate violet --channel <name|id>` maps a Slack channel to this
  project so Violet activity there wakes it. Violet must be in the channel
  first. `--channel-replace` takes a channel over from another project and
  `--channel-remove <id>` removes a mapping.
- The violet skill explains how to handle a Slack wake: read the thread
  first, leave hand-offs to the named person, and reply at most once.

### Fixed — Violet setup (#1164)

- A project `.cas/proxy.toml` block that pointed at the same Violet hub as
  the machine, but named another machine's token, kept `cas integrate violet`
  reporting stale forever. The block is now dropped and this machine's token
  is used; a block for a different hub (such as staging) is kept.
- Violet advice no longer tells you to run `cas login` for a hub token it
  cannot mint. A rejected token names the variable and how to get a new one,
  and a missing project-override token names the file that asks for it.
- Each Claude Code entry in the receipt shows whether the hub accepted it, so
  a rejected token is no longer reported as merely current. Other Claude
  profiles on the machine that already use Violet are repaired too, replacing
  a pasted token or retired variable names with env references.

### Added — write access outside the worktree (#1169)

- The operator can let a project's agents write to folders outside their
  worktree. `cas config set factory.write_roots "~/a,~/b:create+edit+delete"`
  sets project roots; `cas config grant-write --task <id> --path <dir>
  --reason "<why>"` grants one task until it closes, and `cas config
  revoke-write` removes it. Each root allows only its modes (create and edit
  by default, delete when named). Every write allowed this way is logged, and
  a refusal lists the roots in effect.
- Grants can also come from Commander on a paired device with
  `factory:manage`. The hub records the grant itself with the device as its
  source, notes it on the task and posts a receipt to the supervisor. The
  Write access panel asks for confirmation before sending.
- Roots and grants are stored in `.cas/operator/write-policy.toml`, never in
  `config.toml`. Only the operator can change them: the commands refuse to run
  inside an agent session or a factory worker and need a terminal and a typed
  confirmation, and agents are refused when they write under `.cas/operator/`
  or run the commands. This is a guardrail, not a security boundary: agents
  run as the operator's user, so a deliberately hidden write is out of reach.
- With no policy file the workspace contract is unchanged. Once one exists,
  `sed -i` / `perl -i` targets and `mv` sources are judged as edits too.

### Fixed — MCP proxy (#1168)

- An MCP upstream that drops after a transport error now reconnects on the
  next call once its backoff is due. It used to stay "absent" for the rest of
  the session, with advice to restore a credential that was fine.
- Every upstream call has a client-side timeout, 90 seconds by default
  (`call_timeout_secs` in `proxy.toml`), so a hung upstream no longer holds a
  call for about 1,000 seconds. A timed-out connection is dropped and
  reconnects like any other failure.
- `proxy_health` reflects what calls actually see: whether the upstream is
  connected, its last success, and its last failure. It is updated as soon as
  a call changes that state.
- The "absent" message names the real next step: restore a missing or
  rejected credential, install a missing executable, or wait for the
  automatic reconnect (or restart) after a dropped connection.

### Fixed — Cassy Cloud conversation

- A supervisor's answers to questions typed in its terminal now appear in
  the Commander and Cassy Cloud conversation, each under the question it
  answers. Before, answers were mirrored only after a message from a paired
  device, so a conversation driven from the terminal showed the questions and
  never the answers. Answers to earlier terminal questions are filled in from
  the recent transcript, once each, and relayed machine prompts are never
  treated as questions.
- Tapping a conversation in Cassy Cloud opens it every time. Before, a live
  update between press and release could lose the tap or open another
  conversation: grouped rows re-sorted under the finger, and a session
  starting or ending rebuilt the list. The list now holds the pressed row for
  the whole gesture, and an older load never replaces a newer selection.

### Added — Cassy Cloud app icon and install

- A new app icon, a violet-gradient tile with the Cassy ribbons, replaces
  the pale flat mark. Cassy Cloud now ships favicons (16, 32 and SVG), a
  180 px Apple touch icon and a web app manifest with 192 and 512 px regular
  and maskable icons, so it installs as an app on a phone or computer.
- In the installed app, headers and sheets clear the iPhone status bar and
  notch.

### Fixed — visual QA

- `--strict` no longer passes on a page that shows only a loading spinner
  (#1159). It waits up to 5 seconds (`--ready-timeout-ms`) for a rendered
  surface, or for `--ready-selector`, and fails with `page-not-ready` if the
  page never renders. A visual allowlist cannot suppress that.
- Text that fades in after the page loads is no longer reported as invisible
  (#1158). The page is measured once it has stopped changing and animating for
  half a second, at most 3 seconds; text that stays invisible still fails.
- Scoped visual-QA comparisons accept reports that list issues per render
  (`renders[].issues`) as well as a single findings list (#1166).

### Fixed — closes, delivery and workers

- A reviewed drop is accepted when the superseding commits landed before the
  task's delivery anchor (#1160).
- A no-code task with no work target closes without a branch when no commit
  names it (#1167).
- A worker busy in one long turn no longer triggers repeated delivery-stall
  alerts (#1163). Its replies and notes count as activity, and stall alerts
  for one worker are combined per five-minute window.
- `env -u VAR cargo …`, `env -S` and `sudo` value options no longer let a
  worker's cargo command bypass the build guard.
- Read-only git checks no longer take the index lock, so a killed check
  cannot leave a stale `index.lock`; a worker may remove a stale lock in its
  own worktree.
- Cloud sync names the project that already owns a task rejected as
  `duplicate_of_other_project`, with the fix: fold the alias and run
  `cas cloud project adopt-aliases`, or retire the local copy.

### Fixed — SQLite

- Two connections to the same database closing at the same moment in one
  process could hang forever, a deadlock in the bundled SQLite 3.51.1. Cassy
  now bundles SQLite 3.51.3 (rusqlite 0.39), which fixes it and also carries
  SQLite's WAL-reset corruption fix.

### Fixed — release train

- The integration branch is remembered by name, so renaming the checkout no
  longer breaks assembly, and an epic already merged to `main` by PR can be
  assembled from `main`. That path always runs the full gate.
- The release gate's scratch space defaults to the checkout's filesystem, and
  preflight checks that the release-report renderer's browser starts before
  anything is published.

## [3.49.0] - 2026-10-10

### Added — Violet push-wake

- When someone @-mentions Violet, or replies in a thread Violet started, in
  a Slack channel mapped to the project, the supervisor now wakes on that
  message instead of finding it at its next poll (#1145). The factory daemon
  claims the project's Slack activity from Cassy Cloud every 15 seconds,
  admits each event once, and wakes the supervisor with a typed Slack
  activity message. The wake text says the message is a cue to read the
  thread, not authority to answer it (#1146).
- Each channel that woke the supervisor is then read with `violet_read`
  every 5 minutes. The watch stops after an hour without human activity,
  after three failed reads, or when the session ends. Violet's own posts never
  count as human activity; set `slack.violet_bot_user_ids` if the daemon has
  not yet learned the bot's id from a thread Violet started.
- **Behavior change:** push-wake is on by default. A factory daemon that is
  logged in to Cassy Cloud now claims Slack activity every 15 seconds; until
  the hub sends activity events for the project, the claims find nothing and
  nothing wakes. Opt out with `cas config set slack.wake_enabled false`. A
  daemon that is not logged in to Cloud, or whose project has no canonical
  id, does not start the claim loop and logs why once.

### Added — posting files to Slack

- `cas violet post`, `cas violet thread` and `cas violet read` reach the
  Violet hub from the command line, so any agent or script can share a local
  file by path instead of hand-copying its bytes into a `violet_post` call
  (#1157). `post --channel <name> --file <path>` reads, hashes and encodes up
  to ten files from disk; files totalling up to 1 MiB go inline and larger
  sets stream from disk to the hub's upload URL. `thread` posts a top-level
  message and up to 20 ordered replies in one call, `--reply-file` attaches a
  file to the reply before it, and `--idempotency-key` makes a resend never
  post twice. A thread that stops part-way reports what was posted and where
  it stopped. `read` reads a channel since a time, one thread or one message,
  and never prints downloaded file bytes.
- `cas violet` uses the same Violet registration, credential fallback and
  policy as the MCP proxy, passes hub error codes through, prints the hub's
  receipt (or error receipt) with `--json`, and exits non-zero on failure.
  File posts go through the publication gate.
- The `artifact` tool's `action=post` sends a published artifact to Slack by
  its id (#1148). Cassy resolves the record through Cloud, checks the
  downloaded bytes against the published size and SHA-256, and uploads them
  through Violet. The receipt returns the message id, file id, permalink and
  `sha256_verified`, and the permalink is saved on the artifact. A missing
  artifact, missing Cloud login, size or checksum mismatch, missing channel or
  unreachable Violet each fail by name. The publication gate treats it as a
  file share.

### Changed — Violet hub address and credential names

- Violet registrations now point at `https://violet-hub.vercel.app/mcp/slack`
  and use `VIOLET_*` credential names (#1156). `cas update` and sync rewrite
  project and user proxy files in place, keeping their formatting, and update
  the Claude Code and Codex entries' URL. `cas integrate violet` renames the
  legacy `MECHA_*` keys in the credentials file to `VIOLET_*` (idempotently,
  mode 0600, values never printed; a populated new key wins over an old one)
  and reports which names it renamed.
- For one release, credential lookup still falls back to the legacy
  `MECHA_*` variables, and the former hub address is still recognised for
  migration. Custom endpoints and custom server names are left alone.

### Changed — violet skill

- The `violet` skill follows the 2026-10-09 Violet contract: the flat
  `violet_post` schema with `files[]`, `file_external` and `kind=thread`;
  text versus base64 file encoding; a local-file route through
  `cas violet post --file` that needs no hand-copied base64; and
  `violet_read`'s 100,000-byte default, opt-in channel list, `file_id`,
  `message_id` and `thread_id` scopes, and hashes for skipped files (#1154,
  #1149). A thread message that @-mentions another person to validate,
  confirm or review is a human handoff: Violet does not answer it on that
  person's behalf (#1146). Field detail lives in the skill's
  `references/contract.md`, which also documents posting a published artifact
  by id.
- A new attachments reference gives the Violet-only route for reading
  attachments and Slack Connect files, the exact error each failing route
  returns, and the operator fallback: invite @Violet, or re-share the file
  (#1129).
- The release report adapter reads its uploaded PDF back by `file_id` and
  fails closed on a skipped file, and the release announcer no longer asks
  for the channel directory. The publication gate also covers
  `file_external` uploads and thread replies that carry files.

### Fixed — close and QA gates

- A no-code task with no commit of its own closes on its external reference
  and never on the worker branch's other commits (#1133, #1147, #1151).
  Those commits are no longer counted as this task's stranded, parked or
  anchored delivery, and an anchor an earlier park recorded for another
  task is ignored. A branch counts as another task's only when every
  unmerged commit on it names another task.
- A `commit_receipt` that names a multi-task squash no longer charges every
  sibling's files to the closing task (#1144). When the squash carries all of
  the task's delivered paths, QA judges only that task's own delivery.
- Verification binds to `origin/<target>` when the local target branch is
  strictly behind it, so a checkout whose local branch lagged by hundreds of
  commits no longer verifies against a tree without the delivery (#1137). A
  local target that is equal, ahead or diverged (an unpushed local merge)
  still wins.
- A changed snapshot file can be approved once, with a note naming
  `file-sha256:<digest>` of the delivered file, instead of one note per
  changed line (#1141). Any later edit changes the digest and needs approval
  again. Line-level approvals still work.
- Task close runs off the MCP server's async workers (#1142). Parallel closes
  used to queue behind each other and every other tool call, and the 55-second
  budget answered UNKNOWN while the close kept running. A close still running
  at the deadline now answers IN_PROGRESS and appends a `CLOSE_OUTCOME` note
  with its result and the task's status. Pending-close guidance keeps naming
  the caller's own tool prefix, and resending the same completion receipt
  returns the delivery already recorded.
- Visual QA pairs a finding that only moved (same element, size and contrast)
  with its earlier finding instead of reporting it as new, and a rejection is
  always recorded, whatever the bounds (#1152).
- Visual QA no longer reports invisible text when a loading marker
  (`aria-busy=true` or a `data-*-pending`/`-loading` attribute) hides the text
  behind a placeholder that paints its own text (#1150). Blank, transparent,
  hidden or zero-size placeholders, and text hidden by anything else, are
  still findings.

### Fixed — factory and workers

- The delivery watchdog reads the recipient's transcript for a new turn, not
  terminal echo (#1153). Injected text always echoes in the pane, so a worker
  whose input box swallowed a message used to count as delivered. Now a turn
  start retires the watchdog, a worker still mid-turn defers it, and anything
  else gets one nudge and then a supervisor flag, and `message_status` reads
  not-delivered. Pane growth is the fallback only when no transcript exists.
- `cas serve` answers a probe sent before `initialize` with "Method not
  found" instead of exiting, and a worker whose MCP child restarted stays
  alive while its harness process is still running (#1143). On macOS the
  exited child's pid used to make the boot check kill a healthy worker.
- The launch time a worker's SessionStart canary must postdate is now taken
  before its terminal starts. Taken after start-up bookkeeping (0.8 to 19
  seconds on a loaded host), it rejected fresh canaries as stale and killed
  healthy workers. A refusal now says it found an older canary instead of
  saying none exists.
- Spawning a Codex worker refuses a model the ChatGPT account cannot run,
  before any worktree is cut (#1130). A provider's model refusal is fatal: it
  reaches the supervisor as a blocker, and worker liveness reports stalled with
  the provider's own message.
- Opening the task store no longer takes the database write lock when
  nothing needs repair, and the sync queue now waits for a busy lock instead
  of failing at once. A lock-order inversion that stalled task sync for about
  30 seconds, and every process behind it during a fleet boot, is gone.
  Shutting workers down reports unknown task state when the store is
  unreadable, and `force=true` proceeds.
- A `cas serve` that fails to start, for example on an unreadable `cas.db`,
  exits within two seconds instead of hanging until its input closes.

### Fixed — Commander, doctor and MCP proxy

- In a Commander conversation, a hub update no longer drops keyboard focus
  to the page while Tab moves from Raw output to Interrupt, and closing the
  command palette no longer leaves a full-page rebuild waiting to steal focus
  on the next update. The rebuilt control keeps its focus ring.
- Postgres MCP servers (Neon and `server-postgres`) run in UTC unless their
  config sets `TZ`, so a stored `timestamp without time zone` comes back
  unshifted. On an EDT host, 14:34:21 used to come back as 18:34:21Z (#1127).
- `cas doctor` names the project `.cas/proxy.toml` block that shadows the
  machine's registration when an upstream credential is missing: the file,
  the `[servers.<name>]` block, the unset variable and the shadowed
  registration, and says to remove the block (#1128). `cas integrate violet`
  keeps project overrides, so its generic advice never cleared this failure.
- `cas integrate violet` and `cas doctor` check the `violet_post` schema the
  hub serves (#1051). A schema that is not a plain object, uses a top-level
  `anyOf`/`oneOf`/`allOf`, or lacks the message, file, reaction, edit and
  delete kinds turns the integrate receipt non-green and doctor red. Doctor's
  Violet row also states readiness: reachable, bearer accepted, or
  `invalid_token`.

## [3.48.2] - 2026-10-07

### Fixed — Claude worker safety guards

- Claude factory workers now always run with Cassy's hooks, and so with its
  safety guards: the capped cargo runner, worker-memory admission, and the
  Slack, publication and browser guards. Before this release, every Claude
  worker ran with none of them. The launcher sets `IS_DEMO=true`, which skips
  Claude Code's workspace trust dialog without trusting the workspace, and
  Claude Code runs no hooks (SessionStart or PreToolUse, from any settings
  source) in an untrusted workspace.
- Before a Claude agent starts, Cassy now records trust for its working
  directory in that agent's Claude config (`$CLAUDE_CONFIG_DIR/.claude.json`,
  or `~/.claude.json`). The merge writes only
  `projects["<cwd>"].hasTrustDialogAccepted = true` and keeps every other
  value. It runs under an exclusive `.claude.json.cas-lock`, through a temp
  file, fsync and rename that keep the file's mode and any symlink, then reads
  the file back and retries once if a live Claude session rewrote it. A config
  that is empty, does not parse, or is not a JSON object is never rewritten;
  the launch is refused instead. Rewrites re-sort object keys; values are
  unchanged.
- A factory agent's SessionStart hook now writes a launch canary at
  `.cas/factory/hook-canary/<agent>.json`. Spawn, respawn and recycle
  verification kill a Claude worker whose canary has not appeared within 60
  seconds of its launch, mark it crashed and report why. A canary left by an
  earlier worker of the same name never counts. Codex workers are not held to
  the canary; their trust was already recorded before spawn.
- After upgrading, respawn every running Claude worker. Hooks start denying
  what those workers were never denied before: raw `cargo`, `&` background
  jobs under worker-memory admission, non-Violet Slack writes, publication and
  unfiltered browser runs.

### Fixed — Commander

- With Commander open in two tabs, a tab no longer says "pairing was revoked"
  after the other tab renews this browser's credential. When two requests
  left with the old credential and were both refused, only the one that
  adopted the new credential retried; the other found nothing left to adopt
  and reported the pairing revoked. A refused request now retries whenever
  the machine's credential differs from the one it signed with. A refusal of
  the current credential still reads as a revoked pairing.
- On a phone, the conversation header shows the machine's codename whole
  beside the whole machine name, or steps it aside, instead of a cut stub such
  as "patient…". A stepped-aside codename is still in the line's title and
  still read by screen readers. Two pixels of slack on every fit test stop a
  fraction of a pixel of overflow from drawing an ellipsis.

### Fixed — messages between sessions

- A message that wakes a Claude worker or supervisor now arrives in the turn
  that wake starts. With Claude teams delivery, the daemon's transport claim
  hid the message from the turn-start hook and from `inbox_poll`, while Claude
  Code held its own copy back until the next turn boundary, so the woken
  session read "see inbox", found its inbox empty and got the body a turn
  later. The turn-start hook now reads `CAS wake: message N` from the
  submitted prompt and shows that message, if it is addressed to this
  recipient, over the transport claim. Ordinary turns and inbox polls still
  skip claimed messages.
- `message_status` reports a `recipient_receipt` line that tells a body
  rendered into the recipient's turn from one the transport only claimed.
- Without a hook, a woken session can still read the message: `inbox_poll`
  with `notification_id=N` returns that claimed message to its recipient, and
  the wake text names the call.

### Fixed — release and factory tooling

- The close gate's and QA dispatch's "journeys:" reason now comes from the
  journey selector and catalog committed at the delivered head, as
  journey-eval's does. It used to run the store checkout's
  `scripts/journeys-for-diff.py` on its working tree, so a checkout on an
  older main could name a different journey set than journey-eval selected at
  the tip. The working tree serves only when no delivered head is known, and
  a head without a journey catalog names no journeys.
- `scripts/worker-memory.py --shell-command` refuses a command that puts a job
  in the background with a bare `&`, and says to run the suite in the
  foreground of a persistent session with a log. Admission ends the command's
  whole process group when its shell returns, which cut down the suite's
  receipt launcher while a detached Playwright runner lived on unreported.
  `&&`, `>&`, `&>`, `|&` and quoted text are unaffected.
- The hub test fixture's cleanup waits for the hub to exit (a pidfd on Linux)
  for up to 15 seconds, then kills the hub process group and waits up to 10
  seconds more. It used to wait a fixed half second and escalate only while
  the hub held its lock, so a slow graceful shutdown failed the reap test
  under load. The test now asserts at once that no hub remains.
- The worker-memory concurrent-admission tests allow 20 seconds
  (`CAS_TEST_SPAWN_DEADLINE_SECS`) for a counted command to start instead of
  2. Two seconds missed by 7 ms on a loaded host; a serialized admission still
  fails the test.

## [3.48.1] - 2026-10-07

### Fixed — Commander sends

- A message held while Commander was offline goes out promptly after the
  machine reconnects, exactly once. In 3.48.0, if the machine came back while
  a held message was still being settled, the reconnect's wake-up was dropped
  and nothing tried the message again. The held-send flush now remembers a
  wake-up that arrives mid-batch and runs once more. A write whose outcome is
  uncertain is never sent a second time, and revoking the pairing during
  recovery stops the deferred send.
- With two tabs open, "Retry" no longer flashes in either tab while the other
  tab's send is still in flight, so the same message cannot be sent twice by
  accident. A tab that sees another tab's send keeps that send's own
  15-second confirmation clock instead of marking it "Not confirmed" at once,
  and a stale Retry cannot reopen a send another tab is still waiting on. A
  new reply in the conversation no longer cuts the other tab's wait short.
  When that send's confirmation arrives in the machine's history, it settles
  as that one row instead of a second copy.

### Fixed — Commander polish

- On a phone, the conversation's name and machine take their own row under
  the back link and actions, so "Atlas" and "Build Server" read whole. A long
  machine name is never cut while its codename shows: the codename steps
  aside instead. A short screen, such as a phone with its keyboard up, keeps
  the one-row header. Raw output keeps its word beside Interrupt, the phone
  toast drops below the taller header, and the phone Tasks & progress sheet
  names its project and machine.
- After Stop in Tasks & progress, focus moves to the next row and the result
  line is scrolled into view as well. The Stop confirmation says the task
  "goes back to Open", the word its chip then shows.
- Revoking this browser's own access says so and lands on Pair a machine,
  instead of a silent first-run screen.
- A browser permission reads "Blocked by browser" and an unsupported browser
  "Browser can't connect" in the header, list row, footer, empty thread and
  launch sheet, instead of "Can't reach". The Connection log reads the
  header's state, updates on the same render and names its machine. A retry
  in one conversation no longer sets the machine footer to "Reconnecting"
  while another conversation on that machine is answering.
- A "Skip to the conversation" stop after search puts Interrupt three keys
  away instead of about a dozen Tabs, with a focus ring at every stop. The
  header names the device in control before you take it, and a refused take
  says that a pairing with administrator access can take over.
- Hovering the pinned question bar keeps its amber fill in both schemes; in
  dark it had turned navy under dark text.
- The Assign picker names the task by its title, not its ID alone.
- In a short landscape sheet, a refusal notice rides the sheet's bottom edge
  instead of landing below the fold.
- Dismissing the phone Undo offer keeps Undo in Tasks & progress, and the
  dismiss button says so.
- The pair dialog's action bar is part of the sheet, frosting only while
  fields scroll beneath it; in light it had read as a whiter slab.
- Answers five minutes or more apart start a new group with their own time,
  and grouping no longer changes on reload. A reply kept in this browser
  keeps the time it was shown, then the machine's own time once history
  arrives, so it is no longer marked "machine clock ahead" after a reload.
- During an outage, a reload before any machine answers reads "Conversations
  not loaded yet" instead of "0 conversations", and the empty list says how
  many kept messages will go out. The outage banner sits above the thread
  instead of over its first lines, and a reader mid-history keeps their
  place.
- Pairing reads in plain words: the admin permission is "See and revoke other
  browsers on" the named machine and joins the summary only once ticked, a
  new browser is named for itself (for example "Chrome on Linux"), every
  pairing dialog is named by its heading, and sign-in says "Cassy Cloud
  account".
- A focused "Load earlier" keeps a plain ring on the aurora, the conversation
  rail fades only the edges with rows beyond them, an Attention item about
  the open conversation drops its no-op "Open conversation", "Asked …" notes
  are kept per machine and conversation, and the skip link stays out of sight
  until focused.

### Fixed — release and factory tooling

- The close gate's affected-journey selection, and `scripts/journey-receipt.py`,
  now run `scripts/journeys-for-diff.py` from the reviewed head's committed
  blob instead of the shared checkout's copy. An older selector in a checkout
  on another branch could ignore the reviewed head and pick a different
  journey set. A missing or broken committed selector refuses proof rather
  than falling back to the checkout.

## [3.48.0] - 2026-10-06

### Added — Commander Glass look

- Commander has a new look, Glass, in light and dark: a violet-to-teal aurora
  behind frosted conversation list, heading, context and composer panels, a
  glowing violet Send, and an amber-to-coral card for an open question. Glass
  is the only look; Appearance and the system scheme still choose light or
  dark. Layout, controls and the composer's width are unchanged.
- Text on Glass keeps at least 4.5:1 contrast over every part of the aurora in
  both schemes. A refused or unconfirmed message keeps its dashed, unfilled
  record on a solid backdrop, so its recovery text, Edit, Retry and Review stay
  readable instead of sitting on the violet message colour.
- Forced colours, "more contrast" and "reduced transparency" get opaque,
  unblurred panels; more contrast also flattens the aurora and darkens
  secondary text. Glass adds no animation, so reduced motion needs nothing
  extra. The aurora stays still and is painted from a small pre-rendered
  image, and only the four main panels and dialogs are blurred, so long
  threads scroll as smoothly as before.
- `hub-web/scripts/build-preview.sh` builds a clickable preview of the
  production bundle against a fixture hub into a gitignored `preview-dist/`.
  A test proves the shipped bundle never carries the preview's shim.

### Fixed — Commander connection

- A dropped event stream no longer tears down machine connections that are
  still answering, so the header no longer flickers "Reconnecting" while
  conversations keep working. Only a connection that has gone quiet for four
  heartbeats, or a proven machine outage, is replaced; a connection that is
  still opening is left to its own deadlines.
- Event-stream reconnects back off. The retry streak resets only after the
  stream has stayed up for 10 seconds, not as soon as it answers, so a
  flapping stream no longer reconnects at full speed.
- While event delivery recovers, a conversation whose machine is still
  talking reads Live in the header, the list and the footer, and the status
  panel is not marked stale. A conversation that has ended no longer keeps its
  machine labelled "Reconnecting".
- Recovery refreshes the conversation list before going live, so the list
  stays filled when short event streams keep cutting off the regular refresh.

### Fixed — Commander sends

- A message resent with "Send again" settles as Delivered instead of staying
  on "Waiting for … to confirm". The replaced connection was closed before
  its delivery receipt arrived. It now stays readable for up to 5 seconds,
  accepting only receipts for messages it carried under the current pairing,
  while the next send goes out on the new connection at once.

### Fixed — release and factory tooling

- Every epic integration runs the release gate's no-build rows (markdown
  lint, failure log, version literals, changelog, procedure guardrails, test
  shape and the rest) and the CI script tier under release-train conditions,
  with every train control variable exported, and records each row as PASS or
  FAIL against the integration tip. Train preflight refuses a missing, stale
  or failed row before assembly, names it, and points to
  `cas factory integration-recover`. In 3.47.1 a markdown-lint error and a
  script-tier failure surfaced only during the cut.
- Release gate suite children run through `scripts/release-test-env.sh`. It
  removes factory identity, train and gate control variables and receipt
  destinations, gives them a clean temporary HOME and disables global Git
  configuration. In 3.47.1 a test that matched `LEASE` in an inherited
  `CAS_RELEASE_TRAIN_*` variable failed the script tier inside the train.
  Host memory admission and linker context (now `CAS_ASSEMBLY_LINK_CONTEXT`)
  still reach nested suites.
- The lane compile proof (`scripts/check-lane-compile.py --prove`) works again
  from a worker's private checkout. Its preview checkout sat one level too
  deep, so private target ownership refused it before Cargo ran. Previews are
  now direct children of `.cas/worktrees`, with a sibling metadata directory
  bound to the exact checkout path; cleanup still recognises older nested
  previews and refuses unowned ones.
- Capped worker checks and named tests no longer leave a compiler-cache
  daemon holding the worker's build lease. A cold sccache client passed the
  inherited private target lease to its long-lived server, so the next capped
  test failed with "Resource temporarily unavailable". The capped runner now
  clears `RUSTC_WRAPPER` and `RUSTC_WORKSPACE_WRAPPER` for its Cargo child,
  while real builder processes keep the lease until they exit. Supervisor and
  CI builds keep sccache.

## [3.47.1] - 2026-10-06

### Fixed — Commander

- A message held while a machine reconnects is sent once the machine is back,
  and the tab that held it shows it delivered. Before, a held send could stay
  unsent after another tab delivered it, and a tab could stay on
  "Reconnecting" after the machine had returned.
- Pairing that times out now shows the error in view, moves focus to it and
  offers the next step, instead of looking as if nothing happened. On a phone,
  tapping "Technical details" after scrolling the invitation form opens it
  and keeps your place.
- The installations sheet names each browser and shows local, relative times.
  Generation numbers, ISO timestamps and signing keys sit behind a closed
  "Technical details" disclosure. Long browser names wrap, and a blank name is
  refused.
- Polish from the 3.47.0 journey evaluation:
  - plain-language permission wording, with no scope ids or "terminals";
  - a phone header that no longer truncates the machine name;
  - a Connection log that leads with the cause in words and keeps the raw JSON
    behind an expandable "Technical details";
  - inbox status reads "Waiting for soundwave" and then "soundwave received it";
  - clearer captions, toasts and read marks.

### Fixed — CLI and factory

- `cas list` shows a session whose daemon has died as dead rather than
  "stopped", stale factory sockets are removed only when no live process holds
  them, and `cas kill` waits for the daemon to exit before removing both of
  its sockets.
- `cas integrate` output fits the terminal width at 40, 80 and 120 columns and
  stays readable under the C locale, `NO_COLOR` and pipes.
- Closing a parked task no longer re-walks generated bundles: its content proof
  takes about 2 s instead of about 29 s.
- Host memory admission locks are held by a dedicated process, so an sccache
  server or an orphaned test child can no longer keep them after a suite ends.
- A QA round whose review task was cancelled no longer blocks its delivery
  forever; asking for QA again withdraws it and opens a fresh round.
- Leftover release worktrees from a killed run are unregistered and reclaimed
  only when their owner is provably dead and the worktree is clean.

### Changed — release and QA tooling

- The release gate builds and audits the x86_64 release binary for AVX-512
  instructions before the merge queue, instead of discovering them at publish.
- Visual QA no longer reports an editable field's own scrolling, an intentional
  multi-line clamp, or a page that declares it needs JavaScript as defects.
- The QA evidence recipe produces the runner trace the close gate accepts, with
  a `--scrub-trace` step for signed-in runs.
- Journey receipts keep light and dark variants apart, the affected-journey
  selector no longer crashes on regex literals, and CI runs the Commander
  journeys on four workers.
- The release announcement records its Slack message ids in the release-notes
  draft even when the draft starts from the template's empty POSTED table.

## [3.47.0] - 2026-10-06

### Added — Commander cloud operator inbox

- Commander can read and answer supervisors from any signed-in browser while
  the machine is off. The cloud keeps up to 90 days of conversation history,
  encrypted when stored and sent (HPKE and AES-GCM through the new
  `cas-operator-crypto` crate and a byte-identical browser implementation).
  The cloud holds the keys, so this is not end-to-end encryption, and the
  sign-in screen says so. A newly signed-in browser, such as a new phone,
  replays and decrypts that history; a message that can't be opened or
  verified is never shown, and the thread says how many were withheld.
- A reply typed while the machine is off is held as a command sealed to the
  machine's key. It reads "Pending machine" until the machine admits it and
  "Accepted by machine" after its signed receipt; the same command is resent
  unchanged after an outage and admitted at most once. Undelivered commands
  expire after 24 hours.
- Sign-in is approved by the operator's Petra Stella Cloud account: the
  "Operator inbox" dialog shows a code to confirm on the cloud, or to approve
  from an enrolled machine with `cas hub operator approve <code>`.
- The hub enrolls the machine with the cloud (`cas hub operator enroll`,
  `status`, `approve`, `deny`, `principals`, `revoke-device`,
  `revoke-machine`, `bind`, `detach`, `drain`). The machine key lives in
  `~/.cas/hub/operator-inbox/machine.json` (mode 0600), and requests carry a
  proof-of-possession signature.
- Every operator-visible turn (MCP reply, mirrored transcript, watchdog notice)
  is written together with an immutable outbox row in one SQLite transaction
  (migration m263 `operator_delivery_outbox`), so a turn can't be stored
  without its delivery record. A 15-second drain loop sends pending rows. The
  hub's content-security policy names exactly the cloud's operator inbox API
  origin, with no wildcard.
- The hub verifies the cloud's signed enrollment assertion before marking an
  installation enrolled: `POST /v1/auth/account/challenge` and
  `/v1/auth/account/enrollment` (one-use challenge, 5-minute lifetime, at most
  4 per device; the assertion's audience, key thumbprint and account must
  match). The installation inventory's Account row reads "Not in an operator
  inbox" or "Operator inbox (key epoch N)".

### Fixed — Commander sends and replies

- Held sends are claimed atomically per item in an IndexedDB journal scoped to
  the hub, device and session, so two tabs can no longer both send the same
  message. Waiting and uncertain sends survive a reload and are never resent
  automatically.
- A message sent from two tabs, or across a reload, goes out exactly once: a
  delivery receipt seen in either tab settles the send in both, and Retry
  works from either tab, resending under the message's original reference so
  a late receipt can't produce a second copy or leave Retry stuck.
- The composer's "will go out by itself" line shows only while a message is
  actually held, and clears once it is delivered, including when another tab
  delivered it.
- Sending after this device's control of the session has lapsed takes control
  and then sends; the "Taking control of … to deliver this message" notice now
  clears as soon as control is held instead of staying under the reply.
- A refused or unavailable send settles against the claim that sent it, so
  Cancel removes a waiting message and a late refusal can't put a newer Retry
  back on hold.
- A resolved delivery notice stays resolved after a reload: notices are no
  longer kept in the device's reply journal, and older journaled notices are
  pruned, so they can't come back as Blocker bubbles in the conversation.
- A reply counts as delivered to a device only after that browser stores it:
  the browser sends `OperatorReplyPersisted` and the hub records a per-device
  receipt (migration m264 `operator_reply_device_receipts`). Reply captions
  read "Forwarded · not stored on this device" and then "Stored on this
  device"; neither claims the reply was read.

### Changed — Commander pairing and installations

- Re-pairing a browser proves its retained signing key and rotates that same
  installation's credential instead of creating a new device each time (one
  host had accumulated 14). New routes `/v1/auth/pairing/{protocol,commit,abort}`,
  `/v1/auth/devices` and `/v1/auth/devices/{device}/revoke`; tabs coordinate
  through a Web Lock and a secret-free catalog generation.
- Each Paired machines row opens an installation inventory: "This browser"
  with its device ID, generation, first paired, last use, origin, key
  fingerprint and account. "Revoke" confirms the exact device ID, "Remove from
  this browser" is separate, and an uncertain cleanup offers "Retry cleanup".
- An invitation can request "Hub administration" (`hub:admin`), never
  pre-ticked, which allows revoking other installations.

### Added — Commander connection causes and diagnostics

- "Connection details" opens a Connection log that states the cause (for
  example "Hub says access was revoked", "Event sequence gap detected" or
  "Network or browser policy blocked the request"), the recovery action, the
  next retry and the last successful connection. "Export safe diagnostics"
  downloads `commander-connection-diagnostics.json` without credentials, keys
  or prompts.
- 18 typed causes are kept in a bounded 64-entry transition log. The hub adds
  `X-Cas-Request-Id` (exposed only to granted origins) and `x-cas-refusal`,
  and the event stream sends explicit lag and epoch markers instead of
  silently skipping records. Reads, pairing and credential refresh have 10 s
  deadlines; catalog refresh is limited to one request in flight plus one
  trailing, at least 1 s apart.

### Changed — Commander conversations

- The legacy Terminal view is removed; Conversations is Commander's only
  surface (about 6,200 lines deleted; the bundle shrinks from 501 KB to 449 KB
  of JS and from 177 KB to 136 KB of CSS). The conversation header gains
  Interrupt (it names the device it took control from, or says inline why it
  can't act) and Raw output, a read-only drawer on desktop and a bottom sheet
  on phones.
- Questions show quick replies only for options they declare; otherwise the
  reply goes in the composer. Each question appears once, in the thread, with a
  compact "Waiting on you" bookmark that jumps to it. Only questions and
  blockers count as waiting on you.
- Previews render Markdown as plain text, statuses read "In progress",
  "Awaiting merge" or "Held", and the side rail lists the real roster with each
  member's current work, or "Current work not reported".
- A failed fleet action's note spans the full row at desktop widths and names
  its subject without squeezing to a narrow column.

### Fixed — Commander sessions, recovery and layout

- New session asks once: the first "Allow starting sessions on <machine>"
  grants the permission and opens the project list, instead of leading to a
  second confirmation sheet that asked for the same permission under another
  name.
- A half-open machine stays "Unsteady" until four heartbeats are missed. A
  failed or timed-out catalog read triggered by an event is now left to the
  heartbeat instead of ending the event stream, which had turned "Unsteady"
  into "Reconnecting" after one missed beat. A refused pairing still ends the
  stream at once.
- A keyboard-focused or open conversation row shows the whole machine name,
  wrapped clear of the time stamp; resting rows keep their one-line ellipsis.
- On phones, the header's machine line no longer clips its glyphs, and a
  conversation opened from search by touch lands focus on the reading region
  instead of dropping it to the page, without raising the on-screen keyboard.
- In a landscape phone pane (844×390) the connection-failure card scrolls
  vertically, so Retry and Diagnose are no longer cut off.

### Fixed — cloud sync

- A team-linked sync's receipt counts the real queue: the team backlog, held
  personal pending and failed rows with their rejection reasons, and a warning
  when held rows failed. It had read "0 pending, 0 failed/parked" while
  thousands of personal rows sat rejected as `team_owned_project`.
- `cas cloud status` reports the team pull watermark, advanced on every
  successful team pull including ones with nothing newer, and labels the
  personal one. Status and `cas doctor` flag personal `team_owned_project`
  rejections while `cloud.team_only` is off, with the exact fix command.
- Healing a historical parent-child dependency stages its own-project endpoint
  tasks that are missing on the cloud, insert-only so a pending newer write and
  its retry metadata survive, and holds the edge until those tasks exist.
  Deleted, moved and foreign endpoints are refused and parked with a reason
  instead of burning retries as `orphan_dependency`. Legacy memories with no
  origin project park as `unattributed_origin`, and intentional parks are
  reported apart from failures. Existing stranded rows repair after an updated
  runtime pulls, then pushes once.

### Fixed — Commander hub status

- `cas hub status --json` adds a read-only `runtime_receipt` after a restart:
  separate results for the hub, Serve publication, external reach and
  projects, boot prerequisites (Linux linger, macOS login) and the next step.
  It never claims jobs resumed.
- Service-manager probes have a 500 ms limit, so a stuck `systemctl` or
  `launchctl` no longer blocks status; the state reads "service manager
  unknown (timed out)" and restart refuses to change service ownership while
  it is unknown.

### Changed — browser test tiering

- Journey selection follows source impact. `scripts/journeys-for-diff.py`
  ignores the rebuilt `hub-web/dist` and maps changes through each journey's
  declared touches, static imports, `main.ts` symbol ownership and changed CSS
  selectors; tokens, base rules and unattributable changes still select the
  whole surface. A one-module fix selected 1 journey instead of all 18.
- `scripts/journey-eval.sh <artifact-dir>` runs the affected selection at 4
  workers by default (`--affected <base>`, `--workers=1..4`, `--task <id>`);
  an empty selection launches no browser. `--full` runs the whole suite and is
  reserved for epic assembly; a PreToolUse guard denies `--full` and
  unfiltered Playwright to workers and QA reviewers.
- Close and QA gates recompute the selection at the delivered tip and require
  a passing `journey-receipt.json` for every selected journey, refusing with
  the missing, failing or unrecorded IDs. An epic close requires one
  full-suite receipt. Previously each hub-web delivery ran the full ~80-test
  suite three times, about an hour of browser tests.

### Added — release learning loop

- The release receipts stage refuses to finish until every blocked stage and
  every hand fix maps to a learned gate row or an open task, and prints the
  exact `release-gate.sh --learn` command to record it. Four missed lessons
  from 3.42.0–3.46.0 are back-filled in the failure log.
- `INTERVENTIONS` is counted from the evidence (resumed blocked stages, hand
  fixes and manual rows) instead of `kind=manual` rows only, which had printed
  0 since 3.42.0; 3.46.0 replays as 8. The release report gets a "Manual
  interventions" row and both Slack replies a "Release effort" line.
- A `publish-toolchain` gate and cut-preflight row loads the real
  `cargo-zigbuild` configuration before a release lands, catching the class of
  error that needed hotfix PR #1132 in 3.46.0. A failed publish deletes the
  local tag it created once the remote is proven to have none.
- The off-main tooling warning captures git noise and skips merged, stale,
  epic-covered and duplicate refs, printing one summary line.

### Fixed — factory messaging and workers

- A message to a busy worker whose wake budget runs out is parked as
  `awaiting_busy_recipient` instead of abandoned; turn-start and tool-boundary
  hooks still surface it, and `message_status` reads "busy recipient: still
  pending". The "reassign or recycle" notice fires only after 10 minutes of
  transcript silence, once per message.
- `--worker-cli claude` is honoured instead of being replaced by the stock
  worker default; a worker that falls back from Codex to Claude is labelled
  and messaged as Claude; `--worker-spec` names name the initial workers, and
  duplicate or supervisor-clashing names are refused.
- A clean worker holding tasks can be recycled without `force` and keeps its
  name, tasks, lease and worktree; `clear_context` errors now name
  `clear_context`.

### Fixed — close and QA gates

- A park binds its QA round to the delivered tip when every commit since the
  recorded anchor belongs to the task's own branch, and `qa_request` rebinds a
  stale pending round to the tip. A shared worker branch or another open
  task's anchor never moves the anchor, and a close without `commit_receipt`
  refuses rather than bind QA to a shared branch's old work.
- A supervisor close of a parked child on a large epic is bounded: the
  delivery content gate has a 35 s budget, logs
  `stage=delivery_content_gate elapsed_ms`, and returns a retryable "DELIVERY
  CONTENT CHECK TIMED OUT" refusal that changes nothing. Its line walk skips
  unchanged history, batches diffs and messages and leaves regenerated
  `hub-web/dist` bundles to the regenerated-artifact rule; one replay fell
  from about 60,000 git processes to about 1,300. A deadline after only an
  intermediate write reports IN_PROGRESS, not COMMITTED.
- A registered supervisor can park and close a retired worker's corrected
  delivery from its pushed commit, validated against the task's work target
  rather than the retired worker's moved-on worktree, and `qa_request` accepts
  an explicit head on a reopened task. Unmerged receipts are still refused.
- A supervisor close whose `commit_receipt` is already an ancestor of the
  task's work target succeeds without `supervisor_override` when the
  assignee's checkout is detached on other work. It had failed with "expected
  task worktree branch …, found ``". Worker self-close, non-supervisor callers
  and unmerged receipts keep the existing checks.
- Approving a changed snapshot line by its SHA-256 is now proven end to end
  through the MCP notes and close path: a literal over the 1,500-character
  note limit is refused, the correct hash closes, and a same-prefix wrong hash
  is refused.
- Close suggestions prescribe real Cargo test targets: modules that now live
  inside combined targets such as `integration_contracts` are mapped to them,
  and unknown suites fail closed, instead of a prescribed command failing with
  "no test target named".

### Fixed — QA and release tooling

- Scoped visual QA matches findings by rule, page, state, viewport, text and
  accessible name rather than the full CSS selector, so renaming an element no
  longer turns an existing finding into a new one.
- The contrast inspector resolves CSS Color 4 colours and opaque background
  layers; a button reported at 2.57:1 now reads its true 12.08:1.
- Assembly admits parallel test-binary links from live free memory
  (`CAS_RELEASE_GATE_ASSEMBLY_LINK_JOBS`, default 8) instead of one at a time;
  the reserve and compile guard are unchanged.
- Worker browser and JS test suites (npm/npx, Playwright, Vitest and the
  hub-web build, typecheck and visual-QA entry points) now wait for shared
  host-memory admission through `scripts/worker-memory.py` instead of starving
  the release proof. Admission uses weighted counting slots against the fresh
  memory budget: two browser suites and a typecheck run side by side under a
  normal budget, a low budget (8 GiB) still runs browser suites one at a time,
  and light commands (typecheck, Vite build, capped Vitest) never wait behind a
  browser suite. Proofs keep exclusive priority, waits print
  `waiting for host memory (proof running), N s` bounded by
  `CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS` (default 600), and a running
  suite is stopped if free memory falls inside its headroom. A credential or
  other hook deny still wins over admission. Previously 125 headless browsers
  pushed free memory below the proof's reserve and aborted it, and a single
  exclusive lock then made a 1-second build wait minutes behind a browser
  suite.
- Script-test fixtures, in-process and subprocess, use private admission pools,
  so a fixture proof no longer holds or waits on the real host locks. Production
  admission is unchanged, and no environment variable bypasses it.
- Assembly link memory receipts also sample the forked `mold` workers of the
  same link, not only the waited process.
- Newly seeded worker `target/` directories carry CAS ownership provenance
  (checkout, target and lease identity, a generation marker, and process start
  and boot time) before any build data, so retirement can reclaim them even
  when an unrelated unreadable process is present. A `target/` that git does
  not ignore stays legacy, with no marker and no reclaim, and seeding still
  succeeds.
- Visual QA treats content the engine skips (`content-visibility: hidden`,
  Chromium's closed `<details>`) as not drawn, and ellipsised one-line text
  stops the inspector's horizontal clip walk, so neither raises a false
  clipping finding; a real visible clip still fails.
- Commander journey fixtures follow the real phone layout in a responsive
  four-variant lane, and a journey's screenshot settling can no longer outlive
  its page, with repeat captures isolated per run.
- Each worktree's Rust proof builds into its own target directory
  (`scripts/proof_target.py`), and proof logs record the worktree, HEAD and
  target, so another worktree's build output can't produce a false result.
- Proof and release scratch is removed on every exit path. An owner record,
  process start-time identity and an inherited lock let TERM, INT and HUP reap
  child groups before cleanup, and the next start sweeps verified dead owners,
  including killed ones; paths of unknown origin are kept until
  `CAS_RELEASE_SCRATCH_MAX_AGE_HOURS` (default 6). The shared assembly cache is
  evicted whole above `CAS_ASSEMBLY_TARGET_MAX_GIB` (20 GiB) or after
  `CAS_ASSEMBLY_TARGET_MAX_AGE_DAYS` (7 days). Worker shutdown reclaims the
  worker's `target/` after copying its logs and test receipts to the task's
  artifacts. `gc_report` lists reclaimable and retained bytes; scratch
  `gc_cleanup` needs `force=true` and `dry_run=false`. On one host about
  9.7 GB of leaked scratch and a 29 GB assembly cache had built up.
- Merge-queue journeys cap session reattach delays at 10 s, wait for the retry
  and for animations to settle, run Playwright with 6 workers in CI, and
  upload the hidden `.results` traces on failure.

## [3.46.0] - 2026-10-05

### Changed — faster assembly proof

- The assembly proof overlaps the script tier with the native and archive-mode
  producer compiles when memory permits. Both producers finish and the script
  tier passes before the native test consumer runs, followed by the archive
  consumer; the two test consumers stay sequential. Native and clone builds keep
  separate Cargo targets, logs and gate scratch, and the receipt records
  per-leg and compile-phase intervals, CPU timings and the scheduling decision
  (including any serial fallback reason).
- Archive-mode tests run at native speed. Archive extraction stays on disk with
  `--extract-to`, and the plain clone stays outside disposable roots and every
  `.cas` ancestor; only disposable test temp directories and fixture HOMEs move
  to the native temp filesystem. On soundwave the archive test phase fell from
  133.5 s to 34–40 s, and the whole proof from 8 m 36 s to 4 m 56 s on its
  first concurrent run (6 m 32 s on the final tip, where links are serialized).
- `.cargo/config.toml` sets `jobs = "default"`, so a stale `jobs = 16` cap from
  a parent checkout no longer survives into nested worktrees. A cold build on a
  32-thread host went from 107.0 s to 94.6 s. `CARGO_BUILD_JOBS` still
  overrides it; worker throttling and the per-producer assembly ceiling are
  unchanged.
- The proof's environment fingerprint removes harness and session identity
  variables, so a factory shell and a scrubbed release shell share one proof
  without passing harness context to test children. Build, test-safety and
  compiler controls remain fingerprint inputs.

### Added — assembly memory admission

- Assembly reads available memory (Linux `MemAvailable`; macOS `hw.memsize` and
  `vm_stat`) before admitting producers and each consumer. The reserve defaults
  to the larger of 25% of RAM and 8 GiB
  (`CAS_RELEASE_GATE_ASSEMBLY_RESERVE_GIB`). Too little room for concurrent
  legs selects sequential legs with a fresh admission before each phase.
- Admission waits instead of failing outright: it resamples until memory
  recovers, bounded by `CAS_RELEASE_GATE_ASSEMBLY_MEMORY_WAIT_SECS`
  (default 600), and records every refusal and later admission in the receipt.
- A compile guard pauses the producer process group within 2 GiB of the
  reserve and resumes it once 4 GiB is free above it. A deadline or an observed
  reserve breach aborts the producer and prevents a PASS. A shared linker slot
  allows one test-binary link at a time across both producers, budgeted from a
  measured 2.09 GiB `ld.mold` peak; each link records its peak RSS.

### Changed — Commander hub defaults to Tailscale Serve

- `cas hub start`, `cas hub restart`, `cas hub service install` and `cas update`
  publish the hub through Tailscale Serve (tailnet-only HTTPS) by default, even
  when the previous hub was loopback-only. Restart and update keep an existing
  HTTPS port choice. Opt out per launch with `--no-tailscale-serve`, or for the
  host with `hub.tailscale_serve = false` in `~/.cas/config.toml`. Service
  definitions encode the resolved choice, including on macOS launchd.
- After a binary update, `cas update` starts a stopped hub when its service is
  installed or the machine has a hub identity; otherwise it prints
  `hub not running; start with cas hub start`. If Tailscale is missing, logged
  out, lacks Serve permission or cannot publish, the hub stays healthy on
  loopback and the update still succeeds. The update receipt then records
  `transport_verified=false`, a `transport_warning` and a remedy.

### Added — Commander fleet operations

- The hub serves `POST /v1/sessions/{s}/operations` for asking the supervisor
  to merge, focusing an epic, adding, pausing, resuming, restarting and
  stopping workers, and assigning or unassigning a ready task. Each operation
  is scoped, replay-safe per device and operation id for 10 minutes, and
  refused with `409 stale` when the state the operator saw has changed.
  Requested and outcome rows go to the hub audit log (End session now writes
  them too), and assign and focus return an Undo operation. Session status
  reports the focused epic.
- New pairing scopes split fleet control: `factory:operate` (focus, add, pause,
  assign; a control device can self-grant it) and `factory:manage` (stop,
  restart, End session; never self-granted). Paired machines shows each
  pairing's fleet permissions with a copyable `cas hub pair` command.
- Commander's conversation rail gains a per-agent ⋯ menu, Assign… on ready
  tasks, Ask supervisor to merge with a preview of the exact message, Add
  worker… and Focus epic…, with inline confirmation (Cancel first,
  double-click guarded) and an 8-second Undo. Phones get the same operations as
  action sheets. A missing permission is stated once with its command, and a
  failure reads in plain words instead of a raw route.

### Fixed — Commander connection

- Commander no longer floods the hub with session-list requests. Event-driven
  refreshes run one at a time, at most one start per second, with a trailing
  refresh so the list ends current; previously each event issued its own
  `GET /v1/sessions` (about 12 per second under load).
- Commander no longer sticks on "Reconnecting" after a browser-side network
  failure. A failed request no longer counts as a revoked pairing, so the
  pairing is kept and the connection retries; only an explicit refusal asks to
  re-pair. Event-stream reads and catalog refreshes have an abort and a
  deadline.
- When Chrome's Local network access permission blocks a tailnet hub, a notice
  names the machine and the browser setting to change. It sits at the list's
  18 px inset, leaves the single "Can't reach…" sentence to the list's empty
  state, and keeps its edge in forced colors.
- A revoked pairing keeps its conversations listed as Needs pairing, and
  finishing Re-pair returns to the conversation it started from.
- On a machine whose clock runs ahead, reloads keep each turn's recorded time
  and clock-ahead mark, and conversation rows age from arrival rather than
  reading "now" for minutes.
- An outage is announced once: file cards, Terminal view and the banner no
  longer repeat it, Terminal view shows "Earlier output" for replayed text, and
  the header's Terminal view explains why it is unavailable while
  reconnecting. Footers lead with the verb ("Can't reach …"), so a phone
  ellipsis cuts the machine name instead, and long machine labels ellipsize.
- When a machine connection stops retrying because the browser cannot make
  it, the banner, task status and footer all say so ("Unreachable") with one
  recovery step (update the browser, then reload), instead of some of them
  still reading "Reconnecting"; a duplicate session-level card is resolved.

### Fixed — Commander conversations and accessibility

- A draft that is too long to keep, or that full or blocked browser storage
  refuses, now says so and stays on screen instead of vanishing on reload.
  Oversized stored conversation values are cleaned up once on load.
- End session's desktop confirmation no longer accepts a double-click's second
  click, completion is announced, and a failure is announced once with focus
  restored for a retry.
- The keyboard can leave the terminal input. Tab moves on when this browser is
  not in control; in control, Ctrl+Alt+M or the Leave terminal control (which
  keeps its key visible at every width) leaves it.
- Keyboard focus survives shell rebuilds: Load earlier stays focused and in
  view across a reconnect, Paired machines returns focus to its opener after a
  revoked pairing, and Back on a phone returns to the row that was open.
- Attention notices show readable Details instead of JSON, with clearer
  timestamps and reply context; Copy says "Details copied", and the toast
  appears above the phone sheet. The dismissed-messages chip says "dismissed",
  so it no longer disagrees with the thread's not-confirmed count.
- The palette and list search no longer cover the unread count with the Enter
  hint or read "Enter ↵" aloud, and palette Enter skips the conversation
  already open.
- New session copy wraps grant commands only between tokens, shows a copyable
  login command on refusal and uses one "Default" treatment. A read-only pair
  link shows what it withholds and its command without scrolling.
- The pairing countdown never goes up, and spoken names say "the <project>
  supervisor". The header's machine and codename line is heard whole at every
  width.
- Layout fixes: an opened pinned question scrolls to its Ask line on a phone,
  and a double tap opens it without answering; the phone machine drawer sits
  above the Attention panel; Fleet twin tags fit their measured column;
  activity captions that do not fit are omitted; and a hidden terminal no
  longer adds to the conversation's scroll height.

### Fixed — factory reliability

- A stalled worktree provisioning no longer wedges the factory daemon. It runs
  off the loop with a deadline and cancellation, fails only that spawn with a
  named reason, and lets queued shutdowns and messages proceed. Spawns refuse
  below `factory.spawn_min_free_gib` (default 25) before staging a checkout.
- Daemon fork paths close idle shared database connections before `fork()`.
- Terminal prompt-queue rows are pruned on the maintenance schedule after
  `factory.prompt_retention_days` (default 7; 0 disables). Pending rows and
  relay deduplication keys are kept, and `gc_cleanup force=true` prunes
  instead of clearing all history.
- Wake delivery keeps its budget for a worker busy in a long tool call or
  still booting; `worker_status` reports `awaiting_first_prompt` for a booting
  Claude worker, and wakes revalidate the task's current recipient (#1054,
  #1101).
- Delivery-stalled notices coalesce until the recipient reads (#1119), skip
  informational Commander turns, and respect a recorded decision to hold a
  merge while naming the task's own delivery branch.
- One inbox message is no longer delivered on two channels (#1096). Creating
  a task with an assignee notifies that worker (#1123). A supervisor close is
  no longer credited to the worker (#1124).
- Recycling a clean worker keeps its task bindings and re-delivers its brief;
  a live supervisor can release a worker's lease (#1098). `shutdown_workers`
  retires the caller's own dead workers and cancels their unread mail.
  Replacing a dead QA reviewer releases its QA claim (#1089).
- Worker check caches are bounded (`factory.target_cache_retention_count`,
  default 1) and owned lane previews can be reclaimed, preventing per-worktree
  `target/` directories from filling the disk. Seeded worker builds no longer
  reuse stale Cargo freshness records after a branch change.
- `server_start` honors `name` and reports a process that exits during startup
  as a failure (#1102). `server_stop` stops `docker run` containers, including
  `--rm` and detached ones, and lets workers stop servers they own (#1099).
  `server_list` defaults to running servers with filters and a 50-entry cap
  (#1103).
- `factory.supervisor_only_mcp` and `factory.supervisor_only_env` keep named MCP
  servers and credentials on the supervisor; workers get a private MCP
  configuration outside the checkout (#1047). `factory.worker_credential_env`
  explicitly grants named operator credentials to Claude and Codex workers,
  with a names-only warning when one is missing.
- The worker workspace guard covers Codex `apply_patch` writes, judges `rm` as
  deletion (allowing stale Cassy runtime files but never live sockets, locks or
  pid files), expands shell variable assignments before checking paths
  (#1105), and scopes a worker's `cas update`. A Codex worker's `cargo check`
  is routed through the capped runner, and script-only `make test*` targets
  are admitted.
- Factory preflight no longer calls a newer released runtime stale when run
  from an older source checkout.
- Builtin guidance: supervisors merge non-Rust deliveries at once and prove
  parked Rust lanes in parallel; workers stop merge-poll loops after an
  awaiting-merge handoff; and the search-index error names the supported
  reindex action instead of a nonexistent command.

### Fixed — close, merge and QA gates

- Large epic overrides close within the MCP deadline: an assembled or released
  epic defers its build proof to its `ASSEMBLY_PROOF`, and the surface checker
  is bounded to 15 s (PR #1114).
- A task close or note that has committed answers `COMMITTED` within the
  deadline while post-commit work finishes in the background, and message
  enqueues report their commit and `notification_id` on timeout (#1083).
- Merge admission requires CI for delivery code beneath a docs-only tip,
  binds CI lookups to the canonical origin, and withdraws merge intent when CI
  refuses a delivery. A merge request is judged by its requested delivery,
  never by a stale branch's landed tip.
- Close attribution: rebuilt `hub-web/dist` bundles, epic content merged into
  a lane, and identical minified handoffs no longer read as dropped delivery;
  other tasks' merged work no longer counts toward a task's diff; and close
  resolves the branch whose commits claim the task. Epic close measures the
  epic tip, and a branchless no-code epic can close.
- Transfers store the worker's registered name, `adopt_branch=true` moves an
  inherited delivery onto the receiver's branch (#1100), and a re-park on a
  new branch records it as the delivery. Re-parenting moves a task's lane
  target, and a branch-only retarget no longer needs `target_repo` (#1107).
- Supervisors can stage deliveries into an integration batch and close them by
  an aggregate squash (#1097), close evidence-only report tasks (#1121), repair
  a locked execution methodology (#1118), and override-close a merged
  `commit_receipt` after the worker moved on (#1068). Stacked deliveries can
  park (#1087). Snapshot approvals accept a SHA-256 digest for long lines.
- QA: redaction placeholders pass the secret scan (#1069); waivers bind to the
  current tip after a rebase (#1048), to an explicit pushed tip before parking
  (#1122), and satisfy the ledger at the exact tip (#1110); per-render random
  ids are normalized (#1078); deliveries already squash-integrated or
  report-only are not re-dispatched (#1120); classification uses the fresh
  target (#1066); a supervisor `qa_request` opens one round past escalation;
  fixture HTML is not user-facing; follow-up tasks keep their text and epic;
  QA bundles share one artifact path (#1061); and Codex QA reviewers can
  record verdicts (#1111).

### Fixed — CLI, store and integrations

- `cas update` stops a project's later phases when its store migration fails,
  naming the processes holding the store, and migrates legacy stores first.
  Its compact view prints details only for warnings, failures and dry runs.
- Identity and config writes refuse when `CAS_ROOT` and the working directory
  resolve different stores, naming both and the `--store` choices.
- `cas init --force` creates and migrates a missing store while keeping its
  config. Automatic store discovery requires a store marker, so a socket-only
  `~/.cas` is skipped.
- Rule update, promote and delete wait out a concurrent writer instead of
  failing "database is locked".
- `proposal_accept` tolerates explicit nulls from older clients (#1043).
- `cas doctor` inspects the prompt queue without creating it, reports an
  absent queue as healthy and warns in plain words otherwise; it also probes
  a project's overriding hub URL.
- `cas integrate violet --json` prints only JSON. Violet guidance treats a
  failed or partial thread read as unknown, not "no new replies" (#1062).
- A PreToolUse publication gate denies Violet file posts and deliverable
  messages while the epic's verification is open, with a logged six-hour
  operator override (#1057).
- Duplicate-task warnings ignore generic words such as `NOT` and
  `before/after` (#1108).
- Code indexing scopes reconciliation and coverage to each checkout, so
  worktrees of one repository no longer retire each other's files.
- Saved hook defaults map onto canonical matchers, ending matcher churn in the
  tracked `.claude/settings.json`. `purge-foreign` refuses only for queued
  changes to rows it would delete.
- Ghostty builds use a vendored, pinned `uucode`, so they no longer fetch it at
  compile time.

### Fixed — Jev

- `jev.gate.shadow` (default off) logs Jev risk verdicts for Bash, Write and
  Edit without changing any hook decision; `cas jev gate-report` summarizes
  them locally (PR #1117).
- `jev files` reports capped input as incomplete, resumes sweeps with
  `next_offset` and can read one Git revision (#1116).
- The triage recipe abstains without dated, current-code evidence and
  requires cited sources for a suggested verdict (#1115).

### Fixed — QA and release tooling

- Visual QA judges clipping per axis (#1073), skips screen-reader-only helpers,
  intentional ellipsis and closed off-canvas drawers (#1081) and closed
  `<details>` content, and measures colours after transitions settle.
- Terminal QA supports BSD/macOS `script`, fails empty captures and checks
  stderr.
- Release tooling: `--cut --resume` resumes its own release PR; preflight
  checks the Violet announce token before publishing; the train's `--only`
  list matches every gate row; hub-web tests run when their inputs change;
  macOS date/path handling and the release-gate self-test pass on macOS;
  `find-release-prebuild.sh` resolves its repository or fails loudly;
  tracked merges run the target tree's own lane policy; and assembly explains
  metadata blockers.
- Self-hosted CI reclaims a slot job lock left by a crashed runner. Lane
  fast-row budgets scale with host load, and a lane proof's `--tests` step
  inherits its `--lib` step's load admission.

## [3.45.0] - 2026-10-02

### Added — Jev decision support

- Jev is available through the live Cassy cloud proxy using Cassy-login
  authentication. The upstream key stays on the server; team/user rate limits
  and request-size caps bound calls. Live checks verified unauthorized requests
  return 401 and authenticated requests return the pinned model and request id.
- `cas jev ask` / `batch` and the `jev` MCP tool share one client with typed
  answers, probabilities and confidence. Advisory callers receive an unavailable
  result when evaluation cannot run; bounded retries honor rate-limit delays.
  The local decision log records answers, request ids and a state hash, without
  recording state content or credentials.
- `cas jev files` and the MCP `files` action ask Jev about selected project files
  and return answers without returning file contents. Selection respects ignore
  rules and refuses secret paths, outside-project paths and symlinks before
  reads; file/byte caps and explicit skipped reasons bound each batch.
- The builtin `cas-jev` skill ships question-writing rules, confidence routing
  and the evaluated triage question set across supported integrations. Triage
  suggestions always require independent review, including at high confidence;
  Jev alone never authorizes destructive actions.

### Added — Jev evaluation

- A reproducible, two-run Jev triage report compares 172 reviewed tasks.
  Overall agreement is 119/172 (69.19%) before wording tuning and 121/172
  (70.35%) afterward; the held-out subset falls from 98/138 (71.01%) to 97/138
  (70.29%). Tuned high-confidence agreement is 28/32 (87.50%), covering only
  32 tasks. Estimated API cost totals $0.27674; median request latency is
  193.3 ms / 180.1 ms and p95 is 287.8 ms / 276.6 ms across the two runs.
  The report includes confusion matrices, all confidence buckets, the final
  question set and retrieval failures. Its recommendation is human-reviewed
  suggestions; this result does not establish an automatic-action threshold
  or a general benefit from wording tuning.

- A reproducible 208-case Bash/Write risk study compares Jev, the existing hook
  and a composite policy across two runs. At the prespecified default on the
  158-case held-out subset, the composite detects 60/60 destructive cases versus
  16/60 for the hook alone, while passing 71/78 trusted safe cases. The labels
  are study-authored and sparse real-command coverage limits generalization.
  This is shadow-only research: no Jev gate or runtime hook behavior changed.

### Fixed — proposed 3.45.0 scope

- PreToolUse denies parsed Bash writes to `.env` and configured credential files,
  consistently with direct Write. Protection covers redirects, `tee`, `cp`/`mv`
  destinations and recognized Python/Node file writes. Reads and quoted command
  text are unaffected.

- Visual QA redacts authorization and cookie headers and token values from
  its logs, and scrubs authentication data from traces it creates. If an
  existing runner owns tracing, QA continues without publishing a separate
  trace and warns that the runner's trace has not been scrubbed (#1076).
- Visual QA invoked through a symlink runs and produces its report instead of
  silently exiting successfully. Missing inputs and zero-capture runs fail
  with a diagnostic (#1050).
- Completed no-code tasks whose delivery branch has been deleted can close
  with valid external proof. Missing code-delivery evidence produces an
  explicit missing-branch error; retained anchors still require integration.
- Passed or waived QA remains valid after a squash merge when the integrated
  change is proven to match the reviewed delivery. Close no longer reports
  QA as both required and satisfied (#1045, #1052).
- `cloud.team_only` is a registered, usable setting. Team-linked projects
  opting in avoid personal-scope project calls during enqueue, push, pull,
  background sync and MCP startup. `cas cloud queue --purge-team-owned`
  removes only rejected personal queue rows marked `team_owned_project`,
  preserving underlying local records and team rows.
- Orphan recovery checks live process and terminal evidence before reopening
  work with an expired lease. Live work keeps its assignment, Blocked work
  keeps its blocker, and genuinely dead work remains recoverable (#1065).
- Build-check log guidance uses Git-ignored `target/` or task artifacts.
  The hook refuses unignored log destinations inside the checkout before
  they can dirty the clean-commit check; source changes still require a commit.
- CI impact selection recognizes directory-main integration suites. A hooks
  test change selects `integration_contracts` with the `hooks_test::` filter;
  malformed inventories still widen validation.
- Task lists report query-filter, quarantine and foreign-origin exclusions
  with counts, including empty results. Project/all scope is explained, and
  the display limit remains after filtering (#1090).
- Build previews resolve Zig from the configured path, PATH or the source
  checkout, including the main checkout when using a worktree. Zig is required
  only for projects using `ghostty_vt_sys`; a missing toolchain is named before
  Cargo starts (PR #1091).
- Large epic closes bound nested delivery checks and missing-reference fetches
  to one shared deadline. Unchecked children stay unchecked and appear in the
  decision audit; a supervisor override still cannot waive a measured blocker
  (PR #1092).
- Self-hosted release and Linux prebuild jobs serialize shared Rust toolchain
  setup. Existing stable installations are reused rather than auto-updated by
  concurrent jobs; incomplete installations fail with an all-slots-idle repair
  instruction. Hosted toolchain setup remains unchanged.
- Release-gate scripts work on macOS with portable timestamp parsing, native
  Bash installer handling and exact-tree journey validation. Linux-only
  behavior fixtures explicitly skip on Darwin; Linux runner implementations
  are unchanged. The 27-script Mac cohort and the full Mac Make wrapper pass;
  this does not claim Linux execution from Mac-only evidence.

## [3.44.0] - 2026-10-02

### Fixed

- Commander keeps a steady, honest connection picture. One word, "Unsteady",
  describes a machine that has missed heartbeats, and messages wait instead of
  going out while it is unsteady. A machine that drops and retries is shown by
  its banner alone, with no extra "Reconnecting to hub" card. Control held
  before the drop comes back with the machine, and a session whose machine
  socket is replaced mid-reconnect no longer sticks on "Reconnecting". Every
  connection card names the machine, never the hub or its host.
- A browser whose pairing is refused stays on "Needs pairing", with a re-pair
  note and card that no longer revert to "Lost connection" a few seconds later.
- Drafts survive a reload or navigation, per conversation. Waiting, unconfirmed
  and not-sent messages survive a reload too, each in its own state: a waiting
  one goes out exactly once when the session is back, and an unconfirmed one
  is never resent by itself. Revoking or removing a machine clears what was
  kept for it. Corrupted or oversized stored values are ignored safely and
  never sent. After a reload during an outage, the kept messages show at once
  instead of waiting for the session to come back.
- A refused or unconfirmed message says so once. Retry is not pressable while
  another device holds control, and several unconfirmed messages in a row
  show as one notice with Review; a later batch starts collapsed. The
  "not confirmed" marker uses a caution tone, and turns critical only when a
  message was not sent. A "Control released" notice no longer lingers once
  control comes back.
- A reconnect keeps the reader's place in history, "Load earlier" no longer
  comes back after the start is reached, and the start of history is shown.
  Opening a conversation with tall file cards lands on the latest turn.
- A reload keeps each turn's original time and keeps every turn in its own
  session's section, instead of re-stamping them to the moment of the reload.
- Opening a conversation shows one calm loading state, and an empty thread
  shows a clear card. A message held locally while disconnected no longer
  shows the supervisor as working.
- The composer refers to "the <project> supervisor" rather than a generated
  codename, shows a single "Sending…" while a message goes out, and returns
  focus to the reply box after dictation.
- The conversation header keeps the machine name whole and ahead of the
  session codename; when space runs short it drops the OS word first, then
  shortens the codename, and never cuts a word in half.
- The conversation list: a project's grouped session rows lead with each
  session's codename and what it last did; opening a lower row keeps it in
  view; opening a search result clears the filter; the row Enter will open is
  marked; each row's time is its latest message; and the list header stays on
  one row, with a "New session" control that says what it does.
- The Attention panel on a phone opens as a proper sheet from a badge, keeps
  focus and open Details through refreshes and after the phone wakes from
  sleep, and End session leaves focus on a sensible neighbour. Its "Last
  event" time uses the same 24-hour format as the rest of Commander.
- Re-pairing by code says plainly when starting sessions must be allowed
  again, and New session says when a machine is offline instead of hanging.
  That explanation survives a reload, and the re-pair command is shown whole,
  never split inside a word, with a Copy button.
- Paired machines lists a machine that can't be reached first, and the footer
  names it.
- Task close and verification bind to a parked delivery's own commit, so a
  worker moving on to other work no longer invalidates the delivery's proof.
- A message to a busy worker whose wake was declined is retried after the
  turn instead of being marked delivered unread.
- Release-gate checks no longer crash after a branch is force-pushed.

## [3.43.1] - 2026-10-01

### Fixed

- `cas update` refreshes skills in every project again when a Claude or Codex
  account profile's config file is a symlink to a shared file. The Violet
  migration edits the link's target in place, keeps the link and the file's
  permissions, and edits a target shared by several profiles once. A dangling
  link, a link leaving HOME and the explicit profile directories, or an
  unreadable profile file is skipped with a warning naming the file, and the
  rest of the update continues.
- Commander shows each session's own conversation. Messages from earlier
  sessions of the same project appear in a labelled, collapsed "Earlier
  session" section instead of as the current thread. A session that hasn't
  messaged Commander yet shows an honest empty state with its latest
  activity. Several live sessions of one project are grouped, with the most
  recent one marked, and a paired device can end a session after
  confirming.

## [3.43.0] - 2026-10-01

### Changed

- Violet is the only Slack hub name. The MechaCassy proxy registration,
  integration command, issues alias and redirect skill are retired.
  `cas update` migrates matching production-hub MCP entries in Claude and
  Codex profiles and project/user proxy files to `violet`, preserving credential
  references and custom upstreams. Existing `MECHA_*` credentials still work
  as fallbacks.
- Slack posts go through `violet.violet_post` by default. Pre-tool hooks block
  writes through the Claude Slack connector and Codex Slack app; set
  `cas config set slack.transport any` to allow another transport explicitly.
  Where native Codex MCP calls cannot run those hooks, generated guidance
  requires Violet and `cas doctor` warns about locally detected Slack-app
  access, noting that cached evidence may be stale.

### Fixed

- Factory pane text wraps to the visible width instead of being clipped,
  including after layout changes and a web viewer attaches.
- Clicking the ✕ on Claude Code's diff sidebar inside a factory pane closes
  the panel, including after reopening it with `/diff`.
- Closing large groups of tasks resumes from previously checked results
  instead of restarting and repeatedly timing out.
- Task close attributes snapshots and code changes to the delivered task,
  excluding unrelated snapshots, later tasks' commits and merged-in target
  content. Unrelated changes no longer trigger proof or approval demands.
- Closing a merged task accepts its group's assembly proof instead of
  requiring the same build checks again.
- Archiving a memory no longer reports "entry not found" after saving the
  archived state. Archived memories remain available by ID for inspection,
  re-tiering and restoration.
- The activity feed no longer shows push blocks from isolated test runs as
  live activity.
- `worktree_merge` accepts its documented `supervisor_override` and `reason`
  fields for a failed-CI exception, validates authority and the explanation,
  and records the decision.

## [3.42.0] - 2026-10-01

### Fixed

- `loop_start` no longer fails with `UNIQUE constraint failed: loops.id`.
  Loop IDs were the last four hex digits of the millisecond clock, so two
  loops started in the same millisecond, or a multiple of about 65.5 seconds
  apart, collided. New loops get random 64-bit IDs checked against existing
  rows; existing IDs keep working.
- Commander's footer says "Needs pairing", not "Reconnecting", when the only
  paired machine's pairing is revoked. Footer, header, machine rows and
  context now share one connection-label helper.

### Testing

- Tests that could pass without proving anything now fail when the behaviour
  breaks: six semantic-search tests that returned before asserting, the cloud
  sync CLI journeys (now wired into a harness, with required HTTP requests and
  durable read-back), PTY and no-ANSI smoke tests that matched the echoed
  command, and 41 ignored tests for removed CLI commands (replaced by 16
  current-seam tests).
- Environment-mutation hygiene: the MCP tool tests, hook handler tests,
  `cas-pty` and `cas-mux` hold the canonical `TestEnvGuard`. The rule-026
  baseline drops from 1,066 to 143 recorded sites, and `check-test-env`
  flags raw `set_var`/`remove_var` even while a guard is held.
- The consolidated scoped-proof inventory flake is fixed: its stub checker
  exited before reading stdin, so the resolver's write failed and it fell
  back to a wider target set.
- hub-web: source-order assertions are replaced with DOM and built-bundle
  journey assertions; the network-switch journey runs on one protocol clock
  with no fixed waits (121 s to 60 s); a draft-mismatch diagnostic attaches
  composer node identity to failing traces.
- `scripts/test-ci-test-tiers.sh` is split into parsed CI policy, executable
  contracts and reasoned prose pins. PyYAML is pinned and installed into an
  isolated venv only when it is missing.

## [3.41.0] - 2026-09-30

### Added

- `factory.release_gate_home_dir` sets the scratch base for the assembly
  proof's plain clone. The daemon passes it to daemon-initiated rolling
  integration as `CAS_RELEASE_GATE_HOME_DIR`; when it is unset the sweep
  reports `NOT CONFIGURED` naming the key, instead of `FAILED` with
  attribution probes.
- A supervisor can close a deliberately superseded delivery with
  `supervisor_override=true` and
  `reason="reviewed-drop: <SHA>[,<SHA>...] -- <why>"`. Each named commit must
  descend from the delivery anchor, be reachable on the target, change a
  dropped path against its first parent, and together they must cover every
  dropped path. A narrative without commit receipts no longer waives the
  content gate.
- `cas-tdd` references carry worked good/bad test examples in this
  repository's idioms and a system-boundary mocking guide.

### Fixed

- Task close no longer strands a merged delivery whose lines later commits
  deliberately changed. The content gate follows each delivered line through
  descendant history and credits ordinary replacements, the task's own conflict
  resolutions, additive list unions, and the task's own later QA rounds.
  Reverts, stale merges, plain deletions and another task's merge resolution
  still reject.

### Documentation

- The harness changelog diaries cover Claude Code through 2.1.285, Codex
  through 0.159.2 and Grok through 1.0.44.

## [3.40.0] - 2026-09-30

### Added

- A reviewer-accuracy evaluation replays 20 real past changes, 14 of them with
  known defects, through the two-axis reviewers and the legacy verifier.
  Both caught under a third of the defects (3 and 4 of 14), so reviews stay in
  shadow mode and no merge policy reads their verdicts.
- Lane merges that touch Rust need a capped compile proof of the combined
  merge tree before the epic moves. The Git merge helper checks the actual
  merge commit, and `release-train.sh --check-lane` requires the same receipt.
- The full release gate and the assembly proof run the CI script suites
  (`make test-ci-tiers`), so a script-test failure stops before the merge
  queue.

### Fixed

- A quiet terminal pane shows its last bytes: the PTY reader holds back only
  a real partial cursor-position request and flushes it when output goes idle.
- Release trains stop on a rejected branch push and refuse to enqueue a PR
  whose head differs from the gated commit. Reassembled release branches use
  the train's recorded push as an exact lease, preserving concurrent pushes.
- Post-publication waits up to 30 minutes for the Release workflow instead of
  10.
- Announcement lint rejects any unresolved `{{TOKEN}}` in all four Slack
  bodies immediately before posting. Preflight, which runs before
  publication, allows only the two checksum placeholders that
  post-publication fills.
- Adding a standalone task to an epic retargets its delivery to the epic
  branch; a task with a recorded delivery is refused with the exact supervisor
  repair command.

## [3.39.0] - 2026-09-30

### Added

- Factory workers can run targeted Rust tests for their change:
  `cargo nextest run -p <crate> [--lib|--test <harness>] -E 'test(name)'`,
  one package with a named filter, through the same capped runner as
  `cargo check`. Zero matched tests fail, and a clean-commit receipt records
  the result. The full suite stays with epic assembly.
- Two-axis review in shadow mode: separate Spec and Standards reviewers each
  commit their own fixes on side refs and cross-check the other axis. Only the
  supervisor can start, show or apply a review round, and no merge gate reads
  its verdicts yet.
- `CODING_STANDARDS.md`, read only by review: judgement calls, test standards
  and a code-smell baseline.
- The `cas-retro` skill runs an environment retrospective after a release and
  files tasks. The `cas-improve-architecture` skill reports module-deepening
  candidates.
- Pull-request and merge-request bodies carry a Summary visual, before/after
  Evidence and a Merge Danger section that shows the task's risk and an
  optional recorded `door` (one-way or two-way). No merge policy reads `door`.
- The no-build release-gate rows run at every lane merge and in Scoped
  Validation, including a test-shape lint for tests that restate constants or
  read source as text, and a builtin hygiene check shared with the Rust tests.
- A whole-workspace lint requires tests that change environment variables or
  the working directory to hold the shared test guard. A ratcheted baseline
  records existing cases, and it can only shrink.
- A release is complete only after the published binary and a clean install
  prove it contains every change merged to main since the previous release.
  An explicit announcement embargo holds Slack posts without holding
  publication.

### Changed

- The worker prompt, worker skill and `AGENTS.md` are roughly half their
  previous length; branch-only material moved behind pointers and every
  protected guardrail stays.
- A mechanical rule files its "encode as check" task on first occurrence;
  judgement rules still need two sources.
- Skill-text tests assert one registry of reasoned contract phrases instead of
  scattered wording pins, so skills can be reworded without breaking tests.
- The TDD, codebase-design, writing-for-agents and diagnosing-bugs skills
  gain material from mattpocock/skills v1.3 (MIT), including design-it-twice
  and a human-in-the-loop repro template.
- The factory daemon captures a terminal snapshot, drains pending output and
  queues both in one owned step; its test checks the order of delivered frames
  instead of reading source text.

### Fixed

- The release gate reuses a matching assembly proof, and a miss names the
  first differing input.
- A publication that exceeds the latency budget is recorded, not blocked, and
  the worker build-cache refresh receives Zig.
- `--cut --resume` after a gate failure fixed on the epic re-assembles instead
  of re-gating the stale tip.
- The assembly proof refuses a scratch root under a disposable directory.
- Durable task artifacts are namespaced per project.
- `proof_targets` given as a JSON array are parsed correctly, and stored
  legacy fragments are normalised once.
- A cloud team pull no longer moves a parked task backwards in its lifecycle.
- Worker-check locks no longer leak into a compiler-cache daemon.
- Re-launched child tests and the web test runners fail when zero tests ran.

## [3.38.0] - 2026-09-29

### Changed

- The Slack assistant is Violet everywhere. The builtin `violet` skill,
  `VIOLET_*` settings and `violet_read`/`violet_post` are primary. The
  `mecha-cassy` skill, `MECHA_*` variables and `mecha_*` tools keep working
  for one release. New projects file Violet issues in the `violet_ps` tracker.
- Integration tests build as 10 test programs instead of 107, cutting the link
  time of every test build. Every existing suite is still wired in; a check
  refuses any test file that no program includes.
- A release candidate is proved once. The assembly run (the full suite in the
  worktree plus the queue's archive runner in a plain clone) writes a receipt
  keyed by the tested code, and the integration sweep and release gate reuse
  it instead of re-running. The version bump and ledger do not invalidate it.
- Factory pushes and epic pull requests run only the tests affected by a
  change, widening to the whole workspace when unsure. The merge queue still
  runs the full suite. Pull requests into main are unchanged.
- Factory workers may type-check their change with
  `cargo check -p <crate> --lib` (or `--tests` when tests changed), capped by
  `max_concurrent_builders`. Every other build and test stays with assembly.
- After each release, the worker build cache is refreshed from the released
  commit, so a new worker's first check no longer recompiles the workspace.

### Fixed

- Messages to a Claude supervisor are no longer silently dropped. A test run
  with a temporary home could delete a live session's team files; cleanup now
  touches only files it owns, and a deleted team folder is rebuilt from the
  running session. A relay that cannot be delivered is shown as an incident.
- `release-train.sh --cut --resume` completes a release that stopped after
  publishing, instead of failing its clean-worktree check.
- Task closes no longer dead-end after a supervisor merges a branch by hand,
  after tests are consolidated, or after a reviewed conflict resolution.

## [3.37.0] - 2026-09-29

### Added

- Commander can enable starting sessions from a device that is already
  paired with full control (type, send messages and interrupt). The New
  session sheet offers "Allow starting sessions on <machine>"; after one
  confirmation the hub adds only the session-launch permission to that
  device, writes an audit row, and the launch sheet opens. Read-only devices
  still need a pairing invitation that includes session launch. The hub
  endpoint is `POST /v1/auth/scopes` with `{"add":["session-launch"]}`.

### Changed

- Commander history is one conversation per machine: every paired device
  sees every operator message and supervisor reply, each labelled with the
  device it came from, and a send from one device appears live on the others.
  Delivery receipts still go only to the device a reply was addressed to.
- Lines the operator types directly into a supervisor's terminal now appear
  in Commander history as Terminal turns. Machine-generated prompts (task
  notifications, relays, wakes, reminders) are left out.
- Factory workers default to Codex `gpt-6.1-sol` at high reasoning effort.
  The standard lane uses it, falling back to `gpt-6-sol`. A spawn without an
  explicit model honors `llm.worker.harness`, `llm.worker.model` and
  `llm.worker.reasoning_effort`, and those settings apply only when the spawn
  uses the configured harness.

### Fixed

- On Linux without a systemd user session, a session started from Commander
  now runs in the selected project instead of the hub's folder, so it appears
  and opens.
- A supervisor's pane answer is no longer dropped from Commander history when
  the same turn also sent a message to the operator, or when a thinking-only
  entry came before the answer.

## [3.36.0] - 2026-09-29

### Changed

- Every task, memory and rule now records the project that wrote it. A
  one-time upgrade step labels older rows from the folder of the session that
  wrote them; a row it cannot place is marked unknown, stays local and is
  never pushed to a team.
- Cloud sync pushes only rows this project wrote. Queued rows from other
  projects, or with an unknown owner, are dropped before every push and
  counted in `cas cloud status`. Rows queued before the upgrade take their
  owner from the stored row.
- Moving a task to another project deletes this project's cloud copy and no
  longer pushes a copy under the new owner.
- Only real projects are registered and refreshed: a Git top-level folder, or
  a folder whose `.cas/config.toml` pins a `canonical_id`. Container folders
  and anything under `~/Archive` are skipped, and one folder reached by two
  paths is registered once. `cas update` no longer links an unlinked project
  to a team.
- A Git repository without its own `.cas` no longer uses a parent folder's
  store; run `cas init` there. The error now reads "no Cassy store here; run
  `cas init`".
- Rows a push marked unrecoverable stay parked across upgrades until
  `cas cloud queue --retry`. Only rows refused for an old client version are
  retried automatically after an upgrade.
- Captured user prompts ("User request: …") are local only and never sync.
- Cassy starts every Claude session with Claude in Chrome enabled
  (`--chrome`), when the installed Claude supports it.

### Added

- `cas doctor` reports authored, foreign and unknown counts for tasks,
  memories and rules, with the command that fixes each.
- `cas cloud purge-foreign --include-unknown` also removes rows with an
  unknown owner (backup first, as for foreign rows).
- `cas cloud adopt-unknown` claims unknown rows for this project after a
  backup (`--dry-run` previews).
- `cas purge-test-fixtures` also removes the leaked MCP protocol test tasks
  and rule.

## [3.35.0] - 2026-09-29

### Added

- Commander can start a factory session on any paired machine, with no SSH.
  The New session sheet picks the machine, a project (its main folders, or a
  folder browsed under the machine's launch roots), the supervisor CLI
  (Claude, Codex or Grok), the account and an optional worker count. A
  project that already has a running session offers Attach instead.
  - The session keeps running after the browser closes and across hub
    restarts. On Linux it runs in its own systemd user unit, outside the hub's
    unit; on macOS it detaches into its own session. A small reaper records
    its exit, so Commander still explains why a session ended.
  - Starting sessions needs the new `session:launch` scope. It is never part
    of a default pairing; grant it with `cas hub pair --scopes …,session:launch`,
    and `cas hub auth revoke` removes it. Every launch is audited with the
    device, project, CLI, account and placement.
  - Every Claude and Codex account on the machine is listed. The machine's
    default is preselected, and logged-out accounts show their login command.
    Login state comes only from the official CLI, never from credential files.
- Hub endpoints for this: `GET /v1/projects`, `GET /v1/projects/browse`,
  `GET /v1/launch/profiles` and `POST /v1/sessions`, and `/v1/machine` now
  reports `default_supervisor_cli`.
- `[hub] launch_roots` sets the folders Commander may browse (default
  `~/Petrastella` when it exists; an empty list turns browsing off).
  `[hub.launch_profiles]` sets the account each CLI starts on.
  `cas hub service install` and `restart` record the installing shell's
  account when none is set.
- `cas doctor` and `cas hub service status` report, per CLI, whether a hub
  launch is ready and which account it will use.
- A per-project `cloud.team_only` opt-in in `.cas/config.toml`. With a team
  configured, project tasks, dependencies, memories, rules, skills and
  knowledge sync only to the team, and queued personal copies are cleared
  locally. Personal and global rows are held and counted rather than sent
  under a project identity. It does nothing until a team is set.

### Changed

- `cas doctor` no longer warns about a supervisor CLI that is not installed;
  it reports it as "not installed". An installed CLI that cannot launch
  (logged out, or its account folder is missing) still warns.
- `cas hub service install` and `restart` keep going, with a warning on
  stderr, if they can't record the launch account.

## [3.34.1] - 2026-09-28

### Fixed

- A macOS hub no longer stops answering while its process stays alive
  (Commander showed Degraded). The hub's session read model runs on the
  blocking pool with single-flight instead of on the async workers, so a
  stalled SQLite close or open can no longer starve the reactor, and
  `/v1/health` keeps answering. Callers wait at most 20 s and join the
  running read.
- Listed projects' agent registries stay open across session-list passes
  instead of being closed and reopened every second, and are released when
  the project is no longer listed.
- The shared SQLite pool now owns every close. Connections close only
  under their database's lock, the same lock opens take, through an idle
  sweep (60 s TTL, at most 16 idle), so a close can no longer race an open
  of the same file. The CLI closes idle connections on exit, and
  `cas update` backup and rollback close them before copying `cas.db`.

## [3.34.0] - 2026-09-27

### Added

- Failed sends in Commander can be dismissed with a swipe or a × button, and
  a chip counts them and restores them. A pinned question folds to a one-line
  bar that can be dismissed, and it keeps its choices while the supervisor is
  still waiting on it.
- Independent QA reviewers get a preflight for credentials, env files and
  test-account capacity. Deployed authenticated staging runs satisfy the
  evidence gate when local auth is impossible, and supervisors can request QA
  for a user-facing park without a demo.
- `cas hub status` and doctor show a failing hub audit writer, and doctor
  flags an inactive hub service.
- `cas config get`, `set` and `list` cover every settable `factory.*` key,
  including `factory.epic_base_branch`. An unset `factory.artifacts_root`
  reads as its default (GH #1011).

### Changed

- Independent QA judges only what the delivery changed. Only regressions it
  introduced, or unmet acceptance criteria, reject it; pre-existing findings
  become linked follow-ups.
- Raw GitHub merges and taskless worktree merges are held to the independent
  QA verdict (GH #1023, #1024).
- Commander leads with the project in the machine drawer, the Fleet overview,
  headers and the composer. Codenames stay whole, and a machine name that
  cannot fit steps aside. The Fleet overview reads as a product page, and
  plot rows that share a project carry the shortest distinct codename tag.
  When one codename runs on two machines, each row also carries a short
  machine tag (the rail initials, a short name prefix such as "Atl" or
  "Att", or an ordinal), capped so it stays whole at any width.
- The Terminal header shows Checking… until the first latency sample and
  then the machine's own state, names the palette shortcut as Ctrl K or ⌘K
  per platform, and yields the title before Back or ⌘K on narrow screens.
- A detached `cas hub serve` writes its traces to `~/.cas/hub/logs` instead
  of the project it was started from.

### Fixed

- The hub, the MCP server, the bridge and the factory daemon ignore SIGPIPE,
  so a peer that goes away mid-write no longer kills them. The systemd unit
  restarts the hub after a signal death.
- Commander survives network switches: a half-open socket is probed and
  replaced, and messages written while it is in doubt are held and sent
  exactly once.
- A send the hub refuses because the session's daemon link is missing is an
  audited, retryable `upstream_unavailable` rather than `forbidden`. It is
  held and resent once the session is live again. While the link stays
  down, the page backs off its retries (about 1, 2, 4, then 8 s), and a
  message written to the legacy socket after the refused one is held too.
  The backoff starts afresh once the session has stayed live for 10 s, so the
  next brief drop retries within about a second.
- A held message that waits past two minutes says Not sent because the
  session didn't come back, with Retry, instead of advising to re-pair the
  device. The composer no longer promises it will go out by itself.
- When only one conversation's daemon link drops, Commander says
  "Reconnecting to" that conversation in the banner, the composer note and
  the disabled controls, and the machine stays Connected. "Lost connection
  to <machine>" is kept for a real machine drop.
- A revoked pairing reads "needs pairing again" everywhere, with Re-pair on
  the banner, and nothing on screen still says reconnecting. The auth-loss
  alarm resolves once a pairing works again.
- The factory daemon answers a resent `SendMessage` with its first receipt
  and queues it once.
- A refused DPoP proof is retried instead of being read as a revoked
  pairing, so a phone waking from a long idle no longer goes dark. The audit
  row records the denial reason.
- An older client decodes a Welcome that advertises an unknown capability.
- Not confirmed settles once the supervisor replies, a late receipt shows
  Delivered, and a settled card offers a quiet Send again. An acknowledged
  blocker reads as handled.
- Load earlier keeps the reading position. Keyboard focus lands in the
  conversation or on the control after pairing, opening a conversation or
  reaching the start of history.
- A file that fails to open says so on its card and leaves no tab, and
  outage notes clear on reconnect.
- Close no longer reports a hunk the task itself rewrote as DELIVERY CONTENT
  DROPPED, and the refusal names the missing lines. B2, the pre-close check,
  QA and merge honour per-task delivery branches (GH #1040).
- Close receipts follow the task's commits and targets for merge-tip proof,
  epic close headlines, target-sync merges and historical closes (GH #895,
  #1018, #1028, #1038).
- visual-qa parses OKLCH colours, accepts single-source JSON, ignores
  non-content surfaces and counts final Playwright Expect outcomes (GH #1013,
  #1017, #1025, #1027, #1037). Declared journeys capture interaction, loading
  and error states.
- Shared-clone supervisors can no longer reap each other's workers. A
  supervisor decision, or an approved verification commit, wakes a worker
  parked on it (GH #1036). Codex workers no longer read as dead after MCP
  reparenting.
- Cross-session reminders reach their verified recipient, and the rolling
  sweep finds its runner (GH #1039).

## [3.33.0] - 2026-09-25

### Added

- `cas-qa-craft` can generate a verification kit for any project: a
  `docs/qa/verify.md` recipe (Launch with a readiness signal, read-only
  Doctor, Drive, Evidence, Cleanup) and a `docs/qa/features/` map with one
  file per user-facing feature (Sub-features, How to get to it, Driving it,
  Gotchas, Touches). Generation is a capped background chore and never a
  close prerequisite. A per-release sweep reference triages drift as doc
  drift, harness gap or product regression.
- `scripts/check-feature-map.mjs` in `cas-qa-craft` statically checks a
  feature map: the five headings, Touches globs that match files, and an
  index that lists every file. It warns when a route or selector named under
  "Driving it" no longer appears in the touched source. Node only, no
  dependencies; projects without `docs/qa/features/` are skipped.
- An opt-in close gate for projects with `docs/qa/features/`: when a
  delivery touches a feature's Touches globs but not its feature file, close
  names the stale feature files. A `decision` note containing
  `map unchanged: <reason>` accepts the delivery as is.
- A `cas-why` skill answers "why was this built this way" from blame,
  history with provenance, tasks, memories, specs and PRs. Each claim is
  tagged Documented, Inferred or Unknown, and the answer ends with a coverage
  map that keeps the sources that found nothing.
- Session-learn runs on a cadence: an automatic run needs
  `memory.session_learn_min_turns` completed turns (default 10) and
  `memory.session_learn_min_minutes` minutes (default 120) since the last
  run. A per-transcript index means each run reads only what is new, and
  entries for deleted transcripts are pruned. Nothing changes when
  session-learn is off.
- The learning and rule reviewers ask whether a lint, test, close gate, hook
  or schema could enforce a lesson before writing it as a rule, and tag such
  rules `enforceable:<lint|hook|gate|type>`. Promoting a tagged rule with at
  least two sources files one "encode as mechanism" chore, never a
  duplicate.
- A builtin text lint flags stock AI vocabulary and abstract metaphor nouns
  in shipped skill, agent and job text, with a file-and-phrase allowlist
  that carries a reason for each entry.
- The handoff memory template has a four-part brief: capsule, threads with
  one status tag each, problems, and one next move.
- `verify-before-claim` gives every claim a VERIFIED, NOT VERIFIED or
  INCONCLUSIVE verdict, scales proof to blast radius on a four-rung ladder,
  and asks for a same-command baseline and treatment for measurable claims.
  The task verifier and the QA evidence gate use the same verdicts.

### Changed

- `worktree_merge` refuses a branch whose lane CI is already red and names
  the failing run. A registered supervisor can override with
  `supervisor_override=true`, a task id and a non-empty reason, which is
  logged as a decision note. Pending CI still merges with the advisory note
  and never waits; green and unavailable results merge as before.
- Close refusals that tend to repeat end with a stop: after the second
  refusal, write the working premise as a decision note and compare what
  the gate checks with what changed. The routine merge-required handoff
  omits it. A principles reference ships with `cas-codebase-design`, linked
  from `cas-tdd` and `cas-diagnosing-bugs`.
- Worker close reasons start with PASS or ISSUES, then the commit and how it
  was checked. Supervisor guidance retries by failure mode with a two-retry
  cap, requires goal, scope, acceptance and proof in every brief, and
  re-verifies by patch id after a rebase.
- Builtin tests assert behaviour (tool-call shapes, registered files, routed
  references, link targets) instead of freezing skill sentences, so
  rewording a skill no longer fails a release gate. Literal substring
  probes in the scoped tests went from 1,026 to 809; cross-file relation
  pins and the SessionStart size budgets stay.

### Fixed

- The `search` tool's `doc_type` description now lists `spec` and
  `artifact`, which the handler already accepted.
- Removed dead code from `hub-web` (four one-off QA scripts and e2e-only
  exports) and `slack-bridge` (five exports used only in their own file).
  The built hub is byte-identical.

## [3.32.0] - 2026-09-25

### Added

- `cas doctor` reports `host install parity`: installed skills and agents
  that differ from, are missing from, or are no longer in this binary's
  catalog.
- The visual-QA and terminal-QA scripts now ship with the `cas-ui-craft` and
  `cas-cli-craft` skills, so projects outside the Cassy repo can produce the
  receipts their close gates ask for.
- The design language ships a ready-to-paste `tokens.css`, and the image
  helper takes `--aspect` and `--size`.
- A new `factory` MCP tool holds the supervisor's fleet control: spawning and
  shutting down workers, worker and epic status, worktrees, servers, database
  branches, loops and queues. `coordination` keeps identity, messaging and
  reminders.
- A call-shape lint checks every suggested `<tool> action=` call in the
  shipped skills and in the runtime prompt templates. Each call must use a
  dispatched action, known parameters, and the parameters its handler
  requires.
- An operator-data lint keeps real names, ids and project data out of the
  shipped skills.
- `rule action=promote` records a reviewer's promotion decision, where
  promotion used to count a single helpful vote.
- User-invoked-only skills ship `agents/openai.yaml` with implicit invocation
  turned off, so Codex no longer triggers them on its own.
- Factory sessions log a `session_start_fired` event per role. It shows
  whether each harness actually received its SessionStart context.

### Changed

- The release-report skill starts from `cas release report <version> --pdf`
  instead of a hand-written render and pasted PDF programs.
- Report, figure and screen skills share one form table, and `DESIGN.md`
  follows the public DESIGN.md format and its linter.
- The built-in skills are one tree for every harness. Skills name tools by
  bare name, and each harness's tool prefix is stated once in its role
  guidance. The per-harness twin copies are gone, except the few whose
  content really differs; the task-verifier frontmatter is generated per
  harness.
- `AGENTS.md` is the canonical, harness-neutral instruction file. `CLAUDE.md`
  imports it and adds only Claude-specific lines, and each stays under
  10,000 characters. `cas update` removes duplicate `CLAUDE.md` blocks in
  subdirectories, previews the same plan in a dry run, and checks that
  `AGENTS.md` is current.
- SessionStart fits under the 10,000-character hook cap for every role (the
  assembled payload is at most 9,216 bytes). Knowledge and Handoff degrade to
  summaries with a pull command, the usage reminder is 214 bytes, and the
  always-loaded worker skill body went from 7,989 to about 6,490 bytes.
- The MCP tool list is smaller. Every tool description is at most 2,048
  characters (1,500 for `coordination`), and each `action` parameter
  publishes an enum generated from its dispatch table. Schema boilerplate
  (`nullable`, `"default": null`, non-standard formats) is stripped.
- The `coordination` schema publishes only its own parameters, each with a
  description written for its own actions, so workers no longer load the
  supervisor's spawn, worktree and server parameters.
- task-verifier went from 27.7 KB to 9.3 KB. It diffs against the task's
  delivery base, runs with a tools restriction, and escalates checks it could
  not exercise to the supervisor instead of approving or rejecting them
  silently.
- session-learn classifies with its own prompt instead of the skill text, and
  a partial draft no longer drops the whole batch.
- Each Stop-hook job has one body, remapped to the running harness's tool
  prefix.
- Claude, Codex and Grok workers receive one rendered contract with the same
  rules.
- Close-gate refusals come from one renderer per refusal family. Every
  suggested command is complete and carries the reader's tool prefix, and no
  remedy names a call the reader cannot make.
- A worker's close now defers `risk=platform` proof to the assembly build of
  the epic, because workers cannot run a platform build themselves.
- The `cas` skill that `cas init` writes is now a short pointer to the
  built-in skills. Its `task action=create` example no longer passes the
  unknown `start` field. Shipped text no longer uses the retired `mecha_cassy`
  key.
- Skill descriptions are YAML-safe and at most 250 characters, and
  `managed_by` moved under `metadata`.
- The `release-notes` skill is renamed `cas-release-notes`, because the old
  name collided with a Grok built-in command. The mecha-cassy skill now owns
  only Slack transport; the release rubric owns wording and order.
- The factory-core, workflow, tooling, design and report skills went through
  an accuracy pass, so their commands, parameters and paths match the code.
  For example, `codex exec` closes stdin, Viktor calls retry safely with an
  idempotency key, and task creates declare `risk`.

### Deprecated

- The `coordination` actions that moved to `factory` still work through
  `coordination` for this release. They return a note naming the `factory`
  call and will be removed in a later release. The moved actions are:
  - `spawn_workers`, `shutdown_workers`, `recycle_worker`, `hold_worker`,
    `release_worker`
  - `worker_status`, `worker_activity`, `epic_status`, `focus_epic`,
    `sweep_tasks`, `sync_all_workers`, `clear_context`
  - `gc_report`, `gc_cleanup`, `restart_spawn_queue`
  - `agent_list`, `agent_cleanup`, `lease_history`
  - `server_*`, `db_branch_*`, `worktree_*`, `loop_*`, `queue_*`

### Fixed

- On machines with Grok or OpenCode installed, `cas update` now writes their
  own skill copies into `.grok/skills` and `.opencode/skills`. Both load a
  project's own copy ahead of `.claude/skills`, whose tool names do not work
  for them.
- Codex no longer receives `.md` agent files, which it ignores; the old copies
  are removed on the next `cas update`, and the Codex supervisor guidance
  lives in its checklist.
- A team pull no longer replaces one of this project's rules with a legacy
  team rule that shares its id but names no project.
- Rules that name another project are no longer pushed to the cloud from
  this one.
- Pulled knowledge pages record the project that wrote them; pages from
  older clients are kept locally but never pushed back out.
- Rule and knowledge writes that name another registered project are refused
  with an actionable error, and are kept out of rule-file sync.
- `cas update` now refreshes every file a built-in skill ships, including
  scripts, examples and templates, which previously stayed at their first
  installed version. Local edits are still preserved.
- `cas update` removes retired Cassy agents and files a built-in skill no
  longer ships, and retired built-in skills without the `cas-` prefix.
- Links in the supervisor and worker skills now resolve in the installed
  layout.
- Close and merge gates key off the current tip (GH #1022). Merge-request
  suppression compares the pushed tip, not a stale recorded one. A close no
  longer asks for verification before its commit receipt is reachable. Merging
  one sibling no longer invalidates the other pending verification dispatches.
- Suggested calls now work for the agent that receives them:
  - Claude workers in a Codex-default session get their own tool names.
  - Every suggested message carries the required `summary`, and every
    reminder carries its `remind_message`.
  - Verdict guidance names the `dispatch_id`.
  - Local-merge workers are no longer told to push.
- The worktree merge jail points at `factory action=worktree_merge` instead
  of an agent that does not exist.
- The SessionStart pull commands name real actions (`system action=proxy_list`
  and `skill action=show`).
- A pre-assigned worker no longer receives a second assignment prompt after
  its spawn brief.
- `verification action=add` accepts `files_reviewed` as an alias for
  `files`, so older verifier calls no longer drop the list silently.
- Ambient recall strips message-envelope markup from its queries and skips
  evidence the current turn already names.

## [3.31.0] - 2026-09-24

### Added

- Commander opens published reports through a signed, team-scoped link.
- `cas integrate violet` adds Violet as an integration with its own issue
  routing and tools; the previous MechaCassy names remain available for one
  release.
- A disposable Neon database branch can be provisioned for one task, keeping
  its SQL work away from production.
- Read-only proxy access exposes runtime errors and logs without granting
  write access.

### Changed

- Commander shows a supervisor session even when it has no workers, leads
  with the project in the terminal, picker and command palette, and keeps
  conversation turns in order across reloads and clock skew.
- A team-linked project syncs in team scope only; it no longer mixes personal
  push or pull into that project's sync.
- Memory keeps one current handoff per project and role while retaining older
  handoffs as superseded history. Task creation, start and spawn surface
  relevant rules and memories more reliably.
- The release train runs on stock macOS without manual GNU-tool or session
  shims. A guarded release checkout hands tag publication to an operator
  before the audit instead of failing at the final push.

### Fixed

- Commander no longer offers live controls for a disconnected session. A
  failed send becomes "Not confirmed" with Retry instead of spinning forever;
  refused replies explain the next action and keep keyboard focus there.
- Commander clears the command palette after a jump, dismisses the phone
  keyboard after an Enter jump, keeps notices clear of headings, and sizes
  phone actions for touch. Long names and composer hints stay readable.
- Opening a report now distinguishes a Cloud failure from a machine that did
  not answer. Fleet summaries no longer start with a stray separator.
- On macOS, `cas-update` no longer claims it verified old running processes
  without inspecting them. It names the processes to restart manually and
  finds the source checkout through the explicit setting, its own Git tree,
  or the existing default path.
- A rejected independent review can be reclaimed after its task resets, and
  re-parking a changed commit retires the earlier round. Close gates judge
  only the task's own diff and report required proof targets.
- Spawn requests reject an undeliverable `prompt` instead of silently dropping
  it, and their receipts state whether a brief was delivered. Assignment
  warnings no longer mistake a path mentioned in prose for an output location.
- Task transfer accepts a worker name and moves a blocked task. Merge recovery
  keeps the rebased delivery attached to its task, and the integration sweep
  no longer blames a failure on the base when that test exists only in the
  new changes.
- Tag reminders wait for the published tag, red-run notifications require
  the branch tip and a failed job, and inbox delivery no longer buries the
  message that triggered a wake behind old replays.
- Retrieval evaluation uses one clock across its modes, and timing-sensitive
  terminal and preflight tests are more reliable on loaded machines.

## [3.30.0] - 2026-09-24

### Changed

- Commander (the hub web app) got a full polish pass from the journey
  evaluation:
  - Long machine and supervisor names wrap or ellipsise instead of clipping.
  - On a phone, the "Jump to latest" chip and the "connected" toast no longer
    cover the thread or its header.
  - The session picker shows every session's details at 390px and reopens
    with an empty filter.
  - Keyboard focus lands somewhere real on every route and survives page
    rebuilds.
  - Forced-colours mode keeps opaque focus rings and a highlighted open row.
  - Each machine keeps the accent colour it got when it first paired.
  - A refused reply states its reason once.
  - Live turns show the browser's clock instead of a skewed machine clock.
  - During an outage only the terminal dims; the conversation stays readable.

### Fixed

- A team pull can no longer overwrite a local task with another project's
  task that happens to share its ID. Rows that carry no project are set aside
  instead of being claimed by the puller.
- A factory worker's Neon SQL write that resolves to the production branch is
  refused.
- `worktree_merge` never merges a task that was moved into an epic straight
  to `main`. The task's delivery target follows it into the epic.
- After a rejected independent QA round, re-parking always opens the next
  round.
- Task writes wait out SQLite contention instead of failing with "database is
  locked".
- After a task is handed from a worker to the supervisor, closing it measures
  the branch that actually holds its commits.
- The hub web journey suite runs on per-checkout ports and never tests
  another checkout's server.
- The systemd hub restart test no longer kills itself on a machine that is
  running a real hub.

## [3.29.1] - 2026-09-24

### Fixed

- On macOS, a hub restarted by `cas update`, `cas hub restart` or the hub
  service no longer comes back reachable only on this machine. When the
  service's limited PATH had no `tailscale` command, Cassy used the one
  inside the Tailscale app. Without a terminal setting, that command tries to
  open the Tailscale window instead of answering, so the Tailscale route was
  never published and Commander showed the host as unreachable. Cassy now
  prefers the Homebrew `tailscale` command, always runs Tailscale with a
  terminal setting, and adds both to the hub service on every restart.

## [3.29.0] - 2026-09-24

### Fixed

- `cas update` now leaves the Commander hub running and verified, whatever
  state it was in before. A hub from an older version that has stopped
  answering, is stuck starting, or has died and left its lock or Tailscale
  route behind is stopped, restarted on the new binary with the same
  Tailscale setting, and checked. The checks cover the lock, the local
  health check, the Tailscale route and the public URL. If the check fails,
  evidence is saved under `~/.cas/hub/` and one recovery is tried. A healthy
  hub that is already current is only checked, so an update no longer drops
  Commander. If only the public URL cannot be reached (for example with
  MagicDNS off), the hub is reported healthy with a warning.
- On macOS, `cas update` and `cas hub restart --tailscale-serve` now update a
  hub service that was installed without Tailscale, instead of refusing, as
  Linux already did. The service takes over a hand-started hub safely,
  including a stuck one from an older version, and names every lock holder
  when it refuses. The service can now find the Homebrew `tailscale` command.
- `cas hub status`, `cas hub authorize` and `cas hub stop` now report the
  hub's real state: stopped, starting, stuck starting, shutting down, alive
  but not answering, or running. Each state comes with the matching fix.
  Previously a live hub that had stopped answering was reported as exited.
- A forced hub restart or stop no longer leaves a Tailscale Serve route
  pointing at a dead port. The next start reclaims a dead Cassy route left
  by an older version, and routes set up by anyone else are never touched.
- A hub started with `--tailscale-serve` now gets enough time to finish its
  Tailscale setup and reports "starting" instead of failing early.
- On macOS, `cas update` removes the download quarantine flag from the new
  binary and records how long its first launch took.
- On macOS, a worker that is still running is no longer cleaned up as dead,
  and `cas factory kill` checks the process identity before stopping it.
- On macOS, image generation, technical drawing and background maintenance
  jobs work with the stock system tools (Bash 3, BSD `base64`, and no `flock`
  or `timeout`).
- On macOS, supervisor memory writes and release-worktree discovery work
  under the `/var` → `/private/var` path alias.
- A merged task's delivery record no longer moves when its worker reuses the
  branch for its next task.
- Closing a task no longer rejects a passing proof note because a test name
  contains "failure", or because it uses a short commit ID.

### Added

- Commander now shows every supervisor reply. At the end of each supervisor
  turn, its final reply is posted to the paired Commander's conversation
  history, as an answer when Commander started the turn and as a status
  update otherwise. It carries the reply text only, with secrets redacted,
  capped at 4,000 characters and never duplicated. It starts once a Commander
  is paired and never applies to workers.
- A supervisor can close a task whose scoped test run fails only on tests
  that already fail at the base commit. Both runs must use the same command,
  and the decision is recorded on the task.

## [3.28.2] - 2026-09-23

### Added

- Independent QA pass: when a factory delivery changes something a person
  sees, Cassy opens a review for a different agent before the change can
  merge. The reviewer walks the demo and the nearby cases (empty results,
  phone width, dark mode, keyboard only, reduced motion) on the real build,
  scores the polish, and approves or sends the work back with evidence. The
  implementer can never review its own work, and a supervisor can waive a
  review only with a logged reason. Settings: `qa.independent_pass`,
  `qa.user_facing_paths`, `qa.pass_timeout_mins`, `qa.max_rounds`.
- Evidence at close: a user-facing delivery cannot close without a QA
  evidence bundle for its exact commit. The bundle holds a Playwright trace
  with at least one passing check, a screen recording, the final
  accessibility snapshot, light and dark renders at desktop and phone widths,
  a strict visual-QA pass and a critique score. Demo-only changes outside the
  web need a real-build ledger entry, plus a terminal QA receipt when they
  change terminal output. Newly skipped tests are refused unless marked with
  a reason. Setting: `qa.evidence_gate`.
- Hub user journeys: the eleven hub flows people rely on each have an
  end-to-end Playwright test, and a release that changes the hub stops until
  a scored walkthrough of those flows covers the new build.
- `cas-qa-craft` documents the evidence bundle, the independent review and
  the journey walkthrough in all three harness flavours.

### Fixed

- The hub command palette works properly again. Typing narrows the list to
  matching commands and sessions, session matches come first, the panel stays
  still while you type, and a search with no matches says so. Pressing Enter
  opens the chosen conversation, and a keyboard or mouse jump puts you straight
  into its reply box; a touch tap opens it without raising the keyboard.
  Reopening the palette starts from a clean, unfiltered list, and live
  updates no longer wipe what you typed.
- In high-contrast (forced colours) mode the command palette and session
  picker have visible edges, a full focus ring, and a clearly marked open
  session that stays readable on hover.
- A QA evidence ledger written exactly as the `cas-qa-craft` guide shows is no
  longer rejected at close.
- The scoped-proof surface check now qualifies nested test modules, so their
  tests are no longer missed.
- A merged task no longer gets stuck when its declared test scope or risk was
  too narrow. Supervisors can correct scope or risk on a merged delivery, or
  close with a logged override that still requires the real test run.

## [3.28.1] - 2026-09-23

3.28.0 was tagged but never published. 3.28.1 ships every 3.28.0 change listed
below, plus this fix.

### Added

- `cas-playwright-debug`: a built-in skill for Playwright projects that works
  from the saved trace (`npx playwright trace`), reproduces with
  `--debug=cli`, and controls flakes with test locks and isolated retries.

### Changed

- `cas-nuxt-playwright` is rewritten for Playwright 1.63: web-first waits
  instead of `networkidle`, `.visible()`, accessibility snapshots, test locks,
  isolated retries, the clock and storage APIs, passkeys, drag-and-drop uploads
  and the new component-testing model.
- `cas-frontend-engineering` turns each design promise into a concrete
  Playwright check, including reduced-motion, forced-colours and high-contrast
  runs and loading, error and empty states.

### Fixed

- Publishing a release no longer breaks when a commit message contains quotes
  or other shell characters: release notes are passed as a file, and no
  workflow step pastes GitHub values into shell code.

## [3.28.0] - 2026-09-23

Tagged but not published; these changes ship in 3.28.1.

### Changed

- New default model lanes: everyday coding runs on GPT-6 Sol (medium) with
  GPT-6 Luna (extra-high) as fallback; the supervisor, design and hard-problem
  lanes run on Claude Opus 5.5 (high), falling back to Fable 5.1, Opus 5 and
  GPT-6 Astra respectively; small chores run on GPT-6 Luna (extra-high) with
  Opus 5.5 (low) as fallback.
- Claude Haiku is no longer used anywhere. Requests for it are refused with a
  pointer to the light lane.
- Session summaries, learning and rule reviews and duplicate detection run as
  background jobs on the light lane after a session ends, instead of pausing
  the end of the session.

### Added

- Claude Code 2.1.280 is a validated version. `cas factory doctor` and factory
  preflight warn, without blocking, when the installed Claude Code is older and
  suggest `claude update`.
- The Codex check now follows the standard lane, verifies complex tool
  arguments end to end, reports token-budget stops as `budget_aborted` instead
  of a stall, and keeps unattended Codex workers from pausing for input.

## [3.27.8] - 2026-09-22

### Fixed

- The selected conversation row and the active machine icon carry a visible
  accent edge (7:1 or better in light and dark); they were marked only by a
  faint fill.
- The factory's stalled-supervisor nudge no longer tells the supervisor to
  assign a task whose start dependency is still in progress.
- The release train no longer deadlocks when it heals a stale assembly base,
  and its host-update stage installs the release and proves the binary, the
  running hub and the project refresh all report the new version.

## [3.27.7] - 2026-09-22

### Changed

- Commander design polish, from a full light/dark, phone/desktop design
  review: an unanswered question appears once (pinned above the composer) with
  a short pointer in the thread; long or question-bearing status updates read
  as normal replies and short ones clamp at three lines with "Show full
  update"; primary buttons use the operator colour and are the strongest
  control on every screen; the phone thread header is one row (121 to 56 px);
  dark mode tints questions and blockers instead of flooding them, keeping
  Send the brightest element; the desktop context rail shows only what the
  header does not and folds away when empty; the conversation list keeps times
  on unread rows, never breaks codenames, and moves Appearance & commands to a
  header button.
- Mic states: unavailable is a quiet dashed ring; listening is a red fill with
  a halo and a "Listening" placeholder.

### Fixed

- A message that failed to send looks failed ("Not sent", Edit, Retry), and a
  failed reply no longer unpins its question or clears a blocker.
- Readability: every placeholder, the pairing email field, "Loading earlier",
  and the composer edge meet contrast minimums in both schemes (no text below
  4.5:1 across 166 measured renders).
- An unreachable session with a pending message shows that state on its list
  row instead of the last message.
- Visual QA renders the production composer and pairing markup, and picks the
  newest stable Playwright rather than an arbitrary cached copy.

## [3.27.6] - 2026-09-22

### Fixed

- The hub lists every session whose supervisor is live, even before it has
  spawned workers or after its workers have all been retired. Only sessions
  with neither a live supervisor nor workers stay hidden by default; the
  worker-only and dormant reveal switches are unchanged.
- Older plain-text status replies that begin "Status 13:2xZ." now get the bold
  lead line and list layout (the lead pattern accepted only two-digit minutes),
  and an empty conversation shows the empty state instead of the
  "No earlier history" marker.
- The release train keeps the recorded supervisor identity for the assemble
  self-heal's integration recovery and sources its date helper in
  post-publication.

## [3.27.5] - 2026-09-22

### Added

- Hub conversation history is scoped to the project instead of one factory
  session: opening a conversation on a paired phone replays every message the
  operator sent and every reply addressed to that device across all of the
  project's sessions, oldest to newest with paging, a light divider at each
  session boundary, and a "No earlier history" marker after the oldest page.
  Device scoping and pane-text exclusion are unchanged. A machine's running
  session must be restarted on this version for the wider replay to apply.
- Plain-text replies from sessions started before the reply contract render
  readably: a leading "Status HH:MMZ." or "LABEL:" fragment becomes a bold
  lead line and "(1) … (2) …" sequences become a numbered list; bodies that
  already contain Markdown are untouched.

### Fixed

- The hub conversations list clamps each preview to two lines (and 160
  characters), so one long reply no longer fills the phone screen.
- The release train records the factory session at cut start and passes only
  it to the assembly self-heal, runs the version bump inside the release
  worktree, and names the offending line when the announcement lint fails.

## [3.27.4] - 2026-09-22

### Added

- A worker whose delivery pull request merges is woken by the daemon with the
  exact close command, and the supervisor is relayed if the task is still open
  five minutes later. Delivery PR numbers and merge commits are recorded on
  the task.
- Starting a task on a reused worker branch reconciles the branch first: a
  tip whose prior delivery is already merged is reset to the new target base,
  an unrelated tip is refused with the exact rebase command, and the
  merge-request envelope reports how many commits are not on the target base.

### Fixed

- The release train's receipts stage commits the POSTED block and the report
  on the release branch and pushes it, instead of opening a docs-only pull
  request to main; the next prep carries it forward.
- The release train no longer needs the five hand steps recorded on the
  3.27.3 cut: prep refreshes Cargo.lock offline after the version bump,
  assemble rebases docs-only release commits onto the integration tip, the
  run date is pinned when the cut starts so a cut crossing midnight keeps its
  draft, preflight runs the announcement lint before the gate, and the
  receipts enqueue waits for required checks to be reported.
- The rolling union sweep skips epic branches whose tip is already contained
  in main and names main, not a prior epic, when the conflicting content came
  from main.
- The close gate derives its required scoped-proof targets from the same
  surface checker that validates them, cached once per close, so the command
  it suggests is the command that passes.
- The worker commit guard allows `git rebase --continue` and
  `git cherry-pick --continue` by resolving the branch from the rebase
  metadata; a detached HEAD outside a rebase is still refused.
- The checkout-dependency guard derives and scans every test-bearing Rust
  source, including library `#[cfg(test)]` modules, so a compile-time
  checkout read in a unit test cannot reach the merge-queue shard. Two such
  reads it found were fixed.

## [3.27.3] - 2026-09-21

### Added

- Supervisor replies on the hub render as formatted text on the phone. The hub
  now renders a small, safe Markdown subset in reply bubbles, replayed history
  and pinned questions: bold, italics, bullet and numbered lists, inline and
  fenced code, and `https` links. Everything is built as DOM nodes from the
  reply text, so tags, entities and non-`https` link schemes stay literal.
  Conversation-list previews strip the markers. Supervisors answering a
  Commander message now read a phone reply contract (answer first, at most
  five one-line bullets, bold the decision, keep it short) that the
  Commander-row inbox framing points to.
- The hub composer on a phone has a small round mic instead of the wide
  "Tap to talk" pill: it lights while listening, a second tap stops and inserts
  the transcript at the caret for review, and the text box takes the freed
  width. Unsupported or permission-denied voice input is shown on the mic
  itself; the standing "Voice ready" hint line is gone.
- The hub logs each forwarded conversation-history request and the size of
  the relayed response (request id and row counts only, never message text),
  so a thread that opens empty can be traced to the hop that lost it.

### Fixed

- The hub requests conversation history from any protocol-3 daemon even when
  the relayed welcome omits the capability list, and a pane with no recorded
  activity is labelled "No activity yet" instead of "waiting".
- The release train no longer needs hand interventions for the ten gaps
  recorded across the 3.27.0 to 3.27.2 cuts: the merge-queue query goes
  through one quoting path and its response shape is validated; the gate
  scratch base is read from the release env file and checked with the gate's
  own mount rule; Zig is linked as a toolchain directory; prep bumps the
  version before staging; publish fast-forwards the release worktree to the
  landed commit; post-publication waits for the Release workflow and writes
  the workflow, published and latency receipts; report reads the two Slack
  thread ids from the announce receipt; the receipts stage waits for
  mergeability, retries the enqueue and resumes idempotently. Each gap has a
  self-test row.
- The release report and receipts generators emit Markdown that passes the
  Docs Lint lane: Slack permalinks and the GitHub release URL are
  angle-bracket links and the duplicated "Only reply:" heading is
  disambiguated per audience. The checkout-dependency guard now derives its
  list of test sources from the test tree (plus a runtime scan of the
  checkout) instead of a hand-maintained list, so a new test file cannot slip
  a compile-time checkout read past it.
- The release report poster selects the MechaCassy token named by this
  machine's registered Claude profile instead of failing when several token
  variables are set; an explicit `MECHA_SLACK_TOKEN_ENV` still wins.

## [3.27.2] - 2026-09-21

### Added

- The hub replays a conversation's history when it opens. Every message the
  operator sends and every reply addressed to the operator is already durable
  in the project's queue; the hub now asks the daemon for a bounded,
  device-scoped page of those rows on open and on reconnect, hydrates the
  thread before live turns arrive without duplicating them, and offers a
  "load earlier" cursor. Pane text is never part of the page. Machines still
  running an older daemon ignore the request and the thread behaves as before.

## [3.27.1] - 2026-09-21

### Fixed

- A supervisor's answer to a Commander message now reaches the operator even
  when the supervisor addresses it to the message's "From" label
  (`commander:<label>`): that target is routed to the verified operator lane
  with the reply reference inferred from the newest matching Commander row.
  Message targets that can never be an agent (row-source labels such as
  `lifecycle:` or `verification-dispatch:`, or empty names) are refused at call
  time instead of being queued and silently abandoned, while a not-yet-registered
  worker name still queues as before. The inbox, hook and daemon framing of a
  verified Commander message now print the exact reply command.
- On a phone, opening the keyboard no longer pushes the conversation header and
  the latest messages out of view: the hub declares
  `interactive-widget=resizes-content`, sizes the shell from the visual viewport
  as a fallback, and follows the thread tail when the composer takes focus.
- `cas update` restarts the hub through its installed systemd user service
  instead of launching a detached hub beside it, records the old and new hub
  versions in its outcome, keeps the unit's publication flags, and `cas hub
  status` and `cas doctor` warn when the service is installed but inactive
  while a detached hub holds the lock.

## [3.27.0] - 2026-09-21

### Changed

- The hub's default conversation surface is now Pebble, the messaging design the
  operator picked on 2026-09-18. The conversation list shows every supervisor
  as a row with its machine's colour and monogram, the supervisor name, its
  project and machine, a preview line, and two distinct affordances: a waiting
  dot with a highlighted time when a supervisor needs an answer, and a filled
  count pill for unread turns. The thread renders the operator's messages as
  constant indigo pebbles on the right and the supervisor's in its machine
  colour on the left, tightens the corners of consecutive turns into one group
  with a single timestamp, folds runs of status updates into one quiet line,
  marks receipts with a tick, shows a working indicator while the supervisor is
  executing, and fits an evidence table inside a message at phone width without
  a horizontal scroller.
- A supervisor's ask and blocker are objects with their own silhouette: the body
  steps down into a deeper tray holding quick-reply chips, and a blocker swaps
  the tray for an inset evidence window. An unanswered ask is also pinned
  directly above the composer and unpins when answered. Tapping a chip or
  replying to the pinned ask sends the answer bound to that ask.
- Report attachments render as raised dog-eared sheets laid on the thread
  (type plate, name and size; the whole sheet opens the artifact) instead of a
  link row inside a bubble. The composer is a pill field with the send button
  in the machine colour naming the supervisor; a conversation with nothing
  waiting shows a quiet empty state.
- The live terminal pane no longer mirrors into the default conversation; the
  terminal remains one tap away as the explicit alternate view. The thread has a
  single header (monogram, supervisor with the project badge, machine and OS).

### Fixed

- The operator's reply reference now survives the hub. `SendMessage` frames
  carry an optional `in_reply_to`; the hub forwards it unchanged and the daemon
  binds the queued row to the supervisor's ask and confirms it, so a quick reply
  reaches the supervisor bound to its question instead of as an unrelated
  message. Older clients and frames without the field are unchanged.
- Conversation list rows no longer leave a dangling separator when a long
  project name wraps, and the rail footer counts the rows actually rendered.

## [3.26.0] - 2026-09-19

### Fixed

- Credential-bearing structs no longer derive `Debug`. Fourteen types across the
  cloud clients, hub pairing, the bridge server, factory daemon and the
  verification store printed a live bearer token, API key, pairing capability or
  pre-signed upload URL verbatim into any `{:?}`, tracing field, panic message or
  error chain that formatted them. Each now has a redacting `Debug` and a test,
  and a repo-wide guard test fails if a new one is added.

### Added

- `cas artifact publish|show|list` and the matching `artifact` MCP tool: hand the
  runtime a local path and it records a durable, citable artifact for a task.
  Cassy resolves the path against the task's artifacts directory and the
  checkout (refusing symlinks that escape either, and the `.cas`/`.git`
  directories outright), measures and hashes the bytes, enforces a 25 MiB
  ceiling before any network call, and uploads to Cloud storage when it is
  available. A publish still succeeds and returns an `artifact_id` when storage
  is unreachable, so the record can be cited either way.

## [3.25.8] - 2026-09-18

### Added

- `requires_start` dependency type: an explicit hard start gate. Ordinary
  `blocks` edges now gate close and merge, warn on start, and `ready` lists
  blocked dependents with an open-blocker flag.
- `recycle_worker` coordination action (and the `clear_context` fallback for
  Codex workers) restarts a near-limit idle worker in place with the same name,
  worktree and recipe; `worker_status` recommends it above a configurable
  `factory.context_recycle_threshold_percent`.
- MCP parameter aliases: `get` for `show`, `blocked_by` for `to_id`,
  `summary` for `notes` on close, `target` and `worker_names` interchangeably
  on worker actions, `reason` on `shutdown_workers`, `inbox` for `inbox_poll`.
- `cas knowledge build --verbose` reports per-source progress; a timeout names
  the in-flight source and elapsed time; `knowledge status --full` and
  `--json --full` list failed sources with their reasons.

### Fixed

- A parked delivery anchor survives the lane being fast-forwarded onto the
  supervisor's merge commit, and the original content commit is accepted as a
  close receipt.
- A rejected verification verdict is never re-bound; the retry mints a fresh
  dispatch for both code and no-code tasks.
- Epic close derives its anchor from the epic tip or the target merge, so
  `commit_receipt` is optional and target-reachable receipts are accepted when
  the epic ref lags.
- `local_merge` delivery resolves the local target: a worker re-close succeeds
  after a local merge, and merge requests for tips already declined via
  `request_changes` are suppressed.
- Lifecycle relays carry the parked branch tip; stale `awaiting_merge` and
  `close_rejected` redeliveries are suppressed once a later decision
  supersedes them.
- Supervisor actionable-idle resets on any supervisor tool activity.
- `sync_all_workers` and `cas factory sync` refuse to rebase or check out
  inside a live worker's worktree (root cause of detached-HEAD worktrees after
  push).
- `shutdown_workers` in no-remote or `local_merge` repositories checks tip
  reachability against the task target, pinned epic and default branch instead
  of counting unpushed commits.
- Worker target seeding records snapshot provenance, skips crates changed
  since the snapshot commit, refuses non-ancestor snapshots and warns when the
  snapshot is stale.
- Regression coverage for hook observations binding to the Stop harness
  session (fix shipped in 3.25.7).

## [3.25.7] - 2026-09-15

### Fixed

- Focused hosted terminal panes display the cursor at the harness-reported
  position, with cursor visibility and shape preserved through redraw and
  resize. Unfocused panes and modal behavior retain their intended state.
- Rolling integration follows session logs across UTC-day rollover, retains
  incomplete lines until complete, filters by session, and processes each
  matching merge event once.

## [3.25.6] - 2026-09-15

### Added

- `cas doctor` distinguishes intentional Codex skill twins from stale
  Cassy-managed skills and reports scratchpad policy with ownership-safe
  remedies.
- Worker status validates process identity and Git roots, keeps active Codex
  work distinct from dead sessions, and retains available exit status with a
  bounded terminal tail.
- Typed platform and loaded-work proof receipts are supported across
  repositories; bounded parsing rejects ambiguous counts and contradictory or
  failing results.

### Changed

- Worker launches no longer inherit protected operator credentials by default;
  documented task/project grants remain explicit and provider authentication is
  preserved.
- Factory target cleanup discovers Cassy-managed epic and release directories,
  revalidates ownership before reclamation, and leaves active, recent, or
  uncertain user-owned targets untouched.

### Fixed

- Task close verification preserves delivered-content identity across
  request-changes cycles and refuses to close while the delivery is absent from
  its target branch.
- Concurrent task-note appends no longer overwrite one another, and hook
  observations remain bound to their originating harness session with visible
  stop outcomes.
- Release-report receipts now require verified uploaded PDF bytes, matching
  size and SHA-256, and successful PDF decode/page-count checks; the hub-read
  fallback is bounded from the User thread root.

## [3.25.5] - 2026-09-11

### Fixed

- Commander pairing on macOS no longer fails after `cas update` when the Tailscale app is installed without a PATH CLI. The hub resolves the app-bundle CLI at `/Applications/Tailscale.app/Contents/MacOS/Tailscale` (or `~/Applications`), invokes it by its real path, and records the resolved path.
- `cas update` keeps a previously published hub public URL: a failed Tailscale Serve republication is reported as a hub transport error in the update banner and JSON receipt while the project refresh still completes, instead of a silent loopback fallback or an aborted update.
- `cas hub authorize` names the actual transport cause (which Tailscale CLI locations were checked) and prescribes `cas hub restart --tailscale-serve`; the previous `service uninstall` advice is gone from authorize, readiness and status remedies.
- `cas doctor` and `cas hub status` warn when the hub is loopback-only while Tailscale is signed in on the host.

## [3.25.4] - 2026-09-11

### Added

- Cassy Cloud shows paired machines with connection, runtime and last-seen details, plus confirmed removal from the current browser. Cloud and local Commander builds have distinct branding.

### Changed

- The default Hub conversation list requires fresh, reachable sessions with active membership. Heartbeats refresh visibility, expired sessions leave the default list, and explicit dormant/recovery views and pending work remain available.
- Worker status identifies its factory session and separates registered workers inside and outside that session before deduplicating names.
- Role instructions align task-start acceptance, initial close ownership, receipt handling and authenticated wake behavior across Claude, Codex, Grok and OpenCode. Codex uses its own startup checklist; compact prompts retain recall coverage within existing budgets.
- Slack publication guidance uses only the authenticated MechaCassy hub, including direct access to that same hub when the live proxy is unavailable. Personal Slack connectors are no longer a fallback; partial receipts and uncertain-write safeguards remain required.

### Fixed

- Workspace and formatter guards parse executable shell structure rather than treating heredoc bodies, comments or quoted template text as write targets. Actual out-of-worktree writes and formatter invocations remain guarded.
- Bug reporting falls back to the configured Cassy component repository when the project issue repository is absent. Session hygiene scans the resolved artifact root instead of an unrelated home directory.
- Close verification preserves content proof across merge commits, rejects merges that drop delivered edits, and binds post-merge repository proof to the declared target branch rather than the current checkout.
- Scoped proof derives its comparison and receipt base from the declared WorkTarget, resolves nested integration targets without phantom binaries, emits valid multi-module arguments and honors the latest scoped receipt.
- Rolling integration includes live open branches regardless of ownership, excludes closed/cancelled or missing branches, and distinguishes coordination branches from delivery targets. Repeated failures already present on main share stable evidence; unknown failures remain explicitly unclassified.
- MCP recovery commands use caller or registered recipient tool aliases for Claude, Codex, Grok and OpenCode. Role-specific CLI evidence wins over the daemon environment, and unknown recipients receive neutral guidance.
- Lifecycle and preassignment relays render recovery commands for their recipient, retain neutral persisted facts and resume partial outbox delivery without duplicating completed work. Quoted or multiline rejection context no longer breaks the suggested audit argument.
- Authenticated daemon `merged_close_blocked` and `pr_lane_failed` notifications can pass the wake gate. Matching text from an untrusted sender or without a valid envelope remains rejected.

## [3.25.3] - 2026-09-11

### Fixed

- The `Scoped Validation (fast)` CI tier no longer requires ripgrep on the
  runner: the proof-surface script falls back to `git grep` (#836).
- The director's stalled-supervisor relay distinguishes a merged delivery whose
  close is blocked from an unmerged one, posts the close rejection once, and
  stops repeating the merge demand (#835).

## [3.25.2] - 2026-09-11

### Fixed

- `cas codex` / `cas claude` launch the supervisor on a model and effort that
  belong to the chosen harness: an inherited incompatible model is repaired to
  the harness recipe and an explicit incompatible model is refused at
  preflight instead of opening a broken pane (#820).
- Commander lists only sessions with an active supervisor; sessions with no
  live supervisor and no workers are hidden by default behind a dormant toggle.
- `hub_clean_home_test` proves the old hub process is gone instead of
  asserting the next PID differs, removing a PID-reuse flake (#826).

## [3.25.1] - 2026-09-10

### Added

- Rolling integration branch: every epic merge re-merges main plus all open
  epics into `integration/<project>`, sweeps it, and reports conflicts or
  failures to the owning supervisors at merge time.
- Scoped proof maps changed modules to the integration test targets that
  exercise them; a delivery cannot close without running them.
- Fast merge admission: a supervisor scoped proof on the exact lane tip admits
  small deltas without waiting for the full lane CI run, plus a fast CI tier.
- Sweep failures are grouped into ready-to-file tasks per failure class and
  can be accepted and spawned in one call.
- Release gate `--reuse` accepts row receipts written by the integration
  sweep, so a clean assembly needs one short gate.

### Fixed

- `version-literals` gate row scans tracked files only, so gitignored build
  caches cannot fail a release.

## [3.25.0] - 2026-09-10

### Added

- Commander conversations: the Hub opens on a Cassy Cloud-branded list of
  supervisor conversations with a prominent project badge, shows each
  supervisor's real pane text, and sends addressed messages with distinct
  sending, delivered, replied, and rejected states; the terminal is an explicit
  alternate view.
- Verified operator channel: Commander messages are stamped with the paired
  device session, carry `operator` provenance with user authority, and get a
  reply path back to the originating device; sends are acknowledged with a
  durable `MessageQueued` receipt and refusals keep the `client_ref`.
- Worker liveness: `worker_status` reports `executing`, `waiting_for_input`,
  `stalled`, or `dead` per worker from turn and process evidence, in one fast
  call, and the director relays use the same source.
- Task risk declarations (`none`, `blast-radius`, `platform`, `concurrency`)
  are required at create and enforced at close; a per-merge workspace sweep and
  gate/CI row parity catch release blockers at merge time.
- `cas hub` pairing on a supervised (launchd) host: bounded GUI-free Tailscale
  probe, supervision-aware `restart`, exact Serve refusal and recovery guidance,
  consumed-code pairing messages, and a doctor row (#817).

### Changed

- Supervisor skill guidance moved detail behind references to stay within the
  SessionStart budget; `max` is a valid effort level for Fable, Opus, Astra,
  and Sol.
- `cas codex` and `cas claude` profile pickers hand the selected account
  directory to the supervisor launch exactly like an explicit profile (#820).

### Fixed

- Isolated workers can no longer edit, commit, reset, or push inside the
  primary checkout or any path outside their registered worktree (#816).
- Task close verifies delivery content from the task's content commits when
  the branch tip is a merge of the target, and pre-close reachability errors
  name the commit, the resolved ref, and local-vs-origin (#818, #819).
- Skill lookups suggest the nearest name, spawned agents get `~/.local/bin` on
  `PATH`, and doctor flags divergent duplicate skill copies (#810).

## [3.24.0] - 2026-09-10

### Added

- MCP proxy configuration now expands `${VAR}` and `${VAR:-default}` values,
  reporting required unset values as `missing_credential_env` (#803).
- Hub startup is a lock, bind, and process-record transaction; `--force`
  recovers wedged holders during stop/restart, and doctor reports the remedy
  (#804).
- Servers run in dedicated process groups so stopping a server terminates and
  verifies its nested process tree (#796).

### Changed

- Task start, claim, and close accept assignees by UUID or name (#799).
- Factory supervisors route coordination through the CAS message path instead
  of the host `SendMessage` transport (#793).
- Supervisors can close worker-assigned gates, while gate-to-worker assignment
  now warns about the ownership boundary (#800).
- Epic close uses a bounded, batched merge-gate evaluation and fails closed
  with a partial child listing when its budget expires (#802).
- Close retries preserve approved verification provenance, alias superseded
  dispatches, and bind post-merge proof to the published target (#755, #753).
- Spawn-time assignment boilerplate is suppressed after a task becomes
  started, blocked, awaiting merge, or terminal (#756).
- Worker proxies inherit credentials from the configured credentials file and
  preserve symlinked profile write-through (#758).
- Scoped proof ownership resolves module declarations instead of matching
  arbitrary source-path test data (#778).

### Fixed

- Oversized embedding units are truncated with an explicit receipt and retried
  instead of being quarantined permanently (#805).
- Image generation sends the API key through a mode-600 header file rather
  than exposing it in the curl command line (#757).

## [3.23.0] - 2026-09-10

### Added

- Factory coordination traffic is now configurable and bounded: ordinary
  messages allow 1,200 characters, escalations allow 2,500, and task notes
  allow 1,500; over-cap messages are rejected before queueing with an artifact
  remedy, and every message requires a summary.
- Worker launch prompts now use a fixed six-field return contract and silent
  execution, with context headroom reported only below 20 percent.
- Supervisor panes now have a concise output budget, with evidence kept in
  task notes and artifacts and one decision per worker message.

### Changed

- Startup context is delivered once per session; tool-result fallback no
  longer replays transport-delivered mail, task details show the newest five
  notes, claim files are pruned, and queue provenance is a one-line
  `[cas #id origin <age>s <first|replay>]` envelope.
- Close-gate recovery now requires origin reachability for trunk targets,
  rebases merge recovery inside the worker worktree, blocks history-changing
  Git commands outside that worktree, and refuses an unpublished local epic
  parent during base refresh.

### Fixed

- Release reports no longer mistake GitHub pull-request references for issue
  references.

## [3.22.1] - 2026-09-09

### Fixed

- `cas release report` assembles a usable report on its first run from any
  checkout: project settings resolve through the shared store detection (a git
  worktree finds `issues.repo` and the project is named from its config),
  wrapped changelog entries are joined, the release-notes draft's Was → Now
  bullets fill the user and developer sections separately, inline-code headings
  keep their backticks, issues named in the changelog or the release pull
  request are counted and verified, the header shows the publication time from
  the release or its receipt, the draft's opening line becomes the verdict, the
  change map takes its themes from the draft's section labels and records them
  in the front matter, and `--pdf` fetches a disposable Playwright when the
  module is absent.
- `cas update` keeps the post-swap refresh receipt: the refresh child writes it
  to a file the updater reads back, so a partial refresh names the projects that
  did not refresh and the command to rerun instead of reporting an unknown
  outcome; the JSON path falls back to the same file.
- The scoped proof-surface guard requires the cross-flavor, agent-contract, and
  skill-guardrail tests for any built-in skill, reference, or agent edit and
  discovers further guardrail tests by the file's source and installed paths;
  the release failure log records the lesson with a self-test.
- The invalid-invitation hub pairing test no longer races its own listener on a
  loaded CI shard; it waits for readiness, bounds accept and read, and names the
  refused address.

### Changed

- The supervisor skill's reporting-style, release-train ownership, and
  cross-team routing guidance is parked verbatim in
  `cas-supervisor/references/reporting-and-routing.md`, linked from the skill
  body in every harness flavor, keeping the body under its protected budget.

## [3.22.0] - 2026-09-09

### Added

- `cas release report <version> [--out <dir>] [--pdf] [--json]` assembles the
  release-report Markdown source for any project from its Keep-a-Changelog
  section, release-notes draft, GitHub release and closed issues (via `gh` and
  `issues.repo`), and release receipts, then renders the standalone HTML with
  the built-in `cas-release-report` renderer and, with `--pdf`, the PDF through
  Playwright. Issues are themed by labels first, then a keyword table, with an
  explicit Unclassified row; an existing Markdown source is preserved unless
  `--refresh-sources` is given. Human output fits 80 columns with ASCII
  fallbacks; `--json` emits one result object.
- The release train gains a `--report` step: after verified publication it
  produces the report files, hands the PDF and HTML to a posting adapter, and
  validates `release-report.receipt` (worktree-confined paths, both SHA-256
  values, PDF page count, Slack file ids, user and dev thread timestamps).
  `--status` revalidates the receipt against the bytes and reports the release
  report as pending, unavailable, or verified; the cas-cut-release, mecha-cassy,
  and release-notes skills describe the file-post step for every harness.
- Release reports for v3.20.0 and v3.21.0 ship under `docs/release-reports/`
  (Markdown, brief, standalone HTML, A4 PDF, visual-QA receipt); the report
  template keeps print pagination continuous.
- `cas doctor` gains a `SessionStart budget` row that reports supervisor
  guidance size against the protected ceiling and fails under 512 B of headroom.

### Changed

- SessionStart payload compaction orders sections by value: static listings
  compact first, ambient recall and the factory inbox last, and a compacted
  section leaves a one-line marker in the payload. Built-in supervisor guidance
  is over 1 KB shorter in every harness flavor.
- Task close gates judge only the task's own delivery: additive-only and
  value-only posture gates, the no-code intent gate, the receipt diff stat, and
  the receipt epoch check share one first-parent delivery range bounded by the
  target merge-base and the task's work window; a registered supervisor override
  with a reason passes posture and epoch gates with a logged decision; clearing
  `execution_note` no longer invalidates an approved verification when the
  exact repository proof is unchanged (#767).
- The factory worker spawn audit names the launched CLI, model, effort, and the
  provider's account directory instead of always printing a Claude account
  path, with a regression pinning Codex routing to the Codex launcher.

## [3.21.0] - 2026-09-09

### Added

- Epic-level flow walk: when any child of an epic carries a demo statement, the
  supervisor runs one combined user-flow evidence pass on the assembled epic
  tip, concurrently with the release gate, with a 60-minute box and the
  cas-qa-craft quotas. A single `LEDGER.md` and one `Epic flow walk` note record
  child coverage, counts, and cross-child contradictions; epic close
  verification requires that note and applies the same REJECT table before it
  reads the close reason.
- QA telemetry sweep contract: the optional `qa.telemetry_sweep` setting names a
  project-relative read-only command that `cas-qa-craft` runs as its first
  step. Each `NEW`, `RISING`, `HIGH_RATE`, or `BLACKOUT` finding becomes an
  `eyewitness/telemetry` ledger row; without a command the ledger header says
  `sweep: not configured`. A documentation-only PostHog HogQL example and a
  known-noise citation table ship with the skill.
- `cas doctor` reports when the Claude Code prompt hook has gone silent and
  counts observed misses.

### Changed

- Turn context (ambient recall, supervisor reminders, factory inbox) is
  delivered once per prompt through PostToolUse or any Cassy tool response when
  the UserPromptSubmit hook is not invoked, which Claude Code 2.1.263 and later
  omit in long-lived team sessions (#763).
- Ambient discovery ranks candidate memories by term overlap before recency,
  caps prompt terms at 16, gives the semantic step a 1500 ms deadline, and
  sorts unbound commit history behind guidance (#764).
- The release gate runs the workspace suite once across the nextest and archive
  rows, records per-row timings, runs doctests through the CI wrapper, and can
  re-gate an unchanged tree from content-keyed receipts with `--reuse` without
  ever authorizing the pipeline.

## [3.20.0] - 2026-09-08

### Added

- User-facing task labels now require a non-empty demo statement at creation
  time. The configurable `qa.user_facing_labels` setting defaults to `ui`,
  `hub`, `cli-ux`, `commander`, and `frontend`; epics, unlabeled tasks, and
  deliberate supervisor overrides remain exempt, and worker briefs show the
  statement that describes the observable outcome.
- The built-in `cas-qa-craft` skill turns a demo statement into an exploration
  matrix with evidence labels, a 30-minute time box, and a durable `LEDGER.md`
  for honest results.
- Task verification now applies the evidence-mode REJECT table before opening
  captures, then records the capture judgment alongside each row and the close
  reason.
- The built-in `cas-release-report` skill now supplies a reusable renderer,
  template, and PDF recipe, with the release workflow's step 13 requiring the
  report artifacts before the release-notes rubric's authorized Slack post.
- The v3.19.0 release report exemplar now demonstrates the Markdown, brief,
  standalone HTML, PDF, and visual-QA evidence that release preparation carries
  forward.

### Changed

- Release preparation now links the report source and both rendered report
  formats from the announcement draft, preserving source fidelity and
  continuous A4/Letter pagination evidence for review.

### Fixed

- Ambient recall now runs its semantic channel within budget (connect and request
  deadlines are both bounded), ranks recent memories with deep term overlap ahead
  of old rules that match one common word, stops padding prompts with unrelated
  task titles, and filters hex ids and durations out of tool-traffic trigger terms.
- An idle `cas serve` code watcher no longer spins a CPU core while it waits
  for file changes.

## [3.19.0] - 2026-09-08

### Added

- `cas doctor` now includes separate `known repos` and `MCP upstream
  reachability` checks. Missing roots are distinguished from live roots that
  lack a Cassy store, and each result names the appropriate `cas doctor --fix`,
  `cas init`, or `cas known-repos forget` remedy.
- `cas config set/get/list history.github_repo` gives code-history indexing its
  own GitHub repository instead of reusing the bug-intake `issues.repo` key;
  `cas history` and its daemon path use the explicit setting or a GitHub origin
  fallback and report the new key when neither is available.
- `cas config set factory.max_concurrent_builders` and
  `cas config set factory.worker_build_jobs` configure process-level and
  Cargo-level factory build limits, and `mcp__cas__coordination` spawn output
  reports the active throttle.
- `mcp__cas__coordination action=epic_status` now accepts `offset`, `limit`,
  and `summary` for bounded paging and fast ancestry-only reports.

### Changed

- `cas hub status` now reports a healthy CAS-created Tailscale Serve route
  with its live hub target, while retaining distinct loopback-only and
  unavailable states.
- `cas cloud sync` now explains HTTP 409 project-registration conflicts,
  parks affected rows without pretending they are transport retries, and
  gives the exact `cas cloud project set` or alias remedy. `cas doctor` shows
  the same queue diagnosis.
- `cas cloud purge-foreign` now applies only the verified dry-run delete
  set, checks the deleted count, quarantines the exact rows, and aborts if
  rows reappear or the store changes during the operation.
- `cas cloud pull` now materializes accepted cross-project proposals as open
  local tasks, admits them only by the server-attested target, and prints
  concrete pull errors in non-verbose output.
- Factory lifecycle output now bounds `epic_status` work, preserves the
  declared close target through close and commit guards, keeps a focused epic
  as the spawn base, and records rejected non-factory pushes in worker
  activity instead of leaving them only in hook stderr.
- Delivery and messaging now refresh parked anchors, preserve upstream Slack
  errors, validate urgent assignment ownership, keep context-reset identity
  tied to the recipient harness, route relays to the owning supervisor
  session, cap Claude redelivery, and keep transport-delivered inbox rows
  pollable until an observed wake consumes them.
- The `mecha-cassy` skill mirrors now document the read preflight fallback and
  channel/edit/delete contract, while file uploads require downloaded SHA-256
  equality, image decoding, and visible preview checks rather than byte counts.
- Release-receipt fetches now send a GitHub token when present, retry three
  times with backoff, and print the HTTP status and response body on failure;
  install-path proof passes the token on both validation lanes.

### Fixed

- Codex workers now derive `HOME` and the Playwright MCP profile from the
  host home instead of using a fixed path, so MCP startup works on macOS and
  other hosts with different home directories.
- The spawn build guard refuses new workers when the one-minute load exceeds
  the CPU count or live Cargo builders exceed `factory.max_concurrent_builders`,
  unless `force=true`; the release gate now rejects committed hub-web `dist`
  drift.

## [3.18.1] - 2026-09-08

### Added

- `cas doctor --host` now includes a `hub transport` row so the health report
  shows whether the hub's public route is available and points to the live hub.
- Commander connection and pairing fixtures now use the same connection
  surfaces as the product, keeping their failure and recovery states honest.

### Changed

- `cas hub status` now exits non-zero when the CAS-created Tailscale Serve
  route no longer targets the live hub shim.
- CAS-created Tailscale Serve routes now remain part of the hub's durable
  lifecycle across relaunches.

### Fixed

- Pairing diagnostics now distinguish an HTTP 502 route with a missing hub
  backend and explain how to restore the route or use a reachable hub URL.

## [3.18.0] - 2026-09-07

### Added

- A light and dark Commander that follows the Petrastella design language,
  with an Appearance choice in the command palette.
- A fleet view drawn as a verdict and dot plot, a readable attention timeline,
  a ledger-style transcript, and clearer pairing and connection surfaces.

### Changed

- Generated design tokens now have a drift test, the hub-web visual-QA gate
  runs in CI and the release gate, and shared CSS allowlist semantics keep
  intentional visual exceptions explicit.

### Fixed

- A flaky `hub::attention` test, workspace-guard handling of heredoc bodies,
  fail-closed `report_cas_bug` staging for unfiled reports, and pre-close
  recovery of rebased anchors with a named commit receipt.

## [3.17.3] - 2026-09-06

### Added

- A vendored design system for every project: the `cas-ui-craft` skill with a
  required concept brief, a form vocabulary beyond tables and cards, a critique
  rubric with a floor score, and four annotated exemplars; the Petrastella
  design language and design tokens (colour pairs computed per scheme, series
  palette validated by `cas-dataviz`); the `cas-frontend-engineering` skill;
  and an API/DX taste section in `cas-codebase-design` with a public-surface
  review gate for workers and supervisors.
- `scripts/visual-qa.mjs`: a strict headless visual-QA gate that renders in
  light and dark at desktop and phone widths and fails on contrast, clipped,
  overlapping, overflowing, or invisible text; wired into report review, the
  UI critique rubric, and the worker close checklist.
- `cas-cli-craft` skill and `scripts/terminal-qa.mjs`: terminal output craft
  with a gate for contrast, overflow, word splits, truncation, numeric
  alignment, Unicode fallbacks, colour under NO_COLOR, and JSON contracts.
- `cas-technical-drawing` skill with a parametric SVG drafting renderer
  (orthographic, isometric, section, exploded, joint and part sheets) and
  mechanical checks, demonstrated on a woodworking cabinet.
- `cas doctor --host` and `--full`: host-level findings are reported once per
  run instead of per project, with a published check matrix classifying every
  check as auto-fix, consent-fix, human, or info.
- A cross-project factory model history extractor and scorecard
  (`scripts/factory-model-history.py`) covering every harness home, with the
  model lane rubric review rewritten under the new design system.

### Changed

- `cas-html-reports` and `cas-dataviz` now require a concept brief before any
  render, default to the Petrastella design language with a `DESIGN.md`
  override, and ship ambitious annotated examples with before/after pairs.
- `cas doctor`, `cas update --check`, and `cas factory status` render
  verdict-first with marks-only colour, folded repeats, wrapped remedies, a
  light-safe palette, and a corrected `COLORFGBG` light-terminal detection.
- `cas doctor --fix` applies safe repairs (known-repo pruning, code and search
  index rebuilds, stale projections) and previews consent repairs, applying
  them only with `--yes` or one terminal confirmation; a symbol index with no
  eligible files is now informational.
- The factory `heavy` lane routes to Codex GPT-6 Astra at high with Sol as the
  fallback; `taste` and `supervisor` fall back to Claude Opus 5 at high.

### Fixed

- Claude factory workers no longer hang on “Waiting for team lead approval”
  for shell commands that remove or rewrite files inside their worktree; the
  launcher sets bypass permission mode, the workspace guard understands
  variable `rm` targets and heredoc writes, and worker status reports a
  pending leader approval as its own state.
- Epic creation records the canonical `epic/<slug>-<id>` branch as the
  WorkTarget, spawning refuses to recompute a title-derived base, and a spawn
  fast-forward refreshes any checkout that has the epic branch out.
- Codex cached input tokens are billed once in the model scorecard.

## [3.17.2] - 2026-09-05

### Added

- The director now wakes a silent supervisor when the fleet is idle, tasks are
  awaiting merge, or all children are terminal, subject to configurable
  `[factory] stall_after_secs`. The supervisor skill now requires a “Drive to
  the exit” rule and six-rung exit ladder, while `worker_status` and session
  summaries expose actionable-idle minutes.

### Changed

- Factory supervisors now launch through an explicit `supervisor` registry lane
  routed to Claude Fable 5.1 at medium effort (previously the built-in Claude
  Opus/high default); the lane fails closed when the Claude account is
  unavailable, and the Codex Astra recipe records why it is explicit-request
  only.
- Release publication now explicitly dispatches the install-path proof after
  publication, including token-created releases that never emit
  `release.published`.

### Fixed

- Post-merge task re-close now names a stale parked delivery anchor and its
  remediation, and accepts a receipt matching the current assignee's branch tip
  as task identity.
- Epic status now recognizes same-lineage evolution as retained delivery
  content and ancestor branches as merged, avoiding false “content dropped” and
  “1 unmerged” reports.
- `cas update --json` never prompts and keeps its output machine-clean, while a
  plain non-TTY update without explicit consent fails closed with actionable
  guidance.

### Tests

- Parallel test fixtures now use isolated paths and disk-index writers, removing
  cross-test collisions without changing production behavior.

## [3.17.1] - 2026-09-05

### Changed

- The factory `taste` lane now routes to Claude Fable 5.1 at medium effort
  instead of Codex GPT-6 Astra. Explicit Astra and Opus requests still work,
  and `taste` fails closed when the Claude account is unavailable. The registry
  recipe is `claude_fable` with model `claude-fable-5-1` and medium effort, and
  the disabled Opus fallback edge remains fail-closed.
- Bug filing now uses one component registry with four destinations: the
  project repository from `issues.repo`, Cassy, MechaCassy, and Cassy Cloud.
  Compiled defaults can be overridden under `[issues.components]`, and the
  same routing appears in session-start guidance, the `cas doctor` issue
  repositories row, `cas config get/set issues.components.*`, managed
  CLAUDE.md/AGENTS.md guidance, and worker, supervisor, and GitHub-issues skill
  mirrors. Operational bugs are directed to their matching repository first.

### Fixed

- The installer now requires the selected GitHub release asset's published
  SHA-256 before extraction or executable replacement. A missing, malformed,
  or mismatched receipt leaves the existing installation untouched. This is
  corruption detection against the same publishing authority, not independent
  signature or attestation authentication.
- Concurrent project-config updates are serialized across processes and
  committed through a private same-directory temporary file, sync, and atomic
  rename. Existing permissions and unrelated TOML sections are preserved, and
  a failed commit leaves the prior valid config in place.
- Slack bridge sessions are keyed by the full channel, thread, and project
  tuple instead of a truncated timestamp prefix. Successful establishment is
  retained across restart, same-thread requests are serialized, corrupt saved
  state fails closed, and an in-process session remains known if persistence
  fails after the child succeeds. The identity migration starts a fresh child
  conversation once for threads already live during the upgrade; later
  requests resume the new conversation. Its dedicated tests and build now run
  in the CI lanes whenever the bridge changes, including mixed Rust/bridge
  changes, so session-recovery fixes cannot merge without bridge coverage.
- `cas hub authorize` now checks that the advertised public Hub URL answers
  `/v1/health` with `ready=true` before claiming a pairing code or minting a
  one-time invitation. Dead Tailscale Serve routes fail fast with the recovery
  hint `cas hub restart --tailscale-serve`, and redirects to another origin are
  rejected. `--skip-hub-readiness` bypasses only this probe; URL validation,
  consent, authentication, origin checks, and one-time token protections remain
  enforced.
- `cas update --json` now distinguishes a post-swap refresh that ran and
  failed from a skipped refresh. The receipt keeps `refresh_binary_version`
  and per-project `refresh_status=refresh_failed` results, while
  `spawn_failed` and `refresh_failed_no_receipt` remain distinct outcomes with
  truthful recovery guidance.
- The release gate's doctor snapshot now normalizes bare `fatal: not a git
  repository` wording alongside its parent-directory and mount-boundary forms,
  and records the normalized failure in the release ledger.
- Worker pull-request status distinguishes a failed or unavailable lookup from
  a definite absence, reporting a redacted `unknown` reason instead of the
  false `none` result.
- Linked-worktree builds watch the real per-worktree and shared Git metadata
  instead of nonexistent `.git` descendants or broad checkout directories.
  Unchanged builds remain fresh while ref, detached-HEAD, and existing optional
  environment-file transitions still invalidate precisely; creating a
  previously absent optional environment file requires another ordinary build
  trigger or clean rebuild.
- A separately supplied external-production verification receipt can satisfy
  only the zero-commit delivery-evidence check after exact authority, task,
  parent, active-session, gate, completed/pass verdict, and portable-evidence
  validation. All other close gates and the worker completion receipt remain
  unchanged, and accepted evidence is audited.
- The build-script worktree test now resolves `build.rs` from runtime paths, and
  archive portability checks flag tests that use compile-time
  `CARGO_MANIFEST_DIR`.
- The nested-Cargo fixture now scrubs wrapper environment and skips when nested
  Cargo is unavailable, while the release-gate archive row reproduces the
  merge-queue shard environment.

## [3.17.0] - 2026-09-05

### Added

- Commander pairing tells you where you stand at each step: one primary action
  per step, machine-address guidance, a capability summary naming exactly the
  scopes granted, and a saved-versus-connected distinction so a saved machine
  is no longer mistaken for a live one. The saved-access announcement is raised
  at the installation seam, before the connection starts, and is named after
  the installed machine rather than the selection.
- Pairing recovery is visible and owned: a live dialog status, a cancellation
  step that owns its own cleanup and surfaces a late rollback failure, a retry
  owner with outcome-specific copy, a storage-failure outcome, and explicit
  feedback for an invalid or expired link. Retry is serialized and
  generation-guarded, so a stale attempt cannot report over a newer one.
- The Commander fleet board renders workers and tasks as structured rows on its
  own canvas, keyed on its container so a shell rebuild refills it, with phone
  touch targets, header chips only when a session exists, and readable contrast
  on the new surfaces.
- `cas cloud purge-foreign` and the release train carry fuller receipts:
  tag creation and publication are recorded as separate events, so a tag push
  is no longer reported as a published release.

### Fixed

- Generated skill and spec frontmatter is emitted by one proven YAML
  serializer. Three hand-rolled escapers had drifted apart, so a description
  containing a Windows path and a colon — `Use C:\project: inspect` — produced
  frontmatter a YAML reader rejects. `argument-hint: [title]` was also written
  unquoted, which YAML read as a one-element list rather than the string every
  consumer expects. Frontmatter is now read back through a parser too, with the
  previous line scan retained only for files a parser cannot accept.
- A canonical worker merge no longer strands another checkout of the target
  branch. Advancing the branch ref is invisible to a second worktree holding
  it, which was left describing an older commit and reporting the merged
  content as staged deletions. A checkout holding uncommitted work now refuses
  the merge before any ref moves, a clean one is advanced under git's own
  refusal-on-conflict guard, and any checkout left untouched is named in the
  merge receipt. This prevents new occurrences; it does not repair a checkout
  already stranded.
- A merge target that is merely unpublished is no longer reported as diverged.
  A target ahead of origin with nothing behind it received a divergence refusal
  whose recovery — merging origin into it — was a no-op, so following the
  instruction produced the identical refusal.
- Task writes no longer lose their cloud sync intent when the outbox enqueue
  fails: the mutation and its sync intent are bound together, recovery is
  serialized, and a failed enqueue is retained rather than discarded.
- Closing a task with `execution_note` supplied inline is honored, so a no-code
  task no longer has to be updated separately before it can close.
- The published install-path proof asserts the greeting the shipped binary
  actually prints, and a library test now fails if that workflow and the
  front-door command drift apart again.
- `classify-ci-diff.sh` fails closed when `git diff` fails, instead of
  reporting an empty diff and a successful exit.
- Self-hosted runner cache growth gains a guarded periodic budget: a prune
  service and daily timer unit, an installer that registers them, pruning
  serialized against running jobs, descendant mounts rejected, duplicate
  systemd mount rows tolerated, and raw mount records read inside the runner
  sandbox. This is the mechanism, not a hard ceiling enforced at write time,
  and it does nothing on a host where the timer is not enabled.
- Ambient recall rejects identity noise and stale procedural entries, expiry
  instants are parsed rather than guessed, task boosting is narrowed, and
  retrieval measurement explains itself and requires explicit use attribution.

### Removed

- The dormant Slack upload staging helper and its tests. The helper was
  referenced only by its own tests and overwrote a symlink target before
  rejecting the write; deleting it removes the defect rather than shipping a
  repaired feature nothing calls.

### Documentation

- A correctness and architecture audit of this repository, and a review of the
  Commander pairing journeys and their recovery paths.

## [3.16.0] - 2026-09-04

### Added

- `cas integrate mecha-cassy` onboards a machine in one command. The label is
  derived from the hostname (`MECHA_SLACK_TOKEN_<HOSTNAME>`), the bearer is
  minted through the hub with your existing Cassy Cloud login, the bypass is
  read from the hub or Vercel or asked for once without echo, the credentials
  file is written with `0600` permissions and sourced from the login-shell
  profile so every process inherits it, and the registration is verified in
  the same run. It fails closed, naming the missing hub route, until the hub
  ships its half; no value is ever printed or written outside the credentials
  file. `--label` remains only an override.
- `cas cloud purge-foreign --allow-majority-foreign --yes`: an explicit
  operator override for stores where more than half of the local tasks are
  foreign replicas. It lifts only the ratio guard, re-verifies a fresh dry-run
  delete set before deleting anything, records the ratio, backup path, and the
  operator decision in the receipt, and leaves every other guard in place.
- `cas doctor` names registered project roots that must never sync (artifact
  fixtures, probes, temp directories, folder containers) and the command that
  removes the stale registration; `cas known-repos forget <path>` does it.

### Fixed

- `cas update` no longer refreshes test-fixture database copies, probe stores,
  temp directories, or unpinned folder containers as if they were projects, and
  cloud push refuses any store whose identity would be a bare folder name. The
  `control`, `project`, and `apps` cloud scopes were minted that way; skipped
  roots are now listed with their reason.
- Purge-foreign also clears foreign team registrations and a foreign
  knowledge-push identity, and the doctor's cloud-identity advice now points at
  the purge instead of a command that cleared nothing.
- The per-project pull no longer re-admits another project's rows that sit in
  this project's own cloud scope; a malformed cloud row (for example a task with
  a null `metadata`) is parked with its reason instead of aborting the whole
  pull; push failures now log the status, body, and request ids so a bare
  `500` can be traced.
- Hosted Commander pairing works again. Since v3.14.0 the invitation carried a
  third fragment key (`scopes`) that the cloud relay's parser rejected as
  "malformed or has an invalid expiry"; the relay now receives the two-key
  invitation it expects, and a relay rejection names the actual field.
- Factory workers inherit the MechaCassy credential names from the machine
  registration, and a missing variable is reported by name in proxy health and
  in the doctor instead of as a bare connection failure.
- Release gate: the epic worktree must be clean and at its branch tip, and a
  Zig toolchain must resolve, before the gate runs; the pipeline's `pr-body.md`
  input is documented.

## [3.15.8] - 2026-09-04

### Fixed

- `cas update` refreshed every local project in one process but resolved the
  cloud identity once, from the directory the command was launched in, so all
  projects pushed to and pulled from that one project's cloud scope. Each
  project now syncs strictly as itself: the identity is resolved from the
  project root being synced, and a project whose pinned identity names a
  different repository than its git remote is refused instead of silently
  sent to the wrong scope. Bare slug pins (a project pinned as `cas-src` in a
  repository named `cassy`) remain authoritative.
- A team pull no longer writes another project's task rows into the local
  database. Rows whose origin names a foreign project, and rows with no
  ownership evidence at all, are parked; a row with no recorded origin takes
  only the server-attested owner, never the requesting scope.
- `cas cloud purge-foreign` runs from a fresh session again: session-start
  memory refreshes no longer count as unpushed content, and access-only memory
  updates no longer queue a cloud write at all. The purge also clears every
  team-pull watermark so the next pull re-evaluates from a full snapshot.

### Added

- `cas doctor` reports a new `cloud identity metadata` check that names any
  team-pull watermark, team registration, or knowledge-push identity that
  belongs to another project, with the exact command to repair it. Retained
  id-collision rows now print a title-confirmed operator path instead of a
  bare warning.
- Slack release notes follow a readable-at-a-glance format: plain language on
  the user side, one bold-labelled bullet per shipped change, Slack mrkdwn
  only, and a preflight lint in the `mecha-cassy` skill that refuses to post
  markdown-shaped text.

## [3.15.7] - 2026-09-04

### Added

- A worker's blocker now reaches an idle supervisor's screen. Sending a message
  with `blocker=true` wraps it in a marker that Cassy itself attaches, and the
  supervisor's screen wakes for it the same way it does for a merge request.
  Typing the word "blocker" into an ordinary message changes nothing: without
  the flag it waits in the inbox as before.
- When a worker's close is parked for verification, Cassy now delivers the
  verification handoff to the supervisor itself, at the moment it creates the
  dispatch, and wakes the supervisor with it. The worker no longer has to
  forward the dispatch id by hand, and the handoff can no longer sit unread
  while it blocks the close. If that delivery fails, the worker still sees the
  forwarding instructions as before.

## [3.15.6] - 2026-09-04

### Fixed

- Updating no longer ends with a false alarm. After the new version installed
  and its post-install work ran correctly, the update still failed with a
  message saying the wrong version had done the work and telling you to run it
  again. It was reading the build date out of the version line and comparing
  that to the version number. It now reads the version number itself, and still
  stops the moment the work really was done by a different version.

## [3.15.5] - 2026-09-04

### Added

- The release train now publishes from the repository's own script as well:
  after the release pull request lands, one command tags the exact landed
  commit from a clean detached worktree and starts the publisher, refusing up
  front when the landed commit is missing, when the remote has moved past it,
  or when the version recorded in that commit is not the one being released.
  The publisher's log, process id and exit receipt sit beside the gate and
  pipeline receipts for the same run.
- A worker's merge request now reaches an idle supervisor's screen instead of
  waiting in an inbox for the supervisor to go looking. A finished task that was
  parked pending a merge could sit unseen for hours, because the message that
  said so was delivered silently. Only merge requests gain this — ordinary
  status chatter still waits for the supervisor's next turn, so nothing types
  over work in progress.
- A supervisor whose messages have gone unread past their delivery window is now
  told how many are waiting and who sent the oldest, instead of finding out by
  checking. Blockers, verification handoffs and status replies still arrive
  quietly, as before — what changed is that the backlog itself is announced once,
  and again only when a different message becomes the oldest one waiting.

### Changed

- Whether a message may interrupt a supervisor now depends on who actually sent
  it, as recorded when the message was written, rather than on the sender label
  attached to it. Labels can be set by whoever sends the message; the record
  cannot.
- A message that could not interrupt is now reported with which of the two
  reasons applied: it was never the kind of message that interrupts, or it was
  and the moment was wrong. Both used to read the same, so a genuinely stalled
  request looked identical to routine traffic working as intended.

### Fixed

- A message could interrupt a supervisor's screen simply by carrying another
  supervisor's name. The name was checked against the roster, but anyone sending
  a message can choose the name it carries, so spelling a real supervisor's name
  was enough to skip the content check that every other interrupting message has
  to pass. Interruptions are now decided from the sender recorded at send time.
- Messages sent to a worker now appear once instead of twice. A worker that was
  idle received the full message a second time as a separate prompt, moments
  after the first copy, with nothing to mark it as a repeat — so a worker that
  had already acted on it could be led to do the same work again. The second
  copy is now a single line telling the worker a message is waiting and naming
  it, and the message itself is delivered once.

## [3.15.4] - 2026-09-04

### Added

- `worker_status` now names the epic each live supervisor is running, including
  supervisors in other factory sessions that share the same checkout. Knowing
  another supervisor is live was only half of what an operator needs before a
  merge, reset or shutdown; the other half is what that supervisor is in the
  middle of. A supervisor running nothing reads "no epic", and a task store that
  cannot be read this pass says so explicitly rather than being reported as
  "no epic".

### Fixed

- A message from one supervisor to another now reaches the recipient's pane
  instead of waiting in an inbox for a poll. Supervisor-sent messages were
  labelled with the generic sender "supervisor", which the recipient's delivery
  path could not match to any registered supervisor, so the wake was declined
  every time and cross-session coordination depended on the recipient happening
  to check. Messages between supervisors now name the supervisor that sent them,
  which is also the only useful label when two of them share a checkout.
  Messages to workers are unchanged.
- `cas update` now runs its post-install phases with the binary it just
  installed. Updating from an older version used to run the schema-migration,
  all-projects refresh, user-level store and skills-sync phases in the
  pre-update process image, so the first update reported the *old* version's
  behaviour and a second `cas update` was needed to converge. The receipt now
  states `refresh_binary_version`, the version that actually performed the
  refresh, and `--json` emits a single combined document instead of two.
- A release no longer fails because the machine cutting it was busy. `cas init`
  aborts itself after a wall-clock budget so a hang cannot squat a CPU core, and
  that budget was a fixed 300 seconds with an all-or-nothing opt-out. On a loaded
  host a test's child `cas init` reached it and took the release gate's
  archive-mode row down with it, on timing alone — the same tree passed minutes
  later on a quiet box. The budget is now settable with `CAS_INIT_TIMEOUT_SECS`,
  and the release gate raises it to 900 seconds for its own children, naming the
  value in its receipt. An ordinary `cas init` keeps the 300-second watchdog, a
  meaningless override falls back to it rather than disabling it, and
  `CAS_INIT_NO_TIMEOUT=1` still turns the watchdog off outright.
- Concurrent `cas hub start` / `cas hub restart` no longer stall ten seconds
  and report a spurious failure when another command has already brought up
  the hub that was asked for: the waiters resolve as soon as a live hub
  satisfying the request is observed, plain `cas hub stop` is unchanged, and
  each timeout names the wait that expired. The concurrent lock-owner test now
  asserts both commands succeed and exactly one owner exists.
- Release gates run through `scripts/release-train.sh`: each run gets a
  directory keyed by version and worktree with an attributable receipt, the
  gate's PID is recorded, a second gate for the same run is refused by name,
  `--stop` signals only the recorded PID, and no release script locates a
  process by name pattern. `release-train.sh pipeline` opens the PR, waits for
  the pull-request checks to actually pass, enqueues, re-enqueues if the queue
  drops the entry, and records the landed main sha.

### Changed

- **Behaviour change for automation:** if the newly installed binary cannot be
  run for the post-install phases, `cas update` now exits **non-zero** with
  "binary updated to X; refresh did not run — run `cas update` again", and its
  JSON receipt carries `refresh_binary_version: null` and
  `refresh_status: "skipped"`. Previously such a run fell back to refreshing
  with the pre-update image and exited 0, which read as converged when it was
  not. A run whose refresh is performed by a version other than the one just
  installed fails the same way.

## [3.15.3] - 2026-09-04

### Fixed

- `cas doctor` no longer counts a dependency whose endpoint is a quarantined
  task as an "orphaned dependency". Quarantining foreign rows (which doctor
  itself prescribes) hid those tasks from the board but not from the dependency
  table, so every edge touching one read as orphaned with no command that could
  clear it. Quarantined endpoints now fold into the OK row as a stated count;
  only an id genuinely absent from the store warns, naming the offending rows
  and `cas doctor --fix`, which prunes them. A store error during the lookup is
  never mistaken for absence: such rows go to an unresolved bucket, the prune is
  skipped for that run, and the row names the error.
- `cas index code` now reconciles the code-vector queue at the end of every run
  (and on daemon start): queue rows whose symbol no longer exists are dropped,
  failed rows are re-armed (all of them on `--force`, otherwise all but
  provider-rejected input), rows whose content hash drifted are rewritten, and
  eligible symbols that were never queued are enqueued, with the counts printed.
  Doctor's "queue rows name symbols that no longer exist" and "failed" clauses
  now name commands that clear them. Indexer writes take the SQLite write lock
  up front with bounded retry and wait out a concurrent BM25 writer, so a
  `cas doctor` or second `cas serve` running alongside no longer turns into
  "failed to retire deleted source file: database is locked" file failures.
- The cross-project row scan no longer reports a registered root whose database
  has no `tasks` table as a project that "could NOT be read". Such roots (a
  copied CAS root used as a test fixture under the artifacts directory) are
  listed as skipped, never drive a warning, and the registry refuses to
  register roots under the factory artifacts root, `~/.cas/scratch`, or
  Cassy-named temp directories in the first place. New
  `cas known-repos forget <path>` removes a live registry row and its bindings
  with a receipt; it refuses the current project root without `--yes`.
- Closing a task no longer fails the tmpfs proof-receipt gate because the close
  reason merely mentions a temp path in prose. A temp path counts as a cited
  receipt only when it is presented as one (a proof cue word shortly before it,
  or the path opening a line or list item); a reason that cites no durable
  artifact at all is still rejected, and the quoted path no longer carries the
  sentence's trailing period.
- `cas integrate mecha-cassy` now repairs the project `.cas/proxy.toml` that
  shadows the machine registration, so the fix `cas doctor` prescribes actually
  clears the warning it prints. A project proxy file replaces the machine
  allowlist rather than widening it, so one left naming the retired Slack tool
  routes kept them authoritative through any number of re-runs while the
  command reported "already configured". It now rewrites those routes in place
  — comments, key order and every unrelated server and route survive
  untouched — and refuses to claim "already configured" while a shadowing file
  still drifts. A hub server block in that file is removed only when it is
  identical to the machine registration that supplies it; one that differs is
  an override, such as a project pointed at a staging hub, and is kept and
  reported rather than silently switched. A project file that names no hub route is still left alone
  (widening a policy the project declared is not the command's call) and is
  reported with the exact routes to add. The `mecha-cassy` doctor row now names
  the file the stale entries live in, machine or project.

## [3.15.2] - 2026-09-04

### Fixed

- `cas update` no longer reports a project as refreshed while its schema stays
  behind. A migration whose name had already been recorded under another id
  (a store migrated by a pre-release build that numbered it differently) made
  the ledger write fail with "schema was detected but its ledger row could not
  be recorded" on every run, leaving the project pinned two migrations short.
  The runner now reconciles such rows — the stale entry is logged to the
  reconciliation ledger with both ids and removed, the migration is recorded
  under its current id, and whatever migration now owns the vacated id is
  re-evaluated honestly (detected or applied). Every pending migration gets
  its turn before the first failure is reported, and the ledger-write error
  names both ids involved. Read-only checks still report the wedge; only
  `cas update` repairs it.
- `cas update` finds every local project, not just the ones already in its
  registry. The filesystem scan stopped at the first `.cas` it met — on any
  machine with a user-level store that was `$HOME` itself — so discovery had
  silently collapsed to the registry and dozens of projects never received
  migrations while the summary said "N projects refreshed". The scan now
  descends (skipping `.cas/`, hidden and vendored directories), refreshed
  projects are registered so discovery converges, a `.cas` without a database
  is listed as "not refreshed (unregistered)", the user-level `~/.cas` store
  gets its own refresh phase, every schema line names the store it migrated,
  and `cas update --register <path>` adds a project by hand.
- A `CHANGELOG.md` or project doc saved as UTF-16, or with a UTF-8 byte-order
  mark, is now read instead of skipped. The changelog index no longer fails the
  whole pass on an encoding it can decode, a leading byte-order mark no longer
  swallows the file's first release heading, and a doc that genuinely cannot be
  decoded is reported by name and reason in the pass report rather than
  disappearing from the knowledge wiki with no explanation.
- `scripts/release-gate.sh` picks its own scratch base (`/var/tmp/cas-release-gate`)
  instead of one under `$HOME`. On a machine with a user-level `~/.cas` store the
  old default sat beneath it, so the gate refused its own archive-mode and
  snapshot-portability rows until an environment variable was exported by hand.
  Setting `CAS_RELEASE_GATE_HOME_DIR` still overrides the base, and the receipt
  now names the base it used and where that value came from.

## [3.15.1] - 2026-09-04

### Fixed

- Closing a task no longer wedges when the worker's branch moves after the
  verifier dispatch was minted. The proof now binds the delivered commits: new
  commits, merges or fast-forwards on top keep the dispatch valid as long as
  the delivery is still reachable, a rewritten or dropped delivery is still
  refused naming both tips, and re-running close after a genuine drift mints a
  fresh dispatch instead of replaying the spent id.
- `shutdown_workers` accepts a registered worker by name, agent id or session
  id (and a JSON-array argument), using the same identity lookup as
  `worker_status`; the error distinguishes a target refused by policy from one
  that is genuinely unknown instead of claiming both.
- Recording a verification verdict no longer fails instantly with
  "database is locked" under store contention: the write takes the lock up
  front (`BEGIN IMMEDIATE` with bounded retry) instead of upgrading a deferred
  read snapshot, which SQLite refuses without consulting the busy handler; the
  knowledge reindex reads page bodies before opening its write transaction;
  the WAL is capped at 64 MiB; the give-up error states the wait it attempted.
- Tests that set up a temporary HOME no longer read the machine's own project
  configuration: HOME only redirects user-level lookups, so a `proxy.toml`
  registered in a checkout was still visible to tests running from a worktree
  inside it. The test harness pins the project root by default, the release
  gate names such a file and neutralizes it for the run instead of refusing,
  and the gate self-test defaults its scratch base to a path with no `.cas`
  ancestor.
- Insta pending snapshots (`*.snap.new`) are ignored so a test run can no
  longer sweep one into a commit.

## [3.15.0] - 2026-09-03

### Added

- `cas integrate mecha-cassy` sets up MechaCassy on a machine in one command:
  a machine-level proxy registration every project inherits, Claude Code and
  Codex entries by environment-variable name, and an authenticated tool list as
  the receipt. `cas doctor` gains a mecha-cassy row; a credential script handles
  the one human step; onboarding doc for teammates. The `mecha-cassy` skill is
  now the whole posting contract (the separate mecha-cassy-post skill is
  retired) and a new `user skills` doctor row flags stale or orphaned
  user-level skills per harness.
- Cloud sync consumes per-row push outcomes: rows the cloud kept newer are
  acknowledged, rejections are held with their reason and a remedy, and
  `cas update` / `cas doctor` say which is which. Terminal failures parked by
  an older client are requeued once after upgrade.
- Cloud sync consumes dependency deletion tombstones: a removed edge
  propagates everywhere and cannot be resurrected; the "N edges healed" churn
  on every pull is gone.
- Revision-based conflict resolution: when both sides carry a server revision
  the higher revision wins and clocks are not consulted, so a machine with a
  wrong clock no longer silently wins or loses. Revisions arbitrate the
  "keep newest" strategy (personal pulls and teams configured for it); an
  explicit remote-wins or local-wins team strategy stays authoritative.
- `cas doctor --fix-cloud-rows [--yes]` sets aside task rows a past sync leak
  copied into the wrong project: they leave the ready queue and are never
  pushed, stay readable by id, and `--release-cloud-rows` restores them.
  Doctor reports unattributed and colliding rows with the rekey
  recommendation.
- `cas doctor --verbose` prints each check's duration and a slowest-phases
  table; `--json` carries per-check timings.
- `cas doctor`, worker status and the spawn receipt warn when two live
  supervisor sessions share one clone path (GH #699).
- Factory `gc_report` lists stale disposable roots under the temp directory
  with age and size; isolated Cassy roots default to `~/.cas/scratch/<name>`
  and refuse a RAM-backed location that would hold worktrees or build cache
  (GH #704).

### Fixed

- Tasks whose owner project was never recorded can be started and claimed
  again instead of being refused as an "unassigned legacy row" (GH #690).
- Client project-identity canonicalization matches the cloud's rule exactly,
  consumes the cloud's alias record, and a parity audit reports per-project
  convergence (GH #669).
- Pull attribution reads a row's origin project before the server's scope
  stamp, so another project's rows are no longer ingested as this project's;
  throwaway checkouts no longer mint a cloud identity on push (GH #701).
- `cas doctor` on a large store dropped from over two minutes to about three
  seconds: the history-index check ran a per-commit JSON scan over every task
  (GH #700).
- The embedding drain no longer retries a provider refusal forever: oversized
  text is capped, refusals are told apart from outages, a poisoned batch is
  bisected so only the offending unit is quarantined with the provider's
  message, and doctor reports pending and quarantined separately with
  `cas history embed --retry-quarantined` (GH #695).
- Doctor's code-vector figures come from symbol coverage instead of queue
  rows, so they no longer reset when the queue is re-armed, and a discarded
  vector cache generation is named instead of showing every vector gone
  (GH #696).
- `cas doctor --verbose` no longer inserts thousands separators into task ids,
  UUIDs and timestamps, and the "cannot reach N rows" count matches the rows
  it lists (GH #697).
- The code index decodes UTF-16 and UTF-8-BOM source files instead of failing
  forever; undecodable files are skipped with a named reason and excluded from
  coverage; a UTF-8 BOM no longer corrupts a file's first symbol (GH #698).
- A merge request sent after a new push is delivered instead of being
  suppressed as "already landed": the suppressor now checks the live branch
  tip, not the anchor recorded at the previous merge (GH #703).
- `worktree_merge` fetches and fast-forwards the local target before merging,
  refuses a diverged target before touching it, and reports a rejected
  non-fast-forward push honestly with a working remediation (GH #703).
- probe-comm removes its scratch root on success and failure; test fixtures
  no longer leak hand-named directories into the temp directory (GH #704).
- Migration cursor fixtures derive their expectation from the migration
  registry instead of a hand-pinned list.

## [3.14.0] - 2026-09-03

### Added

- Commander gets a session picker in the header and a back control: every
  session on every paired machine is one tap away, and reopening the app
  restores the last session instead of "No session open".
- Commander on a phone shows a reflowed transcript of each pane at readable
  text size, with the true 80-column terminal one tap away, instead of
  squeezing an 80-column agent interface into a ~46-column grid.
- A phone in landscape is treated as a phone: one shared detection rule for
  the stylesheet and the layout logic, and a landscape arrangement with
  full-height terminal and edge rails.
- Clickable phone-sized mock-ups of three mobile Commander directions (Inbox,
  Deck, Voice) and an Android field report are published under docs/reports.

### Fixed

- Remote Commander viewers can no longer shrink the operator's local console:
  while the local dashboard is attached it owns each pane's PTY size, refused
  viewer resizes are audited, and the viewer renders the authoritative size.
- "Talk to the supervisor" sends on Enter and on the Send button, never renders
  a disabled Send, and states the real reason when a message cannot go out,
  including the exact `cas hub pair --scopes` command for a read-only device.
- A default `cas hub pair` invitation pairs on the first attempt: the link
  declares its scope ceiling, the form requests only what was granted, and
  hub refusals are sentences with a next step instead of "unauthorized".
- Opening a pairing link opens the pairing dialog (also in an already-open
  tab), the optional email field no longer pops the keyboard, and the Pair
  button stays reachable with the keyboard up.
- The phone bottom rail is one bar: shared control treatment, no stray
  focused-pane outline on Pair, no seam, readable machine chip, labelled
  attention count, 44px tap targets.
- Terminal attach degrades honestly on older browsers: AbortSignal.any is
  feature-detected, the connecting timer advances so diagnostics appear, a
  missing API is reported once with the minimum browser versions, and repeated
  failures collapse into one attention entry.
- Commander no longer rebuilds the whole page on every hub update, so the
  message composer keeps focus (and a phone keeps its keyboard) while typing.
- The hub session list reports each session's live worker roster from that
  session's own project, instead of an empty list or the wrong project.

## [3.13.1] - 2026-09-03

### Changed

- The built-in `cas-cut-release` skill (Claude, Codex, and Grok mirrors) now
  gives one exact publish procedure: release credentials come from a
  configurable user-level `release.env` and are proven by name only, release
  log/PID/done receipts live under the durable artifacts root instead of the
  clean tag worktree, `release.sh --publish-tag` alone owns annotated-tag
  creation and local preflight, a PID-safe wrapper always records a numeric
  exit receipt, and publication may only be announced with tag, workflow,
  asset, latency, Slack, and host receipts in hand.
- MechaCassy release posts default to the rubric channel name `cas-internal`
  and fall back to the directly configured Slack transport when a live Cassy
  proxy lacks the registration.

## [3.13.0] - 2026-09-03

### Added

- The MechaCassy Slack transport gives every harness a shared, fail-closed way
  to publish release notes with channel checks, paced threads, receipts, and
  environment-only credentials.

### Changed

- Supervisor identity now survives restarts cleanly: live workers remain
  reachable, old same-name sessions retire safely, and task-free worker deaths
  no longer create misleading supervisor warnings (GH #677, GH #678).
- Task creation now detects meaningful overlap in descriptions and shared
  file, line, function, or slug identifiers instead of relying on title words
  alone (GH #679).
- Worktree merge diagnostics now identify the exact check-runs endpoint and
  source SHA, classify missing or failed checks, and state the advisory merge
  policy (GH #680).
- Assignment briefs are rechecked at delivery time so closed or cancelled work
  is not re-delivered, and each worker receives the tool namespace for its
  harness (GH #682).
- The generated code map is refreshed so repository navigation reflects the
  assembled release tree.

## [3.12.1] - 2026-09-02

### Added

- `cas-cut-release`, the supervisor's single release procedure: a ten-step
  fail-closed train with a "what went wrong before" appendix and a
  self-learning `references/failure-log.md` that any agent must extend
  (`scripts/release-gate.sh --learn`) before retrying a new release failure.
- `scripts/release-gate.sh <version>`: the pre-queue gate that refuses to
  proceed on version literals in tests, workspace check, nextest, doctests,
  an archive-mode run outside the checkout without ripgrep, snapshot host
  independence, stale root projections or ledger, changelog and version
  mismatches, release-script preconditions, procedure guardrails, or a dirty
  tree; every failure-log entry must map to a gate check.

### Fixed

- `scripts/release.sh` removes stale BLAKE3 build outputs before its
  portable-ISA audit, so a prior build in the tag worktree no longer fails
  the "exactly one BLAKE3 build output" check.


## [3.12.0] - 2026-09-02

### Fixed

- The task verifier's verdict templates now record `files_reviewed` (the
  field was silently dropped before) and its test-first check uses a valid
  ripgrep flag instead of erroring on every run; the learning reviewer now
  receives the unreviewed learning IDs it is asked to review; the Stop hook
  and the session-learn skill share one prompt source.
- Removed managed builtins are pruned from every synced skill directory
  (the retired code-review skill no longer appears in session menus), the
  orphaned code-review workflow files are removed, the close-rejection and
  factory-planning messages name the real procedures, and dead guide
  constants are gone.
- Supervisor and worker references no longer teach the retired
  `pending_supervisor_review` status or the `bypass_code_review` flag;
  `supervisor_override` is documented once with its constraints; the two
  contradictory merge procedures collapse to the worktree merge; the raw SQL
  recovery recipe is gone; the Codex factory supervisor definition is
  constraints plus a pointer.
- cas-memory-management is rewritten against the live memory API (all
  request fields, one entry type enum, frontmatter inside content, no
  file-store model); the valid-action lists in cas-task-tracking and
  cas-search are generated from the dispatch table and pinned by a test.

### Changed

- Worker session guidance fits the SessionStart budget (about 6 KB instead
  of 9.9 KB), so ready tasks and memories are populated on every spawn;
  harness-enforced rules are no longer restated as prose.
- Eight skill descriptions lead with their trigger; shipped builtins carry no
  operator e-mail, host paths, or unshipped runbook links; the Claude release
  account gate is a config key; cas-brainstorm and cas-ideate can write their
  own artifacts again and hand off to the supervisor instead of a missing
  `/plan` command.
- mcp-integration teaches `cas mcp add/list/import`, proxy.toml and the
  `mcp__cas__system` proxy actions; release-notes is procedure-only and
  rubric-driven; fallow and cas-nuxt-playwright are an opt-in stack tier
  (synced on stack detection or `[skills] optional`).
- The three documentation skills share one hygiene reference;
  cas-domain-modeling is merged into cas-codebase-design; tiny references
  are inlined; the git-history-analyzer and issue-intelligence-analyst agents
  are retired from the builtin set; cas-writing-for-agents now states steps,
  frontmatter, information hierarchy and completion criteria; the projection
  drift guard covers shell, YAML and JS twins.


## [3.11.0] - 2026-09-02

### Changed

- `cas doctor` renders a grouped report: a header with project and version,
  one line per section (Store, Indexes, Cloud, Config, Integrations) when
  every check passes, non-OK checks on their own row with a short message and
  the remediation on an indented line, thousands-separated counts, and a
  counted summary with elapsed time. `--verbose` prints every check in full.
  Cloud-pull attribution warnings no longer spill onto stderr ahead of the
  report; they are collected into one Cloud row.
- `cas update` prints one row per refreshed project (migration, index,
  skills, membership, cloud) with detail only for projects that warned or
  failed, deduplicates warnings that repeat across projects, drops the second
  per-project summary block, keeps step receipts on one line, and closes with
  a version, project count, and elapsed-time banner. `--verbose` restores the
  full transcript. No Rust debug formatting reaches the terminal.
- `cas cloud push`, `cas cloud pull`, and `cas cloud sync` print one summary
  line: non-zero counts only, "nothing newer" when nothing changed, push
  failures grouped by message with a retry hint, and a spinner that only
  appears on a terminal. `--verbose` prints per-kind and per-error detail.
  JSON output for all three commands is unchanged.


## [3.10.1] - 2026-09-02

### Fixed

- Project-scope cloud pull reconciles entries that exist locally as archived
  rows through the same last-writer-wins path as team pull, instead of
  failing every one of them with "entry already exists" (347 per pull on
  one contaminated store); duplicate-key races resolve the same way and
  unrelated store errors still surface.
- `cas cloud purge-foreign` no longer trusts a backfilled `origin_project`
  alone: rows the doctor's cross-database (id, title) and activity evidence
  attribute to another project are now purgeable, id collisions and
  unattributed replicas are never deleted, task deletes match on (id, title),
  safety lookups fail closed, and the dry-run labels each row with the
  evidence that selected it.
- `cas doctor` reports the doctor's foreign-row evidence count next to the
  purge delete-set count, names every retained row with its reason, and stops
  prescribing `purge-foreign` as complete remediation when the two disagree.
- `cas doctor` names the configuration file that registered an unresolvable
  MCP stdio command (the user-scope `code-mode-mcp/config.toml` or the project
  `proxy.toml`) instead of always saying "repair proxy.toml".

## [3.10.0] - 2026-09-01

### Removed

- The multi-persona code-review pipeline: the `cas-code-review` skill and
  workflow, its persona references, the deprecated code-reviewer agents, the
  worker review-dispatch gate, and the `[code_review]` config section.
- The `code_review_findings` close field and its envelope validation; the
  `pending_supervisor_review` task status (existing rows and incoming legacy
  cloud rows map to `awaiting_merge`).

### Changed

- Task review is now the supervisor's merge-time diff review: the worker's
  branch diff is read against the task spec with scoped test receipts, and the
  supervisor records a verification row as the review receipt; the supervisor
  skill documents this as the canonical procedure.
- `bypass_code_review` is replaced by `supervisor_override` (reason required,
  supervisor-only, logged); the old flag remains a deprecated alias for one
  release.


## [3.9.1] - 2026-09-01

### Fixed

- Moving a task to another project now queues exactly one delete for the old
  project key and one destination-keyed upsert, sent in that order in a single
  push, so the old project no longer keeps a re-created copy; deleting a task
  after a move targets its current owner key.
- The hub launched by `cas hub start`, `cas hub restart` or `cas update` runs
  outside any factory worker containment scope, so it survives the worker that
  launched it; `cas hub` shows who launched it and reports a silent exit.
- `cas hub stop`, stale-hub replacement and failed-launch cleanup now only tear
  down a cgroup that Cassy itself created for the hub; a hub record that names
  an inherited terminal or session scope is refused with a warning instead of
  killing every process in it.

### Added

- `delivery_mode = local_merge` on epics/sessions: the close gate tells workers
  to wait for the supervisor's local merge instead of pushing, the worker skill
  documents both modes, and the worker guard refuses `git push` to origin in
  that mode.

## [3.9.0] - 2026-09-01

### Added

- Task dependency edges (epic→child, blocks, related, duplicate) sync to the
  team cloud as a `task_dependencies` entity: enqueued on add/remove/epic
  create, pushed in both envelopes, applied and deleted on pull, with dangling
  edges parked instead of inserted.
- Every pull/sync self-heals dependency edges: edges the cloud lacks are
  re-pushed, edges the local store lacks are materialized, deletes in the same
  envelope are honored, and a one-line "healed N edge(s)" summary appears only
  when something changed (a second sync heals 0).
- `spawn_workers` provisions the worker worktree inside the task's
  `target_repo` (start point resolved there, worktree under that repo's
  `.cas/worktrees/`) and fails fast with an explicit `cross-repo spawn:` message
  when it cannot.
- `cas update` runs the freshly installed binary's post-update hook after the
  swap, so a stale running hub restarts on the same run even across a version
  boundary.

### Changed

- One canonical project identity: git-remote and case variants of a project
  slug resolve to the pinned slug for registration, push stamping, pull
  ownership, dependency ownership and `purge-foreign`; `cas doctor` reports
  rows still attributed to an alias and `cas cloud project --adopt-aliases`
  rewrites them and re-pushes under the canonical identity.
- Cloud push treats a last-write-wins skip as an acknowledgement, consumes
  per-row outcomes/reasons when the server provides them (parking real
  rejections without poisoning the batch), and requeues terminal
  version-gated failures automatically once the client meets the server
  minimum.
- The PTY-timing test family uses bounded deadline polling instead of fixed
  sleeps, ending the recurring merge-queue ejections.
- Moving a task to another project keys the replacement upsert to the
  destination project, so the old project no longer keeps a stale copy.
- A task that becomes unblocked wakes its assigned worker with a start message
  instead of waiting for the stall detector.
- The worker MCP proxy allowlist uses one documented `<server>.<tool>` /
  `<server>.*` format with alias normalization and live reload, denials name
  the exact entry to add, and a missing stdio executable is reported as
  `executable_missing` (also checked by `cas doctor`).
- The sync queue collapses legacy duplicate rows during migration and upserts
  on conflict, `cas cloud queue --retry --retry-reason <reason>` requeues only
  matching parked rows, and `cas doctor` prints the exact counts blocking
  `purge-foreign` with a runnable retry → push → purge remediation.

## [3.8.0] - 2026-09-01

### Added

- `mcp__cas__task` list/ready/blocked/available accept `include_foreign`; the
  board is scoped to the current project by default and prints how many
  foreign-origin rows were hidden. `show` on a foreign task names its owner.
- Supervisor `task update origin_project=<project>` now moves the task in the
  team cloud: the old project-keyed row is deleted before the row is upserted
  under the new owner (delete failure withholds the upsert), the destination
  must be a registered team project, and a decision note records the move.
- `cas hub authorize` resolves the Commander origin without `--hub-url`
  (explicit flag, then the hub process record, then `[hub].public_url`, then
  the last origin that worked), accepts bare hostnames as HTTPS, and prints the
  requested/granted scopes once.

### Changed

- Team push no longer overwrites an explicit `origin_project` with the pushing
  project; only rows with a missing or blank origin inherit it, and Global
  tasks stay unstamped.
- Team pull keeps foreign-keyed task rows and arbitrates duplicates in favor of
  the owning project; a stale replica can no longer reopen a task its owner
  closed (discarded rows are logged as `owner_wins`).
- `cas cloud purge-foreign` classifies only rows explicitly attributed to
  another project as foreign; tasks with no attribution and every local rule are
  kept, and it refuses (even with `--force`) to remove more than half of the
  tasks or any proven rule.
- Bare `cas hub` shows status (with both the running and installed versions);
  `cas hub start` on a live hub whose version or flags differ restarts it
  instead of failing; `cas update` restarts a running hub left on the old
  binary.
- `cas list` reports the live registered worker count for a factory session
  instead of stale session metadata.

## [3.7.7] - 2026-09-01

### Changed

- Factory delivery monitoring now flags a failed required check while
  auto-merge is armed, or an auto-merge arm that disappears after green checks,
  with duplicate notifications suppressed per pull request and head commit
  and new heads re-armed for notification.
- `branch_contained_in` reminders now refresh the exact `origin/<target_branch>`
  ref before checking containment, keep the reminder pending when that refresh
  fails instead of trusting stale state, and show the compared ref and commit.

## [3.7.6] - 2026-08-31

### Changed

- Claude worker launches preserve the requester's config and secure-storage
  selectors independently, auth checks stop waiting after a bounded timeout,
  and failed checks no longer silently choose the main account.

### Fixed

- Custom Claude profile directories now keep truthful names in account badges
  instead of being mislabeled as the main profile.

## [3.7.5] - 2026-08-31

### Added

- `cas setup` guides a newly installed machine through PATH, cloud login and
  team selection, device pairing, hub service, optional Viktor credentials, and
  first-project initialization with safe reruns and dry-run status reporting.

### Changed

- `cas cloud push` drains the complete personal backlog by default, stops when
  no progress is possible, and reports remaining team-scoped rows explicitly.

## [3.7.4] - 2026-08-31

### Changed

- Curated memories with importance at least 0.9 or positive helpful feedback
  stay in the working tier during decay, and reading a cold or archived memory
  brings it back to working; `[memory.decay]` settings and doctor counters make
  the policy visible.
- Legacy search-index inspection counts metadata without loading every stored
  document, so doctor and daemon checks are fast even with stranded entries.
- Tantivy indexes now use versioned paths; schema mismatches preserve the old
  directory and explicit reindexing migrates or quarantines it safely.

## [3.7.3] - 2026-08-31

### Fixed

- `cas update` now turns over stale `cas serve` processes before refreshing
  each project and repairs legacy Tantivy search-index roots; busy locks warn
  without failing the update.
- The contributor `cas-update` helper turns over running processes before
  refreshing project state.

## [3.7.2] - 2026-08-31

### Added

- SessionStart memory injection is now telemetered with honest retrieval
  outcomes: unresolved cards stay distinct from ignored cards, and reading an
  injected memory counts as use. Ambient rule and skill surfaces are recorded
  for the same impact accounting.
- Added a labeled retrieval-evaluation harness with committed fixtures and
  precision@5/recall@5 baseline gates so ranking changes are measured before
  they ship.
- `cas doctor` now detects memory entries stranded in a legacy daemon index;
  `cas doctor --fix` recovers them into the active index with bounded,
  resumable repair.

### Changed

- Helpful Memories now read from the live, curated, tier-aware corpus while
  excluding raw context blobs, and `cas stats` reports the live corpus count.
- `retrieval_metrics` supports session filtering and rejects unsupported
  filters instead of silently widening the result set.
- Rule review is enabled by default, and ambiently surfaced rules count toward
  the promotion flywheel, so useful rules can progress instead of remaining
  invisible drafts.
- Team sync upserts auto-promoted entries without turning a successful pull
  into an error; Claude worker lanes use canonical model identifiers and report
  workers that die during boot as failed. Project-managed skills are ignored by
  git so generated files no longer become repository noise.

### Fixed

- Background and legacy search indexing now share one repair path, making
  daemon-written memories visible to search and giving busy legacy processes a
  bounded doctor warning with a retry remedy.

## [3.7.1] - 2026-08-30

### Added

- Refreshed the generated `.claude/CODEMAP.md` navigation map and added a
  bounded codemap-latency receipt that proves no content change, freshness,
  local commit/push readiness, and the required protected-PR compute budget.
- Added `cas knowledge build --timeout-secs <seconds>` with a 90-second default
  and a single deadline covering the complete build, including source
  processing, model calls, merges, and post-commit repair.
- Added process-group cleanup for knowledge providers so a timed-out provider
  and its ordinary descendants are terminated and reaped, plus regression
  tests for the timeout, receipt, freshness, and no-write contracts.

### Changed

- Protected pull requests now run only the required Fast Validation and macOS
  Check paths; heavy compile lanes remain on main, scheduled, or explicitly
  dispatched runs. First branch pushes use the protected default branch as a
  trusted comparison base, while merge-queue jobs use their event base SHA.
- Updated the Claude, Codex, and Grok codemap skills to invoke the bounded
  knowledge build as best effort, record its exit status, and continue with
  codemap status proof when knowledge distillation is unavailable or times out.

### Fixed

- Codemap latency validation now labels detached GitHub Actions readiness
  separately from ordinary local readiness and rejects stale or missing
  freshness proofs without touching `CODEMAP.md`.
- Added regression coverage for review fixes that intentionally delete content
  after a task parks, preserving the final-tree merge proof contract.

## [3.7.0] - 2026-08-30

### Added

- `cas cloud unlink [--purge-remote]`: sever a project's cloud link locally
  and, with the flag, remove that project's remote records (entries, tasks,
  knowledge pages) — scoped discovery through the single `CloudSyncer` pull
  builder, dry-run support, fail-closed handling for record types the server
  cannot delete, and the local database left byte-untouched.
- Cassy-managed `.gitignore` blocks: consumer-project skill sync now maintains
  an idempotent managed block covering the distributed builtin files
  (`.claude`/`.codex`/`.grok`), preserving user entries, skipping the cas-src
  authoring tree, and surfacing `git rm --cached` remediation when a managed
  file is already tracked.
- Durable external-condition wake triggers for parked work:
  `branch_contained_in` and `tag_exists` reminders persist across sessions,
  are probed by the daemon with bounded git commands, and deliver a
  supervisor notification once on the false→true transition (GH #624).
- Trace archives: bounded, range-readable archive retention in the daemon,
  with archive readers exposed through the CLI store.
- Versioned rule and skill creates with create-history snapshots (m242), rule
  lifecycle gated on measured outcomes, and surfaced-artifact impact tracking
  (m243).
- `cas-supervisor` epic-driving playbook reference: one-integration-PR
  discipline, target pinning, merge sweep, and spawn conventions in a 1.1KB
  imperative reference mirrored to all three harness flavors.

### Fixed

- Team-sync deserialization: `TaskDeliverables` tolerates the legacy
  JSON-string encoding on pull and canonicalizes on serialization, and the
  cross-DB relocation writers normalize payloads before push — ends the
  "Team pull encountered N error(s)" partial pulls (98 cloud rows were
  repaired server-side alongside this release).
- Release workflow: a tag pushed before its Release Prebuild completes now
  waits (bounded, exact-SHA) instead of silently falling back to cold builds,
  and any cold fallback is loudly reported (GH #603).
- Factory: a child task whose WorkTarget equals the parent epic's default no
  longer silently delivers to trunk — it inherits the live epic lane with a
  durable decision note; explicit non-default targets remain authoritative
  (GH #625).
- Skill validation runs network-isolated with a degraded-validation fallback,
  and skill-id parsing tolerates warning-suffixed ids.
- Sync scoping: personal deletes and legacy team task upserts are
  project-scoped; dangling cloud fixture symlinks are rejected.
- Knowledge loop preserves provenance across sync, including synced skills.

## [3.6.0] - 2026-08-29

### Added

- `cas-image-generate` builtin skill: style-aware asset generation for apps,
  websites, and reports. Harvests a project's design context (palette, motifs,
  typography feel) into style tokens, routes each asset type to Google's Nano
  Banana models with per-type prompt templates, supports reference images for
  style consistency, and degrades with explicit setup guidance when
  `GEMINI_API_KEY` is absent. Shipped in Claude / Codex / Grok flavors.
- SVG & web-assets reference for the image skill: agent-authored SVG as a
  first-class route (decision table, 24px-grid authoring standards, palette
  CSS variables, worked examples), a raster→vector bridge that probes local
  tooling (vtracer/potrace/inkscape), and the favicon/OG/WebP web pipeline.
- Explicit origin-project identity on tasks (`origin_project`, m241): foreign
  tasks replicated by the historical unscoped-sync era no longer rank in
  another project's ready/available surfaces; sync stamps identity on push and
  pull, and supervisors get an audited reassignment path.
- New Cassy logo (generated with the image skill) and refreshed README
  capability documentation.

### Fixed

- Image-generate helper: reference payloads are assembled via files instead of
  argv (`--reference` no longer fails on ARG_MAX for real image sizes), and
  output naming honors the API's returned MIME type instead of writing
  mislabeled bytes.
- Merge-request suppression resolves repository context from the explicit
  local root before the host known-repos registry (a duplicate registry
  selector could silently disable suppression), handles linked git worktrees
  correctly, and an explicitly registered agent role now takes precedence over
  the ambient `CAS_AGENT_ROLE` environment.
- Global-scope tasks queue with no origin project instead of inheriting a
  project identity they should not carry.

## [3.5.0] - 2026-08-29

### Fixed

- Team sync deletes now send the project-scoped identity (`project_id`), unparking
  deletions that older servers' project-aware DELETE contract had permanently
  rejected; parked rejections requeue and flush after upgrade.
- Close-gate tree-effect check honors its "present or explicitly evolved" contract:
  union-merged parallel deliveries pass via token-level per-added-line containment,
  and the rejection message names the supervisor merge-receipt recovery path (GH #597).

### Added

- Skill persistence gated on `validation_script` execution at create/update.
- Measured rule promotion: Draft→Proven driven by outcome evidence instead of a
  single call; real rule impact tracking increments `surface_count` at injection.
- Provenance end-to-end: `Entry.source_ids` populated through `Rule.source_ids`.
- Version history with tombstone deletes for rules and skills (rollback capability).
- Append-only trace archive: events/recordings past 30 days compress instead of
  hard-deleting.
- Structured task execution state: schema'd patchable state blob on tasks.
- SessionStart memory injection carries the full first line for high-importance
  memories; hardened session memory hygiene.

## [3.4.2] - 2026-08-27

### Added

- **Cassy can run workers on a fourth AI backend: OpenCode driving Qwen.** `cli=opencode` spawns workers on Qwen 3.8 Max through a QwenCloud Token Plan subscription (`sk-sp-` key), validated end-to-end by a live conformance receipt — a Qwen-driven worker completed a real Cassy task lifecycle (create, code, commit, push, verified close), survived cancellation, retained permission denials, and kept two account roots isolated. Model selectors carry an explicit route (`qwencloud/…`, `alibaba/…`, `local/…`) with no silent cross-route fallback; routes without a receipt are refused before queue insertion.
- **Model routing is now a checked rulebook, not folklore.** A typed, embedded lane registry defines the worker lanes — light: Haiku 4.5, standard: GPT-5.6 Luna at xhigh, taste: Claude Opus 5 at high, heavy: GPT-5.6 Sol at high, with Terra under standing suspension — and every spawn path (MCP, direct CLI, daemon respawn, doctor) enforces it with rejections that name the violated rule and the available alternatives. Work can be requested by `lane=`; a missing primary backend produces a warned, never-silent substitute, and asking for an exact model is never rewritten. Doctor and preflight report per-backend availability as available / unavailable (with the enable command) / unknown. The docs' routing tables are generated from the same registry the code enforces.
- **The Commander hub installs as a managed service.** `cas hub service install|uninstall|status` (with `--dry-run`) writes and enables a systemd user unit on Linux (linger handled) or a launchd agent on macOS: restart-on-failure, discoverable logs, no secrets in unit files, idempotent re-runs, wired into the installer's next steps.
- **Every release now proves its own installer on real machines.** A `release.published`-triggered workflow runs the actual `curl … | sh` install on a hosted Apple Silicon Mac and a clean Linux container, verifies `cas` works from a fresh login shell, and uploads full transcripts; release copy may only claim "install works" with that green receipt.
- **Installing Cassy now leaves a working `cas` in a new terminal.** The installer detects whether its install directory is on PATH in your *login* shell, offers to add a marker-guarded guard to the right startup file (`.zshenv` on zsh, so the non-interactive shells MCP clients spawn also see it; `.bashrc` or `.profile` on bash), never edits the same file twice, and prints the exact line to add if you decline or there is no terminal to ask on. It then checks the result by running `cas --version` in a fresh login shell and only reports success when that actually works.
- **A brand-new machine gets one friendly line.** Typing `cas` before anything is configured now names the next command instead of printing a factory preflight's list of everything missing.

### Fixed

- **Factory messages arrive when sent, not when someone happens to wake.** Enqueuing a message now nudges the daemon immediately, delivered messages carry their age and a staleness marker, and a spawn assignment for a task the worker has already finished is withdrawn instead of replayed as if new.
- **A task can no longer be waved into supervisor review while its branch is unmerged.** Every close-path transition re-fetches and re-validates the live branch tip's ancestry at decision time, closing the race where a straggler commit slid past a partial merge.
- **The workspace-contract hook stops rejecting writes inside a worker's own worktree.** Containment now uses the registered worktree binding with canonicalized, fail-closed path comparison instead of guessing from the current directory, fixing the case where one subtree was allowed and its sibling was blocked.
- **Workers are never cut from a stale epic branch again.** A behind-only epic base fast-forwards at cut time; a diverged base refuses the cut with the exact branches to reconcile, instead of warning and cutting stale anyway.
- **`cas doctor` no longer calls a dead search index healthy.** The search-index check compares per-type document counts and freshness against the store and fails with the exact reindex command when they diverge.
- **Filing a bug report no longer dies on a missing label or guesses a repository.** A missing `agent-reported` label degrades to a labeless filing with the degradation stated, and an unset `issues.repo` refuses with the exact config command instead of proceeding against an implicit default.
- **Release-note posting has a working, verified Slack route again.** The runbook documents the measured transport inventory and the canonical approved route with a real receipt, and codifies the supervisor handoff for workers without a Slack surface.

## [3.4.1] - 2026-08-20

### Changed

- **A tagged release now publishes in about two minutes instead of fifteen.** The release tree is final the moment the version-bump PR lands, so both platform archives are built then, and tagging adopts those prebuilt artifacts instead of starting a cold cross-platform build on the tag's critical path. The Linux lane builds on the self-hosted runner. A tag with no usable prebuild still builds at tag time, so the slow path remains a working fallback rather than a failure.

### Fixed

- **The 3.4.0 notes below now describe what actually shipped.** Two entries overstated the release: the macOS install entry claimed shell-rc PATH wiring that is not in the installer, and the skills entry read as twelve new skills when eight new skill directories landed. Both are corrected in place below rather than left to mislead anyone reading the release history.

## [3.4.0] - 2026-08-20

### Added

- **Eight new built-in skills ship with every harness.** The Matt Pocock skill collection is imported for Claude, Codex, and Grok with harness-correct tool aliases and MIT provenance recorded: eight arrive as new `cas-` prefixed skill directories (writing-for-agents, diagnosing-bugs, domain-modeling, codebase-design, tdd, wizard, resolving-merge-conflicts, to-questionnaire) and four more are folded into existing builtins as reference material. (Corrected in 3.4.1; this entry originally said twelve new skills.)
- **`cas viktor key` completes Viktor setup with one pasted operator key.** The key is validated and stored machine-only with 0600 permissions — never in project state or environment files.
- **One `cas update` now brings every project fully current.** `cas update --all-projects` discovers every local project and runs the whole chain per project — schema migration, skill sync, cloud team-membership refresh, and cloud sync — with per-project receipts, continuable failures, and dry-run support. The contrib `cas-update` helper delegates to it.
- **Macs on Apple Silicon install with the standard one-liner.** `cas-install.sh` handles Darwin/aarch64 including Gatekeeper quarantine clearing and an Intel-Mac gate with a plain-language stop. When the install directory is not on `PATH` it prints the `export PATH=...` line to add; it does not edit any shell startup file. (Corrected in 3.4.1; this entry originally claimed `.zshenv` PATH wiring, which did not ship.)
- **Ambient recall now reads tool traffic and explains itself.** Trigger terms come from tool results and MCP queries (bounded and redacted), a strong-signal floor overrides the conversational precision gate, and every injection or silence carries a source-attributed decision trace.
- **Dropped work announces itself.** Merge-queue ejections and worker delivery stalls push durable, episode-keyed relays to the supervisor and worker instead of leaving tasks waiting silently.

### Changed

- **A code change reaches main in under five minutes.** PR admission checks collapsed to seconds, merge-queue validation runs the full suite once on fast self-hosted hardware with runtime-path-portable test archives, and stale queue runs are cancelled by behavior-tested watchdogs.
- **Release binaries are code-signed after stripping**, with a verification gate before packaging and a dispatch-only job for inspecting published artifacts' signatures.

### Fixed

- **Closed work stays closed.** Cloud pull can no longer silently resurrect terminal tasks: terminal status changes require an attributed reopen, and unattributed remote reopens park in the conflict journal exactly once.
- **Epic close is fast and unambiguous.** Closing a large epic commits first and responds with a compact receipt in about a second; timeout messages state whether the write landed.
- **Concurrent `cas-update` runs no longer corrupt a shared build.** The helper takes an atomic, holder-visible lock.
- **CI cannot skip Rust validation for markdown compiled into the binary.** Everything under `cas-cli/src/` is rust-affecting, enforced by a mutation-proof guard.

## [3.3.0] - 2026-08-19

### Fixed

- **Cloud sync no longer deadlocks on projects whose team bucket predates the git-remote identity contract.** Team registration now adopts the server-resolved canonical project id from the registration response, verifies it, and pins it so the same sync run pushes and pulls against the real bucket. Previously every sync aborted with a misleading "server-side defect" error (gabber-studio was down for two days; any legacy-slug project on a fresh checkout was affected).
- **`cas cloud project set` is authoritative again.** An explicit `[project] canonical_id` pin is no longer silently rewritten to the remote-derived form, and the later team-push adoption path never overrides an existing pin.
- **Registration failure messages now name the server-resolved canonical id** instead of wrongly blaming the server when identity resolution diverges.

## [3.2.0] - 2026-08-19

### Added

- **Factory workers now surface Viktor-originated questions to a live supervisor.** Incoming conversations are persisted and deduplicated, and remain visible for the next supervisor when no live session is available.
- **Cassy can use an alternate worker account without losing its isolated project context.** Each worker now resolves its own project history and hooks instead of inheriting another checkout's state.

### Changed

- **Merge-queue validation now uses the trusted self-hosted route where appropriate, while the required validation set stays intentionally small and explicit.**
- **Worker guidance is more concise and scannable, with evidence-first progress updates and clearer handoff expectations.**

### Fixed

- **Factory spawning and delivery are more reliable.** Workers retain refreshed local epic bases when publication is unavailable, reject unsafe branch-reference state, and handle Codex account, liveness, and terminal-limit conditions more accurately.
- **Task completion and test evidence are stricter and clearer.** Cassy prevents misleading green test receipts, preserves merge and review gates, and repairs the urgent-stop review path.
- **Viktor restart and archive handling now fail visibly and recover safely, reducing silent loss of pending replies and queued work.**

## [3.1.0] - 2026-08-18

### Added

- **Viktor conversations now work through a managed, two-way Cassy gateway.** When a project has no `.cas/proxy.toml`, `cas serve` refreshes a credential-reference-only Viktor upstream with an exact, fail-closed allowlist of nine conversation tools; an explicit project proxy configuration opts out. Run-starting calls are registered for daemon-owned follow-up, so Cassy delivers completed replies as inbound `origin=viktor` notifications instead of agents polling. `cas init` and `cas update --sync` install the `cas-viktor` skill for Claude, Codex, and Grok, and `cas viktor` reports credential-safe provisioning status.
- **Factory spawns can now select each worker's harness and account independently.** `spawn_workers` resolves per-worker `name`, CLI, model, effort, and `config_dir` overrides, validates the matching Claude or Codex account directory, and carries the resolved account into the spawned worker rather than flattening a mixed fleet to one supervisor profile.
- **Ben's Apple Silicon Mac setup guide is now part of the repository.** The guide covers the supported release-binary install, machine-wide Cloud login, project initialization, Commander service, source-checkout maintenance, recovery, and the current macOS process-restart limitation.

## [3.0.0] - 2026-08-18

### Added

- **`cas init` stops scaffolding your home directory by accident.** Run in `$HOME` (or at the filesystem root) it now names what it would create and asks before writing anything; non-interactive runs refuse outright and point at `--allow-non-project` for automation that means it. Project directories, including non-git ones, are unaffected.
- **The Codex account picker remains useful when there is only one account.** An interactive bare `cas codex` launch now offers that account and a `+ Log in a new account…` row, so adding a named account does not require a hidden command.

### Changed

- **CAS now presents itself as Cassy wherever people see it.** Commander, pairing email, installation and documentation copy, CLI help and banners, and Factory startup now use Cassy; commands, paths, environment variables, and code-level CAS names remain unchanged.
- **Factory runs keep the selected Codex account attached to the decision.** Availability checks and explicit `config_dir` preflight inspect that account's `CODEX_HOME`, and launch or attach output names the account home in use.
- **Factory worker guidance now asks for a concise, shaped response to the user.** The runtime prompt carries the response contract instead of leaving worker handoff prose implicit.
- **Signing in to the cloud is a once-per-machine act.** Credentials live in `~/.cas/cloud.json`, so `cas login` works from any directory and every project on the machine is signed in; `cas logout` signs all of them out.
- **Your team is picked up automatically — no setup command to discover.** When you are logged in and CAS can tell which team you are on, `cas cloud sync` scopes the project to that team and registers it, instead of syncing in personal scope until you happen to run a team command. It says which team it adopted and how to undo it; `cas cloud team auto off` keeps a project personal for good, `cas cloud team set` still pins a specific team, and if you belong to several teams with no default CAS asks you to pick rather than guessing.

### Fixed

- **An unavailable explicit Codex profile offers the next login step.** On an interactive terminal, `cas codex --profile <name>` now offers `cas codex login <name>` instead of leaving the account unusable without recovery guidance.
- **`cas claude --workers 0` no longer errors.** The zero-worker path now shares the normal account-selection flow.
- **A successful cloud sync now means your project really is connected to your team.** `cas cloud sync` confirms the project is registered with the active team before reporting success, registers it when it is missing, and stops with the actual reason — including the exact server exchange that failed — instead of printing green checkmarks over a project the team never received. Previously a machine with nothing queued to send registered nothing, so `cas cloud team-memories` answered "this project hasn't been synced to the team yet" right after a clean sync. That message now names the project, team, and endpoint involved instead of repeating the command that just ran.
- **`cas cloud team show` and `cas cloud team auto` agree on which team you are on.** Both resolve the team slug from your cached memberships, so a team set by UUID no longer displays as `<not resolved>` in one command while the other names it.
- **Browser login no longer produces a broken approval link,** and ordinary polling survives a rate limit instead of ending the login with a server-error message.
- **The Mac setup guide runs top to bottom as written.** Its steps are in an order that works on a clean machine, the commands it names exist, its shell block can be pasted twice without duplicating anything, and it names the cloud domain you sign in to.

## [2.72.0] - 2026-08-17

### Added

- **External expertise now crosses one enforced, receipted gateway.** `cas serve` replaces the proxy's compatibility default with an exact configured `(server, tool)` allowlist at boot and reload, denies every external call when that list is empty, and refuses paid verification routes outside the registered-supervisor gateway. The first production flow reserves a durable budget receipt before `ask_viktor`, resumes timed-out runs by their stored run ID, and returns only the fail-closed external-production-verification verdict; configuration and route/budget defaults are documented in `crates/cas-mcp-proxy/README.md`.

### Changed

- **Cloud synchronization retains an auditable outcome.** Terminal task updates are guarded before they can regress, pull provenance and sync receipts surface the applied result, permanent push rejections are parked with concise errors, and canonical task identifiers remain normalized through the full sync path.
- **Repository and release references now point at `Richards-LLC/cassy`.** Install, update, Homebrew, release, and API links follow the canonical repository home.
- **Commander and CLI guidance better match live behavior.** Hosted Commander health checks admit the supported origin, reachable hubs clearly guide users through re-pairing, and command help exposes cloud operations while keeping internal maintenance tools out of the public surface.
- **Supported Rust is now 1.88.** The declared MSRV and all workspace package requirements advance from 1.85 to 1.88.
- **README documentation now explains the knowledge system.**

### Fixed

- **Factory workers see stale output instructions before acting on them.** Spawn briefs name each task's resolved durable artifact directory and warn when task prose prescribes an absolute or home-relative path outside the worktree or sanctioned artifact root.
- **Release and test operations recover more predictably.** The repository includes an Actions-outage release fallback runbook, and PTY tests tolerate loaded runners without flaking.

## [2.71.0] - 2026-08-16

### Added

- **Groundwork: the complete contract for governing external expertise.** This release lands the Viktor delegation gateway as library surface with its enforcement seams in place — a registered-caller policy hook in the MCP proxy, exact parsed (server, tool) allowlist policy, a delegation receipt store with duplicate-call protection, budget reservations, and timed-out-run resumption (migration 236), and a fail-closed verdict contract under which a verifier that could not answer — timeout, malformed output, insufficient scope, transport failure, or any other enumerated non-answer — records a durable non-pass and never reads as approval. **None of it is enforced yet:** no production path installs the policy or writes receipts in this release (the proxy's default policy remains allow-all), so the supervisor-only provider key remains the operative control until the production wiring ships.

### Changed

- **Message status tells the truth.** Activity that merely suggests a recipient saw a message shows as its own weaker state instead of "confirmed", a message repeatedly blocked by a busy recipient is flagged undelivered instead of silently waiting, and only an explicit acknowledgment of the exact message discharges an urgent halt.
- **One authority decides which branch work belongs on.** Worker spawn bases, merge destinations, and newly created or newly linked epic children all resolve through the same declared-work-target precedence chain; an epic branch that has cleanly fallen behind its parent is fast-forwarded before any worker is cut from it.
- **Finished work is recognized as finished.** Epic close reconciles deliveries that were squash-merged and later improved, measures against live branch state instead of stored counts, and keeps unproven anchors fail-closed.
- **Memory keeps instructions, not chatter.** Machine-to-machine relay turns are no longer captured as durable context, while genuine operator instructions are captured again — discriminated by typed delivery provenance instead of text parsing.

### Fixed

- **Commander is finished work on both surfaces.** A polish pass and a UX pass fixed the unreachable phone message button, empty worker panes, the drawer crushing the terminal, dead connection colours, the keyboard closing mid-word, drafts destroyed by the live refresh, sends without feedback, vanishing confirmations, unexplained disabled controls, duplicate alert cards, stale data posing as live, a dead-end first run, and alert cards missing their ticket.
- **Sharp edges removed.** Ending a session no longer risks a nested-runtime panic; requesting an isolated worktree no longer gets a false refusal with invalid TOML instructions; the supervisor checklist no longer instructs an action that severs its own tools; a leak test no longer scatters orphan processes through CI cleanup; and memory list filters (tags, tier, scope) actually filter, with counts that match the rows.

## [2.70.0] - 2026-08-15

### Added

- **Commander opens a session in kilobytes instead of megabytes.** Attaching sends an authoritative terminal keyframe built from current pane state and then streams live updates, so the first screen arrives in roughly 17 KB where it previously required about 44.7 MB, and history is fetched only when scrolled into view.
- **Alerts are triaged instead of listed.** Attention cards are grouped by session, ranked critical, warning or info, and repeated failures collapse into a single card with a count that can be dismissed as a group.
- **The pane you watch gets the room it deserves.** The supervisor pane is dominant by default, panes can be promoted and reordered, and the chosen layout is remembered.
- **Optional AI enrichment can label session cards and attention events.** It ships default off, applies redaction inside the provider so callers cannot bypass it, and honors a deterministic severity floor that enrichment may raise but never lower.

### Changed

- **A failing connection explains itself.** Commander reports an explicit staged lifecycle with per-stage deadlines, jittered backoff, heartbeat latency and authenticated diagnosis, and distinguishes an expired credential from a revoked one, instead of showing an indefinite "Connecting" state.
- **Reporting a viewport is now part of observing a pane, not controlling it.** A read-only viewer can size its own terminal, while a leased pane follows its controller, so an observer can no longer reflow a controller's screen.
- **Browser pairing can request control when it is needed.** Control is no longer fixed at pairing time, and anything still unavailable states why.

### Fixed

- **Terminals render at the size of the pane showing them.** Panes no longer arrive as mangled, mid-word-wrapped text, and a replayed byte tail can no longer begin mid-escape or omit terminal modes.
- **Stopping a registered server is verified rather than assumed.** A stop no longer reports success while a wrapped child process survives it.
- **Release publication reflects the bytes users actually download.** Local audit archives stay local until publishing is explicit, and announced digests come from the published release.
- **Integration and recovery handle real branch state.** Clean epic branches advance on integration, clean-behind receipt closes are unblocked, numeric stash recovery references are disambiguated, and relays no longer pollute session context.

## [2.69.1] - 2026-08-14

### Fixed

- **Commander page-initiated pairing now finishes in browsers that require `fetch` to retain its `Window` receiver.** Every pairing handoff binds the browser fetch function before relay creation, polling, acknowledgement, or credential exchange.

## [2.69.0] - 2026-08-14

### Added

- **Every CAS harness now receives the MCP integration runbook.** `cas update --sync` distributes the same installation and diagnosis guidance to Claude, Codex, and Grok.

### Fixed

- **Commander page-initiated machine pairing now completes.** The pairing handoff sends the hub's canonical origin, so the relay accepts the invitation instead of rejecting it at delivery.
- **A local pairing precondition failure no longer consumes the one-time code.** CAS checks the local hub before claiming and retains the same-machine nonce for a safe retry.

## [2.68.1] - 2026-08-14

### Fixed

- **The v2.68 delivery and recovery wave is now installable on Linux x86_64.** Release builds use the compiler-specific portable baseline, rebuild native dependencies from source before auditing, and fail immediately when a declared native target cannot be produced on the current host.
- **Factory startup and recall keep using the information that is current and relevant.** Base selection consistently prefers the fresh remote-tracking ref, and mid-session recall prioritizes the current request over an overlong task title.
- **Validation diagnostics remain accurate under edge cases.** Scoped-proof validation recognizes nested integration modules, and the unknown-tool MCP test no longer claims an unproven server-side mechanism when its historical timeout cannot be reproduced from retained evidence.

## [2.68.0] - 2026-08-14

### Added

- **Workers can flag a same-session peer collision without leaving the supervisor blind.** Peer warnings stay scoped to the active factory session and include a supervisor copy, while task notes can now be read directly without loading the full task record.

### Changed

- **Delivery and merge decisions now prove the work's content is on its declared target.** Freshness, merge relays, close receipts, and diff attribution follow the actual destination and distinguish present work from commits that were rebased, resolved away, or superseded.
- **Review and recovery state now reflect the real owner and live task state.** Value-only edits retain normal supervisor review, verification recovery names the available escape hatch, and terminal relay backlogs reconcile automatically.

### Fixed

- **Lifecycle instructions no longer turn stale or uncertain state into a misleading action.** Replayed prompts carry provenance, terminal assignments are withheld only with positive current evidence, declined merge anchors cannot reappear as live requests, and urgent stops expire with their acknowledged exchange.
- **Workers start and operate in the correct context more reliably.** Spawned work uses a fresher non-divergent epic base, workers receive queued supervisor corrections before starting, the target checkout is protected from foreign Git writes, and missing local prerequisites are made explicit.
- **Focused recall and memory saving are more dependable under real workloads.** Search opens a consistent schema under concurrency and overlap scoring measures meaningful content similarity rather than shared note structure.

## [2.67.0] - 2026-08-14

### Added

- **Task artifacts are now searchable with their work attached.** Bounded Markdown, text, and JSON deliverables enter the shared search index at close and during reindex, while oversized or unsupported files remain safely excluded.
- **Factory worktrees now surface branch-local prerequisites before work begins.** New checkouts provision the pinned Zig toolchain when available and give lockfile-aware Node installation guidance without sharing path-sensitive dependencies.

### Changed

- **Factory delivery follows the task's declared target branch end to end.** Epic bases, freshness checks, merge relays, and landing status now resolve against the real destination instead of assuming `main` or counting unrelated commits.
- **Task-focused recall and core maintenance contracts are more precise.** Relevant saved guidance remains competitive across search fallbacks, workspace dependency and lint policy is centralized, CLI backends share one typed interface, and lifecycle gate failures retain structured meaning internally.

### Fixed

- **No-code and supervisor-verified work can finish without close-gate deadlocks.** Portable evidence survives parked states, valid updates are no longer discarded alongside one rejected field, stale anchors can be cleared safely, and missing verification dispatches have a bounded recovery path.
- **Lifecycle notifications now identify and acknowledge the event they actually represent.** Wake relays reach the acknowledgement bridge, supervisor-owned gates do not masquerade as worker events, stale completion prompts are revalidated, and instructions use the receiving harness's live tool namespace.
- **Search, startup, and delivery edge cases fail safely instead of losing context.** Unknown colon-bearing terms search literally, custom-profile and Grok supervisors receive startup context, spawn-time corrections arrive before assigned work starts, and squash-landed or non-main-target work is reconciled by content.
- **Harness and checkout setup reports its real capabilities.** Codex context reset names the supported restart path, supervisor launch parity is guarded across harnesses, and worker binding checks reject stale or sibling checkouts before execution.

## [2.66.0] - 2026-08-13

### Added

- **Commander can now begin machine pairing from the page.** A short-lived code lets the target machine authorize the requesting Commander session, with strict controller, relay, and loopback origin boundaries throughout the exchange.
- **Cross-project work can be proposed and followed without losing ownership.** Proposals carry explicit source and target projects, support auditable acceptance or rejection, and keep local dependent tasks blocked until the external work is resolved.

### Changed

- **Proposal synchronization now converges across retries, pagination, and reopen cycles.** Creation is idempotent, replayed feed rows are deduplicated, provenance remains authoritative, and external dependency state follows resolution transitions without duplicating local work.

### Fixed

- **Factory workers now fail closed before starting in the wrong checkout.** Spawn preparation proves the worker's exact worktree and branch, pre-harness validation rejects drift, commit guards deny sibling branches, and binding diagnostics inspect the assigned checkout.
- **Commander pairing handles cancellation, replacement, and cleanup races safely.** Aborted or failed exchanges roll back only their own state, replacement rotates live credentials, stale cleanup cannot erase a newer pairing, and incomplete browser fragments are scrubbed before startup.
- **Coordination and operator surfaces retain truthful state under edge cases.** Supervisor roles survive registration, stale reminders stay quarantined, long code snippets truncate on UTF-8 boundaries, stale-skill warnings render cleanly, and tag CI handles an all-zero base SHA.

## [2.65.0] - 2026-08-13

### Added

- **Silent coordination failures now become visible, recoverable outcomes.** Aged unread messages return a one-shot notice to their sender, stalled sessions escalate durable attention signals, and status output includes recent progress timestamps.
- **Concurrent planning now warns before work is duplicated.** Session startup surfaces simultaneous planning activity, sibling titles are checked without collapsing meaningful distinctions, and duplicate plans are identified early.

### Changed

- **Startup and launch checks fail earlier with actionable context.** Configuration directories are validated before launch, registration failures preserve the relevant terminal tail and reap abandoned processes, and MCP startup applies pending schema migrations only after arming its parent-death watchdog.
- **Close verification handles real delivery shapes without weakening proof.** No-code work can close with portable evidence, target-branch and squash receipts retain a non-empty lint range, merge receipts receive useful correction hints, and merged delivery facts remain immutable.

### Fixed

- **Dead or stale sessions no longer leave work looking active.** Held work is returned to a recoverable state with an audit trail, and epic status marks stale ownership instead of presenting it as live progress.
- **Claude session progress and interrupts are now observable.** Transcript turn watermarks feed stall detection, explicit interrupts report confirmed delivery or a clear failure, and status reports distinguish recent output from recent file changes.
- **Delivery-stall thresholds and bounce eligibility are fail-safe.** Oversized thresholds return a clean error instead of panicking or wrapping, while broadcasts, synthetic traffic, stale rows, cross-session senders, and prior watchdog notices cannot create false bounces.

## [2.64.0] - 2026-08-12

### Added

- **Supervisors can make decisions with the context that matters.** Creating work now surfaces related prior recall, and explicit decision gates make consequential choices visible before work proceeds.

### Changed

- **Release-only changes validate faster.** Workspace version bumps can take the focused required-check path while preserving the heavier validation tier for product changes.
- **Task handoffs now stay current through delivery.** Merge relays refresh the target tip and fetch remote receipts before reporting an outcome.

### Fixed

- **Expired memory and session reminders now respect their intended boundaries.** Valid context survives recall, expired entries stay out, and reminder lifecycle actions remain scoped to the session that created them.
- **Terminal work states and activity reporting are more trustworthy.** Cancelled and superseded work follows a fail-closed lifecycle, and dirty worktrees still report their real activity floor.
- **Automation recovery is clearer and safer.** Negative-result closures retain their evidence, CI red-run receipts are preserved, socket ownership elects one daemon safely, and session end snapshots the current state.

## [2.63.0] - 2026-08-11

### Added

- **Every Codex install now carries the CAS safety hooks pre-trusted.** Provisioning registers hook trust and project trust through a single locked configuration transaction, verified before any agent launches, so agents start working immediately without interactive trust prompts.

### Changed

- **Session startup is leaner and more complete.** The always-injected skill descriptions shrank by two-thirds, team-spawned sessions now receive the same project-memory bundle as direct launches, and retrieval outcomes feed back into recall scoring.
- **Generated hook configuration is byte-stable.** Regenerating `hooks.json` over an unchanged setup produces a byte-identical file, ending spurious git churn.

### Fixed

- **Messages to idle Codex agents now reliably surface a turn.** Prompt delivery is classified and watched end-to-end; an unsurfaced delivery wakes the agent instead of sitting unread indefinitely.
- **Agent cleanup fully tears down what it removes.** Stale-agent maintenance routes through forced shutdown and waits on the terminal process, ending ghost panes and zombie processes after reaps.
- **Concurrent Codex launches no longer race trust registration.** The pre-launch trust write is a verified happens-before of the agent process start; launch refuses rather than parking on an interactive prompt.
- **CI survives build-cache outages.** Cache-service failures at setup or teardown downgrade to an uncached build with a loud warning instead of failing the run.

## [2.62.0] - 2026-08-11

### Added

- **CAS now includes cross-harness data-visualization guidance.** The built-in skill and its quality checks make it easier to turn repository data into readable, reviewable visual artifacts.

### Changed

- **Pull-request validation is faster while preserving the release gate.** A focused warm suite now protects merge requests, while heavier checks remain available on main, schedules, and manual dispatch.
- **Workers and release flows now give clearer, more reliable handoffs.** The checked-in guidance covers the supported task surfaces, artifact evidence, and protected-main release sequence.

### Fixed

- **Hub restarts now complete with live viewers attached.** Existing viewer connections drain within a bounded window, then CAS safely closes any remaining stale connections before the replacement hub starts.
- **Runtime coordination and cloud sync report actionable truth more consistently.** Stale CI failures no longer trigger misleading alerts, weak ambient matches cannot dominate recall, supervisor memory writes use the intended gate, and rejected cloud records retain itemized reasons.

## [2.61.1] - 2026-08-10

### Fixed

- **Hub upgrades now recover Tailscale Serve mappings created by v2.60.0.** Legacy ownership receipts load without the newer diagnostic executable field, so CAS can tear down its exact stale mapping and republish the upgraded hub instead of leaving HTTPS unavailable.

## [2.61.0] - 2026-08-10

### Added

- **Commander hubs can now persist as managed services.** `cas hub service install`, `status`, and `uninstall` provide launchd and systemd integration for durable fleet control.
- **The hosted static Commander origin is explicitly supported.** `https://hub.petrastella.io` is documented as an opt-in trust boundary, and the controller visibly identifies incompatible hub capabilities.

### Changed

- **Hub restart recovers the public Tailscale Serve endpoint on macOS.** CAS discovers the signed Tailscale app-bundle CLI when it is not on `PATH`, preserving the normal start/restart recovery path.

### Fixed

- **Hub stop receipts now report the final Tailscale Serve outcome truthfully.** A mapping removed by the foreground hub during shutdown is recognized as removed rather than reported as untouched.
- **Scoped CI validation now reads ANSI-coloured test summaries correctly.** Matching test failures continue to be reported instead of being obscured by terminal formatting.

## [2.60.0] - 2026-08-10

### Added

- **Failed factory CI runs now reach the supervisor automatically.** The daemon watches completed runs for main and active factory lanes, relays one actionable failure per branch and commit, and includes the run, failing job, and first failing test when available.
- **Merge receipts now report the source lane's latest CI verdict.** A completed red run is surfaced with its URL before a merge decision; unavailable or still-running CI is called out honestly as unknown without changing merge semantics.
- **Cloud sync rejections now explain which records need attention.** Personal and team push failures retain itemized reasons, including partial server-side rejection, so accepted work can continue while the rejected items remain actionable.

### Changed

- **Factory coordination exposes more reliable liveness and recovery signals.** Preassigned workers retain live holders, background process activity is observed across threads, close-gate rejections explain their cause, and a worker wakes only when both delivery and inactivity evidence permit it.
- **Routine checks are more precise and economical.** Scoped test filters explain regex-like input, hook wire captures are audited, migration registry IDs are guarded, workspace write checks recognize valid targets, and CI/test fixtures are isolated consistently.

### Fixed

- **Destructive worker shutdown and merge handling now stay bound to the intended work.** Shutdown targets are validated safely, task-bound delivery merges retain their correct task identity, and merge-conflict status is limited to the affected contribution.
- **Factory startup, test, and interface behavior now tell a truer story.** Startup pulls do not requeue work, panic isolation runs under the intended test profile, ambient recall filters weak matches, and the factory strip consistently shows the running version.

## [2.59.0] - 2026-08-10

### Added

- **Claude users can select and sign into separate accounts directly from CAS.** Bare launch now offers a profile picker, each profile keeps its credentials isolated, and `cas claude login <profile>` makes switching accounts explicit.
- **Cloud queue recovery now has an explicit retry command.** `cas cloud queue --retry` lets operators re-attempt failed queued work without guessing at its state.

### Changed

- **Factory test gates use faster, more targeted defaults.** Scoped nextest runs and shared compiler cache use reduce routine feedback time while retaining the full release checks.

### Fixed

- **Pending cloud work and workspace checks now report and recover more reliably.** Silent pending work is surfaced, failed rows can be retried, and the workspace guard no longer rejects valid Bash write targets.
- **CI fixtures are isolated consistently.** Test runs no longer inherit machine-specific state that can make a healthy change look broken.

## [2.58.0] - 2026-08-10

### Added

- **CAS now carries its own built-in CLI routing guidance.** Common command-line work can reach the right product guidance without relying on a separate external skill setup.
- **Workers now checkpoint before compaction and can retain a sync-conflict journal.** A constrained turn leaves a usable handoff, while a conflicted sync keeps enough history to explain and reconcile the result.

### Changed

- **Factory work now follows a clearer workspace contract.** Writes are constrained to sanctioned roots, durable task artifacts are collected safely, and close evidence cannot point into transient tmpfs paths.
- **Release inputs now fail fast before artifact builds begin.** The release workflow validates the annotated tag, exact version train, changelog, clean inputs, and locked dependency graph before it spends time building platform artifacts.
- **Legacy session and model-effort context now remain attached to the work that needs them.** Daemonized sessions retain their source session through muxing, and queued work preserves the selected model effort.

### Fixed

- **Memory sync no longer drops a daily entry when remote IDs collide.** Collisions are skipped safely instead of silently replacing local history.
- **Verification storage repairs its required schema at open time.** Existing installations converge before verification state is used.
- **Rejected supervisor reviews return through the sanctioned amendment path.** Review state and epic close reporting now agree on the intended target.
- **Cloud sync now exposes intentionally skipped queue work.** Operators can distinguish a retained diagnostic from an unexplained missing update.
- **The factory test and workspace suite is portable across the supported macOS environment.** PTY aliases, hub fixtures, watcher behavior, history cleanup, migration projections, and reminder references now remain stable without Linux-only assumptions.

## [2.57.0] - 2026-08-09

### Added

- **Memory can now consolidate an overlapping entry in one explicit, safe operation.** The opt-in merge returns the surviving identity and receipt, while concurrent edits are detected rather than silently overwritten.
- **Memory recency is now deterministic and self-describing.** Recent results state their ordering and use a stable tie-break; lifecycle guidance explains when to merge, archive, or expire a durable memory instead of creating parallel records.

### Changed

- **A full cloud sync now deliberately re-reads prior history when requested.** `cas cloud sync --full` resets the pull watermark and empty-result streak so recovery starts from a known clean scope.
- **Cloud sync now explains healthy no-op pulls and the active sync state.** Pull output distinguishes up-to-date, personal-only, and fetched work; status names the active team or personal scope, daemon liveness, queue health, and the last successful pull.
- **Factory halt responses now lead with a bounded, actionable exit brief.** Operators get the essential stop context without spending the remaining turn budget on repeated coordination detail.

### Fixed

- **Network probe latency remains an observation, not a one-sample release gate.** Probe conformance now preserves useful p95 telemetry without treating an isolated sample as a verdict.
- **Cloud pull failures now identify the malformed entity that could not be decoded.** Recovery messages name the affected record instead of leaving a generic parse error.
- **Knowledge and team sync now converge more reliably.** Missing knowledge attribution schema is repaired safely, team pulls retain required skills, and the pull-url guard ignores inline test scaffolding while continuing to detect production callers.
- **Supervisor review and amendment flows retain their correct delivery boundary.** Review dispatch binds atomically to the delivery it approves, legacy verification proves the intended repository state, and an amendment stays pinned to its original work.
- **Memory recall returns cleaner, more useful context.** Lexical fallback no longer lets same-session echoes or low-value ambient matches crowd out relevant context.
- **Factory presence and control surfaces now report a truer state.** Held workers stay quiet, duplicate roster entries reconcile to one identity, and repeated startup no longer creates a second visible worker.
- **Reminder guidance now makes bounded waiting explicit.** Long-running operations preserve a reachable session instead of silently occupying the worker pane.

## [2.56.0] - 2026-08-09

### Added

- **Cloud sync now exposes configurable queue-health warnings before work is stranded.** `cloud.queue_pending_warning` and `cloud.queue_oldest_warning_secs` make factory preflight report a growing or aging sync backlog while preserving safe defaults.

### Changed

- **An active CAS daemon now drains cloud work on its regular cadence even when no new activity arrives.** A queued update therefore continues toward the cloud after the event that created it, rather than waiting for a later local action.

### Fixed

- **Cloud deletions now preserve current local truth and record failed deletes for retry.** A stale tombstone is neutralized when its task or entry exists locally again; successful delete routes, already-absent remote rows, personal/team queues, and skipped upserts now converge consistently instead of silently losing or retaining work.

## [2.55.5] - 2026-08-09

### Fixed

- **Commander pairing now returns the exact authorized CORS origin when a bound cross-machine pairing exchange is refused.** A controller-origin browser can read both the successful credential and a generic refusal for its own pairing capability, while unbound, mismatched-origin, or otherwise invalid exchanges remain fail-closed without exposing an allow-origin header.

## [2.55.4] - 2026-08-09

### Fixed

- **Commander hub restart now waits for an authoritative machine-lock handoff before starting the replacement.** Restart propagates stop failures, waits for both the old process and its lock ownership to disappear, and acquires the machine lock before stale-state cleanup or replacement launch. If the bounded handoff deadline expires, the command fails truthfully without starting a competing hub; concurrent start and restart attempts preserve exactly one owner.

## [2.55.3] - 2026-08-09

### Fixed

- **Commander HTTPS origins now instruct browsers to stay on HTTPS for one year.** Responses reached through CAS's verified Tailscale Serve TLS path emit exactly `Strict-Transport-Security: max-age=31536000`, while the documented plaintext loopback listener cannot opt into HSTS through spoofed proxy or identity headers. The policy is bound to a separate server-owned proxy backend and preserves existing CSP, referrer, content-type, frame, authentication, and CORS behavior across successful and error responses.

## [2.55.2] - 2026-08-09

### Fixed

- **Commander now starts securely on a clean installed machine without a manual initialization step.** The hub creates a missing `~/.cas/hub` hierarchy with owner-only permissions for both ordinary and Tailscale Serve startup, preserves existing safe state, and rejects symlinks, non-directories, unsafe final modes, wrong ownership, and unwritable ancestors without exposing filesystem paths.
- **A real daemon `SIGILL` now reaches Commander as evidence-backed `SIGILL`, not `unknown`.** Spawned daemons record an owner-only exit receipt bound to the exact session, PID, and process-start fingerprint; the live hub consumes only an exact match after disconnect, rejects stale PID epochs, distinguishes a still-live transport loss, and leaves absent or malformed evidence honestly unknown. The replacement guidance therefore identifies portable-release remediation only when the operating system actually reported `SIGILL`.

## [2.55.1] - 2026-08-09

### Fixed

- **Do not install the Linux `2.55.0` artifact; use `2.55.1` instead.** The `2.55.0` workflow checked an intermediate Ghostty archive while the final linked executable still contained runtime-dispatched AVX-512 assembly from AWS-LC and BLAKE3. CAS already selects ring as its process-wide TLS provider, so the unused AWS-LC provider is no longer compiled into Linux releases; this intentionally omits AWS-LC's post-quantum-capable paths, which CAS did not exercise. BLAKE3 keeps its portable, SSE, SSE4.1, and AVX2 paths, while an audited 1.8.6 build override makes its upstream runtime-only `no_avx512` feature omit the inactive AVX-512 archive entirely. Explicit portable Rust, C, C++, and Zig targets cover the remaining final-link contributors. Hashes and stored fingerprints are unchanged; AVX-512-capable hosts may see lower throughput only in BLAKE3-heavy indexing and fingerprinting. The strict ISA scanner remains unchanged; the workflow now also checks the locked release features and audits the exact staged executable before it can be uploaded. The immutable `2.55.0` tag, release, and assets remain unchanged for traceability.

## [2.55.0] - 2026-08-09

### Added

- **Commander provides one phone-friendly view across paired CAS machines.** Each machine can run a durable local hub, expose it through an explicitly managed Tailscale Serve route, and contribute its live sessions and terminal panes to a controller-origin catalog without creating another runtime session or model request.
- **Live terminal viewing and control now have an explicit concurrency model.** Multiple observers share one bounded upstream connection per daemon session, one identified controller holds input at a time, slow viewers are isolated, and the embedded offline client supports pane selection, resize, targeted interrupt, and attributed messages through additive protocol negotiation.
- **Browser control is bound to the paired device, origin, operation, and short-lived proof.** Non-extractable device keys, DPoP request binding, exact Origin/CORS handling, one-use pairing and WebSocket credentials, scoped authorization, revocation, controller leases, and attributed audit all fail closed; non-loopback plaintext service is refused.

## [2.54.1] - 2026-08-09

### Fixed

- **The Linux x86_64 release no longer inherits AVX-512 from the build runner.** Ghostty VT now receives an explicit portable Zig target for every supported native and cross build, unknown targets fail closed instead of falling back to the host CPU, and the release path audits the bundled Ghostty archive for forbidden EVEX/AVX-512 instructions. Anyone who installed `2.54.0` should upgrade to `2.54.1`; the original `2.54.0` tag and artifacts remain unchanged for traceability.

## [2.54.0] - 2026-08-09

### Added

- **Relevant project context now arrives automatically at the start of a turn.** CAS creates one bounded query vector and searches knowledge, code history, and the current source index together, then presents only the best role-relevant matches. The path is on by default for authenticated installs, has explicit latency and corpus limits, falls back safely when semantic search is unavailable, and does not turn prompts into stored memory.
- **The live source tree is now a first-class semantic search corpus.** Code files are reconciled automatically, embedded through their own queue and cache, and retired from every index when deleted. Exact-symbol history queries now prioritize the commit that actually touched the requested symbol instead of merely mentioning the same text.
- **"Is this fixed?" can now be answered against the binaries that actually ran.** CAS records executable epochs for its background processes and separates pre-fix, mixed-version, and clean post-fix evidence. Verdicts always include the observed sample size and say when the post-fix window is too small or has not begun, rather than returning an unsupported bare "fixed".
- **The developer updater is now tracked, installable, and safe around running CAS processes.** `contrib/shell-helpers/install.sh` installs `cas-update`; plain `cas-update` builds, installs, migrates, syncs, and turns over only processes whose executable bytes and process-start fingerprint match the replaced binary. `--no-restart`, `--build-only`, `--sync-only`, and `--dry-run` provide explicit narrower modes.

### Fixed

- **Cloud knowledge sync now preserves ownership and deletion truth.** Personal pushes are incremental and carry their repository identity, team pulls and pushes stay within the active team, foreign pages are rejected at ingest, and tombstones propagate deletions instead of allowing removed pages to return.
- **Migration discovery can no longer skip a lower gap or trust a false ledger row forever.** Detection stops at the first missing migration, safe additive migrations recorded without their actual schema effect are reconciled with an audit trail, and the release path automatically runs component-output snapshots whenever the migration registry changes.
- **History and source indexes no longer publish partially reconciled state.** Watcher, vector, and deletion races are closed; doctor reports missing or stale history tables instead of treating them as an empty repository; lag continues to age honestly; and provenance coverage remains visible even on warning paths.

## [2.53.0] - 2026-08-08

### Added

- **CAS can now search the history of your code, not just its current state.** Every commit in the repository is indexed — subject, body, the files it touched and, where the symbol index has data, the functions and types whose lines it changed — and that index keeps itself current in the background rather than needing a command typed at it. You can ask what a query returns across that history from the command line or through the tool surface, and the same history is now a full-standing channel in the blended search everything else already uses, so asking a question about the codebase can be answered by what was done to it and why, not only by what the files say today. Files that keep changing together are reported alongside a result, which is the fastest way to find the second place a change always has to land.
- **A commit can now say which piece of work and which session produced it.** Resolving that link previously depended on a table that had been empty for its entire existence; it is now populated, and each link records both how it was established and how much confidence that method earns, so a reconstructed association is never presented as an observed one. Coverage is reported honestly rather than assumed.
- **Issues, pull requests, their comments and past release notes are indexed alongside the commits.** The discussion around a change is usually where the reason for it lives, so the searchable corpus now covers the written record as well as the diff.
- **The symbol index actually runs.** The tree-sitter index of functions, types and methods had never produced a single row on any installation: the command the tool told you to run did not exist, and the background job that was supposed to do it was gated on the machine being idle — which a working machine never is. The command exists, the index is built and kept fresh automatically, and it is on by default. Quiet moments are still preferred, but politeness can now only defer the work, never cancel it: once the index has gone five minutes without a refresh it is rebuilt regardless of load, and says that it did.
- **Search vectors are computed automatically instead of waiting for someone to remember.** Embeddings were produced in exactly one place — inside a manual sync command — so whether your knowledge was searchable by meaning depended on whether a human had recently typed something. A backlog of over a hundred pages had duly sat unembedded. A logged-in install now drains its own queue in the background and converges to zero pending, across both the knowledge corpus and the new history index.
- **"Is this bug fixed?" is now answered against the software that was actually running, not the date a fix was tagged.** A fix does not start working when it is released; it starts working when the processes serving it restart, and older processes routinely keep running for a further half hour. Anything observed in that overlap comes from both versions at once and proves nothing about either — reading it as evidence of a fix is a real mistake this project made and had to withdraw. CAS now records, for every background process it starts, which binary it is running and how long it was seen alive, and reconstructs that timeline for processes that ran before this landed. A question about a symptom is answered in three parts: the window before the fix ran, the ambiguous overlap, and the clean window after the last old process finally stopped — with the overlap excluded from the verdict by rule rather than by convention. The answer is never a bare "fixed": when the clean window is too small to support the claim it says so and reports how much evidence it actually has, and when no process has yet been seen running the fixed build it says that instead. Replayed against the incident that motivated it, the boundary it derives from live records matches the one that had to be established by hand.

### Fixed

- **A worker running under a second account now receives its messages.** When a session is started against a configuration directory other than the default, its harness reads mail from a mailbox inside that directory — and every routine delivery was being written to the sending daemon's own directory instead, where nothing reads. Such a worker booted deaf: only a forced interruption could reach it, and everything else sat unread forever. Messages are now written into the recipient's own tree, with the roster it needs to make sense of them, while single-account installs are untouched.
- **A session sitting idle with unread mail is now woken to read it.** Thirty-four of thirty-five wake attempts across an entire fleet were declined, every pass, and the only wake that ever landed was a hand-forced interruption. The cause was not any signal from the session: the search for a session's transcript looked in one hardcoded location, so on any machine using a second account it found nothing for every session, and "no transcript" was being read as "busy, do not disturb". Neighbouring checks had always read the same absence as "not busy", which is why one command cheerfully reported a session as available while another refused to wake it. Transcripts are now resolved across every known configuration directory, and an unknown state is no longer allowed to masquerade as a definite one.
- **A message you were interrupted to read no longer comes back.** The path that breaks into a session with an urgent message recorded the delivery in the sender's ledger but never in the per-recipient one the recipient's own unread check reads, so a message that had been delivered, read and acted upon was still eligible to be served again. Every terminal delivery path now writes the receipt, keyed by the name the recipient actually answers to rather than the pane it happened to be typed into.
- **Embedding a large batch no longer fails silently and permanently.** Requests were sent with every pending item in one call against a server that hard-caps them at thirty-two, so any backlog above that size produced a permanent rejection — and the rejection was logged as a warning and otherwise discarded, making the visible result "nothing was embedded", forever, with no error anywhere. Requests are now split at the real limit, and an oversized one is refused before it is sent rather than after.

## [2.52.0] - 2026-08-08

### Fixed

- **Almost every notification about a piece of work changing hands was being destroyed before it could be sent.** 353 of 361 supervisor relays over four days never reached transport — including 34 of 36 "this is ready for you to merge" and 34 of 36 "this close was rejected" notices. The cause was a freshness check that compared the notification's timestamp against the task's timestamp for exact equality, while the two values were read from the clock at two different moments; the test could never pass, and every one of the 397 discarded notices came from that single line. Measured across the discarded rows: zero exact matches, and one missed by 21.9 microseconds. Freshness is now the question it was always meant to be — is the work still in the state this notice announces — which no clock skew can defeat, while the one thing a timestamp can decide soundly is kept. The worst case is a duplicate courtesy notice when a task re-enters the same state; the previous worst case was total silence. Separately, notices withdrawn because their premise expired are no longer filed under the same label as routine de-duplication: a four-day outage sat hidden inside a bucket that reads as normal housekeeping, so a withdrawal now says a decision was made and records which work moved on. (GH #167)
- **A message you had already read, acted on and replied to no longer comes back.** Whole bursts were being re-served. The suspected cause was a second copy of the message; the live records say the duplicate was in the bookkeeping, not the message. "Read" was determined from a per-recipient receipt ledger that only two of the delivery paths ever wrote to — the path that actually hands a message to a session wrote none — so a message could be reported delivered and be simultaneously unread by the recipient's own check, which then handed it back. Every path that declares a message terminally delivered now records the receipt, which matters most for broadcasts, where that ledger is the only thing that can ever retire one. Delivery still does not count as acknowledgement; nothing about what a real reply proves has changed. (GH #176)
- **A supervisor's mail stopped hiding from the supervisor.** A supervisor answers to two names — its own and the generic role everyone addresses it by — and the two readers of the receipt ledger had drifted apart on which names to resolve. A message sent to the role name was unreachable from the supervisor's own inbox check: 40 of 50 such messages were never receipted, against 15 of 59 for the personal name. A message retired under one name also kept no record under the other, so the reader that missed it surfaced it again on a later turn. Both readers now share one identity resolver and a receipt is written for every name the recipient answers to. (GH #176)
- **Sending a message no longer leaves an unreadable copy accumulating in a second place.** When a session runs against a non-default configuration directory, the underlying tool wrote its own copy of every outgoing message into a mailbox tree nothing ever reads and nothing ever prunes. It grew without bound and then arrived as a stale burst the moment any similarly named session started there. Those strays are now marked inert as they appear, and only in trees that were conjured by that write — a real mailbox is never touched. (GH #176)
- **One stuck message no longer floods the log with tens of thousands of identical lines.** A single message wrote 16,604 lines in thirty flat minutes — 12.5% of everything logged that day — because the announcement was emitted before the check that decides whether to actually retry, so the check correctly declined and the line printed anyway, once per hundred-millisecond poll. The line now rides the retry itself and names the attempt number, so volume tracks the deliberate retry budget rather than an internal polling interval. (GH #166)
- **A recorded change no longer disagrees with the record it was written from.** Saving a task re-read the clock rather than reporting the moment it actually stored, so any downstream record derived from a save was stamped microseconds away from the row it described and could never be matched back to it — the mismatch behind the notification loss above. A save now returns exactly the instant it persisted. The test doubles used by the suite also stopped honouring a caller-supplied timestamp, closing the divergence from real storage that made this whole class of defect untestable.
- **Stored commit identifiers are now complete.** Commit fingerprints were recorded at whatever abbreviated width the version-control tool chose at that moment, which grows with repository size, so the records held a mix of widths and anything reading a fixed number of characters silently skipped a large share of them. Full identifiers are now stored and shortened only for display, making every new record an exact match.
- **A fleet-wide rebase no longer drags workers onto a branch they have nothing to do with.** Refreshing everyone against one line of work rebased every session, including those on unrelated standalone work, grafting unrelated unmerged commits onto them and rewriting commits that had already landed — which then made the finish-line check miscount and refuse correct work. Each session is now refreshed only if the branch it actually integrates into matches the one being refreshed, and a skip says which work and which branches. Naming a session explicitly is still an override; consenting to rebase over uncommitted work is not.
- **The sync report now credits the harness that was actually written to.** Updating built-in files printed its summary and its file list under whichever destination heading happened to print last, so a write to one location was reported under another that had not been touched at all, and two of the three destinations were never reported in readable output. Each destination now reports inline, immediately after its own sync, and every claimed write names the directory it landed in.

### Added

- **A scoped test run can no longer report success while running nothing at all.** Three separate runs exited successfully having executed zero tests — a wrong crate name, a path that resolves differently depending on where it is run from, and a filter matching nothing — and all three were read as green. A wrapper now requires three things together: the command succeeded, a test harness genuinely reported, and the number that passed is above zero. The middle one carries the weight, because a success code is exactly what failed in all three cases. (GH #173)

## [2.51.0] - 2026-08-07

### Fixed

- **A message written to a session's inbox was never actually put in front of that session.** The previous release built the turn-start surfacing path that reads a recipient's unread queue and injects it into the turn that is starting. It had seven passing tests and it had never once run in production. The event it hangs off delivers the submitted text under one key; the code declared a different one, and — the part that made this invisible for a full release — the real key was declared on an unrelated, unread field, so nothing failed, nothing warned, and the handler simply returned before reaching the surfacing block. Independent corroboration that the handler had never got that far: the attribution table it also writes held zero rows across the entire life of the database, so the command that reports who wrote a line had never had data to report. The key is now read where it is actually sent, and surfacing was moved ahead of the early return it was sitting behind, so a blank turn can no longer swallow a turn's mail; either change alone restores delivery. Confirmed against a real waiting message, not a synthetic one. The regression tests parse the raw event as it arrives on the wire — every prior test built the payload by hand, which is exactly why a contract mismatch survived a release with a green suite.
- **Two more features were dead on the same wire, found by capturing real events instead of trusting the documentation.** The audit that followed the above deliberately read live captured payloads rather than inferring the shape from our own types, since that circularity is what hid the first defect. It found the guard that keeps long assistant output from wedging the interface reading a whole-message field that is never sent — the text arrives as streaming fragments — so the feature could never have worked had anyone switched it on. And the signal that says "you are already being resumed by a previous stop request" was sent on every relevant event and read nowhere, while five separate places could block a session from stopping, with no way to know they were inside a loop of their own making. Both are now wired to what the wire actually carries. A companion rule requires payload tests to parse raw captured events, so this class of silent mismatch cannot be reintroduced by a hand-built test object.
- **A worker dying no longer leaves its supervisor uninformed.** Death notices were written to one queue that a supervisor only sees if it happens to look, never to the path that actually reaches it, and they were re-emitted every time the death was re-detected — one incident produced over fourteen hundred copies. A death now writes to both places in one idempotent sequence keyed on the death itself, so re-detection collapses onto a single notice while a genuinely separate later death is still reported. The notice carries a wake signal, so it can rouse an idle supervisor and, if it never lands, shows up in the undelivered report instead of vanishing.
- **The delivery-attempt counter now counts the retries that actually happen.** Across nearly eight thousand messages it had never once incremented — not because the writer was broken, but because it was wired exclusively to rare error branches this system had never taken, while the loop that really re-sends a message counted in memory and lost the count on every restart. Attempts are now recorded durably alongside the reason, in one transaction, so a message cannot be seen with a stated reason and no attempt behind it. Being withheld by policy still deliberately costs nothing — a cooldown is not a failed attempt — and a health check now names the messages burning through attempts before they exhaust their budget, which is the only window in which anyone can act. Historical rows are left at zero rather than back-filled; there is no evidence of what their real counts were.
- **A message the recipient genuinely read no longer records itself as still waiting to be read.** Rows acknowledged by the turn-start path were immediately overstamped as delivered-and-awaiting-acknowledgement, crediting the wrong source. No delivery decision was affected — every one of those already keyed off the acknowledgement itself — but the raw records are what post-incident analysis reads, and that state produced two claims that had to be withdrawn. Such a row now records the acknowledgement it holds and names the path that produced it.
- **A completed lane could be refused at the finish line for evidence a supervisor had already produced.** The guard that requires proof a change actually landed did not recognise a supervisor's merge as that proof, so work that was merged correctly still read as undelivered and had to be re-argued by hand. A merge commit now counts as the evidence it is.
- **A background CAS server no longer keeps a project's database open after the session that started it is gone.** Servers were being left behind — four of them on one machine, still holding write-side handles on the shared project database a day and a half after the tools that launched them had died — and because they sit idle they show up in no status view. The mechanism everyone assumed was responsible turned out to work: measured directly, a server whose launcher is killed does shut itself down when its input stream closes. The leak needs the input stream to stay open with nobody on the other end, which happens whenever that stream is a terminal, or was inherited by some unrelated process that is still running, and no amount of input handling fixes it. The server now watches for the disappearance of the process that started it instead, and shuts down within seconds of confirming it, releasing whatever work it was holding on the way out. Confirmation deliberately takes two independent facts — the launcher has been replaced *and* the replacement is the operating system's adopt-orphans process — so a session that is merely quiet is never mistaken for a dead one, and a server started deliberately in the background is left alone entirely. Each server only ever inspects its own launcher and only ever exits itself, so a cleanup in one project cannot reach a live server in another on the same machine.
- **A command-line tool could talk to a socket that no longer belonged to a running program.** Socket election did not verify that the process behind an existing socket was alive and running the current binary, so a stale or superseded listener could keep answering. Election now requires a live server on the current binary before a socket is adopted.
- **Searching stored knowledge for several words at once stopped requiring every one of them.** A multi-word query silently behaved as "all of these terms", so a search that named four related concepts returned nothing while each concept individually had matches. It now ranks results that match more terms higher instead of discarding everything short of a perfect match.
- **Referencing a built-in skill written before the reference ledger existed no longer reads as a broken link.** Older references were being flagged against a ledger that post-dates them; those are now grandfathered, and any reference genuinely skipped at session start is named in a banner rather than dropped in silence.
- **The test suite no longer writes into the real stores it is running next to.** Tests were reaching live project and global databases and left almost a thousand fixture records behind. Tests are now isolated to their own temporary stores, a tool removes the residue and records exactly what it deleted, and thirteen orphaned schema-migration files with no owner were removed with a guard against their reappearance.
- **A wedged test harness can no longer consume ten minutes of a run doing nothing.** The wait for a response is now bounded, so a hang fails fast and legibly instead of burning the job's budget.
- **Retrieval quality measurement now includes the global store instead of quietly dropping it.** The parity check was scoped to project storage only, so a whole tier of what a session actually retrieves was invisible to the numbers everyone was reading.

### Added

- **Reports now ship as a single self-contained HTML file.** A built-in skill produces a report that opens correctly anywhere, with no accompanying folder of assets to keep together or lose.
- **Workers are taught when a fast check is enough and when only full proof will do.** The distinction between an inner-loop check while iterating and the final evidence a piece of work is done was previously left to judgement, which produced both wasted full runs and claims backed by a scoped one.

### Changed

- **The built-in worker and supervisor guidance is substantially shorter.** Project-specific material that had accumulated in shared guidance was removed and a size cap now keeps it from growing back, so every session pays less to be told what it needs.

## [2.50.0] - 2026-08-07

### Fixed

- **A finished piece of work could be parked behind a supervisor who was never told about it.** One session relayed "this lane is ready to merge" notices normally for a while and then went silent for the rest of the day; four completed lanes produced no notice at all and a person ended up carrying the hand-off by hand. The suspected cause — a notice addressed to a session id captured too early — was not what happened. Every notice existed and every one was written to the supervisor's inbox; each was then re-sent on a one-minute cadence because nothing ever drained that inbox, and each was finally stamped as a withheld duplicate at the exact second its task moved on. That stamp was correct about the payload, which had genuinely expired, and fatal for the fact that a notice had failed to arrive, which nothing anywhere recorded. Three things changed, one per layer. A notice that expires without ever being transported is now recorded as a distinct failure rather than being filed alongside "we withheld a copy nobody needed" — conflating those two is what made this invisible for a full day. That failure is surfaced where people already look: a banner above the worker roster, rendered even when no agents are registered, and a check in the health command that warns rather than reporting health when it cannot read the queue. Because both read columns the queue was already writing, the incident is visible retroactively, not just from now on. And the trigger itself is closed: when a supervisor's session restarts mid-run it re-registers under the same pane name with a new identity, and the tie between the old and new rows was being broken by sorting on a random id — a coin flip that could hand every later notice to the identity the operator had already walked away from. Ties now resolve to the session that exists, so a notice sent after a restart reaches a live recipient. Liveness still outranks recency, so a freshly registered but shut-down row cannot swallow notices.
- **A notice retried forever instead of ever reaching a conclusion.** Exempting undelivered notices from the withheld-duplicate stamp closed one silent path and opened another: the stamp only fires when a task leaves the state it is waiting in, so a lane parked behind a supervisor who never came back had no ending at all and would re-send indefinitely. Retries are now bounded — long enough that a merely busy supervisor always wins the race, short enough that an absent one produces a recorded failure instead of a zombie. Every notice now reaches exactly one of delivered or visibly failed. Only a real send attempt counts against the budget; waiting out a cooldown does not.
- **A log line said "delivered" about a message that was never delivered.** The arm that logged success actually fires for an inbox write, which for a message awaiting a turn boundary is not delivery — the row stays untransported and is rewritten every cadence tick. One message logged "delivered" nearly 56,000 times while its row ended up abandoned, never transported. A log line that contradicts the row it describes is a large part of why this took so long to diagnose; deferred writes now say that is what they are.

### Removed

- **Three superseded storage and search paths are gone.** The distilled knowledge library has fully taken over from the older layered store, the markdown-backed store, and the standalone hybrid search path, so those are removed rather than left as a second way to do the same thing. Behaviour for anyone using CAS is unchanged; what goes away is dead weight and the ambiguity of two code paths claiming the same job. A survey documenting what was retired, what replaced it, and what deliberately stays is included alongside.

## [2.49.0] - 2026-08-07

### Fixed

- **A message could be marked delivered to a session that never saw it.** The queue had a delivery path and no surfacing path: a row was written into the recipient's inbox file, stamped `delivered`, and nothing anywhere read it back and put it in front of the recipient. The two explanations that had been argued over — the wake-up never fired, versus a turn starting without a drain — were both true for different populations, and underneath both sat a third defect nobody had named: no hook handler read the queue at all, and the one handler that could have was scoped to a single role and returned early before it could surface anything. A turn-start handler now drains a recipient's unread rows and injects them into the turn that is starting. Selection and receipt happen in one transaction, so a caller can never end up holding content whose receipt failed to persist — the storm guard and the silent-drop guard are the same invariant. The turn-start event is now installed in the generated hooks block, where a handler wired to it would previously have been dead code for exactly the population being stranded; the other twelve events are deliberately untouched. Polling an inbox remains non-consuming.
- **Whether a session was ever woken is now measured rather than asserted.** "Wake: unobserved" was a hardcoded constant with no backing column, which is why three separate incidents produced no signal at all — and the nudge helper returned the same `Delivered` outcome from its success arm, its deferred arm and its error arm, so the three states it already computed were being discarded. They are now carried and persisted (fired, failed, not attempted), status output reports the attempt, and it names the specific signature of a nudge that fired with nothing surfacing behind it. Urgent delivery records an attempted wake too, so the gated and ungated paths are finally comparable. Migration `m220` adds the receipt-source column.
- **Clearing a session's context did nothing while reporting success.** The request enqueued the four characters `/clear` as an ordinary queued message; under team routing that row goes to an inbox, so the recipient read the *string* "/clear" as a note, acknowledged it, and carried on with its entire conversation still loaded — while the tool answered "queued". Six such calls across four sessions in one sitting all "succeeded" and none reset anything, so the checkpoint-and-clear discipline silently degraded into working to exhaustion. The reset is now a control instruction matched ahead of every message-routing path and typed over the same interrupt-and-inject channel urgent traffic uses, so it can no longer land in an inbox. Its post-condition was measured against a real session before anything was built: a genuine clear starts a new session whose transcript records the command. A reset that cannot be proven returns an error naming exactly what was and was not observed, never a cheerful "queued", and the confirmed new session id is written back so subsequent status and activity lookups read the live transcript instead of the dead pre-reset file. Harnesses where the reset is unsupported are refused before anything is queued, rather than guessed at.
- **A review-ownership setting was accepted, reported as unknown, and then ignored.** The runtime always read `code_review.owner`, but it had no command-line surface, so setting it to defer reviews produced an "unknown config key" response — and then the expensive multi-persona review ran five more times against the stated policy. Three layers were broken and instruction alone had already failed to fix any of them. The key is now real to the CLI (get, set and list, with an absent section reporting the owning default and `set` refusing anything outside the two valid values), so the effective policy can be audited. The refusal now covers the entry points that actually spend the tokens, including the current agent-spawn spelling — which had been in no generated matcher and therefore had no seam at all; it is intercepted, never auto-approved, and verifier spawns stay exempt. And the completion path no longer demands from one party the exact artifact the policy says another party produces; a solo caller with nobody to defer to is deliberately still asked for it. The honest limit is documented alongside: the refusal is a pre-tool hook, so a session running on hand-edited settings is advisory again.
- **A new working area could be cut from a months-stale branch, or from the wrong branch entirely.** Two defects on the same path. Work with provably no parent grouping was indistinguishable from work whose grouping could not be determined — both answered "none", and "none" fell through to whatever the operator had pinned; the three states are now distinct, unparented work bases on the trunk, and because that counts as a divergence the override is announced rather than silent. Separately, base resolution read the local branch without ever consulting the remote's copy of it, so a checkout was cut from a ref 71 commits behind a current one sitting right next to it. Local strictly behind now cuts from the remote commit and names both commit ids and the size of the gap; a genuine divergence keeps the local ref but reports the split; ahead, equal, or no remote is unchanged and silent.
- **A command that writes somewhere other than where you are standing now says so.** Root resolution checks the `CAS_ROOT` environment variable before the working directory, and nothing anywhere said so — so an operator who copied a store to a scratch directory, changed into the copy and ran a rehearsal had the rehearsal write to the live store instead. Every change-into-a-copy workflow inherits that trap. The precedence is deliberately unchanged, because checkouts and worktrees depend on it; what changes is that the losing candidate is named out loud, once per process, at the layer all thirty-nine call sites share, together with the one-line way to opt out. The notice goes to stderr and never stdout, so JSON output, hook payloads and stdio framing stay parseable, and no notice is emitted when both candidates resolve to the same directory by different spellings.
- **Help text no longer advertises a build flag that does not exist.** Session recording was documented as requiring a feature flag that had been removed, sending operators to look for something they cannot pass for a capability they already have; the harness options in the same struct listed a mode name the parser rejects, and whose own error message names the real one. Documentation only — no behaviour changed.

### Added

- **Legacy notes can be moved into the distilled knowledge library, and moved back out.** The migration previews by default and writes only when told to, records every page it creates in a ledger, and reports honestly on what it drained rather than rounding up. Its rollback is driven from that ledger rather than by restoring a database backup — deliberately, because the database also holds tasks, leases, sessions, verification records and queued messages that are being written continuously, so restoring it would discard more work than it recovered. Anything the migration never touched cannot be affected, because it is not in the ledger; a page whose stored path no longer matches what the ledger recorded is reported as diverged and left alone, and divergence is not counted as success. Building the rollback so it could be exercised caught a real defect in the migration itself: restored rows were being routed to whichever database happened to have the table, which put one store's rows into another — payloads now carry their origin and an unstamped payload is a hard error rather than a guessed destination. Rehearsed against copies of real databases, the post-rollback state matched the pre-migration state on every axis measured.
- **A retrieval-parity harness proves search does not regress across a migration.** A fixed query set is captured and replayed through read-only channels and the results diffed, so a cutover can be shown not to have degraded retrieval instead of being assumed not to have. Recapturing the baseline inside the frozen window is now a required step, because entries written after a baseline shift a fixed result window and produce parity "regressions" that are nothing of the kind.
- **Content that belongs to a different project is held back from distilled pages.** A cutover rehearsal put another project's client records at the head of the session briefing. Quarantine matching is proper nouns only, chosen against the real corpus: three obvious-looking generic terms were rejected because they match ordinary prose and type names in this codebase, and sixteen further candidates added nothing beyond the proper nouns. Both directions are pinned by tests.

## [2.48.3] - 2026-08-07

### Changed

- **The factory supervisor can be steered remotely again, and it stays patched.** Every agent the factory launched was started with non-essential network traffic switched off and its updater pinned. That is the right posture for a worker — a worker must not swap its own binary partway through a piece of work — but it also silently removed two things from the one session an operator actually sits with. Remote Control depends on feature-flag evaluation, which the traffic switch disables outright, so `claude doctor` inside a supervisor reported the feature as unavailable and its rollout unverifiable. The same switch bundles the updater kill switch, so a long-running supervisor never picked up a security fix. Both settings are now applied to workers only; the supervisor gets Remote Control and auto-updates, and worker behaviour is byte-for-byte unchanged. A machine that has been running with the traffic switch set for a long time may hold frozen feature-flag evaluations in `~/.claude/statsig` (or the equivalent path for an alternate config directory); deleting that cache clears them. One trade-off worth watching: with the updater live, a supervisor can update the shared CLI binary mid-run, so workers started either side of that update may differ in version.

## [2.48.2] - 2026-08-07

### Internal

- **Nothing users run changed in this release: it corrects a test that was mismeasuring a correct product.** The wiring test for a sync run asserted that each pull endpoint is requested exactly once, and it had been failing — reporting that the personal pull happened twice. It did not. A sync makes two genuinely different pulls that happen to share one URL path and are told apart by their query string: the personal pull, and the knowledge pull that asks for distilled pages. The test recognised requests by path alone, so it counted the knowledge pull as a second copy of the personal one. The failure was therefore a description of two requests the product is supposed to make, not a duplicate to be removed — deleting one, which is what the reported diagnosis called for, would have broken knowledge sync outright to make a test pass. The assertion is now made per endpoint rather than per path: the personal-pull expectation requires the discriminating parameter to be absent, and the knowledge pull is asserted in its own right instead of being silently absorbed. "Each pull endpoint exactly once" now means what it says, and the knowledge tail is covered rather than invisible.

## [2.48.1] - 2026-08-07

### Fixed

- **Syncing team knowledge could ask the cloud for every project's pages, not just this one's.** The knowledge pull built its own request and, whenever it could not work out which project it was running in, simply left the project off the request instead of stopping — so in exactly the situation the rest of sync treats as fatal, this one path quietly asked the server for everything and could import another project's pages into your database. That is the cross-project contamination the previous release was cut to clean up, reopened for knowledge pages. Every pull now goes through a single builder that refuses to make the request at all when the project cannot be determined; there is no longer any code path that can produce an unscoped pull, and a test proves the unresolvable case aborts without building a URL.

## [2.48.0] - 2026-08-07

### Internal

- **Database migration numbering.** The knowledge store's migration was developed as `m218` on a feature branch while `m218_prompt_queue_recipient_transport_create_table` shipped independently in 2.47.0. Because a released id is immutable, the knowledge migration was renumbered to **`m219`** before landing; the released `m218` is untouched and applies exactly as it did in 2.47.0. Upgrading from any published release — 2.47.0 or earlier — is unaffected: those databases have never seen either number in the other meaning, and they apply `m218` then `m219` in order. The only database that could misbehave is one that ran a pre-release build of the feature branch itself and therefore recorded id 218 against the knowledge migration; on upgrade it would treat the released `m218` as already applied and skip it. Such a database is not expected to exist outside a development checkout, and the skipped table is additionally created as a startup side effect, so even that case self-heals.

### Added

- **A project can now explain itself to an assistant without anyone writing the explanation.** Understanding an unfamiliar area meant an assistant reading its way there file by file, every session, from scratch — the same expensive rediscovery repeated on every new conversation, and the same questions asked of you again. `cas knowledge build` reads the project's own documentation, README, agent instructions, key configuration and a summary of every indexed code module, and distills them into a wiki of prose pages. The pages are ordinary markdown on disk under `.cas/knowledge/`, so they stay greppable, hand-editable and reviewable in a pull request like any other file. `cas knowledge status`, `list`, `search` and `read` cover the rest of the surface. Distilling costs model tokens, so nothing runs automatically unless you opt in — a pass over an unchanged project is guaranteed to cost nothing at all, because every source is fingerprinted and skipped when it has not moved.
- **A page you write or edit by hand is never overwritten by the machine.** The obvious failure of any generated-documentation system is that it eventually destroys the thing a human corrected. A page can be locked, and a locked page is untouchable from every direction at once: re-distillation cannot rewrite its text, its index row or its file; a cleanup pass that removes pages whose sources are gone will not remove it; and a teammate's copy arriving over sync cannot overwrite it either. Text you write above the first generated section is treated as hand-written and is never edited, even on an unlocked page.
- **Sessions start knowing what the project knows.** The startup briefing now includes a one-line pointer to every distilled page — id, type, title and a short snippet — and an instruction to pull the full text of the ones that matter. Page bodies deliberately never enter the briefing: an index of fifty pages costs a fraction of what one body would, and the assistant fetches only what the actual question needs. The index is capped, fits inside the existing briefing budget, and is byte-identical between runs on an unchanged project so it does not defeat prompt caching.
- **Distilled knowledge is searchable from both the command line and an assistant.** `cas knowledge search` does full-text search across pages, and a new `knowledge` tool gives assistants search, read, write, list and status directly. Project search also gained knowledge as a source: a query matches page text, and pages connected to what you asked about through the project's entity graph surface too — with connected results always ranked below anything that literally matched, so an indirect link can never outrank a direct hit.
- **The codemap and project-overview skills became views over the knowledge store instead of one-off generators.** They now consult existing pages before regenerating and feed what they produce back in, so the documentation they write and the knowledge an assistant retrieves are the same body of text rather than two that drift apart.
- **Teams can share distilled knowledge, and search gets sharper when the cloud is connected.** With an account connected, pages sync alongside memories, tasks and rules, and get semantic embeddings so search matches on meaning rather than only on shared words. Everything here is strictly additive: logged out, no network call is made and no extra files are created on disk, and the local project remains the source of truth either way. `cas cloud status` reports how many pages exist and how many are still awaiting embeddings.

### Fixed

- **Conceptual searches stopped silently discarding most of their own scoring.** Search blends several ranking signals with a fixed weighting, and one of them — meaning-based matching — had been removed without the weighting being updated. Sixty percent of the weight on every conceptual query was allocated to a signal that could only ever return nothing, so every result was scaled down and the remaining signals were left in the wrong proportion to each other. Ranking signals now declare whether they can actually answer, and a dead one's weight is redistributed across the live ones in proportion, preserving the intended emphasis instead of quietly deleting it. The same check stops the meaning-based channel from claiming it can answer when it is connected but has nothing cached yet.
- **A test that had been failing on `main` since the previous release passes again.** The health-check snapshot was last re-pinned before two new health rows were added, so it had been red on `main` from the moment those rows landed. It also captured a value derived from a randomly-named temporary directory, which would have made any naive re-pin fail intermittently; that value is now excluded before the comparison.
- **The startup knowledge index pointed at a command that did not exist.** The index shipped telling readers to fetch page bodies with an action the tool does not accept, so every fetch it invited returned an error — a perfect-looking index where nothing behind it worked. The instruction now names the real action, and a test drives that instruction through the actual tool router so the text and the thing it describes cannot drift apart again.

## [2.47.0] - 2026-08-07

### Fixed

- **Two different repositories that happen to sit in folders with the same name no longer sync into each other.** A project's cloud bucket was decided by its parent-folder name whenever no explicit id was pinned, so two unrelated checkouts both called `accounting` shared one bucket and merged each other's memories, tasks and rules on every sync — for months, across two different clients' work. The git `origin` remote, which identifies the repository rather than where it happens to sit on disk, is now consulted before the folder name; an explicit `cas cloud project set` pin still wins, and a project with no remote still resolves by folder name exactly as before. Because that changes which bucket an unpinned repository with a remote uses, `cas doctor` now reports which bucket the project resolves to and why, and names the exact command to pin the previous one if that is where the synced data lives.
- **`cas doctor` warns when two local projects claim the same cloud bucket.** Nothing anywhere reported a collision — the only symptom was one project's notes turning up in another. Doctor now checks every known local project and raises a warning naming both directories and the shared id. Second clones and git worktrees of the *same* repository are correctly silent; only genuinely different repositories are reported.
- **`cas cloud purge-foreign` can no longer quietly destroy the work it is meant to protect.** Its `--dry-run` reported only how many rows existed, never which ones, so the one preview available before an irreversible delete told you nothing about what you were about to lose; the dry run now lists the concrete delete set (id + title for every entry, task, rule and skill, plus the dependency-edge count) and, with `--json`, the whole set. A real run now refuses — naming the reason — when the last successful cloud pull is missing, unreadable or older than the threshold (`--stale-days`, default 7), or when local changes are still queued and have never reached the cloud; on a long-idle machine the old behaviour deleted everything local and re-pulled a months-old snapshot over it. `--force` is the explicit override. The pre-purge backup is taken with `VACUUM INTO` instead of copying a live WAL database file, which silently omitted every committed transaction still sitting in the `-wal` sidecar — the backup was unreliable exactly when it mattered. A purge whose queue of pending local changes cannot be read now stops and names the reason: that read used to answer "nothing pending" for corruption, schema drift and undecodable rows alike, which disabled the unpushed-work refusal inside the one command that deletes without asking twice.
- **`cas doctor` reports other projects' rows sitting in this project's database.** A sync leak fixed months ago left every database on a multi-project machine carrying copies of other projects' tasks, and nothing ever reported it — frozen replicas of long-finished work still read as open, so a ready list showed another project's backlog as this project's outstanding work. Doctor now scans for that contamination by default, read-only, and reports what it finds; `--foreign-rows` lists every row rather than summarising. Rows are matched on id **and** title together, never id alone: short ids genuinely collide, so an id-only sweep would delete real work — rows that share an id but not a title are reported in a separate warning section for exactly that reason. A row is only called foreign when this database has no local trace of it and another does; a copy with no trace anywhere is reported as unattributed rather than accused, and a replica that is merely closed is distinguished from one that is live. Nothing is deleted or modified by the report.
- **Starting a task that belongs to another project warns first.** Replicated rows could be claimed, worked and closed from the wrong repository — one contaminated database held working records for two other repositories' tasks, while the real rows never moved. Start now warns when a task carries no work target, no link to any local parent, an assignee nobody on this machine has registered, and cloud sync is on. It stays advisory — the task still starts — and the warning names the project, the risk and both ways out. Deliberately narrow: an ordinary task simply missing one of those signals is not flagged.
- **A merge that only moved a local branch no longer reports plain success.** `worktree_merge` moved the target branch on this machine and said "merged", while the remote stayed at the pre-merge commit — so the work was invisible to every other checkout and to the close-time merge check, and only a manual push closed the gap. The merge now publishes the branch and states the outcome every time: pushed, already current, no remote configured, or not pushed with the reason. A failed or slow push degrades to a loud "not pushed" instead of turning a completed merge into an error or hanging; a remote that has diverged is reported, never overwritten; and a branch that does not exist remotely is deliberately not created as a side effect.
- **An ordinary message wakes an idle worker again.** Only an urgent interrupt could reliably wake one — four overnight incidents, the worst leaving a worker idle for two and a half hours with a task already assigned to it. The cause was treating "the message left our outbox" as proof the recipient had seen it, when the assistant only files it away for later. Delivery is now confirmed by evidence that the recipient actually surfaced the message; the storm protection that motivated the old shortcut is unaffected.
- **A finished worker's checkout can be cleaned up through CAS.** Cleanup was unavailable in the shared-checkout setup, so a worker that finished without cleaning up on the way out left a checkout with no supported removal path — the workaround was a manual git command that bypasses tracking entirely. Cleanup now works there too and can target one specific checkout by id, branch or owner, instead of only sweeping whatever it judged abandoned. It refuses while the owner is still live, and refuses to destroy uncommitted or unmerged work unless explicitly forced.
- **A rejected close stops sending people to fix a push that already happened.** The rejection text asserted that the epic branch was unpublished purely from its name, so workers chased a non-existent problem while the real gap — their branch not being merged — went unnamed. It now checks whether the branch exists remotely and says whichever is true.
- **A green build no longer implies the test suite ran.** One job's name suggested full coverage while it compiled the tests without executing any of them; its green was read as a pass while the job that does run the suite was red on the same commit, turning a completely reproducible failure into a day-long hunt for a flaky test. The job is renamed to say it is a compile and build guard that runs no suite, and the one job that does execute the suite is marked as such. The name in your checks list changes accordingly: "Full Matrix" is now "Release-Profile & Build Guard (compile-only, no test suite)".
- **A close that cannot verify the tree now says so instead of passing silently.** The check for uncommitted work returned the same empty answer for "the working tree is clean" and "the working tree could not be inspected at all", and closing treated both as a pass — so a tree that had drifted from what was reviewed could close without a word. Those two answers are now distinct: closing still refuses outright when there is uncommitted work, and it now additionally records what it could not verify — an inspection that failed, leftover untracked files, or a checkout sitting on a different commit than the one being claimed. This is detection, not a fix for any specific drift: it makes an unverifiable close announce itself rather than look identical to a verified one. The guidance for completing work carries the same check, including for quick tasks that skip the longer list.

### Added

- **A one-command way to run tests in a clean environment.** Tests that read configuration from the environment passed locally and failed only on a fresh machine, because the shells they usually run in export a pile of variables — that is how one recently-shipped failure got through. The new command strips them all, enumerated from the live environment rather than a hardcoded list that had already drifted, and prints what it removed.

## [2.46.0] - 2026-08-06

### Fixed

- **Finished work can close after the supervisor merges it.** A worker whose branch showed only a sync-merge after the supervisor had already merged its work was refused closure and steered toward resetting the branch — the exact state that success looks like. A close carrying a valid receipt for merged work now passes, and the refusal text for genuinely empty closes names the receipt path instead of implying branch surgery.
- **A reply no longer counts as having read a message.** Any message back from a recipient used to mark every outstanding message to them "confirmed", silencing the sender's escalation clock while the recipient worked on from a stale premise. Confirmation now requires that the reply came after delivery and that the message was actually shown; without that, the clock keeps counting. Assignments the recipient already acted on are no longer re-served verbatim, and any true redelivery is labeled as one.
- **"Delivered" now means the recipient can actually find it.** Delivery used to be stamped the moment the daemon wrote a message down, with nothing on the recipient's side to corroborate it — messages could sit invisible for an entire task while their status read delivered, and one acknowledgment shape could erase a never-shown message from the recipient's inbox entirely. Every delivery now leaves a per-recipient record in the same transaction, only an explicit acknowledgment or a real surfacing hides an inbox row, and an urgent interrupt is not considered done until there is evidence it actually woke its target — retried on a throttle, never a storm.
- **Workers spawn from the branch their task belongs to.** New worker checkouts were cut from whichever epic the dashboard happened to be focused on, not the epic of the task being assigned — every spawn started on the wrong code and needed a manual reset. Base resolution now follows the task, and the spawn report names which branch it chose and why.
- **The worker-side message storm is over.** A handful of real messages could be re-injected as hundreds of duplicates, flooding a worker's context until it was forcibly compacted — two workers died to it in one afternoon. Identical parked notifications now collapse to one entry with a count, and redelivery follows the once-per-interval contract on the worker path too.
- **A shared checkout can't be silently commandeered.** A worker running without isolation could park the shared repository on its own branch, sending every subsequent supervisor merge and tag quietly to the wrong place. That state is now loudly flagged in status output, and the commit guard steers away from committing onto it.
- **The review gate holds at every door.** The rule that reviews belong to the supervisor was enforced at one entry path and bypassable through another; every dispatch route now applies the same refusal.
- **Test results stopped depending on which account ran them.** Six delivery tests failed on any machine whose environment pointed at an alternate configuration directory — noise that cost real investigation time on unrelated work and could have masked a genuine regression. The fixtures now pin their own configuration root, and a regression test runs the same path under a non-default directory on purpose.
- **Epic status tells the truth about branches.** A merged-and-closed lane could show phantom unmerged commits after its local branch was cleaned up — inviting surgery on work that was already safe — while a branch with no readable state at all reported a reassuring zero. Rows now name which branch they read, fall back to the remote when the local copy is gone, say so explicitly when neither exists, and a leftover base commit inherited from a stale spawn can no longer block an epic from closing.

### Added

- **Workers are told to never sit foreground-blocked.** The worker guides now mandate backgrounding anything long-running, with concrete recipes for builds, test suites, and CI waits — a foreground-blocked worker is unreachable except by turn-breaking interrupt, which was the leading cause of lost in-flight work.

## [2.45.0] - 2026-08-06

### Fixed

- **A parked notification no longer floods the recipient.** A lifecycle transition waiting for its recipient to wake up was re-sent on every queue poll — ten times a second, for as long as it stayed parked — producing byte-identical walls of the same message across turns, outliving both an explicit acknowledgement and the close of the task it referred to. Each transition is now delivered once immediately and then at most once per re-nudge interval, and an acknowledgement stops redelivery permanently instead of merely pausing it.
- **A worker started from a stale branch says so.** When the branch a worker is cut from has fallen behind the trunk or its own remote, the spawn now reports how far behind it is, which commits it is missing, and how to refresh it — in the spawn record and to the supervisor. Previously the only clue was a number in a status column, and workers could quietly begin dozens of commits in the past. Status views also spell out "STALE BASE: N commit(s) behind" instead of leaving that number to be interpreted.
- **Reviews can't be run by the wrong party anymore.** Under the default setup, where reviews belong to the supervisor, a worker asking to run one is now declined at the point of asking and told what to do instead. The instructions attached to the completion step had been telling workers the opposite of the rule, so the conflict resolved in favour of whichever instruction was closest to the action. Setups that assign reviews to workers are unaffected.
- **A busy machine no longer fails a passing test.** A cleanup test that plants a short-lived process could see it exit before the check ran and report a failure that had nothing to do with the code under test. It now retries the setup without weakening a single assertion, and if it genuinely cannot get a foothold it says the machine was loaded rather than blaming the feature.

### Added

- **A recipe for the "hung" test suite that isn't hung.** The recovery guide now covers left-over test processes that sit idle and block the next run — how to tell them apart from a genuinely running suite, how to clear them safely by process, and why clearing them by name is dangerous. One occurrence of this cost an hour; the suite finished in a fraction of a second once cleared.

## [2.44.0] - 2026-08-06

### Fixed

- **Codex reviewers stopped rejecting finished work over a turn of phrase.** The Codex flavor of the completion reviewer still screened close reasons with a keyword blacklist ("pending", "partial", "remaining items") long after that approach was removed elsewhere for flagging work that was genuinely done but mentioned something another team still owed. It now judges a close reason against the task's own acceptance criteria, matching every other flavor, and its review recipes cover TypeScript and Python instead of assuming Rust.
- **The startup briefing always fits, and always arrives.** On busy projects the session-start briefing could outgrow the chat window's size limit, get shunted to a file, and leave the assistant holding only the first couple of KB. The briefing now assembles under a fixed size budget: core guidance is never what gets cut, and bulky sections collapse to a count plus the command that brings the detail back.
- **Team sessions on a second account find their own team.** Team folders, inboxes, and settings files are created inside whichever account the session is actually running as, instead of always landing in the primary account's folder where that session would never look for them. Single-account setups are unchanged.
- **Hook setup follows the account you are configuring.** Installing and removing hooks now reads and writes the settings file of the active configuration directory rather than assuming the default one, so a session on an alternate account comes up with its hooks in place.
- **The operator guides describe the system that actually shipped.** The supervisor and worker guides had drifted from the code: account selection when spawning workers, the long-lived server registry, the merge and fleet-sync commands that keep factory bookkeeping intact, and the evidence a completed review must carry were missing, incomplete, or documented as something the code no longer does. All corrected against the dispatch sites.

- **The supervisor now hears about parked closes.** A worker's close rejected with MERGE REQUIRED previously vanished — fleets idled silently until a human checked in; the event now reaches the supervisor as a push signal.
- **Messages stop lying about being seen.** Wake-up nudges no longer trust the registry's "busy" claim (an automated git checkpoint counted as activity); pane and transcript evidence decide, vetoed nudges retry instead of stranding, and acks record whether they were explicit or merely inferred from a reply.
- **Fleet sync can no longer destroy work in progress.** `sync_all_workers` refuses dirty or mid-task worktrees without force, and a failed stash pop notifies both the worker and the supervisor with the stash ref instead of silently stranding the changes.
- **Status surfaces tell the truth.** `worker_status` no longer shows closed tasks as in-progress for the lease duration, the constantly-crying STALLED flag is replaced for turn-based workers by a NOT-WAKING check built on unread-mail evidence, parked-awaiting-merge workers are labeled "WAITING ON YOU" instead of looking idle, and every row shows real unread-inbox depth.
- **Triage lists stopped hiding work.** `ready`/`blocked`/`available` sort by priority by default, print the true total, and name the withheld count — previously a silent cap of 10 plus a newest-first default buried ready P0s behind fresh P2s for hours. Sort parameters that used to be silent no-ops are honoured everywhere.
- **A worker is no longer "behind" its own merged work.** Behindness is measured by content (tree equality, then cherry-pick) instead of commit topology, dissolving the circular-authorization deadlock where the follow-up assignment was blocked by the very merge it needed — including squash-merged lanes.
- **Orphaned build tools get reaped, honestly.** `rustc`/`rustdoc` orphans that wedge subsequent builds are detected and cleaned by gc (with `cargo` deliberately excluded — an adopted cargo is routinely a live build), the build-jobs derate tracks the real fleet size, and the hang-vs-kill diagnosis recipe is documented. The issue's original OOM premise was measured and corrected on the record.
- **An empty review can no longer pass as a clean one.** A review outcome missing any mandatory persona lane — not just personas_run=0 — is rejected at the close gate, with lane presence computed from what the orchestrator dispatched rather than self-reported skips.
- **The review-workflow parity guard now guards.** The rendered workflow copy had silently drifted from the shipped builtin for two days while the only parity test lived in a suite nothing ran; the guard now runs under `cargo test`, names the divergent line, and states the repair direction.

- **Codex workers no longer wedge silently in untrusted directories.** The factory pre-trusts worker and supervisor workdirs in `~/.codex/config.toml` before launch (hardened against config corruption), and the register-timeout diagnostic now names the trust-prompt cause instead of a generic timeout.
- **Assigning a task actually wakes Codex workers now.** Assignee changes emit durable wake-ups on the Codex path — previously only Claude workers reacted, and assigned P0 work sat idle until a manual nudge. Director idle notices are stamped with the instant their snapshot was read, so stale "worker is idle" claims are identifiable.
- **`task action=update` honours `blocked_by`.** Previously it silently dropped the field and reported "No changes specified", letting work start on stale inputs; blockers are now pre-validated and gated status re-armed, matching `create` semantics.
- **`spawn_workers` no longer demands a ceremonial epic.** Supplying a concrete open `task_id` permits spawning after an epic closes, instead of forcing a single-child wrapper epic.
- **A new epic no longer strands prior work by branching from a stale `main`.** Epic-branch creation compares the intended base against `HEAD`; when `HEAD` is ahead on an epic branch the divergence is surfaced instead of silently basing dozens of commits behind.

- **Finished work closes.** The close guard scopes to task-attributed commits instead of the spawn-repo factory anchor, honors `target_repo`/`target_branch`, measures against fetched remote refs, and accepts unambiguous abbreviated commit receipts; `awaiting_merge` gained a sanctioned amendment path (`request_changes`); the additive-only gate no longer counts a task's own WIP against it; zero-diff investigation closes stopped being a two-stage trap.
- **Requested workers arrive.** The spawn daemon's queue consumer survives `shutdown_workers count=0`, invalid cli/model combinations are rejected at the door instead of silently defaulting, pre-assigned tasks actually reach the worker, and spawn receipts report liveness instead of hope.
- **Coordination messages stopped lying.** Drained messages are no longer re-delivered on the idle-nudge path, signals are computed from fresh state at send time, and months-old queue items no longer land on freshly spawned workers.
- **Epic close keeps its hands off your checkout.** Closing out an epic no longer flips the main checkout's HEAD onto the epic branch.
- **The test suite is hermetic against its host.** Close-path test outcomes no longer depend on the ambient `CAS_FACTORY_WORKER_CLI` of whoever runs `cargo test`, `cas doctor` prints its breakdowns in deterministic order, timing-budget assertions tolerate loaded hosts without weakening what they prove, and registry tests neither collide on cgroup scope names nor leak five-minute orphans that stall piped test runs.
- **Choosing a Claude account starts CAS again.** `cas claude <profile>` resolved the account directory and then exec'd Claude Code directly, so the factory never started — selecting a second subscription and running CAS became two separate commands to be combined by hand with an environment variable. The account is now exported into the launching process before any thread or pane exists, and the command delegates to the same factory path as the other provider shortcuts with Claude pinned as the supervisor, so the supervisor and every worker it spawns land on the chosen account. Bare `cas claude` launches the factory on the ambient account, matching its siblings; the account listing moved to `--list-profiles`, and `--bare` keeps the plain Claude Code launcher with argument passthrough. Explicitly selecting an account now also scrubs an inherited `ANTHROPIC_API_KEY` on this path, which could otherwise override subscription OAuth and silently defeat the selection.

### Added

- **The three assistant flavors can no longer drift apart in silence.** A new test compares every shared builtin guide across all three flavors, normalizing only the differences that are meant to exist, and fails the build on any other divergence — the failure mode that had let one flavor sit four months behind the others.

- **Stacked epics are visible.** Creating an epic on top of an unlanded epic branch surfaces the full ancestry chain (depth, not one level) at creation and in `epic_status`, derived live from git topology so it cannot drift.

- **The GitHub-issues sweep is now a skill instead of folklore.** `cas-github-issues` ships as a builtin for every harness: dedupe double-filed copies, verify-and-close fixed claims, task new issues into the active github-issues epic (creating a successor epic when none is open — never tasking into a closed one), comment each issue with its task ID, unblock chained tasks when lanes merge, and file defects observed since the last sweep.

- **Long-running services get a registry instead of an ambush.** `server_start`/`server_stop`/`server_list` register agent-launched servers with ownership, logs, and a pid-identity fingerprint; registered shared servers live in their own cgroup scope so worker teardown deliberately spares them, and `stop` refuses to signal a reused pid rather than killing a bystander.
- **Worker teardown now takes the whole process tree.** Everything a worker spawns dies with it — by process group everywhere, and by cgroup subtree on delegated cgroup-v2 hosts — so escaped `npm run dev`-style stragglers no longer outlive their worker. `gc_report`/`gc_cleanup` additionally sweep dead-parent processes and stale port squatters.
- **A design-spec skill and a release-notes rubric** ship as builtins for every harness, so projects inherit a DESIGN.md generator and a publication rubric instead of reinventing them.

### Investigated

- **Dev-profile `split-debuginfo` measured end to end and rejected.** With mold and `debug = 1` already in place it buys no link time, no cold-build time, and no net disk at measurable scale; `packed` is strictly worse. Full numbers on the issue.

## [2.40.0] - 2026-08-04

### Changed

- **The default worker tier is now `gpt-5.6-terra` at high effort.** The previous default is reserved for heavy and frontier work, and the supervisor guidance, model-selection reference and code-review workflow were retiered to match. Current Codex model slugs are documented alongside, so the available options are discoverable rather than folklore.

### Fixed

- **Releases now publish a macOS binary.** The release workflow built only `x86_64-unknown-linux-gnu`, so a tag produced a single asset — while the local release script targets both platforms and the Homebrew formula requests `cas-aarch64-apple-darwin.tar.gz`. Mac users had no download path from a published release. A macOS job now builds and packages that artifact, using the pinned runner and explicit Xcode selection that Zig requires to link against a compatible SDK. The release step depends on both builds, so a macOS failure blocks the release rather than publishing a partial one — silently shipping an incomplete release is the defect, not the mitigation.

## [2.39.0] - 2026-08-04

### Fixed

- **Cross-machine sync actually runs.** The automatic sync path pushed personal changes and then pulled, with no team-queue drain between them, so team-scoped rows were never attempted at all — thousands accumulated over a month showing zero retries and no error, which reads as "nothing to do" rather than "never tried". The drain now runs between push and pull, failures record a retry count and an error per row, and a stalled queue is distinguishable from an idle one.
- **Filesystem locks are released across `fork`.** Guards released by closing their descriptor, but POSIX `flock` releases only when every descriptor sharing an open file description closes — and `fork` hands the child a duplicate. A parent dropping its guard released nothing while any forked child survived, producing worktrees and delivery targets held by operations that had already finished, with no live holder to point at. `FD_CLOEXEC` does not help, because it acts on `exec` rather than `fork`. Four call sites now issue an explicit `LOCK_UN` before close in a non-panicking `Drop`; five others already did so, and the fix converges on the pattern that was already the majority.
- **Concurrent atomic writes no longer delete each other's work.** The temporary filename combined only the target name, the process id and a wall-clock timestamp, so same-process writers could collide when clock resolution is coarse — and the loser's unconditional cleanup removed the winner's file, failing a function whose entire purpose is atomicity. Naming now uses a process-local atomic counter, and cleanup is armed only after `create_new` proves ownership, so a collision degrades to a harmless retry instead of corrupting a peer.
- **Merge targets are resolved from the work, not from ambient state.** Resolution now runs task, then assignee's tasks, then explicit authorization, then refusal. The session's display focus is deliberately absent from that chain, and merges into an already-closed target are refused rather than accepted silently.
- **Completion receipts accept the short commit references every git command prints.** A short SHA was rejected with a message indistinguishable from an unrelated merge-state failure, so the natural response was to go looking in the wrong place. Abbreviations are now resolved against the repository and the full immutable id is what gets stored, so the durable record is stronger than the input. Malformed, ambiguous, and non-commit references are rejected with messages that say which problem occurred.
- **Declining delivered work is a supported action.** Reviewing work and asking for changes previously had no sanctioned path — the only mechanism that functioned was a recovery command meant for abandoned work, which cleared the assignee and recorded the outcome as an orphan recovery. There is now an explicit verdict that returns the task to actionable with its assignee intact, records the reason as a first-class decision, and invalidates the declined receipt so refused work cannot close on it.
- **Queued messages are revalidated against live state immediately before delivery.** Notifications could describe a situation that had already changed while they sat in the queue, so a request to merge something already merged was indistinguishable from a genuinely new one without checking by hand. Stale merge requests are now suppressed and replaced with guidance, and stale lifecycle notifications are dropped. Uncertainty always delivers: only positive proof of staleness suppresses.
- **Bug reports reach the project from any machine.** Filing instructions pointed at a local filesystem path that only resolves when two checkouts share a disk, and had no commit step, so reports written elsewhere were lost by construction. Filing now targets a configured issue tracker, the report is written to disk before anything is sent so a failure cannot lose it, and a local fallback states plainly that it must be committed to be visible.

### Changed

- **The `mcp-server` feature is removed and the server is unconditional.** Building without it compiled out the server while the terminal layer still launched it, so the build produced a binary that advertised orchestration and exposed no tools. A flag that cannot produce a working binary is not a flag. Removing it also un-hid roughly 800 tests that had been silently excluded from every run.
- **Continuous integration runs for the first time.** Workflows had been registered and inactive for four months. Enabling them surfaced a linker misconfigured since April, a toolchain mismatch on macOS, tests that passed only where a particular CLI happened to be installed, and a process-environment race between concurrent tests — two of which had broken on the same April day and stayed invisible for three months. The pipeline now returns in about twenty minutes, with the expensive release-profile gate moved off the per-change path onto merges and the nightly schedule.
- **Disk-space checks share one portable helper.** Two call sites read `statvfs` independently and duplicated the platform-width arithmetic that had already caused one macOS-only build failure. They now share a single helper that exposes available and free space as distinct values, because the two callers were never asking the same question.

## [2.33.0] - 2026-07-28

### Fixed

- **A busy Codex worker is no longer reported as stalled.** `worker_status` resolved a worker's transcript through a Claude-only path, so for Codex it always came back empty — the activity clock froze at the last CAS call and in-flight suppression never engaged. A worker running shell commands continuously read as dead, and the documented response to that is to kill it. `worker_status`, `worker_activity` and `cas factory is-wedged` now share one harness-aware resolution, and a read-only `codex exec` shell-out creating a second rollout in the same directory no longer makes that resolution ambiguous. Codex workers also report a context band again.
- **Messages to workers actually arrive.** The prompt queue could re-select the same undeliverable batch indefinitely — 513 stranded rows, the oldest four months old, re-scanned roughly nine times a second — blocking every later message behind them. Undeliverable rows now become terminal under a bounded retry, one stuck target cannot hold up delivery to a live one, and retry budgets are measured from the first real attempt so a long wait before a worker registers no longer consumes them. Delivery to an idle worker, including urgent interrupts, is verified against the worker actually starting a turn rather than against a transport acknowledgement.
- **Restarting a session no longer discards queued work.** The queue's cleanup pass ran on the daemon's first tick with an empty roster, irreversibly abandoning pending messages for workers that were about to be respawned — most likely to fire on exactly the restart that installs a new build. It now waits for a populated roster and counts registered agents, not only attached panes.
- **Reusing a worker name after shutdown works.** A shutdown left a permanent tombstone on the name, so any later spawn reusing it was built and silently discarded, leaving the operator with a success message and no workers. Cancellation is now scoped to the specific in-flight spawn, logged at warning level, and cleans up the worktree it created.
- **The merge check no longer passes work it cannot see.** It keyed on a branch derived from whoever was assigned, so a branch reused across two groups of work could strand an unrelated one; keying instead on each task's own recorded commit then treated "no record" as "verified", which silently passed anything lacking a receipt. It now falls back to inspecting the live branch when no record exists, records the commit that was actually created rather than whatever HEAD points at afterwards, and invalidates that record when work is reopened.
- **Workers start from the right branch.** An isolated worker for a newly-created group of work branched from trunk instead of that group's branch, silently producing a worktree without the code the task referenced. It now branches correctly and reports loudly when a base mismatch is detected.
- **Skill reference docs reach downstream projects.** Only skill bodies synced; their `references/*.md` never did, leaving projects on whatever reference docs they were first initialized with. A managed skill body now owns its references directory, with local edits preserved and reported rather than overwritten.
- **Lease history records release reasons in their own field** instead of the transfer-attribution column, with existing rows still readable.
- **Codex commit hooks are configured.** Codex supports `PostToolUse` hooks; CAS generated them only for Claude. Hook config is now written for Codex too, surfaced for the review Codex requires rather than bypassing its trust boundary.
- **A family of parallel-run test flakes is gone.** Four separate environment-isolation helpers across two different locks were collapsed into one guard, and real-PTY tests serialize across test binaries via a file lock.

## [2.28.5] - 2026-07-22

### Fixed

- **Code review no longer silently discards reviewer findings.** The deterministic merge dropped any persona finding under its confidence threshold with no trace — a P1 that mattered was lost this way and only recovered by reading raw workflow journals. The merge now returns every rejected finding in a `dropped[]` list with reviewer provenance and the exact reason (schema errors or confidence vs threshold), logs each drop, and counts them in `stats.dropped_findings`. The codex adapter is contractually required to emit schema-complete findings, and parity tests lock the standalone, embedded, and shipped copies of the merge logic together.
- **Supervisors closing their own epics are no longer told the epic was "orphaned".** A healthy owner-closed epic now reports "epic verification: owner-closed; child tasks individually verified" in both the close response and the audit row; the orphan-recovery wording is reserved for actual orphans.
- **Workers no longer fire stale merge requests that cross with supervisor replies.** The close-rejection guidance and worker skills now tell workers to re-read just-delivered supervisor messages before escalating (the previously suggested `queue_poll` cannot see supervisor replies), and every escalation carries the current branch tip SHA plus a freshness qualifier so a stale request is self-identifying on sight.
- **The factory MCP integration tests no longer cascade-fail under parallel `cargo test`.** The env-var test lock is poison-tolerant (one failing test no longer poisons every later test), one test acquired its guard after env-sensitive setup and is fixed, and the worker skill documents the single canonical 8-variable env sanitization recipe for full-suite gates.
- **Codemap freshness no longer counts files committed together with CODEMAP.md as drift.** Staleness detection uses a strict commit-range comparison instead of timestamps, so the status line stops reporting phantom staleness after every codemap update — with positive-path coverage proving real drift is still detected.
- **Releasing a started task returns it to the ready pool.** `task action=release` used to drop only the lease, stranding the task as in-progress with no worker; it now resets status to open, clears the assignee, and records an audit note.
- **The tmpfs guardrail no longer warns on routine test runs.** Transient write-then-delete churn (parallel test temp dirs) tripped the staged-artifact warning three times per full test suite; growth must now persist across two samples before warning, while genuinely staged large artifacts still trigger.
- **Director nudges stopped racing the supervisor.** WorkerIdle "assign work" nudges are suppressed when the supervisor has contacted that worker since it went idle, delivery-time revalidation closes the assignment race, and a queued shutdown request can no longer sit unconsumed behind a slow worker spawn (no more zombie workers after shutdown-all).

## [2.28.4] - 2026-07-22

### Added

- **Large writes to memory-backed mounts now trigger a loud warning.** An agent staged 17GB of audio into a 32GB tmpfs `/tmp` over two weeks — swap saturated to 100%, the operator's apps were OOM-killed for days, and the only copies sat one reboot from loss. A new warning-only PostToolUse guardrail tracks per-session writes and usage growth on every tmpfs/ramfs mount (flocked state, single-shot fills detected on first sample, all memory-backed mounts enumerated) and tells the agent where to stage instead. Gated off the hot path: non-Write/Edit/Bash tool calls pay zero config or mount I/O.
- **Per-host staging convention.** `[staging] large_artifact_dir` in `~/.cas/config.toml` (project config wins; only the staging section is host-scoped — operator-level hooks/telemetry/llm settings can never leak into project config). When set, supervisors and workers get a one-line SessionStart notice and the guardrail names the directory in its warning. Settable via `cas config set staging.large_artifact_dir`.
- **Host-scoped memories.** Global memories tagged `host:<hostname>` now inject into SessionStart context for every project on that machine (query-layer filtered, size-capped under the SessionStart budgets). Machine facts like "this host's /tmp is tmpfs" no longer get trapped in the project where they were learned.

### Fixed

- **Task-close lint findings now name the right file and the right line.** The close-gate structural lint reported global diff indices (so multi-file diffs pointed at the wrong line), merged separate comment blocks across files and hunks into false "commented-out code" violations, and pinned findings to a single commit so follow-up fixes could never clear them. Findings are now file-qualified with per-file line numbers, comment runs reset at file and hunk boundaries, XML block doc-headers pass, and the lint evaluates the branch tip — a fix commit clears the finding.

## [2.28.3] - 2026-07-22

### Fixed

- **Factory agents can no longer wedge themselves with `AskUserQuestion`.** In factory topology the tool has no human UI surface — a supervisor calling it (as the built-in skills actively suggested for human-directed questions) got a permission prompt apparently sent to itself and paused the whole session until a human rejected it. The PreToolUse hook now denies `AskUserQuestion` for factory supervisors and workers with role-tailored guidance: ask the human in plain text and end the turn (the director relays replies); reach teammates via `coordination action=message`. The deny works even when no CAS root resolves.
- **The intercept actually fires now: `AskUserQuestion` was missing from every PreToolUse hook matcher.** Both the default settings matcher and the factory per-role settings matcher omitted the tool, so the previous advisory reminder had been dead code in real sessions. Both matchers now include it via an intercept-only list that deliberately stays out of `permissions.allow`, with regression tests preventing matcher/handler drift. Regenerate harness settings (`cas update`) to activate.
- **Skill guidance no longer steers agents into the trap.** The supervisor hard rules, intake reference, and the brainstorm/ideate skills (which mandated `AskUserQuestion` for blocking questions) now carry the factory-mode plain-text rule across all three harness variants (Claude, Codex, Grok).

## [2.28.2] - 2026-07-22

### Fixed

- **The full parallel test gate is green again.** Six `supervisor_push` lifecycle tests raced with env-mutating tests in other modules (a module-local mutex can't guard a process-wide env var), poisoning a shared lock and failing every default-parallelism `cargo test` run. All `CAS_FACTORY_SESSION`-mutating tests now serialize on the process-wide poison-tolerant env lock with panic-safe restore — verified with 5 consecutive green parallel runs. Red gates mean real failures again.
- **Supervisor rubric consistency pass.** Every copyable spawn recipe across the Claude/Codex/Grok supervisor rubrics now specifies explicit `cli`/`model`/`effort` per the GPT-5.6 Sol tier matrix, the harness `reference.md` twins are normalized (including live-worker transfer lifecycle guidance), workflow message examples include every required argument, and a guard test keeps these invariants from drifting.

## [2.28.1] - 2026-07-22

### Fixed

- **`message_status` no longer contradicts itself on pre-telemetry messages.** Rows delivered before the lifecycle columns existed reported `legacy_status: Delivered` alongside `stage: enqueued` / `pending_reason: awaiting_delivery`, forcing audits back to raw logs. A one-time migration backfill hydrates `highest_stage`/`transport_delivered_at` from `processed_at` — gated to the column-creation moment only, so live legacy paths (`queue poll`/`ack`) can never be silently promoted to a fabricated "delivered" later.
- **Lease history records the real release reason.** `release_lease_for_task` hardcoded "Task closed" for every release, so a MERGE-REQUIRED rejected close was indistinguishable from a genuine close. The reason is now threaded through the `AgentStore` trait and all call sites (awaiting-merge park, verification timeout, supervisor-review queue, reset, force-transfer, worker shutdown, preassign abort, wedged recovery, actual close).
- **Workers posting task notes are no longer flagged stalled.** `task action=notes` now emits a `TaskNoteAdded` activity event with the caller's session (non-fatal if the event store fails), and the director's stall detector counts it as worker activity — steady note-writers no longer trip false "stalled, consider interrupting" alerts.

## [2.25.0] - 2026-06-30

### Changed

- **Heterogeneous Claude + Codex factories now run mixed-harness workers reliably end to end (cas-3cb7).** A factory with one Codex worker and one Claude worker previously drifted in several places — assignment, status surfaces, director messages, and the verification/close path. These are now consistent across both harnesses (details under Fixed).
- **The Nuxt + Playwright skill no longer auto-pulls workers into browser E2E during normal dev or verification (cas-e0d1).** Its description advertised proactive triggers ("Trigger when editing files under tests/…", "when investigating Playwright test failures…"), so the model invoked it as a matter of course — doubling dev/verification wall-clock. The description is now explicit opt-in: invoke ONLY when the operator explicitly asks for Playwright/E2E help. Playwright stays fully available locally on demand (the MCP server config is unchanged); it's just no longer a default. Both the Claude and Codex skill mirrors are updated byte-identically.

### Fixed

- **Director assignment hints now name the worker, so assigning by the suggested target actually moves the task off the ready list (cas-dbbb).** The director surfaced raw session IDs as assignment targets, but assigning by ID left tasks stuck in Ready — only the worker's display name worked. Hints now use display names.
- **`worker_status` shows worktree, branch, and git detail for Codex workers, matching Claude (cas-4491).** The Clone/git block was printed for Claude workers but silently omitted for Codex workers even when the worktree existed.
- **The director no longer emits stale idle or close guidance after a task is already assigned or closed (cas-6aaf).** Status messages are now state-aware instead of telling a supervisor to reassign work that's in flight or close a task that's already done.
- **Codex workers hitting the verification gate get guidance they can actually run (cas-8aaf, cas-1b80).** The jail message handed Codex workers a `Task(subagent_type=…)` subagent flow that doesn't exist for them; it now points at `mcp__cs__coordination`, matching the Codex tool surface.
- **Codex-supervisor factories resolve the correct verification alias (cas-1544, cas-7998).** `CAS_FACTORY_SUPERVISOR_CLI` is now injected into the Codex supervisor `cs` MCP env, so close/verify guidance suggests `mcp__cs__verification` instead of a `mcp__cas__` alias a Codex supervisor can't call; the remaining hardcoded alias sites in close guidance were swept and free-text close reasons are quote-escaped so they can't break a suggested command.
- **Codex worker recovery docs use the `mcp__cs__` alias (cas-5b4f).** The built-in Codex recovery guide hardcoded `mcp__cas__` instructions that are unreachable for a Codex worker; a guardrail test now keeps the Claude and Codex copies from drifting.
- **`cas update --user` now prunes legacy non-managed `cas-*` skill orphans at the user level (cas-e0d1).** The project-level sync already drops stale `cas-*` skill dirs that lack a `managed_by: cas` marker, but the user-level path (`sync_user_builtins`) only wrote builtins and never pruned — so the retired `cas-playwright-debug` skill lingered in `~/.claude/skills` and `~/.codex/skills` on every host. The user-level sync now mirrors the project-level guard (remove only `cas-*` dirs that are neither a known builtin nor `managed_by: cas`), so the orphan is removed on the next `cas update --user`.

## [2.24.3] - 2026-06-30

### Fixed

- **Pasting multi-line text into a factory pane no longer submits the first line and queues the rest (cas-5702).** The client coalesces a paste into one event (the terminal strips the bracketed-paste markers), but it was forwarding the raw bytes to the pane, so the daemon's input parser walked them one at a time and every embedded newline reached the inner CLI as an Enter key — submitting mid-paste and dropping the remainder into the prompt queue. Paste is now carried as a single control event and re-wrapped as a bracketed paste before injection (mirroring the image-drop path), so the whole block — including any embedded newlines or control bytes — lands as one literal multi-line input.

## [2.24.2] - 2026-06-30

### Fixed

- **Codex factory no longer panics at INIT with "there is no reactor running" (cas-e202).** Starting a factory on the `codex` profile crashed the supervisor before any agent came up: `Pty::spawn` is a synchronous constructor, but its codex-only branch used `tokio::spawn` to drive the startup cursor-position (DSR) keep-alive, which panics when called from the factory daemon's runtime-free spawn thread. The keep-alive now runs on a detached `std::thread` with `blocking_lock`, mirroring the reader loop that already locks the same Mutex off-runtime — zero Tokio-runtime dependency. The Claude path was never affected (it has no `tokio::spawn`).

## [2.24.1] - 2026-06-26

### Fixed

- **`task start` no longer jails on a merge-gated sibling task (cas-6a99).** In a supervisor-deferred-merge workflow, a worker who finished task A and hit the worktree-merge gate on close (work done, awaiting the supervisor's merge) was blocked from `task start`-ing an unrelated/bundled task B — the verification-pending guard treated *awaiting-merge* the same as *actively-verifying*. `check_pending_verification` now skips tasks flagged `pending_worktree_merge` (the worker can't resolve a merge gate); the verification jail (no approved verification) still blocks, covered by a negative control in the new regression test.

## [2.24.0] - 2026-06-26

Factory-reliability sprint (multi-worker EPIC). Director coordinator hardening,
provider ergonomics, factory spec config, and cross-cutting sync/skill fixes.

### Added (this sprint)

- **Provider ergonomics — `cas claude` / `cas codex` shortcuts, `cas default <provider>`, and `--default` (cas-7f2c).** Detailed entries below.
- **`--worker-spec` / `--supervisor-spec` JSON flags + `[[factory.workers]]` / `[factory.supervisor]` TOML cascade (cas-1948).** Per-worker and per-supervisor spec config for factory spawns.

### Fixed (this sprint)

- **Director coordinator no longer fabricates "completed" notices, mis-keys assignees by name, or idle-spams (cas-889d).** Root cause: the session filter compared display names against session-id-keyed assignees, dropping every in-progress task and firing false completion events each tick. Now gates completion on real task state, resolves session ids for nudges, and suppresses nudges for workers that already hold an active task.
- **Supervisor/lead can never be nudged as an idle worker (cas-c790).** Two-layer guard in the event detector and the prompt generator.
- **Epic + worker worktrees base off the configured trunk, not the supervisor's incidental HEAD (cas-dc28).** Warns and surfaces the chosen base SHA when HEAD diverges from trunk.
- **Personal projects are no longer auto-promoted to team scope on push (cas-f8e3).** User-level team auto-pick now requires explicit opt-in; projects with no team link stay personal.
- **cas-core sync emits the `disallowed-tools` block in `generate_skill_md` (cas-e2e2).** Skills with a tool blocklist no longer drop it when synced via cas-core.
- **`filing-cas-bugs` + codex `code-review-queue` registered in BUILTIN_SKILLS (cas-61af).** `cas update` no longer silently skips syncing those referenced skill files.
- **Role-based effort defaults removed from the spawn layer; Effort threaded through PtyConfig (cas-34f7f).**

### Tests (this sprint)

- **MCP server worktree → parent-repo `.cas/` resolution coverage (cas-9db0).**
- **Non-feature-gated verification-jail regression tests — Agent-tool task-verifier bypass + factory-worker exemption (cas-c496).**

### Added

- **`cas claude` / `cas codex` provider shortcuts (cas-7f2c).** Launch a
  factory with a specific supervisor provider without remembering the
  `--supervisor-cli` flag.  `cas claude` is equivalent to
  `cas factory --supervisor-cli=claude`; `cas codex` is symmetric.  All
  existing `cas factory` flags pass through.

- **`cas default <provider>` — persist supervisor harness without launching
  (cas-7f2c).** `cas default codex` writes `[llm.supervisor] harness =
  "codex"` to `~/.cas/config.toml` and prints a one-line confirmation.
  Other config keys are preserved.

- **`--default` flag on shortcut commands (cas-7f2c).** `cas codex --default`
  both launches the factory with Codex as supervisor AND persists that choice
  for future sessions.  `cas claude --default` is symmetric.

### Fixed

- **`--supervisor-cli=claude` (or `cas claude`) no longer silently ignored
  when a codex default is persisted (cas-7f2c).** The old config-override
  block in `factory::execute` used `supervisor_cli == "claude"` as a proxy
  for "not explicitly set by the user", so an explicit `--supervisor-cli=claude`
  was indistinguishable from the default and was overridden by a persisted
  codex config value.  A new `supervisor_cli_explicit` flag on `FactoryArgs`
  fixes the precedence: explicit shortcut/flag > persisted config > built-in
  default.

### Added (continued)

- **Auto-detection of team scope on login — `cas cloud team set` no longer required for most users (EPIC cas-ab88).** CAS now fetches your team membership from `/api/me` (petra-stella-cloud) immediately after `cas login` and caches `teams[]` + `default_team_id` into `~/.cas/cloud.json`. The resolution chain in `active_team_id()` then picks the right team automatically: project-level explicit override → user `default_team_id` → implicit single-team auto-pick → personal scope. Single-team users need only `cas login` + `cas cloud sync`; no manual UUID or slug lookup required.

- **`cas cloud team default <slug-or-uuid>` subcommand (cas-6804).** Sets a user-wide team default in `~/.cas/cloud.json`. Takes a team slug (e.g. `petra-stella`) or UUID; resolves against the cached `teams[]` populated at login. Use `--personal` to revert to personal scope (clears the default). This is the recommended first-time setup step for multi-team users; single-team users typically don't need it.

- **`cas cloud team set` repositioned as advanced / per-project override (cas-6b8b).** The subcommand still works and is the right tool for per-project overrides that should differ from the user-wide default (e.g. a contractor working across multiple teams). It is no longer the primary onboarding path; `cas login` + `cas cloud team default` is.

- **First-run backfill notice on upgrade (cas-8f23).** When a user upgrades into the new auto-scope world and logs in for the first time with `teams[]` populated, CAS prints a one-time notice describing the auto-detected team and inviting them to run `cas cloud team default --personal` to opt out. The gate is `team_backfill_notified: bool` in `~/.cas/cloud.json`; it is set once and never re-fires.

### Changed

- **`teams[]` and `default_team_id` added to user-level `~/.cas/cloud.json` (cas-6462).** New `TeamInfo { id, slug, name, role }` struct. Fields use `#[serde(default)]` + `skip_serializing_if` so existing `cloud.json` files deserialise cleanly without migration.

- **`active_team_id()` resolution chain extended to read user-level config (cas-ea2f5).** Priority order: (0) kill-switch `team_auto_promote = false` → always `None`; (1) project-level `team_id` if set; (2) user `default_team_id`; (3) sole team auto-pick when `teams.len() == 1`; (4) `None` (ambiguous or no membership). The `active_team_id_with_user_config(user_cfg)` testable inner keeps the chain exercisable without disk I/O.

## [2.21.0] - 2026-06-23

Coordinated release: cloud-sync reliability (EPIC cas-f75f) + the team ticket
explorer **client half** (EPIC cas-71f7). The cloud half shipped separately
(petra-stella-cloud EPIC cas-9133).

### Fixed

- **Cloud-sync reliability — slug fragmentation, queue poison-head stall, duplicate-enqueue, silent re-homing (EPIC cas-f75f).** (A) `cas cloud team show` reports the concrete resolved slug and warns on bucket ambiguity instead of silently syncing an empty bucket. (B) A single un-pushable queue item is parked as `failed` with a reason instead of head-of-line-blocking the whole queue. (C) Legacy `NULL` `team_id` queue rows are normalized so one task mutation enqueues one item (no permanent residue). (D) `cas cloud push` no longer re-homes existing cloud entities to a changed slug without the explicit `--rehome` flag, and prints truthful per-type insert/update counts.

### Added

- **Team ticket explorer — CLI client half (EPIC cas-71f7).** Three behaviors that keep the CLI in sync with the web ticket explorer (petra-stella-cloud). See `docs/team-ticket-explorer-client.md`.
  - **Canonical project-id adoption on push (cas-8ca5).** `cas cloud push` (team scope) sends your normalized git remote; when the server's returned `git_remote` matches your local `origin`, the returned `canonical_id` is adopted into `.cas/config.toml`. Stops an unpinned machine from syncing a fragmented per-remote bucket. Equality-gated so a shared machine with a different remote is never silently re-homed.
  - **Web-initiated close reconcile on pull (cas-fc52).** A teammate's close from the web UI (`closed_via = "web"` tombstone) is reconciled as an authoritative local close — applied even if the local copy is newer, with `close_reason` preserved and `assignee` cleared. Merges only the close signal so locally-authored unpushed content is not clobbered. Idempotent; never reconciles the client's own pushed closes.
  - **Read-only mirror of web-authored comments (cas-7d54).** `task show` surfaces comments authored in the web explorer (author, timestamp, body, image/video/link attachments), fetched per task. Best-effort: degrades to nothing when not logged in / offline; never blocks or fails `task show`.

## [2.20.0] - 2026-06-07

### Fixed

- **Isolated factory workers no longer leak commits onto the supervisor's branch (EPIC cas-073f).** Workers spawned with `isolate=true` could commit to the supervisor's shared checkout (`main`/`epic`) instead of their own worktree. Root cause: the worktree-reuse path in `WorkerSpawnPrep::run` checked `path.exists()` but not that the directory was a real git worktree on the expected branch — a stale dir made git walk up to the main checkout's `.git`, so `HEAD` resolved to the supervisor's branch and every commit landed there (deterministic, not the race the report hypothesized). The reuse path now validates the branch and hard-errors on mismatch; `isolate=true` fails loudly instead of silently degrading to the shared checkout, and a post-spawn assertion verifies each worker sits on `factory/<name>`.

### Added

- **Defense-in-depth worker commit guards.** Three layers, all on a `factory/<name>` *allowlist* — a worker may only commit on its own branch; `main`/`master`/`staging`/`epic/*`/any other branch and detached HEAD are denied: a PreToolUse intercept on `git commit`/`merge`, an installed git pre-commit hook (the bulletproof floor for non-tool commits), and a SessionStart cwd/branch assertion.
- **`coordination action=worker_status` git introspection.** Reports per worker: branch, worktree path, HEAD sha, ahead/behind vs base, dirty/clean, last pushed ref, and open PR URL — worker "done" is verifiable without git forensics.
- **`task close` gated on merge reality.** Refuses (or routes to pending-merge) when no commit is reachable from the worker's `factory/<name>` branch and no PR exists, without blocking additive-only / zero-commit closes.
- **Worker-stop git-state event + PreCompact findings flush.** On worker stop the final git state is emitted to the supervisor feed; on context compaction, in-flight findings are extracted from the transcript and written to the worker's active task so they survive the compaction.
- **Truthful worktree status.** `worktree_list` / `worktree_status` report live factory (isolation) worktrees instead of the misleading "experimental and disabled" message.

## [2.16.1] - 2026-05-14

### Fixed

- **Hook emitters reverted to shell-form to silence Claude Code 2.1.139's `/doctor` validator (cas-c17b).** Claude Code 2.1.139 introduced an exec-form hook shape `{ "type": "command", "args": [...] }` that the runtime accepts but the `/doctor` schema validator rejects with `Expected string, but received undefined`. The warning fires *before the agent loads* in every spawned worker pane, forcing manual dismissal on every factory worker spawn — significant friction in factory mode with 4+ workers per EPIC. CAS migrated to exec-form in cas-7ecd for the no-shell-parsing safety property; the property was theoretical (`cas hook <Event>` takes zero user-controlled args, payload flows via stdin), so the revert costs nothing functional. All 14 emitter sites flipped: 12 in `cas-cli/src/cli/hook/config_gen.rs` (every event: SessionStart, SessionEnd, Stop, SubagentStart, SubagentStop, PostToolUse, PreToolUse, UserPromptSubmit, PermissionRequest, Notification, PreCompact, plus the `cas factory check-staleness` SessionStart entry) and 2 in `cas-cli/src/ui/factory/daemon/runtime/teams.rs::factory_hooks_block()` (the factory's `supervisor-settings.json` / `worker-settings.json` PreToolUse + PermissionRequest). `duplicate_check::has_cas_hook_entries()` keeps recognizing both forms so existing pre-cas-c17b user settings are still detected as CAS-installed. Upstream tracker: [anthropics/claude-code#58441](https://github.com/anthropics/claude-code/issues/58441) — once the /doctor validator is fixed in claude-code, we can re-evaluate.

- **Hook-emission tests cover all 11 events + check-staleness (cas-aee5).** Follow-up to cas-c17b. The original revert's tests only iterated over 6 of the 11 emitted events; a partial regression leaving SubagentStart, SubagentStop, PermissionRequest, Notification, or PreCompact in exec-form would have shipped undetected. `hook_entries_emit_shell_form_command_string` and `hook_entries_do_not_emit_exec_form_args` now iterate all 11. A new `session_start_check_staleness_emits_shell_form` test reaches the second SessionStart hook entry (which `first_hook_command` cannot — added a `nth_hook_command` helper) and asserts the `cas factory check-staleness` invocation is also shell-form. `test_exec_form_still_detected_by_has_cas_hook_entries` fixture upgraded to include `matcher` + `timeout` fields so the dual-form detection contract tests against realistic pre-cas-c17b settings shape.

### Changed

- **New installs default to `https://petra-stella-cloud.vercel.app` instead of upstream `https://cas.dev` (cas-9cbd).** The `pippenz/cas` fork operates its own cloud; the hardcoded upstream default was leaking into every new install of this fork. Default flipped at four code sites: `default_endpoint()` in `cas-cli/src/cloud/config.rs`, the `LoginArgs.endpoint` clap default in `cas-cli/src/cli/auth.rs`, the `LoginArgs::default()` impl, and the corresponding test pin. `cas-cli/src/ui/factory/daemon/cloud_client.rs` doc comment updated. Existing users with `https://cas.dev` already in their `~/.cas/cloud.json` are NOT auto-migrated — re-run `cas auth login` (no args) to opt in to the new default. The serde compat test at `cas-cli/src/ui/factory/daemon/runtime/cloud.rs:309` deliberately keeps `cas.dev` in its JSON literal (it pins roundtrip semantics, not the default value).

- **`CAS_CLOUD_ENDPOINT` env var now actually overrides the endpoint (cas-9cbd).** Previously the env var was set by `scripts/provision-hetzner.sh:195/:312` and by user shell-rcs with the apparent intent of overriding the endpoint, but production code never read it — only the e2e test harness in `cas-cli/tests/e2e/team_sync.rs` referenced it, and those assertions passed via a "not configured" fallback OR clause. `default_endpoint()` now reads `CAS_CLOUD_ENDPOINT` first (non-empty, trimmed) before falling back to the hardcoded URL; the clap `LoginArgs.endpoint` arg gets `env = "CAS_CLOUD_ENDPOINT"` mirroring the existing `CAS_CLOUD_TOKEN` pattern at `auth.rs:31`. `LoginArgs::default()` now delegates to `default_endpoint()` so programmatic callers also honor the env var. Hetzner provisioning's existing `export CAS_CLOUD_ENDPOINT=https://petra-stella-cloud.vercel.app` now works without script changes.

### Security

- **URL scheme validation on `CAS_CLOUD_ENDPOINT` and `--endpoint` (cas-9cbd).** New `is_acceptable_endpoint()` helper accepts `https://*` and `http://localhost` / `http://127.0.0.1` / `http://0.0.0.0` only. Rejects `file://`, plain hostnames, arbitrary `http://`, and anything else that could redirect the device-code token exchange to an attacker-controlled server. The localhost carveout is required by `cas-cli/tests/e2e/team_sync.rs` (wiremock on `http://127.0.0.1:<port>`). Validation behavior is asymmetric by surface: `default_endpoint()` soft-fails (invalid value → `tracing::warn!` + fallback to hardcoded default, preserves the infallible `Default` contract), `LoginArgs.endpoint` clap value_parser hard-fails (`Error: must be https:// or http://localhost`). Whitespace-only env values also fall back (via `.trim().is_empty()` filter).

### Tests

- **Test race condition introduced by env-var wiring fixed (cas-9cbd follow-up).** When `default_endpoint()` started reading `CAS_CLOUD_ENDPOINT`, every test that constructs `CloudConfig::default()` (directly or via `..Default::default()`) became a potential race victim against the 3 new env-var tests. `CLOUD_ENV_LOCK` was moved from a tests-module-local mutex to a `#[cfg(test)] pub(crate) static` at module scope and is now acquired by all 11 sibling tests in `cas-cli/src/cloud/config.rs` (`test_default_config`, `test_save_and_load`, `test_logout`, `test_set_and_clear_team`, four `test_active_team_id_*`, etc.) with `unwrap_or_else(|p| p.into_inner())` for poison recovery. Re-exported through `cas-cli/src/cloud/mod.rs` so the 5 new `auth.rs` tests share the same mutex.

### Upgrade notes

- **In-flight factory sessions retain the old exec-form `supervisor-settings.json` / `worker-settings.json` until you run `cas factory --new`.** The CAS daemon writes those files eagerly at spawn time, not at binary-upgrade time. Worker panes in your current session will keep getting the `/doctor` warning until you restart the factory. New sessions get clean shell-form settings automatically.
- **Existing logged-in users are not auto-redirected to the Petra Stella cloud.** If your `~/.cas/cloud.json` has `endpoint: "https://cas.dev"`, you keep talking to upstream's cloud. Run `cas auth login` (no `--endpoint`) to pick up the new default; or pass `--endpoint https://your-server` to override; or set `CAS_CLOUD_ENDPOINT` in your shell.
- **Provisioning scripts that set `CAS_CLOUD_ENDPOINT` now actually take effect.** If you have any inherited setup expecting that variable to be ignored, verify the new endpoint is the one you want — the variable will now route token exchange and cloud sync to whatever you set.

## [2.16.0] - 2026-05-13

### Changed

- **Stock worker LLM default flipped to Claude Sonnet 4.6 + `reasoning_effort=high` (cas-05e3).** Previously, workers spawned without an explicit `[llm.worker]` block in `.cas/config.toml` (the common case for new installs and most upgraders) fell through to the harness builtin — whatever Claude Code happened to pick by default. New behavior: `LlmConfig::model_for_role("worker")` and `reasoning_effort_for_role("worker")` apply a stock floor when both the role-specific override (`[llm.worker.X]`) and the top-level fallback (`[llm.X]`) are unset. Three-step chain becomes: role-override → top-level → stock-worker-default (`claude-sonnet-4-6` for model, `high` for reasoning effort). Supervisor role is deliberately untouched — `supervisor_does_not_receive_worker_stock_default` regression-locks that boundary. Existing users who explicitly set top-level `[llm] model = "X"` expecting all roles to inherit still see workers resolve to `X` (back-compat hinge). Runtime-only fallback: no changes to `cas init` or `cas update` config seeding, which means updating the stock constant in cas-src automatically propagates the new default to every install without requiring users to re-init or hand-edit. To pin a different worker model, add `[llm.worker] model = "..."` to `.cas/config.toml`. Constants `STOCK_WORKER_MODEL` and `STOCK_WORKER_REASONING_EFFORT` are now public from `cas-cli/src/config/settings.rs` for downstream consumers. 6 new tests + 1 split of the existing `reasoning_effort_for_role_no_config_returns_none` cover the full resolution matrix.

## [2.15.3] - 2026-05-13

### Fixed

- **`cas cloud team set <uuid>` now eagerly resolves the project canonical slug (cas-1ced, EPIC cas-ffc4 closes).** Final task in the EPIC opened against the original cloud-team bug doc — closes hypothesis #3, the last UX paper-cut from daniel.l's onboarding. Previously, `cas cloud team set` printed `Slug resolution deferred — see cas cloud team show` and didn't actually resolve the canonical project ID. When the working-directory name didn't match the canonical slug (daniel.l cloned the repo into `~/cas` while the canonical project ID was `cas-src`), his first `cas cloud sync` went out with `project_id=cas` and routed push/pull to a phantom project; the documented workaround was renaming the directory. Fix: after persisting the team_id, the handler now runs an eager resolution chain — `.cas/config.toml [project] canonical_id` → `git -C <root> remote get-url origin` (normalized via a new `normalize_git_remote_url` helper handling HTTPS, HTTP, `ssh://git@host/`, and the `git@host:owner/repo` shorthand, with `.git` suffix stripping) → defer. The deferred case explicitly does NOT fall back to the working-dir basename (that was the bug). When a slug is resolved, it's written to `.cas/config.toml [project]` and surfaces in subsequent `get_project_canonical_id()` calls. Output indicates the source (`from .cas/config.toml` / `derived from git remote` / deferred); JSON mode carries the same info as `canonical_id_source`. New `cas cloud project set <canonical-id>` subcommand for manual override (monorepo / non-git / custom layout). `cas cloud team show` now displays the resolved project slug alongside the team UUID. Config plumbing adds a `[project]` section to the `Config` struct (`ProjectConfig { canonical_id: Option<String> }`) wired through `merge_missing` + `init.rs`; `resolve_canonical_id` precedence becomes `config.toml → folder-name → path-hash`, backward-compatible. 17 new tests (6 integration in `team_set_slug_resolution_test.rs` covering config-preserve / HTTPS derive / SSH derive / no-basename-default negative / project set / team show; 11 unit in `cloud::config::tests` covering URL normalization shape table + config.toml round-trip + section-preserve + resolution precedence).

### Cross-team coordination

- **EPIC cas-ffc4 closes end-to-end.** Original bug doc moved to `docs/requests/completed/`. The three hypotheses surfaced in the cloud team's filing are all addressed: (#1) missing endpoint wire-up shipped in v2.15.1; (#2) cross-project watermark reuse shipped in v2.15.2; (#3) deferred slug resolution shipped here in v2.15.3. New team-member onboarding now lands the correct project slug at `cas cloud team set`, hits the team endpoint on `cas cloud sync`, and keeps `since=` watermarks scoped per-(team, project).

## [2.15.2] - 2026-05-13

### Fixed

- **`cas doctor --fix` no longer fails with `no such table: skills` on bootstrap-pending DBs (cas-bdb9, EPIC cas-9fdb).** Surfaced on the ozer-health project (macOS): `cas doctor --fix` / `cas update --schema-only` on a fresh `.cas/cas.db` that had never had `SqliteSkillStore::new()` / `SqliteAgentStore::new()` (or any other lazy-bootstrap store) constructed in-process exploded with `migration failed: skills_add_summary - database error: no such table: skills`. Root cause: the `skills` (and `agents`) tables are created lazily by `CREATE TABLE IF NOT EXISTS` inside the store constructors, but the migration runner did not invoke those constructors before running ALTER migrations like `m071_skills_add_summary`. Fix promotes the lazy-bootstrap schema constants (`SKILL_SCHEMA`, `AGENT_SCHEMA`, `TASK_SCHEMA`, `ENTITY_SCHEMA`, `VERIFICATION_SCHEMA`, `LOOP_SCHEMA`, renamed `ENTRIES_RULES_SCHEMA`, plus extracted `WORKTREE_SCHEMA` and `CODE_SCHEMA` for symmetry) to `pub` and adds `Subsystem::ensure_base_schema(&conn)` + `ensure_base_schemas(&conn)` in `cas-cli/src/migration/mod.rs`, wired into `run_migrations` between `ensure_migrations_table` and `bootstrap_migrations`. Sentinel-gated per subsystem — if the canonical table already exists the bootstrap skips that subsystem and leaves the migration chain authoritative, preventing legacy partial-state DBs from being touched. Subsystems bootstrapped: Entries / Tasks / Skills / Agents / Entities / Verification / Loops. Subsystems with explicit `m###_*_create_table` migrations (Worktrees / Code / Events / Recording / Recordings) are DELIBERATELY EXCLUDED — pre-installing the post-ALTER shape would break later ALTERs (e.g., m112 expects `worktrees.task_id` which m120 renames to `epic_id`). The exclusion list and its rationale are spelled out in the `WORKTREE_SCHEMA` / `CODE_SCHEMA` doc comments. Includes the `task_leases` dual-definition cleanup: `TASK_SCHEMA` previously defined `task_leases` with `renewed_at TEXT` (nullable, no FK) AND `AGENT_SCHEMA` defined it with `renewed_at TEXT NOT NULL` + `FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE` — `Subsystem::Tasks` iterating before `Subsystem::Agents` meant the slim version always won on fresh bootstrap, silently losing the NOT-NULL + FK. `AGENT_SCHEMA` is now the single source of truth (lifecycle owns lease semantics). Plus housekeeping: lifted the `idx_entries_helpful_score` expression index into `ENTRIES_RULES_SCHEMA` (was a best-effort `let _ =` in `store_init`); removed the duplicate sessions DDL from `store_init` now that `ENTRIES_RULES_SCHEMA` covers it.

- **`cas cloud sync` no longer reuses stale watermarks across projects within the same team (cas-53d5, EPIC cas-ffc4).** `CloudSyncer::pull_team` previously keyed its `since=` watermark globally per team (`last_team_pull_at_{team_id}`). A user working on team T across two projects P1 and P2 would full-backfill P1, then switch to P2 and have the second pull silently skip historical T+P2 backfill — surfacing as the same "0 of everything" symptom that v2.15.1's cas-6ec7 fixed at the endpoint-routing level (hypothesis #2 from the cloud team's bug doc, the next failure mode lying in wait). Fix re-keys the watermark to `last_team_pull_at_{team_id}_{project_id}`. Absence of the new key is treated as "first sync into this scope" — no `since=` is sent, triggering a full backfill. Best-effort cleanup retires legacy global-per-team keys on first successful per-scope write. `pull_team` now takes `project_id: &str` as an explicit parameter (rather than internal resolution via the process-wide `get_project_canonical_id()` cache); the cached static would otherwise lock all in-binary tests to a single project_id, making the cross-project regression test impossible. `cas cloud pull --full` now scopes its watermark clear to the current `(team, project)` pair only. 3 callers updated (`execute_team_pull`, the `worktree_verification_team_ops` MCP helper, the `team_memories_e2e_test` fixture). 4 new tests in `cas-cli/tests/team_pull_watermark_scope_test.rs` covering: cross-project full backfill (second project sends no `since=`), same-scope incremental (second pull sends the recorded `since=`), `--full` scope isolation (P1 cleared, P2 intact), and key-format lock.

### Cross-team coordination

- **EPIC cas-ffc4 remains OPEN — sibling task cas-1ced still pending.** Eager project-slug resolution at `cas cloud team set` (closes hypothesis #3 from the bug doc, fixes the case where the cloned working-dir name doesn't match the canonical project slug and the first sync goes out with the wrong `project_id`) is queued and will ship as a follow-on patch.

## [2.15.1] - 2026-05-13

### Fixed

- **`cas cloud sync` now actually pulls team data for newly-onboarded team members (cas-6ec7, EPIC cas-ffc4).** Filed by the cloud team as P1 (`docs/requests/BUG-cloud-sync-pull-returns-zero-for-new-team-member.md`): a new team member walking through `cas-login` → `cas cloud team set <uuid>` → `cas cloud sync` would see `0 of every entity type` synced despite thousands of team-scoped rows existing for the active project on the cloud side. Push was correctly hitting the team endpoint; pull was hitting only the personal endpoint (`/api/sync/pull`, filtered by `team_id IS NULL`), so a team-only member legitimately got nothing back. Root cause was a missing call site: `CloudSyncer::pull_team` (the team-pull helper at `cas-cli/src/cloud/syncer/pull.rs:688`) was fully built and tested but only invoked by one MCP worker-verification helper and the e2e tests — never from `cas cloud sync` or `cas cloud pull`. Fix wires a new `execute_team_pull` helper into both `execute_pull` (and transitively, `execute_sync`), symmetric to the existing `execute_team_push` (cli/cloud.rs:1313): same isolation contract (errors never propagate), same `report_team_pull_{result,partial,error}` reporter trio, same JSON output shape. `cas cloud pull --full` also clears the per-team `last_team_pull_at_<team_id>` watermark when a team is configured so team backfill happens on `--full` just like personal does. Behavioral wiremock tests in the new `cas-cli/tests/team_pull_wiring_test.rs` (7 tests, `.expect(1)` on both endpoints in the positive case + `.expect(0)` on the team endpoint in the no-team negative case) lock the contract — including the double-call regression guard caught by multi-persona code review.

### Cross-team coordination

- **Companion follow-on tasks remain open under EPIC cas-ffc4.** `cas-53d5` (re-key team-pull watermark to be per-(team_id, project_canonical_id) so cross-project sync from the same team doesn't silently skip historical backfill) and `cas-1ced` (eager project-slug resolution at `cas cloud team set` so a working-dir name that doesn't match the canonical slug stops causing the `project_id=cas` instead of `project_id=cas-src` misroute) are the next two failure modes the bug doc surfaced. Both will ship as separate patches.

## [2.15.0] - 2026-05-12

### Changed

- **`cas cloud pull` now always sends `?project_id=<canonical>` (cas-ed15, EPIC cas-2eb3).** The `cas cloud pull` CLI handler previously built its URL inline via raw `ureq::get` and never appended `project_id=`, bypassing the scoped `CloudSyncer::pull` abstraction that `cas cloud sync` and `cas cloud purge-foreign` already used. The leak returned `team_id IS NULL` rows from all of a user's projects on every pull, contaminating local DBs with foreign-project data. The fix replaces the inline builder with a `CloudSyncer::pull` construction — same scoped abstraction, hard-fails when `get_project_canonical_id()` returns `None`, gates every store import behind `entity_matches_project`. Three regression tests in `cas-cli/tests/pull_scoping_regression_test.rs` (source-level scan + file-level check + wiremock URL assertion) lock the contract. Empirical wire trace from cas-src confirms `GET /api/sync/pull?since=…&project_id=cas-src` post-fix.

- **`CloudSyncer::pull` extended to all 9 entity kinds, properly scoped (cas-bba4, EPIC cas-2eb3).** cas-ed15 fixed the pull leak by routing through `CloudSyncer::pull`, but that abstraction only covered entries / tasks / rules / skills — `cas cloud pull` previously imported specs / events / prompts / file_changes / commit_links from the inline path *unscoped* (the leak). Removing them in cas-ed15 was strictly better than the leak, but `cas cloud pull` returned zero counts for those kinds. This change extends `CloudSyncer::pull` to handle all 9 kinds with the same `entity_matches_project` scoping the original 4 used. Wire trace from cas-src confirms `cas cloud pull --full` now imports the missing entity kinds (9595 events on the test pull) properly scoped. Forward-compatible: `body.specs.unwrap_or_default()` lets older cloud builds (which don't return `specs` yet) deserialize cleanly. Companion cross-team request `docs/requests/FEATURE-cloud-sync-pull-return-specs.md` filed asking cloud to extend the `/api/sync/pull` response.

- **Cloud push client detects and surfaces server-side skipped rows (cas-f645, EPIC cas-2eb3).** `CloudSyncer::push_sub_batch` now parses the response body into a `PushResponse` carrying an optional `skipped: HashMap<String, usize>` per entity type. When the server reports a non-zero skip count for an entity type (the signal Postgres emits when `ON CONFLICT DO UPDATE … WHERE false` silently excludes a cross-project conflict), the client emits a `tracing::warn!` and leaves the entire sub-batch un-marked-synced so items remain retryable in the local queue. Backward-compatible: every field is `#[serde(default)]` so older cloud builds that omit `skipped` deserialize cleanly and fall back to the legacy mark-synced path. Six tests (4 unit + 2 wiremock integration) pin both paths.

- **`cas update --sync` now surfaces silent-skip warnings for stale unmanaged files (cas-4900).** The `sync_builtin` gate previously collapsed two distinct outcomes — "no-op happy path" and "stale source/dest both lack `managed_by: cas`" — into the same `Ok(false)` return. The latter case silently left projects with stale reference files for unknown durations. New `SyncOutcome` enum distinguishes `Created` / `Updated` / `Unchanged` / `SkippedNotManaged`. `SyncResult::skipped_files` is now populated on the silent-skip path, and `cas update --sync` prints a yellow `! <path>` list under the existing "Built-ins" reporting block with a one-line nudge to add the `managed_by: cas` marker. Pre-existing silently-failing class of refresh failures is now loud and debuggable.

### Performance

- **SIMD `memchr` fast-path on the alt-screen scanner (cas-219d).** `Pane::update_alt_screen` previously walked the input byte-by-byte looking for the ESC (`0x1b`) byte that starts a CSI escape sequence. On bulk non-CSI text (the steady state during normal terminal output) that's ~1 cycle per byte. Outer loop now seeks ESC via `memchr::memchr(0x1b, ..)` (SIMD-accelerated to ~16 bytes per cycle on x86_64). Criterion bench in `crates/cas-mux/benches/alt_screen_scan.rs` measures the impact: 64 KiB ESC-free chunk in ~546 ns (~117 GB/s SIMD throughput); sparse-ESC 64 KiB chunk (1 ESC per 200 B) in ~1.76 µs; dense-match 4 KiB chunk in ~1.47 µs. Strict optimization — every loop exit either breaks (memchr None) or strictly advances `i`; observationally identical to the byte-by-byte scan on every input shape (empty, lone ESC, ESC at end, ESC followed by non-`[`, split sequences across feed calls). Regression test `update_alt_screen_esc_free_64k_preserves_state` pins the no-ESC / no-state-change invariant.

### Added

- **`verify-before-claim` pre-close skill (cas-5b2a, EPIC cas-ebea).** New `.claude/skills/verify-before-claim/SKILL.md` (+ Codex mirror) — a four-step agent-discipline protocol that kills the "narrate done before proving it" failure mode. Steps: (1) name the proof command, (2) run it FRESH, (3) capture exit code + tail output, (4) only then call `mcp__cas__task action=close`. CAS already has the mechanical layer (`verification_store` + close-gate's six checks); this skill is the agent-discipline layer on top. Trigger: any time an agent is about to assert tests pass, the build is clean, the script works, the bug is fixed, or the AC is satisfied. Advisory in v1 — required-paste enforcement is a clean follow-up if telemetry shows the advisory form under-performing. Registered in both `BUILTIN_SKILLS` and `CODEX_BUILTIN_SKILLS`; cas-worker SKILL.md wires it into step 6 of the close routine. Five install-path tests cover presence, frontmatter, four-step markers, registration, and cas-worker cross-reference. Confirmed live: SessionStart's available-skills list now picks it up immediately from the destination `.claude/skills/` without a cas-side daemon restart.

- **"Context budgeting" methodology section in `cas-supervisor` + `cas-worker` skills (cas-5787, EPIC cas-ebea).** New section in both skill bodies (Claude + Codex × supervisor + worker = 4 files, plus 4 destination mirrors) naming the three context layers — Immutable Core / Task Context / Ephemeral — citing the 12 KB SessionStart cap enforced by `test_*_guidance_under_12kb`, cross-linking `project_session_start_truncation.md`, and closing with the decision rule "Adding here? Only if every session needs it; else `references/<name>.md`". Regression test `test_skills_document_context_budgeting_cas_5787` asserts seven required markers across all four bundle-relevant files so silent drift via `cas update --sync` becomes a compile failure. `supervisor_guidance()` bundle goes from 11,898 → 12,277 bytes (11-byte headroom under the 12,288 cap) — tight but deliberate, since the new section literally documents the cap that constrains it.

- **`session-learn` skill: 7-signal session classifier (cas-39f5, EPIC cas-ebea, v1 skill-only).** New `.claude/skills/session-learn/SKILL.md` (+ Codex mirror) borrowed from `third-brain-v5-skills` and adapted to the CAS memory schema. Documents the 7-signal taxonomy (concept / entity / correction / pattern / idea / decision / gap) with each signal mapped to a concrete CAS `entry_type` + tags + scope. Available for manual invocation today ("extract this session", "save what we learned"). New `[memory] session_learn_auto = false` opt-in flag in `.cas/config.toml` reserves the auto-trigger contract; the Stop-hook auto-fire implementation is tracked under sibling task `cas-6156`.

### Fixed

- **Factory worker `task.close` no longer hits `VERIFICATION_JAIL_BLOCKED` under owner=supervisor (cas-8edb).** Regression introduced by v2.13.0's `[code_review] owner = "supervisor"` default flip: workers stopped submitting `ReviewOutcome` envelopes at close (because review now runs at supervisor cherry-pick time), but the v2.12.0 self-cert path required that envelope to bypass the jail. Symptom: every factory worker close was rejected with `VERIFICATION_JAIL_BLOCKED: Mutating operation task.close blocked. Task <id> requires verification before any mutations are allowed.`, forcing supervisor close-on-behalf with `bypass_code_review=true` on every task. Fix: two surgical changes gated on `is_factory_worker && code_review.supervisor_owned()` — `cas-cli/src/mcp/server/mod.rs::authorize_agent_action` exempts workers from the jail on `task.close` when owner=supervisor; `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs::cas_task_close` computes `worker_under_supervisor_review` early and skips the verification gate when true. Supervisor-driven paths untouched (`is_factory_worker=false`). Legacy `owner = "worker"` untouched (`supervisor_owned()=false`). Three new regression tests pin the contract: zero-diff worker close self-certs, additive-only worker close self-certs, legacy `owner=worker` still jails clean close without envelope. Post-mortem in `docs/requests/completed/BUG-cas-8edb-verification-jail-regression-on-supervisor-owned-review.md`.

- **`update_alt_screen` correctly handles CSI sub-params + resets `in_alt_screen` on pane exit (cas-e0b9).** Two distinct bugs in the alt-screen state machine, both fixed characterization-first (failing tests committed before the fix so the bugs are pinned in history). (1) CSI sub-params: the parser didn't handle `\x1b[?1049;1h` style colon- or semicolon-separated sub-parameters per ECMA-48 §5.4.2 — split mid-subparam input flipped state unpredictably, and unknown modes inside the sub-param list could spuriously flip `in_alt_screen`. After the first parameter's digit run, the scanner now consumes the full `[0-9;:]` run before checking the final byte; leading mode controls the toggle (xterm semantics), sub-params are read but not interpreted, truncated-mid-subparam skips safely. `trailing_dec_partial` widened to carry partial sub-param sequences across chunk boundaries. (2) Pane exit: when a pane process exited while `in_alt_screen=true`, the flag was never reset, leaving the next process (or terminal redraw) confused about whether the alt-screen was active. `mark_exited` is now a `pub fn` lifecycle API that clears `in_alt_screen` and drops `partial_esc` while preserving the `PtyEvent::Error` path's existing "preserve previously-set exit_code" semantics; `poll` / `drain_output` route through it. Regression coverage adds multi-param chain (`?1049;1;2:3h`), truncated mid-subparam (no panic / no spurious flip), and unknown-mode (`?25;1h` must not flip alt-screen).

- **`test_alt_screen_scroll_is_noop` now asserts the actual scroll contract (cas-a368).** Empty `is_err()` branch was silently passing — the test exercised `Pane::scroll` on an alt-screen pane and asserted nothing meaningful. Empirical probe showed `Pane::scroll` on alt-screen returns `Ok(())` and silently no-ops (not `Err` as the stale docstring claimed — that text was carried over from an earlier ghostty revision). Test now asserts `result.is_ok()` with a helpful failure message, keeps the existing viewport-offset equality check, and rewrites the docstring to match reality plus explain why the UI must forward wheel events to the PTY on alt-screen (host has no scrollback to give). Companion test additions in cas-72c3 pin the daemon's wheel-dispatch decision table and the exact byte shape of `SCROLL_UP_ARROWS` / `SCROLL_DOWN_ARROWS` (previously only length was asserted; a typo in the byte sequence would have silently broken wheel-to-PTY forwarding).

- **`cas-code-review` SKILL.md frontmatter no longer tells workers to autofire pre-close (cas-ec8f).** Under the v2.13.0+ default `[code_review] owner = "supervisor"`, the supervisor invokes `cas-code-review` at cherry-pick + EPIC-merge time — workers must not self-dispatch personas at `task.close`. The stale description framed `autofix` at `task.close` as "the primary path" and called this skill "the pre-close quality gate for CAS factory workers", causing workers to burn ~100K input tokens per close dispatching 4–8 reviewer personas inline. New description leads with supervisor invocation; demotes `mode=autofix` to opt-in for projects pinning `owner = "worker"`. Two regression tests pin the description contract (substring assertions on forbidden phrases + supervisor mention) and lock byte-identity between the `.claude` and `.codex` mirrors. Amendment commit also unsticks `test_cas_worker_skill_documents_code_review_gate`, which had been silently failing on main since commits 8b82273 and 167c57e (cas-8962 / cas-5815 supervisor-default flip) — replaces five stale inline-block markers with the post-flip ownership contract.

- **`FactoryApp::for_test()` documents its ~10 non-obvious fields (cas-11b0).** Expanded the constructor docstring from 3 lines to a structured field-handling note covering `Mux::new` vs `Mux::factory`, the `DirectorEventDetector.initialize` sequence, the `director_stores=None` / `worktree_manager=None` contracts, the `cas_dir`/`project_dir` placeholder warning, and the terminal-cols/rows-Mux-sync pitfall. Adds a canary clause: any new field on `FactoryApp` must also be added here, otherwise the test constructor fails to compile.

### Cross-team coordination

- **Future cloud-side enforcement of `project_id` on `/api/sync/pull` (cas-990b).** Filed `petra-stella-cloud/docs/requests/FEATURE-mandatory-project-id-on-pull.md` asking the cloud team to mirror the existing `MIN_CLIENT_VERSION` + mandatory-`project_canonical_id` gate from `app/api/sync/push/route.ts:29-57` onto both pull endpoints (`/api/sync/pull` and `/api/teams/[teamId]/sync/pull`). With this binary onwards, every `cas cloud pull` call carries `project_id=` on the wire, so the cas-side fix is the prerequisite for the cloud-side enforcement flip. **No breaking change in this binary**: a future cas-cli release will tighten the contract once the cloud-side gate is live and the `MIN_CLIENT_VERSION` constant has rolled forward past unsafe binaries. Users on this binary onwards will not be affected by the flip; users on earlier binaries will receive a clear `400` instead of silent cross-project data leakage. Defense-in-depth complement to `cas-ed15`: cas-side fix prevents *new* contamination on the wire; cloud-side enforcement guarantees that any *future* parallel pull builder regression becomes loud rather than silent.

- **Cloud `/api/sync/pull` should return specs / events / prompts / file_changes / commit_links (cas-bba4 follow-up).** Filed `docs/requests/FEATURE-cloud-sync-pull-return-specs.md` asking cloud to extend the pull response payload to include the entity-kind arrays cas-cli now consumes. cas-side ships forward-compatible (`unwrap_or_default()` on each new field), so this lands independently from the cas-cli rollout.

## [2.14.0] - 2026-05-12

### Added

#### Claude Code 2.1.122–2.1.139 changelog integration (EPIC cas-871f)

Track upstream Claude Code as it ships features and breaking changes that touch CAS surfaces. Six items shipped this release.

- **`CLAUDE_PROJECT_DIR` for `cas serve` MCP stdio project resolution (cas-7cc3, Claude Code 2.1.139).** Claude Code 2.1.139 passes `CLAUDE_PROJECT_DIR` into stdio MCP server environments. `cas-cli/src/mcp/server/runtime.rs::resolve_mcp_serve_root()` now reads it first, falling back to existing `CAS_ROOT` / cwd-walk detection when unset or invalid. Error message names `CLAUDE_PROJECT_DIR` when it points at an uninitialised directory so the user knows which path to `cas init`. Debug-level tracing logs the chosen resolution branch. 4 unit tests cover happy path, fallback on invalid path, fallback when unset, and explicit-error-mentioning-env-var on uninitialised dir; RAII `EnvGuard` ensures panic-safe env restoration. Documented in `cas-cli/docs/ARCHITECTURE.md`.

- **Hook configs converted to exec-form `args` arrays (cas-7ecd, Claude Code 2.1.139).** All 12 CAS-emitted hook entries across 10 hook types (SessionStart, SessionEnd, Stop, SubagentStart, SubagentStop, PostToolUse, PreToolUse, UserPromptSubmit, PermissionRequest, Notification, PreCompact) plus factory check-staleness now emit `"args": ["cas", "hook", "<Event>"]` instead of shell-string `"command": "cas hook <Event>"`. Eliminates path-quoting bugs when the cas binary lives at a path with spaces or shell metacharacters. `has_cas_hook_entries()` + `strip_cas_hooks()` accept BOTH the new exec form AND the legacy command form so existing user `settings.json` continues to be detected and stripped correctly on `cas init` re-run. Fallow gate hook retains shell-form (requires `$CLAUDE_PROJECT_DIR` expansion that exec form doesn't support); HTML comment in `fallow/references/patterns.md` documents the retention. 3 hook-emission test guards added (`hook_entries_emit_exec_form_args_array`, `hook_entries_no_longer_emit_command_string_form`, plus an updated `test_configure_creates_settings` fixture).

### Documentation

#### Two spike brainstorms filed for forward-looking Claude Code architecture decisions

- **`continueOnBlock` for cas-code-review autofix (cas-8655, Claude Code 2.1.139).** Spike concluded: not applicable. CAS PostToolUse hook is `async: true` with `matcher: "Write|Edit|Bash"` — it neither blocks nor matches `mcp__cas__task`. Code review runs entirely inline in the MCP `task.close` handler, so the Claude Code 2.1.139 `continueOnBlock` hook field is architecturally mismatched. Section 7 of the brainstorm flags `continueOnBlock` as potentially useful for the *PreToolUse* hook path (filesystem-write blocks, dangerous Bash) as a separate future investigation. Brainstorm at `docs/brainstorms/2026-05-12-continue-on-block-code-review-spike.md`.

- **OTEL trace propagation post-Claude Code 2.1.128 (cas-8ad7).** Claude Code 2.1.128 stopped subprocesses inheriting `OTEL_*` env vars. Spike concluded: zero impact on CAS. No `opentelemetry` crate in any workspace `Cargo.toml`; `otel.rs::OtelContext` write side fires at SessionStart but the read side is unimplemented in production; CAS emits no spans. Section 6 of the brainstorm documents forward-looking guidance for when CAS does wire OTEL export: read resource attributes from `otel_context.json` via `get_resource_attributes()`, do NOT fall back to `OTEL_RESOURCE_ATTRIBUTES` env var (CC 2.1.128 strip would break that path). Brainstorm at `docs/brainstorms/2026-05-12-otel-propagation-verification.md`.

#### `CLAUDE_CODE_PACKAGE_MANAGER_AUTO_UPDATE` for Homebrew users (cas-03c6, Claude Code 2.1.129)

README Homebrew section now points Homebrew users at Claude Code 2.1.129's `CLAUDE_CODE_PACKAGE_MANAGER_AUTO_UPDATE=1` env var for background Claude Code self-upgrades, with an explicit "this is for Claude Code only — not CAS; CAS updates via `cas update`" disclaimer to prevent the readability hazard.

#### `skillOverrides` escape hatch for CAS builtin skills (cas-2f3f, Claude Code 2.1.129)

README Claude Code Integration section documents Claude Code 2.1.129's `skillOverrides` setting as the way to hide / collapse specific CAS builtin skills without disabling CAS entirely. Three-mode table (`off` / `user-invocable-only` / `name-only`) + JSON example with real CAS skill names.

### Added (also in this release)

#### `cas update --user` — distribute built-ins to user-level (~/.claude, ~/.codex)

`cas update --sync` only writes to the current project's `.claude/.codex`. Worker worktrees that don't ship `.claude/skills/` in tracked git state (the gabber-studio case) fall back to user-level skills, so a stale `~/.claude/skills/cas-worker/SKILL.md` silently kept workers running the old multi-persona pipeline at close even after `cas-update` re-synced every project.

`cas update --user` mirrors `--sync` for built-ins only — calls `sync_all_builtins_for_harness(Claude, ~/.claude)` (and `Codex, ~/.codex` if the dir exists) without touching project-scoped config (settings.json, CLAUDE.md, hooks, db-backed rules/skills). The `cas-update` wrapper now invokes it on every run so user-level skills track binary version.

## [2.13.0] - 2026-05-05

### Changed

#### Default code-review ownership flipped from `worker` to `supervisor` (EPIC cas-cac3 / cas-b51a Stage 2+3)

**The default `[code_review] owner` is now `"supervisor"`.** Projects with no `[code_review]` block in `.cas/config.toml` now use supervisor-owned review by default — no opt-in required.

- **Workers run only the lightweight structural lint at close (<1s).** The multi-persona review pipeline is no longer invoked inline at `task.close` by default. Tasks transition to `pending_supervisor_review` after a clean lint pass; workers are immediately free to pick up the next task.
- **Supervisor runs `/cas-code-review mode=interactive` at cherry-pick time (per-task) and at EPIC→base merge (integration sweep).** See `cas-supervisor/references/workflow.md` Phase 3 step 5 and Phase 4 step 3 for the exact invocation sequence.
- **Pin to legacy behavior** with `[code_review] owner = "worker"` in `.cas/config.toml`. This restores the original inline dispatch (~14 min per close) for teams that want it.
- **`close_ops.rs` absent-section fix (cas-865b):** `.unwrap_or(false)` at the runtime close gate replaced with `.unwrap_or_else(|| CodeReviewConfig::default().supervisor_owned())` so projects with no `[code_review]` block track the config-layer default instead of being hardcoded to worker mode.
- **Skill prose updated:** `cas-worker` workflow (steps re-numbered), `cas-supervisor` workflow (cherry-pick and integration review steps added), `cas-code-review` SKILL.md (ownership table, mode reference, purpose section all reflect new default).

## [2.12.0] - 2026-05-04

### Added

#### Per-worker CLI/model/effort overrides — heterogeneous factory teams (EPIC cas-b3db)

Supervisors can now spawn workers on different AI harnesses within a single factory session. A Claude supervisor can coordinate a Codex worker (or vice versa) without restarting the daemon.

- **`mcp__cas__coordination action=spawn_workers cli=codex`** — new `cli`, `model`, and `effort` fields on the `spawn_workers` coordination action route per-spawn harness overrides through the full stack: MCP → spawn-queue (m201 migration adds `worker_spec` column) → cloud handler → daemon protocol → `finish_worker_spawn`.
- **`cas factory --worker-spec '{"cli":"codex","name":"alice"}'`** — new `--worker-spec` CLI flag resolves and persists per-worker specs at daemon boot; `WorkerSpec::codex_default(name)` constructor added.
- **`MuxConfig.resolved_worker_specs`** — `Mux` struct replaces the three scalar `worker_cli/model/effort` fields with `default_worker_spec: WorkerSpec` + `worker_specs: HashMap<String, WorkerSpec>`. `factory_pane_configs` and `add_worker` use per-worker spec lookup with fallback chain (explicit > map > default).
- **Live re-resolution at spawn time** — `sync_worker_config_from_live_settings()` called at `finish_worker_spawn` and `respawn_worker` re-reads the live `LlmConfig` from disk so `cas config set llm.worker.harness codex` takes effect without daemon restart.
- **Codex effort arg wired** — `PtyConfig::codex` now emits `-c model_reasoning_effort=<level>` when effort is `Some`; previously silently dropped.
- **Heterogeneous spawn smoke test** (`cas-5570`) — `heterogeneous_spawn` integration test in `crates/cas-mux/tests/` confirms Claude-supervisor-spawns-Codex-worker roundtrip. Supervisor skill docs updated with `cli`/`model`/`effort` parameter table and heterogeneous-team example in both `.claude` and `.codex` mirrors.

#### Supervisor-owned code-review pipeline (cas-b51a)

Moves the expensive multi-persona `cas-code-review` skill dispatch from the worker's close path to the supervisor, cutting the per-close latency cost.

- **`[code_review] owner = "worker" | "supervisor"` config knob** — new `CodeReviewConfig` section in `config.toml`. Default is `"worker"` (Stage 1 backwards-compat; Stage 2 flip is a follow-on).
- **`PendingSupervisorReview` task status** — new status value between `InProgress` and `Closed`. When `owner = "supervisor"`, a worker close that passes the lightweight lint gate transitions the task to `PendingSupervisorReview` instead of triggering `CODE_REVIEW_REQUIRED`. Worker is immediately free to pick up the next task.
- **Lightweight structural lint gate** — fast (<1s) pre-close check run by the worker on the raw diff before handing off to the supervisor. Catches `unimplemented!()`, `todo!()`, `dbg!()`, and >5-consecutive-line commented-out blocks. Lint failure returns a structured error naming the violation; the task stays `InProgress`.
- **5 integration tests** in `supervisor_review_flow.rs` covering: supervisor-mode skips `CODE_REVIEW_REQUIRED`, worker-mode unchanged, `PendingSupervisorReview` SQLite round-trip, supervisor verification on pending task, config default is `"worker"`.
- **Supervisor skill docs** (`cas-supervisor.md`, `code-review-queue.md`) updated with queue-management workflow and lint-fail response guidance.

#### Verification jail self-cert (cas-778a / cas-4c64 / cas-164c)

Clean `ReviewOutcome` envelopes now self-certify the worker close path. Workers no longer need to forward to the supervisor when `VERIFICATION_JAIL_BLOCKED` fires on a clean close — the system detects a valid envelope and clears the gate automatically. The old forwarding dance only applies on pre-2.12.0 binaries.

### Changed

#### dbg!() lint tightened + lint-fail integration test (cas-adf0 + cas-b5ac)

- **`contains("dbg!(")` replaces three-part OR** — the lightweight lint's `dbg!` check previously missed `=dbg!(...)` and `let x=dbg!(...)` (no space before `dbg`). Replaced with a single `contains("dbg!(")` that catches all forms regardless of preceding whitespace.
- **4 new unit tests** covering bare, with-space, no-space-after-equals, and embedded forms.
- **Integration test for lint-fail close path** (`test_lint_fail_close_blocked_before_pending_supervisor_review`) — asserts `is_error=true`, error names the offending lint rule, and task remains `InProgress` (no `PendingSupervisorReview` transition on lint failure).

## [2.11.0] - 2026-05-01

### Added

#### Factory close-merge enforcement (EPIC cas-754b)

Closes the silent data-loss vector where `task action=close bypass_code_review=true` could mark tasks Closed without verifying the worker's `factory/<assignee>` branch was merged into the parent epic. Field evidence from gabber-studio cas-6e07 (2026-05-01): 7 stranded tasks, ~21 commits, ~3000 LOC nearly disappeared. Second occurrence in 48h.

- **Per-task close-merge gate (cas-95ce):** `mcp__cas__task action=close` on a non-epic task now rejects when `factory/<assignee>` has commits not on the parent epic. Bypass-immune at the type level (the helper signature does not consume a bypass flag) and at the physical level (gate runs structurally upstream of `bypass_code_review` evaluation). Error names the stranded commit count, factory branch, parent epic branch, and remediation.
- **Epic-close gate (cas-8f8f):** `mcp__cas__task action=close` on an Epic-type task walks every child's factory branch and rejects when any child is stranded. Same bypass-immunity. Caught a P1 critical in autofix: the original `unwrap_or_default()` on a SQLite-backed lookup would have failed open and defeated the entire enforcement. Now propagates as `INTERNAL_ERROR`.
- **`mcp__cas__coordination action=epic_status id=<epic-id>` diagnostic (cas-8f8f):** new callable surface returning a markdown table per child task (assignee | factory branch | unmerged count | last commit | task ID + status). Useful for in-flight audits before attempting epic close.
- **`cas-supervisor-checklist` skill update (cas-8f8f):** "Before Closing an EPIC" section now references `epic_status` as the canonical check and notes that the gate is automatic (defense-in-depth, no longer manual-only).

### Changed

- **`mcp__cas__verification action=add` authz error (cas-a90f3):** the misleading "Supervisors can only verify epics, not individual tasks" rejection has been replaced with a message that names the actual rule (active-assignee-based) and lists the three exemptions (orphaned / inactive assignee / supervisor IS the assignee). Error embeds the offending assignee ID, gives concrete remediation (`mcp__cas__task action=release`), and clarifies that epics remain always supervisor-verifiable. Predicate renamed `assignee_inactive` → `assignee_inactive_or_absent` to make `unwrap_or(true)` semantics self-documenting (logic unchanged).

### Operator guidance

After upgrading, the new gates fire on `task.close` calls. If a worker hits the gate during close, the supervisor must merge `factory/<assignee>` into the parent epic before the close will succeed (this is the desired ordering and matches how the other workflow guidance now reads). For pre-existing stranded factory branches (e.g. gabber-studio cas-6e07), salvage with: `git checkout <epic-branch> && git merge --no-ff factory/<worker>`.

## [2.10.1] - 2026-04-29

### Changed

- **Shared proxy transport (cas-36fd0):** new `cli/integrate/proxy.rs`
  module exposes `ProxyClient` with the proxy lifecycle (`proxy_config_path`,
  `call`, `block_on`, `unwrap_envelope`). Both `ProxyVercelClient` and
  `LiveNeonClient` are now thin wrappers — ~165 LOC of duplicated boilerplate
  retired. Future `Live<X>Client` implementations inherit the wiring.
- Speculative neon parser tolerance shapes (`orgs/data` alias, flat
  `describe_project`) removed until proven against real envelopes; bail
  messages cite cas-36fd0 and request bug filing on real upstream drift.
- `default_database` "neondb" silent fallback → explicit bail with
  provisioning recovery hint.

## [2.10.0] - 2026-04-29

### Added

#### Vercel/Neon/GitHub Auto-Integration (EPIC cas-b65f)

- `cas integrate <vercel|neon|github> [init|refresh|verify]` standalone subcommands.
  - **Vercel**: detects `vercel.json` / `@vercel/*` deps, fuzzy-matches via
    `mcp__vercel__list_projects`, captures team + project + env→branch mapping.
  - **Neon**: detects Prisma + `@neondatabase/*` / `@prisma/adapter-neon`, prompts
    for org when multiple exist, captures `org_id` + `projectId` + `databaseName` +
    branches via `mcp__neon__{list_organizations,list_projects,describe_project,describe_branch}`.
  - **GitHub**: parses `git remote -v` (https + ssh forms), records `owner/repo`.
- `cas init` runs platform detection and prompts Y/N per detected platform,
  delegating to the corresponding `cas integrate <platform> init` in-process.
  Idempotent on re-run: existing populated SKILL.md flips the prompt to
  "Refresh? [y/N]" with default N.
- `--no-integrations`, `--vercel <id>`, `--neon <id>`, `--github <repo>` flags
  for non-interactive `cas init` use.
- Generated SKILL files land in **both** `.claude/skills/<name>/` and
  `.cursor/skills/<name>/` so both harnesses pick them up.
- `<!-- keep <name> -->` … `<!-- /keep <name> -->` named keep blocks preserve
  user-owned IDs across `refresh` regenerations. `--update-ids` opts into
  re-fetching IDs from the platform MCP.
- `<!-- cas:full_name=... -->` identity tag convention for canonical project
  identity inside keep blocks; sanitized to neutralize markdown injection.
- `cas doctor` audits integration freshness via per-platform `verify_report`
  and surfaces stale IDs as warnings (not errors); MCP-down reports as
  `skipped — MCP not configured` rather than failing the doctor run.
- Optional opt-in `[integrations] session_start_warn = true` in
  `.cas/config.toml` emits a low-severity SessionStart banner when integrations
  go stale. Default off — preserves the codemap banner's signal.

#### Codemap Skill (cas-4d84)

- `/codemap` skill ships in `.claude/skills/codemap/`, builtins, and codex
  variant. Generates `.claude/CODEMAP.md` and resets the freshness counter
  via `cas codemap clear` after writing. Closes the long-standing gap where
  hooks referenced a `/codemap` slash command that did not exist.

### Changed

- `mcp-proxy` is now a default Cargo feature so `cas integrate vercel|neon`
  ships out of the box — the wired `ProxyVercelClient` / `LiveNeonClient`
  require it.
- `cas cloud team-memories`'s "no team configured" error now correctly
  directs users to `cas cloud team set <uuid>` (previously referenced a
  non-existent subcommand with `<slug>` argument).
- `cas cloud team set|show|clear` subcommands to configure the active team
  (UUID input; slug resolution deferred pending cloud-side endpoint).
- `cas memory share <id>|--since <duration>|--all [--dry-run]` for retroactive
  backfill of pre-existing personal memories to the team push queue.
- `cas memory unshare <id>` to mark a memory `share=Private` (blocks future
  team dual-enqueue; does not retract cloud-side copies).
- `share: Option<ShareScope>` (`Private`/`Team`) persisted on Entry, Rule,
  Skill, and Task via SQLite migrations `m037`/`m060`/`m082`/`m121`.
- Automatic dual-enqueue: when a team is configured via
  `cas cloud team set`, `cas memory remember` in any Project-scoped
  non-Preference context queues the entry to both personal and team
  push queues. `cas cloud sync` drains both.
- Coarse kill-switch: `cloud.json.team_auto_promote: false` disables the
  automatic promotion without requiring the team to be cleared.
- Integration test suite: `team_sync_test.rs`, `memory_share_test.rs`,
  `team_memories_e2e_test.rs` cover the full push → pull pipeline.

- `cas cloud team-memories`'s "no team configured" error now correctly
  directs users to `cas cloud team set <uuid>` (previously referenced a
  non-existent subcommand with `<slug>` argument).

#### Factory Skill Bundles (cas-61af)

- `cas-supervisor.md` split from 44 KB into a 6.8 KB SKILL.md + six
  references (`preflight`, `intake`, `planning`, `workflow`,
  `worker-recovery`, `reference`).
- `cas-worker.md` split from 22 KB into a 5.7 KB SKILL.md + three
  references (`close-gate`, `recovery`, `details`).
- `supervisor_guidance()` and `worker_guidance()` no longer bundle
  `cas-task-tracking`, `cas-memory-management`, or `cas-search` — those are
  autonomous skills the agent invokes via the Skill tool. Bundled payload
  dropped from ~61 KB / ~35 KB to ~10 KB / ~5.5 KB respectively.
- Test ceiling at 12 KB enforces the bundle stays small enough that the
  Claude Code harness does not truncate the SessionStart additionalContext
  to a 2 KB preview.

#### Cross-cutting Hardening (cas-fc38)

- New `cli/integrate/fs.rs` shared module: `atomic_write`,
  `atomic_write_create_dirs`, `read_capped` (4 MiB cap with symlink
  rejection), `is_regular_file`, `locate_repo_root[_from]` (with `git -C`
  discipline that resolves the inner repo on submodule / nested-worktree
  invocations).
- New `cli/integrate/md.rs` shared module: `escape_md_cell`,
  `escape_md_cell_code`, `emit_cas_full_name_tag`, `parse_cas_full_name_tag`.
- `IntegrationStatus` split: `TransportError` distinct from `Stale` so a
  failed MCP call is no longer misreported as a stale ID.
- All three platform handlers consume the shared helpers — atomic-write
  semantics, symlink defense, file-size cap, markdown escaping, and
  identity tag behave uniformly.

#### Team Memories

- `cas cloud team set|show|clear` subcommands to configure the active team
  (UUID input; slug resolution deferred pending cloud-side endpoint).
- `cas memory share <id>|--since <duration>|--all [--dry-run]` for retroactive
  backfill of pre-existing personal memories to the team push queue.
- `cas memory unshare <id>` to mark a memory `share=Private` (blocks future
  team dual-enqueue; does not retract cloud-side copies).
- `share: Option<ShareScope>` (`Private`/`Team`) persisted on Entry, Rule,
  Skill, and Task via SQLite migrations `m037`/`m060`/`m082`/`m121`.
- Automatic dual-enqueue: when a team is configured via
  `cas cloud team set`, `cas memory remember` in any Project-scoped
  non-Preference context queues the entry to both personal and team
  push queues. `cas cloud sync` drains both.
- Coarse kill-switch: `cloud.json.team_auto_promote: false` disables the
  automatic promotion without requiring the team to be cleared.
- Integration test suite: `team_sync_test.rs`, `memory_share_test.rs`,
  `team_memories_e2e_test.rs` cover the full push → pull pipeline.

## [2.0.0] - 2026-04-12

### Added

#### Factory System

- Multi-agent factory with supervisor/worker architecture and isolated git worktrees.
- Director event system for task dispatch, worker lifecycle, and epic completion notifications.
- Worker startup confirmation flag to detect crash-on-startup failures.
- Orphaned task reclamation — supervisor can claim tasks from dead workers.
- Coordinator messaging system with priority levels, delivery confirmation, and outbox replay.
- Verification jail exemption for factory workers to prevent universal tool blocking.
- Worker idle/stale notification dedup and suppression.
- Minions theme with ASCII art and themed boot screen for factory workers.

#### Cloud Sync

- Bidirectional cloud sync with Petra Stella Cloud — push/pull tasks, memories, rules.
- Cloud sync queue with shutdown drain, startup push, 10s idle gate, 60s interval.
- Circuit breaker for TLS retry spam with capped event buffer.
- `cas cloud projects` and `cas cloud team-memories` commands.
- `cas cloud purge-foreign` for orphaned dependency cleanup.
- Project-scoped pull requests to prevent cross-project data leaks.

#### MCP Proxy

- `cas-mcp-proxy` crate — proxies upstream MCP servers (Playwright, Neon, GitHub, Vercel, Context7) through CAS. Workers get 2 tools instead of 50+.
- Config-aware hot-reload for proxy server connections.
- Search with keyword matching and server filtering.
- Integration tests, catalog caching, and README.

#### TUI

- Tokyo Night theme variant.
- OSC 52 clipboard copy and auto-inject on image paste.
- `cas open` interactive TUI project picker.
- Tab forwarding to PTY for autocomplete (Ctrl+P for sidecar).
- Clipboard fallback via client-side write with visual feedback.
- Mouse click to focus panes, Ctrl+Arrow pane cycling, Shift+drag text selection.
- Native terminal selection (replaces custom selection implementation).

#### Compound Engineering

- `cas-code-review` skill — multi-persona code review with 7 reviewer personas (correctness, testing, maintainability, project-standards + conditional security, performance, adversarial). Includes bounded autofix loop, confidence gates, fingerprint dedup, and review-to-task routing.
- `cas-brainstorm` and `cas-ideate` skills for structured ideation.
- `git-history-analyzer` and `issue-intelligence-analyst` agent types.
- Multi-persona review merge pipeline with cross-reviewer agreement boost.
- Pre-insert memory overlap detection with configurable threshold actions.
- Implementation Unit Template for EPIC subtask specifications.
- `execution_note` field on tasks: `test-first`, `characterization-first`, `additive-only` postures with enforcement at close.

#### Skills & Agents

- Comprehensive `cas-worker` skill with build failure triage, MCP connectivity guidance, tool selection guide, context exhaustion detection, task reassignment protocol, and section reorder for critical-path-first flow.
- Adversarial supervisor posture with intake gate, scope lock, and rejection authority.
- Partnership posture for supervisor — counter-propose, trajectory gate, situational awareness.
- `cas-supervisor` skill with EPIC sizing heuristics, worker failure recovery, and merge conflict guidance.
- `cas-memory-management` skill with multi-file schema and overlap workflow.
- `cas-search` skill with filter grammar, code symbol search, and module-scoped candidate API.
- CODEMAP system — auto-maintained breadcrumb navigation map with structural change detection hooks.

#### Infrastructure

- Hetzner CCX23 provisioning script for remote CAS server (Ashburn VA).
- Slack bridge: Bolt app scaffolding with per-user daemon architecture, SSE adapter, message formatter, file upload passthrough with security sanitization.
- `cas-install.sh` — portable curl one-liner installer.
- WebSocket endpoint for factory daemon.
- SSE plain-text pane output and tail endpoint.
- Auto-attach prompt with `--attach`/`--new` flags for existing sessions.
- `cas serve` HTTP bridge for Slack integration.

#### Store & Performance

- Sequence table for ID generation (replaces per-insert MAX+LIKE scan).
- SQLite `prepare_cached()` for all statement caching.
- Jitter on SQLite write-retry backoff to break convoy pattern.
- Recursive CTE for dependency cycle-check (replaces iterative BFS).
- Tantivy IndexWriter caching (saves 50MB per write allocation).
- BM25 search index caching and QueryParser reuse.
- Batch code symbol DB inserts in indexing daemon.
- `ImmediateTx` wrapper for atomic store operations.

### Changed

- Bumped version to 2.0.0 with simplified release workflow targeting `pippenz/cas`.
- Config format migrated from YAML to TOML (automatic merge of stale settings).
- `project_canonical_id` derived from folder name instead of git remote URL (required on all cloud pushes).
- Default cloud sync interval reduced from 300s to 60s.
- MCP tool prefix standardized to `mcp__cas__`.
- Worker skill reordered for critical-path-first flow: Task Types and Execution Posture before close procedures.
- Code review section compressed from 65 to 30 lines — pipeline internals moved to `cas-code-review` skill.
- Rules section merged into Rules of Engagement; Valid Actions merged into Schema Cheat Sheet.
- Legacy `code-reviewer` agent deprecated in favor of `cas-code-review` skill.

### Fixed

- **TUI**: Off-by-one in Ghostty VT style run column indices clipping left edge of pane content. Tab click detection using variable-width positions instead of equal-width assumption. Scroll viewport double-compensation when Ghostty preserves viewport position. Task panel flashing empty due to read race between task list and dependency queries. Dark theme contrast — `border_default`, `border_muted`, `hint_description` bumped for readability. Epic state updated before filter in `refresh_data()`.
- **Factory**: Verification jail cascade where one task's pending verification blocked all tools. `CAS_FACTORY_MODE` phantom env var — `pre_tool.rs` required it alongside `CAS_AGENT_ROLE` but no code ever set it. Director dispatching blocked/closed tasks (terminal-status guard added). Supervisor self-verification deadlock. Worktree workers missing MCP access due to gitignored `.mcp.json`/`.claude/` (fixed with symlinks). Duplicate hooks causing PreToolUse errors (`cas hook cleanup` added).
- **Cloud**: WebSocket TLS for `tokio-tungstenite`. HTTP TLS for `ureq` client. Fallback `project_id` for filesystem-root CAS projects. 403/404 error handling with pluralized labels.
- **Store**: N+1 queries in `task_store.rs`. Unbounded `IN` clauses and `LIKE` scans. 8 excessive indexes dropped to reduce write amplification. Lease races and cleanup/prune methods with transaction safety.
- **Close**: Additive-only gate now diffs worker branch commits (not main). Skip close-gate checks for non-isolated tasks. Reject close when worker tree has uncommitted work. Status-update race condition where `status=blocked` overwrites concurrent supervisor close.
- **Other**: `rustls` CryptoProvider installed at startup to prevent daemon crash. Secrets moved from provision script to `~/.config/cas/env` (push protection). GitHub auth token used in self-update to avoid API rate limits.

## [1.0.0] - 2026-03-12

### Added

- Initial open-source release of CAS.
- Factory TUI screenshot in README.
- `.env.worktree.template` for worker environment setup.

### Changed

- Release workflow updated for GitHub Actions with Homebrew auto-update.
- MCP config sync added to `cas update` flow.

### Fixed

- Migration v165 crash when `verifications` table doesn't exist.
- Release workflow secret check moved from job-level to step script.

## [0.6.2] - 2026-02-25

### Added

- Interactive terminal dialog (Ctrl+T) in factory TUI with show/hide/kill.
- MCP proxy catalog caching for SessionStart context injection.
- Billing interval switching buttons (monthly/yearly) with savings display.
- Resume subscription button on cancellation notice.
- `cas changelog` command to show release notes from GitHub releases.

### Changed

- Cloud sync on MCP startup runs in background with 5s timeout (non-blocking).
- Heartbeat uses shorter 5s timeout and spawn_blocking to avoid stalling async loop.
- Refactored cloud routes: org_billing_settings → billing_settings, org_members → members.
- Release bump workflow now requires a matching CHANGELOG.md section.

### Fixed

- Debounced Ctrl+C interrupt to prevent accidental double-sends.
- Update version check now compares versions properly.
- Stripe portal return URL redirects back to billing page instead of settings.
- Removed duplicate type export in types/index.ts.

## [0.5.7] - 2026-02-15

### Fixed

- Avoided macOS factory startup crash by using subprocess daemon mode with attach/socket retries.
- Hardened UTF-8-safe truncation behavior in touched UI/tooling paths to prevent char-boundary panics.

### Changed

- Standardized release-train crate versions to `0.5.7`.

## [0.5.6] - 2026-02-15

### Fixed

- Cleared clippy warnings under `-D warnings` across touched workspace crates.

### Changed

- Standardized release-train crate versions to `0.5.6`.
- Updated local git hook rustfmt invocation to use Rust 2024 edition.

## [0.5.5] - 2026-02-15

### Changed

- Published `0.5.5` release and synchronized release-train crate versions.

## [0.5.4] - 2026-02-15

### Changed

- Improved Supabase auth login UX and callback branding.

## [0.5.3] - 2026-02-15

### Changed

- Initial release carrying Supabase auth login UX and callback branding improvements.

## [0.5.2] - 2026-02-13

### Changed

- Bumped release-train versions to `0.5.2`.

## [0.5.1] - 2026-02-11

### Fixed

- Fixed Sentry transport panic triggered during `cas login`.

## [0.5.0] - 2026-02-11

### Fixed

- Added missing Sentry transport feature to prevent login-time crash.

## [0.4.0] - 2026-01-10

### Added

- Consolidated MCP tool format with unified naming.
- Sort and task type filtering for MCP and CLI.
- ID-based search and CLI/MCP feature parity.
- Git worktree support for task isolation.
- Schema migration system for database upgrades.
- Verification system with task-based exit blocking.
- Statusbar anchoring support.

### Changed

- Extracted `cas-core` and `cas-mcp` crates for better modularity.
- Removed `#[tool_router]` macro from CasCore for compile-time improvement.
- MCP enabled by default in `cas init --yes`.
- Removed legacy MCP mode and added `list_changed` notifications.

### Fixed

- Removed duplicate store implementations from `cas-cli`.
- Fixed scope persistence in crate extraction.
- Task verifier now uses CLI and checks project rules.

## [0.3.0]

### Added

- Initial stable release with core functionality.

[Unreleased]: https://github.com/Richards-LLC/cassy/compare/v3.7.5...HEAD
[3.7.5]: https://github.com/Richards-LLC/cassy/compare/v3.7.4...v3.7.5
[3.7.4]: https://github.com/Richards-LLC/cassy/compare/v3.7.3...v3.7.4
[3.7.3]: https://github.com/Richards-LLC/cassy/compare/v3.7.2...v3.7.3
[3.7.2]: https://github.com/Richards-LLC/cassy/compare/v3.7.1...v3.7.2
[3.7.1]: https://github.com/Richards-LLC/cassy/compare/v3.7.0...v3.7.1
[3.7.0]: https://github.com/Richards-LLC/cassy/compare/v3.6.0...v3.7.0
[2.13.0]: https://github.com/pippenz/cas/compare/v2.12.0...v2.13.0
[2.12.0]: https://github.com/pippenz/cas/compare/v2.11.0...v2.12.0
[2.11.0]: https://github.com/pippenz/cas/compare/v2.10.1...v2.11.0
[2.10.1]: https://github.com/pippenz/cas/compare/v2.10.0...v2.10.1
[2.10.0]: https://github.com/pippenz/cas/compare/v2.0.0...v2.10.0
[2.0.0]: https://github.com/pippenz/cas/compare/v1.0...v2.0.0
[1.0.0]: https://github.com/pippenz/cas/compare/v0.6.2...v1.0
[0.6.2]: https://github.com/pippenz/cas/compare/v0.5.7...v0.6.2
[0.5.7]: https://github.com/pippenz/cas/compare/v0.5.6...v0.5.7
[0.5.6]: https://github.com/pippenz/cas/compare/v0.5.5...v0.5.6
[0.5.5]: https://github.com/pippenz/cas/compare/v0.5.4...v0.5.5
[0.5.4]: https://github.com/pippenz/cas/compare/v0.5.3...v0.5.4
[0.5.3]: https://github.com/pippenz/cas/compare/v0.5.2...v0.5.3
[0.5.2]: https://github.com/pippenz/cas/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/pippenz/cas/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/pippenz/cas/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/pippenz/cas/compare/v0.3.0...v0.4.0
