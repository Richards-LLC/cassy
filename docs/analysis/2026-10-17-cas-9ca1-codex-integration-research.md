# Adopt native Codex projections within Cassy authority

Use native lifecycle and role projections within Cassy's ownership; validate transport replacements before adoption. Confidence: high in the bounded structure, moderate in unmeasured adapter benefits.

| Evidence for native projections | Counter-evidence requiring a boundary |
| --- | --- |
| Executed: SessionStart context and Stop continuation/re-entry work on 0.163.0. | MCP-name matching was exercised; MCP execution enforcement was not. |
| Documented: custom agents, skills and structured turn events provide native integration seams. | Native roles and turn events do not establish a Cassy assignment or independent reviewer. |
| Pinned source: configuration and managed-requirement readers expose useful context. | No complete supported resolved tool/role reader was found; app-server production support is limited. |

Figure source: official guides and pinned 0.163.0 source reviewed 2026-10-17; peer cas-d0fe immutable canaries at 21:13Z and 21:17Z. This is an evidence comparison, not a performance measurement.

## Sources and scope

Research date: **2026-10-17 UTC**. Cassy tree examined: **5e4774024045f19adaddf8da0e7ac1e69d19a9ad**, on the 3.64 epic. This report creates no 3.63 dependency and changes no product source, runtime, credentials or configuration.

Independently queried [OpenAI's latest stable release](https://api.github.com/repos/openai/codex/releases/latest): **0.163.0**, published **2026-10-15T21:30:38Z**, neither draft nor prerelease. The annotated tag resolves to source **83086188ae0614bd882818ff1aa23f3194962012**. Published stable, installed binary, configured channel and demonstrated operation remain separate evidence fields.

The [official Codex index](https://developers.openai.com/codex/llms.txt) redirected to [the current documentation index](https://learn.chatgpt.com/docs/llms.txt). Its 199 linked entries were inventoried. Twenty-five priority pages were fetched with URL, HTTP status and retrieval time; 23 received scoped substantive reads and two were only fetched/indexed. This is a coverage ledger, not a claim to have read every current page. Live documentation has no version guarantee and can lead a stable binary.

| Source / retrieval date | Evidence type | Reliability boundary |
| --- | --- | --- |
| [Official latest-release API](https://api.github.com/repos/openai/codex/releases/latest) / Oct 17 | Published stable metadata and annotated tag | Authoritative publication state at retrieval; not installed state. |
| [Official release notes](https://github.com/openai/codex/releases) / Oct 17 | Seven stable bodies, 0.159.3–0.163.0 | Upstream descriptions; not an executed Cassy regression run. |
| [Official docs index](https://learn.chatgpt.com/docs/llms.txt) and 25 linked pages / Oct 17 | Current supported shapes and stated maturity | Live, unversioned; scoped reads and omissions below. |
| [Pinned upstream source](https://github.com/openai/codex/tree/83086188ae0614bd882818ff1aa23f3194962012) / Oct 17 | Stable implementation and generated schemas | Strong for inspected paths; not a full source audit. |
| Cassy base 5e477402 / Oct 17 | Existing integration seams | Static source evidence, not runtime behavior. |
| cas-d0fe immutable canaries / Oct 17, 21:13Z and 21:17Z | Executed 0.163.0 observations | Exact controls only; MCP execution and full conformance unexercised. |
| cas-6c54 reader map and cas-558a parity matrix / Oct 17 | Peer source analysis and historical conformance provenance | Reused, attributed evidence; pending implementations are not claimed complete. |

Primary guides used below: [hooks](https://learn.chatgpt.com/docs/hooks), [subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents), [skills](https://learn.chatgpt.com/docs/build-skills), [app-server](https://learn.chatgpt.com/docs/app-server), [configuration basics](https://learn.chatgpt.com/docs/config-file/config-basic), [advanced configuration](https://learn.chatgpt.com/docs/config-file/config-advanced), [configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference), [MCP](https://learn.chatgpt.com/docs/extend/mcp), [authentication](https://learn.chatgpt.com/docs/auth), [exec](https://learn.chatgpt.com/docs/non-interactive-mode), and [developer commands](https://learn.chatgpt.com/docs/developer-commands?surface=cli). Each subsequent claim links its supporting primary source; local receipts identify executed evidence separately.

## Release coverage: every missing stable entry

Two official release API pages, 50 records each, reached older stable releases beyond the requested floor. Seven stable releases fall in 0.159.3–0.163.0. All release bodies and appended PR titles were triaged; upstream implementations were inspected only for the specific precedence and reader questions below. The title inventory contains 661 entries: 444 keyword-selected candidates and 217 peripheral entries. These are triage counts, not audited-code counts.

| Stable / published UTC date | Cassy-relevant additions and fixes | Applicability decision |
| --- | --- | --- |
| [0.159.3](https://github.com/openai/codex/releases/tag/rust-v0.159.3) / Sep 30 | Optional account security reminders for eligible ChatGPT local sessions. | Defer UI handling unless observed; account eligibility is contextual. |
| [0.160.0](https://github.com/openai/codex/releases/tag/rust-v0.160.0) / Oct 1 | Reconnect queue deduplication; launch/workspace permission restoration; server-supplied model/provider/history settings retained; subagent setup failures propagated; SQLite failures more visible; explicit catalogs authoritative. | Test delivery/admission and permission-resume adapters. Preserve explicit model selection. Command-center pagination and Linux X11 selection are UI changes, not delivery proof. |
| [0.160.1](https://github.com/openai/codex/releases/tag/rust-v0.160.1) / Oct 5 | Remote stdio MCP preserves required Windows environment variables. | Defer Windows-specific implementation here; keep remote environment forwarding in cross-platform conformance scope. |
| [0.161.0](https://github.com/openai/codex/releases/tag/rust-v0.161.0) / Oct 7 | Background work retains originating permissions; escalation retains denies/network policy; explicit launch permissions survive reconnect; active-session MCP login; authoritative resume history; paste-expiry fix; SQLite corruption backup; conditional Daybreak/Bedrock changes. | Test permissions, resume and PTY admission; do not change provider/model or infer universal eligibility. Voice/device settings are outside this Linux factory integration review. |
| [0.162.0](https://github.com/openai/codex/releases/tag/rust-v0.162.0) / Oct 8 | Managed worktrees; provider search/compaction capability overrides; CodeMode promise/tool-search changes; explicit model/effort preservation; sandbox path protections; lineage, queued-mail attribution and subagent delivery fixes. | Preserve Cassy worktree ownership; test model/tool exposure and queue identity. PRs #50427/#50458 truncate command/MCP history: do not treat retained history as a complete forensic log. #51482–#51493 change skill/capability-root identity and ownership. |
| [0.162.1](https://github.com/openai/codex/releases/tag/rust-v0.162.1) / Oct 9 | Multiline question hyperlink crash fix; daemon compatibility checks use explicit overrides rather than defaults. | Keep profile/default/explicit provenance distinct; no inference that every installed daemon is compatible. |
| [0.163.0](https://github.com/openai/codex/releases/tag/rust-v0.163.0) / Oct 15 | Live config/daemon controls; instant steering cancels nested work; parallel read-only tools; permission-aware cold resume; network proxy validation; thread-affine replies; bounded output draining; shell backend migration; conditional Daybreak changes. | Test cancellation, output and resume at the real transport seam. Appended PRs add experimental read state, async/speculative compaction, provisional children and connection-bound replies; none independently establish Cassy ACK, task ownership or durable reviewer authority. |

The 0.163.0 appendix also covers launch-cwd-independent global app-server configuration (#51872), subagent capability/fork separation (#52241), read state (#52337/#52350/#52395), runtime reset (#52825), retained outputs through compaction (#52937), cold permissions (#53150/#53419/#53424), compaction internals (#53595/#53606/#53614/#53620), hidden native tools with retained runtime (#53617), provisional children (#53654), cloned hook views (#53657), MCP launcher/sandbox separation (#53709) and execution-server capability discovery (#53723). These are applicability leads, not evidence that Cassy's current adapters use them correctly.

## Lifecycle and trust: adopt a bounded projection

The [hook guide](https://learn.chatgpt.com/docs/hooks) documents merged matching hooks across active configuration layers; a higher layer does not simply replace lower hooks. Project trust can exclude project layers while user/system hooks remain. Exact hook-definition trust and managed restrictions are separate controls. `features.hooks=false`, managed feature pins and managed-only hook policy must remain effective operator constraints; a supported feature does not authorize enabling it.

Tool matching covers shell aliases, file-edit aliases and MCP tool names, with stated opt-outs and hosted-tool limitations. A PreToolUse denial is useful only when the handler emits a supported decision. Unsupported/malformed output and some handler errors continue execution; MCP hook failures/timeouts are documented as fail-open. These mechanisms cannot be the only authority boundary. Auto-review failures have a different, fail-closed contract in the [security guide](https://learn.chatgpt.com/docs/agent-approvals-security#automatic-approval-reviews); the two failure policies must not be conflated.

**Executed peer evidence:** cas-d0fe's immutable `canary/results/20261017T211351Z/ANALYSIS.md`, using codex-cli 0.163.0 with model gpt-6.1-sol, proves SessionStart ran and its developer context was consumed, and Bash/MCP-name PreToolUse matching fired before approval. MCP execution itself was refused by the canary's approval policy and remains unexercised. This task launched no model session.

The discriminating `canary/results/stop-20261017T211715Z/ANALYSIS.md` proves trusted Stop blocking caused continuation, a subsequent `stop_hook_active=true` stopped cleanly, and an untrusted UserPromptSubmit control stayed silent while a trusted hook ran. Its `hooks/list` receipt shows matcher-ignored events omit the matcher from trust identity; Cassy's existing helper always included an empty matcher. This resolves the earlier silent-Stop ambiguity. It does not reproduce every historical operator trust prompt.

At Cassy base, `crates/cas-pty/src/codex_trust.rs` pins older hash behavior, `cas-cli/src/cli/hook/config_gen.rs` generates tool hooks, and `crates/cas-mux/src/backend/codex.rs:21` reports static hook support false. **Adopt the adapter direction through cas-d0fe**, preserving exact trust, existing user files and explicit disablement. Then verify installation/review and real failed/successful execution controls before claiming enforcement.

Async hooks are unsuitable for admission, policy or Stop continuation: the guide makes their output deferred to a safe model point, gives them no blocking/rewrite power and does not promise to wake an idle session. Keep blocking lifecycle paths synchronous and continuation bounded by the native re-entry field.

## Roles, skills and independent verification

The [subagent guide](https://learn.chatgpt.com/docs/agent-configuration/subagents) defines personal `~/.codex/agents/*.toml` and project `.codex/agents/*.toml`, with `name`, `description` and `developer_instructions`. Canonical Cassy role bodies can generate these projections without duplicating policy. Preserve unowned files/symlinks and intentional operator overrides. Role configuration can override model/effort selection; inherited approval, sandbox and provider constraints cannot be strengthened merely by describing an agent as read-only.

At base, `cas-cli/src/harness_policy.rs:231` derives required verification from static optional capability bits. `hooks/handlers/handlers_events/pre_tool.rs:1016` and `handlers_state.rs:645` bind a registered Task/Agent handoff to SubagentStart. A discoverable Codex role, spawn response or native turn is not that sealed registration. **Adopt cas-6c54's contextual capability separation and independent-review fallback**, subject to its implementation review. Never make independent review disappear because an optional native feature is unavailable; preserve explicitly authorized skip policy.

There is a concrete docs/stable-source discrepancy. The live guide describes `agents.enabled=false` as disabling multi-agent behavior. In [0.163.0 config resolution](https://github.com/openai/codex/blob/83086188ae0614bd882818ff1aa23f3194962012/codex-rs/core/src/config/mod.rs#L1637), enabled `multi_agent_v2` selects V2 before checking that boolean. The [pinned feature definitions](https://github.com/openai/codex/blob/83086188ae0614bd882818ff1aa23f3194962012/codex-rs/features/src/lib.rs#L1468) distinguish V1 `multi_agent` (stable, default true) and V2 (stable, default false). Consequently one flag is insufficient effective-availability evidence. Record requested disablement and report a conflict/unknown where the native effective state is unresolved; do not silently rewrite settings.

The [skill guide](https://learn.chatgpt.com/docs/build-skills) uses canonical `.agents/skills` discovery, including repository ancestors and personal scope. Equal names do not merge; `agents/openai.yaml` can disable implicit invocation while retaining explicit invocation. Dependencies metadata is not permission to install servers. Cassy currently shares legacy `CODEX_HOME/skills`; an app-server example also uses `.codex/skills`. **Test discovery and duplicate/override behavior before migration**, using owned projections rather than moving user content. [Custom prompts are deprecated](https://learn.chatgpt.com/docs/custom-prompts); cas-558a already owns the existing finish-skill/diary reconciliation, so this report requests no overlapping edit.

## Native readers: preserve unknowns

Prefer supported native evidence over a second implementation of CLI/project/profile/managed precedence. The [app-server reference](https://learn.chatgpt.com/docs/app-server) documents `config/read` with `cwd` and `includeLayers`, and `configRequirements/read` for constraints. The latter spelling matters. They reveal layered on-disk configuration and origins, not a complete callable-tool list for an active model turn. Feature-list state similarly does not establish every model's tool availability.

Pinned schemas confirm the limit: [ConfigReadParams](https://github.com/openai/codex/blob/83086188ae0614bd882818ff1aa23f3194962012/codex-rs/app-server-protocol/schema/json/v2/ConfigReadParams.json) supports cwd/layers; configuration permits additional fields; the fetched `ModelProviderCapabilitiesReadResponse` contains image-generation and web-search information, not an authoritative native-agent catalog.

The documented `debug prompt-input` is not a resolved-tools reader. In [the stable CLI](https://github.com/openai/codex/blob/83086188ae0614bd882818ff1aa23f3194962012/codex-rs/cli/src/main.rs#L1995), it builds configuration and performs authentication/Git/skill setup; [prompt_debug](https://github.com/openai/codex/blob/83086188ae0614bd882818ff1aa23f3194962012/codex-rs/core/src/prompt_debug.rs) returns input messages (`Vec<ResponseItem>`). No invocation was made here. No complete supported resolved role/tool reader was found in the reviewed docs/source, so that field stays **Unknown** rather than guessed.

Recommended capability evidence record: harness/channel, binary version and path, source tag/commit where known, cwd and state-home identity, requested model/profile/CLI overrides, trusted layers and managed requirements, operation, supported/disabled/unknown, evidence kind, receipt and timestamp. Redact secrets; distinguish configuration evidence, available-tool evidence and demonstrated execution. A model catalog or version alone never proves all three.

## Transport and background execution: test before replacement

The [app-server protocol](https://learn.chatgpt.com/docs/app-server) offers initialized connections, thread identity, typed items and turn completion statuses. `turn/start` acknowledgement, `turn/started`, completed items and `turn/completed` are different observations. `turn/steer` targets an expected active turn; it does not create a new turn. `thread/inject_items` persists history without starting a turn. Experimental thread read state is not a Cassy delivery ACK.

Cassy's current `ui/factory/daemon/runtime/queue_and_events.rs:3051` distinguishes normal delivery turn evidence from byte submission; `delivery_probe_jobs.rs:97`, mux pane completion and `commander_mirror.rs:166` already carry transport ownership. Route an experimental adapter through these seams instead of adding a second queue owner. Test UUID-targeted `codex queue`, reconnect/duplicate delivery, idle/busy states, turn attribution and failure controls with cas-353b. Queue acceptance alone cannot prove model consumption.

There is material counter-evidence to immediate replacement. The [command reference](https://learn.chatgpt.com/docs/developer-commands?surface=cli) calls app-server a development/debug surface that may change; the [removed Codex MCP-server guide](https://learn.chatgpt.com/docs/mcp-server) points at app-server while warning about experimental/production support. WebSocket transport is experimental. Use a private, version-pinned pilot with the existing PTY path retained. This task executed neither app-server nor queue delivery.

Do not confuse removal of Codex's own MCP-server mode with support for external MCP servers: Cassy's canonical `cs` stdio server is a Codex client integration. Keep `mcp__cs__` naming/policy identity in `crates/cas-mux/src/backend/codex.rs` and PTY launch identity in `crates/cas-pty/src/pty.rs:1473`. CodeMode/ranked tool search and native alias changes require execution-level policy tests before changing exposure.

Background/process APIs have separate scope and maturity. Some app-server command-execution methods run outside the agent sandbox; they are not interchangeable with policy-governed model tool calls. Experimental terminal list/terminate/clean operations require exact owned process identity. Releases improve permission retention, cancellation and late-output draining, but Cassy's complete saved conformance pin is still `crates/cas-pty/conformance/codex-cli-0.156.0-2026-09-23.json`. Reuse d0fe's narrow 0.163 receipts without presenting them as a complete replacement conformance run.

## State, compaction, account and profile boundaries

[Configuration precedence](https://learn.chatgpt.com/docs/config-file/config-basic) layers explicit CLI, trusted project context, selected profile, user/cloud/system settings and defaults under managed constraints. [Standalone named profile files](https://learn.chatgpt.com/docs/config-file/config-advanced#profiles) replace removed legacy inline profiles; this is not a new 0.163 migration. Project config cannot redirect all sensitive provider/authentication settings. The beta [permission profile](https://learn.chatgpt.com/docs/permissions) system has compatibility/precedence rules with older sandbox flags and requires an active proxy for domain restrictions. Do not switch Cassy's launch policy during this research task.

[Authentication storage](https://learn.chatgpt.com/docs/auth) can be file, keyring, automatic or ephemeral. `cas-cli/src/accounts.rs:118` shares AGENTS, agents, hooks, config, plugins, skills and memories across account homes, while current account handling also reads `auth.json`. A distinct home directory is not sufficient proof of effective authenticated account when keyring or workspace constraints apply. Test identity/credential storage and shared-trust effects in a separately approved isolated fixture; this static review establishes a compatibility question, not a reproduced account leak.

[Environment controls](https://learn.chatgpt.com/docs/config-file/environment-variables) separate `CODEX_HOME` from SQLite home overrides. The [shell environment policy](https://learn.chatgpt.com/docs/config-file/config-advanced#shell-environment-policy) applies filters and explicit sets in an order that can reintroduce excluded values; default secret exclusion must not be assumed. A future evidence harness should preserve required CAS authority markers while giving HOME/XDG/runtime/temp and Git state private roots and an explicit credential-free allowlist.

[Native memories](https://learn.chatgpt.com/docs/customization/memories) are optional, generated background context under Codex state. They do not replace Cassy's durable tasks/memory or authorize a second work owner. Manual compaction acknowledgement is distinct from a completed context-compaction item. Prefer documented item events over transcript-path scraping; a transcript path is not a stable API. Defer automatic/speculative compaction enablement and native goal continuation until cancellation, authority and context preservation are demonstrated. No memory/compaction performance benefit was measured here.

[Codex-managed worktrees](https://learn.chatgpt.com/docs/environments/git-worktrees) have their own storage/cleanup rules, and ignored-file copying is explicitly scoped to desktop-managed worktrees. Preserve Cassy's branch/worktree/scratch ownership; do not import desktop cleanup or secret-copy behavior into factory worktrees. New models/providers, Daybreak eligibility, cloud/plugin workflows and UI controls remain optional context rather than reasons to change account or execution policy.

## Adopt / test / defer matrix

“Adopt” approves a structure for an existing owned implementation task; it does not claim that this report installed it. Every expected benefit below is a hypothesis unless an executed observation is explicitly cited above.

| Decision and owner / seam | Expected benefit and existing evidence | Required proof / compatibility cost |
| --- | --- | --- |
| **Adopt direction — d0fe:** synchronous SessionStart/Stop adapters; `codex_trust.rs`, `config_gen.rs`. | Native context/continuation demonstrated by peer canaries. | Exact trust identity, explicit disabled/managed policy, bounded re-entry, install/review controls; keep existing user definitions. MCP execution remains unproven. |
| **Adopt direction — 6c54:** contextual capability records and independent-review fallback; `harness_policy.rs`, backend. | Avoid static false bits suppressing review; source mismatch demonstrated. | Preserve requested overrides, V1/V2 conflict, unknown model/tool state; no fabricated availability from one feature flag. |
| **Adopt direction — 6c54:** canonical role body to native TOML; `builtins.rs`, sealed handoff handlers. | Avoid policy drift between catalogs. | Discovery and invocation fixture; preserve unowned files; native child must not inherit Cassy verifier authority without registration. |
| **Test — skill integration owner to assign:** canonical skill projection and deprecated-prompt migration. | Supported native discovery; finish skill already reconciled by 558a. | Ancestor/user/legacy roots, duplicate names and explicit/implicit invocation; no blind move or user-file overwrite. |
| **Test — 353b:** UUID queue/admission adapter; queue/turn evidence handlers. | Potentially reduce PTY submission ambiguity; docs/release fixes only. | Idle/busy/reconnect/cancel, duplicate control, actual turn and model consumption; CAS receipt ownership retained. |
| **Test — transport owner to assign:** private app-server event pilot; backend/mux boundary. | Typed thread/item/turn events could improve attribution. | Versioned schema/error handling, reply identity, disconnect, compaction and PTY fallback; production-support warning prevents immediate replacement. |
| **Test — d0fe/policy owner:** MCP execution and failure controls; tool hook ingress. | Matching already exercised before approval. | Actual allowed/denied execution, malformed output/timeout/unavailable handler, hosted/CodeMode opt-outs; hook fail-open cannot become sole authority. |
| **Test — conformance owner:** background, interruption, output, cold resume. | Upstream fixes address real transport failure classes. | Root-approved full conformance at exact version/launch policy; reuse partial receipts, preserve originating permission and process ownership. |
| **Test — account owner:** auth/profile/state-home evidence; `accounts.rs`, PTY launch. | Detect identity/config sharing ambiguity before rollout. | File/keyring/ephemeral and forced-workspace controls, explicit profiles, credential-free fixture; source concern is not observed leakage. |
| **Test — context owner:** compaction item events and retained output. | Could avoid transcript heuristics and stale context. | Completion versus acceptance, cancellation, model-visible context; no auto-feature enablement or timing claim. |
| **Defer:** native memory as an authoritative store; speculative/async compaction enablement. | Optional native context may be useful later. | Separate CAS ownership, settings, cost and retention; no current benefit measurement. |
| **Defer:** native goal/worktree orchestration and wholesale plugin/SDK migration. | Adds another owner before a demonstrated need. | One CAS task/start/close owner; retain factory branches, scratch and process lifecycle. SDK wrapper alone does not solve authority. |
| **Defer:** forced model/provider/Daybreak changes; Windows/voice/desktop-only policy. | Release availability is conditional and platform-specific. | Preserve explicit operator model/account/settings; no inherent model comparison or new 3.63 gate. |

## Coverage ledger and omissions

The complete machine ledger inventories all 199 index entries with reviewed/fetched/unfetched status and reasons. Raw pages, metadata, release bodies, tag resolution, selected stable source and PR-title inventory remain under task artifact `cas-9ca1/`; they are not copied into product configuration.

| Official page group / fetched count | Read scope and explicit omissions |
| --- | --- |
| Hooks, subagents, skills / 3 | Discovery, trust, lifecycle, inheritance and invocation; not every handler/authoring/model-choice example. |
| App-server, developer commands, SDK, removed MCP-server / 4 | Selected lifecycle/config/turn/queue/experimental APIs; full command/auth/app-settings catalog not read. SDK/MCP-server notices read; no execution. |
| Config basic/advanced/reference/environment / 4 | Layering, profiles, feature/role controls, state and shell environment; reference is targeted lookup, not a linear read. Telemetry/provider/UI examples not exhaustively read. |
| AGENTS, rules, managed configuration / 3 | Instruction discovery, command rule precedence and selected managed controls; enterprise deployment and all example trees not read. |
| Auth, MCP, exec / 3 | Storage/identity, server/tool policy, noninteractive events/resume; sign-in/OAuth and CI recipes not exhaustively read. |
| Memories, permissions, security, worktrees, maturity / 5 | Relevant state/ownership/maturity boundaries; permission/network/OS and desktop walkthroughs only scoped reads. |
| Deprecated custom prompts / 1 | Guide read; reuse existing finish-skill ownership. |
| CLI overview, skills-and-plugins / 2 | Fetched/indexed only, not substantively reviewed; no adoption conclusion drawn from these pages. |

Out-of-scope or unresolved content includes the rest of the index (administration, enterprise/cloud/IDE/desktop workflows and adjacent feature pages), full upstream PR diffs, beta platform enforcement matrices, hosted-tool coverage, actual MCP execution, full 0.163 PTY conformance, queue/app-server runtime delivery and model-dependent tool availability. Initial obsolete upstream feature and unversioned schema paths returned 404; the correct pinned feature crate and v2 schema paths were retrieved. No final finding relies on that failed path.

## Confidence, counter-evidence and implications

**High confidence:** published stable/tag provenance; the seven-release inventory; supported documented role/hook/config shapes; pinned V2 precedence; peer canary observations within their exact controls. **Moderate confidence:** which adapter will improve Cassy's delivery reliability, because no end-to-end transport comparison was run. **Unknown:** full effective native tools/roles for every model/profile, complete hook enforcement, account isolation across credential backends and any performance gain.

The strongest case against native adoption is support and authority mismatch: app-server support is limited, hook execution can fail open, roles inherit constraints and do not register independent reviewers, and current Cassy conformance is older. These are reasons for small reversible adapters with independent receipts. A supported native catalog/availability API or a production-support commitment could change the transport recommendation; failed real denial/continuation tests would narrow the lifecycle recommendation.

Send this matrix to cas-d0fe, cas-6c54 and cas-558a; pair it with cas-f1ca's Claude review. The common structure should keep one canonical Cassy role/policy source, harness-native projections, contextual evidence records and a transport adapter that preserves existing task/receipt ownership. Proposed tests without an assigned owner remain proposals, not newly authorized work. No runtime setting, verifier skip, trust bypass, model change or release gate follows automatically from this report.

## Provenance and reproduction

Author: quick-crow-35, task cas-9ca1. Analysis baseline: `5e4774024045f19adaddf8da0e7ac1e69d19a9ad`. Stable upstream baseline: `83086188ae0614bd882818ff1aa23f3194962012`. Source Markdown: `docs/analysis/2026-10-17-cas-9ca1-codex-integration-research.md`; HTML is generated from this complete source with inline house tokens and no network dependency.

Artifact directory: `/home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-9ca1/`. `coverage-ledger.json`, `sources/docs-fetch-ledger.json`, `sources/release-pagination.json`, `sources/release-pr-scope-inventory.json` and per-file HTTP metadata record scope and retrieval. Initial official fetches began 21:15Z on 2026-10-17; individual timestamps are authoritative. `build-coverage.py` inventories the index and annotates read scope; `render-report.mjs` generates the offline review surface.

Read-only discovery commands included:

```sh
exa-search --domains developers.openai.com,learn.chatgpt.com --raw 'Codex official documentation llms.txt'
exa-search --contents https://developers.openai.com/codex/llms.txt --raw
```

Public HTTP GETs recorded in the fetch scripts/metadata included:

```text
https://api.github.com/repos/openai/codex/releases/latest
https://api.github.com/repos/openai/codex/releases?per_page=50&page=1
https://api.github.com/repos/openai/codex/releases?per_page=50&page=2
https://api.github.com/repos/openai/codex/git/ref/tags/rust-v0.163.0
https://learn.chatgpt.com/docs/<slug>.md
https://raw.githubusercontent.com/openai/codex/83086188ae0614bd882818ff1aa23f3194962012/<path>
```

Local inspection used `rg` and bounded `sed` reads against the named baseline paths. Peer evidence was read from immutable files in cas-d0fe, cas-6c54 and cas-558a; no canary, model session, Rust check, production migration or live profile probe was repeated. Report verification consists of document lint, complete-source rendering, strict visual QA in light/dark desktop/phone, JavaScript-disabled content and print review; it does not certify runtime adapters.
