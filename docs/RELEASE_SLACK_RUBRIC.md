# Release Slack Rubric

**This is a hard rule. Runtime releases and harness-diary updates each have a
mandatory #cas-internal publication workflow. They are separate duties.**

> This file defines **what** to post. For transport preflight, the canonical
> supervisor-owned route, and the designed handoff used when a worker has no
> Slack connection of its own, see
> [docs/SLACK_POSTING_RUNBOOK.md](SLACK_POSTING_RUNBOOK.md).
>
> The mechanical source-branch release train that precedes this rubric is
> [cas-cut-release](../cas-cli/src/builtins/skills/cas-cut-release/SKILL.md);
> use it for the gate, merge queue, published receipt, and host verification.

## Transport ownership and worker handoff

**Use only the Violet hub/bot.** Read through `violet.violet_read` and post
through `violet.violet_post`. Never write through Claude.ai Slack, the Claude
Slack connector, the Codex Slack app, or a personal connector, including during
a hub outage. The builtin
[violet](../cas-cli/src/builtins/skills/violet/SKILL.md) owns channel
resolution, authenticated `tools/list`, bounded `violet_read` dedupe, ordered
thread posting with one-second pacing, `## POSTED` receipts and env-only
credential rules. Its read-only outage procedure stays on the same hub and
requires local duplicate proof before any write.

Any connected harness posts as the same bot. A worker without hub access saves
the exact draft and hands its path, target channel, deploy target and receipt
request to the supervisor, who must also use Violet. If the hub cannot
complete publication, preserve the draft and partial receipts and report the
measured failure. Never mark `POSTED` without returned message IDs and permalinks.

## Runtime releases: two top-level posts

After a release is pushed + tagged, post to **#cas-internal** (`C0B44GUKDK2`). Always **two distinct top-level posts** (not threaded replies) — one per audience:

1. **User-perspective post**
2. **Dev-perspective post**

### Protected-main landing publishes the release

`main` is protected by required checks and the merge queue. The release rides
in the epic PR (cas-f95b): run `scripts/release.sh` on the epic branch (a
hotfix branch from `main` uses `scripts/release.sh --hotfix`). It commits the
version bump, the CHANGELOG section and this release-note draft to the branch,
opens the PR to `main` and enables auto-merge, so one tree passes one queue
cycle. Never push the bump to `main` directly, never push a release tag by
hand, and never open a separate release PR.

When the version change lands, `release.yml` adopts the archives the queue
run built, audits and launch-tests the packaged assets, creates the annotated
tag and the GitHub release, and dispatches the install-path proof. The
supervisor host installs within a minute (`scripts/install-on-publish.sh`),
and the release notes end with the timer from PR opened to installed. Only
then do the receipt and announcement steps below start.

### Published assets before announcement

The release object becoming visible is not publication proof: GitHub can expose
it while one asset is still uploading. Do not upload a local `dist/local-audit/`
archive or copy a digest from it into an announcement. A local audit says that
the tagged source compiled; it says nothing about the bytes users download.

Start every runtime draft from
[`docs/release-notes/runtime-release-template.md`](release-notes/runtime-release-template.md).
After the workflow finishes, fill that draft's checksum placeholders from the
published release only:

```bash
cp docs/release-notes/runtime-release-template.md docs/release-notes/YYYY-MM-DD-vX.Y.Z-slack.md
./scripts/release-published-receipt.sh vX.Y.Z --write-draft docs/release-notes/YYYY-MM-DD-vX.Y.Z-slack.md
```

It fails closed until the release is published, both required assets
(`cas-x86_64-unknown-linux-gnu.tar.gz` and
`cas-aarch64-apple-darwin.tar.gz`) exist, and fresh local downloads match
GitHub's SHA-256 metadata. `--write-draft` requires each digest placeholder
and replaces every occurrence, so it fails rather than leaving a human to
transcribe a local build's values.
The workflow publishes macOS ARM64 from its macOS runner regardless of the host
that tagged the release; never describe the release as Linux-only because a
local audit host cannot build Darwin.

### Published report before announcement

A publication latency overrun records `WITHIN_BUDGET=false` and warns; it must
not block announcement, report, receipts or host update for an already-live
release. Missing or incoherent timing evidence remains a blocker. The Dev
reply's final publication timing trailer and the report's Tag to published row
must state the measured time and whether it exceeded the recorded budget.

1. Before announcements, require the published-asset and latency receipts and
   installation proof. Run `cas release report <version> --pdf` and require the
   Markdown, brief, standalone HTML, PDF and QA evidence under
   `docs/release-reports/`. Verify source fidelity and PDF pagination before
   either top-level message is sent. These local artifacts precede the later
   upload/link receipt, which needs existing parent messages.
2. Post the four messages through Violet in order: the reviewed User top-level,
   User logical reply, Dev top-level, Dev logical reply. A logical reply is one
   body submitted once; Violet may split it into multiple Slack messages under
   the same parent. Save every returned part ID in order, the singular message
   ID and returned permalink, including both parent IDs for report delivery.
3. After the four messages (including all returned parts), set `CAS_RELEASE_TRAIN_REPORT_USER_THREAD_TS` and
   `CAS_RELEASE_TRAIN_REPORT_DEV_THREAD_TS` to those returned parent IDs and run
   `scripts/release-train.sh <version> <epic-worktree> --report`. Attach the PDF
   to the existing User thread through Violet `violet_post` with `kind: file`
   and link the HTML from the existing Dev thread. Require uploaded-PDF
   download, decode, page-count and source-hash checks before accepting delivery.
   Preserve the returned file permalink, User/Dev thread receipts, local PDF
   SHA-256/size, verified remote PDF SHA-256/size/page count, the HTML SHA-256,
   both file IDs, the PDF upload's message ID and message permalink, and the
   local page count in `release-report.receipt` in the run
   directory. The adapter must download the returned explicit `download_url`, or
   re-read the uploaded file by its returned ID through authenticated `violet_read`
   when the current hub receipt omits a download URL, and compare the bytes and
   decoded page count before writing that receipt. Bound that fallback read
   inclusively from the User root Slack timestamp as RFC3339 `since`, preserving
   fractional precision so old channel history is excluded while its replies are
   included. A message permalink is not a PDF endpoint; if neither verified path
   is available, fail explicitly. The
   adapter sends hub credentials only to the configured MCP origin (with the
   explicit same-origin loopback exception used by tests), never to external
   signed/private-provider URLs; it rejects plaintext endpoints outside that test
   exception and cross-origin or scheme-changing redirects before following them.
   The train also
   re-hashes the local PDF and checks every remote evidence field agrees with
   the local artifact; local checks alone do not prove uploaded-file integrity.
   Preserve partial receipts and stop on failure; never retry an uncertain write.
   Publication completion requires all message, installation, published-asset
   and verified report receipts. A missing receipt keeps completion pending.

If the release workflow fails after the version lands, fix the cause on
`main` and run `gh workflow run release.yml -f dry_run=false`; it refuses to
replace an existing release. There is no local publisher.

### Recovering a failed or partial release

1. Inspect, then run the receipt command. A complete release has both named
   assets and succeeds; a partial release is one that exists but makes the
   receipt command fail.

   ```bash
   gh release view vX.Y.Z --repo Richards-LLC/cassy --json isDraft,publishedAt,assets
   ./scripts/release-published-receipt.sh vX.Y.Z
   ```

2. If the receipt succeeds, do not rerun or change the release: fill the draft
   with `--write-draft` and continue to the announcement. Do **not** attach a
   missing asset by hand; the workflow must remain the normal single publisher.
3. If the release is partial and nothing has been announced from it, preserve
   the existing tag, delete only the incomplete release, then rerun the same
   failed workflow run. The release workflow will create fresh CI assets; it
   refuses an existing object precisely to prevent replacement-by-rerun.

   ```bash
   gh release delete vX.Y.Z --repo Richards-LLC/cassy --yes
   gh run rerun <failed-run-id> --repo Richards-LLC/cassy
   ```

   Do not use `--cleanup-tag`, force-push, or retag: the annotated tag remains
   the source identity for both the retry and its receipt.
4. If CI cannot publish after that recovery, stop and escalate: there is no
   local publisher (cas-f95b), and
   [the Actions outage runbook](release-actions-outage-runbook.md) covers a
   GitHub outage. In every case, run
   `release-published-receipt.sh --write-draft` after publication and before an
   announcement.
5. If any announcement might already name the release, do not delete or replace
   its assets. Stop and escalate to the release owner; publish a corrective new
   version instead of making existing verification evidence change underneath
   users.

If `factory action=worktree_merge ... allow_trunk=true` encounters the
ruleset first, it returns `PROTECTED_DEFAULT_BRANCH_REQUIRES_PR` with the
source/target-specific form of these commands. Follow that handoff and retry
the merge action after fetching the PR landing so Cassy can reconcile delivery.

### Shape (identical for both posts)

1. **Open with the punch** — one or two lines of plain, punchy language framed as **how it was → how it is now**. Lead with the change in experience, not the mechanism.
2. **Details below** — bullets fleshing out the punch, written from that post's perspective.

### Runtime voice rules

- **User post: ALWAYS plain language.** Describe what the user feels/sees. No jargon dumps.
- **Dev post: may be more technical** — code/behavior level is fine.
- **BOTH posts:**
  - **No Cassy-internal agent actions.** Do not narrate supervisor/worker/factory/director orchestration, task lifecycle bookkeeping, who-closed-what, epics, etc.
  - **No ticket numbers.** No `cas-xxxx`, no epic IDs. Describe the change, not the tracking artifact.
  - Lead with the before→after punch; keep it tight.
  - The release train adds the Dev reply's measured `Publication timing` bullet
    from `release-latency.receipt` after publication. Use that receipt's measured
    seconds and budget result; omit the obsolete intervention and green-to-pipeline
    trailer, whose values the train does not collect.

### User thread first: plain language

Write the User top-level and reply first as a product changelog for someone who
has never seen the codebase. Describe what the person sees and what changed for
them. On the User side, forbid function, struct, and file names; flags the user
does not type; table or column names; and the words `watermark`, `metadata`,
`ingest`, `upsert`, `envelope`, `scope stamp`, `canonical id`, `team pull`,
`purge-foreign`, `cloud identity`, `registration`, `registrations`, `harness`,
`agent`, `factory`, `worker`, `supervisor`, `epic`, and `lane`. Apply a
read-aloud test: if a bullet needs the codebase to make sense, rewrite it.
Attach before/after screenshots to the User reply under the
[release-notes rubric](release-notes/RUBRIC.md)'s before/after rule, with
`--reply-file` after that reply.

### Formatting (Slack mrkdwn, glance test)

Slack receives `text` verbatim. Every runtime and diary post or reply MUST use
Slack mrkdwn only:

- Use `*bold*`, `_italic_`, backtick code, and `•` bullets on their own lines.
- Put a blank line between bullet groups. Do not use `**`, lines beginning with
  `#`, markdown tables, or `[label](url)` links; use bare URLs or `<url|label>`.
- A top-level message is exactly two lines. Line 1 is
  `*Live on production — User — Cassy vX.Y.Z*` (or `Staging` / `Dev`). Line 2
  is a Was → Now punch of 25 words or fewer.
- No line may run over 300 characters, counting a `<url|label>` link as its
  label: the Violet hub refuses the whole post as `long_line`, and
  announcement lint refuses it first.
- A reply has one bullet per shipped change, with no item cap. Each bullet
  starts `• *Short label* —`, contains Was → Now, stays within two lines, and
  is separated from the next bullet by a blank line. A bullet too long for one
  line starts `→ Now:` on its second line. Never chain items with
  semicolons. Put the install or validation trailer last, after a blank line,
  as plain lines exempt from Was → Now, with digests in backticks. If a reply
  grows past roughly 12 bullets, group them under bold sub-headings on their
  own lines instead of dropping changes.
- The glance test passes when every item's bold label is findable within five
  seconds. Put every body exactly as posted inside fenced blocks in the draft;
  the fenced text is the reviewed text.
- Resolve every `{{...}}` token in those four bodies. Announcement lint rejects
  remaining tokens, naming the body and token, at preflight and before posting.
  To correct a Violet-authored message in place, use `violet_post` with
  `kind: "edit"`, its `violet_read` `message_id`, and the complete corrected
  `text`. `kind: "message"` creates a new message and does not accept `message_id`.

## Harness-diary updates: one parent + three replies

After any update to the Claude, Codex, or Grok changelog diary merges to `main`,
publish one thread in **#cas-internal** (`C0B44GUKDK2`):

1. **One top-level parent** — summarize the cross-harness sweep and lead with why
   the changes matter to Cassy users and maintainers.
2. **Exactly three logical threaded replies**, in this order (Violet may split
   each body into several messages under the same parent):
   1. **Grok**
   2. **Claude**
   3. **Codex**

The parent and replies must use impact-first prose. Each harness reply names the
version or version range reviewed, the notable Cassy touchpoints, the resulting Cassy
verdict/action, and any source gaps (write `none` when there are none). Report what
changed and what Cassy users should expect; do not narrate how the diary work was
assigned or executed.

This is one shared three-harness thread even when only one diary changed. Use the
other two replies to state their current reviewed ranges and verdicts so the thread
always presents one complete harness snapshot.

### When runtime and diary changes overlap

- A merge containing both a runtime release and a harness-diary update requires
  **both workflows**: the two runtime-release top-level posts and the separate diary
  parent with exactly three logical replies.
- A diary-only merge uses only the diary thread. Do **not** fabricate a release,
  tag, shipped runtime behavior, or user/dev release announcement.

## Rules for every post and reply

- Lead with impact, not mechanics or bookkeeping.
- Include version ranges, Cassy verdict/action, and source gaps in the diary thread.
- Use **zero ticket/task/epic IDs** (including `cas-xxxx`).
- Use **zero internal agent, worker, supervisor, director, or factory narration**.

## Posting receipt

Immediately after posting, before ending the task or turn, annotate the saved
release-note draft with a `## POSTED` block containing the Eastern 12-hour posting time (for example `9:45 AM ET`),
channel, and the returned message ID and permalink for every post and logical
reply. For each split reply, include every `message.part_ids` entry in returned
order; keep the singular `message_id` and returned permalink alongside that
list. A single ID is not a complete split-reply receipt, and a part ID does not
license inventing a permalink.

`announce.receipt` retains `*_ID` and `*_PERMALINK` and adds JSON arrays
`USER_TOP_LEVEL_PART_IDS`, `USER_REPLY_PART_IDS`, `DEV_TOP_LEVEL_PART_IDS` and
`DEV_REPLY_PART_IDS`. An unsplit message has a one-element list containing its
returned ID. Each generated `## POSTED` row includes `message_id=…`,
`part_ids=[…]` and the returned permalink. The same ordered-list requirement
applies to diary replies recorded manually.

`release-report.receipt` must additionally retain `PDF_MESSAGE_ID` and
`PDF_MESSAGE_PERMALINK`, separate from `PDF_FILE_ID` and `PDF_FILE_PERMALINK`.
The report is incomplete without its upload message identity. The receipts
stage refreshes `## POSTED` from the saved receipts, adding the PDF upload's
message ID, message permalink and file ID after report delivery. Report uploads
and links are separate from the four announcement bodies.

Preserve incomplete receipts and reconcile missing IDs through a bounded
Violet read; do not repeat publication to repair evidence. These rules make
both runtime and diary announcements searchable and verifiable.

## Why

These messages are for a product/stakeholder audience. They communicate *impact*,
not internal process. Runtime releases split the plain-language user story from the
technical dev story; diary threads give both audiences one traceable cross-harness
compatibility snapshot. Neither includes factory plumbing or ticket IDs.

## Checklist

### Runtime release

- [ ] Version/changelog commit landed on `main` through a reviewed PR with required checks green
- [ ] PR URL + required-check status surfaced before the explicit merge (no `--auto` / admin bypass)
- [ ] After every release-train version bump, regenerate `Cargo.lock` and verify `cargo metadata --locked` before tagging.
- [ ] Release tag points at the fetched `origin/main` landing and is pushed
- [ ] Workflow-created release has both Linux x86_64 and macOS ARM64 assets;
  `release-published-receipt.sh --write-draft` succeeded from fresh downloads
  before any digest enters the draft (a local audit archive is not
  shipped-byte evidence)
- [ ] The advisory `Install path proof` workflow is green for the release on
  hosted macOS Apple Silicon and a clean Linux container; retain the run URL
  and both transcript artifacts before claiming that installation works.
- [ ] Hosted install proof is supplemented by the manual consumer-Mac
  Gatekeeper checklist in [the install-proof guide](ci/install-path-proof.md);
  its GUI/SIP limits are not silently presented as covered by CI.
- [ ] `cas release report <version> --pdf` produced the Markdown, brief, HTML,
  PDF and QA receipt under `docs/release-reports/`; A4/Letter pagination and
  source fidelity passed.
- [ ] The PDF is attached to the User thread, the HTML is linked from the Dev
  thread, and `release-report.receipt` records both paths, local PDF
  SHA-256/size, verified remote PDF SHA-256/size/page count, HTML SHA-256, both
  file ids, PDF upload message ID/permalink, the User/Dev thread timestamps,
  file permalink and local PDF page count.
- [ ] Post 1 (user): punch (was→now) + plain-language details
- [ ] Post 2 (dev): punch (was→now) + technical details
- [ ] Both: zero ticket numbers, zero internal-agent narration
- [ ] Draft has a `## POSTED` receipt with Eastern 12-hour posting time,
  channel, every returned permalink and all split part IDs

### Harness-diary update

- [ ] One top-level parent in `C0B44GUKDK2` explains the cross-harness impact
- [ ] Exactly three logical replies, ordered Grok → Claude → Codex; all split part IDs recorded
- [ ] Each reply includes version range, Cassy touchpoints, verdict/action, and source gaps
- [ ] Parent and replies contain zero ticket IDs and zero factory narration
- [ ] Draft has a `## POSTED` receipt with Eastern 12-hour posting time,
  channel, every returned permalink and all split part IDs
- [ ] If runtime code also shipped, complete the runtime-release checklist too
- [ ] If the merge is diary-only, make no runtime-release claim
