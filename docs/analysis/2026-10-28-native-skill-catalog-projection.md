# Native skill catalog projection: supported metadata, root limitation

Worker-local project skill copies cannot safely replace the installed personal
catalog in every harness. Codex supports the tested combination of project
copies and invocation-local disable rules for the original paths. Claude and
Grok continue advertising and invoking the personal copies. No production
launch policy is enabled by this prototype; the launch byte target remains
unmet.

All three installed harnesses support hiding a skill from automatic discovery
while preserving actual explicit invocation. This is different from disabling
the skill. The missing piece is a launch-local way to apply that metadata to
personal skills without changing the account's shared files or its broader
profile semantics.

## Executed evidence

[The reversible probe](../../scripts/probe-skill-catalog.py) creates 50 public
fixture skills in disposable homes, selects six for automatic discovery, and
runs the real installed CLIs against a local scripted HTTP model endpoint. The
endpoint returns only a final message: it never reads files or calls tools.
A unique body marker in the outbound request therefore proves native skill
expansion, rather than a successful lookup or a model pretending to invoke it.
Different original/project markers identify which copy ran. Source fixture
files are checked for changes after every case.

| Native CLI | Baseline visible names | Explicit-only metadata | Hidden skill invoked by name | Project-root copies plus untouched personal originals |
| --- | ---: | ---: | --- | --- |
| Codex 0.165.0 | 50 | 6 | `$fixture-skill-49`: body loaded | 6 visible; project body loaded |
| Claude Code 2.1.312 | 50 | 6 | `/fixture-skill-49`: body loaded | 50 visible; personal body loaded |
| Grok 1.0.70, f50904eaee6c | 50 | 6 | `/fixture-skill-49`: body loaded | 50 visible; personal body loaded |

The explicit-only control changes metadata inside its disposable personal
fixture. It proves native semantics, **not** an acceptable deployment strategy
for the operator's real personal directory. The project-root treatment leaves
the original fixture files unchanged. Codex additionally receives per-path
`skills.config` overrides in its disposable invocation config to suppress the
original copies; the project copies remain invocable.

A separate Codex negative control uses `skills.config enabled=false` without
replacement copies. The native catalog shrinks, but `$fixture-skill-49` does
not expand its body. That control rules out disabling skills as the solution.

### Fixture byte replay

Each baseline/treatment uses the same native executable, fixture generator,
HTTP endpoint implementation, and capture/extraction code. These are measured
UTF-8 bytes, not billed tokens or production savings.

| Harness | Baseline fixture-bearing rows | Explicit-only fixture-bearing rows |
| --- | ---: | ---: |
| Codex | 20,399 | 2,447 |
| Claude | 18,549 | 2,225 |
| Grok | 21,949 | 2,633 |

These row counts exclude surrounding native instructions and normalize the
temporary root to `<fixture>`. Grok renders multiple fixture-bearing lines per
skill. The JSON also records unnormalized containing-text bytes; the saved text
captures make extraction auditable. Containing-text totals can include adjacent
tool instructions and must not be reported as the catalog alone. Neither the
six-skill fixture nor this normalization proves a production catalog below
2,500 characters. Native wrappers, real paths, plugins and non-Cassy skills
must be included in an eventual launch acceptance capture.

## Supported controls and the remaining seam

- [Codex skill documentation](https://developers.openai.com/codex/skills)
  documents `agents/openai.yaml` with `policy.allow_implicit_invocation=false`,
  retaining explicit invocation, and per-path enable/disable config. The native
  probe confirms both the positive and negative invocation behavior.
- [Claude skill documentation](https://code.claude.com/docs/en/skills#resolve-skills-that-share-a-name)
  gives personal skills priority over project skills. It also documents that
  `disable-model-invocation: true` removes automatic descriptions while keeping
  explicit slash invocation. Both facts match the native request captures.
- [Grok skill documentation](https://docs.x.ai/build/features/skills-plugins-marketplaces)
  documents explicit-only metadata and its discovery locations. The tested
  local project copies do not override personal copies; this precedence result
  is an observation for the recorded version, not a claim about every version.
- [Claude's directory documentation](https://code.claude.com/docs/en/claude-directory)
  says `CLAUDE_CONFIG_DIR` redirects the whole personal Claude directory.
  [Grok's settings reference](https://docs.x.ai/build/settings/reference)
  defines `GROK_HOME` as the root for config, auth, sessions, skills, plugins and
  logs. These are whole-profile selectors, rather than skill-only selectors.

The current Cassy launch seam binds a Claude account and independent secure
storage selector in `crates/cas-mux/src/backend/claude.rs:47` and
`crates/cas-pty/src/pty.rs:669`; it records workspace trust in the selected
profile in `crates/cas-mux/src/backend/claude.rs:76`. Codex similarly binds its
account home before trust and hook preparation in
`crates/cas-mux/src/backend/codex.rs:53`. Substituting a new profile directory
is therefore broader than projecting the skill catalog. A native fixture with
no credentials does not verify account selection, credential refresh, hook
trust, session history, recycle, or verifier inheritance under such a change.

No supported skill-only override that defeats personal precedence was found for
Claude or Grok in the checked official documentation and installed CLI help.
This is a bounded design limitation, not a claim that profile isolation can
never work.

## Recommendation

Keep the production launch unchanged. The next implementation decision is
whether to build a worker/reviewer-owned **whole-profile overlay** for Claude
and Grok, preserving the selected account and separating writable state from
shared files. That needs account/trust/history/recycle tests and independent
verifier-launch evidence before enabling it. Codex can retain its existing
account home and use local projected copies plus invocation-local original-path
suppression.

Once that seam is approved, generate compact role/task descriptions and a full
on-demand file catalog from the complete builtin installation inventory. Preserve
every skill's explicit invocation and relative resources. Do not replace native
invocation with MCP lookup, disable slash commands, or claim a SessionStart-only
byte reduction. Supervisor launches and shared profiles remain outside this
prototype.

## Reproduction

For each installed harness, run the same probe with its executable and a separate
JSON output path, for example:

```bash
python3 scripts/probe-skill-catalog.py --harness codex \
  --executable /usr/local/bin/codex --output target/skill-catalog/codex.json
python3 scripts/probe-skill-catalog.py --harness claude \
  --executable /home/pippenz/.local/bin/claude --output target/skill-catalog/claude.json
python3 scripts/probe-skill-catalog.py --harness grok \
  --executable /home/pippenz/.grok/bin/grok --output target/skill-catalog/grok.json
python3 scripts/probe-skill-catalog.py --harness codex \
  --executable /usr/local/bin/codex --mode disabled \
  --output target/skill-catalog/codex-disabled.json
```

The probes use fresh child environments and fixture-only API keys. No real
account/profile is read, copied or changed. Each temporary HTTP listener is
closed before its probe returns, and fixture homes are removed. The installed
harness binaries are prerequisites; absence is an error, never a passing skip.
