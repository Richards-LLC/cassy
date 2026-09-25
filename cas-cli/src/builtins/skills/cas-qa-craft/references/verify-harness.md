# Verification kit generator

Generate a repo-tracked, harness-neutral verification kit so a QA pass starts
from a proven launch recipe and a map of user-facing features instead of
rediscovering both. The kit is two parts:

- `docs/qa/verify.md`: how to launch, check, drive, capture and clean up.
- `docs/qa/features/`: a `README.md` index plus one file per user-facing
  feature (template: [feature-template.md](feature-template.md)).

**Background chore only.** Generating or repairing the kit never blocks a task
close, a merge or a QA gate. No close path waits on it. Run it as its own task
or in otherwise idle time; if a delivery needs QA before the kit exists, run
the normal procedure in `SKILL.md` without it.

## 1. Interview the repo

Answer five questions from the code, the README, package scripts, Makefiles,
CI config and existing tests. Write each answer as a command or a path, not a
description. Mark an answer `unknown` rather than guessing.

| Question | What to find |
| --- | --- |
| Surface | Who uses it and through what: web app, CLI, TUI, API, desktop, mobile. |
| Run | The one command that starts it locally, its port or binary path, required env vars (names only, never values). |
| Drive | How an agent operates it: Playwright against a URL, the real binary with arguments, HTTP requests. |
| Observe | Where a run leaves evidence: logs, a database, stdout, screenshots, telemetry. |
| Isolate | How to run without touching shared state: a temp data dir, a test database, a sandbox account, a spare port. |

## 2. Write `docs/qa/verify.md`

Use exactly these five H2 sections, in this order:

- **Launch**: the command, run in the background, and the readiness signal:
  the log line, HTTP status or file that proves the app is up. Name the
  timeout. Register long-lived processes through `cas-servers`.
- **Doctor**: read-only health checks that run against the launched app
  (a health URL, `--version`, a status command). A Doctor step never writes
  data, migrates or resets anything.
- **Drive**: the smallest command that exercises one real user path, plus the
  conventions an agent follows (base URL, login helper, test account source).
- **Evidence**: where screenshots, traces, logs and terminal captures go and
  which command produces each. Launch plus Doctor output is the producing
  command an evidence bundle cites.
- **Cleanup**: **kill only what you started; evidence survives.** Stop the
  processes Launch started (by the PID or registered server name it recorded),
  remove temp data the run created, and keep every capture.

## 3. Write `docs/qa/features/`

List the user-facing features from routes, commands, menu entries and
navigation. One feature is something a user would name ("Billing settings",
"Export to CSV"), not a component. Write one file per feature with the five
H2s from the template: **Sub-features**, **How to get to it** (the user's
point of view, every entry point on its own line), **Driving it**,
**Gotchas**, **Touches** (source globs, one per bullet, in backticks). Put
routes and selectors under Driving it in backticks so the static check can
grep them.

Write `docs/qa/features/README.md` as the index: one line per feature file,
linked by filename.

Run the static check and fix every failure before the self-proof:

```
node <skills-dir>/cas-qa-craft/scripts/check-feature-map.mjs --root .
```

`<skills-dir>` is `.claude/skills`, `.codex/skills` or `.grok/skills`.

## 4. Self-proof once

Prove the kit works by following it literally, with a hard cap of about
**15 minutes**:

1. Launch, and wait for the readiness signal.
2. Doctor.
3. One drive: the first feature file's first Driving it step.
4. Cleanup.

Record the result at the top of `docs/qa/verify.md`:
`Self-proof: <date> <commit> clean`. If any step fails or the cap expires,
write `Self-proof: <date> <commit> blocked: <step and error>`, commit the kit
as it is, and stop. A blocked kit is still useful, and the next maintenance
sweep ([maintain.md](maintain.md)) picks it up.

## 5. Using the kit in a QA pass

Map rows feed the exploration matrix. For a touched feature, each entry point
under "How to get to it" is its own row; a row passes only through its own
entry point. "Verified via another entry point" does not count. A failed
Driving it step is triaged in the same round as doc drift, harness gap or
product regression (definitions in [maintain.md](maintain.md)).
