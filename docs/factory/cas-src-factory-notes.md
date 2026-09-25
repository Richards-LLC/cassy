# cas-src supervisor notes

Text removed from the shipped `cas-supervisor` skill, its references, and the
supervisor checklists because it is specific to the Cassy source repository or
its release train. Builtin skills ship to every project, so these notes live
here instead. Each section names the file it came from. The text is verbatim
except that headings are demoted, bare code fences carry a `text` language,
numbered steps keep their original number in bold text, and blank lines, list
markers and link brackets are normalized for Markdown lint.

## From `cas-supervisor-checklist.md` and `cas-codex-supervisor-checklist.md` — step 0 binary freshness

The shipped step 0 now runs `cas factory preflight`. The cas-src manual check was:

Step 0. **Binary freshness check.** Before anything else — confirm the running `cas serve` binary matches HEAD of this repo. A stale binary may impose legacy global verification blocks instead of the current exact-task close gate. See [preflight.md](../cas-supervisor/references/preflight.md) for the full command; the 10-second version:

   ```text
   # cas --version format: cas 2.27.0 (9b52e17-dirty 2026-07-16)
   # Fields after '(': short hash, optional -dirty (build tree had local mods), then build date.
   # Do NOT use awk '{print $NF}' — that grabs the date token, not the hash.
   cas --version | sed -E 's/.*\(([0-9a-f]+)(-dirty)? .*/\1/'   # → 9b52e17
   git rev-parse --short HEAD                                   # hash of the repo right now
   ```

   If they don't match AND `git log --oneline HEAD --not <running-hash> -- cas-cli/src/mcp cas-cli/src/hooks cas-cli/src/cli/factory` returns anything, the binary must be rebuilt — but **do not kill or restart `cas serve` from this active MCP session**. That stdio process is this session's Cassy-tool connection, so restarting it here disconnects the very tools needed to finish setup.

   Stop at step 0 and ask the operator to run `cargo build --release` and use the harness's MCP reconnect/restart control (or open a fresh supervisor session) to launch the new `cas serve`. Do not use `pkill` or any name-based process kill. Resume only after the Cassy tool list is restored, then rerun this checklist from step 0.

## From `cas-supervisor/references/workflow.md` — resuming an epic

Step 1. **Check for binary/source drift** — fixes merged to main since last session don't take effect until rebuild. Run `~/.cargo/bin/cargo build --release` if Cassy source changed, then restart `cas serve`. If a "fixed" bug reappears, this is the first thing to check.

## From `cas-supervisor/references/workflow.md` — worker build cache

Refresh the quiescent baseline from the epic tip
with `scripts/refresh-worker-build-cache.sh` during a quiet window.

## From `cas-supervisor/references/workflow.md` — Phase 4 assembly gate

Step 3. Run the final assembled-tree gate. This is the epic's single Rust build:
   workers never build, so one full build + test of the epic tip proves every
   child and checks cross-task integration (add `cargo test -p cas --doc` when
   the epic touches doctests):

   ```bash
   cargo nextest run -p cas
   ```

Step 5. After the fix lands, rerun the final assembled-tree gate yourself on the new
   tip, capture the real exit code, and record a fresh `ASSEMBLY_PROOF` for it:

   ```bash
   cargo nextest run -p cas > <artifacts_root>/<epic-id>/assembly-nextest.log 2>&1; echo $?
   ```

   Never pipe the test run to `tail`; that captures the pipe status, not the
   nextest status.

## From `cas-supervisor/references/planning.md` — review cadence

Phase 4 runs the full final-tree nextest gate for cross-task integration.

## From `cas-supervisor/references/worker-recovery.md` — legacy verification jail

**Recovery (binary is current — exemption should apply):**

1. Rebuild Cassy: `~/.cargo/bin/cargo build --release` and restart the `cas serve` process
2. Respawn workers — they will pick up the new binary

## From `cas-supervisor/references/epic-driving.md` — release train and authoring rules

- Reject any child whose `WorkTarget` resolves to trunk; repair its target before spawning (GH #625).
- At session start, process `awaiting_merge` before open work; re-evaluate externally parked merges and install a durable wake signal when no event can wake them (GH #624).
- Carry the version bump, CHANGELOG section, and release-notes draft as the epic branch’s final commit; land them through its single integration PR before tagging for one tree, one queue cycle; reserve `release/vX-prepare` for multi-PR batch releases (version lives in the tree; the merge queue revalidates every tree).
- Own the release cut; wait for Release Prebuild completion before tagging or publishing.
- Keep this reference under the 2 KB operator budget; split new guidance into a separate reference file and link it when this compact playbook would grow.
- Mirror this skill/reference change into Claude, Codex, and Grok builtin trees; run flavor-drift and sync tests.

## From `cas-supervisor/references/epic-flow-walk.md` — provenance

This combined-tip check addresses the regressions found in
<https://github.com/Richards-LLC/cassy/issues/759>.

## From `cas-supervisor/references/filing-cas-bugs.md` — cas-src routing and receipts

- **Project bug or feature:** `issues.repo` — the current project's own issue
  tracker. In cas-src, create the corresponding in-repo task.

If you hit a bug during operation, file a ticket in the matching repo before moving on. Actionable requests for a Richards-LLC-controlled team belong on
that component's issue board; never write, commit, or push in that team's
checkout from this repository.

- **Receipt:** after every cross-team filing, save a Cassy memory with the issue
  URL, one-line ask, and date. Recent examples are cloud-to-Cassy GH #215 and
  Cassy-to-cloud `Richards-LLC/petra-stella-cloud#44`.
