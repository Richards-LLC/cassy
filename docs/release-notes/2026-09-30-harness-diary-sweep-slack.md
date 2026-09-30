# 2026-09-30 — Harness diary sweep — #cas-internal thread

Channel: `#cas-internal` (`C0B44GUKDK2`). Draft for the diary merge: one parent
plus three replies ordered Grok, Claude, Codex. Not posted; append `## POSTED`
only after publication returns the four message IDs and permalinks.

## Parent post

```text
*Dev — Harness diary sweep — 2026-09-30*
Was: recent harness changes lacked a Cassy compatibility assessment → Now: diaries cover installed Grok 1.0.44, Claude Code 2.1.285 and Codex 0.159.2, with approval, hook and MCP changes distinguished from live validation; the validated pins remain Grok 1.0.40, Claude 2.1.280 and Codex 0.156.0.
```

## Grok reply

```text
• *Coverage* — Was: Grok Build 1.0.41 was the newest observed install, with missing notes after 1.0.34 → Now: 1.0.42–1.0.44 have explicit source-gap entries, 1.0.41's gap is rechecked, and newly available local notes backfill 1.0.35–1.0.40.

• *Hooks, MCP and effort* — Was: changes in that backfill were unknown → Now: managed-only hook policy, API-sourced effort menus and instruction-file reads are compatibility watches; MCP handshake reporting and background-command visibility improve in the host. No required Cassy code change is established.

• *Validation* — Was: Grok 1.0.40 was validated → Now: that pin remains; 1.0.44 is installed but unvalidated, and the previously deferred memory, compaction and MCP audit gaps remain deferred.

Source gaps: 1.0.41–1.0.44; historical 1.0.14–1.0.16 and 1.0.26–1.0.29 remain. The 1.0.40 note names only generic fixes, so it adds no behavior evidence. The public xAI page still ends at 0.2.117.
```

## Claude reply

```text
• *Hooks and MCP* — Was: the diary stopped at Claude Code 2.1.280 → Now: 2.1.281–2.1.285 cover hook completion/cancellation, MCP startup/resume waits, progress delivery and deferred-tool behavior. These are host reliability gains; no required Cassy code change is established.

• *Unattended commands* — Was: bypass mode could still prompt on dangerous removals → Now: dangerous-rm prompts deny after two minutes by default, and background Bash/PowerShell commands have deadlines (30-minute default, two-hour maximum). Long-command behavior remains a compatibility watch; explicit bypass launch settings still override the new auto-mode default.

• *Validation* — Was: Claude 2.1.280 was validated → Now: that pin remains; the installed 2.1.285 has source review, with a fresh live matrix still needed.

Source gaps: none in 2.1.281–2.1.285.
```

## Codex reply

```text
• *Approval and interruption* — Was: the diary stopped at Codex 0.156.0 → Now: seven stables through 0.159.2 are covered, including 0.156.1 and 0.157.1. Elevated terminal-input approval, automatic background-server startup and opt-in instant_interrupt require a fresh Cassy compatibility check; no breakage is established by the notes alone.

• *MCP, hooks and models* — Was: recent client/env changes were unreviewed → Now: local stdio MCP needs no OAuth-client-secret change, while hook spawning, shell-env snapshots and MCP discovery remain live-validation watches. GPT-6.1 Sol is the catalog default; explicit Cassy model pins still take precedence.

• *Validation* — Was: Codex 0.156.0 was validated → Now: that pin remains; installed/latest stable 0.159.2 is source-reviewed, not newly validated. No launch flags or runtime code changed.

Source gaps: 0.157.1 has a stable release but upstream could not determine its highlights. All other reviewed stables have release notes; alpha releases remain outside the stable diary.
```
