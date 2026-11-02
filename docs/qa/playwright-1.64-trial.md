# Playwright 1.64 browser QA trial

Keep the 10 fps QA cap. Decline unattended healer adoption: the historical
flake did not reproduce and the generated healer permits skipping correct tests.
Coding agents default to the CLI; MCP remains available for exploratory walks.

Measured on November 2, 2026, on the local Linux factory host. Product source:
`8d4d5737fcccdac0241eb7b70520cf52f3f7e429`. This is a guidance/research change;
no production journey or application code changed.

## Recording comparison

One named shard, `HUB-J4 headings and labeled caveats stay distinct from actionable
cards`, ran once per condition in three interleaved rounds, one worker, zero
retries. All nine runs passed. Each uses the same source-bound production snapshot,
Chromium 1248 executable, desktop viewport, screenshots, trace settings and stage
captions. Only Playwright version and requested fps change. The snapshot build is
outside the measured interval. `/usr/bin/time -v` measures the whole test process
and its waited-for descendants, including server startup and recording teardown;
it is not encoder-only CPU. `ffprobe` confirms the output codec and frame rate.

| Playwright / requested fps | Actual output | Median CPU user + system | Median wall | Median video bytes |
| --- | --- | --- | --- | --- |
| 1.63.0 / 10 | vp8 / 25/1 fps | 7.47 s | 13.41 s | 948,727 |
| 1.64.1 / 10 | vp9 / 10/1 fps | 6.59 s | 13.47 s | 1,019,420 |
| 1.64.1 / 25 | vp9 / 25/1 fps | 7.28 s | 13.49 s | 1,705,518 |

Important baseline correction: 1.63.0 has no fps option. It silently ignores the
requested 10 and emits VP8 at 25 fps. There is no genuine 1.63/10 fps baseline.
The guidance now requires 1.64 for the example rather than promising that control
on 1.63. The 1.64 comparison is the valid rate control: 10 fps used 9.5% less
median total CPU and 40.2% fewer video bytes than 25 fps, with essentially equal
wall time. The three small runs on a shared host do not establish performance
under encoder contention or justify raising the cap. Across versions, 1.64/10 fps
used less median CPU but produced a larger file than 1.63/25 fps; equal visual
quality was not controlled. No general codec speed/size guarantee is inferred.

## Guarded healer trial

`npx playwright init-agents --loop=claude --project=journeys` ran in an isolated
scratch copy using 1.64.1. The generated planner, generator and healer definitions
and MCP configuration are retained with the receipt. They are trial artifacts,
not installed production repair agents.

The trial restores `read-history.journey.ts` from
`dd0fc0e214230a4b6dd54a763daa5560e3046b04`, the real HUB-J4 tail/layout flake
recorded in the [3.87 batch ledger](batch-evaluations/2026-10-28-3-87-batch-2-08688a9c3.md)
and tracked as cas-c3ac. Its historical missing font/ResizeObserver synchronization
is retained. The product build remains the common current snapshot, so this is a
historical-test replay, not a reproduction of the entire old release.

The generated healer ran via Claude in scratch, restricted to that one configured
test at one worker, retries zero, with only locator edits permitted. Its
`test_run` selected `read-history.journey.ts:169`: **one passed, 7.2 seconds**.
No debug step was needed. A byte comparison proves **zero changed lines** in the
spec. It correctly declined to claim a fix. The generated agent's `test_list`
call was refused by the trial's tool allowlist; it found the named test using
file reads and still executed the required test. Its own uncertainty about retry
configuration is resolved by the retained config (`retries: 0`) and native
journey result (`PASS`), not by claiming that it inspected the config.

Adversarial probe: the generated instructions explicitly allow assertion edits
and `test.fixme()` for a correct failing test. Our reviewed guidance rejects both,
along with new waits, retries, sleeps and timeout increases. This is a source
contract check, not proof that a model attempted or was prevented from a forbidden
patch. No such patch was proposed in this run. **Decision: decline unattended
adoption; no useful patch survived because no patch was made.** Keep optional
locator-only scratch trials and regenerate/review upstream definitions after an
upgrade. An intermittent pass does not prove the original flake fixed.

## CLI and MCP audit

The checkout's `.codex/config.toml` registers `@playwright/mcp@0.0.70` for Codex;
`.mcp.json` registers `@playwright/mcp@latest` for Claude-compatible discovery.
The local Claude user configuration also has a Playwright MCP registration.
There is no checkout `.grok` configuration, and canonical factory code does not
automatically register a separate Playwright server. This audit concerns discovery
configuration; it does not prove a server was injected into every worker run.
Shared registrations remain available for exploratory walks; default CLI choice
is documented in the canonical debug, QA and Nuxt skills.

A paired walk exercised the HUB-J2 invitation entry, filled the operator name,
cancelled and read back the closed dialog. It did not exercise the whole pairing
journey or submit a real credential. CLI 1.64.1 used targeted `find` output;
MCP used the checkout's pinned 0.0.70 server and complete snapshots (including
reading saved files when the response linked them). Both used isolated browser
contexts, the same Chromium executable, viewport and source-bound app build.

| Context material | Tokens with `cl100k_base` |
| --- | --- |
| CLI responses and required targeted discovery | 730 |
| MCP responses and required snapshot reads | 2,684 |
| MCP tool schemas, counted separately | 3,849 |

The CLI's response material was 72.8% smaller for this slice. These are reproducible
tokenizer counts, not billed model input or a full agent-session benchmark; prompt
history, request arguments and CLI help discovery are excluded. They do not prove
a wall-time or dollar saving. The separate healer trial reports actual model usage
in its transcript, but is not a CLI/MCP comparison. Native 1.64 MCP also returned
file-based snapshots in an initial probe, so old claims that MCP always injects
entire trees no longer describe every configuration. Its targeted tools could
narrow this gap; only the pinned-server walk above was completed.

## Release prep and parity

`cas-cut-release` now compares the locked version to `npm view @playwright/test
version` and flags more than one minor of lag or any major gap. For example,
installed 1.64 / latest 1.65 is within policy; latest 1.66 or 2.0 needs an upgrade
or explicit supervisor deferral. The live lookup returned **1.64.1**, matching
`hub-web/package-lock.json`; no package bump is needed. Agent regeneration and
review follow an upgrade. This is a release checklist, not a new network-dependent
merge gate.

Claude, Codex and Grok already embed these same canonical skill files; OpenCode
projects them at runtime. No divergent skill mirrors are added. The builtin
reference-history generator is rerun after committing the guidance. The exact-tip
catalog checks and source hygiene results are recorded in the task ledger.

## Sources and retained evidence

- [Official 1.64 release notes](https://playwright.dev/docs/release-notes#version-164): automatic VP9, fps options, CSS action styles, WebMCP. WebMCP is unnecessary for Commander because the app registers no browser WebMCP tools.
- [Official CLI guide](https://playwright.dev/docs/getting-started-cli): coding-agent CLI use and file-based output.
- [Official test agents guide](https://playwright.dev/docs/test-agents): scratch agent generation and regeneration after updates.
- Task artifact root: `cas-31e6/recording-results.json`, `measurements/*/time.txt`, `measurements/*/run.log`, native reports/videos/traces, `healer-before.ts`, generated agents/config, `healer-transcript.json`, `token-walk/`, and `LEDGER.md`.

Setup errors are retained separately: the first scratch copy lacked ESM metadata
and a relative CSP resource link; the token comparison initially assumed CLI and
MCP snapshot links were inline. The inherited MCP profile also conflicted with
isolated mode, and this host requires the same disabled Chromium sandbox used by
the normal test launcher. These setup attempts are not passing measurements.
