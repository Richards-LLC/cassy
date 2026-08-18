# Slack draft — model steering upgrades (2026-07-06)

Channel: #cas-internal (`C0B44GUKDK2`) — two top-level posts.

---

## Post 1 — User

Cassy used to treat every model the same way: heavy questions burned the expensive model's budget, cheap models got handed wording-sensitive work they write badly, and there was no honest way to get a second opinion from a different AI lab. Now Cassy routes work by what each model is actually good at — and taps the near-free Codex allowance for the heavy lifting.

- Big read-only investigations (digging through logs, giant specs, sweeping hundreds of files) now go to a one-shot Codex helper instead of eating your Claude budget — ask for an investigation and Cassy knows where to send it.
- Anything a human will read — docs, release notes, error messages, API surfaces — routes to a model with taste, even when the change looks trivially small.
- If a cheaper model's work doesn't meet the bar, Cassy escalates to a better one without being asked: judge the output, not the price tag.
- Code reviews on big changes can now include an independent second opinion from GPT-5.5 — a genuinely different reviewer, not another copy of the same one — and it clearly says when it couldn't run rather than pretending it found nothing.

## Post 2 — Dev

Model routing was tier-by-complexity only, token-heavy investigation had no cheap path, and reviews were single-lab. Three upgrades, all field-tested during their own development:

- New `cas-codex-exec` builtin skill: one-shot `codex exec -s read-only -m gpt-5.5` shell-outs for token-heavy read-only work, with verified CLI flags, self-contained-prompt conventions, an explicit "say so if you find nothing" rule, timeout/background patterns, and graceful fallback when codex is absent. Routing pointer lives in the supervisor's model-selection reference.
- Model-selection rubric gains cost/intelligence/taste axes with a glossary, a taste-routing rule (user-facing output never routes "light" just because the diff is small), judgment-based escalation (the two-rejection rule is a floor, not a permission gate), and a hard effort ceiling for Claude workers (`high` max — `xhigh`/`max` multiply per-step reasoning, not capability).
- cas-code-review Workflow gains an optional `gpt-5.5:independent` persona: a Sonnet-low wrapper composes a self-contained codex prompt embedding the literal diff, schema-validates the findings, and activates only on broad diffs (5+ files / 300+ lines) or explicit request. Codex-absent produces a distinguishable skipped envelope — never "reviewed clean". Activation/skip logic is exported as pure functions with executed boundary tests plus a drift check asserting the inline runtime copies match.
- Supervisor references also codify four coordination lessons: review findings become epic-child tasks before any worker message (messages aren't durable state for one-shot workers), a new "injected-but-unwoken" recovery mode with its exact prompt-queue diagnostic, the epic assembly loop (hold merge → review → fix-round-as-task → self-run gate capturing the real exit code), and lifecycle-notification verification steps.
