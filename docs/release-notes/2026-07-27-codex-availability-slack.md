# v2.32.0 — Codex availability probe + one fallback policy (Slack drafts)

Channel: #cas-internal (C0B44GUKDK2). Two top-level posts, per `docs/RELEASE_SLACK_RUBRIC.md`.

---

## Post 1 — USER

**Live on production — v2.32.0**

On a machine where Codex wasn't installed or signed in, starting work just failed with a raw error — after setup had already run. Now it detects that up front, tells you plainly, and carries on with the assistant you do have.

**Was → Now**

- **Was:** if the Codex tool wasn't installed, or was installed but never signed in, work would fail at the moment it tried to start — long after the setup around it had already been done. The error was whatever the operating system said, which didn't tell you what was missing or how to fix it. **Now:** availability is checked before anything is committed to, and if Codex isn't usable the work continues on the other assistant with a clear one-line notice naming what was missing.
- **Was:** if you'd configured individual slots to use Codex, those choices skipped the existing safety check entirely and failed anyway. **Now:** every route is covered, including work started mid-session rather than at launch.
- **Was:** the coordinator itself could be configured to use Codex and hit exactly the same failure — which takes down the whole session, not one lane. **Now:** it's covered too, and its notice says clearly that it's the coordinator falling back, since that changes how the whole session runs.
- **Was:** if you'd rather it stop than quietly continue with a different assistant, there was no way to say so. **Now:** there's a setting for exactly that — refuse to substitute and fail with a clear message instead.
- **Was:** in one situation the system would silently switch you the *other* way, onto Codex, without checking whether you were signed in — so it could hand you something that couldn't actually run. **Now:** substitution only ever happens in the one direction that's been checked, and a missing primary assistant is always reported as a setup problem rather than worked around.

---

## Post 2 — DEV

**Live on production — v2.32.0**

Codex became the default worker harness, but the install-fallback only triggered when the resolved CLI was literally `claude` — so a host without codex hard-errored at spawn, after worktree creation and registration prep had already run on the wrong spec. Availability is now probed post-cascade at spec resolution, with a single fallback policy replacing two contradictory ones.

**Was → Now**

- **Was:** the safety net keyed on the resolved CLI being `claude`, which stopped matching when the worker default flipped to codex. Failure surfaced at `Pty::spawn` — far too late to substitute cleanly, since upstream setup had already committed to the spec. **Now:** a bounded probe (`codex --version` via spawn + poll with a kill on timeout, never a blocking wait, so a hung binary can't stall launch) plus an `~/.codex/auth.json` check runs post-cascade in the spec resolver. Login-flow only — `OPENAI_API_KEY` is deliberately not accepted, so the decision stays deterministic.
- **Was:** per-slot `[[factory.workers]]` / `--worker-spec` entries setting `cli=codex` bypassed the top-level preflight entirely, and the mid-session spawn path had no preflight at all — which is the route the original failure actually took. **Now:** the check is applied at both entry points, covering launch-time and mid-session spawns, and the resolver function itself stays pure so tests can't become dependent on the host's real install state.
- **Was:** the coordinator spec could be set to codex and hit the identical failure, with session-wide rather than per-lane blast radius. **Now:** covered, with a notice deliberately distinct from the per-worker wording because substituting the coordinator changes the harness for the whole session.
- **Was:** no way to opt out of substitution. **Now:** `--strict-cli` or `[factory] strict_cli` refuses to substitute and returns an actionable error naming the missing component; both sources are OR'd, default off.
- **Was:** two fallback mechanisms with contradictory policies, the older one running first. It swapped symmetrically on binary presence alone with no auth check, so a missing primary CLI could silently route you onto an unauthenticated Codex — while the documented decision says that case must be a hard error. **Now:** one policy. Substitution is one-directional, the reverse arm is removed with regression tests pinning it, and "available" means binary *and* auth everywhere.

Full workspace suite green on the release commit: 5417 passed, 0 failed.
