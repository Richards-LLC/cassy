---
name: cas-playwright-debug
description: Use when a Playwright test fails, flakes or times out and needs trace evidence, a root cause or a measured flake diagnosis; authoring/configuration uses cas-playwright-tests.
metadata:
  managed_by: cas
---

# Playwright debugging

Read retained evidence before editing. Use the current stable locked install and matching browsers under the project's supported runtime; [cas-playwright-tests](../cas-playwright-tests/SKILL.md) owns configuration, targets, locators and test-agent policy.

## Read the failure

Open the named failure's retained trace. If absent, reproduce that selection with --trace=retain-on-failure and preserve the original result. The failures reporter keeps console output small without filtering full journey reports.

```bash
npx playwright trace open test-results/<test>/trace.zip
npx playwright trace actions --errors-only
npx playwright trace action <id>
npx playwright trace snapshot <id> --phase before
npx playwright trace snapshot <id> --phase after
npx playwright trace screenshot <id>
npx playwright trace attachments
npx playwright trace snapshot <id> --serve
npx playwright trace requests --failed
npx playwright trace console --errors-only
npx playwright trace errors
npx playwright trace close
```

The action records params, call log, source and available phases (before/action/after). Use --grep to narrow actions/requests; inspect error-context.md beside the trace. Serve snapshots only on a local fixture port and retain captures in task artifacts.

Classify the failed action: locator ambiguity/missing match; actionability including inert controls; wrong contract value; missing/wrong-origin route/auth; or shared-resource interference. Compare base/tip under the same seed, runtime and target before calling a regression. When correct assertions expose a product bug, file it and keep the failure; an assertion rewrite cannot repair the product.

## Step through a named failure

--debug=cli runs headed: on these hosts wrap it in xvfb-run -a and background it with a log. Never open a visible desktop browser.

```bash
xvfb-run -a npx playwright test tests/x.spec.ts:42 --debug=cli > target/pw-debug.log 2>&1 &
npx playwright cli attach tw-XXXXXX
npx playwright cli -s=tw-XXXXXX step-over
npx playwright cli -s=tw-XXXXXX pause-at x.spec.ts:57
npx playwright cli -s=tw-XXXXXX snapshot
npx playwright cli -s=tw-XXXXXX console
npx playwright cli -s=tw-XXXXXX resume
```

Use the session id after attach; snapshot refs expire after changes. Translate generated action code into the canonical semantic locator policy rather than pasting a fragile ref. Resume/close only the owned run. cli kill-all stops sessions globally: use it only in an isolated scratch home where every session is yours. PLAYWRIGHT_DASHBOARD=1 with cli show is an optional dashboard branch, also isolated under xvfb on these hosts.

## Measure a flake

```bash
npx playwright test tests/x.spec.ts:42 --repeat-each=20 --workers=4 --shuffle 1729 --add-reporter failures
```

Record failures/executions, seed, workers and the original failure signature. A single pass or failure cannot prove/disprove contention. Inspect shared identities, ports, mutable data and order-dependent setup; fix isolation first. For trace/runtime hangs, compare base/runtime controls and a labelled trace-off diagnostic without claiming it satisfies evidence requirements.

Locks are per Playwright run, not distributed coordination. Outside fullyParallel a lock may span the whole file; a named resource lock or -exclusive- belongs only to intentionally shared resources. See [current-api.md](../cas-playwright-tests/references/current-api.md) for the complete caveat.

Banned fixes: retries, raised timeouts, sleeps/waitForTimeout, networkidle, strictness-silencing .first(), swallowed catches, skipped product failures and weakened assertions. Repeated diagnostics retain failures; they never turn a failed CI run green.

Done when retained evidence names the cause, the fix preserves the title's promise, the affected selection passes and the chosen repeated/order-varied diagnostic reports its actual result. Use the project's existing integration owner for broad proof.

Sources: [agent CLI](https://playwright.dev/docs/agent-cli/introduction), [release notes](https://playwright.dev/docs/release-notes), [test locks](https://playwright.dev/docs/test-parallel#test-locks).
