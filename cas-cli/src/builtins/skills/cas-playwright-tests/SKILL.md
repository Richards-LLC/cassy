---
name: cas-playwright-tests
description: Use when writing or reviewing optimal Playwright tests, e2e/browser tests, journey specs, playwright.config, locators, auth state or test agents; failures use cas-playwright-debug, evidence uses cas-qa-craft.
metadata:
  managed_by: cas
---

# Playwright tests

## Faith contract

1. Write the title as a user-visible promise. End with assertions proving that exact outcome: rendered state, a persisted record through its consumer, or a delivered message. A click or request alone does not prove the result.
2. Sabotage the guarded behavior through the real consumer: revert the fix, delete the guarded line or return a wrong fixture value. Keep that red run's trace; restore the build and retain green proof. An unrelated timeout is not sabotage evidence.
3. Make green honest: retries zero, failOnFlakyTests true, no swallowed errors or soft returns. Missing required data fails; declared optional lanes skip visibly with reasons. Check the aggregate required executed count and every unexplained skip using the project's report guard; expected failures execute and unexpected passes fail.
4. Use seeded data and bounded event/web-first completion. Install a controlled clock only for time-dependent logic. Banned: networkidle, waitForTimeout/sleeps, strictness-silencing .first(), swallowed catches and isVisible() branching that avoids required assertions.
5. Retain trace, video and final ARIA on failure; journey/evidence projects retain successful captures too. Keep full structured/HTML journey reports; the failures console reporter only trims terminal noise. Follow `cas-qa-craft` for the evidence bundle.

## Build the spec

1. Verify the latest stable version and chosen supported Node runtime; upgrade the locked package and matching browser binaries first. Stable verified November 12, 2026: 1.65.0. Recheck upstream on a later upgrade; this is not a permanent version floor.
2. Use the project's canonical config/projects and testMatch allowlist, not per-ticket configs. Apply [configuration.md](references/configuration.md) for retries, reports, required coverage, headless operation and targets. Keep the established integration owner; local --only-changed is iteration, not final proof.
3. Validate resolved frontend and API origins against a reviewed local/staging allowlist before contexts, login, setup or cleanup. Production is forbidden. Gate remote mutations and cleanup separately; forbidOnly merely rejects focused tests.
4. Choose hermetic or live. Hermetic tests install a context-wide deny-by-default network allowlist before navigation/popups, block bypassing Service Workers and account for WebSockets/other channels. Live tests use reserved QA identities and explicit mutation permission; keep them out of the hermetic PR lane.
5. Reuse auth state from role setup projects, with a fresh context/page per test. Namespace mutating accounts/data by run, worker and shard; prefer isolation over locks. For Firebase/Nuxt/Quasar, load [nuxt-quasar-firebase.md](references/nuxt-quasar-firebase.md).
6. Use role/label locators, then a stable test id, scoped to a semantic container. Assert fixture-derived values, state and structure. Load [locators-and-accessibility.md](references/locators-and-accessibility.md) for Quasar teleports, accessibility and the single allowed CSS assertion category.
7. Load the page fresh at each viewport, capture at that viewport, and verify keyboard/recovery paths. Seed → planner → generator → review is the agent flow; review the final assertion against the title before accepting generated code. Generate official agents/skills in scratch and diff them against this policy.
8. Run the named affected selection. Classify failures against base and tip using the same seed/runtime/target before calling a regression. Diagnostics belong to `cas-playwright-debug`; measured suite audits belong to `cas-test-audit`.

## Fast by design

Reuse storageState, not mutable pages. Default to hermetic fixtures, fullyParallel and independent test data; shard independent tests. Put expensive pass evidence in the journey project and live smoke in its separate post-deploy lane. Do not install a second full-suite gate.

Load [current-api.md](references/current-api.md) for within(), descriptions, ARIA JSON/AI refs, locks, reporters and CLI details. Locks coordinate a single run; independent invocations need unique resources or external coordination.

The healer may diagnose one named failure only in scratch with a reviewed patch: locator/wait fixes may ship after rerun, assertion/expected-value changes never do. Correct tests exposing product defects stay red and get a bug owner; do not suppress them.

Done when the final assertion fulfills the title, sabotage and restored proof are retained, no required coverage silently skipped, and changed consumers share the canonical config/locator policy.
