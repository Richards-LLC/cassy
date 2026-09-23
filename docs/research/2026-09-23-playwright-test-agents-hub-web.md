# Playwright 1.63 Test Agents and bundled MCP in Cassy QA: spike on hub-web

Date: 2026-09-23
Branch: `factory/bright-robin-85`
Test bed: `hub-web/` (Cassy Commander). Before this spike it had vitest only and no Playwright suite.
Versions: `@playwright/test` 1.63.0 (exact devDependency), Codex CLI 0.156.0 (configured default model, high effort), Claude Code 2.1.280.

## Verdict

| Surface | Decision | One-line reason |
|---|---|---|
| Planner + generator agents | **Adapt** | Produced a correct, flake-free 11-spec suite in about 22 minutes. Use it for one-off bootstrapping, not a per-task loop. |
| Healer agent | **Adapt, with a guard** | 4 of 4 correct calls. Its `test.fixme()` turns a real regression into a green run, so a gate must refuse new fixme/skip markers. |
| Bundled `playwright mcp` vs `@playwright/mcp` | **Keep `@playwright/mcp` as the global worker MCP; pin it.** Use the bundled `run-test-mcp-server` only inside projects that adopt Test Agents. | See the MCP decision section below. |
| `Browser.bind` | **Adopt for debugging** | The CLI and both MCP servers attach to one live browser. |
| `--debug=cli` + `playwright trace …` | **Adopt** | Both work headless end to end, with no GUI. |
| Keep the generated suite / wire it into CI | **Operator decides.** Recommended: keep it, and wire it into the existing Commander web-assets CI step behind the fixme guard. | Adds about 7 s to a lane that already installs Chromium. |

## What was run

All commands ran from `hub-web/`. Everything is committed on the branch:

- `cb04bceb`: `npm i -D -E @playwright/test@1.63.0`, `npx playwright init-agents --loop=claude`, and the `--loop=codex` output. Also `playwright.config.ts` (vite dev server on `hub-web/fixtures`, `127.0.0.1:4791`) and `e2e/seed.spec.ts`. `vite.config.ts` gains `test.exclude: e2e/**` because vitest's default include collects any `*.spec.ts`, including the seed.
- `1186824f`: planner output `specs/hub-web-fixtures.md`, 11 scenarios.
- `7f581a15`: generator output `e2e/generated/*.spec.ts`, 11 specs as the generator wrote them.
- `4b50e9da`: the healer's diff, exactly as written.
- `06349a35`: the two fixme markers removed, because their regressions were planted and never committed, plus the rebuilt `dist/app.js` (see the cost-of-adoption section).

The fixture site uses real `src/` renderers with fixture data, selected by `?fixture=<name>`. No hub daemon is involved. The planner was pointed at six interactive fixtures: `conversation-composer`, `conversation-ask`, `pairing-step-1`, `attention-12`, `fleet-populated` and `connection-failed-retry`.

### Loop availability

`init-agents --loop` accepts `claude`, `codex`, `copilot`, `opencode`, `vscode` and `vscode-legacy`. **A Codex loop exists.** It writes `.codex/agents/playwright_test_{planner,generator,healer}.toml`. Each file sets `developer_instructions`, a sandbox mode (read-only for planner and generator, workspace-write for healer) and an agent-scoped `[mcp_servers.playwright-test]` block with an `enabled_tools` allowlist. The Claude loop writes `.claude/agents/*.md` (every agent pinned to `model: sonnet`) and a project `.mcp.json` registering `npx playwright run-test-mcp-server`. Every tool name in the Claude frontmatter exists in the 1.63.0 server's tool list, which I checked mechanically.

**The Claude loop was generated and inspected but not executed.** A real run needs a nested `claude -p`. The cli-routing account gate fails closed here: `release.claude_account_allowlist` is empty, although the probe reports a logged-in first-party `max` account. The supervisor was asked and did not approve during the spike. Both loops drive the same `run-test-mcp-server` and near-identical prompts, so the measured results below are Codex-loop results. A Claude-loop comparison is still open.

## Measured results (Codex loop)

| Agent | Wall time | Input tokens (cached) | Output tokens | Outcome |
|---|---|---|---|---|
| Planner (+ root) | 5 m 12 s | 3.14 M (3.02 M) | 9.0 K | 11 scenarios saved via `planner_save_plan` |
| Generator ×11 (+ root) | 17 m 08 s | 7.77 M (7.39 M) | 34.4 K | 11 of 11 specs written via `generator_write_test`, about 1.5 min each |
| Healer (+ root) | 3 m 10 s | 1.13 M (1.04 M) | 5.9 K | 4 tests edited: 2 fixed, 2 `test.fixme()` |
| **Total** | **25.5 min** | **12.0 M (11.4 M, 95%)** | **49 K** | |

Token counts are summed from the final `token_count` record of each Codex session rollout (root and subagents). Uncached input came to 0.60 M.

### Generated suite

- Size: 406 lines across 11 specs and 157 `expect` calls. Locators are mostly role-based: 80 `getByRole`, 20 `getByText`, 1 `getByLabel` and 23 `.locator()` (CSS or aria-label, several on `data-*` hooks). There are zero `waitForTimeout` and zero `networkidle` calls.
- Out of the generator: **9 of 11 pass**, identical over 3 runs, 0 flakes. The two failures were genuine test defects:
  - A strict-mode violation: `getByText('Fix in-train', {exact})` matches a chip and a paragraph.
  - `innerText()` output compared with `toHaveText()` normalisation.
- After healing, with fixme markers removed and a clean `src/`: **11 of 11 pass** on 3 of 3 runs (about 1.5 s with a warm server, 6.4 s with a cold vite start) and **110 of 110 under `--repeat-each=10 --workers=8`**. Flakiness observed: none.
- The suite catches real regressions. Two app regressions were planted in `src/`, uncommitted, with the diff in the appendix:
  - The pairing dialog drops the last scope from its "Exact scopes" consent line.
  - `fleetVerdict` uses a `> 1` threshold for "needs you".
  
  The final suite fails exactly those two specs (9 passed, 2 failed).

### Healer: correct fixes vs masking

The healer ran over the unhealed suite with both planted regressions live, so 4 tests were failing. It did not read the prompt as a hint: the prompt only asked it to heal `e2e/generated/`.

| Failure | Real cause | Healer action | Correct? |
|---|---|---|---|
| `conversation-ask-fix-option` | Test defect: strict-mode match, and it asserted the status's accessible name instead of its text | Scoped to `paragraph` with `^…$` and asserted the status text | Yes |
| `conversation-composer-draft` | Test defect: `innerText` vs `toHaveText` whitespace | Captured the baseline with `textContent()` | Yes |
| `fleet-populated-summary` | **Planted app regression** | `test.fixme()` with a comment naming the table/verdict disagreement. Expected text was not rewritten. | Yes (did not mask) |
| `pairing-step-1-email-validation` | **Planted app regression** | `test.fixme()` with a comment naming the missing `pane:interrupt`. Expected text was not rewritten. | Yes (did not mask) |

The score is 4 of 4. It edited no `src/` file, although it ran with `workspace-write` and the sandbox bypassed. It reported both app defects in its summary.

**Why the healer still needs a guard.** The healer's own instructions say to `test.fixme()` a failure it believes is correct. That turns a failing run into `9 passed, 2 skipped` with **exit code 0**, so a CI gate would go green on a shipped consent-screen regression. The markers are also sticky: after the regressions were reverted, the suite still skipped both tests. It stayed green without noticing the fix and would stay green on a re-break. The guard: treat any newly added `test.fixme`/`test.skip` in `e2e/generated/` as a blocking finding routed to a human or supervisor, and fail the run on unexpected skips (for example with a reporter or post-run check of `report.json`). n = 4 is too small to trust an unsupervised healer.

## MCP decision: bundled `playwright mcp` vs `@playwright/mcp` 0.0.82

Measured with a stdio `initialize` + `tools/list` client:

| Server | Playwright inside | Tools | Schema bytes | Time to `tools/list` |
|---|---|---|---|---|
| `npx playwright mcp` (project-local 1.63.0) | 1.63.0 | 24 | 18.9 K | 374 ms |
| `npx -y playwright@1.63.0 mcp` (pinned, no local install) | 1.63.0 | 24 | – | 610 ms |
| `npx -y @playwright/mcp@0.0.82` | 1.64.0-alpha | 25 (+`browser_emulate_media`) | 20.1 K | 473–512 ms |
| `npx playwright run-test-mcp-server` (Test Agents) | 1.63.0 | 89 | 60.6 K | 379 ms |

**Decision:** keep `@playwright/mcp` as the global, interactive worker MCP, and **pin it**. Use the bundled `run-test-mcp-server` only inside a project that adopts Test Agents, where it is version-locked to that project's `@playwright/test`. Reasons:

1. **Stall detection depends on the command line.** `cas-cli/src/cli/factory/wedged.rs:1641` (`is_known_sidecar_commandline`) recognises MCP sidecars by the markers `@playwright/mcp`, `playwright-mcp` and `.playwright-mcp-profile`. A bundled `… playwright mcp` or `… playwright run-test-mcp-server` process matches none of them. It would count as a worker background job, and the comment at `wedged.rs:1628` says that "would suppress stall detection forever". Switching the global registration is therefore not a config-only change: the allowlist must learn the new command lines first, with a test beside `known_mcp_sidecars_do_not_count_as_worker_background_jobs`.
2. **Bundled needs a Playwright install.** Most downstream projects have no local `playwright`. The pinned form (`npx -y playwright@1.63.0 mcp`) works but starts slower than the standalone package.
3. **Capability is equivalent.** The only tool difference is `browser_emulate_media`, which only 0.0.82 has. Both attach to a bound browser with `--endpoint`, as tested below.
4. **Our registrations have drifted and should be pinned regardless.** The repo `.mcp.json` and `~/.claude.json` use `@playwright/mcp@latest`, which currently resolves to a 1.64 alpha. The repo `.codex/config.toml` pins `@playwright/mcp@0.0.70`. Recommended: pin both to `0.0.82`.
5. **Schema size.** `run-test-mcp-server` exposes 89 tools (60.6 KB of schema) unless a client filters them. The generated agent definitions filter (`tools:` frontmatter, `enabled_tools`). The Claude loop's project `.mcp.json` does not, so a main session in `hub-web/` would load all 89.

## `Browser.bind`: one browser shared between a script and an agent

A Node script launched Chromium, opened `?fixture=attention-12` and called `browser.bind('cas-d7b7-spike', {workspaceDir})`. That returns a named-pipe endpoint (`/tmp/pw-*/browser/*.sock`).

- `npx playwright cli attach cas-d7b7-spike` attached in 808 ms and saw the script's page. `playwright cli list` shows it as `chromium (attached)`. Note that later commands need `-s=<name>`: plain `playwright cli snapshot` answers "browser 'default' is not open".
- `npx playwright mcp --endpoint <pipe>` and `npx -y @playwright/mcp@0.0.82 --endpoint <pipe>` both returned a `browser_snapshot` of the same live page.
- A second `bind()` on the same browser, for example to add a WebSocket with `{host, port: 0}`, throws `Server is already started`. That gives one bind per browser: choose pipe or WebSocket up front.

For tests, `--debug=cli` (next section) is the built-in form of this: the test runner binds its own browser and prints the attach name.

## Agent debugging surfaces (1.59+) on a deliberately broken test

The broken test is in the appendix. It asserts that the `attention-12` heading reads `dim-heron`; the heading actually reads `bright-otter▾`.

- `npx playwright test <file> --debug=cli`: the test pauses at start and prints `Run "playwright-cli attach tw-<id>"`. After attaching, `pause-at e2e/debug-demo/broken.spec.ts:8` ran to the failing `expect`. `snapshot` and `eval "document.querySelector('h1').textContent"` then returned `"bright-otter▾"`, which is the root cause in one step, and `resume` finished the run. `--debug=cli` forces `maxFailures=1`, and the summary line reads "1 error was not a part of any test"; that line is the early-stop notice, not a second failure.
- `npx playwright trace open <zip>`, then `actions`, `action 10`, `errors` and `snapshot 10`: these gave the timeline (the `toHaveText` step failing at 2.0 s), the expected/received values with the call log (21 × resolved), and an ARIA snapshot at the failing step. No GUI or `show-trace` was needed. Caveat: the stored error text keeps ANSI colour codes even with `NO_COLOR=1`/`FORCE_COLOR=0`, so strip them before quoting.

These surfaces are what the `cas-playwright-debug` skill targets, and the spike confirms they work on this machine.

## Integration friction found (Codex loop)

1. **Agent-scoped MCP servers were not started.** On the first run, the spawned `playwright_test_planner` had its role instructions but no `playwright-test` tools; it only saw the user-global `playwright` server. It fell back to reading fixture source, so that run was discarded as invalid. The fix was to register the server at the parent level: `-c 'mcp_servers.playwright-test.command="npx"' -c 'mcp_servers.playwright-test.args=["playwright","run-test-mcp-server"]'`.
2. **`codex exec` refuses MCP calls under approval policy `never`** ("MCP tool call requires approval, but approval policy is never"). The runs used `--dangerously-bypass-approvals-and-sandbox`. Any factory recipe would need a narrower per-server approval setting before this is acceptable.
3. The planner and the MCP server leave `.playwright-mcp/` snapshot files in the cwd; they are now ignored in `hub-web/.gitignore`.
4. `hub-web/.claude/agents` and `hub-web/.mcp.json` are subdirectory-scoped. Whether a Claude worker whose cwd is the repo root discovers them was not verified, because the Claude loop was not executed.

## Cost of adoption on hub-web

- **Dist churn.** `vite.config.ts` hashes `package-lock.json` and `vite.config.ts` into `__HUB_BUILD__`. Adding the devDependency therefore changes `dist/app.js`, only in the build id (`7dad3847` → `28b6d58f`). CI's `git diff --exit-code -- dist` requires committing it, and `dist/` is a Cargo input. Every future Playwright bump will do the same. If the suite is kept, consider excluding devDependency-only lockfile changes from the digest.
- **CI.** The Commander web-assets step already installs Chromium for `visual-qa`. The suite adds a vite start plus about 1.5 s of tests. `npm exec --package=playwright` in that step now resolves to the local 1.63.0.
- **Brittleness.** Several specs assert exact fixture copy, for example the pairing explainer sentence and the fleet verdict string. Copy edits will fail them; that is arguably intended for consent text.

## Should the generated suite be kept and wired into CI?

The operator decides. The evidence supports keeping it: 11 specs, 0 flakes over 3 + 110 runs, and it catches both planted regressions. If it is wired in, add it to the existing Commander web-assets step (`npx playwright test`) together with the fixme/skip guard above. Do not let the healer run in CI.

## Appendix A: planted regressions (never committed)

```diff
--- a/hub-web/src/fleet-board.ts
-  const lead = counts["needs-you"]
+  const lead = counts["needs-you"] > 1
--- a/hub-web/src/pair-dialog-markup.ts
-<dt>Exact scopes</dt><dd class="pair-identifier">${scopes.map(scopeLabel)
+<dt>Exact scopes</dt><dd class="pair-identifier">${scopes.slice(0, -1).map(scopeLabel)
```

## Appendix B: deliberately broken test used for the debug surfaces

```ts
test("broken: attention header names the wrong session", async ({ page }) => {
  await page.goto("/?fixture=attention-12");
  await page.getByRole("button", { name: "Take control" }).click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("dim-heron", { timeout: 2000 });
});
```

## Appendix C: generated tests

The tests are kept on the branch under `hub-web/e2e/generated/` (11 files), with the plan in `hub-web/specs/hub-web-fixtures.md`. `git show 7f581a15` gives the generator's output and `git show 4b50e9da` the healer's diff.
