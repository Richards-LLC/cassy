# Playwright acceptance

Target `@playwright/test` 1.63+ (the stories model); check with `npx playwright --version`. Each
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
