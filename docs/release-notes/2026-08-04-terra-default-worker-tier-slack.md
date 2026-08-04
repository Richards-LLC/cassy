# Slack release notes — 2026-08-04 — default worker tier: Terra/high

Channel: #cas-internal (C0B44GUKDK2). Two threads, each top-level + one reply.

## User thread

**Top-level:**

Live on production — **User**

Your factory's everyday sessions now run on an engine tuned for routine work at full thinking depth, with the heavyweight model reserved for the genuinely hard problems.

**Reply:**

Was: every worker session used the flagship engine at a modest thinking setting, regardless of how hard the task was — premium rates for routine work, and the hardest problems still didn't get maximum depth.

Now: routine work runs on the mid-tier engine thinking at full depth, and the flagship engine is reserved for the heavyweight cases where it genuinely pulls ahead. On everyday coding benchmarks the two are 1–3 points apart; on the hardest frontier tasks the flagship leads by 12–20 points — so the default follows the small gap and the escalation path follows the big one.

- Nothing to configure. If you previously set an explicit model override, it is still honored.
- Asking "which model should this session use?" now actually surfaces the routing guide, with every valid model name listed.
- Takes effect on each machine as it picks up the update.

## Dev thread

**Top-level:**

Live on production — **Dev**

Stock worker default is now `gpt-5.6-terra` at `high` reasoning effort; `gpt-5.6-sol/high` is reserved for heavy/frontier routing.

**Reply:**

Was: the stock worker fallback resolved to `gpt-5.6-sol/medium`, every tier in the supervisor rubric routed to Sol, the model-selection reference listed exactly one valid Codex slug, and the skill's frontmatter never mentioned routing — so slug/model questions could not match it in discovery.

Now: the stock fallback chain resolves `gpt-5.6-terra/high`; tiers are Terra/low (light), Terra/high (standard + taste/judgment), Sol/high (heavy/frontier); the slug table enumerates `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna` with the supersession mapping (5.4→Terra, 5.4-mini→Luna) and invalid-slug warnings; and the frontmatter carries routing keywords — identically across all three harness skill variants, verified by grep parity. The code-review persona fleet deliberately stays on Sol/high as the judgment tier. Rust default lands with the next binary build; skill markdown syncs without a rebuild. Full sanitized serialized suite exited 0 on the shipped tree.
