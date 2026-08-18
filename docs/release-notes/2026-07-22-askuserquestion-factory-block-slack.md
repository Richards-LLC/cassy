# Slack draft — v2.28.3 AskUserQuestion factory block (#cas-internal C0B44GUKDK2)

## Post 1 — User thread

**Top-level:**
Live on production — **User**
Your factory can no longer freeze itself by asking you a question the wrong way. Before, an agent trying to ask you something could pop a permission prompt addressed to itself and stall everything until you noticed and rejected it; now it just asks you in plain chat.

**Threaded reply:**
Was → Now
- Was: an agent that wanted your input could invoke the interactive question dialog — which has nowhere to render in a factory session. The call turned into a confusing "permission request to itself", the session paused, and work stopped until a human manually rejected it. Worse, the agent would then often give up on asking and guess.
- Now: that dead-end path is blocked outright. The agent gets immediate instructions to ask you in plain text and end its turn, and your typed reply reaches it the normal way. Questions for you arrive as readable chat, never as a stuck dialog.
- Bonus: the built-in brainstorming and ideation guides used to actively recommend the broken path; they now teach the plain-text route in factory sessions.

## Post 2 — Dev thread

**Top-level:**
Live on production — **Dev**
`AskUserQuestion` is now hard-denied at PreToolUse for factory agents — and the intercept actually fires, because the tool was missing from every hook matcher (the previous advisory reminder was dead code).

**Threaded reply:**
Was → Now
- Was: the PreToolUse handler returned `allow` + advisory reminder for supervisor `AskUserQuestion`, and only for supervisors. Unreachable twice over: the handler path needed a resolved Cassy root, and `AskUserQuestion` wasn't in `default_pre_tool_use_matcher()` nor in the factory per-role settings matcher — so in real sessions the call fell through to agent-teams permission routing and surfaced as a self-directed permission prompt.
- Now: `deny` for both supervisor and worker roles, hoisted above Cassy-root resolution, with role-tailored guidance using the caller's own harness tool prefix (mcp__cas__ / mcp__cs__ / cas__). Both matcher paths include the tool via a new intercept-only list that stays out of `permissions.allow`, with regression tests pinning matcher↔handler sync in both directions.
- Skill text (supervisor hard rules, intake, brainstorm, ideate) updated across all three harness variants to teach plain-text-and-end-turn; the every-turn supervisor reminder wording is now asserted by tests, and the deny contract has coverage for the `agent_role` field path, env fallback, and `cas_root=None`.
- Deployment note: run `cas update` so regenerated harness settings pick up the new matcher.
