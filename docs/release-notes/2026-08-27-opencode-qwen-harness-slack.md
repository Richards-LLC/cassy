# 2026-08-27 — OpenCode + Qwen worker harness — #cas-internal

> EMBARGO: do not post before 2026-08-31 (operator confirmation required).
> Draft complete; append the POSTED receipt table after publication.

## User thread

**Top-level (Live on production · User):**

🤖 Cassy can now run workers on a fourth AI backend: OpenCode driving Qwen —
so a QwenCloud subscription can power real factory work alongside Claude,
Codex, and Grok.

**Reply (Was → Now):**

Was: Cassy workers could only run on three backends (Claude, Codex, Grok),
each needing its own account, and there was no way to put a Qwen subscription
to work.

Now: spawning a worker on the OpenCode backend with Qwen 3.8 Max just works —
sign-in uses your QwenCloud Token Plan subscription key, and the whole setup
was proven live before being called supported: a Qwen-driven worker created a
real task, wrote code, committed, pushed, and closed the task through Cassy's
own tools, survived cancellations, respected permission denials, and kept two
accounts cleanly separated. Anything not yet proven this way (running Qwen
locally, or pay-per-token billing) is clearly marked unsupported and politely
refused instead of failing mysteriously halfway through. Guidance also states
how many Qwen workers your subscription tier can run at once, so the plan's
limits don't surprise you.

## Dev thread

**Top-level (Live on production · Dev):**

⚙️ `cli=opencode` is a supported worker harness: QwenCloud Token Plan route
(`qwencloud/qwen3.8-max`, `sk-sp-` key) validated by live conformance receipt
`opencode-1.18.23-hosted-token-plan-2026-08-27`; unreceipted routes are
refused before queue insertion (PR #596).

**Reply (Was → Now):**

Was: three harnesses (claude/codex/grok); no OpenCode selector, no Qwen
provider plumbing, no route concept for hosted-vs-local model serving.

Now: `SupervisorCli::OpenCode` with full launch adapter, spawn policy,
account-root layout, role/plugin projection (async plugin factory), mapped
CAS↔ses_* liveness/blame, and builtin skill projection. Model selectors carry
an explicit route: `qwencloud/qwen3.8-max` (Token Plan, default),
`alibaba/qwen3.8-max` (DashScope pay-as-you-go), `local/<model>` — no silent
cross-route fallback; key-prefix/lane mismatch guards (`sk-sp-` vs
`sk-`/`sk-ws-`); per-route effort tables (low/medium/xhigh for hosted Qwen)
with rejection, never remapping. The Token Plan route's support claim is
scoped to its typed, route-stamped conformance receipt — serial effort
matrix against the live endpoint, real disposable-repo CAS task lifecycle via
tool calls, session resume, deny-retained under --auto, Ctrl+C cancel
recovery, two-account isolation. Local and PAYG routes stay
pending-conformance and are refused pre-queue. Factory preflight probes the
OpenCode binary version; Claude/Codex/Grok parity suites unchanged and green.

## POSTED

(to be filled at publication — parent/reply permalinks + timestamps for both threads)
