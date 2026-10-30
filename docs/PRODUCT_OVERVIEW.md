# Cassy - Product Overview
>
> Auto-generated domain snapshot. Regenerate with /project-overview when the domain model drifts.

## Pitch

Cassy is a local-first multi-agent coding factory and persistent context system for developers working in Git repositories.
The `cas` binary runs a supervisor with isolated workers; `cas serve` gives coding agents durable project context through MCP.
Tasks, memories, rules, skills and repository knowledge live in one local store; Cloud sync, the machine hub and the Commander browser app are optional.
Every delivery is checked before it merges: tests, the journeys a change touches, documentation upkeep and, after release, a live post-release check.

## Personas

- **Operator** - the repository maintainer: starts factory sessions, picks the work, reviews delivery evidence, approves releases and can answer from Commander.
- **Supervisor agent** - plans epics, assigns one task at a time to workers, merges pushed deliveries once CI is green, and coordinates with supervisors on other projects or machines.
- **Worker agent** - executes one assigned task in its own worktree, runs the targeted tests, and pushes a committed result for review.
- **QA reviewer** - walks the journeys a delivery touches on the real build and records an approve or reject verdict in a QA ledger.

## Core Concepts

- **Provider account** - a Claude or Codex subscription identified by provider account and organization/workspace IDs, displayed by login email; `cas accounts list` shows usage, absolute resets and duplicate or login findings while legacy folder names remain launch aliases.
- **Factory session** - a live supervisor and its worker fleet, coordinated through the terminal UI.
- **Epic / Task** - an epic is a larger outcome built from tasks; a task is a prioritized work item with a lease, notes, an owner and close requirements.
- **Epic split** - two supervisors divide one epic's tasks; each side's claim is refused to the other until the split is accepted, cancelled or expires.
- **Worktree** - a separate Git checkout and branch that isolates a worker's changes; under disk pressure, build caches left by removed checkouts are reclaimed oldest first.
- **Delivery branch** - the pushed `factory/<name>` branch a task is proven and merged from; a supervisor `rebind_branch` approves a replacement branch after an audit of the old tip.
- **Close gate** - the evidence, test-receipt and merge checks that decide whether a delivery may close; each merge records its CI admission as green, advisory or override. A supervisor can cite `merge_group_run=<id>` in an audited close override to reuse successful CI for an exact landed tree containing the delivery, avoiding a duplicate scoped suite. Missing or mismatched queue evidence still requires scoped proof.
- **Merge on green** - `READY TO MERGE` requires an owned, pushed delivery with green CI and worker completion: either the task is awaiting merge or the worker declared that exact tip ready. A green checkpoint alone never signals completion, and an explicit merge of an in-progress task returns an unparked-task warning. The merge requests verification at the merge commit, so approval closes the task in one hop and rejection reopens it to the same worker. A reviewed correction must itself land before close, and its delivery follows the worker's pushed fix branch. Duplicate CI checks at the same SHA use the newest completed conclusion. An override never closes automatically.
- **EPIC READY** - a deduplicated action sent to the epic's live owner once every child task, including those of an accepted split, is closed.
- **Journey registry** - a project's `docs/qa/journeys.yaml` (drafted by `cas qa journeys --seed`): each user journey's owner files, specs, surfaces and live, dark or dead status. Cassy's Commander registry preserves import and CSS selector coverage, with enforcement declared in the tracked registry.
- **Journey impact** - which journeys a diff or delivery touches, the evidence each needs, and the files no journey owns; journeys rank by traffic share.
- **Journey traffic** - Commander counts each journey's defining act locally and reports the per-day totals only to the operator's paired machines; `cas qa journeys --traffic` turns them into per-journey shares.
- **Upkeep gates (G1-G9)** - park refuses until the journey registry, regression case, CODEMAP and this overview are updated or a reasoned decision is recorded; release notes wait for the post-release check.
- **Rapid response mode** - the operator opens a time-boxed window (at most 12 hours) for one epic with `cas qa rapid-response`; while it is open, per-task independent QA, unsampled verification, untouched-route journeys and the batch receipt wait are skipped, while the post-release check stays a full gate. Build and touched-test proofs still refuse; a change with a secret-shaped line or an unreadable diff keeps every gate; migrations keep every gate and need the operator's review; and each skipped check becomes a back-fill task when the window closes.
- **Acknowledged delivery** - a message counts as delivered only when the recipient's session receives it; idle recipients get bounded wakes and the sender is told after five minutes.
- **Peer messaging** - supervisors reach each other by name, project, `project@machine` or `supervisor:<project>`; a message to an ended session goes to its live successor.
- **Worker isolation** - on Linux, workers cannot read operator credential stores such as Neon and Vercel, and see only the connectors an operator grants a dedicated worker account.
- **Memory, Rule, Skill** - durable facts, versioned instructions that sync into agent harnesses, and procedural playbooks agents load for a kind of work.
- **DESIGN.md first** - every report, page, chart, note, UI, terminal output, drawing or image an agent produces wears the target project's `DESIGN.md`; a project without one gets one through `design-spec`, and the task records which file and revision it used.
- **Knowledge page** - a distilled, source-linked page about the current repository; the code index connects symbols to that context.

## Primary Journeys

Commander labels each machine's reported Cassy version in its project chip and paired-machine card. A newer published stable release adds a quiet update note; an hourly cached lookup retains its last known release while offline. The Web footer separately identifies the client release and build.

The operator can paste text, screenshots or files into Commander: long text becomes a previewed text attachment, and Send stores files on the session's machine and gives the supervisor readable local paths through the existing durable message lane. Pending files remain through reconnect, with a reminder to send before closing the page. The operator's own sent files appear in their bubble as image previews and named file chips after reload and on other paired devices, without displaying host paths.

1. The operator launches `cas`, reviews the active epic and staffs workers for its ready tasks.
2. A worker starts its lease, commits in its worktree, runs targeted tests and pushes; the supervisor merges on green, and the verifier's verdict at the merge commit closes or reopens the task.
3. A QA reviewer runs `cas qa impact` on a delivery, scaffolds its evidence with `cas qa ledger init --task <id>`, walks the touched journeys, and runs `cas qa ledger check <path> --task <id>` before submitting the verdict. The same format check refuses malformed evidence at close before verification dispatch.
4. A supervisor messages a supervisor on another project or machine, sees RECEIVED or PENDING, or splits an epic with that peer.
5. After a release merges, `cas serve` runs the post-release check, `cas release report` builds the branded release report, and Violet posts the release notes to Slack; `cas violet channels` and `cas doctor` check the configured channels. After publication, the release workflow also updates `hub.petrastella.io` from the released Commander bundle when its files change, verifies public access and bytes, restores the previous deployment on a failed check, and attaches deployment provenance to the release.
6. An operator prepares a client-branded SOW delivery report with before/after evidence for each requested change; `cas deliverable finalize --audience customer` brands it, embeds its screenshots, refuses internal vocabulary and prints and checks the A4 and Letter PDFs. For a client product, `cas-customer-release-notes` writes the period's customer release notes from its `docs/deliverables/PROFILE.md`; the client's `DESIGN.md` shapes the layout, and the same `finalize --audience customer` call produces the HTML and PDFs.
7. An agent calls `cas serve` tools to find tasks, memories, rules, skills and knowledge while it works.
8. The Commander shell Accounts icon lists accounts by machine and provider, with login state, capacity and local 12-hour reset times. Machine-read pairings can inspect; factory-manage pairings can confirm removal/disable, enable, rename or set the default through audited operations. Grok is read-only. Account changes refresh New session without a page reload. The operator also runs `cas accounts list` (or `--json`) to compare Claude and Codex subscriptions by email, usage and reset time, and identify duplicate or dead folders before choosing a session account. Claude/Codex pickers and Commander show local reset dates/times with relative waits for 5h and weekly usage and highlight a recommended subscription with reset/headroom reasons; operators can choose another account. Workers use the recommendation over an inherited requester account; explicit accounts win, and the requester remains the fallback when no account is eligible. A worker usage limit leaves it waiting on its current account and sends one supervisor notice per episode naming the account, held tasks and absolute local reset date/time (or explicitly unavailable). No replacement is queued; pre-upgrade queued switches are refused. Episode records and notice dedupe survive daemon restarts. Every Claude/Codex launch, worker spawn and Commander session first links the chosen account's missing shared items (skills, agents, hooks, settings) to the provider base. It names a retired folder that came back. `cas doctor --fix` repairs link drift, and `--yes` backs up and replaces real items.
9. The operator invokes the builtin `finish` skill (`/finish` in Claude Code) to clean up session work and save a handoff and learnings; updates distribute it across harnesses while preserving personal overrides. From Commander, `/` in the composer or the header command button offers curated Claude/Codex controls under a paired input lease (HUB-J27). Clear, Exit and Finish confirm; busy panes refuse; receipts distinguish submission, native observations and uncertain outcomes. Finish names a completed harness request, while Exit follows the authenticated session-ended receipt.
10. In Commander the operator turns on Show work to see the supervisor's thinking, tool calls and task moves between its replies, collapsed until opened, and opens a worker's live timeline from Tasks & progress instead of watching its terminal (HUB-J25). Typed tool rows name reads, searches, commands and edits with their reported scope; opening one shows its targets or command metadata alongside the exact tool record. Failures are named, and unfamiliar tools retain their raw text. The daemon reads these from existing transcripts and the task DB with zero model tokens, and pushes a contentless change notice to clients that read a page. Tasks & progress leads with focused-release merged/total, merges per hour and last merge, and groups tasks from the supervisor's open epics as Needs you, In progress and Ready with All tasks available. Its desktop width is adjustable by pointer or keyboard and remembered per browser; permission guidance replaces unavailable header controls.

11. In Commander the operator reads safe Mermaid diagrams, expands unified diffs, and inspects inline image, video, audio and PDF first-page attachments. Fenced source has a language or filename header, Copy with visible success or failure, and a per-block Wrap toggle for phone reading. Source and original files remain reachable; previews reserve their space and rich renderers load on demand.

12. A supervisor attaches an HTML page, such as a report with diagrams, images or video, to a reply, and Commander shows it inline beneath the file card, in the conversation's light or dark theme, and opens it in its own tab (HUB-J4). The publishing machine serves the page through a short-lived ticket under a CSP sandbox: no script, an opaque origin, no top navigation or popups, no network beyond its own assets, and framing allowed only from the Commander origin that asked. Agent markup never enters Commander's document.

## Authoritative Sources

- Product pitch and user-facing surfaces: `README.md`.
- Coordination and context-system boundaries: `cas-cli/docs/ARCHITECTURE.md`.
- Task lifecycle and close states: `crates/cas-types/src/task.rs`.
- Agent roles and session responsibilities: `crates/cas-types/src/agent.rs`.
- Memory, rule and skill records: `crates/cas-types/src/entry.rs`, `crates/cas-types/src/rule.rs`, `crates/cas-types/src/skill.rs`.
- Adaptive QA: `cas-cli/src/qa_registry.rs`, `cas-cli/src/qa_impact.rs`, `cas-cli/src/qa_ledger.rs`, `cas-cli/src/qa_post_release.rs`, `docs/qa/impact.md`, `docs/qa/docs-upkeep.md`.
- CLI and MCP surfaces: `cas-cli/src/cli/` and `cas-cli/src/mcp/tools/`.
- Commander browser app: `hub-web/`.
- Release and delivery reports: `cas-cli/src/builtins/skills/cas-release-report/`, `cas-cli/src/builtins/skills/cas-sow-delivery-report/`, `cas-cli/src/builtins/skills/cas-feature-report/`, `cas-cli/src/builtins/skills/cas-customer-release-notes/`; all finish through `cas deliverable finalize` (`cas-cli/src/deliverable/finalize.rs`), the one path for image inlining, audience hygiene, printing and the PDF check (`cas-cli/src/deliverable/assets/`).
- Branding of human-facing output: `cas-cli/src/builtins/skills/design-spec/references/design-md-first.md`; `cas deliverable brand` (`cas-cli/src/deliverable/`) resolves it, and a customer deliverable with no `DESIGN.md` is refused rather than given the default brand.
- Before/after evidence: `cas deliverable capture` captures masked merge-base/head shots or passing journey goals in one bounded run; `cas deliverable pairs` selects complete verified pairs by task or commit provenance. Project defaults live under `[qa.capture]`.
- Session wrap-up: `cas-cli/src/builtins/skills/finish/SKILL.md`.
- Release history: `CHANGELOG.md`.

<!-- cas:project-overview
# Product paths the G6 park gate and overview freshness watch. Keep when regenerating.
domain_paths = ["crates/cas-types/src/task.rs", "crates/cas-types/src/agent.rs"]
watch_dirs = ["cas-cli/src/cli", "cas-cli/src/mcp/tools", "cas-cli/src/builtins/skills", "crates/cas-types/src", "hub-web/src"]
-->

Commander’s lazy Team tab sits beside a supervisor conversation. It uses the same scoped staffing and epic progress model as Tasks & progress: worker/task lanes, merge metrics, and a Needs you strip. The daemon reports harness/model, task start and durable lifecycle receipts; missing evidence stays unreported. Existing hub events refresh Team without an extra polling loop, while the browser updates elapsed text locally. The view includes keyboard tabs, phone stacking, forced colors and reduced motion, with no terminal or raw work output.

The conversation's phase line uses the selected supervisor's recorded lifecycle start/end and the daemon's observation time. A local clock displays elapsed duration; waiting, completed, stale and disconnected states stay still. Missing phases or start times stay unreported. The ordinary status stream supplies these safe facts while Show work remains off; it exposes no tool input, output or reasoning text.
