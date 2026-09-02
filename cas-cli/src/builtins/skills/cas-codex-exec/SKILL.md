---
name: cas-codex-exec
description: Use when a token-heavy READ-ONLY investigation needs a one-shot `codex exec` subprocess, such as log mining, bulk inspection, or an independent analysis; never use for edits or task ownership.
managed_by: cas
---

# cas-codex-exec

Use `codex exec` as a disposable read-only investigator when the question is too token-heavy for the current agent but does not need a factory worker, persistent context, or edits.

## Command

This is the canonical `codex exec` recipe; every other Cassy skill that shells
out to Codex points here rather than restating its own variant.

```bash
/usr/bin/timeout 600 codex exec -s read-only -C "$PWD" "<prompt>"
```

Useful flags:

- `-s, --sandbox read-only` keeps model-generated shell commands read-only.
- `-C, --cd <DIR>` sets the working root.
- Omit `-m/--model`. The configured default is correct and a pinned slug goes
  stale; pass one only when a specific model is the point, and never a
  `-codex`-suffixed slug.
- `-o, --output-last-message <FILE>` writes the final response for polling or later notes.
- `--json` prints JSONL events when event streams are easier to inspect.

For long sweeps, run in the background and poll an output file:

```bash
out=/tmp/codex-exec-investigation.txt
/usr/bin/timeout 1800 codex exec -s read-only -C "$PWD" -o "$out" "<prompt>" &
```

## Prompt Shape

Codex gets no useful conversation context. Give it a self-contained prompt:

- State the question.
- Name the files, directories, logs, or commands it may inspect.
- Specify the output shape and level of detail.
- End every prompt with: "If you find nothing, say so explicitly and name what you inspected."

Example:

```text
Inspect .cas/logs and session JSONL files for the last supervisor close rejection involving cas-1234. Return: root cause, exact files/logs inspected, and the shortest next action. If you find nothing, say so explicitly and name what you inspected.
```

## Use When

- Mining large session transcripts, logs, specs, PDFs converted to text, fixtures, or vendored docs.
- Sweeping many files for a pattern and summarizing hits.
- Asking for an independent read-only second opinion before assigning or escalating work.
- Reducing large evidence into a short report for task notes or supervisor messages.

## Do Not Use When

- The work requires edits, commits, task lifecycle changes, or coordination.
- A factory worker should own the task and produce a verifiable diff.
- The answer depends on current conversation context that is not in the prompt.

## Failure Modes

- If `codex` is not installed or not on `PATH`, say so and do the investigation directly.
- If auth is expired, report that and fall back to direct investigation or ask the supervisor to refresh auth.
- If the command times out, report the timeout, inspect any `-o` output file, and narrow the prompt before retrying.
