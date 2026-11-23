# Cassy - Product Overview
>
> Auto-generated domain snapshot. Regenerate with /project-overview when the domain model drifts.

## Pitch

Cassy is a local-first multi-agent coding factory and persistent context system for developers working in Git repositories.
Workspace write refusals name the configured artifact root and its canonical target; logical and canonical in-root writes share containment checks, and symlink escapes remain refused.
The harness session scratchpad permits directory creation and file writes through Bash and file tools; refusals describe the same session-bound path pattern used for admission.
The `cas` binary runs a supervisor with isolated workers; `cas serve` gives coding agents durable project context through MCP.
Initial MCP startup validates the store and reports readiness before the client handshake; replacement backends stay inactive until the serving connection can switch safely.
Long project and QA sandbox paths use a bounded private embedded-daemon socket, so native caller discovery, reminders and handoffs retain the same isolated store and process ownership checks.
On Unix, new `cas serve` connections keep a stable stdio front while compatible installed backends change. A candidate negotiates privately before the front drains both request directions and partial messages; failed startup, changed protocol/capabilities/tool schemas or a drain timeout keep the current backend serving. Deferred reasons are logged, including the changed tool for a schema mismatch. Startup and compatibility deferrals wait for another install; a drain timeout retries the same binary at a later idle boundary. Serving reports distinguish the backend version from the front version; existing connections started before this transport need one normal harness reconnect to adopt it.
Tasks, memories, rules, skills and repository knowledge live in one local store; Cloud sync, the machine hub and the Commander browser app are optional.
Every delivery is checked before it merges: tests, the journeys a change touches, documentation upkeep and, after release, a live post-release check.

`cas doctor` also identifies guidance that still explicitly names an absent or
disabled MCP server, listing record IDs and file locations for review while
preserving the original rules, skills, memories and brief templates.

Projects declare their main journeys in the existing QA registry. Finding priority comes from severity × reach: reproduced broken main journeys precede theoretical code-read findings, which go to P4 backlog. The theoretical exception is money/data loss on a declared main journey (P2). Staffing theoretical work while a main-journey FAIL is unowned produces an advisory nudge. `factory.focus_mode` follows every worker/reviewer brief; see [triage](qa/triage.md).
An update requesting a priority that conflicts with finding labels returns a typed refusal before saving any accompanying fields; changing the labels reclassifies the finding.

Cassy declares supervision, merge, close, release, spawn and QA receipt as primary factory/CLI journeys alongside Commander. Correcting a finding's severity, reach or journey label replaces that axis; clearing user labels retains finding provenance and resets triage to theoretical, preserving the derived-priority policy.

Main-reach finding corrections require a declared live main journey before any task fields change. Corrections reuse the recorded runtime reproduction, and update replies show the resulting priority even when the request changes nothing.

Managed project updates recognize the owned content Cassy previously wrote, including uncommitted generated settings. A genuine conflict preserves only that path in the working tree and index while other settings and skills refresh. The update summary and SessionStart name the preserved files for review.


Test authoring uses two automatically discoverable skills: cas-writing-tests for general quality and cas-playwright-tests for browser/e2e/journey specs. Browser specs bind their title to a final user outcome, retain meaningful red controls and honest execution evidence, and share one target/auth/config/locator policy. Audit checks those promises; debug owns failure diagnosis and QA owns evidence. Old TDD/Nuxt names redirect for one release.

## Personas

- **Operator** - the repository maintainer: starts factory sessions, picks the work, reviews delivery evidence, approves releases and can answer from Commander.
- **Supervisor agent** - plans epics, assigns one task at a time to workers, merges pushed deliveries once CI is green and the plan target is met, and coordinates with supervisors on other projects or machines.
- **Worker agent** - executes one assigned task in its own worktree, runs the targeted tests, and pushes a committed result for review. Reopening the same task after a review hold preserves its validated source boundary and uncommitted work, including when the target advanced; changed ownership or source and foreign dirty trees still refuse.
- **Browser tools** - Repository MCP clients use the hub-web lockfile’s bundled Playwright in isolated, headless managed Chromium contexts, with captures under checkout artifacts and explicit storage-state opt-in. Worker launchers keep the host HOME without injecting a shared browser profile; bundled MCP sidecars remain separate from running Playwright tests. Committed test agents include the planner and generator; healer trials stay in scratch with a reviewed patch.
- **Worker Git scope** - history writes inside shell command strings inherit the effective directory and must remain in the assigned worktree on an allowed branch. Child shell directory changes stay local; `eval` changes the enclosing shell directory. Literal script-file calls admit arguments and redirects, like direct script execution. Syntax-check calls admit only exact raw `-n` or `--noexec` flags followed by raw script paths matching `[A-Za-z0-9._/][A-Za-z0-9._/-]*`, with no redirects or other options. Quotes, escapes and shell expansions are refused. Command strings require plain `-c <command>` with optional `--` after `-c` and optional leading `-l` or `--login`. Other invocation forms, missing command strings and excessive nesting are refused.
- **QA reviewer** - walks the journeys a delivery touches on the real build and records an approve or reject verdict in a QA ledger. Walks share a project-pinned, versioned harness: wait for actual visible replies, open delivered artifacts and record one row per defect across surfaces. New schema-2 ledgers require cited source lines and captured visible, terminal or HTTP results; database read-back only corroborates. Existing ledger receipts retain their original schema. Batch ledger init accepts `--covers` for every intended receipt delivery; check with the same IDs before publishing. QA and release receipts enforce declared coverage.

## Core Concepts

A new Claude supervisor confirms its startup only after the native transcript records a turn. If its startup text remains unsent in the composer, Cassy retries Enter once; an unavailable turn signal ends as unconfirmed after a bounded observation window.

- **Project identity** - `cas cloud project set <canonical-id>` owns the canonical project pin; `cas config set project.canonical_id` points to that command and leaves the configuration unchanged.
- **Provider account** - a Claude or Codex subscription identified by provider account and organization/workspace IDs, displayed by login email; `cas accounts list` shows usage, absolute resets and duplicate or login findings while legacy folder names remain launch aliases.
- **Queued source boundaries** - a previously blocked task reassigned through the queue records its fresh target branch and base before its first delivery. The former branch stays intact; pending delivery evidence, prepared successor work and dirty checkouts prevent automatic replacement.
- **Worker task queue** - an ordered backlog for one worker. Closing or parking starts the next eligible task on its own target branch in the current conversation by default. Opt in with `factory.restart_worker_between_tasks=true` to recycle between tasks while keeping the name, worktree and launch recipe; automatic restarts default off. MCP results report measured context occupancy at the configured threshold; unavailable harness accounting is labeled unknown. Supervisors inspect, reorder or clear pending work, and stale entries carry skip notes. Queue inspection and worker status derive Current from the worker's task in progress and list blocked tasks as Held, preserving held queue positions.
- **QA candidate** - an exact-revision binary for a QA walk. A worker requests one for its own active task, and the factory daemon builds it, one at a time per machine, and messages the worker the hash and receipt. Each revision keeps its own directory, so a later build never replaces a cited one.
- **Artifact publication and revocation** - ordinary files in registered factory checkouts nested under the project's `.cas` can be published directly; checkout credentials, Git internals and other `.cas` state remain protected. Peers can fetch a committed proof by task and artifact id before its task notes are cached locally, including on large accounts. A registered supervisor can revoke an artifact with a reason. Its audit tombstone remains visible while local fetch, previews and new shares stop immediately. Cloud removal is confirmed separately; an unsupported or unavailable Cloud endpoint reports that bytes may still be retrievable until revocation completes.
- **Structured Slack posts** - Violet and Slack-formatting guidance default to status, release-note thread and decision layouts with titles, fields, compact lists and native tables. Every layout keeps a readable text fallback; project release rubrics still own audiences and thread order.
- **Slack thread receipts** - `cas violet thread --json` returns one result whose success requires a confirmed root and every reply, including file attachments. Readability refusals identify the rejected part and preserve the indices and receipts of earlier posts. Local draft checks run before connecting; the hub owns its remaining readability rules.
- **Managed Slack file replies** - Artifact sharing and `cas violet post` attach files under the caller's live reply intent, including files above the inline limit. External upload keeps the same claim and request identity at begin and complete. Locally known managed threads without a current caller intent are refused before upload; Violet checks remote authority and the exact target.
- **Source Slack status** - The source claim's accepted owner keeps status-only reaction authority through staging, including after reply-claim expiry or release; a subsequently observed takeover stops that owner. The factory reconciles each source post across its tasks: eyes acknowledges, pick means work remains, and a green check requires closed tasks with recorded deliveries contained in remote staging or main. Violet updates retain partial receipts and retryable failures. `cas violet reactions` audits desired and observed marks, pending failures and snapshot freshness; `--retry` rearms failed or terminal-confirmed updates without posting from the CLI. Confirmed terminal posts stop polling until task state changes. Reconciliation follows mention admission and sweeps, shares a five-second deadline, and decodes only unconfirmed Slack-linked task groups. Message and reply authority still require their own live claims.
- **Parallel QA surfaces** - channel/viewport walks run as bounded independent jobs with separate ledgers, browser profiles and personas. Worker status and the director report flat ledger output at the existing stall threshold independently of native execution, and suggest splitting the last focused-epic QA lane when an eligible worker is idle.
- **Pane input diagnostics** - attach snapshots expose per-pane bytes accepted by the input queue and physically written to the PTY. Input sent before an attach client closes is drained even when concurrent output can no longer reach that client; physical writes do not prove child consumption.
- **Factory session** - a live supervisor and its worker fleet, coordinated through the terminal UI. A supervisor exit prints each worker's task, dirty and unpushed work, confirmed stop and task recovery after leaving the terminal UI, and retains that evidence in the session-end receipt. Only confirmed stopped workers have unfinished task bindings released; unverified stops retain their recovery evidence. Shell hooks refuse pattern-based process signals for every factory role, including native subagents inheriting the supervisor role; agents signal the PID they started.
- **Live daemon updates** - `cas factory reexec --session <name>` loads the installed build in the same daemon PID. Panes, child processes, scrollback, drafts and recordings survive; clients reconnect. A shared daemon and app refusal lists active and queued worker actions alongside cleanup, with each known worker, occurrence and elapsed time; unavailable metadata is explicit. Recovery is a full factory restart once workers are idle, keeping cleanup threads and worker-name reservations intact until completion or shutdown. A schema preflight refuses incompatible builds before quiescing, and failed adoption returns once to the pinned previous image. `cas update` requests each live daemon update after its hub step and reports every outcome. Daemons predating handoff support may ignore that request; one sixty-second deadline reports relaunch required even while panes keep printing, without stopping those panes. A hub explicitly stopped with `cas hub stop` stays stopped through updates and install-on-publish; an explicit start, restart or service install resumes automatic recovery.
- **Automatic release installation** - a pending post-release notice does not prevent installing the latest publication. Notice attempts time out after 45 seconds, retain a receipt for each version and retry after installation; release Git probes also have a deadline. Retries reuse existing notice and release receipts.
- **Parallel QA registry updates** - Factory merges retain independent inline bug families and the newest registry date. Competing edits to a family or registry policy still require conflict resolution.
- **Worker commit directories** - the commit guard follows relative directory changes and prior literal path variables to inspect the branch where Git will run. An unresolved directory or unreadable Git identity reports uncertainty; it does not claim HEAD is detached. Non-committing checkout and switch forms can move HEAD from a detached historical tip. Forced branch-ref resets, path restores and commits keep their existing branch/scope checks; replay-branch creation retains its separate navigation allowance.
- **Final delivery proof** - a worker's first park checks the pushed source tip identified by its recorded task start. An earlier red-test commit does not select the changelog or test receipt; existing parked deliveries retain their ownership and anchor protections. Worker starts prefer the published origin target over unpublished local epic commits. A cancelled task explicitly superseded by another task transfers its recorded unmerged delivery commits to that successor in the same repository; unrelated commits retain their ownership.
- **Blocked worker resume** - Unnamed WIP on a task's recorded branch survives a blocked/open transition in linked checkouts that inherit their project identity from the primary repository. The recorded assignee and target must still match, and foreign task commits refuse the start.
- **Supervisor session registration** - Registering a new harness session for the same live supervisor and pane updates its existing identity, keeping one Commander mirror. Registration that outlives its response budget reports that work continues and delivers its final outcome to the caller's inbox with a continuation ID.
- **Worker respawn notes** - Keep `HANDOFF.md` in the task's artifacts directory. Recycling preserves an authored checkpoint or writes the held task's current notes there; the resume brief names that path. Parking refuses a delivery containing a repo-root `HANDOFF.md`, including one retained from an earlier push.
- **Release closeout** - A release retains one durable outcome for its version and shipped commit, stamped with the reconciling binary version. Binaries older than the release tag record a stale refusal without closing tasks; a qualified binary replays a legacy or older-binary outcome once, then reuses the result. Recovery reuses completed closes, verifies finished epics and trains through their ordinary gates, and leaves every remaining task, acceptance check and sync obligation named. Parent review requests use the recorded release identity even when the CLI process has no registered session. Review ledgers resolve from synced artifact references and verified Cloud downloads on the consuming machine, with project ownership and immutable receipt bytes checked before closure. Publication adapters consume one stable notice intent; publishing bytes and finishing closeout remain distinct outcomes.
- **Release train succession** - After a train lands, new standalone tasks join its successor while release reconciliation finalizes the landed train.
- **Release triggers** - A cas-src publication (`cas release published`, run by the install-on-publish timer after installing and verifying the new binary) and a supervisor's staging-to-main promotion PR merge (which starts `cas release promoted --pr <N>` in the background) each reach the release's durable closeout. Pending older publications retain separate receipts and retry after the newest install. Installed notices stop after ten attempts, retain their failed receipt and send one keyed failure record to the release's owning supervisor. Repeating `cas release published` with the saved publication JSON resumes a held operation using the installed binary; successful closes stay recorded and a completed operation is a no-op. `cas release installed` and `cas release backfill` are recovery triggers on that same operation; nothing closes a train directly. One supervisor notice names every remaining task, held epic, exception and unfinished issue with owner and next action. `cas qa ledger init --release-scope` writes the transformation and epic blocks into the release review ledger it publishes.
- **Release closeout outcomes** - An approved premerge candidate can close its delivery when the immutable manifest binds its source anchor and the reviewed tree is in the published ancestry; missing verdicts remain held. Notices and CLI summaries distinguish members closed by the operation, already closed, held and not eligible, alongside live residuals and the persisted completion flag. Before push or enqueue, release cuts preview the exact candidate's member decisions and refuse a nonempty cut with zero closable members unless every member is already closed. Cut receipts retain per-reason counts, explicit `REVIEW_MISSING` task IDs and candidate build seconds. `cas release preview --scope <file>` replays a historical scope without store writes. The durable notice context carries the same typed counts; retries retain one notice per operation.
- **Release issue completion** - After published deliveries close, reconciliation closes their GitHub issues only when every referencing task is closed and shipped. Comments name the version, commit and tasks; partial fixes and GitHub failures stay named in the release receipt, including offline failures.
- **Empty checkpoint ownership** - Inherited project identity alone does not attribute an unnamed empty checkpoint. A checkpoint without file changes still needs its own task reference or recorded commit receipt before the worker can resume it.
- **Close proof claims** - External completion assertions and promises of future proof require a nearby receipt. Local summaries may name files and identifiers or describe complete documentation blocks; refusals name the offending clause and classification.
- **Worker boot diagnostics** - a missing Claude SessionStart canary retains bounded, redacted startup output in its failed spawn record. Debug and liveness inspection accept the worker or spawn request even without registration; liveness stays unverified, and only completed teardown releases the matching task binding.

- **Worker spawn admission** - isolated MCP spawns choose a free factory branch name before queueing or task binding; an explicit colliding name is refused with its branch ref. `factory.codex_home` supplies the local default Codex account when `config_dir` is omitted and names its source in the receipt; explicit overrides win. Account refusals distinguish missing default homes, logged-out accounts and configuration mismatches, with the existing account-selection explanation included. Codex lanes check the selected account's model catalogue before queueing and use a supported registered fallback with a warning. Stale or missing catalogue evidence refuses a lane before a worker or worktree is created; refresh that account with the installed Codex before retrying. Explicit unsupported recipes still refuse. A worker whose latest terminal turn failed authentication or model access is excluded from idle staffing until a later successful turn.
- **Peer wake feedback** - a peer message held by the hourly wake cap sends its author one hold notice with the earliest cap retry time. Substantive linked replies bypass the cap and debounce; explicit quiet and receipt intent remains quiet. Feedback uses the existing durable mailbox outbox.
- **Requested worker replies** - the first settling worker reply to its own supervisor's explicit ask wakes despite the hourly cap, including when `kind` is omitted or set to `status`. Replies to instructions and already answered asks stay capped; explicit quiet and receipt intent stays quiet.
- **Spawn base freshness** - branch fetches update the requested origin tracking ref explicitly, including when the repository's fetch mapping tracks only main, so worker checkouts use the fetched staging commit.
- **Worker contracts across machines** - live workers of your account and repository with active claims under the same epic can exchange API contracts directly. Cassy rechecks scope at receipt and gives both supervisors quiet copies. Cross-project access, urgency and assignment authority remain with supervisors.
- **Build placement across machines** - saturated spawn admission can route a new task and full brief to a fresh supervisor on another machine of the same account and repository. Peers advertise the smaller of free builder slots and free worker slots under `factory.max_workers` (default eight), using native heartbeat/process liveness across local factory sessions. Selection prefers free slots then load per CPU, rejects missing or full worker observations and reports the chosen observation and its age. A recovered local slot cancels an unsent ask before peer lookup. Accepted and in-flight asks retain one durable delivery per task even when peer discovery disappears. Unsent routes expire after fifteen minutes, stale presence, three send failures or a permanent send refusal. Mailbox duty cancels unsent asks when routing is disabled or the task is staffed locally, including queued batches. Existing worker assignments stay local.
- **Dependency waits** - a blocking dependency recorded during an explicit blocker hold suppresses deadline repeats until every blocker closes or cancels, then wakes the supervisor once with the resolved dependencies; the hold stays explicit until cleared.
- **Answered worker blockers** - a supervisor's direct answer or task decision stops repeated deadline notices for that task while its status catches up. A new blocker question starts a fresh five-minute deadline; unrelated task answers leave its alerts armed.
- **Cited code dependencies** - Task creation warns when an explicit code dependency or base is missing from the epic, naming another open epic that contains it. Reproduction evidence and temporary candidate SHAs do not declare a code dependency.
- **Delivery watchdog** - A retry names the sender of the queued notification. Detached worker checks stop prompting after their completion reaches the worker. The delivery record remains available in the inbox.
- **Approved epic integration** - A registered supervisor can pin the exact merge parent on an active integration task. Close recognizes its imported history and measures the merge diff at the pushed tip. Other side parents still need normal attribution.
- **Bounded waits** - a merged delivery without a reusable verdict goes directly to supervisor reconciliation; it never asks the worker to repeat a close that cannot create approval. An approved pre-merge verification queues the supervisor to continue the merge with the same proof. An explicit no-delivery close resumes its saved reason on exact approval, preserves independent holds and never asks the worker to close twice. Other legacy approvals give the worker five minutes to retry close before one supervisor escalation. A dispatch whose verifier still heartbeats or is building or testing in its verifier lane has its deadline moved ten minutes out each sweep, up to one hour after the request; a verifier that stops just after an extension is reported up to about twelve minutes late. A dispatch timed out by close shares the daemon's expiry relay; unresolved waits beyond its 24-hour history window retain one deadline alert, and missing local task rows do not erase recent expiries. Native background jobs stay busy while user-work descendants use CPU; unconfirmed, quiet jobs become stalled after the configured stall window even if the harness itself ticks. Without user jobs, harness CPU can still prove inference. Timeout recovery names the exact supervisor-direct verdict command with the task or epic verification type, then an ordinary close; a receipt alone does not resolve an expired epic dispatch.
- **Progress watchdog** - a factory session with locally actionable work (assigned open, in-progress, blocked or awaiting-merge tasks) and no task, lane or message change for 45 minutes wakes its supervisor once with what is waiting on whom; still nothing after another 45, it pages the operator in Commander once. A current gate-refused close is recorded in task execution state and produces one supervisor notice with its machine code and next action (close_gate/resolve_close_gate when the gate supplies no specific identity), without worker recovery or an operator-page clock; explicit operator-action metadata keeps an operator-owned remedy eligible for escalation. Diagnostic notes do not clear a refusal; changed close inputs or a later successful close do. Parameter errors and merge handoffs never create a gate hold. Blocked operator-owned checks do not count as locally actionable work. Heartbeats, liveness and Cassy's own relays never count as progress.
- **Worker context checkpoints** - a worker's typed respawn status queues its own recycle with the same recipe and task bindings, and leaves the supervisor a quiet receipt. Its resume brief links the saved task-artifact handoff. The old session's task leases are released after process teardown, so a start finishing during shutdown cannot block the replacement. A persistent ten-minute cooldown limits repeat requests for each worker in a factory session.
- **Delivery relay ownership** - PR-lane failures, merged-delivery timeouts and worker merge wakes follow the worker’s local registration and factory session. Replicated tasks for another machine stay quiet; a foreign merge observation cannot record its receipt before the owning daemon delivers the wake.
- **Undelivered lifecycle alerts** - When a task moves on before its supervisor receives a lifecycle relay, Cassy queues one action explaining the delivery failure before retiring the obsolete payload. A failed alert write keeps the original pending; retries reuse the same alert.
- **Detached worker commands** - Capped background runs persist worker/task ownership and their exit status and log path. The worker receives one completion wake, with daemon recovery after interrupted delivery. Live runs and candidate builds suppress idle alerts; a live registered run for the worker's current task also suppresses direct stall nudges, even after its launch tool returns. Finished runs restore ordinary stall detection. Each completed run starts one idle episode whose alert names the exit, and unchanged episodes stay quiet. Higher-priority build queues stay quiet. Disk-pressure waits share a durable session notice listing waiting workers and reclaimable caches; only changes to that waiting set or reclaimable-byte total trigger another notice.
- **Managed Slack intake** - A refused managed activity claim records its error and holds legacy claims and sweeps. Initial legacy fallback requires an explicit unmanaged response; Cloud selects managed delivery owners and reply authority. Cassy checks an owner-routed delivery against the receiving registration; unclaimed fan-out remains unchanged. Thread claims request verified identity without reading the thread digest. A failed manual claim after a managed mention records one typed blocker with the refusal code and thread, waking its owning supervisor when locally registered. If the owner is registered elsewhere, the claim still returns the original refusal and owner details.
- **Recoverable Slack posts** - Permission to post through Violet includes lookup of the caller's own request outcome with the same role and delegation restrictions. The proxy supplies request IDs only when the live schema and caller policy permit recovery, reconciles uncertain replies without another post, and preserves definitive rejections and failures before dispatch. If lookup is unavailable, recovery guidance keeps the same request ID and arguments for an idempotent retry.
- **Expired Slack claim recovery** - When Cloud reports a departed owner's unresolved reply on an expired thread claim, Cassy asks Violet to read it back and settle it before reclaiming with a new durable request. Live owners and unproven outcomes remain held; recovery never posts a reply.
- **Unread supervisor mail** - When a supervisor ends, its next project supervisor inherits unread Commander and daemon-stamped Violet messages, including gated sends and replacements that reuse the same name. Original content and reply routing are retained; already received messages stay with their original recipient. Moving a message does not mark it delivered.
- **Epic branch readiness** - The first child assignment or worker start checks and publishes a local epic lane, including assignments made after workers were spawned without a task. Recorded branch ownership and explicit child targets remain authoritative; publication failures appear in the task receipt.
- **Epic / Task** - an epic is a larger outcome built from tasks; a task is a prioritized work item with a lease, notes, an owner and close requirements.
- **Committed mutation receipts** - merge and close calls answer when their checked mutation commits, naming the merge SHA or parked/closed task state. Slow close, verification and note follow-up continues in the background with its final result and phase timings in task notes; an approved pre-merge verdict returns after its verification write commits while automatic merge continues. A park still awaits merge and an incomplete gate never claims a completed close.
- **Status evidence** - tool output distinguishes committed writes from completed follow-up, recorded worktree lifecycle state from current containment, and cached origin refs from live publication. Release reports count listed changes; doctor names local release-tag containment. [Status word definitions](status-words.md) map the words to their checks and name unavailable evidence.
- **Retained dependency sync failures** - Queue details identify each item and its scope with repair advice; `cas cloud queue --retry --ids <item-id>` selects a repaired failure without resetting neighbors. Scoped pulls revalidate parked endpoint ownership and availability against complete inventories before staging prerequisites and the edge. Foreign tasks, deletes, moves and quarantines remain protected; the local graph survives refusal. Exact repeated conflict snapshots keep one retained copy with their original observation time, while distinct states and revisions remain reviewable.
- **Synced working tasks** - A live owner's assigned InProgress work remains protected from Open or reassignment snapshots after a status-only resume, even without a lease. Newer same-owner blocked and terminal updates remain supported; stale or departed owners do not prevent peer recovery.
- **Automatic peer state** - Background entity sync keeps its configured cadence while indexing, maintenance and quality telemetry run. Knowledge and quality-event work use separate leases, so those optional rounds do not exclude task pulls on another local sync process. A background sync skips all cloud requests when another process holds the store lease, including team discovery and registration. Network failures and entity-store contention remain visible through existing sync status.
- **Supervisor idle staffing** - staffing nudges stay within the focused active epic after the observed idle grace period. With no focus they do not recruit from the historical project backlog; explicit task queues continue independently. Worker holds, review and delivery waits, and repeated-candidate cooldowns remain in force.
- **Supervisor context continuity** - automatic clears measure the latest call's context occupancy against a reported model window and a configured cap. When a wake’s total input exceeds the cap and its latest call is below the context threshold, the supervisor inbox explains the skip. At an idle boundary Cassy saves a bounded handoff of in-progress and awaiting-merge work, readable pending items and active workers, then preserves the supervisor's judgement through the restart.
- **Handoff provenance on edits** - memory updates retain a handoff's machine and owner stamps while ordinary tags remain editable. A new handoff receives its provenance from the authenticated writer; an edit cannot relabel another machine's handoff as local.
- **Retained worker work** - supervisor startup and `cas factory gc report --full` inventory actual Git worktrees after shutdown, including checkouts outside the active store. Unpublished commits or dirty files retain worker/task identity, HEAD, counts and recovery guidance; unavailable probes are explicit. Epic status keeps proven historical delivery and its batch-squash evidence visible, with later local work in a separate warning; retained work never produces an all-clear. These diagnostics never recover or remove work.
- **Peer transcripts** - registered supervisors query their own-account peers through the existing mailbox. The peer daemon returns a redacted, bounded JSONL grep through the inbox, with explicit scan/output truncation and no shell or peer model turn.
- **Stakeholder ask ownership** - a task's `external_ref` records its Slack thread or GitHub issue/PR source. MCP creation and staffing check existing shared claims, naming a competing supervisor, machine and task before local work changes; GitHub aliases share a claim across independent cards. A live supervisor can authorize a deliberate second GitHub task with a recorded reason. Source claims renew while registered to their owner and release on close/cancel. Staffing the same synced task under a live same-account peer supervisor moves its GitHub source claim during admission; the old supervisor retires its source heartbeat without a lost-work alarm. Independent cards and foreign-account claims remain protected. For an independent card, its supervisor can use `factory action=release_claim resource_id=<GitHub issue or PR URL>` to release the normalized source and its heartbeat registration while leaving the local task open; the receiver can then assign it without an override. Slack task work can be offered by its current claim owner through an accepted epic split while retaining the structured source link; a standalone task can be its own split scope. The builder gains task work only, and Slack replies keep their separate claim/handoff authority. Unavailable advisory checks are reported for coordination.
- **Release train** - opt-in (`factory.release_train = true`; Cassy's own repository sets it): one integration per release: fixes merge to the train, the train goes through the gate once. A task created with no epic or target joins the open release-train epic, cut from the integration branch (`factory.epic_base_branch`, else the default branch), so its delivery merges locally into the train; a cut train stops accepting new tasks and refuses further merges into its lane, including after landing and closure; new tasks open or join the next train from the integration branch. After publication, the shared release operation closes children whose delivery tree effects shipped and names every unshipped or held child as a residual; `cas release backfill --dry-run` previews the per-child disposition and `cas release backfill` runs that operation for the four historical trains. Default selection compares origin branch tips before local refs, skips trains contained in main or a newer live sibling, and refuses divergent live trains. Fresh lanes equal to main remain eligible, with the newest admission chosen deterministically. Release cut retires stale admissions with the cut marker; terminal closure remains with release reconciliation. While a train is open, `worktree_merge` refuses direct main/trunk targets, including `allow_trunk`; a one-item hotfix train ships with `scripts/release.sh --hotfix` through the same release path. With no train open, direct-trunk merge behavior is unchanged. With trains off (the default), tasks keep per-task integration targets, and any train opened earlier retires on the next close or daemon scan: unfinished tasks leave it for the integration branch and the emptied train is cancelled.
- **Ordered train merges** - `factory action=train_merge id=<epic>` queues the train's parked deliveries by updated time; `task_ids` supplies an explicit order. The existing daemon resumes at the same position after a candidate verdict or pending CI. Each delivery uses the current target and the normal merge gates: unchanged patches reuse approvals, changed context requests review, and content conflicts return the affected task and files to its worker before the next delivery runs. Single merges into that target are held until the queue finishes. Repeat the action to inspect or resume its durable position. The CLI equivalent is `cas factory train merge <epic> [task ...]`.
- **Published release staffing gap** - Worker spawn and task assignment receipts warn when the task's epic lacks commits from the latest recorded published release tag. The warning names the tag and missing commit count, asks for an integration-branch sync, and admits the work. Unpublished tags and unavailable release evidence stay quiet.
- **Returning batch QA owners** - An outstanding delivery set gets a new QA offer when it returns to a supervisor whose earlier offer was superseded. Durable notice identities preserve deduplication for the current offer and for concurrent retries.
- **Worker task queues** - Assigning later tasks to a booting worker appends them once to its ordered backlog before registration. The next task starts after the current delivery, skipping canceled or reassigned work; no assignment message is needed. Unassigning work removes its previous queue entry; an open cursor without a live lease can be reordered. Hard start prerequisites still gate dispatch, while ordinary blocking dependencies permit parallel preparation. A fresh task handed a clean detached target checkout attaches to its named branch while held delivery branches stay unchanged; detached work, tracked edits and detached resumes require branch setup before start succeeds.
- **Release freeze** - `factory action=train_freeze id=<train> reason="release-freeze: <reason>"` (a live supervisor, with explicit nonempty confirmation) or the release card's **Freeze for release** button in Commander records the train's tip and time as a label on the train, so it reaches peer machines through task sync. While frozen, `worktree_merge` into the train, staffing new work on it and arming CI retries against it are refused ("train frozen for release, parked for the next train"), and new tasks join the next train. A `release-repair` task merges only while the train is red: a train-suite red receipt at or after the frozen tip, or a failed GitHub `full-validation.yml` run at the frozen tip. Each such merge is noted on the train. The card shows "Frozen <time> ET · ETA <time> ET" from the median measured train-suite, merge-queue and publishing minutes, or names the stage that is unmeasured. The freeze ends when a published release covers its tip or when the operator taps **Lift freeze**. A verified default pairing can freeze or lift from the release card without either worker-management permission; preview pairings cannot. No supervisor action lifts it. The MCP tool refuses unconfirmed requests before mutation and warns that this is not a temporary CI hold. A supervisor-reversible shared CI merge hold is a documented follow-up in CONTRIBUTING; until available, supervisors coordinate a manual wait without freezing the train.
- **Auto-cut** - when a release train's exact tip has a green full-suite receipt and changelog fragments, Cassy freezes it and runs `scripts/release.sh` itself with the supervisor's release-notes draft; it never writes or posts notes, and a missing draft wakes the supervisor once with a deadline. It cuts at most `release.auto_cut_max_per_day` times a day (default 4), at least `release.auto_cut_min_interval_hours` apart (default 2), never while a merge-queue run is in flight or the tip is red, and never while the operator has tapped **Pause auto-cut** on the release card.
- **Train validation** - Train pushes and merge-queue entries call one reusable CI graph: exhaustive script/CI tiers, preflight, Commander web checks, cumulative changed-web journeys, doctests and Rust archive/shards. Both use quarantine plus one rerun, including script and web test steps, and cancel superseded train tips. A same-tree requeue restores each successful job with matching workflow, selection and runner inputs, even when another job failed; failed or missing jobs execute again. Expired output artifacts force their producer to execute. Recognized cleanup errors following a harness all-pass summary warn without ejecting. Full-set receipts bind the tree, main baseline, every required job and measured wall clock; train release preparation rejects Rust-only, partial, stale-main or wrong-tree receipts. Explicit hotfix cuts disclose their absent train receipt and run the full set in the queue. The queue reuses that proof only for release plumbing with main unchanged, then compiles and smokes the source and generated assets. Otherwise it runs the full graph. Train and queue durations enter the friction ledger once per train/run attempt. Re-running only failed Rust shards retains successful earlier attempts from the same run and head. A red train holds train_merge and wakes its supervisor once per run attempt with the observed change range; if the run fails before a receipt is available, it records an unknown red result and links the run instead of claiming a complete artifact. Release cuts refuse that state until verified green clears it.
- **Epic split** - two supervisors divide one epic's tasks; each side's claim is refused to the other until the split is accepted, cancelled or expires.
- **Worktree** - a separate Git checkout and branch that isolates a worker's changes; under disk pressure, build caches left by removed checkouts are reclaimed oldest first. A confirmed default-branch merge queue routes an authorized delivery through a source PR and expected-head auto-merge before touching local main. Published targets are prepared in a detached checkout and pushed with compare-and-swap against the observed origin SHA. The local target advances after publication succeeds. Rejected publication preserves its previous tip and retains the source checkout. A delivery PR ejected by a failed merge-queue run returns Open to its own worker once, with the failing job and tests as the change request; a second failed run of the current delivery tip leaves it parked and pages the operator, while stale runs and failures shared with the group entry ahead change nothing.
- **Managed file sync** - `cas update --sync` commits purely managed staged changes once on a named branch, including the project's main checkout. Staged project edits, including those inside mixed managed files, stay staged. A merge blocked by purely managed sync residue names `cas update --sync` as the clearing command; sync never pushes.
- **Delivery branch** - the pushed `factory/<name>` branch a task is proven and merged from; a supervisor `rebind_branch` approves a replacement branch after an audit of the old tip. An attributable target-sync merge advances the parked delivery and QA to that pushed tip, even when it adds no authored commits. A required ledger-only regeneration after that merge binds proof and park to the final pushed tip.
- **Required release imports** - A supervisor change-request reason pins release or sibling history with `merge_source=<full commit SHA>`, bound to the task target. The same grant adopts a fast-forwarded commit whose subject names another task, so a follow-up task can deliver it without cancel-and-recreate. Only the authenticated supervisor request handler grants these imports; quoted QA/verifier findings and queue-ejection text grant nothing.
- **Close gate** - the evidence, test-receipt and merge checks that decide whether a delivery may close; each merge records its CI admission as green, advisory or override. Default-branch deliveries reuse successful merge-group CI for an exact landed tree containing the delivery, avoiding a duplicate scoped suite. Cassy discovers the run from its recorded landing, or a close can cite `merge_group_run=<id>` without an override. Missing or mismatched queue evidence still requires scoped proof; declared-risk, content, CI and QA checks retain their evidence requirements. Queue evidence recognizes the canonical CI wrapper and legacy workflow; a cited run ID may end with separator punctuation.
- **Late close receipts** - A close that outlasts its response budget keeps running and records its final outcome. A committed park is reported from that request's receipt, even when its legacy reply is error-shaped; a prior parked task cannot turn a new refusal into success. Genuine refusal details remain visible.
- **Late park refusals** - The close outcome note lists the failed gates and their next actions. It links to the complete refusal in the task's receipts directory.
- **Queue health** - Setup, doctor and release preflight share [advisory queue diagnostics](branch-protection/README.md#2-merge-queue).
- **Generated merge candidates** - Candidate preparation runs the merged repository's generated-output script in a disposable checkout, using the same step as the real merge. Concurrent exact receipts share the completed candidate within their bounded wait; slower builds retain the background hold and supervisor completion notice. Reviews see the rebuilt Commander bundle, and failed regeneration holds review. Publication checks the final bundle against the reviewed merge tree.
- **Merge on green** - `READY TO MERGE` requires an owned, pushed delivery with green CI and worker completion: either the task is awaiting merge or the worker declared that tip ready, in a task note or a typed ready message, by its full or short SHA. A green checkpoint alone never signals completion, and an explicit merge of an in-progress task returns an unparked-task warning. A pushed parked branch that advances only through merges of its declared target receives an audited anchor update, including after its worker starts another task. New task commits still require a fresh park; CI and review gates use the updated tip. Required task verification reviews an immutable merge candidate outside the target lock before publication. An already-bound task verifier can finish late on the same current source and candidate tree; a deadline does not discard that completed review, while changed or superseded candidates still refuse. When a merge queue publishes its own merge commit, close accepts the original synthetic-candidate approval only when the task's bound source is contained and the first landing has the reviewed merge tree or complete patch; later ordinary edits retain that approval. After train publication, the recorded delivery merge preserves that landing proof when both the delivery and its merge remain contained in the live target. PASS publishes unchanged reviewed delivery content and closes with the same approval; FAIL or an unresolved verdict holds only that delivery. Source, plan, evidence or reviewed-file drift requires a fresh candidate. Unrelated sibling target advances retain approval only when reviewed files and their recomputed merge content stay unchanged; shared append-only changelog, generated bundles, codemap and builtin reference-history are exempt. Independent PRODUCT_OVERVIEW and journey registry additions keep approval when the clean merge preserves the delivery's full-context patch in each shared file; changed reviewed context still holds. When a target move touches reviewed files but the patch the merge adds, with its three lines of surrounding context, keeps its patch-id, the approval is rebound to the new candidate with a receipt and no verifier runs. Tier zero and batch tier one exemptions remain unchanged. A reviewed correction must itself land before close, and its delivery follows the worker's pushed fix branch. Duplicate CI checks at the same SHA use the newest completed conclusion. An override never closes automatically.
- **Release inventory** — `cas release inventory --scope scope.json` resolves deliveries from an exact shipped SHA across declared nested epics and unclosed structured anchors, even when the SQL branch is empty. Frozen task declarations and proven shipped deliveries determine closeout obligations. Unstarted descendants and unshipped successor work remain live, and retry retires obsolete discovery holds while preserving committed close intent. It preserves original anchors, start bases and task snapshot versions, distinguishes QA/review artifact references from shipping evidence, and reports missing evidence explicitly. No-code reports require explicit membership and portable proof carried from audited cut or promotion inputs. Tracking rejects missing or mismatched bindings before freezing the train; reconciliation closes bound reports through the same per-member approval rule as code. Release finalization closes each unchanged member whose own approved head shipped, including peer approvals bound to the original source anchor after merge, using one guarded write. Missing reviews remain individual residuals; already closed members stay counted as closed. Preview evaluates the same rule without store or artifact-cache writes. Membership alone does not grant close approval; see [the contract](RELEASE_INVENTORY.md).
- **Batch receipt recovery** - A release verdict records one published SHA across historical epic targets when every member anchor is its ancestor; ordinary QA retains target containment. Recording batch QA or a release verdict retries ordinary close, including Tier 2 deliveries with their own approved review. A receipt covering the landed delivery preserves its committed target proof after later sibling edits; missing or rejected dedicated reviews still hold close. A valid QA receipt at the delivery anchor removes that task from later QA offers; it never replaces the release verdict. Historical release backfill skips frozen successors; its read-only preview shares receipt admission with real close, names review, CI-override and retired-park holds, and leaves ordinary close gates explicitly pending. Apply reports retain each actual close outcome and refusal code; children shipped by a later published tag use that tag's release receipt instead of being classified as unmerged. Offers follow the current epic verification owner; delivery and inbox reads withhold superseded ownership, obsolete hourly overdue notices and covered evidence waits without deleting history.
- **QA-record release closeout** - An exact-head supervisor waiver lets a release verdict name the author of a delivery containing only committed QA ledgers or batch evaluation Markdown. The recorder remains independent; product code, QA catalogs and self-authored QA receipts retain their author gate. Close rechecks the waiver and immutable delivery diff.
- **Historical QA ledgers** - ledger scaffolding keeps a local task source authoritative and uses its recorded immutable delivery anchor after branch cleanup. Existing evidence and task bindings stay intact; an invalid recorded anchor cannot borrow the caller's HEAD or a moved ref.
- **Independent QA evidence** - Each pass gets a directory keyed by its full reviewed head and immutable pass id, so a reused round number preserves prior captures. Generated ledger and bundle paths share that directory. Verdict recording checks the exact head and any supplied pass id; historical bundles and explicit ledger paths remain readable.
- **Approved close recovery** - Cassy closes eligible merged deliveries on their exact verdict. If a no-code delivery or another gate leaves a task open, its dispatch's supervisor receives the ordinary close recovery; workers never retry close after approval. Exact-head checks and independent holds still apply.
- **Retired worker review** - Tier two enforcement deliveries keep their own verifier and adversarial probe after their worker retires. A live supervisor closing an already merged delivery owns the recovery dispatch; only an explicit, recorded supervisor override waives review.
- **Verifier timing** - Verdicts record milliseconds and their source. Dispatched child runtime uses persisted capability bind/consume timestamps; supervisor-direct duration falls back to the dispatch-to-verdict window, including review wait. Explicit duration overrides remain distinct; old timings remain unknown.
- **EPIC READY** - a deduplicated action sent to the epic's live owner once every child task, including those of an accepted split, is closed.
- **Journey registry** - a project's `docs/qa/journeys.yaml` (drafted by `cas qa journeys --seed`): each user journey's owner files, specs, surfaces and live, dark or dead status. Cassy's Commander registry preserves import and CSS selector coverage, with enforcement declared in the tracked registry.
- **Journey impact** - which journeys a diff or delivery touches, the evidence each needs, and the files no selected journey represents; a file stays listed as unowned when semantic selection excludes its registry owners. Journeys rank by traffic share.
- **Journey traffic** - Commander counts each journey's defining act locally and reports the per-day totals only to the operator's paired machines; `cas qa journeys --traffic` turns them into per-journey shares.
- **Upkeep gates (G1-G9)** - park refuses until the journey registry, regression case, CODEMAP and this overview are updated or a reasoned decision is recorded; release notes wait for the post-release check.
- **Overview upkeep after a target merge** - park checks new live journeys and live/dead status changes against the target history shared by the delivery. A journey introduced by the target stays attributed to the target when the delivery preserves its status.
- **Release draft lint** - the draft check reports all body, line and rule problems together and suggests plain wording. It matches whole words; the writing skill and rubric list the refused vocabulary and length limits, checked against the lint by tests.
- **Hotfix fast lane** - a project that sets `[fast_lane]` merges a small frontend-only bug fix (paths, size and Tier 1 measured from the diff) after its own pre-merge verifier PASS and named merge checks, without a batch release wait; hermetic e2e, the production build and the journey walk become a post-merge receipt that blocks close, one failed check is rerun once, and a failure the hotfix caused opens a revert PR. A failed QA walk asks the operator in Commander to Revert or Fix-forward, with Revert as the fifteen-minute default (`docs/qa/fast-lane.md`).
- **Rapid response mode** - the operator opens a time-boxed window (at most 12 hours) for one epic with `cas qa rapid-response`; while it is open, per-task independent QA, unsampled verification, untouched-route journeys and the batch receipt wait are skipped, while the post-release check stays a full gate. Build and touched-test proofs still refuse; a change with a secret-shaped line or an unreadable diff keeps every gate; migrations keep every gate and need the operator's review; and each skipped check becomes a back-fill task when the window closes.

- **Factory CI coverage** - every factory push validates the delivery from its shared epic fork, so a docs followup cannot cancel a code run and skip its checks. Other events retain their event deltas.

- **Model evaluation proof** - tasks and phases can require numeric evaluation targets on a declared dataset. A passing receipt binds a saved evaluation artifact naming the command, model and measured values to matching machine-readable run-log measurements; both files must resolve within the owning task artifacts directory. Missing prerequisite proof asks for a fresh receipt; a miss stays open and close names its failed bar.
- **Phase outcomes** - an epic or gate can carry numeric targets and evaluation receipts. Missing or missed targets hold dependent starts, closes and merges, including children of dependent phases. Independent work and sequential implementation children continue without waiting on their own phase target. Only an operator decision with a reason accepts a miss, and new measurements require a fresh decision.
- **Gate preflight** - task start lists what close will check for this task and project, and a refused park reports every failed gate in one reply. The test-shape gate resolves the integration target, including remote-only train refs, and reports an unresolved target separately from an assertion violation. Exact CLI argument round trips and HTML/Markdown-escaped Python fixture values preserve caller data, and registered bracketed gate tokens count as machine identifiers; generated prose remains subject to the wording checks.
- **Delivery check policy API** - the registry classifies delivery checks as integrity protections or requirements that apply as of approval. A saved registry membership stamp lets approval consumers exclude later requirements while retaining integrity checks; missing stamps retain current policy. The API supplies snapshots and applicability decisions; approval-by-cycle integration owns saving the stamp and using it in delivery operations.
- **Rule lifecycle** - Unpromoted drafts auto-retire after `rules.draft_ttl_days` (default 30) in the existing maintenance pass, preserving history and leaving Proven rules untouched. Verifier candidates need completed verdicts from at least two different tasks before they are stored. A projection receipt outside `.claude/rules` makes deletion durable: a previously generated file removed from the repository retires its stored rule. The shared store opener records the rules active when receipt tracking starts, before any Cloud or hub pull; a fresh database records an empty cut. Those older active rows with absent files are held locally, excluded from prompts and sync, until an explicit `rule action=restore` or `delete` decision. Later Cloud or team arrivals project normally without echoing into the push queue; corrupt per-rule receipts count as missing evidence, and an unreadable start marker is an empty cut. `rule action=list` identifies store, repository and personal-pattern sources. `rule action=create scope=global` with authenticated operator provenance and `stakeholder_quote` creates an immediately active cross-project personal pattern using those exact words, without a project draft or promotion step.
- **Rule references** - code and tests written because of a cited stakeholder rule carry its id and verbatim scope. Close checks each declared causal site and coverage of every cited stakeholder rule; process-rule citations need no marker; verification checks omitted sites. `cas rules refs <id>` lists tracked markers without changing rules or source. Retiring, superseding, un-attributing or re-scoping a rule, or its stakeholder review date passing, files one linked cleanup chore listing the affected markers; nothing is deleted.
- **Review tiers** - close classifies a delivery by the paths it changes and records the tier on the task. Docs and tests have no independent verifier or QA journeys; most changes get one verifier and their impacted journeys; guard, CI, release and close-gate changes also need an adversarial probe. Verifier improvements stay in the verdict unless they show a reproduced defect affecting someone today, and `cas qa polish --epic <id>` lists them by file at release prep. `task create` and QA's automatic follow-ups enforce that with `repro` and `affected`, and an epic's loop gain, follow-ups from findings per completed review, shows in `epic_status` and notifies the supervisor at 0.5 once the epic has 5 completed reviews.
- **Plan conformance** - the verifier reads the epic's referenced plan and reports `PLAN-FIT` against the delivery's assigned scope and outcome targets. Missing planned fields or a missed target reject even when the task card passes, with a supervisor decision about restoring scope or obtaining an authorized plan revision. Unreadable plans or ambiguous scope require a supervisor decision before approval; a child is checked against its planned slice, and an epic against its assembled outcome.
- **Model-owned plan boundary** - A plan can reserve paths or concerns for model judgment. Added literal collections, regexes and exact-string assertions produce named review sites; new meaning-matching code in the reserved area blocks unless the approved plan allows it.
- **Friction ledger** - an append-only record of every check, supervisor intervention, escape, release, feature verdict and identified Cassy problem, written automatically by the code path that saw it. Filing the problem is the write: a bug task in the Cassy repository, or an issue filed through `system action=report_cas_bug`, becomes a friction row, and the report's "Friction found" section shows each one as open, in progress, fixed in vX or closed. `cas ledger backfill` imports earlier Cassy bug tasks and, through a bounded `gh` call, issues on `issues.components.cassy`; friction rows stay local until Cloud accepts the kind. `cas ledger report --release vX` prints the numbers per release across a team's projects; only metrics, timings, titles and links reach Cloud. The JSON report also counts deliveries that merged in the window from the merge record (unknown when the record does not cover it), lists the skills, guards, rules and gates changed between the release tags, and gives each supervisor's turns by wake channel. In Commander, the command palette's Quality opens the selected conversation's project ledger for the last five releases and work in progress: the rubric that got worse by the largest factor leads with its trend line, worker and supervisor rubrics follow per merged delivery (per hour in each row's data), releases that changed a skill, guard, rule or gate carry a marker naming it, a release in a row's data opens the merged deliveries, friction (with links) and supervisor tokens by wake source behind it, rubrics without a signal show whether they await session scoring or the AI judge, and supervisor input tokens are broken down by wake source. The hub serves it read-only at `GET /v1/sessions/{session}/quality`. Reports retain each delivery cycle's review rounds and verdict provenance, split wakes by authenticated notice kind, and count Commander actions plus manual supervisor close/receipt work. Explicit supervisor-attested manual repairs remain dated and sourced in task notes; backfill imports them idempotently with release attribution. Historical transcript replay enriches immutable wake rows without triggering supervisor clears or counting another wake. Commit-to-main measures the individual deliveries inside a release train, including Git-retained integrations missing from the local merge log. Unknown historical provenance or cycle boundaries stay explicit. Backfill recognizes retained legacy queue-ejection aliases before appending a canonical row, preserving one fact per task, PR and failed run.
- **Acknowledged delivery** - a message counts as delivered only when the recipient's session receives it; idle recipients get bounded wakes and the sender is told after five minutes.
- **Supervisor input** - verified paired-device Commander text can steer a running Codex turn. Other message origins keep their wake policy during tools and idle states, including oversized messages; gated rows remain readable through the inbox. Native confirmation dialogs and operator drafts keep incoming text pending. Wakes, reminders, digests and peer messages carry their current render time in the host operator timezone, in twelve-hour form with a zone; supervisors must read the clock or use a measured start before stating a time or ETA.
- **Director wakes** - supervisor director prompts have a durable queue identity and director source attribution. They honour busy turns and protected drafts; non-green delivery decisions retain their original tip, declined tips become superseded notices, and merged alerts are withdrawn after a queue delay.
- **Peer messaging** - sending reports Cloud queueing; a Cloud delivery acknowledgement proves acceptance by the recipient machine and explicitly leaves supervisor surfacing unconfirmed. Published artifact attachments retain the owning task and sharing project and print a scoped fetch command; older pointers ask for a fresh share or task id. Supervisors reach each other by name, project, `project@machine` or `supervisor:<project>`; a message to an ended session goes to its live successor. Only `kind=ask` messages whose first content line declares `CAS_MERGE_HOLD {"task_id":"cas-example","pr":4249}` hold the matching task or PR (either key is sufficient); incidental or legacy body mentions grant no hold authority. A merge hold recognizes answers from the asked peer by reply link, an explicit ask ID, or a typed answer to that peer's sole open question. Ambiguous answers stay visible by message ID until linked. Inference excludes stale answers and other peers; status updates and receipts keep the hold, and explicit reply links remain valid across supervisor succession. Untyped peer mail wakes an idle peer without creating a merge hold; its incoming envelope renders `kind=ask`, or `kind=answer` for a correlated reply; only explicit receipts and `wake=false` stay in the inbox without waking. Legacy untyped mail keeps its wake behavior, and unknown kinds are refused. CI, task and merge waits use events; plain time reminders are accepted as a backstop.
- **Worker isolation** - on Linux, workers cannot read operator credential stores such as Neon and Vercel, and see only the connectors an operator grants a dedicated worker account.
- **Capped test storage** - Named runs retain untouched selected-test output separately from the raw guard log and expose its path in the receipt for direct supervisor recording. Worker recycle stops registered detached producers and waits for their inherited target leases to clear before tearing down the worker; an unsettled build refuses recycle and keeps the worker running. worker and verifier tests create temporary files and fixture HOMEs in short private directories on the configured scratch drive, leaving room for Unix socket paths. Named worker tests resolve Cargo from launch PATH, `CARGO_HOME/bin` or `~/.cargo/bin`, then stage `rustc` and `rustdoc` from its toolchain directory into the private test PATH without adding that directory. Literal HOME, Rust-home, CI and Git identity/config prefixes reach only the test child, preserving capped admission and runner identity for CI reproduction. Retired-worker and verifier GC checks full checkout ownership before removing these files without copying them into proof artifacts, keeping large fixture copies off a memory-backed system temporary directory.
- **Memory, Rule, Skill** - durable facts, versioned instructions that sync into agent harnesses, and procedural playbooks agents load for a kind of work. Memory edits can rename a title alone or with its content, preserve omitted fields and report the updated field names. Superseded memories expire and leave session context. Only operator-authorised rule writes create encoding chores; agent and supervisor writes name the operator command and leave existing chores intact. Stakeholder rules retain verbatim words and scope; `rule action=drift` reports differences between source, generated files and an explicit peer snapshot without changing policy. Supervisor handoffs replace and restore only the same machine's project and role context, including successors with new names or sessions; owner/session stamps are audit only. Foreign-machine and unstamped legacy handoffs cannot supply local approvals. Session start restores the current role's handoff and checks its named Slack channels and threads through Violet for newer human posts. Author-labelled excerpts appear above the handoff; bounded reads leave a notice when unavailable or incomplete. Explicit memory/search tools and task-create related context retrieve prior knowledge.
- **DESIGN.md first** - every report, page, chart, note, UI, terminal output, drawing or image an agent produces wears the target project's `DESIGN.md`; a project without one gets one through `design-spec`, and the task records which file and revision it used.
- **Knowledge page** - a distilled, source-linked page about the current repository; the code index connects symbols to that context.

- **Peer repair approval** - A later approval from the same authenticated source supervisor and machine can replace that source's stale peer approval for a repaired current delivery. Cassy atomically invalidates the old peer dispatch, preserves both verdicts and records the replacement. Local reviews and stale approval replays retain their protection.
- **Cross-machine approvals** - a confirmed published candidate shares its original approval through the authenticated project peer mailbox. Published landing approvals wake an idle receiving supervisor; explicit receipt or quiet intent still opts out. The receiving daemon validates the supervisor, delivery, plan and landed Git tree, then keeps the proof in its existing verification records so the worker can close after mailbox acknowledgement or source-machine cleanup. Later plan bookkeeping preserves an admitted published approval; close still checks the task, delivery SHA, target and landed Git tree. Changed candidates, foreign senders, non-approved verdicts and replay into a new proof cycle cannot grant authority. Peer failures never fail confirmed publication; permanent receipt refusals are acknowledged as rejected while availability failures retry.

- **Hosted queue precheck** - `cas release precheck <sha>` validates an exact commit on hosted CI and records its run; `--status` reports results and elapsed time. A supervisor's parked merge refused for pending CI resumes automatically on green for the authorized source, repository and target. Terminal CI evidence or a moved lane sends one notice, and pending retries expire after two hours. A landed merge preserves a held close as a follow-up. The precheck isolates Python imports, recovers uncertain dispatches without sending another run, reports JSON errors and deletes completed precheck refs only at the expected SHA.
- **Test-shape guard** - Git helper command arguments and Rust `expect` failure messages remain valid runtime inputs; actual source-text needles and expected returned prose still require a reasoned pin. Changed-string warnings select literals from changed hunks, keep SQL schemas whole, exclude unrelated Rust SQL fixture setup and show the matched value.
- **Queue archive dependencies** - hosted Rust suite builds restore a compatible main-scoped dependency cache while compiling current source. Advisory main priming fills a missing cache after queue-proof reuse; it adds no required check, and disposable queue refs do not save dependency copies.

- **Epic design review** - five Bug tasks labelled `stakeholder-reported` at P0/P1 within 48 hours pause new starts and spawns on their epic. The supervisor reviews the design against the plan; the operator resumes staffing with a recorded reason. Configurable thresholds, acknowledged reports and per-epic scope keep other work and shipping available.

Linked scratch worktrees without local Cassy configuration inherit the main checkout's canonical project identity through the shared Git common directory. Release preview therefore recognizes the same project scope, while an explicit checkout-local pin retains precedence.

- **Doc fragments** - deliveries never edit the shared journey registry map, `.claude/CODEMAP.md` or `docs/PRODUCT_OVERVIEW.md`. They add `docs/qa/bug-families/<slug>.yaml` and `docs/product-overview.d/<task-id>.md` files; the `journey-catalog` lane lint checks them and `scripts/release.sh` folds them in once per release, so parallel deliveries never conflict on shared docs.

- **Ledger scorers** - `cas ledger report` grades each release on five four-label rubrics, each with a name, a question, and labels that carry a value, a description and a score. The pass line sits between 0.8 and 0.4. The rubrics cover review rounds per delivery (a hit above two rounds), approved spikes and no-code deliverables graded by the supervisor alone, hold notices per release receipt, minutes a green release train with its verdict recorded sat uncut, and minutes workers sat held or idle while an epic they work had ready, unblocked tasks. Every hit names its delivery, receipt, train or worker with evidence ids and, for time rubrics, its window. Time spent on in-progress tasks aimed at a held train counts as repair, not waiting. The scorers read existing review rows, supervisor notices, release receipts, task events and factory-session hold records, and they only measure: nothing gates or refuses on them. Unreadable sources show as not measured, and subjects that cannot be graded are listed with the reason.

A copied store may live outside Git when release preview runs from the original repository. Store relocation preserves the repository's Git identity and the candidate proof, key, source and tree bindings.

- **Gate registry** - factory checks have stable codes across gate, guard, watchdog, notice and CI families. `cas gates list` measures the live registry from source declarations. MCP tool, hook and release/pre-push refusals carry their `[CODE]`; `cas gates report --since <tag>` counts recorded refusals, overrides and false-positive declarations per code. Uncoded refusals remain countable. Workflow C checks are counted and coded; their per-code facts follow separately.

- **Measured policy exceptions** - accepting producers carry explicit override tags through successful MCP, hook or CLI results. One observer per request boundary counts each code once; reads quoting codes and approval-receipt replays add no override. The completeness test generates an AST audit of emission sites, bypass classifications and recording boundaries; builds generate only the declaration registry. Personal-memory data quality, non-factory device controls and execution confirmations are reasoned exclusions.

Friction ledger backfill recovers the live branch rebind identity from structured audit notes. Release reports count each action once even when an older backfill already stored an alias; stored history remains intact, and distinct same-minute actions remain separate.

The factory gate audit detects refusals controlled by caller identity independently of gate declarations. A counted legacy baseline preserves existing behavior while preventing new uncoded authority checks; versioned baseline allowances can only shrink. Error-message rewording does not change a site's identity.

Factory-check cleanup retires duplicated test-shape validation at park and CI prose pins, merges proof-scope and independent-QA code identities. Train dispatch holds remain where candidate-review exemptions would otherwise skip a pending verdict. Publication hooks remain until every post route has an equivalent server-side enforcer. Messaging to recorded native child IDs preserves factory-peer coordination policy. The mechanical task audit records removed rows, retained checks without equivalent enforcement, and the measured registry reduction; generated shared docs refresh at the release cut.

- **Release report sources** — Reports accept both text-fenced drafts and exact JSON publication payloads, including replies. JSON message text takes precedence over duplicate blocks. CHANGELOG fallbacks preserve supplied Was/Now pairs and separate shipped changes from supporting notes without repeating them across audiences.

Peer replies with an exact Cloud `in_reply_to` resolve the sender's outgoing ask
before wake-kind filtering. This includes quiet answers to automatic build-route
asks. Messages without an explicit link keep the existing correlation rules;
quiet, status and receipt messages do not implicitly answer questions. Structured
`CAS_MERGE_HOLD` task and PR scopes continue to govern which asks hold merges.

**Cross-machine task ownership at session startup.**

A peer task can be InProgress without a lease on the observing machine. Session startup preserves that status and assignment instead of publishing an Open reset. Recovery of abandoned local work requires matching local lease history and applies only if the task snapshot remains current. Unknown ownership is preserved for supervisor review.

Task label updates replace ordinary labels with the supplied, trimmed and
deduplicated set. Omitting labels preserves them; an empty value clears ordinary
labels. Finding provenance remains protected, and nonempty updates preserve omitted
triage axes. Supervisors can remove a release train's cut label through task update.

- **Proof after checkpoint rebases** — A fresh park on an owned task branch uses the pushed delivery tip for focused proof after a legitimate rebase and ancestry-preserving merge. It reports an unattributable restored checkpoint instead of directing the worker to test an older commit. Ordinary first-parent checkpoints and shared lanes keep their accepted delivery boundaries, proof scope and generated-output safeguards; a body-only mention cannot claim another task's commit.

### First-failure CI repair

The release queue executes native shards once and preserves every nonzero exit.
It no longer excludes tests through issue labels. A hosted publisher files one
P0 repair issue per failed native test, including identity, run and failure output.
Issue intake owns staffing ahead of feature work. Its owner must fix or delete
by the next working day and record covered behaviour when deleting; CI never
rewrites source. Repeated observations keep the original deadline.

The factory browser test guard reads Playwright 1.65 required, optional and variadic option values before checking for a named spec or canonical journey ID. Named-spec shuffle runs accept numeric, string and negative seeds. A spec-looking option value cannot authorize an unfiltered run, and unsupported option tokens are named in the refusal.

**Artifact-only scope corrections.** A supervisor can replace a previously recorded documentation delivery with portable no-code evidence. The correction checks the retained Git boundaries with the existing docs/artifacts path and file-mode rules, retires their source metadata with the old proof cycle, and refuses retained code or unreadable delivery boundaries. Ordinary code delivery gates remain required.

- **CI completion reminders** — An optional workflow selector accepts an exact display name, workflow file path or numeric GitHub workflow ID. Registration resolves it against the origin repository before saving the reminder and refuses unknown or ambiguous selectors. Omit the selector to wait for all applicable runs on the commit.

- **Filename discovery** — Workers can use standalone `rg --files` and
  name-only `find` commands under credential-store parent directories, including
  output redirected to a file. Pipelines and substitutions retain ordinary
  credential inspection; explicit credential-store operands remain protected.

**Complete simplicity findings.** A pre-merge verifier that finds unnecessary code inspects the same file for every instance of that class and reports the locations and smaller changes together in one verdict.

- **Local-only recovery inventory** — Without a Git remote, retained-work inventory compares commits with the task's local target, falling back to the configured base or primary checkout branch. It still shows dirty or ahead worker checkouts. Push instructions use a configured remote and are omitted for local-only repositories; inspection remains read-only.

- **Runtime fixture mutation** — Test-shape lint accepts an immediate same-path formatted Rust fixture rewrite when the read value has no other consumer. Assertions and comparisons of source text still require behavioral coverage instead, so real Git/MCP regressions can change disposable compiled inputs without a pin or bypass.

**Reworked peer deliveries.** When a peer re-parks a delivery, merge preparation follows its new recorded anchor if the local factory ref is an ancestor of the matching origin tip. Local divergence still requires explicit reconciliation. The update uses the existing checked-out-worktree safeguards and expected-old-SHA comparison.

- **QA source citations** — A ledger expectation can cite a repository source path and positive line number, including extensionless scripts and files such as README. URLs and clock times do not count as source citations.

Supervisor operator messages use `[evidence:<tool-use-id>]` beside factual paragraphs or begin uncertain paragraphs with `[unverified]`. The hook accepts only paired, successful tool results after the current user-turn boundary, and numeric literals must appear in the cited result. Perform time conversions and calculations with a tool before citing them. Clipboard claims need a `wl-paste` or `pbpaste` result after the latest clipboard write. Missing transcripts or a turn start outside the bounded transcript tail cannot establish evidence.

This is a structural provenance check, not semantic fact verification: a citation identifies the result the operator can inspect. Claude MessageDisplay labels unsupported replies; Stop refuses unsupported final replies on the first attempt and emits an uncertainty notice on reentrant attempts to avoid a continuation loop. Streaming content may already be visible before Stop runs. Direct transports outside the configured hooks are outside this guard.

Expired unparked task-only verification can be requested again through close, including a corrected portable no-code proof reference. Renewal creates a pending review and retains the expired dispatch for audit. Shipped PR/queue deliveries, active reviews, receipt-bound deliveries and epics keep their existing recovery boundaries.

Supervisor answers recorded with `verification action=add status=supervisor_call` resume the independent review on the same dispatch. The answer supplies context, while a fresh sealed reviewer retains responsibility for the final verdict.

- **Worker checkpoint pushes** — A pushed checkpoint on an owned InProgress task is not a parked delivery, including when commit-time branch and anchor capture exists. After a supervisor-directed target sync, the worker may rebase and publish using an explicit force-with-lease for the observed remote tip. H-40 verifies ownership, recorded start ancestry and authoritative delivery/review state; unknown state, deletion, parked work and rewritten reviewed anchors refuse. Parked or reviewed branches merge their target instead.

**Merge response receipts.** A slow delivery merge returns a preparing receipt after a short wait while its existing safety checks continue in the background. The receipt path records the complete final result, and a supervisor notice reports completion. A published merge reports COMMITTED even when its close follow-up is still running. Callers use the receipt and notice before deciding whether another merge request is needed.

Prepared release pushes and merge-queue entries reuse an ancestor's successful full-set proof when only release plumbing changed and main is unchanged. A durable adoption artifact records the release head/tree and original native producer head/run/attempt. Consumers verify both completed workflows, complete logs, source proof and the actual queue delta before suppressing suite jobs. Compile, launch, generated assets and Release Prebuild remain checked against the release tree. Any source edit or invalid proof keeps the full suite selected.

A release train runs its full suite once, when it is frozen for release. Merges into an unfrozen train start no train suite, because lane CI already proved each delivery. The daemon's train CI scan dispatches the shared full-validation workflow once per frozen tip; a repair or plumbing commit on a frozen train is a new tip with its own single run. The cut consumes that run's green receipt, and a red raises the train-red notice. Auto-cut freezes a ready train with changelog fragments, asks for notes while the suite runs, and cuts on green.

Supervisor replies can use successful same-turn tool results without citing hidden tool-use IDs. The operator evidence guard matches numbers and SHAs against those results and accepts exact command strings in backticks. Optional ID citations narrow the evidence used; clipboard claims still require a read after the latest write.

The portable `cas-test-audit` skill produces one read-only inventory of test value, flake risk and the project's own recent flake history. Findings carry source location, pattern, confidence, evidence and a fix. The report distinguishes a named failure/pass on identical source from a workflow flip or issue report, calibrates static candidates with hand review, and routes actionable findings to the approved fix-or-delete batch. It never reruns a suite or changes tests/issues while auditing.

Train CI runs once when Cassy dispatches the frozen tip; prepared release pushes start no train workflow. Cut retries resolve `Train-CI-Head` directly to the original green source receipt, without a release-head run or adoption artifact. Pull-request admission, queue reuse and Release Prebuild retain their existing behavior.

Code deliveries receive their review before merge, including Tier 1 in batch QA mode. A release cut combines those approvals (local or peer) with the green frozen-train run, naming any missing approval before push. It requires no separate release-review ledger or recorder identity. Batch QA remains independent evidence and the merge queue remains the gate to main.

Deliberate depth=light, rapid-response sampling and Tier 0 exemptions use green lane CI plus the green frozen-train run. Pending or rejected reviews retain their authority. Missing lane CI is named separately from a missing required review.

Release admission reuses the nearest green train ancestor when every intervening commit changes only the release asset allow-list. Source changes, including changes reverted before the cut, require fresh train proof. Receipt identity, complete logs, successful full-set jobs and the current main baseline remain mandatory. A pending CODEMAP refresh can join the release commit. The release verdict uses existing approvals and the green frozen-train receipt without another batch release review; missing required approvals still refuse the cut. The merge queue proves the final tree.

Prepared release assets include the compiled product overview and journey registry as well as their input fragments. Terminal no-code members are reported from their saved outcome without reinterpreting their original external link as an artifact. Active report deliveries retain artifact integrity checks and must carry their required approval before the cut.

### Lane failure attribution

Lane CI reuses explicit failing-test proof only for the recorded base SHA and matching package, binary, test, platform, toolchain and profile. Missing proof triggers a bounded replay; unresolved attribution remains red. Failure receipts are published before base preparation, with separate attribution timings and no additional CI job.

### Delivery-only lane impact

Lane selection excludes changes imported from the train. Script and workflow inputs retain their script contracts and directly referencing Rust tests, while recorded start bases remain available for failure attribution. Agent configuration and documentation roots add no Rust tests; build policy and unmapped code retain conservative coverage.

### Approval across train movement

When the train advances without changing a delivery's reviewed paths, the merge
retains its approved dispatch and records the target movement on the task. Clean
same-file merges retain the existing unchanged-patch approval path. If preparing
that candidate exceeds the response deadline, the merge stays held with its
approval intact until preparation finishes. A retry reuses that preparation;
changed delivery content and merge conflicts keep their existing review checks.

Factory and supervisor-authored epic pushes require local fast checks, including changed release-note drafts. A push changing `hub-web/` also proves committed dist matches a fresh build. The integration engine reuses already checked delivery content with a marker bound to the exact pushed head and observed remote base; a later supervisor commit cannot reuse that exemption. Supervisor startup and doctor verify the main checkout's configured hook directory and name its repair.

A release train records its must-land scope with Blocks dependencies on required deliveries. Auto-cut waits for those deliveries to close after merge, be cancelled or move to another target before freezing the train. It names unresolved deliveries; optional children and prose notes do not define the scope. Trains without Blocks dependencies retain the existing auto-cut rules.

Commander shows a working indicator from fresh supervisor activity independently
of named lifecycle phases. When no phase is known, recent output still indicates
work without a phase label or clock. A delivery receipt alone does not indicate
execution, and losing the live connection clears the activity indicator.

Commander history reads use an independent read-only SQLite connection, including
earlier sessions, open questions and delivery receipts. A daemon writer holding the
shared connection no longer prevents those reads. The conversation shows measured
phase status or a known waiting question; absent or unrecognized phases add no
placeholder phase label or timer. Fresh unanswered activity independently shows
working. An older-page error becomes the reading position when revealed, so a
pending tail pin cannot scroll its Retry control away.

Schema migrations use a private SQLite connection with the same WAL+NORMAL policy as production stores. Store creation and upgrades retain per-migration transactions and rollback while avoiding FULL-mode disk syncs on every commit.

Release dependency caches and compiler objects share GitHub's repository storage quota. Main seeds reusable compiler objects; factory, train and queue builds read them without adding branch-specific copies. Dependency cache identity continues to include third-party dependencies, toolchain and build compatibility inputs while excluding workspace version bumps.

An infrastructure ERROR preserves a pending verification dispatch and its evidence while consuming the failed reviewer's authority. A replacement reviewer or the registered supervisor can settle the same review; a code rejection remains terminal. Factory verifier processes start from the daemon's namespace, retaining the harness's read-only sandbox. Linux startup checks bubblewrap before invoking the model and reports the failing namespace context, policy settings and recovery action.

### JavaScript stability checks

The capped worker runner accepts `cas factory worker-check --cas-root "$CAS_ROOT" -- vitest path/to/file.test.ts`. Substitute `jest` or `playwright` for the runner and select that runner's test files. Paths are relative to the repository root. It resolves each file's package and uses its locally installed runner in an isolated worker or managed review checkout.

Only new or changed test functions are selected from the delivery diff. Vitest and Jest run the selected names five times; Playwright uses five repetitions with retries disabled. The first failure stops execution and names the failing test. A successful run records the selected identities, five consecutive passes and elapsed time against the clean commit. Park reads that evidence without running tests.

Dynamic or parameterized changed titles, out-of-line callbacks and test factories that cannot be mapped safely to runtime identities require an explicit source-to-runtime resolution instead of broadening execution to unchanged tests.

- **Review assignment authority** — A task can name its exact `review_dispatch_id`. Ordinary worker assignment, spawn and start refuse that verdict work before accepting it. The daemon verifier or an authorized supervisor records the dispatch verdict; task staffing grants no verifier rights.

- **Candidate schema at the cut** — A release candidate can be newer than the store's schema. `cas release preview` then reads a private, migrated backup and leaves the live store unchanged. `cas release track` is the cut's first write, so it applies the candidate's migrations first and stops the cut if one fails. `scripts/release.sh` passes `CAS_ROOT` to both candidate calls.

### Changed-test stability at park

Every factory project uses the existing focused-proof gate to require five
consecutive passing executions of its delivery's new or changed tests. Receipts
bind the repository, tested commit and source test identities. Existing
one-pass crate evidence remains useful for unchanged tests. A later overlapping
failure retires overlapping stability evidence at that commit and its ancestors.

Test-only corrections also require five-pass proof. Documentation or changelog
commits preserve evidence through the same input and ancestry checks as crate
proof; target merges use the same resolution checks. Changes to test or build
inputs invalidate old evidence, even when a test keeps its name.

The capped Rust runner resolves Cargo roots and source modules, runs the requested
selection once, then repeats only its changed functions four more times within
the same admission and memory cap. JavaScript adapters use static source names
and runner reports; Playwright uses repeat-each with retries disabled. A failure
stops execution. Park only reads the resulting receipt and names missing tests.

Release inventory and closeout require the task's own first-parent content at the published commit, using the same content continuity proof as merge verification. Train imports remain separate from task delivery. Owned overview and bug-family fragments remain delivered when a release folds their content into the maintained documents and that content remains present. A later revert or a bound release review marking a member as reverted holds that member for re-delivery. Other independently approved shipped members can still close.

Commander's conversation working line follows the supervisor's native turn, from its prompt to the harness's own turn end (Claude `turn_duration` or an Esc interrupt; Codex task completion), across every tool inside it. While busy it names the running tool without its input, shows the turn's elapsed time and the last transcript activity in local time, and colors its dots by state: working, idle or unknown. Status arrives with each machine heartbeat; facts older than fifteen seconds, or a stopped session daemon, read as unknown and possibly stuck rather than idle. Without any reported phase the line stays absent, so a delivery receipt never implies work.

- **Close measures the checked-out delivery lane** - when a task started on the worker's lane and a per-task branch `factory/<worker>-<task>` also exists, close parks the lane if the worker has it checked out and it holds the task's commits since start. The per-task branch is parked only when the worker is on it, or when the lane has no work for this task.

- **Machine-wide worker census** - spawn admission and `worker_status` read one census of live workers across every local factory project. Both print it as "N/M live on this machine: <project> <count>, …", so a refusal shows which projects fill the limit.

- **Live capacity wait readings** - a capacity wait keeps the figure its last refusal measured, labelled "cause at last refusal", and every later report (`worker_status` and the 20-minute supervisor notice) adds a "now" reading of load, builders and disk taken when the report is produced.

- **Inherited worker limit** - `factory.max_workers` resolves the project value, then `~/.cas/config.toml`, then eight. Config get, list and reset, spawn admission and advertised peer capacity all use that one value, and readback and capacity refusals name its source (project, user or default).

Conceptual search accepts shell flags as literal keywords after parsing structured filters, so pasted command terms can be searched alongside memory and task descriptions.

- **Shared release proof work** — Release previews reuse successful parallel-edit
  proofs across deliveries that traverse the same immutable merge edge, file
  and live line positions. Storage is bounded; failed measurements remain
  retryable and each delivery retains its ownership and review checks.

### Factory merge admission

Parking requires a clean merge against the current train. A train update rechecks parked siblings before queued merges resume; conflicting deliveries reopen for their assigned workers, naming the files to resolve and invalidating the old proof cycle.

- **Returned deliveries stay in branch history** - when a parked delivery is returned for changes or a merge conflict, its tip stays reachable on the branch. The returned task's start lists that tip under close checks, and the worker push guard (H-40) refuses a rebase that would drop it; merging the target in is the supported recovery.

Search returns the last committed results while another writer owns the task search index. The next search reconciles canonical task rows once the writer is available; uncommitted changes remain invisible.

- **Release preview's read-only boundary** - `cas release preview` writes no task, receipt, operation, sync-queue, artifact-cache or notification data and leaves every existing store file unchanged. It reads through ordinary SQLite read-only connections with normal locking, so SQLite may add its `cas.db-shm` and `cas.db-wal` coordination files when they are absent; preview does not promise that no file is created.

A no-code task's close request and portable evidence are saved with its verification dispatch. Approval resumes that same close through the ordinary gates and releases ownership when it completes. A changed assignment, replaced proof or independent hold prevents automatic completion. An identical approval retry preserves the existing terminal state and completion notice.

### Shared callback stability checks

Playwright journeys may place `// cas-stability-identities: ["full runtime title", ...]` immediately above each registration that uses a `const` callback local to the registering function. Each registration declares all of its expanded suite/test titles. Worker checks bind changed callback bodies to those identities, verify the complete expansion at the exact registration location using native Playwright discovery, and require five consecutive passes with retries disabled. Invalid, missing or ambiguous identities refuse proof. Vitest and Jest retain static source discovery.

A merge importing a recorded delivery from an earlier worker retains that task's original changes across replacement and restart. Close gates use the same attributed ranges for source, test stability and release notes while excluding target refreshes and foreign work.

- **CI completion reminders** — A workflow display name that several workflows share resolves to the one with a run for the reminder's commit. Otherwise registration is refused as ambiguous and the refusal names each candidate workflow file and ID to choose from.

### Parallel full-set validation

The Rust archive feeds six disjoint suite shards, with complete shard evidence
required by the collector. Classification also handles runner routing and tree
reuse. The hosted full-set gate collects native reports, publishes repair issues
and rejects failed or cancelled producers, publication and queue checks. Its
receipt binds those outcomes to the evaluated tree. A separate Fast Validation
wrapper preserves the protected branch's required job check.

### Full-set proof follows job inputs

A frozen train repair can reuse a successful full-set job when its input content,
workflow, selection, baseline and runner binding match. Commander web and journeys
include the CLI version, hub asset list, preview CSP and Rust crypto fixtures.
Content reuse applies only to full-set train dispatches; queue job receipts retain
exact-tree keys. Rust proof includes embedded web assets and shared test
fixtures; preflight and script tiers retain broad input closures. Unknown paths
invalidate all job scopes, and unknown jobs use the entire tree. Reused receipts
name the current evaluated tree and preserve their source tree and producer run.
The full gate still requires every selected job before a release can proceed.

Frozen release train CI tracks its pending run. A run still queued with zero jobs after ninety seconds gets one replacement at the same frozen head. Durable per-head dispatch history excludes the abandoned run from pending state and prevents a replacement chain. Runs with jobs, runs in progress and younger queued runs keep their existing behavior; recovery never cancels a GitHub run.

The Accounts sheet's Clean up confirmation opens directly under its banner at full sheet width, so the explanation and typed-confirmation field stay readable at desktop and phone widths.

### Frozen train repair admission

For an active release freeze, merge admission requires the repair's exact SHA
  to pass the lane's fast and factory/PR checks. Rust impact jobs defer to the
  full-set run at the new frozen train tip. Completed impact failures still refuse admission. Other job failures and pending work
  retain their normal verdicts. Missing freeze or check evidence retains normal
  lane admission. Existing freeze, repair authorization and main landing gates
  still apply.

After publishing the merge, Cassy makes a bounded attempt to cancel that SHA's
  factory lane when its only unfinished jobs are Rust impact jobs. Cancellation
  never targets the train's full-set workflow, and failure to cancel does not
  undo the merge.

Resumed delivery checks preserve task-owned first-parent intervals across train retargets and use surviving journey changes to determine stability proof. An unchanged restored journey does not need new test identities; current ambiguous test changes and foreign task ownership still refuse.

The JavaScript stability runner starts each test file from the nearest ancestor
holding that runner's config (`jest.config.*`, `vitest.config.*` or
`vite.config.*`, `playwright.config.*`, or a `jest` key in package.json) and
passes that config explicitly; without one it uses the nearest package.json.
The changed-test stability refusal at park names the exact capped-runner command
for each changed test's runner, taken from the file's runner import or the
nearest runner config, package.json dependency or script, or local runner
executable. When nothing identifies the runner, it lists the Jest, Vitest and
Playwright commands separately.

- **Review ownership follows recorded handoffs** — When a supervisor retires,
  its pending review can follow the same-machine successor that claimed its
  session-end receipt. The review keeps its identity and proof, records the
  former owner, and protects any live verifier from replacement.

Factory credential checks use each shell statement's effective working-directory alternatives from the existing shell-effects recognizer. Child scopes restore the parent directory and directory stack. A directory change before a pipe or inside a background list is undone; one after a pipe, or anywhere in a command that defines a function or runs a piped or background compound, is kept only as an alternative, and function bodies are judged from every reachable directory. A statement whose working directory is unresolved is judged from every directory that could reach a store; a glob directory keeps its literal ancestors. A command the recognizer cannot project is refused, by the workspace contract where it runs and otherwise by the credential gate. The command grammar and file-open policy stay unchanged.

Worker SessionStart omits the role-wide current worker handoff: a worker's assigned task brief and its task-artifact `HANDOFF.md` carry its work, so a handoff saved by another worker for a different task never becomes startup instructions. Supervisor handoffs keep their machine-scoped selection.

The daemon-owned pre-merge verifier on the Codex recipe pre-approves only
Cassy's own MCP server, keeping its read-only sandbox. A merge retry on a
pending dispatch whose last verifier attempt failed reports that one
replacement is scheduled on the same dispatch, and the factory daemon logs a
refused verifier launch.

Conversation history preserves upward reading intent when several scroll events share one paint, allowing the oldest cached edge to request the next page without another gesture.

Closing Raw output returns focus to its header button only when focus was lost, so a control the operator moved to meanwhile, such as Undo, keeps keyboard focus through a later catalog rebuild.

A train merge entry that needs a pre-merge verdict starts the daemon-owned
verifier during the daemon's resume pass, and the queue continues at the same
position once the verdict lands. While a train owns the target, the verifier
handoff and recovery notices name `factory action=train_merge`; retrying it
restarts a stopped verifier on the same pending dispatch.

Task and memory sync includes known server revisions on upserts and records accepted revisions. Rejected writes retain their known base and queued edit until reconciliation. Personal and team task/memory pulls refresh queued bodies together with the observed revision, including eligible local-wins task guards; task notes merge once with the established deduplication rules independently of lifecycle. Rebuilt task payloads use the canonical serializer and preserve their queued scope/origin policy. Other entity kinds keep their prior compatibility path. A revision conflict waits visibly for a pull and reconciliation and is rearmed after that succeeds. Foreign, malformed, concurrent-mutation and unresolved delivery or execution conflicts cannot authorize a fresh base. Atomic merging of concurrent task-note appends remains owned by petra-stella-cloud#268; this client change does not claim to implement that server operation.

Artifact lookup distinguishes missing publication metadata from byte availability. If neither the local ledger nor synced task notes contain the publication reference, the diagnostic directs investigation to the publisher's Cloud reference instead of promising that another receiver sync will repair it.

- **Recorded delivery identity** - Cassy treats commits it recorded for a task, and commits a supervisor granted with `merge_source`, as the task's own at start, close and rebind, provided they sit under an anchor the task still holds. Replacement, rebound and retargeted deliveries are no longer refused just because their own cycle commits name another task in the subject line. A branch that only has a retired anchor or a bare target-sync merge still counts as undelivered.

- **Rejected train entries** - A resolved rejected verdict lets the train skip that position while the task remains parked for repair. Active train merges accept appended task IDs in order.

- **Release red from a refused cut** - when `scripts/release.sh` refuses to cut a train for REVIEW_MISSING, LANE_CI_MISSING or zero would-close, the cut's candidate binary runs `cas release refused` and records a release red for the green tip it started from. The release freeze then admits a `release-repair` task against that receipt, as it does for a train-suite red, without an operator lift; other merges into the frozen train stay refused and the train merge queue is not held. Recording is best effort and never changes the refusal.

- **Supervisor freeze lift** - the train's live registered supervisor lifts a release freeze with `factory action=train_freeze id=<train> decision=lift reason="<why>"`. The reason is required. The lift records a decision on the train (who, when, why, the tip) and sends the operator a Commander notice. A freeze the operator applied or re-applied in Commander, or one from before this change, stays the operator's unless they set the operator-only `release.supervisor_lifts_operator_freeze`; a live supervisor of another factory session is refused. After any lift, auto-cut does not re-freeze the same tip; the next merge re-arms it. Supervisors lift when a Cassy gate blocks the cut or the operator asked for new work in the release.

- **Find or file** - Cassy files a new issue or task only when no open one already names the same wrong output. The key is the refusal's registry gate codes plus its normalized text (lowercased, ids and numbers collapsed, at least four words): the first line of `actual` for `system action=report_cas_bug`, the last line of a finding's repro. A matching open issue gets the report as an evidence comment with +1; a matching open task gets an evidence note; a report with no key is refused. Finding task creates and QA ledger follow-ups use the same matcher against the configured project tracker (Cassy's own tracker in its source repository) and open tasks: a match gets the evidence and +1 and its identity is returned, and nothing new is filed or refused (cas-5edc). Filed issues record the key as a `Fingerprint` line so later reports with wrapped wording still match.

**Auditable freeze-lift denials.** Release freeze lifts retain the existing supervisor, reason, train-ownership and operator-permission checks. Their refusals now carry registered gate G-157 for the gate registry and refusal facts, with existing machine error prefixes preserved.

- **Explicit GitHub issue ownership** - MCP task creation creates a GitHub source claim for a supplied `external_ref`, an issue-owning title prefix or suffix, or an explicit `Fixes/Closes #N` binding. Other issue citations remain suggestions and do not claim the issue.

Factory deliveries automatically arm supervisor-owned CI and published-merge completion events when parked or sent for merge. The events survive session restarts, deduplicate repeated requests and recognize new CI attempts on an unchanged commit. The progress watchdog reports an unwatched delivery during its next cycle instead of waiting for its stall timer. A corrected parked anchor refreshes its completion events and retires the prior tip's automatic watches; target selection follows close, including epic inheritance. Recorded holds retain their notices and completion coverage through release.

- Worker credential protection follows a provider credential read from any file into a child process's environment and refuses it, independent of where the file lives.

**Portable telemetry dependencies.** The Linux release dependency graph uses the ring TLS provider and excludes AWS-LC. PostHog remains on 0.3.7 while a follow-up replaces its legacy TLS dependency stack; its existing TLS and certificate-validation advisories remain a known residual. Telemetry call sites and the other dependency security updates are unchanged.

**Changed-test selection (JavaScript).** Park and `worker-check` share one model. Only the delivered tip is parsed. Changed lines come from the net diff against `merge-base(target, tip)`. At park, they are further limited to lines blamed on the task's own delivery commits. Reverted edits, target content merged into the lane, other authors' commits and intermediate commits therefore never select a test. A changed `it.each`/`test.each`/`describe.each` registration becomes one template identity. Its placeholders (`%s`, `%i`, `$field`) select each runtime row, and the proof requires every selected row to pass. `worker-check` accepts the runner with an `npx`, `pnpm exec`, `yarn` or `bunx` prefix, as `vitest run` or `playwright test`, with paths relative to the current directory and a `--config <file>` inside the repository.

**Bounded review repairs.** Previously a non-behavioural rejection needed a supervisor to return the delivery. Cassy now reopens the first eligible rejection to the same live worker and sends the findings as its next prompt. Eligibility uses existing structured categories and awkward/cosmetic triage, limited to explicit documentation, evidence, receipt, incomplete close reason and unnecessary complexity categories. Summary prose grants no authority. A dispatch-bound portable receipt consumes the single repair round across the correction cycle and restart. A new provable source commit is required before another independent review through either merge or completion receipt. A receipt-path rejection can return only its exact failed transaction; external holds stay held. Another rejection, unsafe or incomplete findings, and an unavailable worker hold for the supervisor; automatic repair never approves or publishes a delivery.

Changed-test discovery widens uncertain inputs instead of refusing them: one capped pass covers the complete test file, or the nearest JavaScript package/Rust crate for source inputs. Widened receipts are distinct from five-pass stability receipts and bind to the tested commit. Missing or failing proof still prevents park. Unchanged callbacks beside deleted tests require no new annotations; changed callbacks retain identity discovery or wider runtime proof.

### Factory recovery and account routing

An isolated spawn can initialize worktree management after a running session's project gains Git history. Recycle and cleanup retain that worktree manager. Worker startup briefs stay private, readable until teardown and excluded from Git status without excluding other project state.

Automatic Codex lane requests can choose another eligible account whose model catalogue verifies the lane. Explicit account settings remain binding. Account recommendations distinguish stale, cleared usage from a fresh unopened window, retaining the binding window's reset for headroom ties. An omitted unopened window does not discard another window's observed capacity or its launch recommendation.

Accepted build routes recheck peer freshness before withholding local admission on an unclaimed task. A successful directory lookup showing departure or stale capacity releases that route; an unavailable directory, an in-flight send, a claimed task or a fresh busy peer preserves ownership.

Workers can abort merge, rebase or cherry-pick operations and switch back to an owned branch in their registered Git directory after a suffix change. A disposable diagnostic ref ending in `-diag` can be deleted only when no task, delivery or review binds it. Existing per-task branch names also support a separately based forward-port delivery.

Factory preflight normalizes scp-style and SSH origins with arbitrary SSH usernames. An unresolved repository finding identifies the branch or identity input that failed.

Factory alerts are rechecked against current task and close-refusal state before delivery. A resolved member leaves a grouped close-hold notice; the remaining members retain an updated action notice. Deliberate task holds and retirement of a blocked worker do not become fresh supervisor blockers.

Worker queues retain an unstarted spawn promise when their pending tail changes. A queued code task can reclaim a clean shared checkout when no other work owns it; queued no-code tasks preserve the shared branch and files. Isolated worker checkouts retain their ownership checks. Commander keeps a task transition label as its subject when it has no separator and treats ordinary one-word questions as text.

A registered supervisor can record a reasoned test-stability exception at park. The decision is retained on the task without creating a passing runner receipt or waiving the separate crate/content obligations. Unknown classifier widening remains a capped-run concern.

Clearing an execution-methodology label retains an existing resolved code approval only when the delivery identity, work target, declared risk and proof targets are unchanged. Close still checks the live source and exact landing, so an old approval cannot cover a newer code commit.

Park publishes ordinary task evidence and cited worker target logs through the existing artifact transport, so a reviewer on another machine can fetch them. Ordinary publication problems are reported without a new admission requirement; routed builds retain their existing manifest contract. A historical integrated delivery missing approval receives an explicit authenticated recovery command instead of a no-op merge loop.

Delivery content can be proven by a target path that exactly matches the task-authored final snapshot. Returning to an imported baseline or reverting the task's own delivery does not establish that proof.

Playwright runtime skip reasons are valid close evidence. Reformatted existing skip markers retain their allowance only within the same test and suite when the count does not increase. A removed marker cannot authorize skipping another test.

Disposable database branch env files set both DATABASE_URL and DIRECT_URL to the provisioned branch. Their quoted values survive both dotenv parsing and shell sourcing.

- Capped JavaScript checks can receive an explicit password-free loopback PostgreSQL fixture URL with `--test-env TEST_DATABASE_URL=...` or `--test-env <PREFIX>_TEST_DATABASE_URL=...`; the runner applies the value only to discovery and test children.
- Conditional `describe.runIf`/`describe.skipIf` registrations preserve runtime suite names, and explicit disabled prerequisite registrations are excluded from required passing identities. Changed-test receipt rows carry their selection reason.
- Widened file, package and crate receipts record the classification cause that selected broader proof. Unknown conditional aliases run through that broader proof with the same child-only fixture environment.
- CI selector command rows preserve each integration test stem's owner or import-consumer cause in the existing receipt's `reason` field, alongside changed-test and broader policy causes.
- QA ledger preflight and close share canonical owner parsing and permit contained absolute capture paths. Initialization uses the matching caller worktree for its journey registry and fallback revision; existing recorded delivery references remain authoritative.
- A failed attempt to attach QA evidence to an existing issue or task is reported without filing a duplicate follow-up.

### Release delivery proof

Release previews share ownership walks across members and index changed hunk lines once. Delivery inventory recognizes conventional commit task owners while retaining historical changelog-fragment evidence.

Immutable commit metadata and changed paths are read in batches. Repeated measurements reuse successful immutable ranges and promotion evidence; moved refs and failed reads are measured again.

A release checkout that aliases a train's exact tip names the canonical checkout before preparing a release. Auto-cut resolves a live local supervisor after handoff and waits for unfinished release-repair children. Installer adapter timeouts identify their phase, deadline and retained receipt.

An active parked delivery contained in a versioned release tag on origin's default branch can close from its recorded anchor, release tag and that branch's immutable tip. The close records its ancestry evidence and clears stale verification waiting state. It does not depend on a retired worker checkout or replay park evidence. Historical anchors, unshipped work, newer cycles and current rejected reviews retain ordinary checks.

Release closeout judges missing generated documentation, bundles and builtin reference history through the surviving task-owned source. Those outputs cannot prove a source revert. A delivery with only generated output still needs evidence; folded fragments retain their existing content proof.

### Artifact inventory, retention and candidate fallback

Artifact list reads the named task's synced publication references as well as its local artifact ledger. CLI JSON and MCP structured results carry `task_id`, `state: "partial"` and `artifacts`; the local catalog cannot prove an exhaustive Cloud inventory. Listing does not fetch bytes or mutate task/publication records. Explicit artifact fetch remains the recovery route for a known ID missing from synced notes.

The factory performs bounded host retention across registered projects on idle maintenance and above 80% disk usage. Expired artifacts retain a 14-day window and live/release citation protection. Dead-owner targets and resolved review builds are regenerable; citations into build folders do not preserve them. Source ownership, unpushed commits, active processes, ambiguous references and unsafe paths continue to preserve data. Sweep logs include per-project/category bytes and every considered path; preflight reports disk pressure with the last sweep outcome. `cas factory gc cleanup --dry-run` previews retention.

Workers request candidates through `factory action=candidate_request`. If the queue is unavailable or the host lacks its sandbox, the supervisor runs `cas qa candidate build --task <task-id> --rev <sha>` directly and supplies its receipt.

**Exact-tip freeze preparation.** Before freezing a release train, supervisors dispatch CI and confirm green for its exact SHA, audit portable dependencies, gate registration and test shape, and prepare committed, linted notes before the suite goes green. Frozen-suite jobs receive active checks between short reminders; other CI retains event-driven waits.

**Consistent config and hook policy.** Moving files into an operator write-root directory is allowed while deleting or replacing that root remains separate. User-level parent grants apply to ordinary files in nested projects. SessionStart and config get share the effective issue destination, and store diagnostics distinguish the CAS_ROOT default from explicit store selection. Pure generated agent/rule projections stay ignored; mixed harness configuration stays tracked and is regenerated from current source before the final train proof and cut.

Merge regeneration commits tracked generated outputs while leaving ignored native projections untracked.

`factory.max_workers` in a project's `.cas/config.toml` limits that project's live workers. The same key in `~/.cas/config.toml` independently limits all live workers on the machine, defaulting to eight when unset. Leaving the project key unset adds no separate project cap. Spawn requests, including batches, must fit both limits. Worker status and refusals show the machine census plus any project cap; advertised peer capacity uses the smaller remaining allowance and the existing builder guard.

An owned release train with merged work and no children in flight wakes its supervisor to run tip CI and freeze. Until recorded work changes, the watchdog repeats on its existing forty-five-minute cadence and pages the operator after ninety minutes. Frozen trains and trains owned by another supervisor remain excluded. Older trains with an explicit owner and release-train branch receive the same recovery.

Candidate preparation completion and failure notices use the existing authenticated supervisor wake path, so an idle supervisor receives the next action without waiting for an unrelated event.

Factory changed-test stability survives an unrelated failure in a broader Rust selection when each retained test has valid prior evidence and a positive passing outcome in every attempted run. Admission still removes overlapping evidence before execution; failed, unrun and changed-input cases cannot reuse it. Restored evidence covers stability only and does not claim the failed selection passed.

- **Release friction sensors** — `cas ledger report --release vX.Y.Z` and `--json` show wake notice types, confirmed hosted runner communication/shutdown losses grouped by job and runner labels, daily project GitHub issue opens/closes and agent-filed share, cross-machine park observation delays, and first park to subsequent train merge median/p90. `--park-visibility-minutes N` changes the measurement deadline (default ten minutes). Missing evidence is named; unobserved parks are censored rather than declared lost. Online GitHub captures are retained under `.cas/ledger-sensors/` for `--offline` replay. No admission, refusal or release policy changes.

Test authoring starts from production contracts and plausible regressions, keeps one strongest owner per contract, and offsets additions through consolidation. Approved test-deletion campaigns work in narrow subsystem slices, preserve the only regression guard, and record removed tests, remaining contracts and risks.

### Integrated delivery review recovery

A live registered supervisor can record a review directly for a parked delivery already integrated through a PR. The review binds its declared target tree and immutable delivery anchor, including PR follow-up commits. Unreviewed merges remain held; existing dispatch authority and all other close gates remain intact.

- **Reviewed drop at release closeout** - A member whose lines a later shipped task deleted or moved is held as reverted. The release review ledger may carry a NOT A ROUTE row for that task with `reviewed-drop: <commit>[,<commit>...] -- <why>`; closeout checks it with the same review a normal close uses (the commits exist, reach the release and touch the dropped files) and releases the member. Without a valid review the member stays held.

- **Withdrawn merge alerts** - A Director merge alert waits in the queue until its recipient's next turn. At that turn the inbox checks the alert's task: if it is no longer parked or active (changes were requested, or it merged), the alert is withheld and marked read instead of shown under MERGE REQUIRED. Only rows the daemon itself queued can be withheld this way.

- **Resolved merge identity** - A delivery whose task targets the epic's base branch merges to the epic lane under one repository and branch identity; the merge no longer refuses it as a repository mismatch. A re-parked peer delivery whose earlier local branch ref is an ancestor of the live origin tip is fast-forwarded to it, while a local ref carrying its own commits still refuses.

- Blocked-task lifecycle emission rechecks the current task status before constructing an alert episode. A resumed task no longer creates another notice from the cleared hold's fallback event timestamp. Delivered notices retain their receipts; a new blocked episode still alerts the owning supervisor.

- **Operator holds do not page at night** - A task the supervisor blocks for the operator's decision stops counting toward the stall watchdog even when the worker left an older blocker note; a block the factory still owns keeps waking the supervisor and paging once.

Was: a killed release adapter logged structured timeout fields followed by a generic pending notice.
Now: its timeout event also says which adapter timed out and after how many seconds, such as `installed adapter timed out after 45s`.

- **Supervisor startup budget** - The supervisor's startup message carries the predecessor handoff and the integrated deliveries to adopt. The complete lists live in the handoff receipt and in a recovery receipt under `supervisor-recovery/`, both named in the message. When the whole message would pass the 9 KB budget, both collapse to counts and receipt paths.

- Worker credential protection permits validated Python pathlib edits to repository source containing credential-path fixtures. Replacement strings are data; file targets still pass credential-path checks, and scripts with execution or unknown calls retain conservative inspection.

- **Batch QA ownership at park** — Resolved batch mode defers implementer evidence to the staging receipt even when target refs are unavailable. Proven integrated deliveries retain their final-close evidence path. Per-task mode still requires the individual bundle, and recorded batch evidence debt still requires its receipt before final close.

- **Disk-pressure notice counts review scratch** - When builds wait on disk pressure, the notice to the supervisor now includes the build output of finished verifier review walks, and output already queued for deletion, in its reclaimable total, so it no longer says nothing can be freed while that output still fills the disk.

- **Missing epic branch at task start** - Starting a task whose epic target branch was never pushed or was deleted now refuses with the branch name and asks the supervisor to create or push it, instead of failing with an empty git start point; no task branch is created.

- Release tag verdicts reuse recorded release closeout reviews for superseded delivery content. A review must bind the exact task anchor and unchanged ledger bytes, with its reviewed commit contained by the published tag. The ordinary delivery-drop validator checks the named replacement commits and dropped paths against that tag; absent or invalid reviews leave the member held.

- **Epic close measures shipping** - Children close when they merge into their epic's branch, so closed children do not show the work reached the default branch. Closing an epic outside a release now checks the epic's own branch against its destination (the declared target, else the default branch, local or origin): if the destination lacks commits that branch carries, the close is refused as EPIC NOT SHIPPED with the commit count, and it goes through once the branch is merged. A release close keeps its batch-receipt path and is not measured this way. `epic_status` prints the same answer as one `epic_shipping` line (shipped, unshipped with the commit count, or unknown), so the board shows merged-to-epic apart from shipped before anyone tries to close.

Test-shape lint no longer treats a Rust loop variable that iterates only string literals (`for source in ["a", "b"]`) as source text inside that loop's body. Code outside the body, a body that rebinds the name, and any shell or other-language source comparison keep the existing detection.

Changed-test discovery shares Rust inventories across files and delivery ranges within one evaluation. Each new evaluation reads its delivery again; missing-test detection, uncertainty widening and five-consecutive-pass evidence remain required. The G-61 timing log includes the task and delivery head for latency diagnosis.

- Local factory verifier startup queues a critical external job and returns its dispatch-correlated result to the requesting socket. Authority, session scope and exact dispatch claim remain checked before a verifier starts. Shared SQLite guards record the holding operation, source location, thread and hold duration for contention diagnostics; task close names its operation, and untracked external mutex owners remain unknown. A late handoff ownership or state change is a typed dispatch refusal. Store contention and dropped daemon jobs do not consume launch attempts, and the daemon's store wait budget is shorter than the client's request deadline.

- **Source citations in close reasons** - A close reason may cite a source line (`file.rs:743`) or a design revision (`DESIGN.md@rev`) beside its receipts without being refused as a missing receipt file; a citation like that is not accepted as a receipt either, and a cited receipt path that does not exist still refuses the close.

- The operator-evidence guard treats "merged", "landed", "shipped" and "green" as delivery claims that need same-turn delivery evidence (git ancestry, a passing GitHub check or run, a Cassy task close or status, or a merge receipt), not any successful tool result. "Green", "fixed" and "landed" are now checked as completion claims.

- Background task closes retain a local recovery journal until their terminal `CLOSE_OUTCOME` note is saved. Outcome recording participates in backend retirement. Lost continuations record an unknown mutation outcome and wake the owning supervisor; the daemon recovers unlocked journals after process exit without retrying the close mutation.

- **Release closeout issue closes** - After a release, closeout closes each GitHub issue its shipped tasks reference. Each close runs under its own 30-second deadline, up to three attempts with a growing pause, with at most four closes in flight; the ownership and task-state checks still run per issue before any write. An issue that still fails is listed by number in the closeout notice and stays open, so the next closeout pass reads it as open and tries it again.

- **Hub publication cause** - When Tailscale Serve cannot publish, `cas hub status` shows the cause on a row under the loopback-only warning, and the running hub writes one stderr line (the service journal) each time that cause changes. Retries of the same failure add no further lines.

- **Train main sync** - An open release train that lacks commits on the integration branch takes them in a merge on each CI poll, published to origin, and the workers with unfinished deliveries on the lane are told. A frozen train is left alone. A conflicting merge changes nothing and files one sync task under the train naming the files, plus one supervisor notice.

The test-shape checker admits whole bare stability `Identity::exact(...)` values as machine data when their binding is unambiguous. Helper calls, field or display projections, wildcard or aliased imports, and unknown macros do not grant the exemption.

- **Release freeze starts its proof** — `train_freeze` immediately dispatches or adopts one `full-validation.yml` run at the current frozen train tip. It returns the run URL or an explicit skip/failure reason; an absent run carries no release ETA. Repeating the freeze shares dispatch history with the daemon, which retains its single replacement for a run queued without jobs for ninety seconds. A repair tip gets its own run.

Knowledge build previews expose the complete pending source plan in distillation order. Previously, `cas knowledge build --dry-run --json` printed text and dry-run output showed only counts. It now emits a JSON report with `pending_sources`, `sources_scanned`, `sources_pending`, `sources_skipped` and `sources_deferred`; ordinary dry-run output lists the ordered paths too. The plan includes paths beyond `--max-sources`, while `sources_deferred` counts those beyond the pass limit. Both previews leave knowledge pages and source ledger rows unchanged and make no model calls.

- **Auto-cut uses the train owner's policy** — The machine registered for the train's `epic_verification_owner` controls automatic cuts through its project config. Setting `release.auto_cut_max_per_day = 0` there keeps the shared train unfrozen even when a peer daemon has auto-cut enabled. A live supervisor successor on the same machine can recover the cut; an unknown or remote owner declines with G-122. Freeze notes name the machine that applied the freeze or took ownership of it.

- **Cut rollover precedes proof work** — A release cut moves unfinished children to its published successor and sends each assigned worker its new target. Compatible parked deliveries retain their anchors and proof; a tip missing the successor base must merge it before testing and re-parking. Close checks cut/frozen target admission first and returns `G-100` with the successor train. Exact published ancestry and the cut train's admitted red repair preserve their existing close paths.

- A verifier launch refuses an owner whose recorded process is no longer running, even if its row is still marked active. The factory daemon names each launch refusal with a leading code, and the requesting side retries by that code: owner, scope, terminal-task, unsupported-daemon and inactive-handoff refusals are final, store contention and dropped jobs wait without spending an attempt, and anything else retries a bounded number of times. A refused owner is named by agent id, role and status, never by its full row.

- **Latest artifact in the task brief** - `task start` and `task show` end the notes with a `Latest artifact` line. It names the published artifact receipt with the newest creation time, regardless of the order notes arrive in after a sync; a receipt without a time is used only when no receipt has one.

- Supervisor briefs, `request_changes` reasons and task acceptance criteria are checked against the testing rules: asking for message-text assertions returns warning `N-106`, asking for a test per item returns `N-107`. The checks warn and never refuse. The cas-supervisor brief-fields reference lists the Test plan every brief carries.

- **Approved carry-overs keep their verdict** — A target-only scope correction can carry an existing approval to a clean merge of the reviewed anchor with its new target. Cassy checks the merge history, patch and review scope, preserves the original verdict, and records the derived candidate for merge, publication and close. Source edits, conflict resolutions, review-scope changes and explicit new delivery cycles keep their normal review requirements.

The supervisor operator-evidence guard (hook gate H-47) is retired. Nothing inspects the text of a supervisor's reply to the operator or Commander: no reply is refused, held at Stop or prefixed for lacking same-turn tool results, and the `[evidence:<tool-use-id>]` and `[unverified]` markers no longer exist, nor does the delivery-completion claim rule for "merged", "landed", "shipped" and "green". Supervisor quality is enforced at Cassy's tool boundaries, not by policing reply text.

<!-- Release prep, remove from this document: the paragraph beginning "Supervisor operator messages use `[evidence:<tool-use-id>]`", the paragraph beginning "This is a structural provenance check", the paragraph beginning "Supervisor replies can use successful same-turn tool results", and the bullet beginning "The operator-evidence guard treats". The fragment compiler only appends. -->

- `/standby` parks a factory supervisor session: it runs the finish flow, stops every worker through the normal shutdown path (never forced), records a standby marker (workers, held tasks, current handoff) and goes quiet. While parked, the daemon wakes the supervisor pane only for the operator and CAS lifecycle events; other messages wait in its inbox. `factory action=standby status=resume` respawns a replacement for each worker that held tasks, from its task queue, and clears the marker; the supervisor's first operator message does the same through its prompt hook.

Worker recycling preserves assignments through the gap between releasing the old process's lease and starting its replacement. Startup orphan cleanup respects pending lifecycle requests and live replacements in the same factory session. Codex context accounting excludes artifacts older than the actual recycle boundary, recorded after shutdown and before relaunch for that factory session and worker. A worker that has not recycled keeps its own usage, including records written before registration.

- **Repair class audit** - When a merge-queue ejection names failing tests, Cassy records them on the task. Sending that delivery back (`request_changes`) or spawning a repair worker for it then needs `class_audit`, a nextest filter for the whole test class that selects those tests. The class is stored on the task, and the worker's next park needs passing capped receipts at the tip that run every part of it. A task with no recorded failing tests is unaffected,.

- **Worker liveness and owned children** - `worker_status` and the director classify a worker from its harness transcript and process evidence. A turn that has finished reads as waiting for input only while nothing the worker owns is advancing; a background build or test whose CPU or output advanced within the stall window keeps it executing and suppresses the idle relay. A worker with no advancing child and no activity still reads as waiting for input.

### Commander standby controls

Commands includes Standby with one confirmation. A parked supervisor's conversation shows Standby and a Resume action. Resume sends an operator message through the existing conversation path; the supervisor restores its workers and handoff. The session catalog reads the same standby marker as the daemon, so the badge clears after resume.

- **Combined merge registry validation** — Before publishing a Cassy merge, including a retry onto a moved train tip, the engine runs the same gate-declaration validator as `build.rs` on the exact combined checkout without invoking Cargo. Duplicate IDs refuse with G-160 before the remote or local train ref advances.

- **Codex worker readiness and wakes** - A Codex worker the daemon launches or recycles waits for its Cassy tools before its first message is typed in. A worker that was already running when the daemon upgraded in place is not held, so its queued and urgent messages are typed into its pane. A message typed into a Codex pane records a fired wake, and the worker's turn-start hook marks it read.
- **Recycling with local commits** - `recycle_worker` and `clear_context` keep the worker's worktree, branch and task bindings. A clean worktree whose commits are only local is recycled, and the replacement starts on the same branch and HEAD. Uncommitted files still refuse the recycle.

- **Train focus follows active work** — `worktree_merge`, `train_merge`, and proof-scope retargets recover an unset or retired session focus and report the epic they pin. Cut moves existing pins to the successor; an automatic context-clear handoff saves and restores its train ID, following successor links when needed. A local peer pin does not grant ownership of the train's Cloud claim.

Worker recycling waits while a live detached build from the same worker registration and factory session holds its private target lease. The existing request resumes automatically after the lease releases, without a failure notice or manual retry. Ownership requires matching target provenance and producer process identity; unknown holders, completed producers and foreign-session runs retain visible lease refusals.

- **Carry-over content proof** — A delivery merged into a fresh branch from a moved target is measured from its fork with that target before integration. The close gate excludes inherited target content from the delivery's obligations while continuing to refuse missing delivery changes.

Widened web proof distinguishes inherited journey failures from delivery failures before a task parks. Previously, any failed journey blocked the proof, even when the untouched merge base failed identically. The worker now reuses lane CI's attribution engine to replay only failed Playwright identities at that immutable base. A matching project, path, title, failure status and error fingerprint records a release repair and permits the widened receipt. A base pass, a different failure, skipped or retried results, and incomplete attribution keep the delivery blocked. Evidence remains attached to the original failed report and the repair link; identified changed tests still owe their five passing runs.

Factory repair spawns accept an optional structural `class_audit` nextest selector with `task_id` and `spawn_brief`. Cassy validates coverage of recorded failures and saves the effective selector before enqueueing; an omitted selector retains the task's recorded class. Park requires capped proof covering that class.

A train's `epic_verification_owner` pins its decision maker across machines. A peer factory started with `--worker-host <train>` creates workers under that owner's session without launching a local supervisor. Status, blockers, parked work, corrections and final results use the durable mailbox directly between workers and their owner. A released worker claim does not remove the owner's result route.

Remote parks bind the project, owner generation, worker identity and lease epoch, task cycle, pushed branch and commit, target, and committed artifact manifest. The owner restores required artifacts and validates portable test, stability and widened-scope claims against immutable Git inputs before admitting its ordinary verification and merge path. These are worker claims; independent owner verification and CI remain authoritative. Missing evidence holds admission, and stale identities or delivery cycles cannot authorize publication. A dead owner process does not transfer ownership; an explicit transfer command remains separate work.

- **One reachability rule in Commander** - A conversation is reachable while its machine's event stream is live, or while that stream reconnects and the conversation's own socket still answers. Rows, machine chips, the footer, the header and sends all use this rule, and a send that meets a busy machine database waits and is queued in order instead of being refused.

- **Delivery CI watches stay within the GitHub budget.** A parked delivery gets
  a CI-completion watch only when its project defines the workflow being
  watched and the delivery is not already on its target branch. The landing
  watch still reports when the delivery merges. Watches for a workflow the
  project does not define are never polled.

- **One `/finish` per harness** - Cassy ships finish as a builtin skill installed once at user level for Claude, Codex and Grok (OpenCode keeps its project copy), so each harness lists it once. `cas update` removes the old personal `/finish` seed from every harness by its exact bytes and prunes a managed project copy; edited copies stay.

- **Cancelled lane CI re-runs once** - when a parked delivery's lane run is cancelled (a job past its 45-minute limit reports cancelled and nothing replaces it), the armed automatic merge re-runs that run once, keeps waiting while the original cancellation is still the latest result, and merges on green. It stops with the existing notice only when the re-run is cancelled too.

- **Team sync settles each queued row once** - A personal sync row in a team project is cleared by the first team write the cloud confirms, so it is not re-sent every cycle. The conflict journal keeps one row per discarded state; a repeat that differs only in its revision number is not recorded again.

- **Bounded conflict journal** - The sync conflict journal keeps the ten newest discarded-row copies per entity, plus the newest copy of each strategy that lifecycle guards, `cas doctor` collision reports and memory folds read. Older copies are removed when a new conflict for that entity is recorded; `cas cloud conflicts --prune <days>` still removes copies by age.
- **Bounded session ends and operator snapshots** - The canonical daemon's retention tick also removes session-end records older than `factory.supervisor_queue_retention_days`, except an unpublished supervisor handoff, one claimed by a live successor, one whose agent row still exists, the newest supervisor end on each machine and the newest end per role and name. It removes operator-turn snapshots older than `factory.prompt_retention_days` once unleased, with their prompt processed or gone and no cloud row still awaiting a receipt; the prompt-queue pass can then release those prompts. A window of 0 disables its sweep.

- **Bounded supervisor startup** - Supervisor startup checks parked integrated deliveries for 10 seconds and never waits more than 12; deliveries it did not reach are listed with their commit receipts, and close still checks each one. A compacted session keeps its registration and does not repeat the check.
- **Claude supervisor context window** - Claude's status line input reports the session's context window, which Claude transcripts omit. Cassy records it for supervisor sessions, so the automatic handoff and clear start at 80% of that window, below Claude's own compaction point.

For uncertain Jest source changes, focused proof selects the source's dependent
tests with `--findRelatedTests` from the owning Jest config root. Close consumes
that source-specific receipt; unrelated package tests no longer hold the run.

- **Supervisors stay reachable** — the supervisor's Bash hook (gate H-49) refuses a foreground call expected to run past a minute: Cargo builds and tests, release scripts, npm/pnpm test or build, loops with sleep, ssh running any of those, `timeout` above 60 seconds, an explicit tool timeout above 60 seconds, and the other commands in one known-long list. The remedy is `run_in_background: true` or delegation to a worker. Short commands, backgrounded calls, subagents and Codex or Grok shells are not affected.

- **Review completion across supervisors.** Was: an approved review could remain
  open because its original source belonged to another supervisor. Now: a
  registered supervisor can declare no delivery and close after Cassy verifies
  the integrated source base and absence of reachable task commits. The source
  audit remains intact; any recorded delivery still follows the delivery gates.

### Train owner review

Workers publish completed train children for the pinned owner to review. Artifact-only closes park with their portable evidence and release the worker lease; they do not acquire a code delivery from the worker's shared lane. The pinned owner retains final verification, merge and close authority.

A supervisor's release freeze makes it the train's release owner when the recorded owner has left, gone stale or lives on another machine. After a cross-machine handoff, the receiving supervisor's freeze makes its machine govern auto-cut. A live owner of another factory session on the same machine keeps the train. Auto-cut refusals name the owner and its machine, and the cut notice counts each delivery merged into the train once.

Checkpoint scope repairs can restore unrelated tests exactly to the active factory task's recorded start base. Test lint excludes those unchanged target files only when the branch binding is unique and the recorded base precedes the comparison base. Missing or invalid provenance retains all checks, and new prose assertions remain refused.

### Build admission on macOS

The CPU pressure threshold limits Linux PSI stalls or measured macOS CPU utilisation. macOS CPU sampling handles 32-bit tick counter wraparound; load per CPU is a fallback when no CPU measurement is available.

Release delivery commands (`cas release installed`, `cas release published`) work on a host that has never registered a project. They create the empty host registry and record the release for the supplied project.

A task's explicit target (`task update target_branch=...`) is pinned and wins over its epic's lane, including a retarget to trunk. worktree_merge merges a pinned trunk target without `allow_trunk` and still reports the trunk push. `request_changes` declines only parked deliveries. On an in-progress task it refuses and returns the ready-to-run note and worker-message calls that hand the findings back.

Focused proof for Vue components and Vitest test helpers runs their importing
specs through `vitest related --run`. Worker-check accepts the suggested command,
and close consumes the source-specific receipt without running the whole package.

### Capped test diagnostics

Failed capped Vitest selections keep redacted assertion reports under the task artifacts directory. The failure names the durable report path so the assertion can be inspected after the test process and temporary reports have gone.

Appending deliveries to an active train merge queue acknowledges the committed queue revision and each requested delivery's position. Origin reads, candidate preparation and verifier handoffs run in the existing daemon pass so slow advancement does not delay the append response.

### Verification against rulings and real output contracts

Delivery review discovers applicable stakeholder and operator rulings even when
the task does not cite them. RULING-FIT rejects implementations and tests that
contradict those rulings. FAKE-SEAM checks mocked component outputs against the
real producer's contract, rejecting impossible fixtures. Both checks cite their
sources and distinguish missing evidence from a known violation.

Automatic Codex account selection treats a login check that timed out as inconclusive, not as logged out. A spawn still picks that registered account, and the spawn's own login check verifies it before launch, so parallel spawns resolve the same account as a single spawn.

A failed capped Playwright run keeps its evidence. Worker-check runs Playwright with its reports directory under `<artifacts>/<task>/worker-check/`, keeps that directory when the run fails, and names it in the failure. Passing runs remove it.

A worker recycle (`clear_context`) that meets brief store contention stays queued. The worker keeps running, and the next factory pass completes the recycle, so the supervisor does not have to request it again.

Worker proof supports a capped package-scoped library check with default features disabled. Default and no-default compile receipts retain their configuration at the same commit and are imported separately by close, while focused test and integration requirements remain in force. Other feature overrides remain outside the worker command grammar.

The macOS operator launch smoke submits temporary launchd jobs with an explicit clean environment, captures their terminal output and command exit status, and removes the jobs afterward. This lets release validation exercise operator launches independently of the factory process running the checks while retaining the agent refusal controls.

### Read-only release previews

Release previews can migrate a private snapshot of a lagging store while retaining the live project identity and release settings. Projects do not need to add a canonical identity to their configuration to preview a release; the snapshot carries that identity privately.

Capped worker checks validate physical TMPDIR ancestry before creating temporary output or starting Cargo. A `.git` file, directory or symlink in that ancestry refuses admission with H-50, independently of Git discovery ceilings. Existing safe scratch selection and per-worktree ownership checks remain in force.

The repair class-audit gate also consumes delivery-caused test failures from structured lane-CI attribution receipts. The receipt's plan and delivery must name the same head; inherited or stale failures do not create a class requirement. Lane and merge-queue names accumulate without duplicates, retaining the first twenty distinct names and logging truncation. Requesting changes then requires a selector covering the failing tests' class, with matching capped proof before re-park. Verifier findings require a separately reviewed schema extension.

Delivery integrity follows an epic into staging and main once both its recorded tip and delivery anchor are integrated. Content and reviewed supersession use that later history without changing the task's recorded work target. Arbitrary branches and immutable review snapshots cannot supply newer proof.

A parked delivery that lands on its recorded target outside `worktree_merge`
(through a PR, a manual merge or a promotion, into any branch) is reconciled by
the factory daemon. Cassy records the target tip that contains the delivery,
then runs the ordinary close. An existing approval closes the task. With no
verdict, the close starts the post-merge verifier on the contained delivery
(at most four at once), and its approval closes the task. A gate that holds the
close goes to the supervisor once. Deliveries not yet on their target keep the
pre-merge approval requirement.

A successor supervisor can recover crash-retained committed work with one
`recover_branch` action, even when the old factory's dead worker still holds a
lease. Recovery preserves the source checkout and publishes under the successor's
own task branch before normal reassignment. It retires only the exact proven-dead
local lease; live, foreign and unverifiable holders remain protected. Releasing
the task first is unnecessary and would clear its source assignment.

A raw `gh pr merge` checks the PR's current head rollup before publication.
Known pending or failed checks hold the command with `PR_CI_PENDING` or
`PR_CI_FAILED`, including when independent QA is disabled or the target has no
required GitHub checks. Once those checks succeed, the same command can proceed
through the existing QA gates. Missing or unavailable CI evidence retains the
existing admission policy.

Merge-ready guidance asks for the repository's configured PR gate or gate-backed
queue evidence on the reviewed head and uses `--match-head-commit` when merging.
A default branch name does not establish hosted protection, and `--auto` alone
does not guarantee waiting for CI.

Parked review corrections retain their task identity across later worker
assignments when the pushed recorded source, task-identified authored subjects,
first-parent continuation and target imports agree. Another open delivery's
anchor cannot be acquired. Foreign or unnamed work retains the existing lease
fence. Attribution does not authorize a worker to restart a clean parked task:
the supervisor first requests changes, and any unfinished successor is held
before the worker starts the returned task on its recorded source branch.

Evidence restoration compares published artifact times before replacing a local
file. A newer published local ledger survives an older park manifest. A sealed
train park requires re-parking when that newer ledger differs from its manifest,
and its refusal identifies the newer artifact. Newer manifests continue to
restore older local evidence.

Commander keeps queued supervisor commands and their receipts in the local store across daemon restarts and reexec. Pending commands wait for the next native turn boundary and retain their original device and command reference. A command already claimed for typing is never replayed automatically after a restart. A daemon receipt confirming abandonment before typing shows Not sent. Session end with a potentially stale Queued status stays uncertain and reconciles late submission or outcome evidence. Receipts are bound to the daemon process that recorded them: a later launch reusing the session and supervisor names marks an earlier queue Not sent instead of typing it. Session teardown always completes, even when the receipt store cannot record abandoned commands.

A commitless spike closed with `no_delivery=true` waits for its pinned train
owner without acquiring a factory delivery branch. Its approved proof remains
usable when the owner finishes the close. The declaration still requires
positive absence measurements; tasks with delivered code retain their normal
branch, review and merge checks.

- **Publication ownership.** Was: session focus could make an operational file
  wait for an unrelated release. Now: file publication uses its explicit epic,
  owning task or artifact record, and checks each attributed report in a batch.
  A standing support report can post while a different epic is under review.

A parked delivery merges through an isolated merge that consumes its exact
anchor and leaves the worker's checkout untouched. Its source proof depends on
the delivery mode. A `push_branch` delivery needs a freshly fetched origin ref
that matches the anchor. A `local_merge` delivery is proven by its local branch
at the anchor alone, so it merges even when origin is unavailable.

After a supervisor session ends or its daemon stops, Commander asks the hub for each still-open command card's retained receipt. The hub answers from the project's local store without contacting the daemon, only for the device that sent the command. A stored final outcome is always shown; a queued command is shown as Not sent once the daemon that recorded it is gone, because nothing was typed before its claim. When no receipt remains, the card stays Unconfirmed.

### Parked close ownership during worker shutdown

A committed park can answer before evidence, owner publication and its outcome note finish. Worker shutdown, forced shutdown and recycle refuse while that close still owns its process. Queued shutdown rechecks at the actual teardown boundary and preserves task bindings and live mail on refusal.

If owner publication fails after the park, the outcome records that failure and alerts the supervisor. A durable close journal retries only owner publication during daemon recovery, using the existing outbox deduplication key. Recovery stops when that delivery has been superseded; it never repeats task close.

- **Review recovery.** Was: an expired claimed review could keep a merge blocked
  after its verifier died, requiring a manually recorded error. Now: the owning
  supervisor's merge retry can confirm local verifier death and release that exact
  claim for replacement. The candidate and review history stay intact; live or
  unverifiable children remain protected.

- **Explicit child delivery targets.** Was: a child explicitly aimed at its
  epic's base could silently inherit the epic lane. Now: the requested branch
  stays authoritative through display, start and merge. Supervisor scope repair
  can correct historical effective-target mismatches using the same stored
  branch; omitted branch defaults still follow epic lane renames.

- **Epic CI evidence.** Was: an intentionally skipped post-merge job blocked
  epic close even when the PR had tested the identical tree. Now: required
  check identities and an exact merge-source tree receipt can establish that
  proof. Missing metadata, untested changes and unsuccessful checks still
  block close; project-specific CI names are supported.

The director's 10-minute "CI unknown" merge decision applies only when CI could exist for the delivery. When every GitHub Actions workflow at the delivery tip provably skips a push to the factory branch (for example, pull-request-only workflows), the director raises no CI-unknown decision for that delivery. Unreadable workflows, an empty workflow set and filters it cannot evaluate still count as covered, so genuinely missing CI keeps its single bounded alert and clears once CI posts.

Contained historical deliveries close under an operator recovery decision. The operator runs `cas task recovery-decision --scope <epic-id> --note "..."` once per epic from their own terminal; agent contexts and agent shells are refused, and the record lives under `.cas/operator/`. The command prints and records what the decision waives for that epic's contained, measured non-Tier-2 deliveries: a `reviewed-drop:` reason for G-56 (the named target commits must still cover every dropped path), the G-64 independent QA pass for the cited commit, and the G-70 missing pre-merge approval. A live registered supervisor closes each delivery with `operator_decision=<id>` and `commit_receipt=<containment commit>`, and never with `supervisor_override`. The close records the decision ID, scope and commit, creates no pre-merge approval and dispatches no post-merge review. G-164 refuses an unknown or foreign decision, a combined override and any delivery outside those conditions.

### Slow task response recovery

When a committed task operation answers before its follow-up finishes, the MCP response identifies the action and task and includes a bounded notes request. Start responses confirm assignment acceptance and direct callers to START CONTEXT and close checks once follow-up finishes. Close responses continue to describe close follow-up.

A task show/get or notes read that reaches its response deadline includes pagination guidance and a structured recovery request with full=false, offset and limit. Notes writes retain mutation uncertainty/commit semantics and do not receive read-only retry advice.

Legacy MCP callers without a factory session use the same live-supervisor lookup as unscoped lifecycle callers. This compatibility lives in the shared snapshot resolver so direct message routing and store wrappers agree. Explicit factory sessions retain owner availability, competing-name and foreign-session protections.

A burst of directed messages to the supervisor is delivered as one turn. While the batch waits for its last delivery claim, it keeps every message it holds. A later queue refresh cannot send one of those messages on its own.

A release train's supervisor notices (cut ready, post-merge close recovery) go only to the train's owner. A supervisor that does not own the train receives none of them, whether the owner runs on the same machine or another, and focusing the epic does not change that.

A task start, queued or direct, never moves a worker's checkout while it is busy. A start that would switch, cut or reset the checkout waits with code G-165 while the worker has a live detached run or uncommitted changes. A queued start proceeds on a later queue pass once the checkout is clear. Journey runs, worker pre-push rows and foreground capped checks register a run bound to their own process, so they hold the checkout for their whole life. A start that leaves the checkout as it is, such as resuming on the task's own branch, is not held.

A task close that outlives its MCP response budget returns progress with a durable continuation ID. The task's final outcome note uses the same identity, so workers can observe a delayed park or refusal without repeating the mutation. Only a checked close or park reports a committed disposition; a parked delivery still awaits its merge verdict.

The director's "CI unknown" merge decision names its evidence. It quotes the exact GitHub check-runs query for the delivery tip and why the answer was not settled, for example skipped test checks or only a deployment preview reported. The conclusions themselves are unchanged.

- **Worker OS sandbox** - on macOS, a Claude worker's Bash tool runs in Claude Code's sandbox with a profile Cassy generates at launch:
  - **Writable:** the worktree, the artifact and scratch roots, the operator's effective write roots and the capped runner's `CAS_ROOT` directories.
  - **Git:** the worktree's own git directory, objects, `factory/*` refs and remote-tracking refs. The shared `.git` directory is never writable as a whole, and its `config`, `hooks` and worktree config are denied.
  - **Denied:** writes to protected secret files such as `.env`, keys and certificates.
  - **Network:** GitHub, crates.io and the npm registry.

  In shadow mode (`factory.claude.worker_sandbox=shadow`, the default), the text guards the sandbox replaces count would-deny facts for sandboxed Bash calls instead of refusing them. `cas gates report` shows these in its `shadowed` column. Write and Edit tools, Linux workers and supervisors keep the guards; `off` launches workers without the sandbox.

Worker registration proves the harness connected, while a native transcript turn proves it took its first prompt. The daemon watches registered Claude launches through the five-minute first-prompt grace. A timeout records a failed spawn and sends a critical notice with the promised task and recovery steps. A confirmed PTY teardown releases the preassignment; uncertain teardown preserves the binding for recovery.

The local quarantine ledger hides rows whose home project cannot be established, and a decision applies to the row that existed when it was taken. Because the ledger is keyed by bare task id, an entry older than a local task stamped with this project's identity is stale: pull applies that task's peer updates, its queued pushes are kept, and it stays on the board. Rows that already existed when they were quarantined, and ids whose local row belongs to another project or has no origin, remain quarantined.

Commander sends are taken in turn per conversation. A Send made while an earlier Send on the same conversation is still being held waits behind it instead of being dropped, so a quick second message during a network outage is held with the first and both are delivered once, in order, when the network recovers. A turn ends as soon as its message is held, before it is flushed to the supervisor.

A named Commander journey prepares its source-bound snapshot before starting Playwright's webServer clock. Memory admission can wait normally; refusal reports NOT_STARTED with reason memory_admission and exit code 75, without claiming a journey failure. Ready snapshots are verified before serving, while fixture-only and registered-hub configurations avoid local snapshot builds.

Watchdog notices that concern one task name that task in their header. When the supervisor's inbox delivers such a notice after the task has closed or been cancelled, it marks the notice stale instead of showing it, so finished work never prompts a recovery step.

Park evidence summaries stay below sixteen KiB per note. Full publication details live in the task's evidence-sync report; large manifests are published as verified artifacts and restored by the reviewing machine. Automatic evidence scans skip dependency and repository metadata trees, while small and legacy inline manifests remain readable.

### Task-bound QA metadata

Task QA predictions read the registry and product overview from the assigned worker checkout, including linked worktrees sharing a primary Cassy store. The checkout must share Git repository identity with the task's effective work target. Tasks without a worker checkout use the resolved target repository; a checkout from another repository produces a diagnostic instead of supplying unrelated metadata.

Task show provides current registry/impact context independently of historical creation notes, with structured QA impact on full reads. Task start recomputes prediction for its response and saved start advisories. Finding label validation and priority normalization use the same task-bound main journey declarations; an unrelated primary-checkout journey still fails validation. Metadata is read locally without changing the registry, checkout or historical notes.

An integrated delivery can end in a merge whose two parents carry the same tree. Content proof recovers the task's work from their common base instead of treating an empty merge delta as an empty delivery. Exact-anchor integration and later unrelated target changes preserve this proof; missing or reverted content and foreign task ownership still refuse.

### Retained worker publication counts

Factory GC reports, epic status and startup retained-worktree diagnostics distinguish `ahead_of_own_remote` from `on_no_remote`. The former counts commits ahead of the same worker branch on the selected push remote (origin when configured, otherwise the first remote); the latter counts commits absent from all remote-tracking refs. A missing own tracking ref reads `missing`, an unsuccessful probe reads `unknown`, and a repository without remotes reads `no_remote` with a separate `ahead_of_local_target` count. Reports use local tracking refs without fetching or modifying the checkout.

A checkout remains visible when its own remote is behind even if all its commits are already published on another remote branch. Clean checkouts with no local work remain absent; a missing own remote is never presented as zero commits ahead.

A parked worker receives an informational, quiet receipt when its approved PR
merges. The receipt records the task and merge commit without asking for another
close. Cassy's merge/verdict path owns finalization; missing approval and timeout
recovery remain addressed to the supervisor who owns that delivery.

Cassy bug reports search the selected component tracker before filing. Task deduplication requires component ownership: a matching remote work target, or a local task in the component's source project or explicitly configured tracker project. Product tasks mentioning the same refusal remain unchanged; genuine component matches receive the new evidence.

A worker's own pending reminder for its in-progress task is a declared wait. Stall detection skips the worker while that reminder is pending, the same way idle detection already did, and restarts its nudge-then-escalate ladder once the reminder has fired. A parked delivery (awaiting merge) or a blocked task is never treated as stalled.

Each MCP backend records its PID and version when its daemon starts. That record is the only proof of the live server's version for `worker_status`, the post-install stale-server notice and Commander's Reload Cassy. The write takes its database lock up front, so another factory process writing at the same moment, for example during a reconnect or a launch burst, delays the record instead of discarding it.

A delivery merge grants publication authority over its source delivery, not
unrelated work already on the local target. Fresh origin evidence under the
target lock detects unrelated unpublished target history and refuses with its
exact commit list. That work requires a separate publication decision; the merge
preserves source, local target and origin. Ordinary and transactional publication
use the fence, while equal and behind targets keep their existing merge path.
The delivery's own ancestry and its single integration merge are authorized.
Typed receipt failures persist before network reads, and detached candidate
validation preserves concurrent writers before a reviewed publication is refused.

Worker task admission reconciles stale leases against terminal task state. A cancelled or closed predecessor cannot prevent the next assigned task from starting, including during queue repair. Unfinished assignments and claims still enforce one active task per worker.

An explicit `spawn_workers branch=` request checks the first assigned task's effective target before queueing. Disagreements identify both branches and leave the task and queue unchanged. Supervisors correct the task through the existing target update operation, then retry; launch briefs do not grant target authority. Correctly targeted replacements start from their declared target while foreign-commit protection remains in force.

An unparked task with an untouched source cannot silently change to an integration target that excludes its recorded start base. Cassy refuses the ordinary edit and provides explicit recovery: the assigned worker first starts an Open task on its old lane, then the supervisor corrects the target and approves a clean replacement branch from the new target. The original lane stays preserved. Nonempty or rewritten deliveries keep their recorded boundary and existing recovery; dirty-work and commit-attribution protections remain in force.

Task read offsets are UTF-8 byte cursors for show, get and notes. Task list
rejects a supplied offset instead of silently returning its first page again;
list callers select results with filters and a limit.

Release membership reconciles lane CI that completed after an Advisory merge.
The release preview checks the immutable delivery anchor using the normal lane
CI classifier, accepting successful documentation checks with classified code
skips. It retains merge containment checks and leaves the historical admission
unchanged. Pending, failed, missing, and different-anchor CI cannot authorize
release membership.

A supervisor can approve a clean replacement task branch with a recovery suffix
after retiring a delivery's proof scope. Blocked recovery distinguishes retained
branch history and commit capture from active review or merge authority. The
worker resumes, publishes the fresh branch and parks normally; the old local and
origin refs remain intact, and the prior anchor stays in the audit history.

A team task push rejected for a stale revision is recovered by one pull. The pull adopts the cloud's current revision and rebuilds the queued push from the local row, including when the local row is a parked or closed delivery that outranks the cloud's older Open state. A pull blocked by a store veto or a row owned by another project still leaves the push parked as an explicit conflict.

A train-merge queue is held by its recorded supervisor only while that session is live. When the train's owner has changed and the prior supervisor has ended, the current train owner's train_merge adopts the queue, settles entries already merged and resumes it. A live prior owner still holds the queue, and only the current train owner may adopt it.

Lane CI attributes failures against the newest target commit already contained in the delivery. A worker's later train merge therefore updates the comparison point instead of leaving inherited train failures blamed on that delivery. An unavailable target can still fall back to the recorded ancestor; genuine delivery-caused failures remain red.

A close retry that completes its checks and evidence publication at the same parked tip confirms that durable park with a COMMITTED continuation while follow-up runs. It preserves the existing anchor and delivery cycle. A close still waiting for required pre-park evidence publication remains in progress; this confirmation does not bypass that review fence.

A worker can shut down after close follow-up finishes: the continuation releases its ownership lock before signaling that its work is idle.

After a supervisor approves blocked supplemental work in the same reviewed scope, the worker resumes directly with task start. An existing delivery or verification proof stays intact; progress notes carry supplemental findings. Changing that reviewed scope still requires the supervisor to recover its proof cycle first.

Reminder admission tolerates transient write contention from other factory processes. A `coordination action=remind` request retries a held write lock within the store's bounded write budget and records the reminder exactly once; persistent contention is refused with no reminder stored.

Push-branch delivery authority comes from the worker's pushed, parked anchor.
An active task or missing anchor cannot authorize integration, even when a
supervisor explicitly names it and CI is advisory. Clean, dirty and retired
workers all use the live origin proof for that immutable anchor before Git can
merge it. A local-only rebase remains on the worker, preserving the target.
Local-merge tasks keep their local delivery policy; already-integrated receipts
can still report containment without merging unfinished work.

Approved completion receipts require the same live source-ref check before
fresh integration. Receipt recovery can proceed without that ref only after
live origin containment proves the delivery is already published.

Lane CI records each own/ring invocation and replays it at the delivery head before building the base. This preserves archive identity, selection and partition while avoiding contamination from base dependencies in the shared target directory. Repeated delivery failures that pass at base are caused; failures that pass the identical head invocation remain unattributed and red. Older receipts without replay metadata and empty replays stay unresolved.

A merge retry after a failed pre-merge candidate preparation reports that failure again. The retry resolves the same source, target and context as the first attempt, and each failed attempt sends its own failure notice to the supervisor, so a held merge never waits on a background preparation that has already ended. A successful preparation still reports one ready notice, and publication still requires the exact review.

A coordination send that outlives its response deadline is never left without an identifier. It keeps running under a `send-` continuation id returned with `mutation_outcome=IN_PROGRESS`; `message_status id=<continuation id>` answers IN_PROGRESS, COMMITTED with the notification id, NOT_COMMITTED only when the send returned a refusal before any write, or UNKNOWN when no write was confirmed, and a `send_outcome` notice delivers the final outcome to the sender.

### Revalidating an already-landed task

When a supervisor reopens a task whose delivery already landed, the next cycle may only confirm that landed work on the current tree. That cycle can close with `no_delivery=true`. The prior cycle's retained commits are exempt only when every one of them is on the integration target. The new cycle must add no commit that names the task after its recorded start. Anything else keeps the ordinary close contract. Verification still applies. While the close waits for its verdict, the worker can start their next queued task, and Cassy resumes the close on the verdict.

A replacement worker on a reassigned task reaches a bound lane without a supervisor override. When the predecessor's branch is untouched, start binds the replacement's own branch automatically. When the predecessor committed work, that branch is kept, the replacement keeps working and delivers, and the supervisor's `rebind_branch` then binds the replacement's branch.

A QA ledger written on another machine keeps passing the evidence check after its evidence directory is restored. A capture cited by the writing machine's absolute artifacts path is found at the same place under the local task directory; it must still sit inside the ledger directory, and the ledger file is never rewritten.

Historical integrated-delivery recovery accepts the prescribed supervisor
command when the receipt resolves to the task's exact recorded anchor already
contained in its declared target. A completed reviewer ERROR, either legacy or
bound to the resolved dispatch, does not trap that recovery in G-70. The audit
records the supervisor, receipt, target and original error; the original verdict
and dispatch remain intact, and the new record is a verification skip rather
than a pre-merge approval. Pending or rejected reviews retain their authority.

Literal supervisor messages resolve the worker's live session owner from one registry snapshot. Unavailable or competing owners return structured reasons and supervisor status, session and heartbeat details. Same-name restart ordering and legitimate sessionless fallback remain supported, while metadata cannot borrow an explicitly different factory's supervisor.

A factory worker owns two kinds of branch: `factory/<name>` and `factory/<name>-<task-id>`. The tool guard and the installed pre-commit and pre-push hooks all apply this one rule, from the branch name alone. A branch's owner is never guessed from its name: a refusal names another worker only when that worker is registered. When the branch isn't yours, the refusal prints how to save your edits and switch back. Registration keeps worker namespaces exclusive: a worker name cannot extend another worker's name across a hyphen (`ab-owner` beside `ab` is refused), while `eager-panda-2` and `eager-panda-27` both register. Supervisors own no factory branches and are exempt.

`transfer adopt_branch` adopts the delivery a task actually holds, never anything past it. It works when the receiver already sits on one of the task's branches. It also works when the receiver is at a newer train tip with no work of its own, and after a review return has moved the task's anchor into history.

Finding label updates that claim main reach without a declared live main journey emit gate `G-161`. The existing `finding_main_journey_required` rule data and preservation of task fields remain unchanged.

- **Worker OS sandbox, per task** - a sandboxed worker's Bash writes reach only its current tasks' artifact directories and its own checkout's scratch, never a sibling task's evidence. The profile is fixed at launch, so starting a task it does not cover relaunches the worker with a new profile; the task stays started and the relaunched worker resumes it.

A recorded capacity wait (an admission refusal with its scheduled retry, fenced to the worker's current task lease) counts as a worker wait, like a running build. The director withholds idle and detached-exit notices for it; the capacity-wait escalation after a long wait and notices for real check failures are unchanged.

A worker with a spawn or recycle still in flight in the durable spawn queue is not idle. The director withholds its idle and pending-delivery notices until the replacement registers, then resumes the normal one-notice-per-episode behaviour.

The recorded start boundary decides which branch close measures. After a rebind, the rebound lane is the close-measured branch whether its tip is local, only on origin, or not yet committed, and the merge-reality check judges that same lane. The old per-task branch is never measured in its place.

Spawn keeps worker namespaces exclusive across machines too. A new worker name is refused (`WORKER_NAME_OVERLAPS`) when it would share a `factory/` namespace with a registered worker or with an existing local or origin `factory/*` branch. A name's own task branches never block it. `cas factory` applies the same check to its initial names, including `--worker-spec` names, and workers added later from the GUI or WebSocket go through it before they are bound or launched. It stops when the registry cannot be read. Generated names, including the daemon's fallback names, skip such candidates, and overflow minion names read `kevin2`, not `kevin-2`. `cas doctor` reports any overlap that already exists in a `worker names` row, with the way back, and changes nothing.

A changed Playwright helper with no tests of its own is proved through the specs that import it. Worker-check runs each importing spec under the Playwright config that lists it, and park accepts the helper only when every importing spec has a passing capped run at the tip. A spec run on its own under its config counts the same way, and so does its ordinary five-pass stability run when that run covered every test of the spec; a partial selection, a missing spec or an unrelated spec does not.

Park evidence publication is limited to declared evidence: top-level files in the task evidence directory, LEDGER.md and the paths its rows cite (read exactly as the ledger gate reads an evidence cell, including absolute and other-machine paths), files the close reason cites, and the `qa/` bundle and `terminal-qa/` receipt trees Cassy's verifier reads. Nested files nothing cites are listed in the evidence report as "uncited; not published" and are never uploaded, so a source snapshot or raw capture tree kept beside the proof cannot flood the task with artifact notes or exhaust the close deadline. Routed builds still require their ledger citations, and a cited file that is missing still refuses the park.

## Primary Journeys

Factory merges combine disjoint journey registry entries by ID. Shared-file notices let workers proceed with separate overview, codemap and registry entries, and ask them to coordinate when edits touch the same entry.

Completed native Claude, Codex and Grok turns mirror into durable Commander history, including answers to questions typed in the terminal. Replies selected just as Commander disconnects return to the queue with a saved one-second retry. Each actual forward saves its attempt count and next retry; a concrete paired-device receipt settles replies addressed to "*". A receipt write that hits SQLite contention retries on the store worker. A selected reply with no device receipt expires fifteen minutes after its first selection and tells its sender once, including automatic mirrors. Never-selected offline replies keep waiting; ordinary expired transcript replays retire quietly while their durable operator outbox keeps retrying. Recent ask/blocker mirrors and answers linked to verified Commander or local terminal input can raise one batched supervisor notice per sweep only after five minutes without a device receipt and before the copy is an hour old. Their own creation time controls the window; per-row markers prevent repeat alarms. Status, receipt and unlinked answer copies stay quiet. Explicit undelivered operator asks and blockers retain delivery alerts. Long key-shaped credentials are redacted while ordinary hyphenated words stay verbatim. Grok text streams appear only after native turn completion; thoughts, text before tool calls and cancelled turns stay out of the reply. Native synthetic prompts never become operator questions, and text/image chunks in one pasted prompt keep the answer linked to that question. OpenCode transcript mirroring remains unsupported.

A Commander history read that receives no answer within 15 seconds shows an error and Retry history. Retrying an older page preserves its cursor; cached turns remain readable without implying the history is complete.

Commander omits unsolicited supervisor replies that only acknowledge a message or say there is no action. Direct answers, substantive status updates and native Claude API errors remain visible; a failed turn appears in the conversation and session preview without waiting for another turn.

Claude factory launches prefer the native installer’s active binary, then its newest installed version, then PATH. Commander checks the same executable before launch. Each launch verifies Claude Code 2.1.280 or newer and refuses with the detected path and version when no compatible client is available. Hub service status gives missing, outdated and unverifiable Claude clients their own install, update or repair action; missing clients remain not installed in doctor and the account picker.

Commander labels each machine's reported Cassy version in its project chip and paired-machine card. A newer published stable release adds a quiet update note; an hourly cached lookup retains its last known release while offline. The Web footer separately identifies the client release and build. Each online supervisor with a known MCP version also shows its own version badge; an upgrade arrow marks a supervisor running an older Cassy than its machine has installed, with reconnect guidance. Unknown versions stay unreported.

Commander's conversation header keeps identity and actions on one row with the observed state in a pill beside the project title. On a phone Raw output and Interrupt remain direct controls; Commands, Show work, Tasks & progress and session attention use the native overflow menu, whose visible invoker retains the attention count and return focus.

Commander's left navigation pins Needs you at the top with each waiting question quoted (retained machine-code summaries use the message’s first sentence), then shows one compact card per project: machine dots, a mini release-train bar and one status line of train progress, workers and blocked tasks, read from each running session's status at most once a minute. Sessions are one line each; versions appear only on skew, and quiet projects are dimmed. Commander lists running conversations only. Ended and crashed sessions leave navigation without dismissal or attach retries; empty project cards disappear. A machine outage keeps its last-known running conversations while recovery is shown at machine level. Waiting messages to a confirmed stopped session become Not sent, with their words available in Saved messages; running replacements return without a reload.

Project cards keep the Most recent badge on the same session while sibling activity alternates within a whole minute, using stable session identity to break ties. Activity in a later minute moves the badge without shifting rows.

The operator can paste text, screenshots or files into Commander: long text becomes a previewed text attachment, and Send stores files on the session's machine and gives the supervisor readable local paths through the existing durable message lane. Pending files remain through reconnect, with a reminder to send before closing the page. The operator's own sent files appear in their bubble as image previews and named file chips after reload and on other paired devices, without displaying host paths.

The composer also offers a local, lazily loaded emoji tray beside the mic: search or choose a recent emoji, replace the current selection and continue the same draft. Messages, previews and waiting questions preserve Unicode text while selecting native color-emoji fonts.

1. The operator launches `cas`, reviews the active epic and staffs workers for its ready tasks. Worker startup and continuation deliver full assignments through private launch files inside the worker worktree, including harnesses that deny external-directory reads; process arguments carry only a short read instruction and path. Worker teardown removes the brief file.
2. A worker starts its lease, commits in its worktree, runs targeted tests and pushes; the supervisor merges on green, and the verifier's verdict at the merge commit closes or reopens the task, unless its review tier needs no verifier.
   In batch mode, a later exact release verdict also clears an expired Tier 1 dispatch and deferred batch debt. Its receipt is acknowledged once recorded; delivery closes that exceed the response grace continue through the ordinary gates, with each outcome retained as a bounded task-note summary. After a receipt’s member closes settle, one recorder-addressed notice reports the closed count and held deliveries grouped by refusal kind, each with its exact gate and next action even when its worker has retired. Full per-member details stay on task notes; replaying that receipt deduplicates both outcome notes and the grouped action. Batch, post-commit and late-close system summaries cannot cite producer evidence. Long close remedies are retained in immutable receipt files with complete pointers in their summaries. Restored and synced producer citations get their own timestamped note, and another covering receipt adds no outcome note to an already-closed task. A new supervisor receives integrated project deliveries across all review tiers as close actions with current holds at startup, independently of the retired session's handoff preview. Unreadable verification rows are skipped and reported without losing the inherited handoff or other recovery rows. Active reviews, rejected verdicts and lost content stay outside recovery; dedicated reviews and Tier 2 probes remain required by close.
   Retired or unregistered implementers keep required verification, including for no-code external work. Close names the missing registration accurately and records deliberate policy exemptions or supervisor waivers separately; an outstanding review or error retains its authority.
   Shipped historical tasks with lost delivery anchors can use supervisor-only `task action=recover_delivery` with an attributable integrated `commit_receipt` and reason. The audited binding restores Git actor lineage before independent batch recording; it preserves review and close obligations.
3. A QA reviewer runs `cas qa impact` on a delivery, scaffolds evidence for the task's recorded source branch tip with `cas qa ledger init --task <id>`, walks the touched journeys, and runs `cas qa ledger check <path> --task <id>` before submitting the verdict. The same format check refuses malformed evidence at close before verification dispatch.
4. A supervisor messages a supervisor on another project or machine, sees RECEIVED or PENDING, or splits an epic with that peer.
5. `scripts/release.sh` prepares releases on the canonical open release train, reuses its PR and checks queue admission once. Before pushing, its existing cut preview refuses any of the release’s own included deliveries with missing review and names the receipt action; older-release review debt remains a residual and memberless hotfixes retain their existing flow. Release admission requires singleton landing with `ALLGREEN`, while queue builds remain parallel so the landed tree keeps its prebuilt archives. Durable cross-session SHA/tag reminders replace PR and CI polling, including with `--no-watch`; hotfix/non-train branches retain the caller's base flow. A matched train persists its cut marker before push/enqueue under the merge target lock; marker failure stops with a retry command, while reminder-only failures stay advisory. Version re-bumps cancel superseded watches. Pending auto-merge is distinguished from admission, and one failed required-check snapshot is reported without polling. The install timer verifies publication and installation, then queues one owner-addressed post-release action; failed delivery retries without reinstalling. Publication receipts show first-child commit to GitHub mergedAt (30-minute target), mergedAt to publishedAt (20-minute target), and observed queue CI attempts; absent measurements remain unavailable. Release preparation proceeds through the merge queue; release lessons follow that command. During preparation, it checks task fragments and explicit trailing task IDs on non-merge commits against Git delivery history, accepts patch-equivalent fixes and reports all missing tasks together. The retired local train is removed, while CI and pre-push checks retain their fast-row catalog. After a release merges, `cas serve` runs the post-release check and, in batch QA mode, sends the supervisor the exact batch receipt call once per target branch and live parked task set for deliveries already live on that release (unrelated merges do not repeat it), `cas release report` builds the branded release report, and Violet posts the release notes to Slack; `cas violet channels` and `cas doctor` check the configured channels. After publication, the release workflow also updates `hub.petrastella.io` from the released Commander bundle when its files change, verifies public access and bytes, restores the previous deployment on a failed check, and attaches deployment provenance to the release.
6. An operator prepares a client-branded SOW delivery report with before/after evidence for each requested change; `cas deliverable finalize --audience customer` brands it, embeds its screenshots, refuses internal vocabulary and prints and checks the A4 and Letter PDFs. For a client product, `cas-customer-release-notes` writes the period's customer release notes from its `docs/deliverables/PROFILE.md`; the client's `DESIGN.md` shapes the layout, and the same `finalize --audience customer` call produces the HTML and PDFs.
7. An agent calls `cas serve` tools to find tasks, memories, rules, skills and knowledge while it works.
A project hold is an operator-owned full stop: `cas factory hold set --reason
"<reason>"` records the operator and time; `cas factory hold lift --reason
"<reason>"` records the resumption. Startup shows it first. Ready and
awaiting-merge lists, merge requests and supervisor-stalled relays wait, while
`spawn_workers` and `worktree_merge` refuse with the hold's reason. Existing
unread action relays retain their receipts and resume after the lift; ordinary
messages remain readable. With no hold, normal dispatch and staging delivery
admission are unchanged. Internal close, ownership and recovery checks keep
seeing parked deliveries under a hold. READY TO MERGE generation pauses before
queueing, so lifting the hold releases no stale READY backlog.
`cas --json factory hold status` shows the full audit.

   Measured CPU or output writes from owned children keep quiet workers executing
   and reset factory progress; sleeping or exited children do not. Worker/session
   and process-start identities fence this evidence.
   Delivery-watchdog nudges name their notification and stop after it is read or
   acknowledged; fresh unread directed mail keeps its wake.
   The progress watchdog wakes the supervisor after 45 quiet minutes of local work
   and pages the operator after 90. Parked delivery anchors already contained in
   their recorded local or origin target, and work or target epics assigned to a
   peer by a binding epic split, leave the local waiting set. Blocked tasks whose
   current blocker belongs to the operator also leave that set; a newer
   factory-owned blocker restores escalation. Missing Git evidence and ordinary
   prose-only deferrals retain escalation; local in-progress work still counts.
   Tasks blocked by open dependencies and tasks assigned to held workers also
   leave the waiting set. Resolving their dependencies or releasing the worker
   hold restores ordinary escalation for work that remains actionable.
   Parked deliveries whose target train is frozen for release also leave that
   set: the supervisor hears once per freeze which tasks it holds and that only
   the operator lifts it in Commander; release repairs still count.

8. The Commander shell Accounts icon lists accounts by machine and provider, with login state, capacity and Eastern 12-hour reset times. Each Claude/Codex row in Accounts and New session has Check usage: it rereads the same native snapshots as `cas limits`, shows Checked at separately from Usage as of, and names stale or missing data with a refresh action. It never starts a model turn or switches accounts. Machine-read pairings can inspect; factory-manage pairings can confirm removal/disable, enable, rename or set the default through audited operations. Grok is read-only. Account changes refresh New session without a page reload. New session recommendations follow the selected project’s account selection and threshold policy and refresh when the project changes. The operator also runs `cas accounts list` (or `--json`) to compare Claude and Codex subscriptions by email, usage and reset time, and identify duplicate or dead folders before choosing a session account. Claude/Codex pickers and Commander show local reset dates/times with relative waits for 5h and weekly usage and highlight a recommended subscription with reset/headroom reasons; operators can choose another account. Workers use the recommendation over an inherited requester account; explicit accounts win, and the requester remains the fallback when no account is eligible. A worker usage limit leaves it waiting on its current account and sends one supervisor notice per episode naming the account, held tasks and absolute local reset date/time (or explicitly unavailable). No replacement is queued; pre-upgrade queued switches are refused. Episode records and notice dedupe survive daemon restarts. Every Claude/Codex launch, worker spawn and Commander session first links the chosen account's missing shared items (skills, agents, hooks, settings) to the provider base. On macOS, account linking keeps active or unverified Claude authentication at its original path, including Keychain logins without a credential file; an authenticated default that already links to the base does not count as drift. It names a retired folder that came back. `cas doctor --fix` repairs link drift, and `--yes` backs up and replaces real items.
9. The operator invokes the builtin `finish` skill (`/finish` in Claude Code) to stop the background work it started, clean up session work and save a handoff and learnings; a handoff that names each task the session still holds lets it end with those tasks left open; updates distribute it across harnesses while preserving personal overrides. From Commander, `/` in the composer or the header command button offers curated Claude/Codex controls under a paired input lease (HUB-J27). Clear, Exit and Finish confirm. Finish queues until the native turn ends and its prompt is empty; background shell footer output does not keep an idle supervisor busy. Exit and Reload Cassy use the same native turn completion signal while retaining empty-prompt, draft and modal checks. Claude local-command transcript entries do not start a model turn and appear as compact terminal receipts in the thread and preview. Receipts distinguish queued, submitted, observed and uncertain outcomes, and the Finish reply mirrors to the conversation. Autocomplete consumes its selected slash prefix and closes; both command entry points use a compact status receipt. Long Finish turns retain read-only outcome checks after the short confirmation timeout, so their own later native completion settles the status without another command. Finish names a completed harness request, while Exit follows the authenticated session-ended receipt. Usage recognizes Claude’s session-cost and quota views, and an uncertain Usage receipt retains Close so the operator can return to the prompt before sending again. Claude supervisors offer Reload Cassy directly on a card whose MCP is older than its installed binary, and through the conversation-header command menu or typed `/mcp`. Ordinary cards keep their density and one navigation Tab stop per conversation. The existing command reconnects their managed MCP server and confirms its replacement PID and version from the registry. The header slash button opens its dropdown beside the button; Dismiss cancels a queued Finish or chooser tracking in the daemon without typing into the pane, while submitted commands still await their outcome. Closed native choosers release stale tracking before later commands, refusals name the blocking command and its recovery, and accepted retries replace earlier refused busy rows. Other harnesses receive a named refusal; the owning daemon must already contain the action.
10. In Commander the operator turns on Show work to see the supervisor's thinking, tool calls and task moves between its replies, collapsed until opened, and opens a worker's live timeline by its name in Tasks & progress instead of watching its terminal (HUB-J25); card and timeline state words use the same reported lifecycle/liveness projection, with no separate Activity pill; on a phone it is a bottom sheet whose note keeps the worker's name whole and whose buttons align with the note's text. Worker task cards, Show work task chips, timeline subjects and Write access task choices lead with task titles, with IDs retained as secondary metadata. Typed tool rows name reads, searches, commands and edits with their reported scope; opening one shows its targets or command metadata alongside the exact tool record. Failures are named, and unfamiliar tools retain their raw text. The daemon reads these from existing transcripts and the task DB with zero model tokens, and pushes a contentless change notice to clients that read a page. Tasks & progress shows one quiet evidence line when no release train is reported and keeps unavailable merge counts in evidence text. With a reported release it leads with focused-release merged/total, merges per hour and last merge, and groups tasks from the supervisor's open epics as Needs you, In progress and Ready with All tasks available. Its desktop width is adjustable by pointer or keyboard and remembered per browser; permission guidance replaces unavailable header controls.

11. In Commander the operator reads safe Mermaid diagrams, expands unified diffs, and inspects inline image, video, audio and PDF first-page attachments. An image card opens a zoomable, pannable gallery of that turn's images, names loading failures, and returns focus and reading position on close (HUB-J4). A supervisor can also place a screenshot inline in a reply with `![alt](artifact:<id>)` or a path under the project's artifacts directory; the machine re-proves it as a PNG, JPEG, WebP or GIF of at most 10 MiB, and anything else, including a remote URL, stays text. To judge a Commander change itself, the operator runs `cas hub preview --dist <build> --task <id>`: the candidate build is served on its own loopback origin, paired once, reads live sessions read-only (the hub refuses every mutation from it) and is revoked when its window ends; meanwhile that task's awaiting-merge row links to it as Preview this delivery. Fenced source has a language or filename header, Copy with visible success or failure, and a per-block Wrap toggle for phone reading. Source and original files remain reachable; previews reserve their space and rich renderers load on demand.

12. A supervisor attaches an HTML page, such as a report with diagrams, images or video, to a reply, and Commander shows it inline beneath the file card, in the conversation's light or dark theme, and opens it in its own tab (HUB-J4). The publishing machine serves the page through a short-lived ticket under a CSP sandbox: no script, an opaque origin, no top navigation or popups, no network beyond its own assets, and framing allowed only from the Commander origin that asked. Agent markup never enters Commander's document.

Pre-merge reviews launched by the MCP daemon start the existing task verifier
automatically, carrying the candidate, criteria, receipts and supervisor review
notes. The merge returns its held dispatch after claiming one verifier; account
probes, prompt preparation and startup notices continue in the background.
Approval continues merge admission and leaves a quiet receipt; rejection,
questions and failed or expired reviews wake the owner with findings. A single
delivery that conflicts before candidate review reports CONTENT CONFLICT with
the paths and reopens the task to its assigned worker, invalidating the old proof
without changing the target checkout. Host-path
redaction preserves surrounding questions in review notices and verification
show/latest while hiding the path itself. Manual
verifiers keep their existing handoff and authority. Automatic reviews select
a provider account through the shared launch resolver; failed harness notices
include the stderr tail and full log path. A crashed automatic verifier releases
only its own unresolved claim, consumes its old authority and retains an audit
of the failure. Retrying the same merge starts one replacement on the same
candidate; stale verdicts are refused and each attempt retains separate logs.

Native Task/Agent subagents and Codex children share their parent's MCP
caller. Shared connections refuse child registration before changing caller or
session state. The daemon-owned verifier reviews no-code close dispatches as
well as merge candidates; retrying the close or merge that opened a dispatch
starts one replacement after a stopped verifier. Dispatches, retry notices and
verifier instructions name only those routes and a supervisor's direct verdict.
A supervisor's own review, recorded with supervisor_direct provenance, covers
mechanical evidence only; designs, spikes and operator-approved deliverables get
an independent reviewer. A native child returns identity evidence without
submitting even an error verdict.

Factory turn costs are recorded locally for Claude, Codex and Grok supervisors and workers, independently of Commander mirroring. `cas ledger report --release vX.Y.Z --offline` reads release-window ledger rows to show wakes, no-op wakes, input/output tokens and minutes by channel, with counts for missing measurements. Wake channels come from authenticated queue provenance, including a session-less daemon row addressed to that exact agent. Without a row, a Claude background-task notification counts as the agent's own role, CAS's fixed worker startup and delivery-watchdog texts count as daemon, and any other input counts as terminal (unauthenticated pane input). A no-op means no successful supported action was observed. Prose counts as an answer only for an authenticated operator wake or a question verified against its authenticated queue row; Teams wrappers and pointer wakes use the original row provenance, and launch-context messages do not take the wake slot. For other wakes, only typed tool actions count: task, coordination and edit writes, factory and verification writes such as merges and spawns, and subagent launches. Reads, waits, receipt-only replies and status restatements do not count. Shell actions are conservatively recognized only for explicit commit, push and check commands, including the patches and commands inside a Codex code-mode `exec` script. OpenCode turns are not recorded yet. Unsupported Cloud Turn events remain queued locally behind their own push cursor and six-hour kind cooldown; other ledger kinds keep pushing, and every sync round attempts pull even if push fails.

Merged delivery close reads test receipts from the assigned worker's registered checkout even after recovery changes its path or branch. Receipts remain bound to the delivered commit and declared repository; the original proof target declaration stays unchanged.

A supervisor command sent from Commander while the supervisor's native turn is open is queued and typed once at the turn boundary, the same way Finish already waited. The card stays Queued until then and can be dismissed without typing anything. If the session ends first, the card settles with the end reason.

- **Account recommendation by headroom** - Workers, `cas claude` / `cas codex` and Commander recommend the subscription with the most headroom on its binding window, the fuller of the 5h and weekly windows, with ties broken by the earliest reset. A window whose reset has passed counts as rolled over, and each spawn rereads the native usage snapshots before choosing. `accounts.selection soonest-reset` restores the earlier policy of draining the account that resets first.

Commander keeps zero-rate quality improvements finite and distinguishes successful table cells from actual failures, including cells that state a zero failure count. Visual QA uses the explicitly selected Playwright package and detects page-level horizontal overflow on phone viewports.

### Violet fallback, client findings and macOS update identity

`cas violet post` accepts explicit reply intent and generation fields alongside a stable request ID. `cas violet react` adds or removes reactions. Reconciled reply failures retain provider diagnostics without repeating writes.

The shared `cas-client-report` skill covers client findings and recommendations, reusable accounting validation, responsive HTML and print inspection. SOW completion, feature showcases and customer product updates retain their specialized workflows.

On macOS, updates can use an operator-created local signing identity to preserve their designated requirement across versions. Creating that identity is opt-in; the updater does not change system privacy or Gatekeeper settings.

### Cloud authorization and isolated proof checks

Proof publication, peer messaging, message-status reads and agent discovery retry one 401 response that identifies an invalid API key with the current stored login or a verified environment credential. The environment credential is scoped to its configured endpoint and shares the direct-token login path. Other 401 responses retain the server cause without retrying. Failed credential recovery reports `Cloud session expired, run cas login`; transport failures and uncertain writes do not trigger this retry.

Capped tests use short scratch paths outside a checkout and protected `.cas` ancestry. The credential-fixture sentinel also covers arbitrary login tokens, so an intended no-project fixture cannot overwrite the operator's project or user login cache.

No-code factory workers can start artifact work while a shared checkout remains on another worker's branch with pending edits. Isolated worker checkouts keep their normal source attribution. An artifact-only task can sync published trunk for report tooling and close using portable proof; an unpublished source commit still requires delivery handling.

When repository discovery is incomplete, the lifecycle receipt names the requested selector and branch, the roots that failed or were not reached, and the identities successfully read. It retains the declared work target and does not infer a storage outage or select a different branch from incidental Git history.

When a replacement worker inherits a departed worker’s task delivery, task start records the replacement’s owned source branch after proving that it contains the previous source tips. Queued spawns preserve that resumed lane. An already parked source mismatch can be returned with `request_changes`, then approved through `rebind_branch` before the worker starts and parks again.

Train merges accept added deliveries while a review waits. Advancement uses a bounded lease, so a stalled or departed driver cannot indefinitely hold an approved queue. Recovery records the retired owner and retains normal supervisor authority, verification, CI and publication checks.

Factory CI excludes merged epic refs and bounds task-parent selection to 64 live refs and at most 127 ancestry comparisons. Overflow retains conservative validation. Release publication prunes remote epic tips contained in the published default-branch commit; deletion uses an exact-tip lease, is logged, and safely repeats on later releases.

A green release-train CI result checks the current train state and containing version tags before asking the supervisor to cut. Shipped or closed trains produce no new cut-ready notice; an unreleased train still notifies once when it first turns green or recovers from red.

Task parking publishes the ledger and its cited evidence before optional copies. Cloud begin, upload and completion share a twenty-second publication budget, including one retry while time remains. Unpublished optional files are listed in the evidence note; previously committed evidence is reused on retry.

Release marker pushes can reuse recorded fast-row proof for a committed tip even when another commit is checked out. Proof remains bound to the pushed commit and comparison range, including web output when required. Missing proof directs the operator to a clean checkout where the pre-push hook can perform the complete validation.

Every delivery park publishes its cited local evidence for the reviewing machine. Missing files or failed required uploads refuse the park with the unpublished paths so the worker can repair them before a peer review starts. Existing committed evidence is reused on retry.

On ordinary deliveries, local QA artifacts that the close reason does not cite remain optional for publication, preserving log-only and evidence-waiver parks. Routed deliveries retain their required ledger evidence.

Train owners can answer a peer-machine verifier's `supervisor_call` without moving train ownership. Read the public question in task notes, then use `verification action=add` with its task, dispatch, `status=supervisor_call` and answer in `summary`. Cassy queues an authenticated authorization to the dispatch's builder supervisor; that supervisor records the identical payload and the independent verifier continues. The authorization does not grant a final verdict, merge or close permission. Changed questions, deliveries, responders and train owners require fresh authorization.

Cross-machine and in-factory messages report delays honestly. A supervisor's peer message that the peer still has not received two minutes after Cloud accepted it alerts the sender once, with the Cloud message id to check. A message to a worker that is being recycled waits for the worker's successor instead of being abandoned. A lifecycle event queued late says how late it was queued. An idle notice never names a task the worker received after the idle state was detected.

A directed message to a recipient that is mid tool call, such as a worker's blocker to its supervisor, waits until the call ends. The daemon offers a wake again every 10–30 seconds, and the recipient's next tool-call boundary shows the message. Each re-offer claims the message briefly. Releasing that claim, whether by parking behind the busy recipient, by a token release or by a restart sweep, signals the recipient's inbox again. A tool boundary that checked during the claim therefore still reads the message at the next boundary.

Personal and team Cloud pulls can reconcile an owner's older close over a receiver's stale Blocked or AwaitingMerge status when both records identify the same approved, merged delivery cycle, anchor and target. This projection retains the receiver's newer body and notes while dependency gates observe the closed blocker. Later body-only refreshes retain their configured conflict policy. Reconciliation respects explicit rework, changed cycles and targets, LocalWins policy, and the store's atomic mutation and live-delivery guards.

Commander relay delivery rechecks a prepared claim on the store lane before handing it to the pane. Inbox reads and explicit acknowledgements that happen while the claim waits retire that prepared delivery without inventing a transport or confirmation receipt. Unobserved successor messages and fresh messages still reach the pane.

A builder receiving request_changes can complete a corrective delivery and re-park it through team sync. When the owner retains an Open replica with a newer bookkeeping clock, reconciliation requires the owner's exact latest request_changes audit followed by the builder's canonical park, with the same delivery cycle, recorded start, target and assignee. The shared park reconciliation retains newer owner body and notes while preserving ordinary Open and rework holds. Train publication and exact pushed-anchor checks remain unchanged.

A live supervisor can hold a parked delivery that waits on an external review, such as a product owner's GitHub approval. The hold names an owner, a reason and a deadline of at most 7 days. Cassy stamps the supervisor and the parked anchor and records the owner and reason in the task notes. Until the deadline, the progress watchdog leaves that delivery out of its stall count, so it neither wakes the supervisor nor pages the operator for it. A lapsed deadline or a re-park at another anchor makes the delivery actionable again, and genuinely stalled work still escalates. Workers cannot set the hold.

Task transfer reports when a receiver belongs to another machine, including a live worker found through the existing scoped peer roster. The receiving machine's supervisor must perform adoption in that worker's own checkout. Peer discovery is read-only and does not register the receiver locally or change assignment, lease or branch state. Local transfer and adoption retain their existing validation.

A delayed lifecycle message names a recorded hold as its cause only for the time that hold covers. Queue rows record when their first hold began, and a repeated decline keeps that start. When the hold began more than nine minutes after the message was queued, the label states how long the earlier wait went unrecorded instead of crediting the late hold with the whole delay. Operator drafts in the supervisor composer stay protected as before.

Child verifiers receive local paths for cited parent epic plan and sequence artifacts. The receiver resolves each reference within the parent task's project scope and downloads verified bytes into that parent's evidence directory. Missing, revoked, foreign or corrupt proof stops preparation with an explicit artifact error. A bare artifact ID grants no access, and an added parent proof reference invalidates the candidate context.

Personal and team Cloud pulls reconcile a peer's completed park over an older InProgress or Blocked lifecycle whose row clock advanced through local bookkeeping. The park must match the recorded start, assignee, delivery cycle and target, include its canonical park audit, and bind the recorded branch to a full commit anchor. The receiver retains its newer body and notes. Train ownership, verification, remote park admission and exact pushed-anchor checks continue to govern publication.

Automatic verification resolves a delivery's committed artifact references through the existing Cloud fetch path before starting its review command. Each download is task-scoped, checked against its published size and SHA-256, and placed under the reviewing machine's project namespace. Structured review inputs name the artifact ID and local path. Unavailable or corrupt evidence holds verifier startup with the artifact ID and Cloud error, leaving the same dispatch available for retry.

Commander's live regions and controls carry accessible names: the conversation's connection chip ("Connection"), message status, notices, the command palette ("Commands"), pairing status, fleet progress and results, and the outage banner. Screen readers can therefore tell each announcement apart. Elements that journey tests check, but that have no meaningful role, carry stable `data-testid` attributes. The Commander journey suite finds the interface by role, accessible name and test id.

## Authoritative Sources

Delivery checks use observable behavior and stable error or rule identifiers.
Park checks new error-message assertions for prose pins; vocabulary and required
instruction sentences do not decide whether a delivery or release can proceed.
Exact structured command and redirect-command projections are valid machine
identifiers; assertions on their surrounding human error sentences remain refused.

- Product pitch and user-facing surfaces: `README.md`.
- Coordination and context-system boundaries: `cas-cli/docs/ARCHITECTURE.md`.
- Task lifecycle and close states: `crates/cas-types/src/task.rs`.
- Agent roles and session responsibilities: `crates/cas-types/src/agent.rs`.
- Memory, rule and skill records: `crates/cas-types/src/entry.rs`, `crates/cas-types/src/rule.rs`, `crates/cas-types/src/skill.rs`.
- Adaptive QA: `cas-cli/src/qa_registry.rs`, `cas-cli/src/qa_impact.rs`, `cas-cli/src/qa_ledger.rs`, `cas-cli/src/qa_post_release.rs`, `docs/qa/impact.md`, `docs/qa/docs-upkeep.md`.
- CLI and MCP surfaces: `cas-cli/src/cli/` and `cas-cli/src/mcp/tools/`.
- Commander browser app: `hub-web/`.
- Analysis reports: `cas-cli/src/builtins/skills/cas-html-reports/` (supervisors brief from `references/supervisor-brief.md`). Authors and reviewers check the originating goal, criteria, evidence and plan verdict before scoring form; added thresholds and recommendations stay separate from the plan verdict; the G7 close gate (`cas-cli/src/deliverable/report_quality.rs`) refuses a changed `docs/` report missing its concept brief, critique floor, brand tokens, verdict, height budget or audience-clean text, and lists each gap.
- Release and delivery reports: `cas-cli/src/builtins/skills/cas-release-report/`, `cas-cli/src/builtins/skills/cas-sow-delivery-report/`, `cas-cli/src/builtins/skills/cas-feature-report/`, `cas-cli/src/builtins/skills/cas-customer-release-notes/`; all finish through `cas deliverable finalize` (`cas-cli/src/deliverable/finalize.rs`), the one path for image inlining, audience hygiene, printing and the PDF check (`cas-cli/src/deliverable/assets/`). PDF printing uses an explicit or repository-pinned Playwright module and its matching browser, falls back to full Chromium in headless mode when its shell is missing, and preserves launch diagnostics with an installation remedy in text and JSON.
- Branding of human-facing output: `cas-cli/src/builtins/skills/design-spec/references/design-md-first.md`; `cas deliverable brand` (`cas-cli/src/deliverable/`) resolves it, and a customer deliverable with no `DESIGN.md` is refused rather than given the default brand.
- Before/after evidence: `cas deliverable capture` captures masked merge-base/head shots or passing journey goals in one bounded run; `cas deliverable pairs` selects complete verified pairs by task or commit provenance. Project defaults live under `[qa.capture]`. New pair manifests use schema 2 and seal both PNG and JPG files. Close also accepts historical schema 1 PNG-sealed evidence without rewriting it or requesting new BEFORE captures; every supplied seal is verified, and unsupported versions require a compatible reader or an explicit migration.
- Session wrap-up: `cas-cli/src/builtins/skills/finish/SKILL.md` inventories and stops the session's background tasks and registered servers, verifying none remain before handing back.
- Test value audit: `cas-cli/src/builtins/skills/cas-test-audit/` is a read-only agent procedure (buckets, detector recipes, CI-timing recipes, ledger format); cuts need operator approval and each names a covering test.
- Release history: `CHANGELOG.md`; unreleased per-delivery entries live in `changelog.d/<task-id>.md` fragments, which the close gate credits and the release cut folds into the version section. A returned task retains its own fragment's delivery provenance when a merge resolution revises the entry; its current tip must still contain a valid titled entry.



The Focus epic picker lists titled live epics owned by the selected supervisor or its verification identity, excludes the current focus, and puts the release train first, then priority. Operation feedback and Undo use titles; desktop choices scroll within a short menu. Worker cards in Tasks & progress keep the worker name visible after its avatar on desktop and phone, independently of Team. Commander's Tasks & progress pane lists the selected project's live epics after the release header, joined across paired machines. Each row and the release header show the factory TUI's open / in progress / merged split with a segmented bar and live worker dots; blocked epics lead, and a row expands in place to its tasks. Other projects' epics and untitled epics never appear, and missing counts stay unreported. Commander’s lazy Team tab sits beside a supervisor conversation. It uses the same scoped staffing and epic progress model as Tasks & progress: task-led worker lanes, merge metrics, and an operator action queue sourced only from unresolved conversation requests. Request IDs and optional task IDs survive live delivery and history replay; the ask pinned above the composer is not duplicated. Agent merge, verification, sync and blocked states remain staffing details. Closed or cancelled epics and trains contribute no active navigation or quiet count; planned and paused work remains visible. Stale peer rows are labelled last known, including a reported reconciliation identity that differs from the selected conversation. A missing release identity remains unknown. Blocked tasks and stalled workers lead ahead of merge waits, while equal-priority workers keep roster order. The daemon reports harness/model, task start and durable lifecycle receipts; missing evidence stays unreported. Existing hub events refresh Team without an extra polling loop, while the browser updates elapsed text locally. The view includes keyboard tabs, phone stacking, forced colors and reduced motion, with no terminal or raw work output.

The conversation's phase line uses the selected supervisor's recorded lifecycle start/end and the daemon's observation time. A local clock displays elapsed duration; waiting, completed, stale and disconnected states stay still. Missing phases or start times stay unreported. The ordinary status stream supplies these safe facts while Show work remains off; it exposes no tool input, output or reasoning text. A peer's retained events and catalog refresh leave unchanged conversations' controls alone; their own events and health heartbeats keep status fresh.

Accounting entries use the report skill's saved-chart/source validator before rendering balanced journal grids; the simple/compound before-after example keeps account identities, numeric cells, memos and revision changes separate.

Factory-lane own-test builds run on the trusted local `cas-lane-light` runners. Their existing evidence upload includes `target/ci-impact/own-build.log`, which captures compiler output, test execution and watchdog resource heartbeats without masking command failures. Ring shards and browser jobs keep their existing runners and stage budgets.

Self-hosted Rust setup exposes the runner home's Cargo binaries, bootstraps the repository-pinned toolchain when missing, and serializes toolchain installation across slots sharing that home. It leaves `RUSTC_WRAPPER` empty and skips sccache; hosted setup retains its existing cache behavior.

The lane selector includes the existing gate-registry audits in the own-test phase for production Rust changes. This catches uncoded refusals before later ring shards or the final train suite. Test-fixture-only changes avoid an additional library-test build; uncertain input still widens selection through the existing planner.

Automatic pre-merge review keeps small review contexts inline. When serialized context exceeds 256 KiB, it writes the complete task, parent, dispatch and review context to a content-addressed JSON file under the dispatch's verification-runs directory and tells the reviewer to read it. This keeps accumulated repair history out of the harness's launch request without discarding evidence or changing verification authority.

The independent historical reviewer-eval script checks Linux bubblewrap namespace support before invoking Codex. Its explicit --source-input-only mode supplies pinned base/head source and diff on stdin, keeps the read-only sandbox, disables shell/integration tools, and rejects reports accompanied by tool events. It remains a simulated evaluation without live verification authority.

<!-- cas:project-overview
# Product paths the G6 park gate and overview freshness watch. Keep when regenerating.
domain_paths = ["crates/cas-types/src/task.rs", "crates/cas-types/src/agent.rs", "crates/cas-types/src/entry.rs"]
watch_dirs = ["cas-cli/src/cli", "cas-cli/src/mcp/tools", "cas-cli/src/builtins/skills", "crates/cas-types/src", "hub-web/src"]
-->

Measured estimates reuse the friction ledger: `cas ledger estimate` and MCP `system action=estimate` return typical (p50) and bad-day (p90) minutes, counts and five references, with explicit uncalibrated results for small samples. Task references rank closed project tasks and retained lease/model/file metadata; pipeline references use GitHub run/job timings. Landed subjects emit linked outcomes; actual ≤ p90 is a hit, early completions are marked separately, and overruns add friction. Ledger reports show hit rate, misses and release history. The new Cloud-kind lane holds rejected rows for replay while ordinary pushes and pulls continue. See [estimate semantics and post-install fleet backfill](ESTIMATES.md).
