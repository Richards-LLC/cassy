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

### Steps 2-3 (Accounts, left nav, header, avatars, cards)

| Screen | Distinctiveness | Fit | Hierarchy | Craft | Accessibility | Evidence |
| --- | --- | --- | --- | --- | --- | --- |
| Accounts | 5 | 5 | 4 | 4 | 4 | Machine → provider cards in headroom order; "Lives in ~/.claude-…" answers where; one usage-as-of per machine; cleanup banner; Set default + ⋯ menu; read-only shows the exact factory:manage command. |
| Left nav | 4 | 5 | 4 | 4 | 4 | Surface cards, pill machine chips, provider-mark avatars (24px mark on 42px), "Version unknown" as evidence. |
| Conversation header | 4 | 5 | 5 | 4 | 4 | One desktop row: mark, project, "Claude Opus 5.5 · high", machine and session in one typeface, actions right; wraps when the rail leaves under 20rem; phone keeps one avatar row. |
| Tasks & progress, Team | 4 | 5 | 4 | 4 | 4 | Worker cards and lanes on the card rule with lane-size marks; Activity is a 32px action pill. |

Strict visual QA PASS, 72 fixtures (Accounts and Accounts read-only added) x 2 schemes x 3 viewports; it caught Default/Recommended badges at 4.41:1 in dark and the phone header overflowing, both fixed. Affected journeys 27 / 211 PASS at 4d5af7f70 after HUB-J13 caught a 390px Accounts overflow. Before: the 3.88 catalog (pages 20, 21, 77, 96) and the operator's screenshots; after: `cas-652f/after/*` at 1280 and 390 in light and dark.
