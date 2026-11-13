---
metadata:
  managed_by: cas
---

# Locators and accessibility

Choose getByRole with its accessible name, then getByLabel, then an explicit stable getByTestId where semantics cannot identify the target. Accessible-name inputs locate a control; they are not a reason to pin marketing copy as output. Assert fixture-derived values and contract state with web-first expectations; toHaveText is valid for fixture data, not prohibited by matcher name.

Scope same-named controls to a landmark/dialog/row or use accessible description. Inspect the rendered DOM when strictness fails. Banned: CSS-first styling-class selectors and .first() as a strictness fix. An intentional ordered-list test may select an index only when order is the contract, with an assertion proving that order.

```ts
const order = page.getByRole('row').filter({ hasText: fixture.orderId });
await order.getByRole('button', { name: 'Open', exact: true }).click();
await expect(page.getByRole('dialog')).toBeVisible();
await expect(page.getByRole('dialog').getByTestId('order-id')).toHaveText(fixture.orderId);
```

## Quasar map

| Component | Rendered contract | Locator |
| --- | --- | --- |
| q-btn | button, or link with to/href | getByRole('button', { name }) or getByRole('link', { name }) after inspecting output |
| q-input | labelled input | getByLabel(label) or named textbox |
| q-select | combobox and teleported options | getByRole('combobox', { name }), then page.getByRole('option', { name }).visible() |
| q-dialog | semantic dialog, possibly teleported | page.getByRole('dialog', { name }); add accessible semantics or test id if absent |
| q-banner | status/alert when appropriate | getByRole('status'/'alert'); otherwise add explicit test id, never a framework styling class |
| hidden retained menus | several mounted copies | .visible() on the semantic locator when visibility is the actual distinction |

Teleported options/dialogs are not descendants of the triggering component; scope them from page or their semantic container, then assert the selected result back on the form.

## Structure and keyboard

Review toMatchAriaSnapshot for roles, landmark hierarchy and states. Keep fixture-derived names/values; omit incidental copy or use the snapshot pattern language to express the structural contract. Review generated baselines against the promise rather than blindly accepting updates.

Tab through the primary path and assert toBeFocused on meaningful stops. Escape closes a dialog/menu and returns focus to its trigger. Under reducedMotion: 'reduce', verify the action still reaches its state without unnecessary movement. Under forcedColors: 'active' and contrast: 'more', verify the focused control remains visible and usable; capture the focus ring at the actual viewport.

The one allowed CSS assertion category is a named accessibility-property contract. A computed-style assertion such as toHaveCSS may support effective focus visibility or reduced motion only when that property itself is the documented contract; it must not require arbitrary styling. Banned accessibility pins: exactly solid/auto outline-style, zero transition-duration, color/spacing tokens or a specific ring implementation when other implementations satisfy the contract. Generated symbols conveying meaning also need accessible semantics; a CSS content assertion alone cannot prove it.

Use loading/error/empty fixtures and assert the visible recovery action. Screenshots support a contractual visual baseline; they do not replace semantic outcome assertions. For stories/gallery component testing, use the built-in mount fixture from @playwright/test and assert through the mounted component's public controls; do not add retired experimental component-test packages or reuse a mutable page across independent journeys.

Sources: [locators](https://playwright.dev/docs/locators), [accessibility](https://playwright.dev/docs/accessibility-testing), [ARIA snapshots](https://playwright.dev/docs/aria-snapshots).
