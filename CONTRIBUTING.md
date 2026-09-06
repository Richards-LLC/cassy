# Contributing to CAS

Thank you for your interest in CAS.

## Contribution Model

This is the `Richards-LLC/cassy` development repository. Contributions are accepted on a **case-by-case** basis.

- **Small, high-signal fixes** (bug fixes with clean diffs, warning hygiene, regression repros) are welcome. Send a PR.
- **Larger changes** (new features, refactors, public API changes) — please open an issue first so we can talk about scope before you spend time on a diff.
- **No process** beyond that. We trust contributors to be honest about what they're sending.

## How to Participate

### Report Bugs

Open an [issue](https://github.com/Richards-LLC/cassy/issues/new) with:

- A clear description of what happened vs. what you expected
- Steps to reproduce
- Your OS, CAS version (`cas --version`), and relevant configuration

### Suggest Features

Open a [discussion](https://github.com/Richards-LLC/cassy/discussions) before writing code, especially for anything that touches the public CLI / MCP surface or the daemon protocol.

### Build from Source

```bash
git clone https://github.com/Richards-LLC/cassy.git
cd cassy
cargo build --release
```

See the [README](README.md) for full build instructions.

## Visual QA for user-visible HTML

Run the headless visual gate before merging an HTML report, Commander surface, or other user-visible
HTML artifact. It renders light and dark schemes at desktop and phone widths, writes JSON/Markdown
findings and full-page screenshots to `docs/factory/data/visual-qa/`, and exits non-zero with
`--strict` when a finding remains:

```bash
node scripts/visual-qa.mjs \
  --strict --artifact-dir docs/factory/data/visual-qa \
  docs/factory/2026-09-06-model-lane-rubric-review.html \
  hub-web/dist/index.html
```

Use `scripts/visual-qa-allowlist.json` only for intentional exceptions. Every entry must include a
finding `type`, an element `selector`, and a specific `reason`; the generated Markdown receipt keeps
the allowlisted count visible for review.

## Viktor distribution

Viktor changes must keep the Claude, Codex, and Grok builtin skill mirrors in sync, preserve the
credential-safe `cas viktor` output, and retain the proxy's exact allowlist boundary. Do not add
credential literals to source, fixtures, docs, or artifacts: `cas serve` receives only the
`VIKTOR_API_KEY` environment reference. New behavior needs a clean-project `cas init`/command
assertion and a registry test so every harness receives the managed skill.

## CI check names are pinned

CI job names are not free text. `docs/branch-protection/main-ruleset.json` lists required
status checks for `main` by their **exact** job `name:` in `.github/workflows/ci.yml`, and
GitHub matches required checks by name string.

**If you rename a CI job, update that JSON in the same commit.** A required check whose name no
longer exists is never reported, so it never passes — and every affected branch stays
unmergeable until someone notices. GH #138 renamed these jobs once already.

Currently pinned: `Fast Validation` and `macOS Check`.

`Release-Profile & Build Guard (compile-only, no test suite)` is deliberately **not** a required
check: its `if:` condition skips pull requests, so requiring it would make every PR wait forever
on a check that cannot run. Keep it that way unless its triggers change.

See [docs/branch-protection/README.md](docs/branch-protection/README.md) for the full ruleset
and the rationale.

## Code of Conduct

Be respectful and constructive in all interactions.
