# Task depth: a speed lane for feel-driven work

Cassy tasks have a **depth**: `deep` (the default) or `light`. Depth controls the
speed-vs-rigor tradeoff at close time. It does not change what a task *is* — only
how much machine verification stands between "I'm done" and "closed".

If you never set it, every task is `deep` and nothing about your workflow
changes. `light` is opt-in.

## The two lanes

| | **deep** (default) | **light** |
|---|---|---|
| Who decides "it's correct"? | The machine — verifier + review gates | **You**, by looking at the result |
| Verification jail on close | Armed (task-verifier runs) | Skipped |
| P0 code-review / supervisor-review gate | Enforced | Skipped |
| Worker's pre-close self-checks | Run | Skipped |
| Close speed | Full gate (minutes) | Immediate |
| Good for | Logic, features, data, anything where correctness must be *proven* | Feel-driven UI iteration where *you are the evaluator* |

`deep` and unset behave identically (a task with no depth reads back as `deep`),
so existing tasks and existing habits are untouched.

## When to reach for `light`

Use `light` when **a human looking at the result is the real test** and a
machine gate would only slow the loop down:

- Nudging spacing, color, copy, animation timing — you'll *see* whether it's
  right on localhost faster than any check could tell you.
- Rapid "does this feel better?" passes where you're iterating by eye.

Stay on `deep` (do nothing) when correctness is not something you can eyeball:

- Business logic, auth, data migrations, API contracts, money.
- Anything where a regression would be invisible in a screenshot.

Rule of thumb: **if you can't verify it by looking at it, it's deep.**

## How to set it

Depth is a field on the task tool — there is no separate CLI command (tasks are
MCP-tool driven). Set `depth` when the task is created:

```
task action=create title="tighten card padding on the pricing page" depth=light
```

In an agent chat you can just say it in words and the agent passes it through:

> "Create a **light** task to nudge the hero spacing."

Valid values are `light` and `deep`; anything else is rejected. Omitting `depth`
means `deep`. You can see a task's depth in `task action=show` (the `Depth:`
line) and in `task action=mine`.

## The tradeoff — read this before using `light`

`light` **turns off machine verification**. There is no verifier, no review
gate, and no pre-close self-check standing behind a light close — *you* are the
only evaluator. That is exactly the point for feel-driven UI work, and exactly
why you must not use it for anything whose correctness you can't personally
confirm by looking at the running result. When in doubt, leave it `deep`.

## See also

- [`docs/notes/2026-06-25-task-depth-speed-mode.md`](../notes/2026-06-25-task-depth-speed-mode.md)
  — why this feature exists (the design record).
