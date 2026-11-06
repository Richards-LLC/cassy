# Shared walk kit

Use one versioned harness per project, shared by every worker. Copy the shipped
`cas-qa-craft/scripts/walk-kit.mjs` once to the project's tracked
`scripts/qa/walk-kit.mjs`; commit its project adapter beside it. Import the same
copy throughout the release. Every produced ledger row includes `kit:<version>`.
Update that copy deliberately between releases; no separate drift check is needed.

The kit adapts proven project walk primitives without authentication,
SQL, phone or host logic. Project adapters own those boundaries and cleanup.
Keep secrets in memory and use the project's existing fixture tools.

## Launch

Keep `docs/qa/verify.md` as the shared launch, readiness, Doctor, Drive, Evidence
and Cleanup recipe. Name the timeout, base URL, isolated state, binary or server
receipt, and the command that starts it. Register long-lived processes through
`cas-servers`. Never replace the launch recipe with a per-worker runtime.

Map user-facing features in `docs/qa/features/`: a README index and one file per
feature using [feature-template.md](feature-template.md). Each entry path gets
its own row, ordered by real traffic. Check the map with:

```sh
node <skills-dir>/cas-qa-craft/scripts/check-feature-map.mjs --root .
```

`<skills-dir>` is `.claude/skills`, `.codex/skills` or `.grok/skills`.

## Drive

Import these functions from the project's pinned copy:

- `requireBuild({backend: sha, frontend: sha}, readBuild)` reads actual served
  identities at start and end. Missing or moved SHAs hold the walk; no fallback.
- `waitForReply(page, options)` receives pre-send IDs, measured `sentAt` and a
  `readThread(page)` adapter returning `{id,user,working,visible,text}` bubbles.
  It waits past typing, skeletons and hidden replies, then measures
  `firstReplyMs`; record that result with the row. Polling observes state;
  fixed sleeps never substitute for a completed visible reply.
- `frameMessage(page, text, file, adapters)` uses `locateMessage(page,text)`
  and `readVisibleText(page)` to scroll, verify viewport intersection and save
  the frame, its hash and visible text.
- `openDraft(page,url,expected,options)` uses `readDraft(page)` returning
  `{itemId,visible,working,text}`. It opens the delivered URL and waits for real
  content; a PASS requires readiness and no missing expected text.
- `shouldRun(step,open)` permits `ifAsked` answers only for the current question.

Project browser creation stays headless. Adapters enforce the project's target
allowlist, authentication and fixture policy; the kit never starts a browser or
reads credentials itself.

## Evidence

`createLedger({file,task,build,scope,surface})` writes schema 2. Before acting,
call `expect(id,{cell,expected,cite,quote})` with a spec `path:line` and its
quoted words. An uncited expectation becomes a spec question, never FAIL.

Call `record({id,channel,view,sent,verdict,observed,frame,claims,reply,draft,defect})`
with the exact `sent.text` and `sent.url`. Graded rows require readable frames
inside the ledger directory and claims that appear in their visible text.
Database read-back corroborates the visible result. A failed step names its
`cas-*` defect; the kit merges that cause across widths/channels, refuses reused
frames and conflicting verdicts, and atomically rewrites the ledger after each
row. Fixtures use `label: 'fixture'`; they never prove a live build.

## Cleanup

In `finally`, call the project's existing cleanup adapter and stop only the
servers or PIDs this walk started. Keep captures and read cleanup results back.
Run `cas qa ledger check <path> --task <id>` before submission. Mark unrun cells
`NOT EXERCISED: <reason>` and retain automation errors in Honesty.
