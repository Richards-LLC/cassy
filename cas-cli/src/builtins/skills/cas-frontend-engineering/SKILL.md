---
name: cas-frontend-engineering
description: Use when turning an approved concept brief into accessible, performant frontend implementation with explicit component, state, token, motion, and Playwright acceptance.
metadata:
  managed_by: cas
---

# Frontend engineering

Use this skill for implementation craft. Read the approved concept brief and
`DESIGN.md` before editing UI. `cas-ui-craft` owns visual direction and critique;
`cas-playwright-debug` and `cas-nuxt-playwright` own harness setup and debugging.
This skill turns their intent into a maintainable, measurable implementation.

## Procedure

1. **Map the component boundary.** Name components by domain role, give each one
   responsibility, and keep its public props/events/slots small. Keep data
   ownership in the nearest component that can make the decision; do not let a
   child reach around its parent or make a visual detail into a global API.
   Record the component map and the reason for every non-obvious boundary.

2. **Make state transitions explicit.** Classify every value as server data,
   URL/form state, local interaction state, or derived state. Give each mutable
   value one owner, derive instead of duplicating, and model loading, success,
   error, retry, and empty transitions. The implementation is done when each
   transition has a visible result and a recovery action where one is possible.

3. **Consume the design source of truth.** Read the token section of `DESIGN.md`
   and its named token source. Use semantic color, type, spacing, radius, and
   elevation tokens; do not invent one-off values. If a needed token is absent,
   propose it in the handoff instead of silently adding a parallel scale.

4. **Make accessibility an acceptance criterion.** Use semantic landmarks and
   controls, labels, alt text (or an explicit decorative decision), and a
   logical heading order. Verify the primary path with keyboard only, visible
   focus, focus return after dialogs/menus, and no keyboard trap. Check text and
   control contrast in every theme and viewport. Honor
   `prefers-reduced-motion` by removing non-essential movement and preserving
   the state change.

5. **Set and measure performance budgets.** Unless the project defines stricter
   limits, target initial route JavaScript ≤200 KB gzip, critical CSS ≤30 KB
   gzip, LCP ≤2.5 s p75, INP ≤200 ms p75, and CLS ≤0.1. Ship responsive,
   dimensioned AVIF/WebP images; reserve image space, lazy-load below the fold,
   and preload only the LCP asset. Keep a hero image ≤250 KB when practical.
   Attach production-like measurements or document an intentional exception.

6. **Use motion as feedback, not decoration.** Animate opacity or transforms,
   avoid layout-thrashing properties, keep transitions brief, and never make
   motion the only signal for a state. Verify the reduced-motion path and the
   no-JavaScript/error path before handoff.

7. **Cover the concept brief's hero first.** Add a Playwright journey for the
   hero's heading, primary action, media treatment, and first meaningful state.
   Assert user-visible roles and state, exercise the primary action by keyboard,
   and cover loading/error/empty behavior when the hero depends on data. Build
   it from the assertions in
   [references/playwright-acceptance.md](references/playwright-acceptance.md). Wait for a
   user-visible condition, never a timeout or network idle. Screenshots never
   replace semantic checks.

8. **Write the handoff and critique record.** Link the brief, `DESIGN.md`, and
   token source. List the component map, state table, accessibility evidence,
   measured budgets, image decisions, motion/reduced-motion behavior, Playwright
   command and result, intentional deviations, and known gaps. End with the
   critique response: what changed, what remains, and who owns each follow-up.

## Playwright acceptance

Each contract the brief makes becomes one assertion from
[references/playwright-acceptance.md](references/playwright-acceptance.md) (named controls, ARIA
snapshot, keyboard path, reduced motion, forced colors, loading/error/empty, component states).
Read it at step 7.

When the task has a `demo_statement`, the traces, captures, and ledger are
evidence for [cas-qa-craft](../cas-qa-craft/SKILL.md). Follow its ledger and
labels; do not keep a second evidence format here. `cas-nuxt-playwright` covers
Nuxt harness setup, locks, and retries.

## Framework notes

- **Nuxt/Vue:** keep fetch/mutation ownership in a composable or page boundary;
  expose typed state, use `defineProps`/`defineEmits`, and prefer `computed`
  over watchers for derived values. Keep client-only behavior out of SSR output.
- **React:** keep remote data and mutations in the feature boundary, derive with
  render-time expressions or memoization only when measured, and make effect
  dependencies explicit. Preserve semantic HTML instead of wrapping every
  interaction in a custom component.

Done means the brief-to-implementation checklist is complete, the hero journey
passes, accessibility and performance evidence is attached, and critique gaps
are either fixed or named with an owner and next check.
