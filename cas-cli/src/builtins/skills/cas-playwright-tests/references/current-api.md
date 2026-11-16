---
metadata:
  managed_by: cas
---

# Current Playwright authoring APIs

Verify the latest stable package/lock and matching browser binaries using official release notes and the project's runtime. These examples were checked against 1.65.0 on November 12, 2026. Node 26 exposed old tracing failures in a surveyed project; that incident is not a universal package-engine requirement. Keep Node as the default runner unless this project measures another supported runner.

## Locators and evidence

- Use getByRole(role, { name, description }) when accessible descriptions distinguish same-named controls; prefer a container when that represents the contract.
- within() selects the containing match: page.getByRole('cell', { name: fixture.id }).within(page.getByRole('row')). Scope its controls from the resulting row.
- ariaSnapshotJSON() provides structured evidence for final.aria.json. Retained evidence may include full copy; assertions still prove structure/state or independent fixture values.
- getByRef('e2') consumes the current AI-mode snapshot's reference. Refresh snapshots after changes and translate exploratory refs into durable semantic locators before committing a spec.
- Use .visible() for hidden retained matches and the public upload/drop/passkey APIs for those behaviors; let actionability wait for non-inert controls rather than forcing clicks.

## Reports and iteration

Use the failures console reporter with complete structured journey reports. --add-reporter adds a diagnostic reporter without replacing configured reporters. ['html', { onlyFailures: true, open: 'never' }] and onlyFailures on JSON/JUnit are for non-journey reports without success-evidence obligations only.

Run --only-changed for local iteration, then the affected selection required by the project. For a flake investigation, use --repeat-each and --shuffle <seed> on the named selection, preserve the original failure and report failures/executions with seed and worker count. Banned: retries as a fix; do not infer that one pass/fail disproves a flake.

## Locks

Declare { lock: 'shared-fixture' } only for deliberately shared resources that cannot be isolated. -exclusive- runs while no other test in that Playwright run executes. Outside fullyParallel, a declared lock may hold the whole file, including tests without that lock; account for lost parallelism. Locks coordinate within one invocation across workers/files/projects, not a distributed mutex across independent CI runs/shard machines. Give those runs unique ports/accounts/data or use external coordination.

## Upgrade compatibility

Description-only skip/fixme/fail/slow overloads are additions; boolean and callback conditions remain. Skip policy is separate from API compatibility. Check useDefineForClassFields constructor initialization, RFC 6265 cookie behavior, appended request params and globally empty sharded selections where the project uses them. An empty shard is valid if other shards execute required tests; a globally empty required suite is not.

After upgrading, generate init-agents and init-skills for the project's harness in scratch, inspect the diff and retain this policy layer. Do not commit an upstream healer that edits assertions or hides product bugs.

Sources: [release notes](https://playwright.dev/docs/release-notes), [test locks](https://playwright.dev/docs/test-parallel#test-locks), [reporters](https://playwright.dev/docs/test-reporters), [agents](https://playwright.dev/docs/test-agents), [tagged types](https://github.com/microsoft/playwright/blob/v1.65.0/packages/playwright/types/test.d.ts).
