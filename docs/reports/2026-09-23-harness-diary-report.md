# Harness diary report — 2026-09-23: 52 diary items Cassy never addressed

**Verdict:** across the full history of the three harness diaries, **52 of 146 audited items
(36%) were never addressed**. That means no task, no commit, and no recorded check. Another 32 are
in flight, almost all owned by two open conformance runs (Codex `cas-0d4f`, Grok `cas-ef93`).
Three of the gaps can fail a factory silently:

- a Grok worker that starts with the `cas` MCP server disabled;
- no minimum Claude Code version on factory hosts;
- no proof that a message reaches a busy Grok worker mid-turn.

This sweep (Claude Code 2.1.246→2.1.280, Codex 0.150.0→0.156.0, Grok 1.0.6→1.0.40) added no new
code work. It widened the gap between the validated pin and the installed version for Codex
(0.149.1 → 0.156.0) and Grok (1.0.5 → 1.0.40).

| Field | Value |
| --- | --- |
| Question | Which diary items did Cassy never act on, and what changed in the 2026-09-23 sweep? |
| Scope | `docs/notes/{claude-code,codex,grok}-changelog-diary.md`, full history (Entries + Backlog) |
| Items audited | 146 rows: every 👀 bullet, every 🏗 EPIC, every item naming a Cassy task |
| Commit examined | `4929bf38` (epic branch `epic/epic-2026-09-23-harness-diary-sweep-claude-codex-g-cas-6f39`, Grok sweep merged) |
| Confidence | High for classifications (each cites a task, commit, file:line, or a zero-hit search); impact ranks are judgement |
| Date / author | 2026-09-23 · factory worker for task `cas-df20` (report only — no tasks filed, no code changed) |

## Never addressed — the ones that matter

Impact is ranked by what breaks if the item bites. **High** means a factory can fail silently.
**Medium** means a worker can stall or lose instructions, or a strategic decision is owed. The
full list of 52, including the low-impact items, follows this table.

| Impact | Harness | Version | Item | Why it matters to Cassy | Evidence checked |
| --- | --- | --- | --- | --- | --- |
| High | Grok | 0.2.113, 1.0.19 | Nothing reports the health of Grok's MCP discovery. Operators can disable a server, and an org policy can block `cas` | A Grok worker can start with zero `cas__` tools and nothing flags it | `cas factory doctor` CAS-MCP row is Codex-only (`cas-cli/src/cli/factory/doctor.rs:179`); preflight checks only the project `.mcp.json` `cas` entry and live observation from the caller (`cas-cli/src/factory_preflight.rs:609-660,1236-1244`), not Grok's enable/disable or org-policy state; `cas-ef93` validates discovery once and does not add a standing probe |
| High | Claude Code | backlog | No Claude Code version floor for factory hosts (`requiredMinimumVersion`) | Every "verify on upgrade" watch assumes hosts run a known-good version | `grep -rn requiredMinimumVersion cas-cli/src crates` = 0; `git log --all -i --grep=requiredMinimumVersion` = 0; no task |
| High | Grok | 0.2.100, 0.2.101 | Messages entered while a turn is running: queue and Enter behavior changed | Supervisor redirects to a busy Grok worker may never land | The 1.0.5 receipt injects only into an idle worker; the Grok busy-turn trial in the `cas-5c02` communication probe was BLOCKED, and its report `db27ec44` is not an ancestor of HEAD (`git merge-base --is-ancestor` exit 1) |
| Medium | Claude Code | 2.1.233, 2.1.228–229 | Verify after upgrades that Cassy's skill mirror still wins over user and cloud-synced skills | Workers silently lose required instructions if a mirror is shadowed | `git log --all -i --grep="skill precedence"` = 0; no post-upgrade check recorded; no task |
| Medium | Claude Code | 2.1.183 | Verify tmux teammate pane launch and the spawn keystroke-leak fix on upgrade | Touches the factory worker spawn path | No verification recorded; `git log` keystroke/pane-launch hits are unrelated TUI work (`c5c0a30d`, `420b943c`) |
| Medium | Claude Code | 2.1.212 | MCP calls longer than two minutes auto-background (`CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS`) | Long `mcp__cas__` close/search calls could return detached | `grep -rn MCP_AUTO_BACKGROUND` = 0; `git log --grep` = 0; no task |
| Medium | Claude Code | 2.1.163 + backlog | Spike: Stop/SubagentStop `additionalContext` as the session-learn and guidance channel | A cleaner hook channel than blocking Stop; explicitly deferred "before the next hook-surface EPIC" | `HookSpecificOutput` has no Stop/SubagentStop variant (`crates/cas-core/src/hooks/types.rs:397-470`); no task filed |
| Medium | Codex | 0.137.0–0.142.x + backlog | No decision on Cassy's stance toward Codex-native multi-agent v2 orchestration | Native orchestration keeps growing (0.149 agents dashboard) and overlaps the factory | `git log --all -i --grep='multi-agent v2'` = 0; task search returned no results |
| Medium | Codex | 0.130–0.144 | Smoke-test multi-argument `cs` tool schemas against compaction, `oneOf`/`allOf`, `$ref` | Large Cassy tool schemas could be mangled on the Codex path | Receipts call only `coordination whoami` and `task mine` (`crates/cas-pty/conformance/codex-cli-0.149.1-2026-08-25.json:49,56`); no schema-fidelity test |
| Medium | Grok | 0.2.105 | Rules and identity surviving a real long-session compaction | Long factory tasks compact; a worker that loses its role derails | `grep -i compact crates/cas-mux/tests/grok_factory_contract_runtime.rs` = 0; not in `cas-ef93` acceptance criteria |
| Medium | Grok | 1.0.8 | MCP servers can request form or URL consent through a popup | A new interactive path that can stall an unattended worker | The diary says "the matrix must ensure" this, but `cas-ef93`'s description and criteria omit it |
| Medium | Grok | 0.2.104 | Background-task counts moved to the status line, out of the transcript | Long background work can look idle and trigger a false wedge | The receipt covers only a 22-second turn; no long-background liveness test |
| Medium | Grok | 1.0.34 | Grok memory is generally available | Grok memory can inject context that competes with Cassy's rules | Not in `cas-ef93` scope; `git log --all -i --grep='grok.*memory'` = 0 |

### All never-addressed items

One row per diary item, grouped by harness, newest first within each. The two Claude Code rows
marked *duplicate* restate an earlier row and are counted once in the 50 distinct concerns.

| Harness | Version | Diary line | Item | Why it matters to Cassy | Evidence checked |
| --- | --- | --- | --- | --- | --- |
| Claude Code | 2.1.239 | L482-487 | 👀 BOM skills no longer ignored; "continue checking mirror generation on upgrades" | Synced skill mirrors must load in workers | No upgrade check recorded; BOM commits (11e573cc, 11cf4f20) cover code index/CHANGELOG only; BOM grep in sync code = 0 |
| Claude Code | 2.1.237 | L514-519 | 👀 watch: deleted-cwd recovery, skill hot-reload, runner hooks | Stale sessions/skills after worktree removal | `git log --grep "deleted (cwd\|working dir)"` = 0; no task; watch has no follow-up |
| Claude Code | 2.1.233 | L586-590 | 👀 skill aliases survive shadowing; "verify mirror precedence on upgrades" | Cassy skill mirrors must win over user/cloud skills | "skill precedence" git/grep = 0; CAS search found no verification task |
| Claude Code | 2.1.231/229 | L619-626 | 👀 retain MCP startup watch (OAuth redirect, managed-MCP remote startup) | Workspaces mixing `cs` stdio with remote MCP | No task/commit; `git log --grep "remote MCP\|mcp oauth"` hits unrelated (8983abdd neon .mcp.json portability) |
| Claude Code | 2.1.228/229 | L631-634 | *duplicate* — 👀 watch skill precedence vs cloud-synced skills after upgrade | Required worker instructions could be silently displaced | Same searches as L586 = 0; no recorded post-upgrade check |
| Claude Code | 2.1.212 | L779 | 👀 MCP calls >2 min auto-background; inspect `CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS` | Long `mcp__cas__` search/close calls may detach | grep/git-log MCP_AUTO_BACKGROUND = 0; no task |
| Claude Code | 2.1.211 | L810 | 👀 `--forward-subagent-text` for headless verifier diagnostics | Verifier transcript diagnosability | grep/git-log forward-subagent-text = 0 |
| Claude Code | 2.1.206 | L903 | 👀 `EnterWorktree` confirms outside `.claude/worktrees/`; Cassy uses `.cas/worktrees` | Agent entering Cassy worktree hangs on confirm | EnterWorktree not in `--disallowedTools` (`crates/cas-pty/src/pty.rs:1253` only AskUserQuestion); no PreToolUse rule; grep = 1 non-goal doc line |
| Claude Code | 2.1.196 | L1114-1117 | 👀 streaming idle watchdog default-on (5 min) | Long silent tool turns may abort/retry | STREAM_WATCHDOG grep/git-log = 0; no task |
| Claude Code | 2.1.187 | L1160-1163 | 👀 structured-output hardening benefits `cas-code-review` schema dispatch | Review pipeline reliability | No follow-up/verification recorded; moot since the Workflow was deleted (cas-7216, 9cf31b2a; `.claude/workflows/` has only `fixtures`) but the diary never records that |
| Claude Code | 2.1.183 | L1176-1181 | 👀 touchpoint: tmux pane launch + keystroke leak; "verify on upgrade" | Factory worker spawn reliability | No upgrade verification recorded; git-log keystroke/rc-file hits unrelated (c5c0a30d, 420b943c are Cassy TUI) ; CAS search no task |
| Claude Code | 2.1.169 | L1277-1281 | 👀 `--safe-mode` as first triage step in troubleshooting/onboarding doc | Fast "Cassy vs harness" isolation | safe-mode grep/git-log = 0; CAS search no task/doc |
| Claude Code | 2.1.169 | L1282-1285 | 👀 `disableBundledSkills` namespace hygiene option | Skill menu clutter/slash collisions | disableBundledSkills grep/git-log = 0 |
| Claude Code | 2.1.163 | L1328-1333 | 👀 Stop/SubagentStop `additionalContext` for session-learn/supervisor guidance spike | Cleaner Stop-hook guidance channel | "No task filed yet" (L1332-1333) still true; `HookSpecificOutput` has no Stop/SubagentStop variant (`crates/cas-core/src/hooks/types.rs:397-468`); last Stop change baa540bc (2026-04-13) predates 2.1.163 |
| Claude Code | backlog | L1399-1400 | *duplicate* — session-learn/guidance via Stop-hook `additionalContext` | Stop-hook channel | Duplicate of L1328 row; same evidence |
| Claude Code | backlog | L1401-1402 | Factory CC version floor via `requiredMinimumVersion` | Pins factory hosts to known-good CC | requiredMinimumVersion grep/git-log = 0; no min-CC-version check in `cas-cli/src`/`crates` |
| Codex | 0.146.0 / 0.139.0 / 0.143.0 | L371–375, L602–605, L509 | Proxy routing / proxy-only networking under host proxy policy | A host proxy policy could block `cs` startup or worker network under `--yolo` | Receipts run with `danger-full-access`, no proxy config; `grep -rn -i -E 'proxy.only' crates cas-cli/src` hits=0; `git log --all -i -E --grep='proxy.only'` hits=0 |
| Codex | 0.145.0 | L395–400 | `/import` of Claude Code/Cursor config could shadow `cs` registration/skills | Imported config could replace Cassy MCP/skill wiring | No import-scenario check in R146/R149 checklists; `git log --all -i -E --grep='codex.*import'` hits=0; task search "codex /import onboarding" → no relevant task |
| Codex | 0.145.0 / 0.144.0 | L411–415 (Windows proxy), L459–461 | Windows sandbox writable-root/proxy enforcement changes | Would affect only Windows Codex workers | Both receipts are Linux runs (rollout paths under /home/pippenz); `git log --all -i -E --grep='codex.*windows'` → only 08a710fc (unrelated stall windows) |
| Codex | 0.144.0 | L446–450 | MCP auth elicitation default: smoke `cs`; watch hang when a second MCP server needs auth | Mixed-server configs could stall worker MCP startup | cs: R149 `mcp_cs_root_turn`. Second-server scenario: absent from both receipt checklists; no task found |
| Codex | 0.144.0 / 0.139.0 / 0.130–0.135 | L462–465, L606–609, L636–641 | Schema compaction threshold, `oneOf`/`allOf`, `$ref`/`$defs`: smoke multi-arg `cs` tools | Large multi-arg `cs` tool schemas could be mangled | Receipts exercise only `coordination whoami` and `task mine` (R149 checks 5–6 detail); no schema-fidelity test: `git log --all -i -E --grep='oneOf\|allOf\|schema compaction'` hits=0 for codex |
| Codex | 0.142.x | L531–534 | If budgets default on: raise via `-c` or surface "turn aborted: budget" distinctly from stall | Budget abort would look like a silent stall to the factory | Only test-side detection exists (`codex_factory_contract_runtime.rs:431`); `grep -rn -i 'rollout token budget' crates cas-cli/src --include=*.rs` (non-test) hits=0; conditional trigger not yet met |
| Codex | 0.142.x / 0.141.0 / 0.139.0 / 0.138.0 / 0.137.0 + Backlog | L546–550, L610–612, L670–673, L693–694, L725–726 | Multi-agent v2 strategic posture: decide Cassy stance toward Codex-native orchestration | Strategic overlap/competition with Cassy factory orchestration | `git log --all -i --grep='multi-agent v2'` hits=0; code grep `multi_agent` hits=0; task search "codex native subagents multi-agent stance" → No results |
| Codex | 0.141.0 | L570–572 | TUI input prompts auto-resolve after inactivity; could auto-answer worker prompt | A worker hang/auto-answer on an input dialog | `grep -rn -i request_user_input crates cas-cli/src` hits=0; `git log --all -i --grep=request_user_input` hits=0; no receipt check |
| Codex | 0.140.0 | L579–583 | `/import` from Claude Code: flag for onboarding-doc pass | Cross-harness onboarding story | `grep -rln -E 'codex.*/import\|/import.*codex' docs cas-cli/docs` → only a CSV data file; no task found |
| Codex | 0.140.0 | L584–589 | Codex SQLite auto-recover; same hazard class as cas.db restore breaking MCP | Informational hazard parallel | Diary itself says "no overlap expected"; no follow-up task/commit (`git log --all -i --grep='codex.*sqlite'` hits=0) |
| Codex | 0.138.0 + Backlog | L658–663, L723–724 | If Codex ships first-class `--effort`, switch from `-c model_reasoning_effort` | Cleaner, version-stable effort passing | `codex --help` on 0.156.0: grep `effort` hits=0 (only `-p, --profile`); still uses `-c` at `crates/cas-pty/src/pty.rs:1379`; R149 confirms key still works |
| Codex | 0.130–0.135 | L646–649 | Codex git helpers ignore repo hook/fsmonitor config in worktrees | Interaction with factory commit guard | Informational ("no conflict expected"); no receipt check covers commits; `git log --all -i --grep=fsmonitor` hits=0 |
| Codex | 0.136.0 | L714–715 | Codex "memories" root moved; confirm no collision with Cassy memory via MCP | Naming collision with Cassy memory tools | No task/commit (`git log --all -i -E --grep='codex.*memor'` hits=0); no receipt check |
| Grok | 1.0.34 | L212 | Memory GA; verify rules/env/cas__ discovery with memory enabled | Grok memory could inject stale context competing with Cassy rules | not in cas-ef93 AC/notes (scope gap); `git log --all -i --grep='grok.*memory'` hits=0 |
| Grok | 1.0.33 | L225 | Structured JSON MCP results; cancelled MCP calls stop | Could change how cas__ results are parsed/receipted | not in cas-ef93 AC (scope gap); `git log --all -i --grep='structured.*mcp\|structuredContent'` → only 6ad75a2f (cas-remember, unrelated to Grok) |
| Grok | 1.0.30 | L270 | Session timing/workflow status/tmux lag | Diagnostic only; liveness uses transcripts | diary: no action indicated; no task (cas-ef93 AC silent); low |
| Grok | 1.0.22 | L321 | Subagent continuation/background completion/monitor wakeups | Grok subagents are not Cassy workers | no follow-up; cas-ef93 AC omits Grok-owned subagents; low |
| Grok | 1.0.19 | L356 | Org-blocked MCP refused; MCP connects in background after login | Blocked cas server must fail preflight, not look like missing tools | `cas factory doctor` CAS-MCP row is Codex-only (`cas-cli/src/cli/factory/doctor.rs:179`); preflight checks only the project `.mcp.json` `cas` entry and caller-side live observation (`cas-cli/src/factory_preflight.rs:609-660,1236-1244`), not Grok's own enable/disable or org-policy state |
| Grok | 1.0.18 | L378 | Auth recovery; subagents inherit model retry settings | Subagent/model separation | no follow-up/task; low |
| Grok | 1.0.8 | L491 | MCP servers can request form/URL consent via popup | New interactive path could stall unattended worker | diary says "the matrix must ensure", but cas-ef93 description/AC/notes omit it (scope gap); no task |
| Grok | 1.0.6 | L536 | Queued messages during goals | Factory message queue UX | no follow-up; low |
| Grok | 0.2.115–0.2.117 | L625 | GROK_EXTRA_CA_BUNDLE TLS roots; background subagents stopped | Transport env must not hide identity/MCP config | `git grep GROK_EXTRA_CA_BUNDLE` hits=0; receipt ran without it; low |
| Grok | 0.2.113 | L670 | MCP enable/disable CLI; preflight must report discovery health | Operator can disable `cas` → worker with zero cas tools | `cas factory doctor` CAS-MCP row is Codex-only (`cas-cli/src/cli/factory/doctor.rs:179`); preflight checks only the project `.mcp.json` `cas` entry and caller-side live observation (`cas-cli/src/factory_preflight.rs:609-660,1236-1244`), not Grok's own enable/disable or org-policy state; cas search "grok MCP discovery preflight" → no open task |
| Grok | 0.2.112 | L687 | Version policy: hard startup requirements | Distinguish Grok version gate from Cassy lifecycle failure | preflight only compares receipt vs default version (factory_preflight.rs:877-935); no hard-gate detection; low |
| Grok | 0.2.112 | L693 | Provider env headers + shell-variable allowlist | Could strip CAS_* from tool subprocesses | receipt ran default config only; no allowlist test/task; low |
| Grok | 0.2.112 | L714 | Plugin subagents inherit parent MCP tools | Grok subagents need cas__ too | no Grok-subagent MCP test; `git log -i --grep='grok.*subagent'` → only cas-8888 phase commits; low |
| Grok | 0.2.112 | L723 | Workflow overlays/resume | Not Cassy authority | no follow-up; low |
| Grok | 0.2.112 | L733 | Queued prompt edit; repeated identical calls stop silently | Factory-message visibility | no follow-up; low |
| Grok | 0.2.105 | L816 | Background tasks/fleet roster fixes | Not Cassy roster | no follow-up; low |
| Grok | 0.2.105 | L821 | Long-session compaction fix; verify rules/identity survive real compaction | Long factory workers compact; losing role would derail | no Grok compaction test (`grep -i compact grok_factory_contract_runtime.rs` = 0; `git log --all -i --grep=compaction` grok hits=0); not in cas-ef93 AC |
| Grok | 0.2.104 | L834 | Background counts move to status line, not transcript | Long background work may look idle → false wedged | receipt covers a 22 s turn only; no long-background liveness test; cas-921f min-of-ages is partial mitigation |
| Grok | 0.2.104 | L839 | Idle-session auth recovery | Worker longevity | `cas-cli/src/factory_auth_health.rs` has Grok only as label (:197); no follow-up; low |
| Grok | 0.2.101 | L879 | Queued Enter messages appear immediately | Supervisor→worker injected turns | cas-5c02 comm probe (db27ec44): Grok busy-urgent trial BLOCKED, W2S deliver FAIL; report only on epic cas-04a6 branch (`git merge-base --is-ancestor db27ec44 HEAD` → not-in-HEAD) |
| Grok | 0.2.100 | L897 | Cross-harness session picker (note for onboarding docs) | Host UX only | no docs mention (grep docs for grok+picker/resume = 0); low |
| Grok | 0.2.100 | L905 | Queue/Enter during running turn; smoke after big bumps | Mid-turn coordination delivery | same as L879; no "message during running turn" smoke in 1.0.5 receipt (inject only to idle worker, test :550) |
| Grok | 0.2.100 | L911 | No crash printing resume hint after pane closed | Could look like worker death on shutdown | `git log --all -i --grep='resume hint'` = 0, `'pane.closed'` = 0; low |

## What the 2026-09-23 sweep changed

| Harness | Range reviewed | Versions | Items verdicted | Verdicts | Touches Cassy | Source gaps |
| --- | --- | ---: | ---: | --- | --- | --- |
| Claude Code | 2.1.246 → 2.1.280 | 35 | 69 | 31 🟢 · 11 ✅ · 27 ⏭ · 0 👀 | Model defaults (Opus 5.5, Fable 5.1), MCP, hooks, subagent/message reliability — all 🟢 or ✅ | 8 versions absent from the official changelog; 3 generic "bug fixes" rollups |
| Codex | 0.150.0 → 0.156.0 | 7 | 14 | 8 👀 · 6 ⏭ | MCP (`cs`), `--yolo`/sandbox, AGENTS.md trust, skills/plugins, interrupt/resume | None |
| Grok | 1.0.6 → 1.0.40 | 35 | 55 | 27 👀 · 11 🟢 · 1 ✅ · 16 ⏭ | Esc no longer cancels (1.0.24), MCP input/consent, permissions, worktrees, sessions | 13 versions (1.0.14–16, 1.0.26–29, 1.0.35–40) with no per-version notes |

Verdict counts are the first verdict glyph on each entry bullet added by the sweep. Grok's two
single-line entries (1.0.20, 1.0.23) count as one ⏭ each. Source: `git diff main...4929bf38 --
docs/notes/`.

### Validated pin vs installed

| Harness | Validated pin | Installed (2026-09-23) | Gap | Owner |
| --- | --- | --- | --- | --- |
| Claude Code | no pin concept; host tracks latest | 2.1.280 | none | — |
| Codex | 0.149.1 (`crates/cas-pty/conformance/codex-cli-0.149.1-2026-08-25.json`) | 0.156.0 | 7 stable releases unvalidated | `cas-0d4f` — in progress: matrix retargeted to 0.156.0; no receipt yet |
| Grok | 1.0.5 (`crates/cas-pty/conformance/grok-build-1.0.5-2026-08-25.json`) | 1.0.40 | 35 version numbers unvalidated | `cas-ef93` — in progress: full `PtyConfig::grok` matrix plus the 1.0.24 urgent-interrupt check |

### Per-harness notes

**Claude Code.** This sweep produced no 👀. Model-default changes (Opus 5.5 at 2.1.280, Fable 5.1
at 2.1.257) route to the model-lane refresh `cas-8505`, which is in progress and scoped to a
decision brief. The operator's decision, recorded 2026-09-23, is that no lane-registry edit
happens without his approval. `crates/cas-factory/policy/lane-registry.toml` still pins
`claude-opus-5`.

**Codex.** Every Cassy-relevant 0.150–0.156 item is an upgrade-validation 👀 grouped under
`cas-0d4f`. No release-note item names a standalone code fix.

**Grok.** The headline item is 1.0.24: Esc no longer cancels a running turn. The Grok code path
already accounts for it:

- `Pane::break_turn` writes the harness's cancel bytes (`crates/cas-mux/src/pane/mod.rs:1462-1466`).
- The Grok backend returns Ctrl+C `0x03`, not Esc (`crates/cas-mux/src/backend/grok.rs:70-72`,
  pinned by `crates/cas-mux/src/harness.rs:293-296`, commit `415a30de`).

A live confirmation on 1.0.40 remains with `cas-ef93`. The diary's citation `pty.rs:5421-5426`
points at a Pty-layer test comment, not the break path.

### Scope gaps in cas-ef93

The Grok diary hands four checks to "the matrix" that `cas-ef93`'s description and acceptance
criteria do not list:

- 1.0.17 mid-call MCP input
- 1.0.8 MCP consent popup
- 1.0.34 memory generally available
- 0.2.105 compaction survival

Unless the matrix adds these four checks, cas-ef93 will close with them still open.

### Stale diary text found during the audit

- **Codex touchpoints (diary L77–78)** still say Codex has no Claude-style hook system. Trusted
  Codex hooks shipped in `1b4e03bf` (cas-ba048) and PostToolUse wiring in `cb7ce9a6` (cas-5ae8).
- **Codex version status (L41–43, L226–227)** says Cassy maps only `Effort::XHigh`. `Effort::Max`
  shipped in `47430713` (PR #813, cas-556a).
- **Claude Code 2.1.219 (L665)** and **2.1.187 (L1160)** cite the `cas-code-review` Workflow,
  which was removed in `9cf31b2a` (cas-7216, 2026-09-01).
- **Grok 1.0.24 / version status** cites `pty.rs:5421-5426` as the interrupt path. The real path
  is `crates/cas-mux/src/pane/mod.rs:1462` together with `crates/cas-mux/src/backend/grok.rs:70`.

## How the classification was done

Each harness diary was read in full (Entries and Backlog). Every candidate item was checked
against four sources:

- task status (`mcp__cas__task action=show`);
- commits (`git log --all --grep`, `git merge-base --is-ancestor`);
- code (`grep -rn` over `cas-cli/src`, `crates`, `scripts`, `.claude`, `docs`);
- conformance receipts (`crates/cas-pty/conformance/*.json`).

### Classes

- **Addressed:** a closed-delivered task, a commit, a code path, or a passing receipt check covers
  the item.
- **In flight:** an open task owns it.
- **Never addressed:** no task; a task that was abandoned without delivery; or a 👀 with no
  follow-up anywhere. The evidence is the zero-hit searches listed per row.
- **No longer applicable:** only when the diary itself resolves the item.

A "verify on upgrade" touchpoint counts as addressed once a later passing receipt exercises it. An
"adopt X" opportunity is not addressed by a matrix run.

### Falsification

Any never-addressed row is wrong if a task, commit, or recorded check exists
that the listed searches missed. Every row names the search terms used, so this can be checked.

| Harness | Rows | Addressed | In flight | Never addressed | No longer applicable |
| --- | ---: | ---: | ---: | ---: | ---: |
| Claude Code | 42 | 20 | 4 | 16 | 2 |
| Codex | 32 | 17 | 2 | 13 | 0 |
| Grok | 72 | 23 | 26 | 23 | 0 |
| **Total** | **146** | **60** | **32** | **52** | **2** |

Codex's 0.144 auth-elicitation row is split. The `cs` smoke test is addressed, but the case of a
second MCP server that needs auth is not, so the row counts as never addressed here. Grok's 1.0.24
row counts as addressed in code, with its live proof in flight.

## Evidence appendix — every audited item

### Claude Code

| Version | Diary line | Item | Why it matters to Cassy | Class | Evidence |
| --- | --- | --- | --- | --- | --- |
| 2.1.280 | L163 | Opus 5.5 default Opus, 1M context; lane drift recorded for `cas-8505` | Registry `claude_opus` pins `claude-opus-5`; 5.5 placement undecided | IN FLIGHT | cas-8505 InProgress (P1, epic cas-6f39); `lane-registry.toml:40` still `claude-opus-5`; operator decision note 2026-09-23 12:02 requires approval before registry edits |
| 2.1.267 | L259 | `maxEffortLevel` / effort placement tracked by `cas-8505` | Effort per lane is Cassy registry policy | IN FLIGHT | cas-8505 InProgress |
| 2.1.260 | L307 | Fable 5.1/model availability recorded by lane-rubric work `cas-8505` | Model availability drives lane routing | IN FLIGHT | cas-8505 InProgress |
| 2.1.257 | L327 | Fable 5.1 default; placement shipped `cas-bddf`, drift via `cas-8505` | Taste lane routes to Fable 5.1 | ADDRESSED | cas-bddf Closed/delivered 2026-09-05 (ee131691 merged to epic 186e0de7, CI 33982089713); `lane-registry.toml:47-50,116` |
| 2.1.251 | L365 | Pre/PostModelSwitch hooks, Opus 5 Enterprise default; placement tracked by `cas-8505` | Model placement registry | IN FLIGHT | cas-8505 InProgress |
| 2.1.239 | L482-487 | 👀 BOM skills no longer ignored; "continue checking mirror generation on upgrades" | Synced skill mirrors must load in workers | NEVER ADDRESSED | No upgrade check recorded; BOM commits (11e573cc, 11cf4f20) cover code index/CHANGELOG only; BOM grep in sync code = 0 |
| 2.1.237 | L514-519 | 👀 watch: deleted-cwd recovery, skill hot-reload, runner hooks | Stale sessions/skills after worktree removal | NEVER ADDRESSED | `git log --grep "deleted (cwd\|working dir)"` = 0; no task; watch has no follow-up |
| 2.1.233 | L586-590 | 👀 skill aliases survive shadowing; "verify mirror precedence on upgrades" | Cassy skill mirrors must win over user/cloud skills | NEVER ADDRESSED | "skill precedence" git/grep = 0; CAS search found no verification task |
| 2.1.231/229 | L619-626 | 👀 retain MCP startup watch (OAuth redirect, managed-MCP remote startup) | Workspaces mixing `cs` stdio with remote MCP | NEVER ADDRESSED | No task/commit; `git log --grep "remote MCP\|mcp oauth"` hits unrelated (8983abdd neon .mcp.json portability) |
| 2.1.228/229 | L631-634 | 👀 watch skill precedence vs cloud-synced skills after upgrade | Required worker instructions could be silently displaced | NEVER ADDRESSED | Same searches as L586 = 0; no recorded post-upgrade check |
| 2.1.219 | L656 | 👀 Opus 5 default; Cassy `opus` alias follows by design | Worker model identity | ADDRESSED | Registry now pins explicit `claude-opus-5` (`crates/cas-factory/policy/lane-registry.toml:37-40`); 5.5 drift carried by cas-8505 |
| 2.1.217 | L693 | Review task `cas-9642` (diary 2.1.210–2.1.217) | Diary coverage | ADDRESSED | cas-9642 Closed/delivered 2026-07-22 (6195a82) |
| 2.1.212 | L779 | 👀 MCP calls >2 min auto-background; inspect `CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS` | Long `mcp__cas__` search/close calls may detach | NEVER ADDRESSED | grep/git-log MCP_AUTO_BACKGROUND = 0; no task |
| 2.1.211 | L810 | 👀 `--forward-subagent-text` for headless verifier diagnostics | Verifier transcript diagnosability | NEVER ADDRESSED | grep/git-log forward-subagent-text = 0 |
| 2.1.209 | L835 | Review task `cas-aeec9` (diary 2.1.203–2.1.209) | Diary coverage | ADDRESSED | cas-aeec9 Closed/delivered 2026-07-14 (962e950 via 67b2e6a) |
| 2.1.208 | L846 | 👀 DSP catastrophic `rm` with `$(…)` now prompts; stall detection is backstop | Unattended worker hangs on permission prompt | ADDRESSED | cas-6027a Closed/delivered 2026-09-06 (ae845570 in HEAD): rm approval-hang root-caused, launcher fixed, `ApprovalHang` state `cas-cli/src/cli/factory/wedged.rs:252,343` |
| 2.1.206 | L903 | 👀 `EnterWorktree` confirms outside `.claude/worktrees/`; Cassy uses `.cas/worktrees` | Agent entering Cassy worktree hangs on confirm | NEVER ADDRESSED | EnterWorktree not in `--disallowedTools` (`crates/cas-pty/src/pty.rs:1253` only AskUserQuestion); no PreToolUse rule; grep = 1 non-goal doc line |
| 2.1.202 | L989 | Workflow parse fixes aid `cas-code-review` Workflow (`cas-b667`) | Review pipeline reliability | ADDRESSED | cas-b667 EPIC Closed/delivered (v2.19.0, 78656f4). Note: Workflow later deleted by cas-7216 (9cf31b2a, 2026-09-01) |
| 2.1.200 | L1012-1019 | 👀 AskUserQuestion no auto-continue; `cas-e603` reminder cited | Unattended factory pane hangs forever | ADDRESSED | cas-e603 Closed (5d21fd8); then blocked: cas-afe9 e3f855a9/de0b04fa (`pre_tool.rs:119`), cas-d8ea d5a89755 (`pty.rs:1253 --disallowedTools AskUserQuestion`) |
| 2.1.198 | L1061-1066 | 👀 subagents background by default; verify close-time verification flow | Close could race an unfinished verifier | ADDRESSED | Diary L1063-1066 records live evidence; structural guard: in-flight dispatch blocks close (`close_ops.rs:5261`, cas-164c Closed, cd34f46) |
| 2.1.197 | L1089-1103 | 👀 Sonnet 5 GA; re-score tier rubric, ignore promo pricing | Worker model/effort placement | ADDRESSED | v2.26.0 b0a457fe (Sonnet 5 default, later superseded); `docs/factory/2026-09-06-model-lane-rubric-review.md:334,413` scores Sonnet 5; cas-8505 refresh in flight |
| 2.1.196 | L1114-1117 | 👀 streaming idle watchdog default-on (5 min) | Long silent tool turns may abort/retry | NEVER ADDRESSED | STREAM_WATCHDOG grep/git-log = 0; no task |
| 2.1.196 | L1120 | Built-in `/code-review` token cut vs Cassy `cas-b667` Workflow | Disambiguation only | ADDRESSED | cas-b667 Closed/delivered (v2.19.0) |
| 2.1.187 | L1156 | `Agent(type)` enforcement orthogonal to `cas-5be8` gating | Tool gating authority | ADDRESSED | cas-5be8 Closed/delivered 2026-06-02 (bdf93e4, 65c6368) |
| 2.1.187 | L1160-1163 | 👀 structured-output hardening benefits `cas-code-review` schema dispatch | Review pipeline reliability | NEVER ADDRESSED | No follow-up/verification recorded; moot since the Workflow was deleted (cas-7216, 9cf31b2a; `.claude/workflows/` has only `fixtures`) but the diary never records that |
| 2.1.183 | L1176-1181 | 👀 touchpoint: tmux pane launch + keystroke leak; "verify on upgrade" | Factory worker spawn reliability | NEVER ADDRESSED | No upgrade verification recorded; git-log keystroke/rc-file hits unrelated (c5c0a30d, 420b943c are Cassy TUI) ; CAS search no task |
| 2.1.183 | L1186-1188 | 👀 noted: teammate background tasks killed at turn end | Only if Cassy used turn-scoped teammate tasks | NO LONGER APPLICABLE | Diary L1186-1188 itself resolves: Cassy does not use them (long-lived workers) |
| 2.1.178 | L1200-1204 | Caveat to watch: prompts still calling `TeamCreate` now dead | Dead instructions in worker prompts | ADDRESSED | Diary L1203-1204 checked (none found); `grep -rn TeamCreate` = 0 today |
| 2.1.178 | L1206-1208 | 👀 namespace note: nested skills `<dir>:<name>` on clash | Skill name collisions | NO LONGER APPLICABLE | Diary L1208 resolves: Cassy skills at project root, no collision expected |
| 2.1.178 | L1210 | disallowedTools MCP specs honored; `cas-5be8` relies on them | Tool gating enforcement | ADDRESSED | cas-5be8 Closed/delivered |
| 2.1.170 | L1243-1255 | 👀 Fable 5 opportunity: model id, cost, safeguard fallback blockers | Supervisor/hard-task lane choice | ADDRESSED | cas-bddf Closed/delivered: taste lane → `claude-fable-5-1` with explicit Opus fallback (`lane-registry.toml:116-119`) |
| 2.1.170 | L1257-1270 | 👀 transcript-save with inherited CC env | Session-log mining/attribution | ADDRESSED | Diary L1265-1270: checked 2026-06-10, 161 worker JSONLs present, downgraded |
| 2.1.169 | L1277-1281 | 👀 `--safe-mode` as first triage step in troubleshooting/onboarding doc | Fast "Cassy vs harness" isolation | NEVER ADDRESSED | safe-mode grep/git-log = 0; CAS search no task/doc |
| 2.1.169 | L1282-1285 | 👀 `disableBundledSkills` namespace hygiene option | Skill menu clutter/slash collisions | NEVER ADDRESSED | disableBundledSkills grep/git-log = 0 |
| 2.1.166 | L1320 | Deny-rule globs orthogonal to `cas-5be8` | Tool gating | ADDRESSED | cas-5be8 Closed/delivered |
| 2.1.163 | L1328-1333 | 👀 Stop/SubagentStop `additionalContext` for session-learn/supervisor guidance spike | Cleaner Stop-hook guidance channel | NEVER ADDRESSED | "No task filed yet" (L1332-1333) still true; `HookSpecificOutput` has no Stop/SubagentStop variant (`crates/cas-core/src/hooks/types.rs:397-468`); last Stop change baa540bc (2026-04-13) predates 2.1.163 |
| 2.1.152–160 | L153, L1375-1376 | 🏗 EPIC hook surface: reloadSkills, sessionTitle, disallowed-tools, MessageDisplay | Core hook integration | ADDRESSED | cas-2f29 EPIC Closed/delivered 2026-07-21, 5/5 children delivered, ver-9386ccba2f71; shipped v2.18.0 |
| 2.1.160 | L1380 | acceptEdits sensitive-file prompt characterized in `cas-2f29`/`cas-f97d` | Worker stall risk | ADDRESSED | cas-f97d Closed/delivered 2026-06-02 (NOT IMPACTED; bypassPermissions) |
| 2.1.160 | L1385-1389 | Terminology watch: `workflow` → `ultracode` trigger rename | Stale user guidance | ADDRESSED | Diary L1388-1389 checked (none found); `grep ultracode` = 0, no stale "say workflow" prose |
| backlog | L1396-1398 | Model-tier rubric refresh for Sonnet 5 | Worker placement | ADDRESSED | Same as L1089: 2026-09-06 rubric review scores Sonnet 5; cas-8505 refresh in flight |
| backlog | L1399-1400 | session-learn/guidance via Stop-hook `additionalContext` | Stop-hook channel | NEVER ADDRESSED | Duplicate of L1328 row; same evidence |
| backlog | L1401-1402 | Factory CC version floor via `requiredMinimumVersion` | Pins factory hosts to known-good CC | NEVER ADDRESSED | requiredMinimumVersion grep/git-log = 0; no min-CC-version check in `cas-cli/src`/`crates` |

### Codex

| Version | Diary line | Item | Why it matters to Cassy | Class | Evidence |
| --- | --- | --- | --- | --- | --- |
| 0.150.0–0.156.0 | L122–213 (7 entries) | Interrupt hooks, AGENTS trust gating, required/remote MCP, MCP OAuth/credential recovery, sandbox hardening, resume/permission restore, model-aware effort fallback | Every Cassy touchpoint (`--yolo`, `cs` MCP, AGENTS.md, skills mirror, effort, interrupt/resume) unvalidated past 0.149.1 | IN FLIGHT | cas-0d4f InProgress (matrix retargeted to 0.156.0, build running); no 0.15x receipt in `crates/cas-pty/conformance/` |
| 0.149.1 | L221–227 | `-c model_reasoning_effort=max` probe accepted; Cassy maps only XHigh | Effort vocabulary gap for max-capable models | ADDRESSED | cas-556a closed/delivered, commit 47430713 (PR #813, main 7331d9e3); `Effort::Max` at `crates/cas-mux/src/spec.rs:29`; pty test `crates/cas-pty/src/pty.rs:4338-4356` |
| 0.146.0 | L347–370 | MCP live refresh/reconnect; executor skills + truncation; Agent Plugins shadowing; approval continuity on resume | `cs` catalog staleness, hidden worker guidance, lost `--yolo` bypass on resume | ADDRESSED | R146 + R149 checks `mcp_cs_root_turn`, `mcp_cs_followup_turn`, `skills_agents_and_agents_md_discovery`, `interruption_resume_approval_continuity` all pass; fix commit 02226d1a (cas-8c80) |
| 0.146.0 / 0.139.0 / 0.143.0 | L371–375, L602–605, L509 | Proxy routing / proxy-only networking under host proxy policy | A host proxy policy could block `cs` startup or worker network under `--yolo` | NEVER ADDRESSED | Receipts run with `danger-full-access`, no proxy config; `grep -rn -i -E 'proxy.only' crates cas-cli/src` hits=0; `git log --all -i -E --grep='proxy.only'` hits=0 |
| 0.145.0 | L387–394 | Multi-agent V2 stable: configurable sub-agent model/effort/roles | Native role/spawn defaults could shadow Cassy developer_instructions/model/effort | ADDRESSED | R149 `developer_role_priming`, `model_and_reasoning_effort` pass; diary L239–243 records native delegation kept out of contract |
| 0.145.0 | L395–400 | `/import` of Claude Code/Cursor config could shadow `cs` registration/skills | Imported config could replace Cassy MCP/skill wiring | NEVER ADDRESSED | No import-scenario check in R146/R149 checklists; `git log --all -i -E --grep='codex.*import'` hits=0; task search "codex /import onboarding" → no relevant task |
| 0.145.0 | L401–406 | MCP startup timeout + catalog reuse | Slow `cas serve` start could be classified failed | ADDRESSED | R149 `mcp_cs_root_turn` ("initialized CAS_ROOT served mcp__cs tools before the root lifecycle turn") |
| 0.145.0 / 0.144.0 / 0.143.0 / 0.142.x / 0.138.0 / 0.137.0 | L407–410, L451–454, L497–501, L542–545, L664–666, L683–689 | Skills plumbing churn (codex-skills crate, SkillsService, extension bridge, malformed-field warnings) | `cas integrate` `.codex/skills`/`.codex/agents` mirror must still load | ADDRESSED | R149 `skills_agents_and_agents_md_discovery` pass (AGENTS.md, .codex skill, .codex agent markers) + `cas-cli/tests/factory_parity_test.rs` |
| 0.145.0 / 0.142.x / 0.137.0 / 0.139.0 / 0.136.0 | L411–415, L535–538, L690–692, L602–605, L704–707 | Approval/sandbox tightening, env-scoped approvals, env identity, escalation preservation, deny-read on bypass paths (Linux) | Could reintroduce prompts or block reads for `--yolo` workers | ADDRESSED | R149 `noninteractive_permission_bypass` (approval_policy=never, danger-full-access on root/follow-up/resumed) + `inline_tui_and_cwd` |
| 0.145.0 / 0.144.0 | L411–415 (Windows proxy), L459–461 | Windows sandbox writable-root/proxy enforcement changes | Would affect only Windows Codex workers | NEVER ADDRESSED | Both receipts are Linux runs (rollout paths under /home/pippenz); `git log --all -i -E --grep='codex.*windows'` → only 08a710fc (unrelated stall windows) |
| 0.144.0 | L441–445 | New `writes` app-approval mode might default under `--yolo` | Could reintroduce prompts for factory sessions | ADDRESSED | R149 `noninteractive_permission_bypass` pass; `grep -rn -E 'approval_mode\|"writes"' crates cas-cli/src` hits=0 (Cassy config never pins it) |
| 0.144.0 | L446–450 | MCP auth elicitation default: smoke `cs`; watch hang when a second MCP server needs auth | Mixed-server configs could stall worker MCP startup | ADDRESSED (cs smoke) / NEVER ADDRESSED (second-server hang) | cs: R149 `mcp_cs_root_turn`. Second-server scenario: absent from both receipt checklists; no task found |
| 0.144.0 / 0.139.0 / 0.130–0.135 | L462–465, L606–609, L636–641 | Schema compaction threshold, `oneOf`/`allOf`, `$ref`/`$defs`: smoke multi-arg `cs` tools | Large multi-arg `cs` tool schemas could be mangled | NEVER ADDRESSED | Receipts exercise only `coordination whoami` and `task mine` (R149 checks 5–6 detail); no schema-fidelity test: `git log --all -i -E --grep='oneOf\|allOf\|schema compaction'` hits=0 for codex |
| 0.143.0 | L475–481 | First-class `max` effort; verify `xhigh` still accepted | Effort mapping must match Codex vocabulary | ADDRESSED | R149 `model_and_reasoning_effort` (xhigh) + `max_effort_probe`; cas-556a / 47430713 added `Effort::Max` |
| 0.143.0 | L482–487 | MCP tools use tool search by default (highest-risk 0.143 item) | Could hide `mcp__cs__*` behind search step | ADDRESSED | cas-8c80 fix 02226d1a (direct-only `mcp__cs` namespace); R146/R149 `code_mode_and_direct_cs_coexist` pass |
| 0.143.0 / 0.136.0 / 0.149 | L488–490, L708–710 | rmcp 1.8.0 / 1.7.0 client bumps | MCP client protocol regression risk for `cs` | ADDRESSED | R149 `mcp_cs_root_turn` + `mcp_cs_followup_turn` pass on rmcp 3.1.2-era client |
| 0.143.0 / 0.142.x / 0.138.0 / 0.130–0.135 | L491–496, L539–541, L667–669, L632–635 | AGENTS.md env-reactive, foreign-env, symlink, invalid-UTF-8 loading | Worker role priming rides worktree AGENTS.md | ADDRESSED | R149 `skills_agents_and_agents_md_discovery` + `developer_role_priming` pass |
| 0.143.0 / 0.130–0.135 | L502–505, L623–627 | Sandbox profile flag rename; `--profile` primary, legacy profile configs rejected | Legacy profile block in Cassy-written `.codex` would be rejected | ADDRESSED | `grep -rn -E '\[profiles\|profiles\.' cas-cli/src/cli/hook/config_gen.rs init.rs update.rs crates/cas-pty/src` hits=0; R149 launch passed |
| 0.143.0 / 0.142.x | L506–508, L529–534 | Rollout token budgets abort turns; verify no low default budget | Long factory turns killed mid-task | ADDRESSED | R146/R149 `no_low_rollout_token_budget_abort` pass; assertion `crates/cas-mux/tests/codex_factory_contract_runtime.rs:326,431` |
| 0.142.x | L531–534 | If budgets default on: raise via `-c` or surface "turn aborted: budget" distinctly from stall | Budget abort would look like a silent stall to the factory | NEVER ADDRESSED | Only test-side detection exists (`codex_factory_contract_runtime.rs:431`); `grep -rn -i 'rollout token budget' crates cas-cli/src --include=*.rs` (non-test) hits=0; conditional trigger not yet met |
| 0.142.x / 0.141.0 / 0.139.0 / 0.138.0 / 0.137.0 + Backlog | L546–550, L610–612, L670–673, L693–694, L725–726 | Multi-agent v2 strategic posture: decide Cassy stance toward Codex-native orchestration | Strategic overlap/competition with Cassy factory orchestration | NEVER ADDRESSED | `git log --all -i --grep='multi-agent v2'` hits=0; code grep `multi_agent` hits=0; task search "codex native subagents multi-agent stance" → No results |
| 0.141.0 / 0.140.0 / 0.130–0.135 + Backlog | L558–563, L594–596, L628–631, L730–732 | Codex hooks (hooks.json, trust bypass, PostToolUse, subagent identity): optional adoption for Claude-path parity | Hook-based auto-approve/jail parity on Codex path | ADDRESSED | cas-ba048 closed (1b4e03bf "provision trusted Codex hooks", 2026-08-11; `crates/cas-pty/src/codex_trust.rs:95-99` `CAS_HOOK_HARNESS=codex cas hook PreToolUse`); cas-5ae8 closed (Codex PostToolUse wiring, cb7ce9a6). NOTE: diary L77–78 still says "Codex has no Claude-style hook system" — stale |
| 0.141.0 | L564–567 | Per-thread plugin stdio MCP activation (highest-risk 0.141 item) | `cs` might not load on every worker thread | ADDRESSED | R149 `mcp_cs_root_turn` + `mcp_cs_followup_turn` pass |
| 0.141.0 | L570–572 | TUI input prompts auto-resolve after inactivity; could auto-answer worker prompt | A worker hang/auto-answer on an input dialog | NEVER ADDRESSED | `grep -rn -i request_user_input crates cas-cli/src` hits=0; `git log --all -i --grep=request_user_input` hits=0; no receipt check |
| 0.140.0 | L579–583 | `/import` from Claude Code: flag for onboarding-doc pass | Cross-harness onboarding story | NEVER ADDRESSED | `grep -rln -E 'codex.*/import\|/import.*codex' docs cas-cli/docs` → only a CSV data file; no task found |
| 0.140.0 | L584–589 | Codex SQLite auto-recover; same hazard class as cas.db restore breaking MCP | Informational hazard parallel | NEVER ADDRESSED | Diary itself says "no overlap expected"; no follow-up task/commit (`git log --all -i --grep='codex.*sqlite'` hits=0) |
| 0.140.0 | L590–593 | Encrypted MCP-OAuth secret storage; verify `.codex/config.toml` MCP read unchanged | `cs` registration read path | ADDRESSED | R149 `mcp_cs_root_turn` pass (spawn-injected `cs` read and served) |
| 0.138.0 + Backlog | L658–663, L723–724 | If Codex ships first-class `--effort`, switch from `-c model_reasoning_effort` | Cleaner, version-stable effort passing | NEVER ADDRESSED (trigger unmet) | `codex --help` on 0.156.0: grep `effort` hits=0 (only `-p, --profile`); still uses `-c` at `crates/cas-pty/src/pty.rs:1379`; R149 confirms key still works |
| 0.138.0 / 0.137.0 | L658–663 | Verify `model_reasoning_effort` key and vocabulary still validate | Effort passthrough | ADDRESSED | R149 `model_and_reasoning_effort` (xhigh) pass |
| 0.130–0.135 | L646–649 | Codex git helpers ignore repo hook/fsmonitor config in worktrees | Interaction with factory commit guard | NEVER ADDRESSED | Informational ("no conflict expected"); no receipt check covers commits; `git log --all -i --grep=fsmonitor` hits=0 |
| 0.136.0 | L714–715 | Codex "memories" root moved; confirm no collision with Cassy memory via MCP | Naming collision with Cassy memory tools | NEVER ADDRESSED | No task/commit (`git log --all -i -E --grep='codex.*memor'` hits=0); no receipt check |
| Backlog | L727–729 | Future upgrade validation: rerun typed matrix before advancing pin | Keeps validated pin current | IN FLIGHT | 0.149.1 done (cas-b9a4, R149); 0.156.0 run = cas-0d4f InProgress |

### Grok

| Version | Diary line | Item | Why it matters to Cassy | Class | Evidence |
| --- | --- | --- | --- | --- | --- |
| (header) | L6 | EPIC cas-8888: first-class Grok harness (cli=grok) | Whole Grok factory support | ADDRESSED | merge f4a3e4b5 "first-class Grok Build harness … (cas-8888)"; phase commits 04bb55f5, 9ab4edb2, cfc7ef3b |
| (touchpoints) | L116 | cas-921f: CAS_FACTORY_WORKER_CLI=grok set unconditionally | Liveness globbed Claude tree, Grok workers never resolved | ADDRESSED | de68d039 + merge 1512f044; `crates/cas-pty/src/pty.rs:1649`; cas-921f Closed |
| 1.0.14–16, 1.0.26–29, 1.0.35–40 | L61, L205 | Source gap; 1.0.40 needs full PtyConfig::grok matrix | Pin 1.0.5 vs installed 1.0.40, 35 versions unvalidated | IN FLIGHT | cas-ef93 InProgress; preflight already flags stale receipt (`cas-cli/src/factory_preflight.rs:877-935`) |
| 1.0.34 | L212 | Memory GA; verify rules/env/cas__ discovery with memory enabled | Grok memory could inject stale context competing with Cassy rules | NEVER ADDRESSED | not in cas-ef93 AC/notes (scope gap); `git log --all -i --grep='grok.*memory'` hits=0 |
| 1.0.33 | L225 | Structured JSON MCP results; cancelled MCP calls stop | Could change how cas__ results are parsed/receipted | NEVER ADDRESSED | not in cas-ef93 AC (scope gap); `git log --all -i --grep='structured.*mcp\|structuredContent'` → only 6ad75a2f (cas-remember, unrelated to Grok) |
| 1.0.33 | L231 | Subagent cancel, compacted history retained, rewind/session recovery | Transcript path/liveness keyed on injected UUID | IN FLIGHT | cas-ef93 (transcript/liveness) |
| 1.0.32 | L242 | Plugin/skill listing reads config.toml before first session | Prompt/config layering could displace --rules or cas__ discovery | IN FLIGHT | cas-ef93 (--rules, persistent discovery) |
| 1.0.31 | L258 | Worktree headers drop suffix; subagent scrollback counts | Worktree identity vs CAS_CLONE_PATH | IN FLIGHT | cas-ef93 (worktree containment) |
| 1.0.30 | L270 | Session timing/workflow status/tmux lag | Diagnostic only; liveness uses transcripts | NEVER ADDRESSED | diary: no action indicated; no task (cas-ef93 AC silent); low |
| 1.0.25 | L280 | Successful hooks silent; only blocking/failing shown | Could hide a blocking hook signal needed for cleanup | IN FLIGHT | cas-ef93 (hooks-disabled posture) |
| 1.0.25 | L285 | Headless timeout, `grok -c` session fix, concurrent-boot settings clobber | Session UUID + discovery; factory boots workers concurrently | IN FLIGHT | cas-ef93 (session UUID, discovery); caveat: matrix is single-worker, concurrent boot not enumerated |
| 1.0.24 | L298 | Esc no longer cancels a running turn | Urgent interrupt-and-redirect must break Grok turns | ADDRESSED (code) / live proof IN FLIGHT | Grok cancel already Ctrl+C: `crates/cas-mux/src/backend/grok.rs:70-72` (0x03), `crates/cas-mux/src/pane/mod.rs:1462` break_turn uses harness bytes, urgent path `cas-cli/src/ui/factory/daemon/runtime/delivery.rs:748`; pinned by test `cas-cli/src/ui/factory/app/sidecar_and_selection.rs:1873-1888` (cas-7f6f, 415a30de). Diary cite pty.rs:5421-5426 is a stale Pty-layer test comment. Live check: cas-ef93 note 12:08 |
| 1.0.22 | L317 | Built-in tools beat user MCP on name collision; MCP reauth state | cas__ tools could be shadowed | IN FLIGHT | cas-ef93 (cas__ namespace/discovery) |
| 1.0.22 | L321 | Subagent continuation/background completion/monitor wakeups | Grok subagents are not Cassy workers | NEVER ADDRESSED | no follow-up; cas-ef93 AC omits Grok-owned subagents; low |
| 1.0.22 | L326 | Auto mode refuses destructive checkout; bash settings propagate | Bypass mode must stay authoritative | IN FLIGHT | cas-ef93 (permission bypass) |
| 1.0.21 | L336 | Permission mode persists/restores around plan mode | Could reintroduce interactive gate | IN FLIGHT | cas-ef93 (permission bypass) |
| 1.0.19 | L356 | Org-blocked MCP refused; MCP connects in background after login | Blocked cas server must fail preflight, not look like missing tools | NEVER ADDRESSED | `cas factory doctor` CAS-MCP row is Codex-only (`cas-cli/src/cli/factory/doctor.rs:179`); preflight checks only the project `.mcp.json` `cas` entry and caller-side live observation (`cas-cli/src/factory_preflight.rs:609-660,1236-1244`), not Grok's own enable/disable or org-policy state |
| 1.0.19 | L361 | Resume reports loops/subagents; headless `--worktree` | Worktree ownership, UUID-keyed liveness | IN FLIGHT | cas-ef93 (worktree containment, transcript) |
| 1.0.18 | L373 | Sessions prep in background; start hooks async; async bookkeeping | Transcript registration timing vs "worker live" | IN FLIGHT | cas-ef93 (transcript/liveness) |
| 1.0.18 | L378 | Auth recovery; subagents inherit model retry settings | Subagent/model separation | NEVER ADDRESSED | no follow-up/task; low |
| 1.0.17 | L390 | MCP tools may request input mid-call | Headless worker could block on unanswered prompt | IN FLIGHT (scope gap) | diary routes to cas-ef93 (L392) but cas-ef93 description/AC do not enumerate it |
| 1.0.13 | L403 | Truncated responses continue; retries; session saves reliably | Transcript/liveness | IN FLIGHT | cas-ef93 (transcript/liveness) |
| 1.0.13 | L407 | Hooks can request confirmation/deferral/post-tool context | Hook result surface vs hooks-disabled posture | IN FLIGHT | cas-ef93 (hooks-disabled posture) |
| 1.0.12 | L424 | MCP connection retries; subagent waits after interjection | cas__ discovery reliability | IN FLIGHT | cas-ef93 (discovery) |
| 1.0.12 | L429 | Context/token estimates, compaction progress, auto-recap timing | Transcript evidence | IN FLIGHT | cas-ef93 (transcript); compaction itself untested (see 0.2.105 row) |
| 1.0.11 | L444 | Headless auto-allow; configurable default permission mode | Bypass + CAS_SESSION_ID lookup | IN FLIGHT | cas-ef93 (bypass, session UUID) |
| 1.0.11 | L450 | Background waits finish; command-chain permission prompts | Approval semantics | IN FLIGHT | cas-ef93 (bypass) |
| 1.0.10 | L459 | `grok clone` reuses local checkouts as linked worktrees | Could touch Cassy-managed worktrees/branches | IN FLIGHT | cas-ef93 (worktree containment; diary L462) |
| 1.0.9 | L471 | MCP User-Agent; MCP startup/subagent burst reliability | Discovery/identity | IN FLIGHT | cas-ef93 (discovery, identity) |
| 1.0.9 | L475 | `grok clone` fetches branch tip into linked worktree | Worktree containment | IN FLIGHT | cas-ef93 |
| 1.0.9 | L478 | Rules markdown headings; workflow prompt/subagent visibility | --rules is load-bearing | IN FLIGHT | cas-ef93 (--rules) |
| 1.0.8 | L491 | MCP servers can request form/URL consent via popup | New interactive path could stall unattended worker | NEVER ADDRESSED | diary says "the matrix must ensure", but cas-ef93 description/AC/notes omit it (scope gap); no task |
| 1.0.7 | L509 | GROK_CONNECT_UI_TIMEOUT_SECS; tokenless MCP no auth headless | Startup budget; Cassy does not set it | IN FLIGHT | cas-ef93 (startup/discovery); `git grep GROK_CONNECT_UI_TIMEOUT` hits=0 (tuning opportunity unowned) |
| 1.0.7 | L513 | Persistent Always/Never grants for MCP tools/domains | Bypass must stay authoritative | IN FLIGHT | cas-ef93 (bypass) |
| 1.0.6 | L526 | Subagent spawn drops `capability_mode` | Tool scope of Grok subagents | ADDRESSED (no dependency) | `git grep -i capability_mode` hits=0 — Cassy never passes it; worker tool set covered by cas-ef93 discovery check |
| 1.0.6 | L532 | Projected clone trees; large repos no longer hang startup | Worktree containment/startup | IN FLIGHT | cas-ef93 (worktree containment) |
| 1.0.6 | L536 | Queued messages during goals | Factory message queue UX | NEVER ADDRESSED | no follow-up; low |
| 1.0.5 | L549 | GROK_CONFIG/GROK_CONFIG_PATH overrides | Override could hide cas server/rules/identity | ADDRESSED | 1.0.5 receipt passes discovery/rules/identity (74f6086f, cas-444a) |
| 1.0.5 | L556 | Auto-reclaim of ~/.grok/worktrees | Grok cleanup must never delete Cassy checkouts | IN FLIGHT | 1.0.5 receipt has no containment check; `git grep grok/worktrees` hits=0; now in cas-ef93 scope |
| 1.0.5 | L561 | Hook blocks labelled as hook blocks | Hooks posture | ADDRESSED | receipt `compatible_hooks_disabled` pass |
| 1.0.5 | L565 | Earlier titles; /resume recap | Liveness keyed by UUID not title | ADDRESSED | receipt `session_uuid`, `transcript_and_liveness` pass |
| 1.0.4 | L584 | Tools/MCP receive GROK_SESSION_ID | Must not diverge from CAS_SESSION_ID | ADDRESSED | receipt `session_uuid` (same ID names transcript); `git grep GROK_SESSION_ID` hits=0 (Cassy never consumes it) |
| 1.0.4 | L590 | Auto mode honors always-allow/narrow rules | Bypass semantics | ADDRESSED | receipt `noninteractive_permission_bypass` (yolo_mode=true) |
| 1.0.4 | L596 | Headless waits for MCP; subagent lifecycle ordering | cas__ discovery on headless start | ADDRESSED | receipt `persistent_mcp_discovery_and_namespace` |
| 1.0.4 | L602 | Session search disable; subagent transcripts rebuilt | Transcript availability | ADDRESSED | receipt `transcript_and_liveness` |
| 1.0.4 | L608 | Hook stderr; welcome permission mode; child cleanup | Hook/bypass posture, process cleanup | ADDRESSED | receipt hooks + bypass + lifecycle pass; process-group teardown cas-99f5 Closed |
| 0.2.115–0.2.117 | L625 | GROK_EXTRA_CA_BUNDLE TLS roots; background subagents stopped | Transport env must not hide identity/MCP config | NEVER ADDRESSED | `git grep GROK_EXTRA_CA_BUNDLE` hits=0; receipt ran without it; low |
| 0.2.118–0.2.119 | L630 | Broad bash allow-list editing | Approval semantics | ADDRESSED | receipt bypass pass |
| 1.0.0–1.0.1 | L635 | Verify cas__ discovery + --rules/env on fresh headless worker | Core launch contract | ADDRESSED | 1.0.5 receipt (74f6086f) |
| 1.0.2–1.0.3 | L639 | Re-run full PTY matrix before advancing pin | Pin honesty | ADDRESSED | cas-444a Closed, 74f6086f; worktree-containment part carried to cas-ef93 |
| 0.2.113 | L670 | MCP enable/disable CLI; preflight must report discovery health | Operator can disable `cas` → worker with zero cas tools | NEVER ADDRESSED | `cas factory doctor` CAS-MCP row is Codex-only (`cas-cli/src/cli/factory/doctor.rs:179`); preflight checks only the project `.mcp.json` `cas` entry and caller-side live observation (`cas-cli/src/factory_preflight.rs:609-660,1236-1244`), not Grok's own enable/disable or org-policy state; cas search "grok MCP discovery preflight" → no open task |
| 0.2.112 | L687 | Version policy: hard startup requirements | Distinguish Grok version gate from Cassy lifecycle failure | NEVER ADDRESSED | preflight only compares receipt vs default version (factory_preflight.rs:877-935); no hard-gate detection; low |
| 0.2.112 | L693 | Provider env headers + shell-variable allowlist | Could strip CAS_* from tool subprocesses | NEVER ADDRESSED | receipt ran default config only; no allowlist test/task; low |
| 0.2.112 | L704 | /resume defaults, title resume, fork copying | UUID-keyed transcript lookup | ADDRESSED | receipt `session_uuid` |
| 0.2.112 | L710 | Remote-client terminal output recorded | Transcript/liveness | ADDRESSED | receipt `transcript_and_liveness` |
| 0.2.112 | L714 | Plugin subagents inherit parent MCP tools | Grok subagents need cas__ too | NEVER ADDRESSED | no Grok-subagent MCP test; `git log -i --grep='grok.*subagent'` → only cas-8888 phase commits; low |
| 0.2.112 | L718 | Hooks definable in config.toml | Hook layering | ADDRESSED | test disables compat hooks via `GROK_CLAUDE_HOOKS_ENABLED=false` + `grok inspect` (`crates/cas-mux/tests/grok_factory_contract_runtime.rs:387-395`); receipt hooks pass (native config.toml hooks not exercised) |
| 0.2.112 | L723 | Workflow overlays/resume | Not Cassy authority | NEVER ADDRESSED | no follow-up; low |
| 0.2.112 | L733 | Queued prompt edit; repeated identical calls stop silently | Factory-message visibility | NEVER ADDRESSED | no follow-up; low |
| 0.2.105 | L794 | Default Grok 4.5 + effort vocabulary | Unpinned workers inherit new defaults | ADDRESSED | receipt `model_and_reasoning_effort` (grok-4.5, medium) |
| 0.2.105 | L800 | Shell tools see login-shell env | Could override CAS_* identity | ADDRESSED | receipt `factory_identity_environment` |
| 0.2.105 | L806 | Global ~/.grok/rules discovered | Competes with --rules | ADDRESSED (partial) | receipt `worker_role_rules` (payload in system_prompt.txt); no conflicting global-rules fixture |
| 0.2.105 | L816 | Background tasks/fleet roster fixes | Not Cassy roster | NEVER ADDRESSED | no follow-up; low |
| 0.2.105 | L821 | Long-session compaction fix; verify rules/identity survive real compaction | Long factory workers compact; losing role would derail | NEVER ADDRESSED | no Grok compaction test (`grep -i compact grok_factory_contract_runtime.rs` = 0; `git log --all -i --grep=compaction` grok hits=0); not in cas-ef93 AC |
| 0.2.104 | L834 | Background counts move to status line, not transcript | Long background work may look idle → false wedged | NEVER ADDRESSED | receipt covers a 22 s turn only; no long-background liveness test; cas-921f min-of-ages is partial mitigation |
| 0.2.104 | L839 | Idle-session auth recovery | Worker longevity | NEVER ADDRESSED | `cas-cli/src/factory_auth_health.rs` has Grok only as label (:197); no follow-up; low |
| 0.2.101 | L867 | `grok inspect` multi-harness settings (opportunity) | Debug compat layers after upgrade | ADDRESSED | used in conformance test `grok_factory_contract_runtime.rs:387-395` |
| 0.2.101 | L879 | Queued Enter messages appear immediately | Supervisor→worker injected turns | NEVER ADDRESSED | cas-5c02 comm probe (db27ec44): Grok busy-urgent trial BLOCKED, W2S deliver FAIL; report only on epic cas-04a6 branch (`git merge-base --is-ancestor db27ec44 HEAD` → not-in-HEAD) |
| 0.2.100 | L897 | Cross-harness session picker (note for onboarding docs) | Host UX only | NEVER ADDRESSED | no docs mention (grep docs for grok+picker/resume = 0); low |
| 0.2.100 | L905 | Queue/Enter during running turn; smoke after big bumps | Mid-turn coordination delivery | NEVER ADDRESSED | same as L879; no "message during running turn" smoke in 1.0.5 receipt (inject only to idle worker, test :550) |
| 0.2.100 | L911 | No crash printing resume hint after pane closed | Could look like worker death on shutdown | NEVER ADDRESSED | `git log --all -i --grep='resume hint'` = 0, `'pane.closed'` = 0; low |
| 0.2.100 | L920 | Claude/Cursor hooks honor disabled-at-start | Hooks posture | ADDRESSED | receipt `compatible_hooks_disabled`; test sets GROK_CLAUDE_HOOKS_ENABLED=false |

## Provenance

- **Markdown source:** `docs/reports/2026-09-23-harness-diary-report.md`; HTML beside it; concept
  brief `2026-09-23-harness-diary-report.brief.md`.
- **Commit examined:** `4929bf38` (epic branch after the Grok sweep merge). The diff base is
  `main` at `f2af1fc7`.
- **Installed versions checked 2026-09-23:**
  - `claude --version` → 2.1.280
  - `codex --version` → codex-cli 0.156.0
  - `grok --version` → 1.0.40 (eb1a2256660d)
- **Task statuses read 2026-09-23 (~12:10–12:20 UTC):** `cas-0d4f` in progress; `cas-ef93` in
  progress; `cas-a073` closed and delivered; `cas-8505` in progress.
- **Sweep counts:** `git diff main...HEAD -- docs/notes/*-changelog-diary.md`, first verdict glyph
  per added entry bullet.
- **Audit working files:** `/home/pippenz/.cas/artifacts/cas-df20/`.
