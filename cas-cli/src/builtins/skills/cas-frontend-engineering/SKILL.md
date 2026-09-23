---
name: cas-frontend-engineering
description: Use when turning an approved concept brief into accessible, performant frontend implementation with explicit component, state, token, motion, and Playwright acceptance.
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
   it from the assertions in "Playwright acceptance" below. Wait for a
   user-visible condition, never a timeout or network idle. Screenshots never
   replace semantic checks.

8. **Write the handoff and critique record.** Link the brief, `DESIGN.md`, and
   token source. List the component map, state table, accessibility evidence,
   measured budgets, image decisions, motion/reduced-motion behavior, Playwright
   command and result, intentional deviations, and known gaps. End with the
   critique response: what changed, what remains, and who owns each follow-up.

## Playwright acceptance

Target `@playwright/test` 1.63; check with `npx playwright --version`. Each
contract the brief makes becomes one assertion below. Record the spec path,
command, and result in the handoff.

| Contract | Assertion |
|---|---|
| Controls are named | `page.getByRole('button', { name: 'Start trial' })`. For icon-only controls, add `toHaveAccessibleName()`. Use `getByRole(role, { name, description })` to tell same-named controls apart. Use `getByTestId` only when no role or label exists; never use CSS classes or incidental copy. |
| Structure and heading order | `await expect(page.getByRole('main')).toMatchAriaSnapshot(...)` pins landmarks, heading levels, and the hero's controls. Generate the first version with `--update-snapshots` and review its diff like code. |
| Keyboard path | Drive the path with `page.keyboard.press('Tab')`, then assert `toBeFocused()` on each stop. Close dialogs and menus with `Escape`, then assert `toBeFocused()` on the trigger. |
| Reduced motion | Under a `reducedMotion: 'reduce'` project, the state change still shows (`toBeVisible()`, `toHaveAttribute('aria-expanded', 'true')`) and the animated element shows the reduced path, e.g. `toHaveCSS('transition-duration', '0s')`. |
| Forced colors and high contrast | Under `forcedColors: 'active'` and `contrast: 'more'` projects, the primary action stays visible. The focused control keeps an outline: `toHaveCSS('outline-style', 'solid')`, or `'auto'` for the browser's default ring; a box-shadow ring disappears in forced colors. |
| Pseudo-element decoration | Check required markers and generated icons with `toHaveCSS('content', '"*"', { pseudo: 'after' })`. Content that carries meaning must also be in the accessible name. |
| Loading, error, empty | Force each state with `page.route('**/api/…', (r) => r.fulfill({ status: 500 }))` or `r.fulfill({ json: [] })`. Assert its `getByRole('alert')` or `status` message and the recovery action. |
| Visual baseline (only if the brief makes it contractual) | `await expect(page).toHaveScreenshot('hero-dark.webp')`: a `.webp` name stores a WebP baseline. Set `colorScheme` per theme the brief names. |
| Component states | Use the stories/gallery `mount` fixture: `const card = await mount('components/PricingCard/Annual', { plan })`. Scope queries to `card`, call `card.update(props)` for transitions, and run it in a project with `use: { reuseContext: true }`. Do not add `@playwright/experimental-ct-*` packages. |

Run the accessibility modes as tagged projects so the default run stays fast.
Tag the relevant tests with `{ tag: '@a11y' }`:

```ts
const a11y = { grep: /@a11y/, use: { ...devices['Desktop Chrome'] } };
projects: [
  { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
  { ...a11y, name: 'reduced-motion', use: { ...a11y.use, reducedMotion: 'reduce' } },
  { ...a11y, name: 'forced-colors', use: { ...a11y.use, forcedColors: 'active' } },
  { ...a11y, name: 'more-contrast', use: { ...a11y.use, contrast: 'more' } },
],
```

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
