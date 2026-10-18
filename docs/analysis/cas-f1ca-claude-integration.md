# Claude integration research for Cassy 3.64

Keep task authority in Cassy; adapt native execution.

Research cutoff: 17 October 2026, 21:30 UTC. Cassy source:
`5e4774024045f19adaddf8da0e7ac1e69d19a9ad`. This is a recommendation, with
high confidence in the documented distinctions and source mapping, and no new
runtime validation. It creates no dependency on the 3.63 release.

## Recommendation

Prefer the existing backend and delivery adapters, with canonical role content
projected into each supported native format and capability evidence bound to
the actual launch. Keep assignment, task start, durable messages, verifier
dispatch and independent QA in Cassy. A native role, task, goal, permission
approval or successful write to a transport does not grant that authority.

| Layer | Existing Cassy seam | Recommended responsibility |
| --- | --- | --- |
| Authority | Task lifecycle, verifier and QA services | Assignment, leases, authenticated dispatch and reviewed-tip acceptance |
| Native execution | Backend, role assets, hook encoding and delivery adapters | Harness-specific fields, modes, launch prerequisites and transport |
| Evidence | Conformance receipts and delivery observations | Version, channel, effective policy/trust, loaded behavior and actual turn; missing facts stay unknown |

This is a preferred structure under Cassy's existing constraints, not a measured
claim of the fastest or cheapest implementation.

## Decision context

The 3.64 supervisor decides which follow-ups to authorize after this research.
This research feeds the cas-558a diary and the 3.64 plan. The merged
[cas-9ca1 Codex research](2026-10-17-cas-9ca1-codex-integration-research.md)
is a comparison source; the diary and plan are the only planned consumers.
Hook/trust canaries remain with cas-d0fe, and native role/capability work remains
with cas-6c54. This task changes none of those implementations and starts no
account, connector, harness or factory session.

### Version and source evidence

| Observation | Value at retrieval | Primary source |
| --- | --- | --- |
| Latest published non-prerelease | 2.1.302, published 16 October 2026 at 20:53:06 UTC | [Official release](https://github.com/anthropics/claude-code/releases/tag/v2.1.302) |
| npm latest | 2.1.302 | [Official package metadata](https://registry.npmjs.org/@anthropic-ai/claude-code/latest) |
| Native latest channel | 2.1.302 | [Latest feed](https://downloads.claude.ai/claude-code-releases/latest) |
| Native stable channel | 2.1.292 | [Stable feed](https://downloads.claude.ai/claude-code-releases/stable) |
| Installed executable target | `~/.local/share/claude/versions/2.1.302` | Local symlink inspection only; no CLI invocation |
| Retained Cassy conformance | 2.1.280, dated 23 September 2026 | `crates/cas-pty/conformance/claude-code-2.1.280-2026-09-23.json` |

Freshness recheck on 18 October 2026 confirmed the official release API and npm
latest remain 2.1.302, while the native latest and stable feeds remain 2.1.302
and 2.1.292. No release was published after the reviewed 2.1.302 entry. The
four endpoint timestamps are recorded as separate metadata rows in the coverage
CSV. This check updates release/channel currency only; the rolling documentation
page reads remain explicitly dated 17 October.

Latest non-prerelease and the native updater's stable channel are different
claims. Official setup documents separate channels and version floors. Keep a
user's chosen channel and pins; do not silently switch or downgrade it. The
installed pathname is not a new execution receipt. [Setup](https://code.claude.com/docs/en/setup#configure-release-channel)

The official index contains 275 link positions and 239 unique URLs: 228 English
pages and 11 translation indexes. Review covers 35 pages: three full reads and
32 scoped reads. Twelve attempted pages were unavailable, two were fetched but
not reviewed, and 190 remain inventory-only. The complete URL/date/scope ledger
is [cas-f1ca-claude-coverage.csv](cas-f1ca-claude-coverage.csv), including exclusions.
Current documentation is rolling, not an exact-version runtime contract.
[Documentation index](https://code.claude.com/llms.txt)

## Options

These are qualitative engineering judgments, not estimated costs or benchmarks.

| Option | Cost and effort | Risk | Reversibility | Outcome |
| --- | --- | --- | --- | --- |
| **Existing adapters plus canonical native projections — recommended** | Maintain concrete serializers and scoped evidence; extend existing owners' seams | Native mode and trust differences still need proof | Preserve current backend and PTY fallback | Common task policy, native execution where supported |
| Native teams/workflows own coordination | Integrate another task and recovery model | Native completion, replay and lead approval differ from Cassy authority | State migration is harder once ownership moves | Attractive for bounded Claude work, unsuitable as the durable factory authority |
| Universal session/connector engine | New registry, policy, identity and protocol framework | Speculative interfaces hide different authority and failure modes | Large shared interface to unwind | Defer until multiple demonstrated callers need it |

## Why this one

The deciding criterion is preserving durable authority while using supported
native capabilities. Three independent source-only proposals converged on that
criterion: a narrow delivery interface; shared role delivery for ordinary launch
and asset reconciliation; and separate launch, hook and proxy policy contracts.
Their useful parts fit existing modules. They do not justify one universal
engine or a new connector option whose every use would fail.

`Backend` already varies launch and cancellation behavior
(`crates/cas-mux/src/backend/mod.rs:58`); `HookOutput` owns differing wire shapes
(`crates/cas-core/src/hooks/types.rs:49`); proxy policy authorizes before
forwarding (`crates/cas-mcp-proxy/src/lib.rs:1430`). Keep those responsibilities
separate. The existing verifier body macro demonstrates shared content with
native tool-name projection (`cas-cli/src/builtins.rs:62`).

### Adopt, test or defer

**Adopt** means a recommended 3.64 design direction, not a shipped change.
**Test** means owner-approved evidence is required before adoption; no new tests
or probes ran here. **Defer** names a limitation rather than an implementation.

| Decision | Evidence and Cassy seam | Bounded next step |
| --- | --- | --- |
| **Adopt: contextual capability evidence** | Static `supports_hooks`/`supports_subagents` live in `crates/cas-mux/src/harness.rs:42`. The latter directly selects verification mode in `cas-cli/src/harness_policy.rs:231`. Existing `HarnessConformanceReceipt` carries version and evidence at `crates/cas-pty/src/conformance.rs:73`. | cas-6c54 should distinguish documented support, effective disable/trust and executed proof by backend/version/mode. Keep independence policy explicit; a boolean change must not silently change review authority. |
| **Test: hook context and failure behavior** | Current [Stop output docs](https://code.claude.com/docs/en/hooks#stop) permit continuation context; Cassy's type documentation excludes Stop variants and uses `systemMessage` (`crates/cas-core/src/hooks/types.rs:364`, `:762`). [2.1.294](https://github.com/anthropics/claude-code/releases/tag/v2.1.294) and [2.1.295](https://github.com/anthropics/claude-code/releases/tag/v2.1.295) also change hook behavior. | Feed the concrete discrepancy to cas-d0fe. Establish exact CLI payload, model-visible context, malformed output/timeout behavior and bounded continuation. Documentation/source disagreement is not a demonstrated runtime failure. |
| **Test: native messaging beneath durable delivery** | Native [cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging#message-delivery) distinguishes idle turn start from busy delivery. Cassy separates attention evidence (`message_attention.rs:72`) and delivery (`delivery.rs:1142`) under `cas-cli/src/ui/factory/daemon/runtime/`. | Keep handoff, surfaced, turn-start, reaction and ACK separate. Prove supported framing, recipient generation, held/refused outcomes, duplicate retry and composer safety before an optional adapter. The docs do not establish a complete stable daemon-client frame protocol. |
| **Adopt: keep wake authorization central** | `queue_and_events.rs:6622` classifies authenticated lifecycle traffic; `:6709` keeps ordinary worker chatter inbox-only. Native messaging can wake an idle session. | Revalidate the existing queue row and current authority before a wake-capable route. A native hold/refusal must not trigger a fallback that circumvents it. A hook/tool result alone proves no idle turn. |
| **Adopt: one role body, mode-specific projection** | Native [subagent definitions](https://code.claude.com/docs/en/sub-agents) and [teams](https://code.claude.com/docs/en/agent-teams) differ. With teams enabled, named Agent calls may become teammates; teammates ignore definition skills, and some modes ignore MCP/compaction fields. Cassy already shares verifier content at `builtins.rs:62`. | cas-6c54 owns the role seam. Preserve explicit model/tool choices and user overrides; distinguish factory, subagent, fork and teammate modes. Verify actual loading and invocation, not only file presence. |
| **Adopt: independent verification remains outside native roles** | `verification_tools.rs:258` requires a registered supervisor or distinct sealed verifier child; `qa_pass_ops.rs:76` rejects self-review. Native teammate plan approval is automatically granted in the lead session; it supplies no independent review. [Teams](https://code.claude.com/docs/en/agent-teams) | Treat native role completion as an observation. Preserve assigned-task start, sealed dispatch, separate reviewer identity and exact reviewed head; do not accept a role label or native completed state as authority. |
| **Test: worktree provenance and recovery** | Native [worktrees](https://code.claude.com/docs/en/worktrees) have base, instruction-source and cleanup semantics. Cassy's cwd/trust seam is `backend/claude.rs:85`, with worktree binding at `crates/cas-pty/src/pty.rs:1250`. | Prove source branch, effective instruction root, hook cwd, resume/deletion and shell-write behavior. Native isolation is not automatically Cassy delivery-branch ownership. |
| **Adopt: portable skill content, explicit native field limits** | [Skills](https://code.claude.com/docs/en/skills) have invocation, context and permission lifetimes. `context: fork` is not a full conversation fork. Stored fields live at `crates/cas-types/src/skill.rs:249`; reference reconciliation at `builtins.rs:2770` preserves custom content. | Reuse canonical content and the existing history/baseline ledger. Test old shipped content, custom edits, duplicate names and unsupported fields. Do not treat temporary `allowed-tools` or `disallowed-tools` as a durable cross-harness security policy. |
| **Adopt: durable knowledge outside native memory and compaction** | Native [auto memory](https://code.claude.com/docs/en/memory#auto-memory) is scoped to local configuration/repository identity; worktrees can share it, ordinary subagents do not inherit the main memory, and compaction can lose early instructions. [Checkpoints](https://code.claude.com/docs/en/checkpointing) exclude some shell, external and subagent edits. Cassy persists knowledge through `cas-cli/src/mcp/tools/core/memory.rs:454`. | Keep durable task checkpoints and accepted knowledge in Cassy. Preserve the user's native memory settings; test instruction reload and compact/resume against the exact mode. Native summary or rewind is neither a complete delivery checkpoint nor independent proof of filesystem state. |
| **Test: account intent versus effective authentication** | Explicit account precedes requester account in `factory_ops.rs:215`; `pty.rs:2543` removes selected key/OAuth overrides. Official [authentication precedence](https://code.claude.com/docs/en/authentication#authentication-precedence) also includes provider, gateway and helper choices. | Audit with synthetic selectors first, then authorized-owner proof where needed. Do not read credentials or broaden stripping in this research. Account selection grants neither connectors nor proxy routes. |
| **Adopt: strict native scope plus proxy authorization** | Claude's strict private configuration and denial refusal are at `backend/mod.rs:208–240`. Proxy decisions check routes/roles before forwarding (`cas-mcp-proxy/src/lib.rs:230`, `:334`). [Managed MCP](https://code.claude.com/docs/en/managed-mcp) can make a strict launch incompatible with administrator policy. | Preserve GH1047 and the operator's Neon/Vercel CLI-only directive. Explain incompatibility; never remove strict flags or clear denials silently. Discovery visibility and a declared read route do not authorize a call by themselves. |
| **Defer: selective native connector contract** | Native-fetched, cloud-delivered and desktop in-process [connectors](https://code.claude.com/docs/en/mcp#how-connectors-reach-claude-code) have different enforcement paths. Display names are not secure identities; ordinary allowlists merge. [Managed controls](https://code.claude.com/docs/en/managed-mcp#policy-based-control-with-allowlists-and-denylists) | cas-16c0 retains the dedicated-owner/interface question. Require exact identity, operation, transport and enforcement proof. Do not add the rejected always-refused request field, copy profiles, or infer mailbox authority from an explicit directory. |
| **Test: bounded workflows; defer scheduler replacement** | [Workflows](https://code.claude.com/docs/en/workflows) can replay work after interruption. [Native schedules](https://code.claude.com/docs/en/scheduled-tasks) depend on session lifetime and have missed-fire/expiry behavior. CAS reminder semantics live at `factory_remind.rs:95`, `:435`. | Pilot bounded read-only fan-out inside one assigned task. Keep durable reminders, restart ownership and completion in Cassy. Native success or a restarted transcript is not proof that side effects occurred exactly once. |
| **Defer: goals as completion authority** | [/goal](https://code.claude.com/docs/en/goal) evaluates the conversation and can continue turns, pause, fail or clear. Its evaluator does not independently run checks or inspect files. | A goal may assist one authorized task after separate proof; it cannot replace a verifier, clean blocked/done stop, artifact receipt or external wake policy. Do not add unconditional keep-running behavior. |
| **Defer: SDK replacement of the interactive backend** | Official [streaming input](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode) supports persistent input, unlike one-shot query. [SDK hooks](https://code.claude.com/docs/en/agent-sdk/hooks) have language/async differences; [SDK overview](https://code.claude.com/docs/en/agent-sdk/overview) constrains authentication routes. | An SDK route needs its own supported auth, session, interruption and trust design. Python callback lifecycle support is not identical to TypeScript. Do not move existing subscription-profile PTYs to SDK execution by assumption. |
| **Defer: plugin/mod packaging migration** | [Plugin loading](https://code.claude.com/docs/en/plugins/loading) depends on provenance, trust, sync, settings and names; [manifest components](https://code.claude.com/docs/en/plugins/manifest-reference) have replacement/merge rules. Current worker traffic settings can suppress account sync. | Keep managed builtin reconciliation until a concrete packaging benefit is demonstrated. A plugin can execute code; tool rules are not a sandbox for its own process. No install, profile mutation or plugin experiment is authorized here. |

### Release window reviewed

Every top-level bullet in all 15 published entries from 2.1.286 through the
installed/latest 2.1.302 was reviewed: **1,266 bullets**. This includes the native
stable boundary at 2.1.292. Counts were cross-checked between official release
bodies and the official changelog. The summaries below select Cassy implications;
they do not claim all bullets require changes.

| Version | Bullets | Relevant additions and disposition |
| --- | ---: | --- |
| [2.1.286](https://github.com/anthropics/claude-code/releases/tag/v2.1.286) | 88 | Resume, mixed-output hooks and agent/workflow delivery fixes merit compatibility checks. Bare mode removes background/reminder capabilities and narrows discovery; do not substitute it for normal factory launch without mode-specific proof. |
| [2.1.287](https://github.com/anthropics/claude-code/releases/tag/v2.1.287) | 106 | Mod extension points arrive alongside async-rewake, heartbeat, background reply and plugin-startup fixes. Test the existing event/output path; defer adopting mods as a replacement for Cassy hooks or delivery. |
| [2.1.288](https://github.com/anthropics/claude-code/releases/tag/v2.1.288) | 89 | Held/delivered distinctions, hook fail-closed fixes, background identity and attended/headless timeout differences matter. Keep transport evidence truthful and test each execution mode separately. |
| [2.1.289](https://github.com/anthropics/claude-code/releases/tag/v2.1.289) | 27 | Mostly mod/plugin stabilization. Teammate event identity may aid observation, but is not CAS task authority or proof that native coordination is an equivalent replacement. |
| [2.1.290](https://github.com/anthropics/claude-code/releases/tag/v2.1.290) | 190 | Hook identity/permission ceilings, schedule restore, message replay and rewritten-input checks affect integration. Preserve holds and permissions; do not infer delivery or side-effect guarantees from version order. |
| [2.1.291](https://github.com/anthropics/claude-code/releases/tag/v2.1.291) | 2 | Repairs cloud permission replies and latest-message persistence on shutdown. Relevant reliability improvements; no new native role/orchestration interface to adopt. |
| [2.1.292](https://github.com/anthropics/claude-code/releases/tag/v2.1.292) | 92 | Invocation-specific effort, marketplace behavior and extensive policy/background/session fixes. This is the observed stable update channel; newer features need their own applicability evidence. |
| [2.1.293](https://github.com/anthropics/claude-code/releases/tag/v2.1.293) | 56 | Compaction, queued messages, agent metadata and managed plugins change; a cloud wakeup recovery change is reverted. Scheduling durability must be established directly rather than assumed monotonic. |
| [2.1.294](https://github.com/anthropics/claude-code/releases/tag/v2.1.294) | 2 | Repairs instruction hooks that allowed intended denials and improves Stop/SubagentStop continuation interpretation. Feed owned canaries; this upstream fix is not a Cassy acceptance receipt. |
| [2.1.295](https://github.com/anthropics/claude-code/releases/tag/v2.1.295) | 143 | Fail-closed hook handling, retries, headless output, MCP reconnection and skill-result/lifetime fixes. Test timeout/malformed-output policy and native role result routing before adoption. |
| [2.1.296](https://github.com/anthropics/claude-code/releases/tag/v2.1.296) | 79 | Subagent compaction windows, workflow model choice, managed hooks, headless MCP and startup ordering change. Test per-mode field applicability; new diagnostics do not prove actual loading. |
| 2.1.297 / 2.1.298 | — | No sections in the official changelog, and official release-tag API returned 404 at retrieval. Source gaps, not invented contents or a claim that an unpublished feature exists. |
| [2.1.299](https://github.com/anthropics/claude-code/releases/tag/v2.1.299) | 190 | Policy-before-MCP-start, name matching, connector redirect, hook serialization and managed precedence hardening. Keep route parsing, launch filtering and execution authorization separate. |
| [2.1.300](https://github.com/anthropics/claude-code/releases/tag/v2.1.300) | 88 | Rewritten hook inputs receive normal validation; MCP, account, worktree and headless resume improve. Test exact payload validation and recorded context; no new runtime claim follows. |
| [2.1.301](https://github.com/anthropics/claude-code/releases/tag/v2.1.301) | 51 | Classifier telemetry and unattended waits expand; skipped workflow success, cleanup and identity races are fixed. Preserve truthful skipped/failed outcomes and bounded waiting. |
| [2.1.302](https://github.com/anthropics/claude-code/releases/tag/v2.1.302) | 63 | Classifier model selection, MCP interoperability, queued config writes and nested/forked agent behavior change. Some headless defaults depend on route/configuration; pass intended mode explicitly and verify effective behavior. |

### Common structures with the Codex research

The merged [cas-9ca1 Codex report](2026-10-17-cas-9ca1-codex-integration-research.md)
and [practitioner brief](2026-10-17-cas-9ca1-codex-integration-research.brief.md)
independently recommend canonical Cassy authority, native projections and
contextual evidence. The crosswalk links shared decisions to the counterpart's
source-specific findings; it does not combine the two coverage counts or imply
that Codex runtime observations validate Claude behavior.

| Shared area | Claude decision here | Codex counterpart |
| --- | --- | --- |
| Durable authority and independent review | **Adopt:** task start, identity, verifier dispatch and independent QA remain Cassy-owned; native completion is an observation. | [Roles, skills and independent verification](2026-10-17-cas-9ca1-codex-integration-research.md#roles-skills-and-independent-verification) and [adopt/test/defer matrix](2026-10-17-cas-9ca1-codex-integration-research.md#adopt--test--defer-matrix) reach the same boundary for native roles and SubagentStart. |
| Effective capabilities, trust and policy | **Adopt/Test:** collect mode-specific evidence; test exact hook behavior and preserve operator trust/disablement. | [Lifecycle and trust](2026-10-17-cas-9ca1-codex-integration-research.md#lifecycle-and-trust-adopt-a-bounded-projection) and [native readers](2026-10-17-cas-9ca1-codex-integration-research.md#native-readers-preserve-unknowns) distinguish support, requested policy and effective state. |
| Shared roles and portable skills | **Adopt/Test:** one canonical body with native projection; test mode-specific loading, field limits and user overrides. | [Roles and skills](2026-10-17-cas-9ca1-codex-integration-research.md#roles-skills-and-independent-verification) recommends TOML projection while retaining unowned files and testing discovery. |
| Messages, state and interruption recovery | **Adopt/Test/Defer:** keep durable memory in Cassy; test message delivery and compact/resume behavior; defer scheduler replacement and goal authority. | [Transport and background execution](2026-10-17-cas-9ca1-codex-integration-research.md#transport-and-background-execution-test-before-replacement) and [state, compaction, account and profile](2026-10-17-cas-9ca1-codex-integration-research.md#state-compaction-account-and-profile-boundaries) preserve the same delivery and recovery boundary. |
| Account, worktree and MCP/connector policy | **Test/Defer:** test resolved account/trust context; retain Cassy's route authorization; defer native connector or SDK replacement. | [State and account boundaries](2026-10-17-cas-9ca1-codex-integration-research.md#state-compaction-account-and-profile-boundaries) and the [decision matrix](2026-10-17-cas-9ca1-codex-integration-research.md#adopt--test--defer-matrix) retain identity and enforcement gaps. |

A common caller should describe an assigned operation and required safeguards,
not construct native JSON, socket paths or role frontmatter. Reuse existing
launch inputs and conformance records before adding interfaces. A future role
delivery plan can share the canonical body while reporting unsupported native
fields; delivery should accept a durable message ID and return the observed
attempt/evidence. Do not combine launch, account selection, message delivery and
authorization in one module.

For example, an already-assigned task requests a specialist through the existing
backend with its resolved model, tools and workspace. The native adapter prepares
the appropriate role assets and launch. Successful loading is evidence only;
a child owning assigned implementation work still needs its genuine CAS
identity and authorized task start. A task-verifier needs its separate sealed
dispatch. Bounded research delegation can remain within the parent's task
ownership. A message reply updates delivery evidence without changing task
ownership or independently closing work.

The deletion test is concrete: a shared role projection earns its place only
when deleting it redistributes body/tool-prefix/field/migration rules into both
sync and launch callers. A delivery seam earns its place only when it removes
duplicate generation, hold, retry and receipt logic from actual callers. A
wrapper around one function or a speculative connector registry does not.

## What we give up

The strongest alternative is native-first orchestration: teams, workflows,
goals and streaming sessions can reduce manual prompt and context plumbing.
That is a useful pilot hypothesis for bounded work. It also inherits native
replay, feature rollouts, scope-dependent fields and session persistence. Keep
its potential benefit separate from an unmeasured speed/cost claim and from
CAS assignment or independent review.

## Reversal cost

Preserve the current backend and durable records while a native adapter is
evaluated. Disable a new execution route without moving task ownership,
rewriting live profiles or converting the queue. Keep canonical role bodies and
managed-file provenance so an adapter rollback preserves user customization.
Moving durable ownership into native tasks would make that reversal materially
harder; no such migration is recommended.

## Confidence, gaps and next evidence

- **Documented and source-observed:** release/channel metadata, the stated docs
  scopes, concrete source consumers and the retained 2.1.280 receipt. That receipt
  explicitly excludes cloud-skill precedence and long-running MCP backgrounding.
- **Not exercised here:** 2.1.292/302 conformance, Stop-context handling, native
  idle wake/framing, plugin/skill precedence, team field behavior, selective
  connectors, SDK auth/session behavior and effective live account policy.
- **Existing owner evidence:** cas-d0fe's 21:15 UTC note records a Codex 0.163.0
  SessionStart context and PreToolUse matching canary. Stop and MCP execution
  were not exercised there. This is narrowly attributed owner evidence, not a
  new Claude receipt or proof of the separate idle-wake issue.
- **Unavailable pages:** direct `.md` retrieval returned 403 for commands, tools
  reference, environment variables, feature availability, configuration debug,
  agents overview, directory layout, context window, prompt caching, SDK
  structured output, plugin components and server-managed settings. Goal and
  SDK streaming-input text were recovered separately through Exa and reviewed
  within their listed scope. No missing page is silently marked reviewed.
- **Retrieved but unreviewed:** best-practices and common-workflows pages. Other
  English pages and translation indexes remain inventory-only in the coverage
  CSV; language variants were not recursively traversed.

Implementation owners should reuse authorized conformance work, then prove
loaded identity/trust, required capability, actual output and failure behavior
for the precise operation. Local fixtures can establish serialization and
policy decisions; the external CLI needs a bounded owned run. No new runtime
PASS, product tests, builds, live configuration changes or 3.63 gate are claimed.

### Provenance

Official sources were retrieved on 17 October 2026, with per-URL UTC timestamps
in the coverage CSV. Public retrieval used Exa and HTTPS GET; no installer was
executed. The official installer script itself returned 403 and was not used
as evidence. The correct npm latest endpoint succeeded after an invalid
dist-tag endpoint returned 404. Raw retrieval logs and reviewed source snapshots
remain in the cas-f1ca task artifacts; the portable bundle contains research
and receipts, not a republished full documentation corpus.

Source anchors are pinned to Cassy commit
`5e4774024045f19adaddf8da0e7ac1e69d19a9ad`. Review used `rg`, scoped file reads,
official release JSON bodies and changelog sections. Three independent research
passes reviewed 500, 374 and 392 release bullets respectively and compared
minimum, common-caller and flexible interface alternatives. Documentation
sections were reviewed fully only where the CSV explicitly says so. See the
adjacent brief for report rendering and QA evidence; none of those browser
checks are a Claude/Codex product conformance run.
