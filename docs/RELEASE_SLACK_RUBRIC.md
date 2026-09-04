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

**The default transport is the MechaCassy hub**, and the procedure for using it
is the builtin [mecha-cassy](../cas-cli/src/builtins/skills/mecha-cassy/SKILL.md)
skill: channel resolution, the two-check preflight, ordered thread posting with
one-second pacing, the `## POSTED` receipt, and the env-only credential rules.
The hub holds the Slack bot credential server-side, so any harness on any
account posts as the same bot — a Codex or Grok worker posts directly instead of
handing its draft back.

The fallback is the approved `pippenz@gmail.com` Claude profile's
`claude.ai Slack` MCP, owned by the supervisor or an explicitly approved Claude
posting owner. Use it when the hub is unavailable; it cannot upload files and is
bound to that one profile. Either way a `Connected` server listing is not a
receipt: the read-only preflight must succeed before a post is attempted.

When a worker genuinely has neither transport, it saves the exact draft and
hands the draft path, target channel, deploy target, and requested receipt back
to the supervisor, who posts and returns timestamps/permalinks for the worker to
record. This is the designed path, not a failed fallback.

If no approved transport passes preflight, leave the draft saved and report the
duty blocked with the measured error. Do not post from an unapproved profile or
mark the draft `POSTED` without returned timestamps and permalinks.

## Runtime releases: two top-level posts

After a release is pushed + tagged, post to **#cas-internal** (`C0B44GUKDK2`). Always **two distinct top-level posts** (not threaded replies) — one per audience:

1. **User-perspective post**
2. **Dev-perspective post**

### Protected-main landing before the tag

`main` is protected by required checks. Prepare the version bump, changelog,
and release-note draft on a source branch; do not merge or push that commit
directly to `main`, and do not create the release tag before the PR lands.

When a release follows an epic, carry the version bump, CHANGELOG section, and release-note draft as the epic branch’s final commit and land them through its single integration PR before tagging for one tree, one queue cycle; reserve a standalone `release/vX-prepare` PR for multi-PR batch releases, because the version lives in the tree and the merge queue revalidates every new tree.

1. Push the source branch: `git push -u origin <source-branch>`.
2. Open the release PR: `PR_URL=$(gh pr create --base main --head <source-branch> --fill)`.
3. Surface its URL and required checks to the operator/supervisor:
   `gh pr view "$PR_URL" --json url,number,statusCheckRollup`.
4. After the required checks are green, merge explicitly:
   `gh pr merge "$PR_URL" --merge`. Do not use `--auto` or an admin bypass.
5. Fetch the landed commit (`git fetch origin main`), tag that exact
   `origin/main` commit, then run `./scripts/release.sh --publish-tag`. The
   explicit flag pushes the tag; the tag-triggered GitHub workflow is the
   normal release publisher. A bare `release.sh` is audit-only and never
   touches the remote.

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

`scripts/release.sh --publish-tag --manual-publish
--acknowledge-workflow-conflict` is an emergency failover for a disabled or
unavailable workflow, not a normal release lane. It deliberately competes
after its tag push, so disable/cancel the CI workflow first when possible; it
uses the same receipt command before any announcement digest is posted.

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
4. If CI cannot publish after that recovery, use the explicitly acknowledged
   `--publish-tag --manual-publish --acknowledge-workflow-conflict` failover
   only after the incomplete release is deleted. In every case, run
   `release-published-receipt.sh --write-draft` after publication and before an
   announcement.
5. If any announcement might already name the release, do not delete or replace
   its assets. Stop and escalate to the release owner; publish a corrective new
   version instead of making existing verification evidence change underneath
   users.

If `coordination action=worktree_merge ... allow_trunk=true` encounters the
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

### User thread first: plain language

Write the User top-level and reply first as a product changelog for someone who
has never seen the codebase. Describe what the person sees and what changed for
them. On the User side, forbid function, struct, and file names; flags the user
does not type; table or column names; and the words `watermark`, `metadata`,
`ingest`, `upsert`, `envelope`, `scope stamp`, `canonical id`, `team pull`,
`purge-foreign`, `cloud identity`, `registration`, `registrations`, `harness`,
`agent`, `factory`, `worker`, `supervisor`, `epic`, and `lane`. Apply a
read-aloud test: if a bullet needs the codebase to make sense, rewrite it.

### Formatting (Slack mrkdwn, glance test)

Slack receives `text` verbatim. Every runtime and diary post or reply MUST use
Slack mrkdwn only:

- Use `*bold*`, `_italic_`, backtick code, and `•` bullets on their own lines.
- Put a blank line between bullet groups. Do not use `**`, lines beginning with
  `#`, markdown tables, or `[label](url)` links; use bare URLs or `<url|label>`.
- A top-level message is exactly two lines. Line 1 is
  `*Live on production — User — Cassy vX.Y.Z*` (or `Staging` / `Dev`). Line 2
  is a Was → Now punch of 25 words or fewer.
- A reply has one bullet per shipped change, with no item cap. Each bullet
  starts `• *Short label* —`, contains Was → Now, stays within two lines, and
  is separated from the next bullet by a blank line. Never chain items with
  semicolons. Put the install or validation trailer last, after a blank line,
  as plain lines exempt from Was → Now, with digests in backticks. If a reply
  grows past roughly 12 bullets, group them under bold sub-headings on their
  own lines instead of dropping changes.
- The glance test passes when every item's bold label is findable within five
  seconds. Put every body exactly as posted inside fenced blocks in the draft;
  the fenced text is the reviewed text.

## Harness-diary updates: one parent + three replies

After any update to the Claude, Codex, or Grok changelog diary merges to `main`,
publish one thread in **#cas-internal** (`C0B44GUKDK2`):

1. **One top-level parent** — summarize the cross-harness sweep and lead with why
   the changes matter to Cassy users and maintainers.
2. **Exactly three threaded replies**, in this order:
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
  parent with exactly three replies.
- A diary-only merge uses only the diary thread. Do **not** fabricate a release,
  tag, shipped runtime behavior, or user/dev release announcement.

## Rules for every post and reply

- Lead with impact, not mechanics or bookkeeping.
- Include version ranges, Cassy verdict/action, and source gaps in the diary thread.
- Use **zero ticket/task/epic IDs** (including `cas-xxxx`).
- Use **zero internal agent, worker, supervisor, director, or factory narration**.

## Posting receipt

Immediately after posting, before ending the task or turn, annotate the saved
release-note draft with a `## POSTED` block containing the UTC timestamp,
channel, and a permalink for every post and reply. This makes both runtime and
diary announcements searchable and verifiable.

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
- [ ] Post 1 (user): punch (was→now) + plain-language details
- [ ] Post 2 (dev): punch (was→now) + technical details
- [ ] Both: zero ticket numbers, zero internal-agent narration
- [ ] Draft has a `## POSTED` receipt with UTC timestamp, channel, and every permalink

### Harness-diary update

- [ ] One top-level parent in `C0B44GUKDK2` explains the cross-harness impact
- [ ] Exactly three replies, ordered Grok → Claude → Codex
- [ ] Each reply includes version range, Cassy touchpoints, verdict/action, and source gaps
- [ ] Parent and replies contain zero ticket IDs and zero factory narration
- [ ] Draft has a `## POSTED` receipt with UTC timestamp, channel, and every permalink
- [ ] If runtime code also shipped, complete the runtime-release checklist too
- [ ] If the merge is diary-only, make no runtime-release claim
