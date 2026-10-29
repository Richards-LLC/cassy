# Brief: Commander shape and polish system (DESIGN.md, glass.css)

## Single idea

Commander reads as one product when every bounded thing takes its shape, card, pill and avatar from a single written rule, so the left nav, Tasks & progress, Team and Accounts stop looking like four different apps with square boxes.

## Hero form

A component system, not a screen: a six-step shape scale (inline 4 · control 12 · surface 16 · glass 22 · sheet 24 · pill) plus one card, three pill kinds and a provider-mark avatar, written once in `hub-web/DESIGN.md` and carried by tokens in `glass.css`. The scale's geometry is the claim: a nested shape is always one step smaller than its parent, so corners never collide and nothing is square.

## Emotional register

Calm and finished. Earned by soft 16px cards separated by fill and spacing instead of 1px outlines, pills whose height tells a fact (24px status) from an action (32px), and provider marks that make each worker's model recognisable at a glance.

## Distinctive move

The model is the face: every session and worker avatar is its provider's mark (Claude, OpenAI for Codex/Sol/Luna, Grok) at a legible 24px inside a 42px disc, with the live-state dot riding its edge.

## Deliberately omitted

No new palette, gradient or motion: Glass colours stay as shipped, and shape is the only system-wide change. No per-surface radius literals: the house `--radius-card` and `--radius-pane` are re-shaped once (12px and 16px), so every existing consumer inherits the scale without a markup change. No bold "not reported" verdicts: unknowns are evidence lines in `--ink-mid`.

## Delivery steps

1. DESIGN.md rules (Shapes, Cards and rows, Pills, Avatars, Header and action spacing, Unknown and empty states) and the shape/size tokens. Mergeable alone; every surface using the house corners rounds at once.
2. Shared component classes for card, status pill, action pill, machine chip and provider avatar, applied to Tasks & progress worker cards, Team lanes, project cards, Needs you, the conversation rows and the conversation header (project, model and version, machine, session, live state).
3. Accounts (grouped by machine then provider, location, usage bars with ET resets, visible actions or "Get manage access", stale-duplicate cleanup) and the left-nav specifics.

## Critique

Appended per step after render, with before/after pairs at 1280 and 390 in light and dark against the 3.88 screen catalog.

### Step 1 (rules and tokens)

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | One shape scale: worker cards, menus, project cards and fields lose their 4px/8px corners at once; provider-mark avatars and the header pattern are specified for step 2. |
| Fit | 5 | Uses Glass tokens only; no new colour. DESIGN.md frontmatter, maps and glass.css agree (tokens.test drift check). |
| Hierarchy | 4 | Status pill (24px) vs action pill (32px) vs chip are distinct by rule; unknowns are demoted to evidence lines. The surfaces adopt the classes in step 2. |
| Craft | 4 | Strict visual QA PASS, 70 fixtures x 2 schemes x 3 viewports, 0 findings; 27 affected journeys, 211 tests pass on the rebuilt bundle. |
| Accessibility | 4 | Shape only; contrast pairs unchanged; more-contrast and forced-colours edges keep --line-strong. |

Before/after: the 3.88 catalog pages 20, 21 and 77 against `step1/after/*` (1280 and 390, light and dark) in the task artifacts.
