# 2026-10-17 — Harness diary parity audit — #cas-internal thread

Channel: `#cas-internal` (`C0B44GUKDK2`). Draft for the diary merge (named `-thread.md`
because the release-notes gate lints every `*-slack.md` as a four-body runtime release draft): one parent
plus three replies ordered Grok, Claude, Codex. Diary-only: no runtime release is
claimed. Not posted; append `## POSTED` only after publication returns the four
message IDs and permalinks.

## Parent post

```text
*Dev — Harness diary parity audit — 2026-10-17*
Was: the diaries stopped at Grok 1.0.44, Claude Code 2.1.285 and Codex 0.159.2, and Cassy still treated Codex as having no hooks and no agents → Now: the installed Grok 1.0.62, Claude Code 2.1.302 and Codex 0.163.0 are recorded with a Claude↔Codex parity check. Codex has stable lifecycle hooks, custom agents and a session queue that Cassy does not use yet; the validated pins stay Grok 1.0.40, Claude 2.1.280 and Codex 0.156.0.
```

## Grok reply

```text
• *Installed* — Was: 1.0.44 was the newest Grok on the host → Now: 1.0.62 is installed. Hooks, subagents and the wrap-up skill are unchanged, and no Cassy change is needed.

• *Validation* — Was: 1.0.40 was validated → Now: that pin stands; 1.0.41–1.0.62 are unvalidated, and the earlier deferral of the memory, compaction and MCP audit gaps stays in place.

Source gaps: 1.0.45–1.0.62 have no attributable release notes (the local changelog still ends at 1.0.40).
```

## Claude reply

```text
• *Reference side* — Was: unchanged since 2.1.285 → Now: 2.1.302 is installed. Cassy wires Claude's startup, stop, prompt, compaction and tool hooks, and its MCP write guards run as hooks. Claude remains the fully wired reference.

• *Validation* — Was: 2.1.280 was validated → Now: that pin stands; 2.1.302 has a wiring review, not a new live matrix.

Source gaps: 2.1.286–2.1.302 release notes are not yet reviewed.
```

## Codex reply

```text
• *Hooks* — Was: Cassy guidance said Codex has no session hooks → Now: Codex 0.163.0 ships stable lifecycle hooks (startup, stop with continuation, prompt submit, compaction). Cassy uses only its shell tool hooks, which do run and do block. Wiring them and correcting the stale guidance is planned for 3.64. Whether any Codex hook covers MCP tool calls is not yet measured.

• *Agents and messaging* — Was: Cassy treats Codex as having no agent catalog → Now: Codex supports custom agents defined in TOML and a `codex queue` command that adds a message to a running session. Cassy uses neither yet. Native Codex roles and queue delivery are 3.64 work; neither is established as the cause of a missed wake.

• *Finish and validation* — Was: on Codex the wrap-up was only a custom prompt, a feature Codex has deprecated → Now: it also runs as the supported `$finish` skill. The 0.156.0 pin stands; 0.163.0 has an installed-capability review, not a live matrix.

Source gaps: release notes for 0.159.3–0.163.0 (seven stables) are not yet reviewed.
```
