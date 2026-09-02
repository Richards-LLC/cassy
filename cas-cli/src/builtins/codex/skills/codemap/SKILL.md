---
name: codemap
description: Use when the user asks for a codemap, or when .claude/CODEMAP.md is missing or stale.
managed_by: cas
---

# Codemap

Produce a **short, structural map** of the repo at `.claude/CODEMAP.md`. The goal is to replace blind glob/grep exploration with a 100–150 line index that names every directory worth opening and one-lines what lives there. File-structure facts only — domain content belongs in `docs/PRODUCT_OVERVIEW.md` (the `project-overview` skill).

**IMPORTANT: All file references use repo-relative paths** (e.g., `crates/cas-core/src/lib.rs`), never absolute paths.

## What this skill is (and isn't)

- **IS:** a navigational index of the repo. Names directories, workspace members, key entrypoints, where each subsystem lives.
- **IS NOT:** a product overview, a domain doc, an architecture deep-dive, or a README. `project-overview` covers product/domain; READMEs cover human onboarding.

If the project also has `docs/PRODUCT_OVERVIEW.md`, assume the reader has it. Don't restage product/domain content here.

## Check the knowledge store first

`.claude/CODEMAP.md` is a **view** over the project knowledge store, not an independent artifact. Before reading any source, ask the store what it already knows:

```bash
cas knowledge search "codemap module layout workspace"
```

- **A page comes back and its sources are current** (the paths it cites have not moved since it was written) — read it with `cas knowledge read <id-or-path>` and use it as your draft. You are reconciling a document, not writing one from scratch.
- **Nothing comes back, or the page cites paths that no longer exist** — the store has nothing usable. Do the full read order below.

Never skip writing `.claude/CODEMAP.md` just because a page exists. The page is the distilled form; the file is the artifact the freshness gate and other agents read.

## Read order (highest signal first)

Read only what's needed to map the structure. Stop once every top-level directory has a one-liner — do not exhaustively skim files.

1. **Workspace / package roots** — `Cargo.toml` `[workspace.members]`, `package.json` `workspaces`, `pnpm-workspace.yaml`, `pyproject.toml` — these enumerate every first-class package.
2. **Top-level directory listing** — name and purpose of every entry under repo root.
3. **Crate / package entrypoints** — `src/lib.rs`, `src/main.rs`, `src/index.ts`, `__init__.py`. Read enough to know what each crate exports.
4. **Module roots inside large crates** — `mod.rs` files, `src/` subdirectories that group cohesive subsystems.
5. **Route / handler files** — `cli/mod.rs`, `routes/`, `app/`, `handlers/`, `pages/`. These reveal user-facing surface area.
6. **Tests directory layout** — note the convention (`tests/`, `__tests__/`, inline `mod tests`), don't enumerate every test file.

**Skip** framework chrome and noise: `target/`, `node_modules/`, `dist/`, `build/`, lockfiles, `vendor/`, generated clients, snapshot directories, fixture trees, CI YAML, ESLint/Prettier configs, `.git/`.

## Output structure (fixed)

Write to `.claude/CODEMAP.md`. Target **100–150 lines**. Hard cap 200 lines. The file is grep-bait — short lines, lots of paths, one-liner per entry.

```markdown
# <Project Name> — Codemap
> Auto-generated structural map. Regenerate with `/codemap` when the layout drifts (modules added, removed, or renamed).

## Top-level layout
- `<dir>/` — one-line purpose
- `<dir>/` — ...
(every entry under repo root that isn't junk)

## Workspace / packages
- `<member-path>` — one-line purpose, language/framework hint
- ...
(only if the repo is a workspace; otherwise omit this section)

## <Member or top-level dir name>
Brief sentence (one line) on what this subsystem does.
- `path/to/module/` — purpose
- `path/to/entrypoint.ext` — purpose
- ...
(repeat per major subsystem; aim for 5–15 entries each, not exhaustive)

## Cross-cutting
- **Tests:** convention + where they live
- **Docs:** `docs/`, `README.md`, `CLAUDE.md`, planning dirs
- **Tooling / scripts:** `scripts/`, `.github/`, `Makefile`, etc.
- **Config:** `.claude/`, `.cas/`, env files, root-level configs

## Entrypoints
- CLI: `<path>` (binary name)
- Library: `<path>` (crate/package name)
- Service: `<path>` (binary/server name)
- Tests: `<command>` (e.g., `cargo test`, `pnpm test`)
```

## Quality bar — every line earns its place

Every line in the codemap must answer: *"if I'm hunting for X, does this line tell me where to look?"*

If yes, keep it. If it just restates the directory name, cut it.

- ❌ `src/` — source code
- ❌ `tests/` — tests
- ❌ `lib.rs` — library entrypoint
- ✅ `crates/cas-core/src/hooks/` — hook input schema, dispatcher types, handler trait
- ✅ `cas-cli/src/cli/codemap_cmd.rs` — `cas codemap status|pending|clear` subcommands
- ✅ `apps/web/src/routes/api/` — public REST endpoints (one file per resource)

When in doubt, **name a concrete module or filename** that lives there.

## After writing the doc

Follow [references/doc-hygiene.md](references/doc-hygiene.md) for the three steps this skill shares with `project-overview` and `design-spec`: keep-block preservation on re-runs, the thin pointer memory, and the commit. This skill's specifics:

- **Keep-blocks** most often wrap `## Cross-cutting` notes (hot paths, migration gotchas). Put them back in the section they came from.
- **Pointer memory title:** `project_<slug>_codemap.md` (slug = lowercase kebab-case of project name). Body example: `See [.claude/CODEMAP.md](.claude/CODEMAP.md) — Rust workspace + TS frontend; CLI lives in `cas-cli/`, hooks in `crates/cas-core/`.`
- **Commit** `.claude/CODEMAP.md`. The freshness gate (SessionStart hook + `cas codemap status`) uses **git history** as the sole authority, so the commit is what resets the signal. Verify with `cas codemap status` → `Status: up to date`. No manual `cas codemap clear` is required.
- **Report back:** (a) total line count, (b) how many top-level subsystems are mapped, (c) anything notable about the layout (workspace? monorepo? polyglot?).

### Seed the knowledge store

`.claude/CODEMAP.md` is a distillable source, so one build turns the doc you just wrote into a knowledge page plus a source-ledger entry:

```bash
if cas knowledge build --timeout-secs 90 --max-sources 5; then
  build_exit_status=0
else
  build_exit_status=$?
fi
```

The command is deliberately best effort. If it returns a non-zero exit status, record the durable receipt in task notes with the command and exit status, then continue with the CODEMAP commit and `cas codemap status` proof; a failed build is non-blocking and may leave the knowledge page stale or missing. Rust enforces one 90-second wall-clock deadline across the complete build, stops later completions after exhaustion, and terminates/reaps the active provider process group, so a stalled build leaves no ordinary orphan descendant.

Do not detach or background the build, run a manual polling loop, or wait beyond the 90-second bound. This is one bounded invocation observed once. Nothing else in the repo changed, so the ledger short-circuits every other source and this costs at most one model call. Confirm it landed:

```bash
cas knowledge search "codemap"
```

If the store is not initialized in this project, the command says so and there is nothing to fix — the doc on disk is still the artifact of record.

## When to run

- **Missing:** `.claude/CODEMAP.md` does not exist → SessionStart fires a `severity="high"` banner. Nothing is blocked. Generate from scratch.
- **Stale:** SessionStart reports structural changes since CODEMAP.md was last updated → regenerate, keep-blocks survive. Only once staleness reaches `SignificantlyStale` does PreToolUse gate a supervisor's `task create` / `spawn_workers`; a missing CODEMAP never does.
- **Manual:** user invokes `/codemap` or asks to refresh the codemap.
- **After refactors:** modules were added, removed, or renamed across more than a handful of files.

## Anti-patterns

- Listing every file in the repo. This is a map, not an inventory. If a directory has 50 files, name the directory and 1–3 representative files.
- Drifting into product/domain content (personas, journeys, business concepts). That's `project-overview`'s job.
- Generic one-liners that just restate the path (`tests/ — tests`). Cut the line or write a real one.
- Skipping the keep-block check on regeneration. Destroying hand-edits is a trust breaker.
- Forgetting to commit `.claude/CODEMAP.md`. Freshness is computed from git history — committing resets the staleness signal for the next session.
- Forgetting to write the memory pointer.
- Regenerating from scratch without checking `cas knowledge search` first. A current page is a draft you should be reconciling, not discarding.
- Forgetting `cas knowledge build` after writing the doc. The store then keeps serving a stale page to every agent that queries it.
- Including `target/`, `node_modules/`, `dist/`, `vendor/` as if they were source. They aren't — skip them.
